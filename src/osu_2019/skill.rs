use super::skill_kind::RecentObject;
use super::{DifficultyObject, SkillKind};

use std::cmp::Ordering;

const SPEED_SKILL_MULTIPLIER: f32 = 1400.0;
const SPEED_STRAIN_DECAY_BASE: f32 = 0.3;

const AIM_SKILL_MULTIPLIER: f32 = 26.25;
const AIM_STRAIN_DECAY_BASE: f32 = 0.15;

const DECAY_WEIGHT: f32 = 0.9;

/// Maximum number of recent objects to keep for angle repetition lookback
const MAX_RECENT_OBJECTS: usize = 8;

pub(crate) struct Skill {
    current_strain: f32,
    current_section_peak: f32,

    kind: SkillKind,
    pub(crate) strain_peaks: Vec<f32>,

    prev_time: Option<f32>,
    pub(crate) object_strains: Vec<f32>,

    difficulty_value: Option<f32>,

    /// Ring buffer of recent objects for angle repetition lookback (newest first)
    recent_objects: Vec<RecentObject>,
}

impl Skill {
    #[inline]
    pub(crate) fn new(kind: SkillKind) -> Self {
        Self {
            current_strain: 1.0,
            current_section_peak: 1.0,

            kind,
            strain_peaks: Vec::with_capacity(128),

            prev_time: None,
            object_strains: Vec::new(),

            difficulty_value: None,

            recent_objects: Vec::with_capacity(MAX_RECENT_OBJECTS),
        }
    }

    #[inline]
    pub(crate) fn save_current_peak(&mut self) {
        self.strain_peaks.push(self.current_section_peak);
    }

    #[inline]
    pub(crate) fn start_new_section_from(&mut self, time: f32) {
        self.current_section_peak = self.peak_strain(time - self.prev_time.unwrap());
    }

    #[inline]
    pub(crate) fn process(&mut self, current: &DifficultyObject<'_>) {
        self.current_strain *= self.strain_decay(current.delta);
        self.current_strain +=
            self.kind.strain_value_of(current, &self.recent_objects) * self.skill_multiplier();

        self.object_strains.push(self.current_strain);

        self.current_section_peak = self.current_section_peak.max(self.current_strain);
        self.prev_time.replace(current.base.time);

        // Update recent objects buffer (newest first)
        let recent = RecentObject {
            normalised_vector_angle: current.normalised_vector_angle,
            normalised_pos: current.normalised_pos,
            strain_time: current.strain_time,
            jump_dist: current.jump_dist,
            angle: current.angle,
        };

        if self.recent_objects.len() >= MAX_RECENT_OBJECTS {
            self.recent_objects.pop();
        }
        self.recent_objects.insert(0, recent);
    }

    pub(crate) fn difficulty_value(&mut self) -> f32 {
        if self.strain_peaks.is_empty() {
            return 0.0;
        }

        // --- Backload Spike Penalty Logic ---
        // We identify maps that dump all their difficulty into the final moments.
        let n_sections = self.strain_peaks.len();
        let early_section_count = (n_sections as f32 * 0.85).floor() as usize;

        if n_sections > 5 && early_section_count > 0 {
            // 1. Calculate max strain of the first 85% (the "baseline")
            let early_max = self
                .strain_peaks
                .iter()
                .take(early_section_count)
                .cloned()
                .fold(0.0, f32::max);

            // 2. Identify and nerf spikes in the final 15%
            // By using early_max, maps with linear difficulty increases are preserved.
            let spike_threshold = (early_max * 1.35).max(1.0);

            for i in early_section_count..n_sections {
                let peak = self.strain_peaks[i];
                if peak > spike_threshold {
                    // Penalty: pull the peak towards the threshold.
                    // NewPeak = Threshold + (OldPeak - Threshold) * 0.62
                    self.strain_peaks[i] = spike_threshold + (peak - spike_threshold) * 0.62;
                }
            }
        }
        // ------------------------------------

        let mut difficulty = 0.0;
        let mut weight = 1.0;

        self.strain_peaks
            .sort_unstable_by(|a, b| b.partial_cmp(a).unwrap_or(Ordering::Equal));

        for &strain in self.strain_peaks.iter() {
            difficulty += strain * weight;
            weight *= DECAY_WEIGHT;
        }

        self.difficulty_value = Some(difficulty);

        difficulty
    }

    pub(crate) fn count_difficult_strains(&mut self) -> f32 {
        let difficulty_value = self.difficulty_value.unwrap_or(self.difficulty_value());
        let single_strain = difficulty_value / 10.0;

        self.object_strains
            .iter()
            .map(|strain| 1.1 / (1.0 + (-10.0 * (strain / single_strain - 0.88)).exp()))
            .sum::<f32>()
    }

    #[inline]
    fn skill_multiplier(&self) -> f32 {
        match self.kind {
            SkillKind::Aim => AIM_SKILL_MULTIPLIER,
            SkillKind::Speed => SPEED_SKILL_MULTIPLIER,
        }
    }

    #[inline]
    fn strain_decay_base(&self) -> f32 {
        match self.kind {
            SkillKind::Aim => AIM_STRAIN_DECAY_BASE,
            SkillKind::Speed => SPEED_STRAIN_DECAY_BASE,
        }
    }

    #[inline]
    fn peak_strain(&self, delta_time: f32) -> f32 {
        self.current_strain * self.strain_decay(delta_time)
    }

    #[inline]
    fn strain_decay(&self, ms: f32) -> f32 {
        self.strain_decay_base().powf(ms / 1000.0)
    }
}
