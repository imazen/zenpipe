use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::simd;

/// One-button automatic tone correction.
///
/// Analyzes the image and applies exposure, highlight recovery, shadow lift,
/// contrast, and color cast corrections as needed. Each sub-correction
/// activates only when the image statistics warrant it — a well-exposed
/// image gets minimal adjustment.
///
/// This is the "auto fix" that most users want: correct obvious problems
/// without imposing a creative look. All corrections are fused into
/// minimal passes over the pixel data.
///
/// `preserve_intent` controls how aggressively the filter corrects.
/// At 0, it fixes everything detectable. At 1, it only fixes severe
/// problems (hard-clipped highlights, crushed shadows, strong color cast).
/// This prevents an intentionally moody photo from being "fixed."
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AutoTone {
    /// Master strength. 0.0 = off, 1.0 = full correction.
    pub strength: f32,
    /// Intent preservation. 0.0 = correct everything, 1.0 = only severe problems.
    pub preserve_intent: f32,
}

impl Default for AutoTone {
    fn default() -> Self {
        Self {
            strength: 0.0,
            preserve_intent: 0.3,
        }
    }
}

impl Filter for AutoTone {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let pc = planes.pixel_count();
        if pc == 0 {
            return;
        }

        let a = ctx.analyze(planes).clone();
        let s = self.strength;
        let pi = self.preserve_intent;

        // ── 1. Exposure correction ──────────────────────────────────
        // Correct if median is far from perceptual mid-grey (0.45 in Oklab L).
        let median = a.p50();
        let exposure_factor = {
            let target = 0.45;
            let error = target - median;
            // Only correct if error is meaningful. High preserve_intent
            // requires larger error before triggering.
            let threshold = 0.05 + pi * 0.15; // 0.05 at pi=0, 0.20 at pi=1
            if error.abs() > threshold {
                let correction = target / median.max(0.01);
                // Clamp to reasonable range (2^(±2/3) in Oklab ≈ ±2 EV)
                let max_f = 2.0f32.powf(2.0 / 3.0);
                let clamped = correction.clamp(1.0 / max_f, max_f);
                1.0 + (clamped - 1.0) * s * (1.0 - pi * 0.5)
            } else {
                1.0
            }
        };

        if (exposure_factor - 1.0).abs() > 1e-6 {
            simd::scale_plane(&mut planes.l, exposure_factor);
            simd::scale_plane(&mut planes.a, exposure_factor);
            simd::scale_plane(&mut planes.b, exposure_factor);
            ctx.invalidate_analysis();
        }

        // ── 2. Highlight recovery ───────────────────────────────────
        // Soft-clip if highlights are hard-clipped.
        let highlight_range = a.p99() - a.p95();
        if a.p95() > 0.80 && highlight_range < 0.03 {
            let recovery_strength = 0.5 * s * (1.0 - pi * 0.3);
            let knee = (a.p95() - highlight_range * recovery_strength * 2.0).clamp(0.5, 0.98);
            let range = 1.0 - knee;
            let comp_s = 0.4;
            for v in planes.l.iter_mut() {
                if *v > knee {
                    let x = (*v - knee) / range;
                    *v = knee + range * x / (x + comp_s);
                }
            }
        }

        // ── 3. Shadow lift ──────────────────────────────────────────
        // Open crushed shadows with toe curve.
        if a.p5() < 0.12 {
            let density = ((0.15 - a.p5()) / 0.15).max(0.0).sqrt();
            let lift = s * density * 1.5 * (1.0 - pi * 0.3);
            if lift > 1e-4 {
                let toe = (0.12 + 0.15 * s).clamp(0.05, 0.3);
                let gamma = 1.0 / (1.0 + lift);
                let inv_toe = 1.0 / toe;
                for v in planes.l.iter_mut() {
                    if *v > 0.0 && *v < toe {
                        let t = *v * inv_toe;
                        *v = toe * crate::fast_math::fast_powf(t, gamma);
                    }
                }
            }
        }

        // ── 4. Contrast correction ──────────────────────────────────
        // Boost contrast if the image is flat.
        if a.contrast_ratio < 0.18 {
            let boost = (0.18 - a.contrast_ratio) / 0.18 * 0.15 * s * (1.0 - pi * 0.5);
            if boost > 0.005 {
                let pivot = median.clamp(0.2, 0.8);
                let exp = 1.0 + boost;
                let scale = pivot.powf(-boost); // keeps pivot unchanged
                for v in planes.l.iter_mut() {
                    *v = (crate::fast_math::fast_powf((*v).max(0.0), exp) * scale).max(0.0);
                }
            }
        }

        // ── 5. Color cast correction ────────────────────────────────
        // Remove strong color cast (mean a/b deviation from neutral).
        let cast_threshold = 0.02 + pi * 0.04; // 0.02 at pi=0, 0.06 at pi=1
        let cast_scale = s * (1.0 - pi * 0.5);

        if a.mean_b.abs() > cast_threshold {
            let correction = -a.mean_b * cast_scale * 0.8;
            simd::offset_plane(&mut planes.b, correction);
        }
        if a.mean_a.abs() > cast_threshold {
            let correction = -a.mean_a * cast_scale * 0.8;
            simd::offset_plane(&mut planes.a, correction);
        }
    }
}

static AUTO_TONE_SCHEMA: FilterSchema = FilterSchema {
    name: "auto_tone",
    label: "Auto Tone",
    description: "One-button automatic tone and color correction",
    group: FilterGroup::Auto,
    params: &[
        ParamDesc {
            name: "strength",
            label: "Strength",
            description: "Master correction strength (0 = off, 1 = full)",
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
        },
        ParamDesc {
            name: "preserve_intent",
            label: "Preserve Intent",
            description: "Respect intentional exposure (0 = correct everything, 1 = only severe problems)",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.3,
                identity: 0.3,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for AutoTone {
    fn schema() -> &'static FilterSchema {
        &AUTO_TONE_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "strength" => Some(ParamValue::Float(self.strength)),
            "preserve_intent" => Some(ParamValue::Float(self.preserve_intent)),
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
            "preserve_intent" => self.preserve_intent = v,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.l {
            *v = 0.3;
        }
        let original = planes.l.clone();
        let mut f = AutoTone::default();
        f.strength = 0.0;
        f.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn corrects_dark_image() {
        let mut planes = OklabPlanes::new(100, 1);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.05 + (i as f32 / 100.0) * 0.1; // very dark: 0.05-0.15
        }
        let mean_before = planes.l.iter().sum::<f32>() / 100.0;
        let mut f = AutoTone::default();
        f.strength = 1.0;
        f.preserve_intent = 0.0;
        f.apply(&mut planes, &mut FilterContext::new());
        let mean_after = planes.l.iter().sum::<f32>() / 100.0;
        assert!(
            mean_after > mean_before * 1.2,
            "dark image should be brightened: {mean_before} -> {mean_after}"
        );
    }

    #[test]
    fn removes_color_cast() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.5;
        }
        for v in &mut planes.b {
            *v = 0.08; // warm cast
        }
        let cast_before = planes.b.iter().sum::<f32>() / 100.0;
        let mut f = AutoTone::default();
        f.strength = 1.0;
        f.preserve_intent = 0.0;
        f.apply(&mut planes, &mut FilterContext::new());
        let cast_after = planes.b.iter().sum::<f32>() / 100.0;
        assert!(
            cast_after.abs() < cast_before.abs(),
            "color cast should be reduced: {cast_before} -> {cast_after}"
        );
    }

    #[test]
    fn preserve_intent_reduces_correction() {
        let make = || {
            let mut planes = OklabPlanes::new(100, 1);
            for v in &mut planes.l {
                *v = 0.15; // moderately dark
            }
            planes
        };

        let mut planes_low = make();
        let mut planes_high = make();

        let mut f_low = AutoTone::default();
        f_low.strength = 1.0;
        f_low.preserve_intent = 0.0;
        f_low.apply(&mut planes_low, &mut FilterContext::new());

        let mut f_high = AutoTone::default();
        f_high.strength = 1.0;
        f_high.preserve_intent = 0.8;
        f_high.apply(&mut planes_high, &mut FilterContext::new());

        let change_low = (planes_low.l[0] - 0.15).abs();
        let change_high = (planes_high.l[0] - 0.15).abs();
        assert!(
            change_low > change_high,
            "high preserve_intent should correct less: low={change_low} high={change_high}"
        );
    }
}
