//! Classical segmentation masks from Oklab planes.
//!
//! Produces soft float masks (0.0–1.0) based on physical/optical properties
//! of the image. These masks enable selective filter application — for example,
//! sharpening only textured regions, or adjusting exposure only in shadows.
//!
//! All masks use smooth transitions (no hard binary edges) for natural blending.
//!
//! # Available masks
//!
//! | Mask | Basis |
//! |------|-------|
//! | **Sky** | Bright, blue-ish, low variance, weighted toward upper frame |
//! | **Shadow** | Low luminance with smooth ramp |
//! | **Highlight** | High luminance with smooth ramp |
//! | **Midtone** | Bell curve centered at L=0.5 |
//! | **High-texture** | High local variance (edges, detail, foliage, fabric) |
//! | **Smooth-region** | Low local variance (inverse of texture) |
//! | **Saturated-color** | High Oklab chroma |
//! | **Foliage** | Green hue angle range in Oklab (chlorophyll reflectance) |
//!
//! # Usage
//!
//! ```
//! use zenfilters::{FilterContext, OklabPlanes};
//! use zenfilters::segment::{SegmentMasks, SegmentConfig};
//!
//! let planes = OklabPlanes::new(64, 64);
//! let mut ctx = FilterContext::new();
//! let config = SegmentConfig::default();
//! let masks = SegmentMasks::compute(&planes, &config, &mut ctx);
//!
//! // Use individual masks for selective processing
//! let shadow_mask = &masks.shadow;
//! let texture_mask = &masks.high_texture;
//! ```

use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;
use crate::planes::OklabPlanes;
use crate::prelude::*;

/// Smoothstep: 0 when x <= edge0, 1 when x >= edge1, smooth cubic in between.
#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Configuration for segment mask computation.
///
/// All thresholds are in Oklab space (L in 0–1, a/b roughly -0.5–0.5).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SegmentConfig {
    /// Sigma for local statistics (mean/variance). Default: 10.0.
    pub sigma: f32,

    // --- Sky ---
    /// Minimum L for sky detection. Default: 0.55.
    pub sky_l_min: f32,
    /// Maximum b (blue-ish means negative b) for sky. Default: -0.01.
    pub sky_b_max: f32,
    /// Maximum local L variance for sky (smooth regions only). Default: 0.005.
    pub sky_var_max: f32,

    // --- Shadow ---
    /// L below this is fully shadow. Default: 0.15.
    pub shadow_lo: f32,
    /// L above this is zero shadow. Default: 0.35.
    pub shadow_hi: f32,

    // --- Highlight ---
    /// L below this is zero highlight. Default: 0.65.
    pub highlight_lo: f32,
    /// L above this is fully highlight. Default: 0.85.
    pub highlight_hi: f32,

    // --- Midtone ---
    /// Center of the midtone bell curve. Default: 0.5.
    pub midtone_center: f32,
    /// Width (standard deviation) of the midtone bell. Default: 0.2.
    pub midtone_width: f32,

    // --- Texture ---
    /// Variance value that maps to 0.5 in the texture sigmoid. Default: 0.01.
    pub texture_midpoint: f32,
    /// Steepness of the texture sigmoid. Default: 200.0.
    pub texture_steepness: f32,

    // --- Saturated color ---
    /// Chroma below this is zero saturation mask. Default: 0.04.
    pub saturated_lo: f32,
    /// Chroma above this is full saturation mask. Default: 0.12.
    pub saturated_hi: f32,

    // --- Foliage ---
    /// Green hue angle lower bound (radians). Default: 1.8.
    pub foliage_hue_lo: f32,
    /// Green hue angle upper bound (radians). Default: 2.8.
    pub foliage_hue_hi: f32,
    /// Smooth falloff width at hue boundaries (radians). Default: 0.2.
    pub foliage_hue_falloff: f32,
    /// Minimum chroma for foliage detection. Default: 0.03.
    pub foliage_chroma_min: f32,
}

impl Default for SegmentConfig {
    fn default() -> Self {
        Self {
            sigma: 10.0,
            sky_l_min: 0.55,
            sky_b_max: -0.01,
            sky_var_max: 0.005,
            shadow_lo: 0.15,
            shadow_hi: 0.35,
            highlight_lo: 0.65,
            highlight_hi: 0.85,
            midtone_center: 0.5,
            midtone_width: 0.2,
            texture_midpoint: 0.01,
            texture_steepness: 200.0,
            saturated_lo: 0.04,
            saturated_hi: 0.12,
            foliage_hue_lo: 1.8,
            foliage_hue_hi: 2.8,
            foliage_hue_falloff: 0.2,
            foliage_chroma_min: 0.03,
        }
    }
}

/// All computed segmentation masks for an image.
///
/// Each mask is a `Vec<f32>` with the same dimensions as the input planes
/// (width * height elements, row-major). Values are in 0.0–1.0 with smooth
/// transitions.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SegmentMasks {
    /// Bright, blue-ish, smooth, upper-frame-weighted sky regions.
    pub sky: Vec<f32>,
    /// Low-luminance regions (smooth ramp from shadow_lo to shadow_hi).
    pub shadow: Vec<f32>,
    /// High-luminance regions (smooth ramp from highlight_lo to highlight_hi).
    pub highlight: Vec<f32>,
    /// Midtone regions (bell curve centered at midtone_center).
    pub midtone: Vec<f32>,
    /// High local variance: edges, detail, foliage texture, fabric.
    pub high_texture: Vec<f32>,
    /// Low local variance: smooth gradients, sky, skin. Inverse of high_texture.
    pub smooth_region: Vec<f32>,
    /// High Oklab chroma (saturated colors).
    pub saturated_color: Vec<f32>,
    /// Green hue angle range in Oklab (chlorophyll reflectance band).
    pub foliage: Vec<f32>,
}

impl SegmentMasks {
    /// Compute all segmentation masks from Oklab planes.
    ///
    /// This shares intermediate results (local mean, local variance, chroma)
    /// across masks for efficiency. The blur kernel is computed once at the
    /// configured sigma and reused for all local statistics.
    pub fn compute(planes: &OklabPlanes, config: &SegmentConfig, ctx: &mut FilterContext) -> Self {
        let w = planes.width;
        let h = planes.height;
        let n = planes.pixel_count();

        // --- Shared local statistics via Gaussian blur ---
        let kernel = GaussianKernel::new(config.sigma);

        // Local mean of L: blur(L)
        let mut local_mean = ctx.take_f32(n);
        gaussian_blur_plane(&planes.l, &mut local_mean, w, h, &kernel, ctx);

        // Local mean of L^2: blur(L^2) — needed for variance
        let mut l_squared = ctx.take_f32(n);
        for i in 0..n {
            l_squared[i] = planes.l[i] * planes.l[i];
        }
        let mut local_mean_sq = ctx.take_f32(n);
        gaussian_blur_plane(&l_squared, &mut local_mean_sq, w, h, &kernel, ctx);
        ctx.return_f32(l_squared);

        // Local variance: var = blur(L^2) - blur(L)^2
        let mut local_var = ctx.take_f32(n);
        for i in 0..n {
            local_var[i] = (local_mean_sq[i] - local_mean[i] * local_mean[i]).max(0.0);
        }
        ctx.return_f32(local_mean_sq);

        // --- Per-pixel chroma: sqrt(a^2 + b^2) ---
        let mut chroma = ctx.take_f32(n);
        for i in 0..n {
            chroma[i] = (planes.a[i] * planes.a[i] + planes.b[i] * planes.b[i]).sqrt();
        }

        // --- Shadow mask ---
        let mut shadow = Vec::with_capacity(n);
        for i in 0..n {
            // Smooth ramp: 1.0 at shadow_lo, 0.0 at shadow_hi
            shadow.push(smoothstep(config.shadow_hi, config.shadow_lo, planes.l[i]));
        }

        // --- Highlight mask ---
        let mut highlight = Vec::with_capacity(n);
        for i in 0..n {
            highlight.push(smoothstep(
                config.highlight_lo,
                config.highlight_hi,
                planes.l[i],
            ));
        }

        // --- Midtone mask (bell curve) ---
        let mut midtone = Vec::with_capacity(n);
        let inv_2w2 = 1.0 / (2.0 * config.midtone_width * config.midtone_width);
        for i in 0..n {
            let d = planes.l[i] - config.midtone_center;
            midtone.push((-d * d * inv_2w2).exp());
        }

        // --- High-texture mask (sigmoid on variance) ---
        let mut high_texture = Vec::with_capacity(n);
        let steepness = config.texture_steepness;
        let midpoint = config.texture_midpoint;
        for i in 0..n {
            // Logistic sigmoid: 1 / (1 + exp(-steepness * (var - midpoint)))
            let x = steepness * (local_var[i] - midpoint);
            // Clamp argument to avoid overflow in exp
            let sig = if x > 15.0 {
                1.0
            } else if x < -15.0 {
                0.0
            } else {
                1.0 / (1.0 + (-x).exp())
            };
            high_texture.push(sig);
        }

        // --- Smooth-region mask (inverse of texture) ---
        let mut smooth_region = Vec::with_capacity(n);
        for i in 0..n {
            smooth_region.push(1.0 - high_texture[i]);
        }

        // --- Saturated-color mask ---
        let mut saturated_color = Vec::with_capacity(n);
        for i in 0..n {
            saturated_color.push(smoothstep(
                config.saturated_lo,
                config.saturated_hi,
                chroma[i],
            ));
        }

        // --- Foliage mask (green hue angle in Oklab) ---
        let mut foliage = Vec::with_capacity(n);
        let hue_lo = config.foliage_hue_lo;
        let hue_hi = config.foliage_hue_hi;
        let falloff = config.foliage_hue_falloff;
        let chroma_min = config.foliage_chroma_min;
        for i in 0..n {
            // atan2(b, a) gives hue angle in Oklab
            let hue = planes.b[i].atan2(planes.a[i]);
            // Shift negative angles to 0..2*PI
            let hue = if hue < 0.0 {
                hue + core::f32::consts::TAU
            } else {
                hue
            };

            // Smooth falloff at hue boundaries
            let hue_weight = smoothstep(hue_lo - falloff, hue_lo, hue)
                * smoothstep(hue_hi + falloff, hue_hi, hue);

            // Require minimum chroma (achromatic pixels have undefined hue)
            let chroma_weight = smoothstep(0.0, chroma_min, chroma[i]);

            foliage.push(hue_weight * chroma_weight);
        }

        // --- Sky mask ---
        // Combines: bright L, blue-ish b, low variance, upper-frame weighting
        let mut sky = Vec::with_capacity(n);
        let height_f = h as f32;
        for y in 0..h {
            let y_weight = smoothstep(0.7, 0.0, y as f32 / height_f);
            let row_start = (y as usize) * (w as usize);
            for x in 0..w {
                let i = row_start + x as usize;
                let l_weight = smoothstep(config.sky_l_min - 0.1, config.sky_l_min, planes.l[i]);
                let b_weight = smoothstep(config.sky_b_max + 0.02, config.sky_b_max, planes.b[i]);
                let var_weight = smoothstep(config.sky_var_max * 2.0, 0.0, local_var[i]);
                sky.push(l_weight * b_weight * var_weight * y_weight);
            }
        }

        // Return scratch buffers
        ctx.return_f32(chroma);
        ctx.return_f32(local_var);
        ctx.return_f32(local_mean);

        Self {
            sky,
            shadow,
            highlight,
            midtone,
            high_texture,
            smooth_region,
            saturated_color,
            foliage,
        }
    }

    /// Compute only the shadow mask (avoids computing unused masks).
    pub fn shadow(planes: &OklabPlanes, config: &SegmentConfig) -> Vec<f32> {
        let n = planes.pixel_count();
        let mut mask = Vec::with_capacity(n);
        for i in 0..n {
            mask.push(smoothstep(config.shadow_hi, config.shadow_lo, planes.l[i]));
        }
        mask
    }

    /// Compute only the highlight mask.
    pub fn highlight(planes: &OklabPlanes, config: &SegmentConfig) -> Vec<f32> {
        let n = planes.pixel_count();
        let mut mask = Vec::with_capacity(n);
        for i in 0..n {
            mask.push(smoothstep(
                config.highlight_lo,
                config.highlight_hi,
                planes.l[i],
            ));
        }
        mask
    }

    /// Compute only the midtone mask.
    pub fn midtone(planes: &OklabPlanes, config: &SegmentConfig) -> Vec<f32> {
        let n = planes.pixel_count();
        let inv_2w2 = 1.0 / (2.0 * config.midtone_width * config.midtone_width);
        let mut mask = Vec::with_capacity(n);
        for i in 0..n {
            let d = planes.l[i] - config.midtone_center;
            mask.push((-d * d * inv_2w2).exp());
        }
        mask
    }

    /// Compute only the high-texture mask.
    pub fn high_texture(
        planes: &OklabPlanes,
        config: &SegmentConfig,
        ctx: &mut FilterContext,
    ) -> Vec<f32> {
        let w = planes.width;
        let h = planes.height;
        let n = planes.pixel_count();
        let kernel = GaussianKernel::new(config.sigma);

        let mut local_mean = ctx.take_f32(n);
        gaussian_blur_plane(&planes.l, &mut local_mean, w, h, &kernel, ctx);

        let mut l_squared = ctx.take_f32(n);
        for i in 0..n {
            l_squared[i] = planes.l[i] * planes.l[i];
        }
        let mut local_mean_sq = ctx.take_f32(n);
        gaussian_blur_plane(&l_squared, &mut local_mean_sq, w, h, &kernel, ctx);
        ctx.return_f32(l_squared);

        let steepness = config.texture_steepness;
        let midpoint = config.texture_midpoint;
        let mut mask = Vec::with_capacity(n);
        for i in 0..n {
            let var = (local_mean_sq[i] - local_mean[i] * local_mean[i]).max(0.0);
            let x = steepness * (var - midpoint);
            let sig = if x > 15.0 {
                1.0
            } else if x < -15.0 {
                0.0
            } else {
                1.0 / (1.0 + (-x).exp())
            };
            mask.push(sig);
        }

        ctx.return_f32(local_mean_sq);
        ctx.return_f32(local_mean);
        mask
    }

    /// Compute only the smooth-region mask (inverse of high-texture).
    pub fn smooth_region(
        planes: &OklabPlanes,
        config: &SegmentConfig,
        ctx: &mut FilterContext,
    ) -> Vec<f32> {
        let mut mask = Self::high_texture(planes, config, ctx);
        for v in &mut mask {
            *v = 1.0 - *v;
        }
        mask
    }

    /// Compute only the saturated-color mask.
    pub fn saturated_color(planes: &OklabPlanes, config: &SegmentConfig) -> Vec<f32> {
        let n = planes.pixel_count();
        let mut mask = Vec::with_capacity(n);
        for i in 0..n {
            let chroma = (planes.a[i] * planes.a[i] + planes.b[i] * planes.b[i]).sqrt();
            mask.push(smoothstep(config.saturated_lo, config.saturated_hi, chroma));
        }
        mask
    }

    /// Compute only the foliage mask.
    pub fn foliage(planes: &OklabPlanes, config: &SegmentConfig) -> Vec<f32> {
        let n = planes.pixel_count();
        let hue_lo = config.foliage_hue_lo;
        let hue_hi = config.foliage_hue_hi;
        let falloff = config.foliage_hue_falloff;
        let chroma_min = config.foliage_chroma_min;

        let mut mask = Vec::with_capacity(n);
        for i in 0..n {
            let chroma = (planes.a[i] * planes.a[i] + planes.b[i] * planes.b[i]).sqrt();
            let hue = planes.b[i].atan2(planes.a[i]);
            let hue = if hue < 0.0 {
                hue + core::f32::consts::TAU
            } else {
                hue
            };

            let hue_weight = smoothstep(hue_lo - falloff, hue_lo, hue)
                * smoothstep(hue_hi + falloff, hue_hi, hue);
            let chroma_weight = smoothstep(0.0, chroma_min, chroma);

            mask.push(hue_weight * chroma_weight);
        }
        mask
    }

    /// Compute only the sky mask.
    pub fn sky(planes: &OklabPlanes, config: &SegmentConfig, ctx: &mut FilterContext) -> Vec<f32> {
        let w = planes.width;
        let h = planes.height;
        let n = planes.pixel_count();
        let kernel = GaussianKernel::new(config.sigma);

        // Local variance of L
        let mut local_mean = ctx.take_f32(n);
        gaussian_blur_plane(&planes.l, &mut local_mean, w, h, &kernel, ctx);

        let mut l_squared = ctx.take_f32(n);
        for i in 0..n {
            l_squared[i] = planes.l[i] * planes.l[i];
        }
        let mut local_mean_sq = ctx.take_f32(n);
        gaussian_blur_plane(&l_squared, &mut local_mean_sq, w, h, &kernel, ctx);
        ctx.return_f32(l_squared);

        let mut local_var = ctx.take_f32(n);
        for i in 0..n {
            local_var[i] = (local_mean_sq[i] - local_mean[i] * local_mean[i]).max(0.0);
        }
        ctx.return_f32(local_mean_sq);
        ctx.return_f32(local_mean);

        let height_f = h as f32;
        let mut mask = Vec::with_capacity(n);
        for y in 0..h {
            let y_weight = smoothstep(0.7, 0.0, y as f32 / height_f);
            let row_start = (y as usize) * (w as usize);
            for x in 0..w {
                let i = row_start + x as usize;
                let l_weight = smoothstep(config.sky_l_min - 0.1, config.sky_l_min, planes.l[i]);
                let b_weight = smoothstep(config.sky_b_max + 0.02, config.sky_b_max, planes.b[i]);
                let var_weight = smoothstep(config.sky_var_max * 2.0, 0.0, local_var[i]);
                mask.push(l_weight * b_weight * var_weight * y_weight);
            }
        }

        ctx.return_f32(local_var);
        mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;
    use crate::planes::OklabPlanes;

    fn make_uniform_planes(w: u32, h: u32, l: f32, a: f32, b: f32) -> OklabPlanes {
        let n = (w as usize) * (h as usize);
        OklabPlanes {
            width: w,
            height: h,
            l: vec![l; n],
            a: vec![a; n],
            b: vec![b; n],
            alpha: None,
        }
    }

    // --- Smoothstep unit tests ---

    #[test]
    fn smoothstep_boundaries() {
        assert!((smoothstep(0.0, 1.0, -0.1) - 0.0).abs() < 1e-6);
        assert!((smoothstep(0.0, 1.0, 0.0) - 0.0).abs() < 1e-6);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert!((smoothstep(0.0, 1.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((smoothstep(0.0, 1.0, 1.1) - 1.0).abs() < 1e-6);
    }

    // --- Shadow mask ---

    #[test]
    fn shadow_mask_dark_pixels() {
        let planes = make_uniform_planes(32, 32, 0.1, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::shadow(&planes, &config);
        // L=0.1 is well below shadow_lo=0.15, so should be ~1.0
        for &v in &mask {
            assert!(v > 0.95, "dark pixel shadow mask should be ~1.0, got {v}");
        }
    }

    #[test]
    fn shadow_mask_bright_pixels() {
        let planes = make_uniform_planes(32, 32, 0.8, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::shadow(&planes, &config);
        for &v in &mask {
            assert!(v < 0.01, "bright pixel shadow mask should be ~0.0, got {v}");
        }
    }

    #[test]
    fn shadow_mask_transition() {
        // At L=0.25 (midpoint of default shadow range 0.15–0.35), should be ~0.5
        let planes = make_uniform_planes(32, 32, 0.25, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::shadow(&planes, &config);
        for &v in &mask {
            assert!(
                (v - 0.5).abs() < 0.15,
                "shadow transition at L=0.25 should be ~0.5, got {v}"
            );
        }
    }

    // --- Highlight mask ---

    #[test]
    fn highlight_mask_bright_pixels() {
        let planes = make_uniform_planes(32, 32, 0.95, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::highlight(&planes, &config);
        for &v in &mask {
            assert!(
                v > 0.95,
                "very bright pixel highlight should be ~1.0, got {v}"
            );
        }
    }

    #[test]
    fn highlight_mask_dark_pixels() {
        let planes = make_uniform_planes(32, 32, 0.3, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::highlight(&planes, &config);
        for &v in &mask {
            assert!(v < 0.01, "dark pixel highlight should be ~0.0, got {v}");
        }
    }

    // --- Midtone mask ---

    #[test]
    fn midtone_mask_center() {
        let planes = make_uniform_planes(32, 32, 0.5, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::midtone(&planes, &config);
        for &v in &mask {
            assert!(
                (v - 1.0).abs() < 1e-4,
                "midtone at L=0.5 should be ~1.0, got {v}"
            );
        }
    }

    #[test]
    fn midtone_mask_extremes() {
        // At L=0.0 and L=1.0, midtone bell should be near zero
        let config = SegmentConfig::default();

        let planes_dark = make_uniform_planes(32, 32, 0.0, 0.0, 0.0);
        let mask_dark = SegmentMasks::midtone(&planes_dark, &config);
        for &v in &mask_dark {
            assert!(v < 0.05, "midtone at L=0 should be near 0, got {v}");
        }

        let planes_bright = make_uniform_planes(32, 32, 1.0, 0.0, 0.0);
        let mask_bright = SegmentMasks::midtone(&planes_bright, &config);
        for &v in &mask_bright {
            assert!(v < 0.05, "midtone at L=1 should be near 0, got {v}");
        }
    }

    // --- Texture masks ---

    #[test]
    fn texture_mask_uniform_is_low() {
        let planes = make_uniform_planes(64, 64, 0.5, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mut ctx = FilterContext::new();
        let mask = SegmentMasks::high_texture(&planes, &config, &mut ctx);
        // Uniform image has zero local variance -> texture should be ~0
        for &v in &mask {
            assert!(v < 0.3, "uniform image texture mask should be low, got {v}");
        }
    }

    #[test]
    fn texture_mask_checkerboard_is_high() {
        let w = 64u32;
        let h = 64u32;
        let n = (w * h) as usize;
        let mut planes = OklabPlanes::new(w, h);
        // Strong checkerboard pattern (high local variance)
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize) * (w as usize) + x as usize;
                planes.l[i] = if (x + y) % 2 == 0 { 0.9 } else { 0.1 };
            }
        }
        let config = SegmentConfig::default();
        let mut ctx = FilterContext::new();
        let mask = SegmentMasks::high_texture(&planes, &config, &mut ctx);

        // Interior pixels should have high texture value
        let interior_mean: f32 = mask[n / 4..3 * n / 4].iter().sum::<f32>() / (n / 2) as f32;
        assert!(
            interior_mean > 0.5,
            "checkerboard texture should be high, mean = {interior_mean}"
        );
    }

    #[test]
    fn smooth_region_is_inverse_of_texture() {
        let w = 64u32;
        let h = 64u32;
        let mut planes = OklabPlanes::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize) * (w as usize) + x as usize;
                planes.l[i] = if (x + y) % 2 == 0 { 0.9 } else { 0.1 };
            }
        }
        let config = SegmentConfig::default();
        let mut ctx = FilterContext::new();
        let texture = SegmentMasks::high_texture(&planes, &config, &mut ctx);
        let smooth = SegmentMasks::smooth_region(&planes, &config, &mut ctx);

        for (i, (&t, &s)) in texture.iter().zip(smooth.iter()).enumerate() {
            assert!(
                ((t + s) - 1.0).abs() < 1e-5,
                "texture + smooth should == 1.0 at pixel {i}: {t} + {s}"
            );
        }
    }

    // --- Saturated-color mask ---

    #[test]
    fn saturated_mask_achromatic() {
        let planes = make_uniform_planes(32, 32, 0.5, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::saturated_color(&planes, &config);
        for &v in &mask {
            assert!(
                v < 0.01,
                "achromatic should have zero saturation mask, got {v}"
            );
        }
    }

    #[test]
    fn saturated_mask_vivid_color() {
        // a=0.15, b=0.0 -> chroma = 0.15, well above saturated_hi=0.12
        let planes = make_uniform_planes(32, 32, 0.5, 0.15, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::saturated_color(&planes, &config);
        for &v in &mask {
            assert!(
                v > 0.95,
                "vivid color saturation mask should be ~1.0, got {v}"
            );
        }
    }

    // --- Foliage mask ---

    #[test]
    fn foliage_mask_green_hue() {
        // Green in Oklab: negative a (green-red axis), slightly positive b
        // atan2(b, a) for a=-0.1, b=0.05 = atan2(0.05, -0.1) ~ 2.68 rad (in range 1.8–2.8)
        let planes = make_uniform_planes(32, 32, 0.5, -0.1, 0.05);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::foliage(&planes, &config);
        // chroma = sqrt(0.01 + 0.0025) = ~0.112, above foliage_chroma_min=0.03
        for &v in &mask {
            assert!(
                v > 0.7,
                "green-hue pixel should have high foliage mask, got {v}"
            );
        }
    }

    #[test]
    fn foliage_mask_red_hue() {
        // Red in Oklab: positive a, near zero b
        // atan2(0.0, 0.15) = 0.0 rad (far from 1.8–2.8 range)
        let planes = make_uniform_planes(32, 32, 0.5, 0.15, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::foliage(&planes, &config);
        for &v in &mask {
            assert!(
                v < 0.01,
                "red-hue pixel should have zero foliage mask, got {v}"
            );
        }
    }

    #[test]
    fn foliage_mask_achromatic() {
        let planes = make_uniform_planes(32, 32, 0.5, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mask = SegmentMasks::foliage(&planes, &config);
        for &v in &mask {
            assert!(
                v < 0.01,
                "achromatic pixel should have zero foliage mask, got {v}"
            );
        }
    }

    // --- Sky mask ---

    #[test]
    fn sky_mask_blue_bright_smooth_top() {
        // Simulate sky: bright, blue (b < -0.01), uniform, at top of frame
        let w = 64u32;
        let h = 64u32;
        let n = (w * h) as usize;
        let planes = OklabPlanes {
            width: w,
            height: h,
            l: vec![0.75; n],  // bright
            a: vec![0.0; n],   // neutral a
            b: vec![-0.05; n], // blue
            alpha: None,
        };
        let config = SegmentConfig::default();
        let mut ctx = FilterContext::new();
        let mask = SegmentMasks::sky(&planes, &config, &mut ctx);

        // Top rows should have high sky mask
        let top_row_mean: f32 = mask[0..w as usize].iter().sum::<f32>() / w as f32;
        assert!(
            top_row_mean > 0.3,
            "top row of blue-bright image should have high sky mask, got {top_row_mean}"
        );

        // Bottom rows should have lower sky mask (y_weight drops off)
        let last_row_start = ((h - 1) as usize) * (w as usize);
        let bottom_row_mean: f32 = mask[last_row_start..last_row_start + w as usize]
            .iter()
            .sum::<f32>()
            / w as f32;
        assert!(
            bottom_row_mean < top_row_mean,
            "bottom row sky mask ({bottom_row_mean}) should be less than top ({top_row_mean})"
        );
    }

    #[test]
    fn sky_mask_red_is_zero() {
        // Red tones should not trigger sky mask
        let planes = make_uniform_planes(64, 64, 0.7, 0.1, 0.05);
        let config = SegmentConfig::default();
        let mut ctx = FilterContext::new();
        let mask = SegmentMasks::sky(&planes, &config, &mut ctx);
        for &v in &mask {
            assert!(
                v < 0.01,
                "red-toned pixel should have zero sky mask, got {v}"
            );
        }
    }

    #[test]
    fn sky_mask_dark_is_zero() {
        // Dark blue should not trigger sky mask (not bright enough)
        let planes = make_uniform_planes(64, 64, 0.2, 0.0, -0.1);
        let config = SegmentConfig::default();
        let mut ctx = FilterContext::new();
        let mask = SegmentMasks::sky(&planes, &config, &mut ctx);
        for &v in &mask {
            assert!(v < 0.01, "dark pixel should have zero sky mask, got {v}");
        }
    }

    // --- Full compute ---

    #[test]
    fn compute_all_masks_correct_size() {
        let w = 48u32;
        let h = 32u32;
        let n = (w * h) as usize;
        let planes = make_uniform_planes(w, h, 0.5, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mut ctx = FilterContext::new();
        let masks = SegmentMasks::compute(&planes, &config, &mut ctx);

        assert_eq!(masks.sky.len(), n);
        assert_eq!(masks.shadow.len(), n);
        assert_eq!(masks.highlight.len(), n);
        assert_eq!(masks.midtone.len(), n);
        assert_eq!(masks.high_texture.len(), n);
        assert_eq!(masks.smooth_region.len(), n);
        assert_eq!(masks.saturated_color.len(), n);
        assert_eq!(masks.foliage.len(), n);
    }

    #[test]
    fn compute_all_masks_in_range() {
        let w = 48u32;
        let h = 32u32;
        let mut planes = OklabPlanes::new(w, h);
        // Varied content
        for y in 0..h {
            for x in 0..w {
                let i = (y as usize) * (w as usize) + x as usize;
                planes.l[i] = x as f32 / w as f32;
                planes.a[i] = -0.15 + 0.3 * (y as f32 / h as f32);
                planes.b[i] = -0.1 + 0.2 * (x as f32 / w as f32);
            }
        }
        let config = SegmentConfig::default();
        let mut ctx = FilterContext::new();
        let masks = SegmentMasks::compute(&planes, &config, &mut ctx);

        for (name, mask) in [
            ("sky", &masks.sky),
            ("shadow", &masks.shadow),
            ("highlight", &masks.highlight),
            ("midtone", &masks.midtone),
            ("high_texture", &masks.high_texture),
            ("smooth_region", &masks.smooth_region),
            ("saturated_color", &masks.saturated_color),
            ("foliage", &masks.foliage),
        ] {
            for (i, &v) in mask.iter().enumerate() {
                assert!(
                    (0.0..=1.0).contains(&v),
                    "{name} mask out of range at pixel {i}: {v}"
                );
            }
        }
    }

    #[test]
    fn texture_plus_smooth_equals_one_full_compute() {
        let w = 48u32;
        let h = 32u32;
        let planes = make_uniform_planes(w, h, 0.5, 0.0, 0.0);
        let config = SegmentConfig::default();
        let mut ctx = FilterContext::new();
        let masks = SegmentMasks::compute(&planes, &config, &mut ctx);
        for (i, (&t, &s)) in masks
            .high_texture
            .iter()
            .zip(masks.smooth_region.iter())
            .enumerate()
        {
            assert!(
                ((t + s) - 1.0).abs() < 1e-5,
                "texture + smooth should == 1.0 at pixel {i}: {t} + {s}"
            );
        }
    }
}
