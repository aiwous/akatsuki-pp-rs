use super::DifficultyObject;

const SINGLE_SPACING_TRESHOLD: f32 = 125.0;
const SPEED_ANGLE_BONUS_BEGIN: f32 = 5.0 * std::f32::consts::FRAC_PI_6;
const PI_OVER_4: f32 = std::f32::consts::FRAC_PI_4;
const PI_OVER_2: f32 = std::f32::consts::FRAC_PI_2;

const MIN_SPEED_BONUS: f32 = 75.0;
const MAX_SPEED_BONUS: f32 = 45.0;
const SPEED_BALANCING_FACTOR: f32 = 40.0;

const AIM_ANGLE_BONUS_BEGIN: f32 = std::f32::consts::FRAC_PI_3;
const TIMING_THRESHOLD: f32 = 107.0;

// Angle repetition nerf constants (from kwotaq commit f3d97c08)
const MAXIMUM_REPETITION_NERF: f32 = 0.08;
const REPETITION_THRESHOLD: f32 = 1.3;
const MAXIMUM_VECTOR_INFLUENCE: f32 = 1.0;
const NORMALISED_DIAMETER: f32 = 52.0 * 2.0; // NORMALISED_RADIUS * 2
const NOTE_LIMIT: usize = 6;

/// Data stored per recent object for angle repetition lookback
#[derive(Copy, Clone, Default)]
#[allow(dead_code)]
pub(crate) struct RecentObject {
    pub(crate) normalised_vector_angle: Option<f32>,
    pub(crate) strain_time: f32,
    pub(crate) jump_dist: f32,
    pub(crate) angle: Option<f32>,
}

#[derive(Copy, Clone)]
pub(crate) enum SkillKind {
    Aim,
    Speed,
}

impl SkillKind {
    pub(crate) fn strain_value_of(
        self,
        current: &DifficultyObject<'_>,
        recent_objects: &[RecentObject],
    ) -> f32 {
        match self {
            Self::Aim => {
                if current.base.is_spinner() {
                    return 0.0;
                }

                let mut result = 0.0;

                if let Some((prev_jump_dist, prev_strain_time)) = current.prev {
                    if let Some(angle) = current.angle.filter(|a| *a > AIM_ANGLE_BONUS_BEGIN) {
                        let scale = 90.0;

                        let angle_bonus = (((angle - AIM_ANGLE_BONUS_BEGIN).sin()).powi(2)
                            * (prev_jump_dist - scale).max(0.0)
                            * (current.jump_dist - scale).max(0.0))
                        .sqrt();

                        result = 1.5 * apply_diminishing_exp(angle_bonus.max(0.0))
                            / (TIMING_THRESHOLD).max(prev_strain_time)
                    }
                }

                let jump_dist_exp = apply_diminishing_exp(current.jump_dist);
                let travel_dist_exp = apply_diminishing_exp(current.travel_dist);

                let dist_exp =
                    jump_dist_exp + travel_dist_exp + (travel_dist_exp * jump_dist_exp).sqrt();

                let mut aim_strain = (result
                    + dist_exp / (current.strain_time).max(TIMING_THRESHOLD))
                .max(dist_exp / current.strain_time);

                // Penalize angle repetition (kwotaq angle repetition nerf)
                aim_strain *= vector_angle_repetition(current, recent_objects);

                aim_strain
            }
            Self::Speed => {
                if current.base.is_spinner() {
                    return 0.0;
                }

                let dist = SINGLE_SPACING_TRESHOLD.min(current.travel_dist + current.jump_dist);
                let delta_time = MAX_SPEED_BONUS.max(current.delta);

                let mut speed_bonus = 1.0;

                if delta_time < MIN_SPEED_BONUS {
                    let exp_base = (MIN_SPEED_BONUS - delta_time) / SPEED_BALANCING_FACTOR;
                    speed_bonus += exp_base * exp_base;
                }

                let mut angle_bonus = 1.0;

                if let Some(angle) = current.angle.filter(|a| *a < SPEED_ANGLE_BONUS_BEGIN) {
                    let exp_base = (1.5 * (SPEED_ANGLE_BONUS_BEGIN - angle)).sin();
                    angle_bonus = 1.0 + exp_base * exp_base / 3.57;

                    if angle < PI_OVER_2 {
                        angle_bonus = 1.28;

                        if dist < 90.0 && angle < PI_OVER_4 {
                            angle_bonus += (1.0 - angle_bonus) * ((90.0 - dist) / 10.0).min(1.0);
                        } else if dist < 90.0 {
                            angle_bonus += (1.0 - angle_bonus)
                                * ((90.0 - dist) / 10.0).min(1.0)
                                * ((PI_OVER_2 - angle) / PI_OVER_4).sin();
                        }
                    }
                }

                (1.0 + (speed_bonus - 1.0) * 0.75)
                    * angle_bonus
                    * (0.95 + speed_bonus * (dist / SINGLE_SPACING_TRESHOLD).powf(3.5))
                    / current.strain_time
            }
        }
    }
}

/// Penalize repetitive angle patterns (N, X, V patterns) while keeping
/// rotating patterns (1-2s, triangles) less nerfed.
/// Port of kwotaq's `vectorAngleRepetition` from commit f3d97c08.
fn vector_angle_repetition(current: &DifficultyObject<'_>, recent_objects: &[RecentObject]) -> f32 {
    // Need both current and previous angles
    let (curr_angle, curr_nva) = match (current.angle, current.normalised_vector_angle) {
        (Some(a), Some(nva)) => (a, nva),
        _ => return 1.0,
    };

    // Need at least one previous object with an angle
    if recent_objects.is_empty() {
        return 1.0;
    }

    let prev = &recent_objects[0]; // most recent previous object
    if prev.angle.is_none() {
        return 1.0;
    }
    let last_angle = prev.angle.unwrap();

    // Count how many recent objects have similar vector angles
    let mut constant_angle_count = 0.0_f32;
    let limit = NOTE_LIMIT.min(recent_objects.len());

    for index in 0..limit {
        let loop_obj = &recent_objects[index];

        // Only consider vectors in the same jump section — stopping to change rhythm ruins momentum
        let max_dt = current.strain_time.max(loop_obj.strain_time);
        let min_dt = current.strain_time.min(loop_obj.strain_time);
        if max_dt > 1.1 * min_dt {
            break;
        }

        if let (Some(loop_nva), Some(_curr_nva)) =
            (loop_obj.normalised_vector_angle, Some(curr_nva))
        {
            let angle_difference = (curr_nva - loop_nva).abs();
            // cos(8 * min(11.25°, angleDiff)) — peaks at 0 difference, goes to -1 at 11.25°
            let max_angle = 11.25_f32.to_radians();
            constant_angle_count += (8.0 * angle_difference.min(max_angle)).cos();
        }
    }

    let vector_repetition = (REPETITION_THRESHOLD / constant_angle_count.max(0.0))
        .min(1.0)
        .powi(2);

    // Stack factor: reduce nerf for stacked notes
    let stack_factor = smootherstep(current.jump_dist, 0.0, NORMALISED_DIAMETER);

    // Angle difference between current and previous, adjusted for stacks
    let angle_diff_adjusted =
        (2.0 * ((curr_angle - last_angle).abs() * stack_factor).min(45.0_f32.to_radians())).cos();

    // CalcAcuteAngleBonus(lastAngle) — smoothstep from 140° to 40°
    let acute_bonus = calc_acute_angle_bonus(last_angle);

    let base_nerf = 1.0 - MAXIMUM_REPETITION_NERF * acute_bonus * angle_diff_adjusted;

    (base_nerf + (1.0 - base_nerf) * vector_repetition * MAXIMUM_VECTOR_INFLUENCE * stack_factor)
        .powi(2)
}

/// Smoothstep function: 0 at start, 1 at end
fn smoothstep(x: f32, start: f32, end: f32) -> f32 {
    let x = ((x - start) / (end - start)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Smootherstep function (5th-order)
fn smootherstep(x: f32, start: f32, end: f32) -> f32 {
    let x = ((x - start) / (end - start)).clamp(0.0, 1.0);
    x * x * x * (x * (6.0 * x - 15.0) + 10.0)
}

/// CalcAcuteAngleBonus — smoothstep from 140° (=0) to 40° (=1)
fn calc_acute_angle_bonus(angle: f32) -> f32 {
    smoothstep(angle, 140.0_f32.to_radians(), 40.0_f32.to_radians())
}

#[inline]
fn apply_diminishing_exp(val: f32) -> f32 {
    val.powf(0.99)
}
