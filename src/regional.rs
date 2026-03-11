//! Spatially-aware image comparison using luminance and chroma region masks.
//!
//! Divides an image into perceptually meaningful regions (shadows, midtones,
//! highlights, saturated colors, neutrals) and computes per-region histograms
//! for diagnostic comparison. This tells you WHERE edits diverge, not just
//! that they do.
//!
//! # Regions
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

use crate::planes::OklabPlanes;

/// Number of luminance zones.
pub const LUM_ZONES: usize = 5;
/// Number of chroma zones.
pub const CHROMA_ZONES: usize = 4;
/// Number of hue sectors (for saturated pixels).
pub const HUE_SECTORS: usize = 6;
/// Histogram bins per channel per region.
pub const HIST_BINS: usize = 32;

/// Luminance zone boundaries.
const LUM_BOUNDS: [f32; 4] = [0.15, 0.35, 0.65, 0.85];
/// Chroma zone boundaries.
const CHROMA_BOUNDS: [f32; 3] = [0.03, 0.08, 0.15];
/// Hue sector boundaries in degrees.
const HUE_BOUNDS: [f32; 6] = [60.0, 120.0, 180.0, 260.0, 320.0, 360.0];

/// Per-region statistics extracted from an image.
#[derive(Clone, Debug)]
pub struct RegionalFeatures {
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

        feat
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
    /// Weighted aggregate distance (lower = more similar).
    pub aggregate: f32,
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

        // Aggregate: weighted combination of all three dimension scores
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

        // Luminance differences are most important, then color, then saturation
        result.aggregate = lum_score * 0.50 + chroma_score * 0.20 + hue_score * 0.30;

        result
    }

    /// Return human-readable labels for all zones.
    pub fn zone_labels() -> (
        &'static [&'static str],
        &'static [&'static str],
        &'static [&'static str],
    ) {
        (
            &["DeepShadow", "Shadow", "Midtone", "Highlight", "Specular"],
            &["Neutral", "LowSat", "MedSat", "HighSat"],
            &["Warm", "YellGreen", "GreenCyan", "Cool", "Purple", "MagRed"],
        )
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
        let (lum, chroma, hue) = RegionalComparison::zone_labels();
        assert_eq!(lum.len(), LUM_ZONES);
        assert_eq!(chroma.len(), CHROMA_ZONES);
        assert_eq!(hue.len(), HUE_SECTORS);
    }
}
