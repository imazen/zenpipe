use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Auto levels: stretch the luminance histogram to fill [0, 1].
///
/// Scans the L plane to find cutoff points, then remaps luminance so the
/// low cutoff maps to 0 and the high cutoff maps to 1. Equivalent to
/// ImageMagick `-auto-level`, with several improvements over a naive
/// implementation.
///
/// ## Range detection
///
/// With default `clip_low = 0.0` / `clip_high = 0.0`, *smart plateau detection*
/// is used: the histogram is scanned from both ends to find where the density
/// first rises above 0.5 % of total pixels. This correctly keeps large white or
/// dark regions (white backgrounds, night scenes) while discarding isolated
/// dead/hot pixels that would otherwise force the stretch to include them.
///
/// Set `clip_low` / `clip_high` to a non-zero fraction to use explicit
/// percentile clipping instead (e.g. `0.01` = clip the darkest 1 %).
///
/// ## Midpoint correction
///
/// Set `target_midpoint` (e.g. `0.5`) to additionally apply gamma correction
/// so that the *median* luminance lands at the target after the stretch.
/// Without this, a dark-but-contrasty scene will still look dark after
/// histogram stretching because the median is well below 0.5.
///
/// ## Chroma policy
///
/// `scale_chroma = false` (default): a/b channels are left unchanged.
/// The levels operation is purely luminance — colours keep their absolute
/// Oklab coordinates, which means perceived saturation is unchanged.
///
/// `scale_chroma = true`: a/b are scaled by the same factor as L.
/// This preserves the chroma-to-L ratio (hue and relative saturation are
/// constant) but raises absolute chroma, which can look vivid on
/// underexposed photos.
///
/// ## Color cast removal
///
/// `remove_cast = true`: subtracts `mean(a)` and `mean(b)` from the
/// respective planes, driving the average scene colour toward neutral grey.
/// Applied before the optional chroma scale. Useful for studio shots with
/// non-neutral backgrounds or for scanned documents with paper yellowing.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AutoLevels {
    /// Fraction of pixels to clip at the dark end.
    /// 0.0 = smart plateau detection (default).
    /// Range: 0.0–0.1.
    pub clip_low: f32,
    /// Fraction of pixels to clip at the bright end.
    /// 0.0 = smart plateau detection (default).
    /// Range: 0.0–0.1.
    pub clip_high: f32,
    /// Blend strength. 0.0 = no change, 1.0 = full stretch.
    /// Range: 0.0–1.0. Default: 1.0.
    pub strength: f32,
    /// Move the median luminance to this value via gamma correction.
    /// 0.0 = off (default). 0.5 = natural midpoint target.
    /// Range: 0.0–1.0.
    pub target_midpoint: f32,
    /// Scale a/b chroma by the same factor as L.
    /// false = leave chroma unchanged (default).
    pub scale_chroma: bool,
    /// Subtract mean(a) and mean(b) to neutralize color cast.
    /// false = off (default).
    pub remove_cast: bool,
}

impl Default for AutoLevels {
    fn default() -> Self {
        Self {
            clip_low: 0.0,
            clip_high: 0.0,
            strength: 1.0,
            target_midpoint: 0.0,
            scale_chroma: false,
            remove_cast: false,
        }
    }
}

struct LevelsAnalysis {
    in_black: f32,
    in_white: f32,
    /// Median of L values normalized into [in_black, in_white] → [0, 1].
    /// Clamped to [0.02, 0.98]. Only valid when `target_midpoint > 0`.
    median_normalized: f32,
}

// 64 bins balances precision with robustness: each bin holds ~pc/64 pixels on
// average, so a 0.5%-of-total threshold reliably separates isolated outlier
// pixels (1 pixel) from the main distribution (~pc/64 pixels per bin).
// 512 bins would make each bin hold only ~pc/512 pixels, too close to 1 for
// the threshold to distinguish outliers from body content in small images.
const HIST_BINS: usize = 64;

impl AutoLevels {
    fn analyze(&self, l: &[f32]) -> Option<LevelsAnalysis> {
        let pc = l.len();
        if pc == 0 {
            return None;
        }

        // Pass 1: find true min/max.
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for &v in l {
            if v < lo {
                lo = v;
            }
            if v > hi {
                hi = v;
            }
        }
        let range = hi - lo;
        if range < 1e-6 {
            return None; // flat image
        }

        // Pass 2: build histogram over [lo, hi].
        let mut hist = [0u32; HIST_BINS];
        let scale = (HIST_BINS - 1) as f32 / range;
        for &v in l {
            let bin = ((v - lo) * scale) as usize;
            hist[bin.min(HIST_BINS - 1)] += 1;
        }

        // Threshold: a bin must have at least 0.5% of total pixels to count
        // as significant content (not an isolated outlier). Minimum of 1.
        let threshold = ((pc as f32 * 0.005).ceil() as u32).max(1);
        let inv_scale = range / (HIST_BINS - 1) as f32;

        // Determine in_black / in_white.
        let (in_black, in_white) = if self.clip_low < 1e-7 && self.clip_high < 1e-7 {
            // Smart plateau detection: walk inward until we hit a significant bin.
            let low_bin = hist.iter().position(|&v| v >= threshold).unwrap_or(0);
            let high_bin = hist.iter().rposition(|&v| v >= threshold).unwrap_or(HIST_BINS - 1);
            if high_bin <= low_bin {
                return None;
            }
            (
                lo + low_bin as f32 * inv_scale,
                lo + high_bin as f32 * inv_scale,
            )
        } else {
            // Explicit percentile clipping.
            let low_target = (self.clip_low * pc as f32) as u32;
            let high_target = (self.clip_high * pc as f32) as u32;

            let mut cumsum = 0u32;
            let mut low_bin = 0;
            for (i, &count) in hist.iter().enumerate() {
                cumsum += count;
                if cumsum > low_target {
                    low_bin = i;
                    break;
                }
            }

            let mut cumsum = 0u32;
            let mut high_bin = HIST_BINS - 1;
            for (i, &count) in hist.iter().enumerate().rev() {
                cumsum += count;
                if cumsum > high_target {
                    high_bin = i;
                    break;
                }
            }
            if high_bin <= low_bin {
                return None;
            }
            (
                lo + low_bin as f32 * inv_scale,
                lo + high_bin as f32 * inv_scale,
            )
        };

        // Find median for midpoint gamma correction.
        let median_normalized = if self.target_midpoint > 1e-6 {
            let in_range = (in_white - in_black).max(1e-6);
            let half = (pc / 2) as u32;
            let mut cumsum = 0u32;
            let mut median_bin = HIST_BINS / 2;
            for (i, &count) in hist.iter().enumerate() {
                cumsum += count;
                if cumsum >= half {
                    median_bin = i;
                    break;
                }
            }
            let median_l = lo + median_bin as f32 * inv_scale;
            ((median_l - in_black) / in_range).clamp(0.02, 0.98)
        } else {
            0.5 // unused sentinel
        };

        Some(LevelsAnalysis {
            in_black,
            in_white,
            median_normalized,
        })
    }
}

impl Filter for AutoLevels {
    fn channel_access(&self) -> ChannelAccess {
        if self.scale_chroma || self.remove_cast {
            ChannelAccess::L_AND_CHROMA
        } else {
            ChannelAccess::L_ONLY
        }
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::AutoLevels
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let pc = planes.pixel_count();
        if pc == 0 {
            return;
        }

        // Analysis can return None for flat images (no L range to stretch).
        // Chroma operations (remove_cast, scale_chroma) don't need a valid range,
        // so we proceed even without a usable L histogram.
        let analysis = self.analyze(&planes.l);

        let (in_black, inv_range, gamma_inv, l_noop) = match &analysis {
            Some(a) => {
                let inv_range = 1.0 / (a.in_white - a.in_black).max(1e-6);
                // t^gamma_inv = target  →  gamma_inv = ln(target) / ln(median)
                let gamma_inv = if self.target_midpoint > 1e-6 {
                    let t = a.median_normalized;
                    if (t - self.target_midpoint).abs() < 0.02 {
                        1.0
                    } else {
                        (self.target_midpoint.ln() / t.ln()).clamp(0.1, 10.0)
                    }
                } else {
                    1.0
                };
                let noop = a.in_black.abs() < 1e-4
                    && (a.in_white - 1.0).abs() < 1e-4
                    && (gamma_inv - 1.0).abs() < 1e-3;
                (a.in_black, inv_range, gamma_inv, noop)
            }
            None => (0.0, 1.0, 1.0, true), // flat image: skip L transform
        };

        // Early exit if every operation is identity.
        if l_noop && !self.scale_chroma && !self.remove_cast {
            return;
        }

        // Measure color cast means (needed before any L transform touches cache).
        let (cast_a, cast_b) = if self.remove_cast {
            let mean_a = planes.a.iter().sum::<f32>() / pc as f32;
            let mean_b = planes.b.iter().sum::<f32>() / pc as f32;
            (mean_a, mean_b)
        } else {
            (0.0, 0.0)
        };

        let s = self.strength;
        let use_gamma = (gamma_inv - 1.0).abs() > 1e-4;

        // ── Luminance ────────────────────────────────────────────────────────
        if (s - 1.0).abs() < 1e-6 {
            // Full strength — no blending needed.
            if use_gamma {
                for v in &mut planes.l {
                    let t = ((*v - in_black) * inv_range).clamp(0.0, 1.0);
                    *v = t.powf(gamma_inv);
                }
            } else {
                for v in &mut planes.l {
                    *v = ((*v - in_black) * inv_range).clamp(0.0, 1.0);
                }
            }
        } else {
            // Partial strength — blend toward the stretched value.
            if use_gamma {
                for v in &mut planes.l {
                    let t = ((*v - in_black) * inv_range).clamp(0.0, 1.0);
                    *v += (t.powf(gamma_inv) - *v) * s;
                }
            } else {
                for v in &mut planes.l {
                    let stretched = ((*v - in_black) * inv_range).clamp(0.0, 1.0);
                    *v += (stretched - *v) * s;
                }
            }
        }

        // ── Chroma ───────────────────────────────────────────────────────────
        // Unified transform: a' = a * factor - offset
        //
        // With scale_chroma = false, remove_cast = false → factor=1, offset=0 (identity).
        // With remove_cast = true                        → offset = cast * s.
        // With scale_chroma = true                       → factor includes inv_range.
        // Applied at partial strength s via blending arithmetic:
        //   a_out = a + (target - a) * s
        //         = a + ((a - cast) * chroma_scale - a) * s    (target = (a - cast) * scale)
        //         = a * (1 + (chroma_scale - 1) * s) - cast * chroma_scale * s
        let chroma_scale = if self.scale_chroma { inv_range } else { 1.0 };
        let factor = 1.0 + (chroma_scale - 1.0) * s;
        let a_offset = cast_a * chroma_scale * s;
        let b_offset = cast_b * chroma_scale * s;

        let chroma_noop = (factor - 1.0).abs() < 1e-6 && a_offset.abs() < 1e-6;
        if !chroma_noop {
            for v in &mut planes.a {
                *v = *v * factor - a_offset;
            }
            for v in &mut planes.b {
                *v = *v * factor - b_offset;
            }
        }
    }
}

static AUTO_LEVELS_SCHEMA: FilterSchema = FilterSchema {
    name: "auto_levels",
    label: "Auto Levels",
    description: "Stretch the luminance histogram to fill the full range",
    group: FilterGroup::Auto,
    params: &[
        ParamDesc {
            name: "clip_low",
            label: "Clip Low",
            description:
                "Fraction of pixels to clip at the dark end. 0 = smart plateau detection.",
            kind: ParamKind::Float {
                min: 0.0,
                max: 0.1,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Range",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "clip_high",
            label: "Clip High",
            description:
                "Fraction of pixels to clip at the bright end. 0 = smart plateau detection.",
            kind: ParamKind::Float {
                min: 0.0,
                max: 0.1,
                default: 0.0,
                identity: 0.0,
                step: 0.005,
            },
            unit: "",
            section: "Range",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "target_midpoint",
            label: "Midpoint Target",
            description:
                "Move the median luminance to this value via gamma correction. 0 = off.",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.0,
                identity: 0.0,
                step: 0.05,
            },
            unit: "",
            section: "Tone",
            slider: SliderMapping::Linear,
        },
        ParamDesc {
            name: "scale_chroma",
            label: "Scale Chroma",
            description:
                "Scale a/b channels by the same factor as L (raises saturation on stretch).",
            kind: ParamKind::Bool { default: false },
            unit: "",
            section: "Color",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "remove_cast",
            label: "Remove Color Cast",
            description: "Subtract mean(a) and mean(b) to neutralize the average color cast.",
            kind: ParamKind::Bool { default: false },
            unit: "",
            section: "Color",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "strength",
            label: "Strength",
            description: "Blend strength (0 = off, 1 = full stretch).",
            kind: ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 1.0,
                identity: 1.0,
                step: 0.05,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for AutoLevels {
    fn schema() -> &'static FilterSchema {
        &AUTO_LEVELS_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "clip_low" => Some(ParamValue::Float(self.clip_low)),
            "clip_high" => Some(ParamValue::Float(self.clip_high)),
            "strength" => Some(ParamValue::Float(self.strength)),
            "target_midpoint" => Some(ParamValue::Float(self.target_midpoint)),
            "scale_chroma" => Some(ParamValue::Bool(self.scale_chroma)),
            "remove_cast" => Some(ParamValue::Bool(self.remove_cast)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match (name, value) {
            ("clip_low", ParamValue::Float(v)) => self.clip_low = v,
            ("clip_high", ParamValue::Float(v)) => self.clip_high = v,
            ("strength", ParamValue::Float(v)) => self.strength = v,
            ("target_midpoint", ParamValue::Float(v)) => self.target_midpoint = v,
            ("scale_chroma", ParamValue::Bool(v)) => self.scale_chroma = v,
            ("remove_cast", ParamValue::Bool(v)) => self.remove_cast = v,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    #[test]
    fn default_is_strength_one() {
        let f = AutoLevels::default();
        assert!((f.strength - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 16.0 * 0.5 + 0.1;
        }
        let original = planes.l.clone();
        AutoLevels {
            strength: 0.0,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        for (a, b) in planes.l.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-6, "zero strength changed L: {a} vs {b}");
        }
    }

    #[test]
    fn stretches_to_full_range() {
        let mut planes = OklabPlanes::new(4, 1);
        planes.l = vec![0.2, 0.4, 0.6, 0.8];
        AutoLevels::default().apply(&mut planes, &mut FilterContext::new());
        assert!(
            planes.l[0] < 0.01,
            "minimum should map near 0: {}",
            planes.l[0]
        );
        assert!(
            (planes.l[3] - 1.0).abs() < 0.01,
            "maximum should map near 1: {}",
            planes.l[3]
        );
    }

    #[test]
    fn flat_image_unchanged() {
        let mut planes = OklabPlanes::new(4, 1);
        planes.l = vec![0.5, 0.5, 0.5, 0.5];
        let original = planes.l.clone();
        AutoLevels::default().apply(&mut planes, &mut FilterContext::new());
        for (a, b) in planes.l.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-6, "flat image changed: {a} vs {b}");
        }
    }

    #[test]
    fn already_full_range_unchanged() {
        let mut planes = OklabPlanes::new(4, 1);
        planes.l = vec![0.0, 0.33, 0.67, 1.0];
        let original = planes.l.clone();
        AutoLevels::default().apply(&mut planes, &mut FilterContext::new());
        for (a, b) in planes.l.iter().zip(original.iter()) {
            assert!((a - b).abs() < 1e-4, "full-range image changed: {a} vs {b}");
        }
    }

    #[test]
    fn smart_detection_ignores_single_pixel_outlier() {
        // 1000 pixels in [0.2, 0.6], one outlier at 0.95.
        // Smart detection should find in_white ≈ 0.6, not 0.95.
        let n = 1001usize;
        let mut planes = OklabPlanes::new(n as u32, 1);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = if i == 0 { 0.95 } else { 0.2 + (i as f32 / n as f32) * 0.4 };
        }
        AutoLevels::default().apply(&mut planes, &mut FilterContext::new());
        // The outlier (index 0) should be clipped to 1.0, not drive the stretch.
        assert!(
            planes.l[0] >= 1.0 - 1e-3,
            "outlier should be clipped to ≥1.0: {}",
            planes.l[0]
        );
        // Pixels that were at the top of the main body (~0.6) should land near 1.0.
        let near_top = planes.l[n - 1];
        assert!(
            near_top > 0.95,
            "top of main body should map near 1.0: {near_top}"
        );
    }

    #[test]
    fn explicit_clip_uses_percentile() {
        // 100 pixels: 5 low outliers (0.05), 90 body (0.5), 5 high outliers (0.95).
        let mut planes = OklabPlanes::new(100, 1);
        planes.l = (0..100)
            .map(|i| if i < 5 { 0.05f32 } else if i >= 95 { 0.95 } else { 0.5 })
            .collect();
        AutoLevels {
            clip_low: 0.06,
            clip_high: 0.06,
            strength: 1.0,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        // After clipping the 5% tails, the body (uniform 0.5) gets stretched.
        // All body pixels should end up at the same value.
        let body: alloc::vec::Vec<_> = planes.l[5..95].to_vec();
        let all_same = body.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-4);
        assert!(all_same, "body pixels should all map to same value after clip");
    }

    #[test]
    fn target_midpoint_moves_median() {
        // 100 pixels uniformly spread [0.1, 0.5] → median ≈ 0.3.
        // With target_midpoint=0.5 the median should end up near 0.5 after stretch+gamma.
        let n = 100u32;
        let mut planes = OklabPlanes::new(n, 1);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.1 + (i as f32 / (n - 1) as f32) * 0.4;
        }
        AutoLevels {
            target_midpoint: 0.5,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        let mut vals = planes.l.clone();
        vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let median = vals[vals.len() / 2];
        assert!(
            (median - 0.5).abs() < 0.06,
            "median should be near 0.5 after midpoint correction: {median}"
        );
    }

    #[test]
    fn scale_chroma_off_leaves_chroma_unchanged() {
        let mut planes = OklabPlanes::new(4, 1);
        planes.l = vec![0.2, 0.4, 0.6, 0.8];
        for v in &mut planes.a {
            *v = 0.1;
        }
        let original_a = planes.a.clone();
        AutoLevels {
            scale_chroma: false,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        for (a, b) in planes.a.iter().zip(original_a.iter()) {
            assert!((a - b).abs() < 1e-6, "chroma changed with scale_chroma=false");
        }
    }

    #[test]
    fn scale_chroma_on_scales_chroma() {
        let mut planes = OklabPlanes::new(4, 1);
        planes.l = vec![0.2, 0.4, 0.6, 0.8]; // stretched by ×1.667
        for v in &mut planes.a {
            *v = 0.1;
        }
        AutoLevels {
            scale_chroma: true,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        // inv_range = 1/(0.8-0.2) = 1.667; a should be 0.1 * 1.667 ≈ 0.167
        for &v in &planes.a {
            assert!(
                (v - 0.1 * (1.0 / 0.6)).abs() < 0.01,
                "chroma should be scaled by inv_range: {v}"
            );
        }
    }

    #[test]
    fn remove_cast_drives_mean_toward_zero() {
        let mut planes = OklabPlanes::new(100, 1);
        for v in &mut planes.l {
            *v = 0.3;
        }
        // Set a clear cast on both channels.
        for (i, v) in planes.a.iter_mut().enumerate() {
            *v = 0.2 + i as f32 * 0.001; // mean ≈ 0.25
        }
        for v in &mut planes.b {
            *v = -0.1; // mean = -0.1
        }
        AutoLevels {
            remove_cast: true,
            scale_chroma: false,
            ..Default::default()
        }
        .apply(&mut planes, &mut FilterContext::new());
        let mean_a: f32 = planes.a.iter().sum::<f32>() / 100.0;
        let mean_b: f32 = planes.b.iter().sum::<f32>() / 100.0;
        assert!(
            mean_a.abs() < 0.01,
            "mean(a) should be near zero after cast removal: {mean_a}"
        );
        assert!(
            mean_b.abs() < 0.01,
            "mean(b) should be near zero after cast removal: {mean_b}"
        );
    }

    #[test]
    fn partial_strength_blends() {
        let mut planes_half = OklabPlanes::new(4, 1);
        let mut planes_full = OklabPlanes::new(4, 1);
        for planes in [&mut planes_half, &mut planes_full] {
            planes.l = vec![0.2, 0.4, 0.6, 0.8];
        }
        let original = planes_half.l.clone();

        AutoLevels {
            strength: 0.5,
            ..Default::default()
        }
        .apply(&mut planes_half, &mut FilterContext::new());
        AutoLevels::default().apply(&mut planes_full, &mut FilterContext::new());

        for i in 0..4 {
            let expected = original[i] + (planes_full.l[i] - original[i]) * 0.5;
            assert!(
                (planes_half.l[i] - expected).abs() < 1e-4,
                "half-strength blend wrong at {i}: {} vs expected {expected}",
                planes_half.l[i]
            );
        }
    }
}
