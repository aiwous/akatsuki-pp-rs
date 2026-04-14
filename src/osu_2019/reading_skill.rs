//! Reading skill — port of kwotaq's Reading.cs (HarmonicSkill-based)
//! Aggregates per-object reading difficulty into a final strain value
//! using harmonic weighted summation instead of section-peak decay.

use super::reading::{self, ReadingObject, ReadingParams};

const SKILL_MULTIPLIER: f32 = 2.5;
const STRAIN_DECAY_BASE: f32 = 0.8;

// HarmonicSkill defaults
const HARMONIC_SCALE: f32 = 1.0;
const DECAY_EXPONENT: f32 = 0.9;

// Memorization reduction — first 60 seconds of objects have reduced difficulty
const REDUCED_DIFFICULTY_DURATION: f32 = 60.0 * 1000.0;
const REDUCED_DIFFICULTY_BASE_LINE: f32 = 0.0; // Assume first seconds completely memorised

/// Logistic function: maxValue / (1 + exp(multiplier * (midpointOffset - x)))
fn logistic(x: f32, midpoint_offset: f32, multiplier: f32, max_value: f32) -> f32 {
    max_value / (1.0 + (multiplier * (midpoint_offset - x)).exp())
}

pub(crate) struct ReadingSkill {
    object_difficulties: Vec<f32>,
    current_difficulty: f32,
    note_weight_sum: f32,
}

impl ReadingSkill {
    pub(crate) fn new() -> Self {
        Self {
            object_difficulties: Vec::new(),
            current_difficulty: 0.0,
            note_weight_sum: 0.0,
        }
    }

    /// Process all objects and compute per-object reading difficulties
    pub(crate) fn process_all(
        &mut self,
        objects: &[ReadingObject],
        params: &ReadingParams,
    ) {
        self.object_difficulties.clear();
        self.current_difficulty = 0.0;

        for i in 0..objects.len() {
            // Strain decay using delta time
            if i > 0 {
                let delta = objects[i].delta;
                self.current_difficulty *= strain_decay(delta);
            }

            let reading_value = reading::evaluate_difficulty_of(objects, i, params);
            self.current_difficulty += reading_value * SKILL_MULTIPLIER;

            self.object_difficulties.push(self.current_difficulty);
        }
    }

    /// Compute the final reading difficulty value using harmonic weighted sum.
    /// This is fundamentally different from the section-peak approach used by aim/speed.
    pub(crate) fn difficulty_value(&mut self, first_object_time: f32, clock_rate: f32) -> f32 {
        if self.object_difficulties.is_empty() {
            return 0.0;
        }

        // Filter out zero-difficulty objects
        let mut difficulties: Vec<f32> = self
            .object_difficulties
            .iter()
            .copied()
            .filter(|&d| d > 0.0)
            .collect();

        if difficulties.is_empty() {
            return 0.0;
        }

        // Apply memorization reduction for first ~60 seconds
        self.apply_difficulty_transformation(
            &mut difficulties,
            first_object_time,
            clock_rate,
        );

        // Sort descending
        difficulties.sort_unstable_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        // Harmonic weighted sum
        let mut difficulty = 0.0_f32;
        self.note_weight_sum = 0.0;

        for (index, &note) in difficulties.iter().enumerate() {
            let idx = index as f32;
            let harmonic_term = HARMONIC_SCALE / (1.0 + idx);
            let weight = (1.0 + harmonic_term) / (idx.powf(DECAY_EXPONENT) + 1.0 + harmonic_term);

            self.note_weight_sum += weight;
            difficulty += note * weight;
        }

        difficulty
    }

    /// Reduce difficulty for the first ~60 seconds of objects (memorization)
    fn apply_difficulty_transformation(
        &self,
        difficulties: &mut [f32],
        first_object_time: f32,
        clock_rate: f32,
    ) {
        if difficulties.is_empty() {
            return;
        }

        let reduced_note_count =
            self.calculate_reduced_note_count(first_object_time, clock_rate);

        if reduced_note_count == 0 {
            return;
        }

        let count = difficulties.len().min(reduced_note_count);

        for i in 0..count {
            let t = (i as f32 / reduced_note_count as f32).clamp(0.0, 1.0);
            // Lerp(1, 10, t) then log10 to get a smooth ramp from 0 to 1
            let lerped = 1.0 + 9.0 * t; // lerp(1, 10, t)
            let scale = lerped.log10();
            // lerp(0, 1, scale) = scale (since baseline is 0)
            difficulties[i] *= REDUCED_DIFFICULTY_BASE_LINE + (1.0 - REDUCED_DIFFICULTY_BASE_LINE) * scale;
        }
    }

    /// Number of objects in the first ~60 seconds
    fn calculate_reduced_note_count(
        &self,
        first_object_time: f32,
        clock_rate: f32,
    ) -> usize {
        // We count how many objects fall within the first 60 seconds
        // This mirrors the upstream which uses the 2nd note and objectList
        let _reduced_duration = (first_object_time / clock_rate) + REDUCED_DIFFICULTY_DURATION;

        self.object_difficulties.len().min(
            // Simple heuristic: we don't have the original object list here,
            // so we use the object count directly
            // The upstream counts objects whose startTime/clockRate <= reducedDuration
            // Since we don't store times in difficulties, we pass this count externally
            // For now, return all objects (will be refined when integrated with stars.rs)
            self.object_difficulties.len(),
        )
    }

    /// Compute the reading strain rating from difficulty value
    /// Uses sqrt * difficulty_multiplier same as aim/speed
    #[allow(dead_code)]
    pub(crate) fn reading_rating(&self, difficulty_value: f32) -> f32 {
        difficulty_value.sqrt() * 0.0675
    }

    /// Count of difficult notes, weighted by logistic function against top difficulty
    pub(crate) fn count_top_weighted_object_difficulties(&self, difficulty_value: f32) -> f32 {
        if self.object_difficulties.is_empty() || self.note_weight_sum == 0.0 {
            return 0.0;
        }

        let consistent_top = difficulty_value / self.note_weight_sum;

        if consistent_top == 0.0 {
            return 0.0;
        }

        self.object_difficulties
            .iter()
            .map(|&d| logistic(d / consistent_top, 1.15, 5.0, 1.1))
            .sum::<f32>()
    }

    /// DifficultyToPerformance for HarmonicSkill: 4 * d^3
    /// (Used in upstream for converting reading rating to base performance)
    #[allow(dead_code)]
    pub(crate) fn difficulty_to_performance(difficulty: f32) -> f32 {
        4.0 * difficulty.powi(3)
    }
}

fn strain_decay(ms: f32) -> f32 {
    STRAIN_DECAY_BASE.powf(ms / 1000.0)
}
