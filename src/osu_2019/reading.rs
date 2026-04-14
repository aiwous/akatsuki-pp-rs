//! Reading evaluator — port of kwotaq's ReadingEvaluator.cs
//! Evaluates per-object reading difficulty based on note density,
//! hidden mod invisibility, preempt (AR) difficulty, and constant angle nerf.

/// Data stored per difficulty object for reading evaluation
#[derive(Clone, Debug)]
pub(crate) struct ReadingObject {
    pub(crate) time: f32,        // raw start time (not clock-rate adjusted)
    pub(crate) jump_dist: f32,   // lazy jump distance (normalized)
    pub(crate) strain_time: f32, // adjusted delta time (clock-rate adjusted)
    pub(crate) delta: f32,       // raw delta / clock_rate
    pub(crate) angle: Option<f32>,
    pub(crate) normalised_vector_angle: Option<f32>,
}

/// Map-wide constants needed for reading evaluation
pub(crate) struct ReadingParams {
    /// Raw preempt time in ms (before clock rate adjustment)
    pub(crate) time_preempt_raw: f32,
    /// Clock-rate adjusted preempt = time_preempt_raw / clock_rate
    pub(crate) preempt: f32,
    /// Fade-in duration (raw) = 400 * min(1, time_preempt_raw / 450)
    pub(crate) time_fade_in_raw: f32,
    /// HD TimeFadeIn = time_preempt_raw * 0.4
    pub(crate) hd_fade_in_raw: f32,
    /// Clock rate
    pub(crate) clock_rate: f32,
    /// Whether hidden mod is active
    pub(crate) hidden: bool,
}

// Constants from ReadingEvaluator.cs
const READING_WINDOW_SIZE: f32 = 3000.0;
const NORMALISED_RADIUS: f32 = 52.0;
const NORMALISED_DIAMETER: f32 = NORMALISED_RADIUS * 2.0;
const DISTANCE_INFLUENCE_THRESHOLD: f32 = NORMALISED_DIAMETER * 1.5;
const HIDDEN_MULTIPLIER: f32 = 0.28;
const DENSITY_MULTIPLIER: f32 = 2.4;
const DENSITY_DIFFICULTY_BASE: f32 = 2.5;
const PREEMPT_BALANCING_FACTOR: f32 = 140_000.0;
const PREEMPT_STARTING_POINT: f32 = 500.0; // AR 9.66 in ms
const MINIMUM_ANGLE_RELEVANCY_TIME: f32 = 2000.0;
const MAXIMUM_ANGLE_RELEVANCY_TIME: f32 = 200.0;

// HD constants
const HD_FADE_OUT_DURATION_MULTIPLIER: f32 = 0.3;

/// Evaluate reading difficulty for object at `index` in the list.
pub(crate) fn evaluate_difficulty_of(
    objects: &[ReadingObject],
    index: usize,
    params: &ReadingParams,
) -> f32 {
    if index == 0 {
        return 0.0;
    }

    let curr = &objects[index];
    let velocity = 1.0_f32.max(curr.jump_dist / curr.strain_time);

    let current_visible_density = retrieve_current_visible_object_density(objects, index, params);
    let past_influence = get_past_object_difficulty_influence(objects, index, params);
    let constant_angle_nerf = get_constant_angle_nerf_factor(objects, index);

    // Next object for future density influence
    let next = if index + 1 < objects.len() {
        Some(&objects[index + 1])
    } else {
        None
    };

    let density_difficulty = calculate_density_difficulty(
        next,
        velocity,
        constant_angle_nerf,
        past_influence,
        current_visible_density,
    );

    let hidden_difficulty = if params.hidden {
        calculate_hidden_difficulty(objects, index, params, past_influence, current_visible_density, velocity, constant_angle_nerf)
    } else {
        0.0
    };

    let preempt_difficulty =
        calculate_preempt_difficulty(velocity, constant_angle_nerf, params.preempt);

    // Norm(1.5, preempt, hidden, density)
    norm(1.5, &[preempt_difficulty, hidden_difficulty, density_difficulty])
}

// --- Sub-calculations ---

fn calculate_density_difficulty(
    next: Option<&ReadingObject>,
    velocity: f32,
    constant_angle_nerf: f32,
    past_influence: f32,
    current_visible_density: f32,
) -> f32 {
    // Consider future densities — makes cursor path less clear
    let mut future_influence = current_visible_density.sqrt();

    if let Some(next_obj) = next {
        // Reduce difficulty if movement to next object is small
        future_influence *= smootherstep(next_obj.jump_dist, 15.0, DISTANCE_INFLUENCE_THRESHOLD);
    }

    // Value higher note densities exponentially
    let mut density_difficulty =
        (past_influence + future_influence).powf(1.7) * 0.4 * constant_angle_nerf * velocity;

    // Award only denser-than-average maps
    density_difficulty = (density_difficulty - DENSITY_DIFFICULTY_BASE).max(0.0);

    // Soft cap for partial memorization
    density_difficulty = density_difficulty.powf(0.45) * DENSITY_MULTIPLIER;

    density_difficulty
}

fn calculate_preempt_difficulty(velocity: f32, constant_angle_nerf: f32, preempt: f32) -> f32 {
    // Arbitrary curve for preempt difficulty as AR increases
    // https://www.desmos.com/calculator/c175335a71
    let diff = PREEMPT_STARTING_POINT - preempt;
    let half_rect = (diff + diff.abs()) / 2.0; // max(0, diff)
    let preempt_difficulty = half_rect.powf(2.5) / PREEMPT_BALANCING_FACTOR;

    preempt_difficulty * constant_angle_nerf * velocity
}

fn calculate_hidden_difficulty(
    objects: &[ReadingObject],
    index: usize,
    params: &ReadingParams,
    past_influence: f32,
    current_visible_density: f32,
    velocity: f32,
    constant_angle_nerf: f32,
) -> f32 {
    // Duration spent invisible (clock-rate adjusted)
    // = (hd_fade_in + time_preempt_raw * 0.3) / clock_rate
    let time_spent_invisible =
        (params.hd_fade_in_raw + params.time_preempt_raw * HD_FADE_OUT_DURATION_MULTIPLIER)
            / params.clock_rate;

    // Value time spent invisible exponentially
    let time_invisible_factor = time_spent_invisible.powf(2.2) * 0.022;

    // Account for both past and current densities
    let density_factor = (current_visible_density + past_influence).powf(3.3) * 3.0;

    let mut hidden_difficulty =
        (time_invisible_factor + density_factor) * constant_angle_nerf * velocity * 0.01;

    // Soft cap for partial memorization
    hidden_difficulty = hidden_difficulty.powf(0.4) * HIDDEN_MULTIPLIER;

    // Buff perfect stacks if current note is completely invisible when clicking previous note
    if index > 0 {
        let curr = &objects[index];
        let prev = &objects[index - 1];

        if curr.jump_dist == 0.0 {
            // Check: is curr invisible at the time prev needs to be clicked?
            // prev click time = prev.time, curr opacity at prev.time
            let prev_click_with_preempt = prev.time + params.time_preempt_raw;
            let curr_opacity = opacity_at(curr.time, prev_click_with_preempt, params, true);

            if curr_opacity == 0.0 && prev_click_with_preempt > curr.time {
                // Perfect stacks harder the less time between notes
                hidden_difficulty += HIDDEN_MULTIPLIER * 7500.0
                    / curr.strain_time.powf(1.5);
            }
        }
    }

    hidden_difficulty
}

fn get_past_object_difficulty_influence(
    objects: &[ReadingObject],
    index: usize,
    params: &ReadingParams,
) -> f32 {
    let curr = &objects[index];
    let mut influence = 0.0_f32;

    // Iterate backwards through past visible objects
    for i in (0..index).rev() {
        let loop_obj = &objects[i];

        let time_gap = curr.time - loop_obj.time;

        // Stop conditions
        if time_gap > READING_WINDOW_SIZE {
            break;
        }
        // Current object not visible at the time loop_obj needs to be clicked
        if loop_obj.time + params.time_preempt_raw < curr.time {
            break;
        }

        // Opacity of current object at loop_obj's start time (non-hidden)
        let loop_difficulty = opacity_at(curr.time, loop_obj.time, params, false);

        // Small distances mean previous objects may be cheesed
        let dist_factor =
            smootherstep(loop_obj.jump_dist, 15.0, DISTANCE_INFLUENCE_THRESHOLD);

        // Less influence for objects far in time
        let time_nerf = get_time_nerf_factor(time_gap);

        influence += loop_difficulty * dist_factor * time_nerf;
    }

    influence
}

fn retrieve_current_visible_object_density(
    objects: &[ReadingObject],
    index: usize,
    params: &ReadingParams,
) -> f32 {
    let curr = &objects[index];
    let mut density = 0.0_f32;

    // Iterate forward through future objects
    let mut i = index + 1;
    while i < objects.len() {
        let hit_obj = &objects[i];

        let time_gap = hit_obj.time - curr.time;

        // Stop conditions
        if time_gap > READING_WINDOW_SIZE {
            break;
        }
        // Object not visible at the time current needs to be clicked
        if curr.time + params.time_preempt_raw < hit_obj.time {
            break;
        }

        // Opacity of future object at current's click time (non-hidden)
        let opacity = opacity_at(hit_obj.time, curr.time, params, false);
        let time_nerf = get_time_nerf_factor(time_gap);

        density += opacity * time_nerf;

        i += 1;
    }

    density
}

/// Returns a nerf factor for constant angles (repeated patterns)
/// https://www.desmos.com/calculator/eb057a4822
fn get_constant_angle_nerf_factor(objects: &[ReadingObject], index: usize) -> f32 {
    let curr = &objects[index];
    let mut constant_angle_count = 0.0_f32;
    let mut current_time_gap = 0.0_f32;

    let mut i = 0_usize;

    while current_time_gap < MINIMUM_ANGLE_RELEVANCY_TIME {
        if i >= index {
            break;
        }

        let loop_obj = &objects[index - 1 - i];

        // Less weight for objects close to the time limit
        let long_interval_factor = 1.0
            - reverse_lerp(
                loop_obj.strain_time,
                MAXIMUM_ANGLE_RELEVANCY_TIME,
                MINIMUM_ANGLE_RELEVANCY_TIME,
            );

        if let (Some(curr_angle), Some(loop_angle)) = (curr.angle, loop_obj.angle) {
            let angle_difference = (curr_angle - loop_angle).abs();
            let stack_factor =
                smootherstep(loop_obj.jump_dist, 0.0, NORMALISED_RADIUS);

            let clamped_angle = angle_difference * stack_factor;
            let clamped_angle = clamped_angle.min(30.0_f32.to_radians());
            constant_angle_count += (3.0 * clamped_angle).cos() * long_interval_factor;
        }

        current_time_gap = curr.time - loop_obj.time;
        i += 1;
    }

    (2.0 / constant_angle_count).clamp(0.2, 1.0)
}

// --- Helpers ---

/// Compute opacity of an object at a given time
/// `obj_time` = object's start time (raw)
/// `query_time` = time to compute opacity at (raw)
fn opacity_at(obj_time: f32, query_time: f32, params: &ReadingParams, hidden: bool) -> f32 {
    let fade_in_start = obj_time - params.time_preempt_raw;
    let fade_in_duration = params.time_fade_in_raw;

    if hidden {
        let fade_out_start = obj_time - params.time_preempt_raw + fade_in_duration;
        let fade_out_duration = params.time_preempt_raw * HD_FADE_OUT_DURATION_MULTIPLIER;

        let fade_in = ((query_time - fade_in_start) / fade_in_duration).clamp(0.0, 1.0);
        let fade_out = ((query_time - fade_out_start) / fade_out_duration).clamp(0.0, 1.0);

        fade_in * (1.0 - fade_out)
    } else {
        ((query_time - fade_in_start) / fade_in_duration).clamp(0.0, 1.0)
    }
}

/// Nerf factor for distant objects in time
fn get_time_nerf_factor(delta_time: f32) -> f32 {
    (2.0 - delta_time / (READING_WINDOW_SIZE / 2.0)).clamp(0.0, 1.0)
}

/// Smootherstep function (5th-order)
fn smootherstep(x: f32, start: f32, end: f32) -> f32 {
    let x = ((x - start) / (end - start)).clamp(0.0, 1.0);
    x * x * x * (x * (6.0 * x - 15.0) + 10.0)
}

/// Reverse linear interpolation (clamped 0..1)
fn reverse_lerp(x: f32, start: f32, end: f32) -> f32 {
    ((x - start) / (end - start)).clamp(0.0, 1.0)
}

/// P-norm of a set of values
fn norm(p: f32, values: &[f32]) -> f32 {
    values
        .iter()
        .map(|v| v.powf(p))
        .sum::<f32>()
        .powf(1.0 / p)
}
