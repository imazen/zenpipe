//! sRGB-compatible filter types for ImageMagick parity.
//!
//! These filters operate on RGB plane values directly, matching
//! ImageMagick's default sRGB-space behavior. Use with
//! `PipelineConfig::srgb_compat()` which puts sRGB values into the planes.
//!
//! For perceptually correct photo adjustments, use the Oklab-native filters
//! (`Contrast`, `Saturation`, `Grayscale`) with the default pipeline instead.

use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::{Filter, PlaneSemantics};
use crate::planes::OklabPlanes;

// ─── SigmoidalContrast ─────────────────────────────────────────────

/// Sigmoidal contrast adjustment on all planes.
///
/// Uses the same S-curve as ImageMagick's `-sigmoidal-contrast` and
/// `-brightness-contrast 0xN`. The sigmoid maps [0,1] → [0,1] with
/// steepening around the midpoint for positive contrast, and flattening
/// for negative contrast.
///
/// Formula: `sig(x) = 1 / (1 + exp(β * (α - x)))`
/// Normalized: `(sig(x) - sig(0)) / (sig(1) - sig(0))`
///
/// Unlike Oklab `Contrast` (power curve on perceptual lightness), this
/// applies a sigmoid S-curve identically to all channels in sRGB space.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct SigmoidalContrast {
    /// Contrast strength. 0.0 = no change.
    /// Positive = increase contrast (steeper S-curve).
    /// Negative = decrease contrast (flatter curve).
    /// Range: roughly -1.0 to 1.0. Maps internally to sigmoid β of ±10.
    pub amount: f32,
    /// Midpoint of the S-curve. Default: 0.5.
    pub midpoint: f32,
}

impl Default for SigmoidalContrast {
    fn default() -> Self {
        Self {
            amount: 0.0,
            midpoint: 0.5,
        }
    }
}

/// Evaluate the sigmoid at x with contrast β and midpoint α.
#[inline]
fn sigmoid(x: f32, beta: f32, alpha: f32) -> f32 {
    1.0 / (1.0 + (beta * (alpha - x)).exp())
}

/// Normalized sigmoidal contrast: maps [0,1] → [0,1].
#[inline]
fn sigmoidal_contrast(x: f32, beta: f32, alpha: f32) -> f32 {
    let sig_0 = sigmoid(0.0, beta, alpha);
    let sig_1 = sigmoid(1.0, beta, alpha);
    let denom = sig_1 - sig_0;
    if denom.abs() < 1e-10 {
        return x;
    }
    ((sigmoid(x, beta, alpha) - sig_0) / denom).clamp(0.0, 1.0)
}

/// Inverse sigmoidal contrast (for negative amount — reduces contrast).
#[inline]
fn inverse_sigmoidal_contrast(x: f32, beta: f32, alpha: f32) -> f32 {
    let sig_0 = sigmoid(0.0, beta, alpha);
    let sig_1 = sigmoid(1.0, beta, alpha);
    let denom = sig_1 - sig_0;
    if denom.abs() < 1e-10 {
        return x;
    }
    // Map x back through the inverse sigmoid
    let scaled = sig_0 + x * denom;
    let clamped = scaled.clamp(1e-6, 1.0 - 1e-6);
    (alpha - (1.0 / clamped - 1.0).ln() / beta).clamp(0.0, 1.0)
}

impl Filter for SigmoidalContrast {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Rgb
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        // Map amount to sigmoid β. IM uses β ≈ 10 for full contrast.
        let beta = self.amount * 10.0;
        let alpha = self.midpoint;

        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            if beta > 0.0 {
                for v in plane.iter_mut() {
                    *v = sigmoidal_contrast(*v, beta, alpha);
                }
            } else {
                for v in plane.iter_mut() {
                    *v = inverse_sigmoidal_contrast(*v, -beta, alpha);
                }
            }
        }
    }
}

// Keep LinearContrast as well — simpler, useful for non-IM use cases.

/// Linear contrast adjustment on all planes.
///
/// Formula: `v' = (v - 0.5) * (1 + amount) + 0.5`, clamped to [0, 1].
/// Simpler than `SigmoidalContrast` but doesn't match ImageMagick.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct LinearContrast {
    /// Contrast amount. 0.0 = no change, 1.0 = double contrast, -1.0 = flat gray.
    pub amount: f32,
}

impl Filter for LinearContrast {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Rgb
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.amount.abs() < 1e-6 {
            return;
        }
        let factor = 1.0 + self.amount;
        let offset = 0.5 * (1.0 - factor);
        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            for v in plane.iter_mut() {
                *v = (*v * factor + offset).clamp(0.0, 1.0);
            }
        }
    }
}

// ─── LinearBrightness ──────────────────────────────────────────────

/// Additive brightness adjustment on all planes.
///
/// Formula: `v' = v + offset`, clamped to [0, 1].
/// Matches ImageMagick's `-brightness-contrast Nx0` in sRGB space.
///
/// Unlike Oklab `Exposure` (photographic stops on perceptual lightness),
/// this is a simple additive offset applied identically to all channels.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct LinearBrightness {
    /// Brightness offset. 0.0 = no change. Range: -1.0 to 1.0.
    pub offset: f32,
}

impl Filter for LinearBrightness {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Rgb
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.offset.abs() < 1e-6 {
            return;
        }
        let off = self.offset;
        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            for v in plane.iter_mut() {
                *v = (*v + off).clamp(0.0, 1.0);
            }
        }
    }
}

// ─── HslSaturate ──────────────────────────────────────────────────

/// HSL-based saturation adjustment on RGB planes.
///
/// Converts each pixel RGB→HSL, scales S by `factor`, converts back.
/// Matches ImageMagick's `-modulate 100,N,100` in sRGB space.
///
/// Unlike Oklab `Saturation` (chroma scaling on a/b axes), this operates
/// in HSL which can shift hue for highly saturated colors.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct HslSaturate {
    /// Saturation factor. 1.0 = no change, 0.0 = grayscale, 2.0 = double.
    pub factor: f32,
}

impl Default for HslSaturate {
    fn default() -> Self {
        Self { factor: 1.0 }
    }
}

impl Filter for HslSaturate {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Rgb
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if (self.factor - 1.0).abs() < 1e-6 {
            return;
        }
        let s = self.factor;
        let n = planes.l.len();
        for i in 0..n {
            let r = planes.l[i];
            let g = planes.a[i];
            let b = planes.b[i];

            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let l = (max + min) * 0.5;
            let delta = max - min;

            if delta < 1e-6 {
                continue; // achromatic
            }

            let sat = if l <= 0.5 {
                delta / (max + min).max(1e-6)
            } else {
                delta / (2.0 - max - min).max(1e-6)
            };
            let new_sat = (sat * s).clamp(0.0, 1.0);

            // Hue (0-6)
            let hue = if (max - r).abs() < 1e-6 {
                (g - b) / delta + if g < b { 6.0 } else { 0.0 }
            } else if (max - g).abs() < 1e-6 {
                (b - r) / delta + 2.0
            } else {
                (r - g) / delta + 4.0
            };

            // HSL → RGB
            let c = (1.0 - (2.0 * l - 1.0).abs()) * new_sat;
            let x = c * (1.0 - ((hue % 2.0) - 1.0).abs());
            let m = l - c * 0.5;
            let (r1, g1, b1) = match hue as u32 {
                0 => (c, x, 0.0),
                1 => (x, c, 0.0),
                2 => (0.0, c, x),
                3 => (0.0, x, c),
                4 => (x, 0.0, c),
                _ => (c, 0.0, x),
            };
            planes.l[i] = (r1 + m).clamp(0.0, 1.0);
            planes.a[i] = (g1 + m).clamp(0.0, 1.0);
            planes.b[i] = (b1 + m).clamp(0.0, 1.0);
        }
    }
}

// ─── LumaGrayscale ────────────────────────────────────────────────

/// Rec.709 luma grayscale on RGB planes.
///
/// Formula: `lum = 0.2126*R + 0.7152*G + 0.0722*B`, applied to all planes.
/// Matches ImageMagick's `-colorspace Gray`.
///
/// Unlike Oklab `Grayscale` (zero chroma, preserving perceptual lightness),
/// this uses weighted luma coefficients in the working RGB space.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct LumaGrayscale;

impl Filter for LumaGrayscale {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Rgb
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let n = planes.l.len();
        for i in 0..n {
            let lum = 0.2126 * planes.l[i] + 0.7152 * planes.a[i] + 0.0722 * planes.b[i];
            planes.l[i] = lum;
            planes.a[i] = lum;
            planes.b[i] = lum;
        }
    }
}

// ─── ChannelPosterize ──────────────────────────────────────────────

/// Posterize all RGB planes uniformly.
///
/// Quantizes each channel to N levels independently.
/// Matches ImageMagick's `-posterize N`.
///
/// Unlike Oklab `Posterize` (quantizes L with optional chroma), this
/// applies the same quantization to all planes.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ChannelPosterize {
    /// Number of levels per channel (2–256).
    pub levels: u32,
}

impl Default for ChannelPosterize {
    fn default() -> Self {
        Self { levels: 4 }
    }
}

impl Filter for ChannelPosterize {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Rgb
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let steps = (self.levels.max(2) - 1) as f32;
        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            for v in plane.iter_mut() {
                *v = (*v * steps).round() / steps;
            }
        }
    }
}

// ─── ChannelSolarize ───────────────────────────────────────────────

/// Solarize all RGB planes.
///
/// Inverts pixels above the threshold on each channel independently.
/// Matches ImageMagick's `-solarize N%`.
///
/// Unlike Oklab `Solarize` (operates on L with optional chroma), this
/// applies threshold inversion to all planes uniformly.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ChannelSolarize {
    /// Threshold (0.0–1.0). Pixels above this are inverted.
    pub threshold: f32,
}

impl Default for ChannelSolarize {
    fn default() -> Self {
        Self { threshold: 0.5 }
    }
}

impl Filter for ChannelSolarize {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Rgb
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let t = self.threshold;
        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            for v in plane.iter_mut() {
                if *v > t {
                    *v = 1.0 - *v;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;

    #[test]
    fn sigmoidal_contrast_zero_is_identity() {
        let mut planes = OklabPlanes::new(8, 8);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 64.0;
        }
        let orig = planes.l.clone();
        SigmoidalContrast { amount: 0.0, midpoint: 0.5 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, orig);
    }

    #[test]
    fn sigmoidal_contrast_increases_range() {
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.3;
        planes.l[1] = 0.7;
        SigmoidalContrast { amount: 0.5, midpoint: 0.5 }.apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] < 0.3, "dark should get darker: {}", planes.l[0]);
        assert!(planes.l[1] > 0.7, "bright should get brighter: {}", planes.l[1]);
    }

    #[test]
    fn sigmoidal_contrast_negative_reduces_range() {
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.1;
        planes.l[1] = 0.9;
        SigmoidalContrast { amount: -0.5, midpoint: 0.5 }.apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] > 0.1, "dark should get lighter: {}", planes.l[0]);
        assert!(planes.l[1] < 0.9, "bright should get darker: {}", planes.l[1]);
    }

    #[test]
    fn linear_contrast_zero_is_identity() {
        let mut planes = OklabPlanes::new(8, 8);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 64.0;
        }
        let orig = planes.l.clone();
        LinearContrast { amount: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, orig);
    }

    #[test]
    fn linear_contrast_increases_range() {
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.3;
        planes.l[1] = 0.7;
        LinearContrast { amount: 0.5 }.apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0] < 0.3, "dark should get darker");
        assert!(planes.l[1] > 0.7, "bright should get brighter");
    }

    #[test]
    fn luma_grayscale_produces_neutral() {
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.8; // R
        planes.a[0] = 0.4; // G
        planes.b[0] = 0.2; // B
        LumaGrayscale.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l[0], planes.a[0]);
        assert_eq!(planes.a[0], planes.b[0]);
    }

    #[test]
    fn hsl_saturate_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.8;
        planes.a[0] = 0.3;
        planes.b[0] = 0.1;
        let orig_l = planes.l[0];
        let orig_a = planes.a[0];
        let orig_b = planes.b[0];
        HslSaturate { factor: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l[0], orig_l);
        assert_eq!(planes.a[0], orig_a);
        assert_eq!(planes.b[0], orig_b);
    }

    #[test]
    fn channel_posterize_quantizes_all() {
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.4;
        planes.a[0] = 0.6;
        planes.b[0] = 0.9;
        ChannelPosterize { levels: 2 }.apply(&mut planes, &mut FilterContext::new());
        // 2 levels: values should be 0.0 or 1.0
        assert!(planes.l[0] == 0.0 || planes.l[0] == 1.0);
        assert!(planes.a[0] == 0.0 || planes.a[0] == 1.0);
        assert!(planes.b[0] == 0.0 || planes.b[0] == 1.0);
    }

    #[test]
    fn channel_solarize_inverts_above_threshold() {
        let mut planes = OklabPlanes::new(4, 4);
        planes.l[0] = 0.3; // below
        planes.l[1] = 0.8; // above
        ChannelSolarize { threshold: 0.5 }.apply(&mut planes, &mut FilterContext::new());
        assert!((planes.l[0] - 0.3).abs() < 1e-6, "below unchanged");
        assert!((planes.l[1] - 0.2).abs() < 1e-6, "above inverted");
    }
}
