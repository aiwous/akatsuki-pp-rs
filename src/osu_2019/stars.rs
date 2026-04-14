//! The positional offset of notes created by stack leniency is not considered.
//! This means the jump distance inbetween notes might be slightly off, resulting in small inaccuracies.
//! Since calculating these offsets is relatively expensive though, this version is faster than `all_included`.

use super::{
    reading::{ReadingObject, ReadingParams},
    reading_skill::ReadingSkill,
    DifficultyObject, OsuObject, Skill, SkillKind,
};

use crate::{Beatmap, GameMods};

use rosu_map::section::hit_objects::CurveBuffers;

const OBJECT_RADIUS: f32 = 64.0;
const SECTION_LEN: f32 = 400.0;
const DIFFICULTY_MULTIPLIER: f32 = 0.0675;
const NORMALIZED_RADIUS: f32 = 52.0;

// AR -> preempt conversion constants (same as AR_WINDOWS in attributes.rs)
const AR_PREEMPT_MIN: f32 = 1800.0;
const AR_PREEMPT_MID: f32 = 1200.0;
const AR_PREEMPT_MAX: f32 = 450.0;

// PREEMPT_MIN from OsuHitObject (for fade-in calculation)
const PREEMPT_MIN: f32 = 450.0;

/// Star calculation for osu!standard maps.
///
/// Slider paths are considered but stack leniency is ignored.
/// As most maps don't even make use of leniency and even if,
/// it has generally little effect on stars, the results are close to perfect.
/// This version is considerably more efficient than `all_included` since
/// processing stack leniency is relatively expensive.
///
/// In case of a partial play, e.g. a fail, one can specify the amount of passed objects.
pub fn stars(
    map: &Beatmap,
    mods: GameMods,
    passed_objects: Option<u32>,
    clock_rate: Option<f64>,
) -> OsuDifficultyAttributes {
    let mut builder = map.attributes().mods(mods.clone());

    if let Some(clock_rate) = clock_rate {
        builder = builder.clock_rate(clock_rate);
    }

    let map_attributes = builder.build();

    let mut diff_attributes = OsuDifficultyAttributes {
        ar: map_attributes.ar,
        od: map_attributes.od,
        cs: map_attributes.cs,
        ..Default::default()
    };

    let take = passed_objects.unwrap_or(map.hit_objects.len() as u32) as usize;

    if take < 2 {
        return diff_attributes;
    }

    let section_len = SECTION_LEN * map_attributes.clock_rate as f32;
    let radius = OBJECT_RADIUS * (1.0 - 0.7 * (map_attributes.cs as f32 - 5.0) / 5.0) / 2.0;
    let mut scaling_factor = NORMALIZED_RADIUS / radius;

    if radius < 30.0 {
        let small_circle_bonus = (30.0 - radius).min(5.0) / 50.0;
        scaling_factor *= 1.0 + small_circle_bonus;
    }

    let mut ticks_buf = Vec::new();
    let mut curve_bufs = CurveBuffers::default();

    let mut hit_objects = map.hit_objects.iter().take(take).filter_map(|h| {
        Some(OsuObject::new(
            h,
            map,
            radius,
            scaling_factor,
            &mut ticks_buf,
            &mut diff_attributes,
            &mut curve_bufs,
        ))
    });

    let mut aim = Skill::new(SkillKind::Aim);
    let mut speed = Skill::new(SkillKind::Speed);

    // Buffer reading objects for the second pass
    let mut reading_objects: Vec<ReadingObject> = Vec::with_capacity(take);

    // First object has no predecessor and thus no strain, handle distinctly
    let mut current_section_end =
        (map.hit_objects[0].start_time as f32 / section_len).ceil() * section_len;

    let mut prev_prev = None;
    let mut prev = hit_objects.next().unwrap();
    let mut prev_vals = None;

    // Store first object for reading
    reading_objects.push(ReadingObject {
        time: prev.time,
        jump_dist: 0.0,
        strain_time: 0.0,
        delta: 0.0,
        angle: None,
        normalised_vector_angle: None,
    });

    // Handle second object separately to remove later if-branching
    let curr = hit_objects.next().unwrap();
    let h = DifficultyObject::new(
        &curr,
        &prev,
        prev_vals,
        prev_prev,
        map_attributes.clock_rate as f32,
        scaling_factor,
    );

    while h.base.time as f32 > current_section_end {
        current_section_end += section_len;
    }

    aim.process(&h);
    speed.process(&h);

    // Store for reading
    reading_objects.push(ReadingObject {
        time: curr.time,
        jump_dist: h.jump_dist,
        strain_time: h.strain_time,
        delta: h.delta,
        angle: h.angle,
        normalised_vector_angle: h.normalised_vector_angle,
    });

    prev_prev = Some(prev);
    prev_vals = Some((h.jump_dist, h.strain_time));
    prev = curr;

    // Handle all other objects
    for curr in hit_objects {
        let h = DifficultyObject::new(
            &curr,
            &prev,
            prev_vals,
            prev_prev,
            map_attributes.clock_rate as f32,
            scaling_factor,
        );

        while h.base.time as f32 > current_section_end {
            aim.save_current_peak();
            aim.start_new_section_from(current_section_end);
            speed.save_current_peak();
            speed.start_new_section_from(current_section_end);

            current_section_end += section_len;
        }

        aim.process(&h);
        speed.process(&h);

        // Store for reading
        reading_objects.push(ReadingObject {
            time: curr.time,
            jump_dist: h.jump_dist,
            strain_time: h.strain_time,
            delta: h.delta,
            angle: h.angle,
            normalised_vector_angle: h.normalised_vector_angle,
        });

        prev_prev = Some(prev);
        prev_vals = Some((h.jump_dist, h.strain_time));
        prev = curr;
    }

    aim.save_current_peak();
    speed.save_current_peak();

    let aim_strain = aim.difficulty_value().sqrt() * DIFFICULTY_MULTIPLIER;
    let speed_strain = speed.difficulty_value().sqrt() * DIFFICULTY_MULTIPLIER;

    let aim_difficult_strain_count = aim.count_difficult_strains();
    let speed_difficult_strain_count = speed.count_difficult_strains();

    // --- Reading pass ---
    // Compute AR preempt values
    // map_attributes.hit_windows.ar IS the clock-rate-adjusted preempt.
    // Raw preempt = hit_windows.ar * clock_rate
    let time_preempt_raw = map_attributes.hit_windows.ar as f32 * map_attributes.clock_rate as f32;
    let preempt = map_attributes.hit_windows.ar as f32; // already clock-rate adjusted
    let time_fade_in_raw = 400.0 * (time_preempt_raw / PREEMPT_MIN).min(1.0);
    let hd_fade_in_raw = time_preempt_raw * 0.4; // HD TimeFadeIn

    let reading_params = ReadingParams {
        time_preempt_raw,
        preempt,
        time_fade_in_raw,
        hd_fade_in_raw,
        clock_rate: map_attributes.clock_rate as f32,
        hidden: mods.hd(),
    };

    let mut reading_skill = ReadingSkill::new();
    reading_skill.process_all(&reading_objects, &reading_params);

    let first_object_time = if !reading_objects.is_empty() {
        reading_objects[0].time
    } else {
        0.0
    };

    let reading_difficulty_value =
        reading_skill.difficulty_value(first_object_time, map_attributes.clock_rate as f32);
    let reading_strain = reading_difficulty_value.sqrt() * DIFFICULTY_MULTIPLIER;
    let reading_difficult_note_count =
        reading_skill.count_top_weighted_object_difficulties(reading_difficulty_value);

    // Star rating: aim + speed + reading + largest component bonus
    let components = [aim_strain, speed_strain, reading_strain];
    let sum: f32 = components.iter().sum();
    let max_component = components.iter().cloned().fold(0.0_f32, f32::max);
    // The original formula was: aim + speed + |aim - speed| / 2
    // Which equals: max(aim, speed) + min(aim, speed) / 2 + max(aim, speed) / 2
    // = 1.5 * max + 0.5 * min. Generalizing to 3 components:
    // sum + max_component_bonus (extra weight to the hardest component)
    let stars = sum + (max_component - sum / components.len() as f32).abs() / 2.0;

    diff_attributes.stars = stars as f64;
    diff_attributes.speed_strain = speed_strain as f64;
    diff_attributes.aim_strain = aim_strain as f64;
    diff_attributes.reading_strain = reading_strain as f64;
    diff_attributes.aim_difficult_strain_count = aim_difficult_strain_count;
    diff_attributes.speed_difficult_strain_count = speed_difficult_strain_count;
    diff_attributes.reading_difficult_note_count = reading_difficult_note_count;

    diff_attributes
}

/// Convert AR value to preempt time in ms (raw, not clock-rate adjusted)
#[allow(dead_code)]
fn ar_to_preempt(ar: f32) -> f32 {
    if ar > 5.0 {
        AR_PREEMPT_MID + (AR_PREEMPT_MAX - AR_PREEMPT_MID) * (ar - 5.0) / 5.0
    } else if ar < 5.0 {
        AR_PREEMPT_MID - (AR_PREEMPT_MID - AR_PREEMPT_MIN) * (5.0 - ar) / 5.0
    } else {
        AR_PREEMPT_MID
    }
}

#[derive(Clone, Debug, Default)]
pub struct OsuDifficultyAttributes {
    pub aim_strain: f64,
    pub speed_strain: f64,
    pub reading_strain: f64,
    pub ar: f64,
    pub od: f64,
    pub hp: f64,
    pub cs: f64,
    pub n_circles: usize,
    pub n_sliders: usize,
    pub n_spinners: usize,
    pub stars: f64,
    pub max_combo: usize,
    pub aim_difficult_strain_count: f32,
    pub speed_difficult_strain_count: f32,
    pub reading_difficult_note_count: f32,
}

#[derive(Clone, Debug)]
pub struct OsuPerformanceAttributes {
    pub difficulty: OsuDifficultyAttributes,
    pub pp: f64,
    pub pp_acc: f64,
    pub pp_aim: f64,
    pub pp_speed: f64,
    pub pp_reading: f64,
    pub effective_miss_count: f64,
}
