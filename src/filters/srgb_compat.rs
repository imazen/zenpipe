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
            // Don't clamp S — let it exceed 1.0, clamp final RGB instead.
            // This matches ImageMagick's behavior for already-saturated pixels.
            let new_sat = sat * s;

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

// ─── ChannelSharpen ────────────────────────────────────────────────

/// Unsharp mask sharpening on all planes (R, G, B).
///
/// Matches ImageMagick's `-sharpen 0xSIGMA` which applies Gaussian
/// unsharp mask to all channels. Amount is always 1.0 (IM convention).
///
/// Unlike Oklab `Sharpen` (L-only, avoids color fringing), this
/// sharpens each channel independently.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ChannelSharpen {
    /// Gaussian sigma for the blur used in unsharp mask.
    pub sigma: f32,
    /// Sharpening amount. IM's `-sharpen` uses 1.0.
    pub amount: f32,
}

impl Default for ChannelSharpen {
    fn default() -> Self {
        Self {
            sigma: 1.0,
            amount: 1.0,
        }
    }
}

impl Filter for ChannelSharpen {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Rgb
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        // IM's kernel radius: ceil(2*sigma + 0.5)
        (2.0 * self.sigma + 0.5).ceil() as u32
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        use crate::blur::{GaussianKernel, gaussian_blur_plane};
        use crate::simd;

        if self.amount.abs() < 1e-6 {
            return;
        }
        let kernel = GaussianKernel::new(self.sigma);
        let n = planes.pixel_count();
        let w = planes.width;
        let h = planes.height;

        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            let mut blurred = ctx.take_f32(n);
            gaussian_blur_plane(plane, &mut blurred, w, h, &kernel, ctx);
            let mut dst = ctx.take_f32(n);
            simd::unsharp_fuse(plane, &blurred, &mut dst, self.amount);
            ctx.return_f32(blurred);
            let old = core::mem::replace(plane, dst);
            ctx.return_f32(old);
        }
    }
}

// ─── GaussianMotionBlur ────────────────────────────────────────────

/// Gaussian-weighted directional blur matching ImageMagick's `-motion-blur`.
///
/// IM's `-motion-blur 0xSIGMA+ANGLE` uses a Gaussian-weighted line kernel
/// along the specified angle. Unlike our `MotionBlur` (uniform weights),
/// this uses proper Gaussian falloff for smoother results.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct GaussianMotionBlur {
    /// Gaussian sigma for the blur kernel.
    pub sigma: f32,
    /// Blur angle in degrees (0 = horizontal right, 90 = down).
    pub angle: f32,
}

impl Default for GaussianMotionBlur {
    fn default() -> Self {
        Self {
            sigma: 5.0,
            angle: 0.0,
        }
    }
}

impl Filter for GaussianMotionBlur {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        // IM's kernel radius: ceil(2*sigma + 0.5)
        (2.0 * self.sigma + 0.5).ceil() as u32
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.sigma < 0.5 {
            return;
        }

        let w = planes.width as usize;
        let h = planes.height as usize;
        let a = self.angle.to_radians();
        let dx = a.cos();
        let dy = a.sin();
        // IM's kernel radius formula
        let radius = (2.0 * self.sigma + 0.5).ceil() as usize;

        // Precompute Gaussian-weighted sample positions
        let mut weights = alloc::vec::Vec::new();
        let mut total = 0.0f32;
        let sigma2 = 2.0 * self.sigma * self.sigma;
        for i in 0..=(2 * radius) {
            let t = i as f32 - radius as f32;
            let w = (-t * t / sigma2).exp();
            weights.push((t * dx, t * dy, w));
            total += w;
        }
        let inv_total = 1.0 / total;

        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            let src = plane.clone();
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0.0f32;
                    for &(ox, oy, wt) in &weights {
                        let sx = (x as f32 + ox).round().clamp(0.0, (w - 1) as f32) as usize;
                        let sy = (y as f32 + oy).round().clamp(0.0, (h - 1) as f32) as usize;
                        sum += src[sy * w + sx] * wt;
                    }
                    plane[y * w + x] = (sum * inv_total).clamp(0.0, 1.0);
                }
            }
        }

        if let Some(alpha) = &mut planes.alpha {
            let src = alpha.clone();
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0.0f32;
                    for &(ox, oy, wt) in &weights {
                        let sx = (x as f32 + ox).round().clamp(0.0, (w - 1) as f32) as usize;
                        let sy = (y as f32 + oy).round().clamp(0.0, (h - 1) as f32) as usize;
                        sum += src[sy * w + sx] * wt;
                    }
                    alpha[y * w + x] = (sum * inv_total).clamp(0.0, 1.0);
                }
            }
        }
    }
}

// ─── DifferenceEmboss ──────────────────────────────────────────────

/// Difference-based emboss matching ImageMagick's `-emboss`.
///
/// IM's `-emboss N` operates as: blur(sigma=N) → compute directional
/// difference (shifted copy minus original) → scale + bias to [0,1].
/// This is NOT a standard 3x3 emboss kernel — it produces a relief
/// effect based on the blurred directional derivative.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct DifferenceEmboss {
    /// Blur sigma. Larger = broader emboss effect.
    pub sigma: f32,
}

impl Default for DifferenceEmboss {
    fn default() -> Self {
        Self { sigma: 1.0 }
    }
}

impl Filter for DifferenceEmboss {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        (self.sigma * 3.0).ceil() as u32 + 1
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        // Construct IM's emboss kernel: Gaussian-weighted diagonal derivative.
        // For 3x3 with sigma, the diagonal corners get -gaussian(sqrt(2), sigma),
        // center gets 1 + 2*|corner| to preserve DC (kernel sum = 1).
        let sigma2 = 2.0 * self.sigma * self.sigma;
        let corner = (-(2.0f32) / sigma2).exp(); // gaussian at distance sqrt(2)
        let center = 1.0 + 2.0 * corner;

        // Kernel layout (diagonal emboss, +45° direction):
        //  0      0     -corner
        //  0    center    0
        // -corner  0      0
        let kw = 3usize;
        let kh = 3usize;
        let rx = kw / 2;
        let ry = kh / 2;
        let coeffs = [
            0.0, 0.0, -corner,
            0.0, center, 0.0,
            -corner, 0.0, 0.0,
        ];

        let w = planes.width as usize;
        let h = planes.height as usize;

        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            let n = w * h;
            let mut dst = ctx.take_f32(n);
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0.0f32;
                    for ky in 0..kh {
                        for kx in 0..kw {
                            let sy = (y as isize + ky as isize - ry as isize)
                                .clamp(0, h as isize - 1) as usize;
                            let sx = (x as isize + kx as isize - rx as isize)
                                .clamp(0, w as isize - 1) as usize;
                            sum += plane[sy * w + sx] * coeffs[ky * kw + kx];
                        }
                    }
                    // +0.5 bias: maps derivative-centered output to [0, 1]
                    dst[y * w + x] = (sum + 0.5).clamp(0.0, 1.0);
                }
            }
            let old = core::mem::replace(plane, dst);
            ctx.return_f32(old);
        }
    }
}

// ─── Normalize (per-channel histogram stretch) ────────────────────

/// Per-channel histogram stretch matching libvips `normalise()` / sharp `normalise()`.
///
/// Finds the Nth percentile dark and light points per channel, then linearly
/// stretches each channel so those points map to 0.0 and 1.0. This is the
/// sRGB-space equivalent of Oklab `AutoLevels`.
///
/// Matches ImageMagick's `-normalize` (0.1% clip) and `-auto-level` (0% clip).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Normalize {
    /// Fraction of pixels to clip at the dark end per channel.
    /// Default: 0.001 (0.1%, matches IM's -normalize).
    pub lower: f32,
    /// Fraction of pixels to clip at the bright end per channel.
    /// Default: 0.001 (0.1%).
    pub upper: f32,
}

impl Default for Normalize {
    fn default() -> Self {
        Self {
            lower: 0.001,
            upper: 0.001,
        }
    }
}

impl Normalize {
    /// IM's `-auto-level` equivalent (no clipping).
    pub fn auto_level() -> Self {
        Self {
            lower: 0.0,
            upper: 0.0,
        }
    }
}

impl Filter for Normalize {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Rgb
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let n = planes.l.len();
        if n == 0 {
            return;
        }

        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            // Build histogram (256 bins for [0, 1])
            let mut hist = [0u32; 256];
            for &v in plane.iter() {
                let bin = (v * 255.0).clamp(0.0, 255.0) as usize;
                hist[bin] += 1;
            }

            // Find lower percentile
            let lower_count = (n as f32 * self.lower) as u32;
            let mut cumulative = 0u32;
            let mut low_bin = 0usize;
            for (i, &count) in hist.iter().enumerate() {
                cumulative += count;
                if cumulative > lower_count {
                    low_bin = i;
                    break;
                }
            }

            // Find upper percentile
            let upper_count = (n as f32 * self.upper) as u32;
            cumulative = 0;
            let mut high_bin = 255usize;
            for i in (0..256).rev() {
                cumulative += hist[i];
                if cumulative > upper_count {
                    high_bin = i;
                    break;
                }
            }

            if high_bin <= low_bin {
                continue; // flat channel, nothing to stretch
            }

            let low_val = low_bin as f32 / 255.0;
            let high_val = high_bin as f32 / 255.0;
            let range = high_val - low_val;
            let inv_range = 1.0 / range;

            for v in plane.iter_mut() {
                *v = ((*v - low_val) * inv_range).clamp(0.0, 1.0);
            }
        }
    }
}

// ─── CLAHE (Contrast Limited Adaptive Histogram Equalization) ──────

/// CLAHE — Contrast Limited Adaptive Histogram Equalization.
///
/// Splits the image into tiles, equalizes histograms per-tile with a
/// clip limit, then bilinear-interpolates between tiles. Produces
/// locally-adaptive contrast enhancement without the artifacts of
/// global histogram equalization.
///
/// Matches ImageMagick's `-clahe WxH+bins+slope` and sharp's
/// `clahe({width, height, maxSlope})`.
///
/// Operates on the first plane (L in Oklab, R in sRGB) by default.
/// In sRGB mode, apply to each channel separately or convert to a
/// luminance-based space first.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Clahe {
    /// Tile width in pixels. Default: 8.
    pub tile_width: u32,
    /// Tile height in pixels. Default: 8.
    pub tile_height: u32,
    /// Number of histogram bins. Default: 256.
    pub bins: u32,
    /// Clip limit (max slope). 1.0 = no clipping (standard HE),
    /// 3.0 = moderate CLAHE. Default: 3.0.
    pub clip_limit: f32,
}

impl Default for Clahe {
    fn default() -> Self {
        Self {
            tile_width: 8,
            tile_height: 8,
            bins: 256,
            clip_limit: 3.0,
        }
    }
}

impl Filter for Clahe {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_ONLY
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        self.tile_width.max(self.tile_height)
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        let w = planes.width as usize;
        let h = planes.height as usize;
        let tw = self.tile_width as usize;
        let th = self.tile_height as usize;
        let bins = self.bins as usize;
        let n = w * h;

        if tw == 0 || th == 0 || bins == 0 {
            return;
        }

        // Number of tiles (round up)
        let nx = (w + tw - 1) / tw;
        let ny = (h + th - 1) / th;

        // Compute CDF for each tile
        // cdfs[ty][tx][bin] = cumulative fraction
        let mut cdfs = ctx.take_f32(nx * ny * bins);

        for ty in 0..ny {
            for tx in 0..nx {
                let x0 = tx * tw;
                let y0 = ty * th;
                let x1 = (x0 + tw).min(w);
                let y1 = (y0 + th).min(h);
                let tile_pixels = (x1 - x0) * (y1 - y0);

                // Build histogram
                let cdf_offset = (ty * nx + tx) * bins;
                let cdf = &mut cdfs[cdf_offset..cdf_offset + bins];
                cdf.fill(0.0);

                for y in y0..y1 {
                    for x in x0..x1 {
                        let v = planes.l[y * w + x];
                        let bin = ((v * (bins - 1) as f32) as usize).min(bins - 1);
                        cdf[bin] += 1.0;
                    }
                }

                // Apply clip limit
                if self.clip_limit > 1.0 {
                    let limit = (self.clip_limit * tile_pixels as f32 / bins as f32).max(1.0);
                    let mut excess = 0.0f32;
                    for c in cdf.iter_mut() {
                        if *c > limit {
                            excess += *c - limit;
                            *c = limit;
                        }
                    }
                    // Redistribute excess evenly
                    let redistrib = excess / bins as f32;
                    for c in cdf.iter_mut() {
                        *c += redistrib;
                    }
                }

                // Convert histogram to CDF
                let mut cumulative = 0.0f32;
                let inv_pixels = 1.0 / tile_pixels as f32;
                for c in cdf.iter_mut() {
                    cumulative += *c;
                    *c = cumulative * inv_pixels;
                }
            }
        }

        // Apply CLAHE with bilinear interpolation between tiles
        let mut dst = ctx.take_f32(n);

        for y in 0..h {
            for x in 0..w {
                let v = planes.l[y * w + x];
                let bin = ((v * (bins - 1) as f32) as usize).min(bins - 1);

                // Tile coordinates (center of tile)
                let ftx = (x as f32 + 0.5) / tw as f32 - 0.5;
                let fty = (y as f32 + 0.5) / th as f32 - 0.5;

                let tx0 = (ftx.floor() as isize).clamp(0, nx as isize - 1) as usize;
                let ty0 = (fty.floor() as isize).clamp(0, ny as isize - 1) as usize;
                let tx1 = (tx0 + 1).min(nx - 1);
                let ty1 = (ty0 + 1).min(ny - 1);

                let fx = (ftx - tx0 as f32).clamp(0.0, 1.0);
                let fy = (fty - ty0 as f32).clamp(0.0, 1.0);

                // Bilinear interpolation of CDF values
                let c00 = cdfs[(ty0 * nx + tx0) * bins + bin];
                let c10 = cdfs[(ty0 * nx + tx1) * bins + bin];
                let c01 = cdfs[(ty1 * nx + tx0) * bins + bin];
                let c11 = cdfs[(ty1 * nx + tx1) * bins + bin];

                let top = c00 + (c10 - c00) * fx;
                let bot = c01 + (c11 - c01) * fx;
                dst[y * w + x] = (top + (bot - top) * fy).clamp(0.0, 1.0);
            }
        }

        ctx.return_f32(cdfs);
        let old = core::mem::replace(&mut planes.l, dst);
        ctx.return_f32(old);
    }
}

// ─── LaplacianEdge ─────────────────────────────────────────────────

/// Laplacian edge detection matching ImageMagick's `-edge`.
///
/// IM's `-edge N` constructs a (2N+1)×(2N+1) kernel with all -1s and
/// center = count_of_neighbors. This is an isotropic Laplacian that
/// detects edges in all directions. The output is the absolute value
/// of the convolution, clamped to [0, 1].
///
/// Unlike our Oklab `EdgeDetect` (Sobel gradient on L only), this
/// operates on all channels and uses a different kernel shape.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct LaplacianEdge {
    /// Kernel radius. 1 = 3×3, 2 = 5×5. Default: 1.
    pub radius: u32,
}

impl Default for LaplacianEdge {
    fn default() -> Self {
        Self { radius: 1 }
    }
}

impl Filter for LaplacianEdge {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn plane_semantics(&self) -> PlaneSemantics {
        PlaneSemantics::Any
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        self.radius
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.radius == 0 {
            return;
        }
        let w = planes.width as usize;
        let h = planes.height as usize;
        let r = self.radius as usize;
        let ksize = 2 * r + 1;
        let neighbors = (ksize * ksize - 1) as f32; // center excluded

        for plane in [&mut planes.l, &mut planes.a, &mut planes.b] {
            let n = w * h;
            let mut dst = ctx.take_f32(n);

            for y in 0..h {
                for x in 0..w {
                    let center = plane[y * w + x];
                    let mut sum = center * neighbors;
                    for ky in 0..ksize {
                        for kx in 0..ksize {
                            if ky == r && kx == r {
                                continue; // skip center
                            }
                            let sy = (y as isize + ky as isize - r as isize)
                                .clamp(0, h as isize - 1) as usize;
                            let sx = (x as isize + kx as isize - r as isize)
                                .clamp(0, w as isize - 1) as usize;
                            sum -= plane[sy * w + sx];
                        }
                    }
                    // Absolute value — edges are positive regardless of direction
                    dst[y * w + x] = sum.abs().clamp(0.0, 1.0);
                }
            }

            let old = core::mem::replace(plane, dst);
            ctx.return_f32(old);
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

    // ─── Normalize tests ──────────────────────────────────────────

    #[test]
    fn normalize_stretches_range() {
        let mut planes = OklabPlanes::new(4, 4);
        // All values in [0.3, 0.7] — should stretch to [0, 1]
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.3 + 0.4 * (i as f32 / 15.0);
        }
        Normalize { lower: 0.0, upper: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        let min_v = planes.l.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_v = planes.l.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(min_v < 0.05, "min should be near 0, got {min_v}");
        assert!(max_v > 0.95, "max should be near 1, got {max_v}");
    }

    #[test]
    fn normalize_constant_plane_unchanged() {
        let mut planes = OklabPlanes::new(4, 4);
        planes.l.fill(0.5);
        let orig = planes.l.clone();
        Normalize::default().apply(&mut planes, &mut FilterContext::new());
        // Constant plane can't be stretched — should stay the same or go to 0
        assert!(planes.l.iter().all(|&v| v == orig[0] || v == 0.0 || v == 1.0));
    }

    // ─── CLAHE tests ──────────────────────────────────────────────

    #[test]
    fn clahe_enhances_contrast() {
        let mut planes = OklabPlanes::new(32, 32);
        // Low-contrast image: all values in [0.4, 0.6]
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.4 + 0.2 * (i as f32 / (32 * 32) as f32);
        }
        let before_range = {
            let min = planes.l.iter().cloned().fold(f32::INFINITY, f32::min);
            let max = planes.l.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            max - min
        };
        let mut c = Clahe::default();
        c.tile_width = 8;
        c.tile_height = 8;
        c.apply(&mut planes, &mut FilterContext::new());
        let after_range = {
            let min = planes.l.iter().cloned().fold(f32::INFINITY, f32::min);
            let max = planes.l.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            max - min
        };
        assert!(
            after_range > before_range * 1.5,
            "CLAHE should increase range: before={before_range:.3}, after={after_range:.3}"
        );
    }

    #[test]
    fn clahe_constant_plane_stable() {
        let mut planes = OklabPlanes::new(16, 16);
        planes.l.fill(0.5);
        Clahe::default().apply(&mut planes, &mut FilterContext::new());
        // Constant plane: CLAHE should map everything to the same CDF value
        let first = planes.l[0];
        for &v in &planes.l {
            assert!(
                (v - first).abs() < 0.01,
                "constant plane should stay uniform: {v} vs {first}"
            );
        }
    }
}
