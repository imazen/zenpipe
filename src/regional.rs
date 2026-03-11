//! Spatially-aware image comparison using tonal, chromatic, and texture region masks.
//!
//! Two complementary classification schemes:
//!
//! ## Tonal/chromatic regions (per-pixel)
//!
//! Classifies pixels by their color properties:
//!
//! **Luminance zones** (5):
//! - Deep shadows: L < 0.15
//! - Shadows: 0.15 ≤ L < 0.35
//! - Midtones: 0.35 ≤ L < 0.65
//! - Highlights: 0.65 ≤ L < 0.85
//! - Specular: L ≥ 0.85
//!
//! **Chroma zones** (4):
//! - Neutral: chroma < 0.03
//! - Low-sat: 0.03 ≤ chroma < 0.08
//! - Medium-sat: 0.08 ≤ chroma < 0.15
//! - High-sat: chroma ≥ 0.15
//!
//! **Hue zones** (6, for saturated pixels only):
//! - Warm (skin/earth): hue 0°–60°
//! - Yellow-green: hue 60°–120°
//! - Green-cyan: hue 120°–180°
//! - Cool (sky): hue 180°–260°
//! - Purple: hue 260°–320°
//! - Magenta-red: hue 320°–360°
//!
//! ## Texture regions (per-patch, spatial)
//!
//! Classifies 16×16 patches by local texture content:
//!
//! **Texture zones** (4):
//! - Smooth: low variance, low gradient (sky, skin, OOF backgrounds)
//! - Gradient: low variance, moderate gradient (soft transitions, vignettes)
//! - Textured: high variance (foliage, fabric, gravel, hair)
//! - Edge: high gradient magnitude (building edges, horizons, sharp boundaries)

use crate::planes::OklabPlanes;

/// Number of luminance zones.
pub const LUM_ZONES: usize = 5;
/// Number of chroma zones.
pub const CHROMA_ZONES: usize = 4;
/// Number of hue sectors (for saturated pixels).
pub const HUE_SECTORS: usize = 6;
/// Number of texture zones (spatial patch classification).
pub const TEXTURE_ZONES: usize = 4;
/// Patch size for texture classification (pixels per side).
pub const PATCH_SIZE: usize = 16;
/// Histogram bins per channel per region.
pub const HIST_BINS: usize = 32;

/// Luminance zone boundaries.
const LUM_BOUNDS: [f32; 4] = [0.15, 0.35, 0.65, 0.85];
/// Chroma zone boundaries.
const CHROMA_BOUNDS: [f32; 3] = [0.03, 0.08, 0.15];
/// Hue sector boundaries in degrees.
const HUE_BOUNDS: [f32; 6] = [60.0, 120.0, 180.0, 260.0, 320.0, 360.0];

/// Texture zone thresholds (empirically tuned for Oklab L in [0,1]).
/// Variance threshold: patches above this have significant texture.
const TEXTURE_VARIANCE_THRESH: f32 = 0.002;
/// Gradient threshold: patches above this have strong edges.
const TEXTURE_GRADIENT_THRESH: f32 = 0.04;
/// Gradient threshold for distinguishing gradient from smooth.
const TEXTURE_GRADIENT_LOW: f32 = 0.015;

/// Classify a patch into a texture zone given its variance and mean gradient.
#[inline]
fn texture_zone(variance: f32, mean_gradient: f32) -> usize {
    if mean_gradient >= TEXTURE_GRADIENT_THRESH {
        3 // Edge
    } else if variance >= TEXTURE_VARIANCE_THRESH {
        2 // Textured
    } else if mean_gradient >= TEXTURE_GRADIENT_LOW {
        1 // Gradient
    } else {
        0 // Smooth
    }
}

/// Per-region statistics extracted from an image.
#[derive(Clone, Debug)]
pub struct RegionalFeatures {
    // --- Tonal/chromatic (per-pixel) ---
    /// Per-luminance-zone L histograms (LUM_ZONES × HIST_BINS).
    pub lum_l_hist: [[f32; HIST_BINS]; LUM_ZONES],
    /// Per-luminance-zone chroma mean.
    pub lum_chroma_mean: [f32; LUM_ZONES],
    /// Per-luminance-zone pixel count (fraction of total).
    pub lum_fraction: [f32; LUM_ZONES],

    /// Per-chroma-zone L histograms.
    pub chroma_l_hist: [[f32; HIST_BINS]; CHROMA_ZONES],
    /// Per-chroma-zone pixel count (fraction of total).
    pub chroma_fraction: [f32; CHROMA_ZONES],

    /// Per-hue-sector a/b histograms (saturated pixels only).
    /// a histogram: HUE_SECTORS × HIST_BINS
    pub hue_a_hist: [[f32; HIST_BINS]; HUE_SECTORS],
    /// b histogram: HUE_SECTORS × HIST_BINS
    pub hue_b_hist: [[f32; HIST_BINS]; HUE_SECTORS],
    /// Per-hue-sector pixel count (fraction of total).
    pub hue_fraction: [f32; HUE_SECTORS],

    // --- Spatial texture (per-patch) ---
    /// Per-texture-zone L histograms (TEXTURE_ZONES × HIST_BINS).
    pub texture_l_hist: [[f32; HIST_BINS]; TEXTURE_ZONES],
    /// Per-texture-zone chroma histograms.
    pub texture_chroma_hist: [[f32; HIST_BINS]; TEXTURE_ZONES],
    /// Per-texture-zone mean L.
    pub texture_l_mean: [f32; TEXTURE_ZONES],
    /// Per-texture-zone mean chroma.
    pub texture_chroma_mean: [f32; TEXTURE_ZONES],
    /// Per-texture-zone patch fraction (fraction of total patches).
    pub texture_fraction: [f32; TEXTURE_ZONES],
}

impl Default for RegionalFeatures {
    fn default() -> Self {
        Self {
            lum_l_hist: [[0.0; HIST_BINS]; LUM_ZONES],
            lum_chroma_mean: [0.0; LUM_ZONES],
            lum_fraction: [0.0; LUM_ZONES],
            chroma_l_hist: [[0.0; HIST_BINS]; CHROMA_ZONES],
            chroma_fraction: [0.0; CHROMA_ZONES],
            hue_a_hist: [[0.0; HIST_BINS]; HUE_SECTORS],
            hue_b_hist: [[0.0; HIST_BINS]; HUE_SECTORS],
            hue_fraction: [0.0; HUE_SECTORS],
            texture_l_hist: [[0.0; HIST_BINS]; TEXTURE_ZONES],
            texture_chroma_hist: [[0.0; HIST_BINS]; TEXTURE_ZONES],
            texture_l_mean: [0.0; TEXTURE_ZONES],
            texture_chroma_mean: [0.0; TEXTURE_ZONES],
            texture_fraction: [0.0; TEXTURE_ZONES],
        }
    }
}

#[inline]
fn lum_zone(l: f32) -> usize {
    if l < LUM_BOUNDS[0] {
        0
    } else if l < LUM_BOUNDS[1] {
        1
    } else if l < LUM_BOUNDS[2] {
        2
    } else if l < LUM_BOUNDS[3] {
        3
    } else {
        4
    }
}

#[inline]
fn chroma_zone(chroma: f32) -> usize {
    if chroma < CHROMA_BOUNDS[0] {
        0
    } else if chroma < CHROMA_BOUNDS[1] {
        1
    } else if chroma < CHROMA_BOUNDS[2] {
        2
    } else {
        3
    }
}

#[inline]
fn hue_sector(hue_deg: f32) -> usize {
    // hue_deg in [0, 360)
    if hue_deg < HUE_BOUNDS[0] {
        0
    } else if hue_deg < HUE_BOUNDS[1] {
        1
    } else if hue_deg < HUE_BOUNDS[2] {
        2
    } else if hue_deg < HUE_BOUNDS[3] {
        3
    } else if hue_deg < HUE_BOUNDS[4] {
        4
    } else {
        5
    }
}

#[inline]
fn hist_bin(val: f32, lo: f32, hi: f32) -> usize {
    let t = ((val - lo) / (hi - lo)).clamp(0.0, 1.0 - 1e-6);
    (t * HIST_BINS as f32) as usize
}

impl RegionalFeatures {
    /// Extract regional features from Oklab planes.
    pub fn extract(planes: &OklabPlanes) -> Self {
        let n = planes.pixel_count();
        let inv_n = 1.0 / n as f32;

        let mut feat = Self::default();
        let mut lum_counts = [0u32; LUM_ZONES];
        let mut lum_chroma_sum = [0.0f64; LUM_ZONES];
        let mut chroma_counts = [0u32; CHROMA_ZONES];
        let mut hue_counts = [0u32; HUE_SECTORS];

        for i in 0..n {
            let l = planes.l[i];
            let a = planes.a[i];
            let b = planes.b[i];
            let chroma = (a * a + b * b).sqrt();

            // Luminance zone
            let lz = lum_zone(l);
            lum_counts[lz] += 1;
            lum_chroma_sum[lz] += chroma as f64;
            let bin = hist_bin(l, 0.0, 1.0);
            feat.lum_l_hist[lz][bin] += 1.0;

            // Chroma zone
            let cz = chroma_zone(chroma);
            chroma_counts[cz] += 1;
            let bin = hist_bin(l, 0.0, 1.0);
            feat.chroma_l_hist[cz][bin] += 1.0;

            // Hue sector (only for saturated pixels)
            if chroma >= CHROMA_BOUNDS[0] {
                let hue_rad = b.atan2(a);
                let mut hue_deg = hue_rad.to_degrees();
                if hue_deg < 0.0 {
                    hue_deg += 360.0;
                }
                let hs = hue_sector(hue_deg);
                hue_counts[hs] += 1;
                let a_bin = hist_bin(a, -0.4, 0.4);
                let b_bin = hist_bin(b, -0.4, 0.4);
                feat.hue_a_hist[hs][a_bin] += 1.0;
                feat.hue_b_hist[hs][b_bin] += 1.0;
            }
        }

        // Normalize histograms and compute fractions
        for z in 0..LUM_ZONES {
            let c = lum_counts[z];
            feat.lum_fraction[z] = c as f32 * inv_n;
            if c > 0 {
                let inv_c = 1.0 / c as f32;
                for bin in &mut feat.lum_l_hist[z] {
                    *bin *= inv_c;
                }
                feat.lum_chroma_mean[z] = (lum_chroma_sum[z] / c as f64) as f32;
            }
        }

        for (&c, (frac, hist)) in chroma_counts.iter().zip(
            feat.chroma_fraction
                .iter_mut()
                .zip(feat.chroma_l_hist.iter_mut()),
        ) {
            *frac = c as f32 * inv_n;
            if c > 0 {
                let inv_c = 1.0 / c as f32;
                for bin in hist.iter_mut() {
                    *bin *= inv_c;
                }
            }
        }

        for (&c, (frac, (a_hist, b_hist))) in hue_counts.iter().zip(
            feat.hue_fraction
                .iter_mut()
                .zip(feat.hue_a_hist.iter_mut().zip(feat.hue_b_hist.iter_mut())),
        ) {
            *frac = c as f32 * inv_n;
            if c > 0 {
                let inv_c = 1.0 / c as f32;
                for bin in a_hist.iter_mut() {
                    *bin *= inv_c;
                }
                for bin in b_hist.iter_mut() {
                    *bin *= inv_c;
                }
            }
        }

        // --- Spatial texture classification (per-patch) ---
        extract_texture_features(planes, &mut feat);

        feat
    }
}

/// Compute per-patch texture features and accumulate into `feat`.
///
/// Divides the image into PATCH_SIZE × PATCH_SIZE tiles. For each tile:
/// 1. Compute mean L and variance of L (texture energy)
/// 2. Compute mean gradient magnitude (edge strength)
/// 3. Classify tile into texture zone
/// 4. Accumulate L and chroma histograms for that zone
fn extract_texture_features(planes: &OklabPlanes, feat: &mut RegionalFeatures) {
    let w = planes.width as usize;
    let h = planes.height as usize;

    if w < PATCH_SIZE || h < PATCH_SIZE {
        // Image too small for patch analysis — leave texture features at zero
        return;
    }

    let patches_x = w / PATCH_SIZE;
    let patches_y = h / PATCH_SIZE;
    let total_patches = patches_x * patches_y;
    if total_patches == 0 {
        return;
    }

    let mut zone_counts = [0u32; TEXTURE_ZONES];
    let mut zone_l_sum = [0.0f64; TEXTURE_ZONES];
    let mut zone_chroma_sum = [0.0f64; TEXTURE_ZONES];
    let mut zone_pixel_counts = [0u32; TEXTURE_ZONES];

    for py in 0..patches_y {
        for px in 0..patches_x {
            let x0 = px * PATCH_SIZE;
            let y0 = py * PATCH_SIZE;

            // Pass 1: compute mean L for this patch
            let mut sum_l = 0.0f64;
            let patch_n = PATCH_SIZE * PATCH_SIZE;
            for dy in 0..PATCH_SIZE {
                let row = (y0 + dy) * w;
                for dx in 0..PATCH_SIZE {
                    sum_l += planes.l[row + x0 + dx] as f64;
                }
            }
            let mean_l = (sum_l / patch_n as f64) as f32;

            // Pass 2: variance and gradient magnitude
            let mut var_sum = 0.0f64;
            let mut grad_sum = 0.0f64;
            let mut chroma_sum = 0.0f64;
            for dy in 0..PATCH_SIZE {
                let y = y0 + dy;
                let row = y * w;
                for dx in 0..PATCH_SIZE {
                    let x = x0 + dx;
                    let idx = row + x;
                    let l = planes.l[idx];

                    // Variance
                    let diff = l - mean_l;
                    var_sum += (diff * diff) as f64;

                    // Gradient (central differences, clamped at image borders)
                    let gx = if x > 0 && x + 1 < w {
                        (planes.l[idx + 1] - planes.l[idx - 1]) * 0.5
                    } else if x + 1 < w {
                        planes.l[idx + 1] - l
                    } else if x > 0 {
                        l - planes.l[idx - 1]
                    } else {
                        0.0
                    };
                    let gy = if y > 0 && y + 1 < h {
                        (planes.l[idx + w] - planes.l[idx - w]) * 0.5
                    } else if y + 1 < h {
                        planes.l[idx + w] - l
                    } else if y > 0 {
                        l - planes.l[idx - w]
                    } else {
                        0.0
                    };
                    grad_sum += (gx * gx + gy * gy).sqrt() as f64;

                    // Chroma
                    let a = planes.a[idx];
                    let b = planes.b[idx];
                    chroma_sum += (a * a + b * b).sqrt() as f64;
                }
            }

            let variance = (var_sum / patch_n as f64) as f32;
            let mean_gradient = (grad_sum / patch_n as f64) as f32;
            let mean_chroma = (chroma_sum / patch_n as f64) as f32;

            let tz = texture_zone(variance, mean_gradient);
            zone_counts[tz] += 1;
            zone_l_sum[tz] += sum_l;
            zone_chroma_sum[tz] += chroma_sum;
            zone_pixel_counts[tz] += patch_n as u32;

            // Accumulate per-pixel histograms for this zone
            for dy in 0..PATCH_SIZE {
                let row = (y0 + dy) * w;
                for dx in 0..PATCH_SIZE {
                    let idx = row + x0 + dx;
                    let l_bin = hist_bin(planes.l[idx], 0.0, 1.0);
                    feat.texture_l_hist[tz][l_bin] += 1.0;

                    let a = planes.a[idx];
                    let b = planes.b[idx];
                    let c = (a * a + b * b).sqrt();
                    let c_bin = hist_bin(c, 0.0, 0.3);
                    feat.texture_chroma_hist[tz][c_bin] += 1.0;
                }
            }

            let _ = mean_l;
            let _ = mean_chroma;
        }
    }

    // Normalize
    let inv_total = 1.0 / total_patches as f32;
    for (tz, (&count, (frac, (l_hist, (c_hist, (l_mean, c_mean)))))) in zone_counts
        .iter()
        .zip(
            feat.texture_fraction.iter_mut().zip(
                feat.texture_l_hist.iter_mut().zip(
                    feat.texture_chroma_hist.iter_mut().zip(
                        feat.texture_l_mean
                            .iter_mut()
                            .zip(feat.texture_chroma_mean.iter_mut()),
                    ),
                ),
            ),
        )
        .enumerate()
    {
        *frac = count as f32 * inv_total;
        let pc = zone_pixel_counts[tz];
        if pc > 0 {
            let inv_pc = 1.0 / pc as f32;
            for bin in l_hist.iter_mut() {
                *bin *= inv_pc;
            }
            for bin in c_hist.iter_mut() {
                *bin *= inv_pc;
            }
            *l_mean = (zone_l_sum[tz] / pc as f64) as f32;
            *c_mean = (zone_chroma_sum[tz] / pc as f64) as f32;
        }
    }
}

/// Per-region comparison scores between two images.
#[derive(Clone, Debug, Default)]
pub struct RegionalComparison {
    /// Per-luminance-zone histogram distance (0 = identical, higher = more different).
    pub lum_zone_dist: [f32; LUM_ZONES],
    /// Per-chroma-zone histogram distance.
    pub chroma_zone_dist: [f32; CHROMA_ZONES],
    /// Per-hue-sector color distance.
    pub hue_sector_dist: [f32; HUE_SECTORS],
    /// Per-texture-zone histogram distance.
    pub texture_zone_dist: [f32; TEXTURE_ZONES],
    /// Weighted aggregate distance (lower = more similar).
    pub aggregate: f32,
}

/// Human-readable labels for each zone type.
pub struct ZoneLabels {
    pub luminance: &'static [&'static str],
    pub chroma: &'static [&'static str],
    pub hue: &'static [&'static str],
    pub texture: &'static [&'static str],
}

/// Compute histogram intersection distance between two normalized histograms.
/// Returns 0.0 for identical, 1.0 for completely disjoint.
fn hist_distance(a: &[f32; HIST_BINS], b: &[f32; HIST_BINS]) -> f32 {
    let intersection: f32 = a.iter().zip(b.iter()).map(|(&x, &y)| x.min(y)).sum();
    1.0 - intersection.min(1.0)
}

/// Perceptual importance weights for luminance zones.
/// Midtones are weighted highest (most visually prominent).
const LUM_WEIGHTS: [f32; LUM_ZONES] = [0.10, 0.20, 0.35, 0.25, 0.10];

/// Perceptual importance weights for chroma zones.
/// Saturated areas draw more attention.
const CHROMA_WEIGHTS: [f32; CHROMA_ZONES] = [0.15, 0.20, 0.30, 0.35];

/// Perceptual importance weights for hue sectors.
/// Warm/skin tones weighted highest (most perceptually sensitive).
const HUE_WEIGHTS: [f32; HUE_SECTORS] = [0.30, 0.15, 0.10, 0.25, 0.10, 0.10];

/// Perceptual importance weights for texture zones.
/// Smooth areas (sky/skin) and textured areas (foliage/fabric) are most
/// important — these are where over-sharpening or over-denoising shows up.
const TEXTURE_WEIGHTS: [f32; TEXTURE_ZONES] = [0.30, 0.15, 0.35, 0.20];

impl RegionalComparison {
    /// Compare two sets of regional features.
    pub fn compare(a: &RegionalFeatures, b: &RegionalFeatures) -> Self {
        let mut result = Self::default();

        // Luminance zone distances
        let mut lum_total = 0.0f32;
        let mut lum_weight_sum = 0.0f32;
        for (z, (&lw, dist)) in LUM_WEIGHTS
            .iter()
            .zip(result.lum_zone_dist.iter_mut())
            .enumerate()
        {
            let d = hist_distance(&a.lum_l_hist[z], &b.lum_l_hist[z]);
            // Also factor in chroma mean difference
            let chroma_diff = (a.lum_chroma_mean[z] - b.lum_chroma_mean[z]).abs();
            *dist = d + chroma_diff * 2.0;
            // Weight by presence: if a zone has few pixels, reduce its influence
            let presence = (a.lum_fraction[z] + b.lum_fraction[z]) * 0.5;
            let w = lw * presence;
            lum_total += *dist * w;
            lum_weight_sum += w;
        }

        // Chroma zone distances
        let mut chroma_total = 0.0f32;
        let mut chroma_weight_sum = 0.0f32;
        for (z, (&cw, dist)) in CHROMA_WEIGHTS
            .iter()
            .zip(result.chroma_zone_dist.iter_mut())
            .enumerate()
        {
            *dist = hist_distance(&a.chroma_l_hist[z], &b.chroma_l_hist[z]);
            let presence = (a.chroma_fraction[z] + b.chroma_fraction[z]) * 0.5;
            let w = cw * presence;
            chroma_total += *dist * w;
            chroma_weight_sum += w;
        }

        // Hue sector distances
        let mut hue_total = 0.0f32;
        let mut hue_weight_sum = 0.0f32;
        for (s, (&hw, dist)) in HUE_WEIGHTS
            .iter()
            .zip(result.hue_sector_dist.iter_mut())
            .enumerate()
        {
            let a_dist = hist_distance(&a.hue_a_hist[s], &b.hue_a_hist[s]);
            let b_dist = hist_distance(&a.hue_b_hist[s], &b.hue_b_hist[s]);
            *dist = (a_dist + b_dist) * 0.5;
            let presence = (a.hue_fraction[s] + b.hue_fraction[s]) * 0.5;
            let w = hw * presence;
            hue_total += *dist * w;
            hue_weight_sum += w;
        }

        // Texture zone distances
        let mut texture_total = 0.0f32;
        let mut texture_weight_sum = 0.0f32;
        for (tz, (&tw, dist)) in TEXTURE_WEIGHTS
            .iter()
            .zip(result.texture_zone_dist.iter_mut())
            .enumerate()
        {
            let l_dist = hist_distance(&a.texture_l_hist[tz], &b.texture_l_hist[tz]);
            let c_dist = hist_distance(&a.texture_chroma_hist[tz], &b.texture_chroma_hist[tz]);
            // Also factor in mean L and chroma differences
            let l_diff = (a.texture_l_mean[tz] - b.texture_l_mean[tz]).abs();
            let c_diff = (a.texture_chroma_mean[tz] - b.texture_chroma_mean[tz]).abs();
            *dist = (l_dist + c_dist) * 0.5 + l_diff + c_diff * 2.0;
            let presence = (a.texture_fraction[tz] + b.texture_fraction[tz]) * 0.5;
            let w = tw * presence;
            texture_total += *dist * w;
            texture_weight_sum += w;
        }

        // Aggregate: weighted combination of all four dimension scores
        let lum_score = if lum_weight_sum > 1e-6 {
            lum_total / lum_weight_sum
        } else {
            0.0
        };
        let chroma_score = if chroma_weight_sum > 1e-6 {
            chroma_total / chroma_weight_sum
        } else {
            0.0
        };
        let hue_score = if hue_weight_sum > 1e-6 {
            hue_total / hue_weight_sum
        } else {
            0.0
        };
        let texture_score = if texture_weight_sum > 1e-6 {
            texture_total / texture_weight_sum
        } else {
            0.0
        };

        // Four dimensions: luminance, color, hue detail, texture
        result.aggregate =
            lum_score * 0.35 + chroma_score * 0.15 + hue_score * 0.20 + texture_score * 0.30;

        result
    }

    /// Return human-readable labels for all zone types.
    pub fn zone_labels() -> ZoneLabels {
        ZoneLabels {
            luminance: &["DeepShadow", "Shadow", "Midtone", "Highlight", "Specular"],
            chroma: &["Neutral", "LowSat", "MedSat", "HighSat"],
            hue: &["Warm", "YellGreen", "GreenCyan", "Cool", "Purple", "MagRed"],
            texture: &["Smooth", "Gradient", "Textured", "Edge"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform_planes(l: f32, a: f32, b: f32, w: u32, h: u32) -> OklabPlanes {
        let mut planes = OklabPlanes::new(w, h);
        planes.l.fill(l);
        planes.a.fill(a);
        planes.b.fill(b);
        planes
    }

    #[test]
    fn identical_images_zero_distance() {
        let planes = uniform_planes(0.5, 0.0, 0.0, 32, 32);
        let fa = RegionalFeatures::extract(&planes);
        let fb = RegionalFeatures::extract(&planes);
        let cmp = RegionalComparison::compare(&fa, &fb);
        assert!(
            cmp.aggregate < 1e-5,
            "identical images should have ~0 distance: {}",
            cmp.aggregate
        );
    }

    #[test]
    fn different_luminance_detected() {
        let a = uniform_planes(0.3, 0.0, 0.0, 32, 32);
        let b = uniform_planes(0.7, 0.0, 0.0, 32, 32);
        let fa = RegionalFeatures::extract(&a);
        let fb = RegionalFeatures::extract(&b);
        let cmp = RegionalComparison::compare(&fa, &fb);
        assert!(
            cmp.aggregate > 0.1,
            "different luminance should show high distance: {}",
            cmp.aggregate
        );
    }

    #[test]
    fn different_color_detected() {
        let a = uniform_planes(0.5, 0.15, 0.0, 32, 32); // red
        let b = uniform_planes(0.5, 0.0, 0.15, 32, 32); // yellow
        let fa = RegionalFeatures::extract(&a);
        let fb = RegionalFeatures::extract(&b);
        let cmp = RegionalComparison::compare(&fa, &fb);
        assert!(
            cmp.aggregate > 0.05,
            "different colors should show distance: {}",
            cmp.aggregate
        );
    }

    #[test]
    fn luminance_fractions_sum_to_one() {
        let mut planes = OklabPlanes::new(64, 64);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / (64.0 * 64.0);
        }
        let feat = RegionalFeatures::extract(&planes);
        let sum: f32 = feat.lum_fraction.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-4,
            "lum fractions should sum to 1: {sum}"
        );
    }

    #[test]
    fn chroma_fractions_sum_to_one() {
        let mut planes = OklabPlanes::new(32, 32);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.5;
            planes.a[i] = (i as f32 / 1024.0 - 0.5) * 0.3;
        }
        let feat = RegionalFeatures::extract(&planes);
        let sum: f32 = feat.chroma_fraction.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-4,
            "chroma fractions should sum to 1: {sum}"
        );
    }

    #[test]
    fn zone_labels_correct_count() {
        let labels = RegionalComparison::zone_labels();
        assert_eq!(labels.luminance.len(), LUM_ZONES);
        assert_eq!(labels.chroma.len(), CHROMA_ZONES);
        assert_eq!(labels.hue.len(), HUE_SECTORS);
        assert_eq!(labels.texture.len(), TEXTURE_ZONES);
    }

    #[test]
    fn texture_fractions_sum_to_one_or_zero() {
        // Large enough for patches
        let mut planes = OklabPlanes::new(64, 64);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 / (64.0 * 64.0)).clamp(0.0, 1.0);
        }
        let feat = RegionalFeatures::extract(&planes);
        let sum: f32 = feat.texture_fraction.iter().sum();
        // Should sum to 1.0 if image is large enough for patches
        assert!(
            (sum - 1.0).abs() < 1e-4,
            "texture fractions should sum to 1: {sum}"
        );
    }

    #[test]
    fn smooth_image_classified_smooth() {
        let mut planes = OklabPlanes::new(64, 64);
        // Uniform luminance = all patches smooth
        planes.l.fill(0.5);
        let feat = RegionalFeatures::extract(&planes);
        assert!(
            feat.texture_fraction[0] > 0.9,
            "uniform image should be mostly smooth: {:?}",
            feat.texture_fraction
        );
    }

    #[test]
    fn checkerboard_classified_textured() {
        let mut planes = OklabPlanes::new(64, 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let i = (y * 64 + x) as usize;
                planes.l[i] = if (x + y) % 2 == 0 { 0.3 } else { 0.7 };
            }
        }
        let feat = RegionalFeatures::extract(&planes);
        // High variance checkerboard: should be textured or edge, not smooth
        assert!(
            feat.texture_fraction[0] < 0.1,
            "checkerboard should not be smooth: smooth fraction = {}",
            feat.texture_fraction[0]
        );
    }

    #[test]
    fn edge_image_classified_edge() {
        let mut planes = OklabPlanes::new(64, 64);
        // Sharp vertical edge in the middle of each patch
        for y in 0..64u32 {
            for x in 0..64u32 {
                let i = (y * 64 + x) as usize;
                // Edge at x=8 within each 16px patch
                let local_x = x % PATCH_SIZE as u32;
                planes.l[i] = if local_x < 8 { 0.2 } else { 0.8 };
            }
        }
        let feat = RegionalFeatures::extract(&planes);
        // Should have significant edge or textured classification
        let non_smooth = 1.0 - feat.texture_fraction[0];
        assert!(
            non_smooth > 0.8,
            "edge image should be mostly non-smooth: smooth={}, fracs={:?}",
            feat.texture_fraction[0],
            feat.texture_fraction
        );
    }

    #[test]
    fn texture_comparison_detects_smoothing() {
        // Image A: textured (checkerboard)
        let mut a = OklabPlanes::new(64, 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let i = (y * 64 + x) as usize;
                a.l[i] = if (x + y) % 2 == 0 { 0.4 } else { 0.6 };
            }
        }
        // Image B: same but smoothed (uniform)
        let mut b = OklabPlanes::new(64, 64);
        b.l.fill(0.5);

        let fa = RegionalFeatures::extract(&a);
        let fb = RegionalFeatures::extract(&b);
        let cmp = RegionalComparison::compare(&fa, &fb);

        // Texture zone distance should be significant
        let max_texture_dist = cmp.texture_zone_dist.iter().fold(0.0f32, |a, &b| a.max(b));
        assert!(
            max_texture_dist > 0.01 || cmp.aggregate > 0.05,
            "smoothing should show in texture comparison: texture_dist={:?}, aggregate={}",
            cmp.texture_zone_dist,
            cmp.aggregate
        );
    }
}
