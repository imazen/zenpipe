use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Automatic highlight recovery: soft-clips blown highlights.
///
/// Analyzes the L histogram to detect how much highlight content exists,
/// then applies a proportional soft knee compression. Images with
/// properly exposed highlights are barely affected.
///
/// This fixes the most common phone camera failure: hard-clipped skies,
/// bright windows, and specular highlights that lose all detail.
///
/// Algorithm:
/// 1. Compute histogram percentiles (p95, p99.5)
/// 2. If p99.5 - p95 is small (highlights compressed), set knee lower
/// 3. Apply soft knee: above knee, L' = knee + (1-knee) * x/(x + strength)
///    where x = (L - knee) / (1 - knee)
///
/// The result is a smooth shoulder that preserves detail in bright areas
/// without affecting anything below the knee point.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct HighlightRecovery {
    /// Recovery strength. 0.0 = off, 1.0 = full recovery.
    pub strength: f32,
}

impl Default for HighlightRecovery {
    fn default() -> Self {
        Self { strength: 0.0 }
    }
}

impl Filter for HighlightRecovery {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::HighlightRecovery
    }
    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let pc = planes.pixel_count();
        if pc == 0 {
            return;
        }

        // Histogram analysis: find p95 and p99.5
        let (p95, p995) = percentiles(&planes.l, &[0.95, 0.995]);

        // If nothing is bright, skip
        if p95 < 0.7 {
            return;
        }

        // Adaptive knee: place it where highlights start getting dense.
        // If p99.5 ≈ p95, highlights are hard-clipped → lower the knee for more recovery.
        // If p99.5 >> p95, highlights have natural rolloff → higher knee, less aggressive.
        let highlight_range = (p995 - p95).max(0.001);
        let base_knee = p95 - highlight_range * self.strength;
        let knee = base_knee.clamp(0.5, 0.98);

        // Soft knee compression: rational function above knee
        // L' = knee + (1-knee) * x / (x + s)
        // where x = (L - knee) / (1 - knee), s = strength * compression_factor
        let range = 1.0 - knee;
        let s = self.strength * 0.5; // tuned: 0.5 gives natural rolloff at strength=1

        for v in planes.l.iter_mut() {
            if *v > knee {
                let x = (*v - knee) / range;
                *v = knee + range * x / (x + s);
            }
        }
    }
}

static HIGHLIGHT_RECOVERY_SCHEMA: FilterSchema = FilterSchema {
    name: "highlight_recovery",
    label: "Highlight Recovery",
    description: "Automatic soft-clip recovery for blown highlights",
    group: FilterGroup::ToneRange,
    params: &[ParamDesc {
        name: "strength",
        label: "Strength",
        description: "Recovery strength (0 = off, 1 = full)",
        kind: ParamKind::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            identity: 0.0,
            step: 0.05,
        },
        unit: "",
        section: "Main",
        slider: SliderMapping::Linear,
    }],
};

impl Describe for HighlightRecovery {
    fn schema() -> &'static FilterSchema {
        &HIGHLIGHT_RECOVERY_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "strength" => Some(ParamValue::Float(self.strength)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "strength" => self.strength = v,
            _ => return false,
        }
        true
    }
}

/// Compute percentiles from a slice using approximate histogram.
fn percentiles(data: &[f32], pcts: &[f64]) -> (f32, f32) {
    // 1024-bin histogram over [0, 1]
    const BINS: usize = 1024;
    let mut hist = [0u32; BINS];
    for &v in data {
        let bin = ((v * BINS as f32) as usize).min(BINS - 1);
        hist[bin] += 1;
    }

    let n = data.len() as f64;
    let mut results = [0.0f32; 2];
    for (ri, &pct) in pcts.iter().enumerate() {
        let target = (n * pct) as u64;
        let mut cumsum = 0u64;
        for (bin, &count) in hist.iter().enumerate() {
            cumsum += count as u64;
            if cumsum >= target {
                results[ri] = bin as f32 / BINS as f32;
                break;
            }
        }
    }
    (results[0], results[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 16.0;
        }
        let original = planes.l.clone();
        HighlightRecovery { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn recovers_blown_highlights() {
        let mut planes = OklabPlanes::new(100, 1);
        // 80% of pixels are normal, 20% are blown highlights
        for i in 0..80 {
            planes.l[i] = 0.3 + (i as f32 / 80.0) * 0.5; // 0.3-0.8
        }
        for i in 80..100 {
            planes.l[i] = 0.95 + (i as f32 - 80.0) / 200.0; // 0.95-1.05
        }
        let bright_before: f32 = planes.l[80..100].iter().sum();
        HighlightRecovery { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let bright_after: f32 = planes.l[80..100].iter().sum();
        // Highlights should be compressed (sum decreases)
        assert!(
            bright_after < bright_before,
            "highlights should be compressed: {bright_before} -> {bright_after}"
        );
        // But they should still be ordered (monotonic)
        for i in 81..100 {
            assert!(
                planes.l[i] >= planes.l[i - 1] - 1e-6,
                "should remain monotonic at {i}: {} < {}",
                planes.l[i],
                planes.l[i - 1]
            );
        }
    }

    #[test]
    fn does_not_affect_dark_images() {
        let mut planes = OklabPlanes::new(100, 1);
        // All pixels below 0.5 — no highlights to recover
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 200.0; // 0.0-0.5
        }
        let original = planes.l.clone();
        HighlightRecovery { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original, "dark images should not be affected");
    }

    #[test]
    fn does_not_modify_chroma() {
        let mut planes = OklabPlanes::new(10, 1);
        for v in &mut planes.l {
            *v = 0.9;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        let a_orig = planes.a.clone();
        HighlightRecovery { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
    }

    #[test]
    fn stronger_recovers_more() {
        let make = || {
            let mut planes = OklabPlanes::new(100, 1);
            for i in 0..80 {
                planes.l[i] = 0.4 + (i as f32 / 80.0) * 0.3;
            }
            for i in 80..100 {
                planes.l[i] = 0.98;
            }
            planes
        };

        let mut planes_weak = make();
        let mut planes_strong = make();
        HighlightRecovery { strength: 0.3 }.apply(&mut planes_weak, &mut FilterContext::new());
        HighlightRecovery { strength: 1.0 }.apply(&mut planes_strong, &mut FilterContext::new());

        // Strong recovery should compress more
        let bright_weak = planes_weak.l[90];
        let bright_strong = planes_strong.l[90];
        assert!(
            bright_strong < bright_weak,
            "stronger recovery should compress more: weak={bright_weak} strong={bright_strong}"
        );
    }
}
