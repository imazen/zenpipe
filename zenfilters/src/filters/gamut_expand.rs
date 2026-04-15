use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::prelude::*;
use zenpixels_convert::gamut::GamutMatrix;

/// Gamut expansion: hue-selective chroma boost simulating wider color gamuts.
///
/// Display P3 extends significantly beyond sRGB in specific hue regions:
/// - Red-orange: the biggest difference (vivid reds, rich oranges)
/// - Green: moderately wider (deeper greens)
/// - Blue: slightly wider
///
/// This filter selectively boosts chroma in the hue regions where P3
/// extends beyond sRGB, producing the "P3 look" — more vivid reds,
/// richer greens, and punchier oranges — that modern phones display.
///
/// Unlike uniform saturation, this targets only the hues that benefit
/// from expansion, preserving the balance of other colors.
///
/// The boost amount adapts per-pixel: already-saturated colors get less
/// boost (vibrance-style protection) to prevent clipping.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct GamutExpand {
    /// Expansion strength. 0.0 = sRGB, 1.0 = full P3-like expansion.
    pub strength: f32,
}

impl Default for GamutExpand {
    fn default() -> Self {
        Self { strength: 0.0 }
    }
}

static GAMUT_EXPAND_SCHEMA: FilterSchema = FilterSchema {
    name: "gamut_expand",
    label: "Gamut Expand",
    description: "Hue-selective chroma boost simulating wider color gamuts (P3)",
    group: FilterGroup::Color,
    params: &[ParamDesc {
        name: "strength",
        label: "Strength",
        description: "Expansion strength (0 = sRGB, 1 = full P3-like)",
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

impl Describe for GamutExpand {
    fn schema() -> &'static FilterSchema {
        &GAMUT_EXPAND_SCHEMA
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

/// Hue-selective expansion weights for P3 vs sRGB gamut difference.
///
/// These approximate how much wider P3 is than sRGB at each hue angle
/// in Oklab space. Values are relative: 1.0 = maximum expansion region.
///
/// Computed from the P3-sRGB gamut boundary difference at L=0.6.
/// The actual boundary varies with L, but this captures the shape.
fn p3_expansion_weight(a: f32, b: f32) -> f32 {
    // Hue angle in Oklab a/b space
    let hue = b.atan2(a); // -π to π

    // P3 extends beyond sRGB most in these regions:
    // - Red-orange: hue ≈ 0.5 to 1.2 rad (peak at ~0.8)  → biggest difference
    // - Green:      hue ≈ 2.2 to 2.9 rad (peak at ~2.5)  → moderate difference
    // - Blue:       hue ≈ -1.8 to -1.2 rad               → small difference
    //
    // Use smooth bell curves centered on each peak.

    let red_orange = gaussian_bell(hue, 0.8, 0.5); // center 0.8, width 0.5
    let green = gaussian_bell(hue, 2.5, 0.4) * 0.6; // slightly less expansion
    let blue = gaussian_bell(hue, -1.5, 0.4) * 0.3; // minimal expansion

    (red_orange + green + blue).min(1.0)
}

/// Smooth bell curve: exp(-0.5 * ((x - center) / width)²)
#[inline]
fn gaussian_bell(x: f32, center: f32, width: f32) -> f32 {
    let d = (x - center) / width;
    (-0.5 * d * d).exp()
}

impl Filter for GamutExpand {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::CHROMA_ONLY
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::GamutExpand
    }
    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength < 1e-6 {
            return;
        }

        let s = self.strength;
        // Maximum chroma expansion factor (at strength=1 in peak P3 region).
        // After hue weighting and protection, the effective per-pixel boost is
        // much smaller than max_expansion. Use 1.0 so that strength=0.25 gives
        // a visible boost even on photos with moderate chroma in P3 regions.
        let max_expansion = 1.0;

        // Vibrance-style protection: already-saturated colors get less boost.
        // 0.35 is a conservative limit for P3 chroma; colors approaching it
        // get progressively less expansion to avoid gamut clipping.
        const MAX_CHROMA: f32 = 0.35;

        for (a_val, b_val) in planes.a.iter_mut().zip(planes.b.iter_mut()) {
            let a = *a_val;
            let b = *b_val;

            let chroma = (a * a + b * b).sqrt();
            if chroma < 1e-7 {
                continue; // near-neutral, skip
            }

            // Hue-selective weight: how much P3 extends beyond sRGB here
            let hue_weight = p3_expansion_weight(a, b);
            if hue_weight < 0.01 {
                continue; // hue not in P3 extension region
            }

            // Protection: reduce boost for already-saturated colors.
            // Linear ramp (not squared) so moderate-chroma pixels still
            // get meaningful expansion — squared protection was too aggressive.
            let protection = (1.0 - (chroma / MAX_CHROMA)).max(0.0);

            // Final expansion factor
            let expansion = 1.0 + s * max_expansion * hue_weight * protection;

            *a_val = a * expansion;
            *b_val = b * expansion;
        }
    }
}

// ============================================================================
// sRGB ↔ Oklab ↔ Oklch scalar conversions (for LUT generation)
// ============================================================================

/// Convert linear sRGB to Oklab.
#[inline]
fn linear_srgb_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let m1 = &SRGB_TO_LMS;
    let l = m1[0][0] * rgb[0] + m1[0][1] * rgb[1] + m1[0][2] * rgb[2];
    let m = m1[1][0] * rgb[0] + m1[1][1] * rgb[1] + m1[1][2] * rgb[2];
    let s = m1[2][0] * rgb[0] + m1[2][1] * rgb[1] + m1[2][2] * rgb[2];

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    [
        0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    ]
}

/// Convert Oklab to linear sRGB.
#[inline]
fn oklab_to_linear_srgb(lab: [f32; 3]) -> [f32; 3] {
    let l_ = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m_ = lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2];
    let s_ = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    let m1_inv = &SRGB_FROM_LMS;
    [
        m1_inv[0][0] * l + m1_inv[0][1] * m + m1_inv[0][2] * s,
        m1_inv[1][0] * l + m1_inv[1][1] * m + m1_inv[1][2] * s,
        m1_inv[2][0] * l + m1_inv[2][1] * m + m1_inv[2][2] * s,
    ]
}

/// Convert Oklab to Oklch (cylindrical form).
#[inline]
fn oklab_to_oklch(lab: [f32; 3]) -> [f32; 3] {
    let c = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
    let h = lab[2].atan2(lab[1]);
    [lab[0], c, h]
}

/// Convert Oklch to Oklab.
#[inline]
fn oklch_to_oklab(lch: [f32; 3]) -> [f32; 3] {
    [lch[0], lch[1] * lch[2].cos(), lch[1] * lch[2].sin()]
}

/// Convert linear sRGB to Oklch.
#[inline]
fn linear_srgb_to_oklch(rgb: [f32; 3]) -> [f32; 3] {
    oklab_to_oklch(linear_srgb_to_oklab(rgb))
}

/// Convert Oklch to linear sRGB.
#[inline]
fn oklch_to_linear_srgb(lch: [f32; 3]) -> [f32; 3] {
    oklab_to_linear_srgb(oklch_to_oklab(lch))
}

// ============================================================================
// sRGB ↔ Display P3 matrix conversions
// ============================================================================

/// Convert linear sRGB to linear Display P3 using the standard 3x3 matrix.
#[inline]
fn linear_srgb_to_linear_p3(rgb: [f32; 3]) -> [f32; 3] {
    let m = &SRGB_TO_P3;
    [
        m[0][0] * rgb[0] + m[0][1] * rgb[1] + m[0][2] * rgb[2],
        m[1][0] * rgb[0] + m[1][1] * rgb[1] + m[1][2] * rgb[2],
        m[2][0] * rgb[0] + m[2][1] * rgb[1] + m[2][2] * rgb[2],
    ]
}

// ============================================================================
// Cached gamut matrices (computed once at startup)
// ============================================================================

// BT.709/sRGB → LMS combined matrix.
// Matches zenpixels_convert::oklab::rgb_to_lms_matrix(Bt709).
const SRGB_TO_LMS: GamutMatrix = [
    [0.412_221_46, 0.536_332_55, 0.051_445_995],
    [0.211_903_5, 0.680_699_5, 0.107_396_96],
    [0.088_302_46, 0.281_718_85, 0.629_978_7],
];

// LMS → BT.709/sRGB combined matrix.
// Matches zenpixels_convert::oklab::lms_to_rgb_matrix(Bt709).
const SRGB_FROM_LMS: GamutMatrix = [
    [4.076_741_7, -3.307_711_6, 0.230_969_94],
    [-1.268_438, 2.609_757_4, -0.341_319_38],
    [-0.004_196_086_3, -0.703_418_6, 1.707_614_7],
];

// BT.709 → Display P3 gamut matrix.
const SRGB_TO_P3: GamutMatrix = [
    [0.822_462_1, 0.177_538, 0.0],
    [0.033_194_2, 0.966_805_8, 0.0],
    [0.017_082_6, 0.072_397_4, 0.910_519_9],
];

// ============================================================================
// Gamut boundary detection
// ============================================================================

/// Check if an Oklch color is near the sRGB gamut boundary.
///
/// Returns a value from 0.0 (well inside gamut) to 1.0 (at or outside boundary).
#[inline]
fn srgb_boundary_distance(lch: [f32; 3]) -> f32 {
    let rgb = oklch_to_linear_srgb(lch);

    let max_val = rgb[0].max(rgb[1]).max(rgb[2]);
    let min_val = rgb[0].min(rgb[1]).min(rgb[2]);

    let upper_dist = if max_val > 0.95 {
        (max_val - 0.95) / 0.05
    } else {
        0.0
    };
    let lower_dist = if min_val < 0.05 {
        (0.05 - min_val) / 0.05
    } else {
        0.0
    };

    (upper_dist.max(lower_dist)).clamp(0.0, 1.0)
}

// ============================================================================
// Oklch-based gamut expansion
// ============================================================================

/// Expand sRGB to P3 using Oklch chroma boost.
///
/// Detects pixels near the sRGB boundary and expands their chroma toward
/// the P3 gamut boundary, preserving hue and lightness. Output is in
/// linear Display P3.
#[inline]
fn expand_oklch(rgb: [f32; 3], strength: f32) -> [f32; 3] {
    let lch = linear_srgb_to_oklch(rgb);
    let boundary_dist = srgb_boundary_distance(lch);

    if boundary_dist < 0.1 {
        return linear_srgb_to_linear_p3(rgb);
    }

    // P3 has ~12% more chroma headroom than sRGB on average
    let boost = 1.0 + boundary_dist * 0.12 * strength;
    let expanded_lch = [lch[0], lch[1] * boost, lch[2]];
    let expanded_srgb = oklch_to_linear_srgb(expanded_lch);
    linear_srgb_to_linear_p3(expanded_srgb)
}

// ============================================================================
// 3D LUT implementation
// ============================================================================

/// 3D lookup table for fast sRGB → P3 gamut expansion.
///
/// Pre-computes the Oklch-based gamut expansion for a grid of RGB values,
/// then uses trilinear interpolation for fast inference. This is an
/// alternative to the [`GamutExpand`] filter for use cases where you need
/// actual Display P3 output rather than perceptual enhancement within sRGB.
///
/// # Input/Output
///
/// - **Input:** linear sRGB RGB in [0, 1]
/// - **Output:** linear Display P3 RGB
///
/// # Common sizes
///
/// - 17: ~59 KB, good for smooth gradients
/// - 33: ~431 KB, excellent quality
///
/// # Example
///
/// ```
/// use zenfilters::filters::GamutLut;
///
/// let lut = GamutLut::generate(17, 1.0);
/// let p3_rgb = lut.lookup([0.9, 0.3, 0.1]);
/// assert!(p3_rgb[0] > 0.0); // valid P3 output
/// ```
#[derive(Clone, Debug)]
pub struct GamutLut {
    /// LUT data: [r][g][b] -> [r', g', b'] in P3
    data: Vec<[f32; 3]>,
    /// LUT resolution per axis
    size: usize,
}

impl GamutLut {
    /// Create a new LUT with the specified resolution, filled with zeros.
    ///
    /// Use [`generate`](Self::generate) for a ready-to-use LUT.
    pub fn new(size: usize) -> Self {
        let total = size * size * size;
        GamutLut {
            data: vec![[0.0, 0.0, 0.0]; total],
            size,
        }
    }

    /// Generate a LUT using Oklch-based gamut expansion.
    ///
    /// Each grid point maps linear sRGB to linear Display P3 with the
    /// specified expansion strength (0.0 = pure matrix conversion,
    /// 1.0 = full Oklch chroma boost at sRGB boundary).
    pub fn generate(size: usize, strength: f32) -> Self {
        let mut lut = GamutLut::new(size);
        let scale = 1.0 / (size - 1) as f32;

        for r in 0..size {
            for g in 0..size {
                for b in 0..size {
                    let rgb = [r as f32 * scale, g as f32 * scale, b as f32 * scale];
                    let expanded = expand_oklch(rgb, strength);
                    lut.set(r, g, b, expanded);
                }
            }
        }

        lut
    }

    /// Set a LUT entry at grid coordinates (r, g, b).
    #[inline]
    pub fn set(&mut self, r: usize, g: usize, b: usize, value: [f32; 3]) {
        let idx = r * self.size * self.size + g * self.size + b;
        self.data[idx] = value;
    }

    /// Get a LUT entry at grid coordinates (r, g, b).
    #[inline]
    pub fn get(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        let idx = r * self.size * self.size + g * self.size + b;
        self.data[idx]
    }

    /// Look up a color with trilinear interpolation.
    ///
    /// Input is linear sRGB RGB in [0, 1]. Output is linear Display P3 RGB.
    /// Values outside [0, 1] are clamped to the LUT boundary.
    #[inline]
    pub fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let scale = (self.size - 1) as f32;

        let r = (rgb[0] * scale).clamp(0.0, scale);
        let g = (rgb[1] * scale).clamp(0.0, scale);
        let b = (rgb[2] * scale).clamp(0.0, scale);

        let r0 = r.floor() as usize;
        let g0 = g.floor() as usize;
        let b0 = b.floor() as usize;

        let r1 = (r0 + 1).min(self.size - 1);
        let g1 = (g0 + 1).min(self.size - 1);
        let b1 = (b0 + 1).min(self.size - 1);

        let rf = r.fract();
        let gf = g.fract();
        let bf = b.fract();

        // Trilinear interpolation
        let c000 = self.get(r0, g0, b0);
        let c001 = self.get(r0, g0, b1);
        let c010 = self.get(r0, g1, b0);
        let c011 = self.get(r0, g1, b1);
        let c100 = self.get(r1, g0, b0);
        let c101 = self.get(r1, g0, b1);
        let c110 = self.get(r1, g1, b0);
        let c111 = self.get(r1, g1, b1);

        let mut result = [0.0f32; 3];
        for i in 0..3 {
            let c00 = c000[i] * (1.0 - rf) + c100[i] * rf;
            let c01 = c001[i] * (1.0 - rf) + c101[i] * rf;
            let c10 = c010[i] * (1.0 - rf) + c110[i] * rf;
            let c11 = c011[i] * (1.0 - rf) + c111[i] * rf;

            let c0 = c00 * (1.0 - gf) + c10 * gf;
            let c1 = c01 * (1.0 - gf) + c11 * gf;

            result[i] = c0 * (1.0 - bf) + c1 * bf;
        }

        result
    }

    /// LUT resolution per axis.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Size of the LUT data in bytes.
    pub fn size_bytes(&self) -> usize {
        self.data.len() * core::mem::size_of::<[f32; 3]>()
    }

    /// Apply this LUT to a row of interleaved linear sRGB pixels in-place,
    /// producing linear Display P3 output.
    ///
    /// `stride` is 3 (RGB) or 4 (RGBA). Alpha is preserved.
    pub fn apply_row(&self, data: &mut [f32], stride: usize) {
        debug_assert!(stride == 3 || stride == 4);
        for pixel in data.chunks_exact_mut(stride) {
            let rgb = [pixel[0], pixel[1], pixel[2]];
            let p3 = self.lookup(rgb);
            pixel[0] = p3[0];
            pixel[1] = p3[1];
            pixel[2] = p3[2];
        }
    }
}

// ============================================================================
// Hand-coded MLP implementation
// ============================================================================

/// Hand-coded MLP for sRGB → P3 gamut expansion.
///
/// A 3-layer neural network (3 → 32 → 32 → 3) with ReLU activations and
/// a residual (skip) connection. The network predicts a correction term
/// on top of the linear sRGB-to-P3 matrix conversion.
///
/// This is an alternative to the [`GamutExpand`] filter for use cases
/// where you need actual Display P3 output. With trained weights, the
/// MLP can learn the optimal expansion mapping from paired sRGB/P3 data.
///
/// # Input/Output
///
/// - **Input:** linear sRGB RGB
/// - **Output:** linear Display P3 RGB
///
/// # Architecture
///
/// Matches the GamutMLP architecture (CVPR 2023):
/// - Layer 1: 3 → 32, ReLU
/// - Layer 2: 32 → 32, ReLU
/// - Layer 3: 32 → 3, linear (residual)
/// - Skip connection: output = sRGB-to-P3-matrix(input) + residual
///
/// With zero weights, output equals the standard sRGB → P3 matrix
/// conversion (the residual is zero, skip connection passes through).
///
/// Total: 1283 weights = 5132 bytes
///
/// # Example
///
/// ```
/// use zenfilters::filters::GamutMLP;
///
/// // Zero weights = pure matrix conversion
/// let mlp = GamutMLP::new();
/// let p3_rgb = mlp.forward([0.5, 0.3, 0.8]);
/// assert!(p3_rgb[0] > 0.0);
/// assert_eq!(mlp.weight_count(), 1283);
/// ```
#[derive(Clone, Debug)]
pub struct GamutMLP {
    /// Layer 1: 3 → 32
    w1: [[f32; 3]; 32],
    b1: [f32; 32],
    /// Layer 2: 32 → 32
    w2: [[f32; 32]; 32],
    b2: [f32; 32],
    /// Layer 3: 32 → 3
    w3: [[f32; 32]; 3],
    b3: [f32; 3],
}

impl GamutMLP {
    /// Create a new MLP with zero weights.
    ///
    /// With zero weights, the residual is always zero, so the output
    /// equals the standard sRGB → P3 matrix conversion (identity + P3 matrix).
    pub fn new() -> Self {
        GamutMLP {
            w1: [[0.0; 3]; 32],
            b1: [0.0; 32],
            w2: [[0.0; 32]; 32],
            b2: [0.0; 32],
            w3: [[0.0; 32]; 3],
            b3: [0.0; 3],
        }
    }

    /// Load weights from a flat array.
    ///
    /// Expected layout: w1 (96), b1 (32), w2 (1024), b2 (32), w3 (96), b3 (3).
    /// Total: 1283 floats = 5132 bytes.
    ///
    /// Returns `Err` if the slice length is not exactly 1283.
    pub fn from_weights(weights: &[f32]) -> Result<Self, &'static str> {
        if weights.len() != 1283 {
            return Err("Expected 1283 weights");
        }

        let mut mlp = GamutMLP::new();
        let mut idx = 0;

        // w1: 32 x 3 = 96
        for neuron in &mut mlp.w1 {
            for w in neuron.iter_mut() {
                *w = weights[idx];
                idx += 1;
            }
        }

        // b1: 32
        for b in &mut mlp.b1 {
            *b = weights[idx];
            idx += 1;
        }

        // w2: 32 x 32 = 1024
        for neuron in &mut mlp.w2 {
            for w in neuron.iter_mut() {
                *w = weights[idx];
                idx += 1;
            }
        }

        // b2: 32
        for b in &mut mlp.b2 {
            *b = weights[idx];
            idx += 1;
        }

        // w3: 3 x 32 = 96
        for neuron in &mut mlp.w3 {
            for w in neuron.iter_mut() {
                *w = weights[idx];
                idx += 1;
            }
        }

        // b3: 3
        for b in &mut mlp.b3 {
            *b = weights[idx];
            idx += 1;
        }

        Ok(mlp)
    }

    /// Forward pass: predict linear Display P3 RGB from linear sRGB RGB.
    ///
    /// Computes input through three layers with ReLU activations, then
    /// adds the residual to the sRGB-to-P3 matrix conversion of the input.
    #[inline]
    #[allow(clippy::needless_range_loop)]
    pub fn forward(&self, rgb: [f32; 3]) -> [f32; 3] {
        // Layer 1: 3 → 32 with ReLU
        let mut h1 = [0.0f32; 32];
        for i in 0..32 {
            let mut sum = self.b1[i];
            for j in 0..3 {
                sum += self.w1[i][j] * rgb[j];
            }
            h1[i] = sum.max(0.0); // ReLU
        }

        // Layer 2: 32 → 32 with ReLU
        let mut h2 = [0.0f32; 32];
        for i in 0..32 {
            let mut sum = self.b2[i];
            for j in 0..32 {
                sum += self.w2[i][j] * h1[j];
            }
            h2[i] = sum.max(0.0); // ReLU
        }

        // Layer 3: 32 → 3 (residual output)
        let mut residual = [0.0f32; 3];
        for i in 0..3 {
            let mut sum = self.b3[i];
            for j in 0..32 {
                sum += self.w3[i][j] * h2[j];
            }
            residual[i] = sum;
        }

        // Skip connection: P3 matrix conversion + learned residual
        let p3 = linear_srgb_to_linear_p3(rgb);
        [
            p3[0] + residual[0],
            p3[1] + residual[1],
            p3[2] + residual[2],
        ]
    }

    /// Total number of trainable weights.
    pub fn weight_count(&self) -> usize {
        1283
    }

    /// Size of all weights in bytes.
    pub fn size_bytes(&self) -> usize {
        1283 * 4
    }

    /// Apply this MLP to a row of interleaved linear sRGB pixels in-place,
    /// producing linear Display P3 output.
    ///
    /// `stride` is 3 (RGB) or 4 (RGBA). Alpha is preserved.
    pub fn apply_row(&self, data: &mut [f32], stride: usize) {
        debug_assert!(stride == 3 || stride == 4);
        for pixel in data.chunks_exact_mut(stride) {
            let rgb = [pixel[0], pixel[1], pixel[2]];
            let p3 = self.forward(rgb);
            pixel[0] = p3[0];
            pixel[1] = p3[1];
            pixel[2] = p3[2];
        }
    }
}

impl Default for GamutMLP {
    fn default() -> Self {
        GamutMLP::new()
    }
}

// ============================================================================
// GamutExpandMethod enum
// ============================================================================

/// Method for sRGB → Display P3 gamut expansion.
///
/// These methods produce **linear Display P3** output from **linear sRGB** input.
/// They are alternatives to the [`GamutExpand`] filter, which works in Oklab
/// space for perceptual enhancement within sRGB.
///
/// Use these when you need actual P3 pixel values (e.g., for P3-capable displays
/// or P3-tagged output files).
#[derive(Clone, Debug)]
pub enum GamutExpandMethod {
    /// Direct colorspace conversion via the sRGB → P3 matrix (no expansion).
    Direct,
    /// Oklch-based chroma expansion at the sRGB boundary.
    Oklch {
        /// Expansion strength (0.0 = none, 1.0 = full, >1.0 = aggressive).
        strength: f32,
    },
    /// Pre-computed 3D LUT with trilinear interpolation.
    Lut(GamutLut),
    /// Hand-coded MLP neural network (boxed to reduce enum size).
    Mlp(Box<GamutMLP>),
}

impl Default for GamutExpandMethod {
    fn default() -> Self {
        GamutExpandMethod::Oklch { strength: 1.0 }
    }
}

impl GamutExpandMethod {
    /// Expand a single linear sRGB pixel to linear Display P3.
    #[inline]
    pub fn expand(&self, rgb: [f32; 3]) -> [f32; 3] {
        match self {
            GamutExpandMethod::Direct => linear_srgb_to_linear_p3(rgb),
            GamutExpandMethod::Oklch { strength } => expand_oklch(rgb, *strength),
            GamutExpandMethod::Lut(lut) => lut.lookup(rgb),
            GamutExpandMethod::Mlp(mlp) => mlp.forward(rgb),
        }
    }

    /// Apply this method to a row of interleaved linear sRGB pixels in-place,
    /// producing linear Display P3 output.
    ///
    /// `stride` is 3 (RGB) or 4 (RGBA). Alpha is preserved.
    pub fn apply_row(&self, data: &mut [f32], stride: usize) {
        debug_assert!(stride == 3 || stride == 4);
        for pixel in data.chunks_exact_mut(stride) {
            let rgb = [pixel[0], pixel[1], pixel[2]];
            let p3 = self.expand(rgb);
            pixel[0] = p3[0];
            pixel[1] = p3[1];
            pixel[2] = p3[2];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_strength_is_identity() {
        let mut planes = OklabPlanes::new(4, 4);
        for v in &mut planes.a {
            *v = 0.1;
        }
        for v in &mut planes.b {
            *v = 0.05;
        }
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();
        GamutExpand { strength: 0.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a, a_orig);
        assert_eq!(planes.b, b_orig);
    }

    #[test]
    fn boosts_red_orange() {
        // Red-orange in Oklab: positive a, positive b
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.15; // red-ish
        planes.b[0] = 0.12; // orange-ish
        let chroma_before = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let chroma_after = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        assert!(
            chroma_after > chroma_before,
            "red-orange should be boosted: {chroma_before} -> {chroma_after}"
        );
    }

    #[test]
    fn boosts_green() {
        // Green in Oklab: negative a, positive b
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = -0.12;
        planes.b[0] = 0.10;
        let chroma_before = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let chroma_after = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        assert!(
            chroma_after > chroma_before,
            "green should be boosted: {chroma_before} -> {chroma_after}"
        );
    }

    #[test]
    fn minimal_effect_on_yellow() {
        // Yellow in Oklab: near zero a, positive b (between red and green peaks)
        // This is NOT a major P3 extension region
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.01;
        planes.b[0] = 0.15;
        let chroma_before = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let chroma_after = (planes.a[0].powi(2) + planes.b[0].powi(2)).sqrt();
        // Yellow gets some boost via the tail of the red-orange bell curve.
        // With max_expansion=1.0, it should stay below ~30% (P3 barely extends in yellow).
        let boost_pct = (chroma_after - chroma_before) / chroma_before * 100.0;
        assert!(
            boost_pct < 30.0,
            "yellow should get limited boost: {boost_pct:.1}%"
        );
    }

    #[test]
    fn protects_already_saturated() {
        // Highly saturated red — should get less boost
        let mut planes_low = OklabPlanes::new(1, 1);
        planes_low.a[0] = 0.05;
        planes_low.b[0] = 0.04;
        let chroma_low_before = (planes_low.a[0].powi(2) + planes_low.b[0].powi(2)).sqrt();

        let mut planes_high = OklabPlanes::new(1, 1);
        planes_high.a[0] = 0.30;
        planes_high.b[0] = 0.24;
        let chroma_high_before = (planes_high.a[0].powi(2) + planes_high.b[0].powi(2)).sqrt();

        GamutExpand { strength: 1.0 }.apply(&mut planes_low, &mut FilterContext::new());
        GamutExpand { strength: 1.0 }.apply(&mut planes_high, &mut FilterContext::new());

        let chroma_low_after = (planes_low.a[0].powi(2) + planes_low.b[0].powi(2)).sqrt();
        let chroma_high_after = (planes_high.a[0].powi(2) + planes_high.b[0].powi(2)).sqrt();

        let pct_low = (chroma_low_after - chroma_low_before) / chroma_low_before;
        let pct_high = (chroma_high_after - chroma_high_before) / chroma_high_before;

        assert!(
            pct_low > pct_high,
            "low chroma should get more boost: {pct_low:.3} vs {pct_high:.3}"
        );
    }

    #[test]
    fn does_not_modify_luminance() {
        let mut planes = OklabPlanes::new(10, 1);
        for v in &mut planes.l {
            *v = 0.6;
        }
        for v in &mut planes.a {
            *v = 0.1;
        }
        let l_orig = planes.l.clone();
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
    }

    #[test]
    fn preserves_hue() {
        // Expansion should scale chroma without changing hue angle
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.12;
        planes.b[0] = 0.10;
        let hue_before = planes.b[0].atan2(planes.a[0]);
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        let hue_after = planes.b[0].atan2(planes.a[0]);
        assert!(
            (hue_after - hue_before).abs() < 1e-5,
            "hue should be preserved: {hue_before} -> {hue_after}"
        );
    }

    #[test]
    fn neutral_unchanged() {
        let mut planes = OklabPlanes::new(1, 1);
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;
        GamutExpand { strength: 1.0 }.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a[0], 0.0);
        assert_eq!(planes.b[0], 0.0);
    }

    // ====================================================================
    // GamutLut tests
    // ====================================================================

    #[test]
    fn lut_generation_size() {
        let lut = GamutLut::generate(5, 1.0);
        assert_eq!(lut.size_bytes(), 5 * 5 * 5 * 12); // 1500 bytes
        assert_eq!(lut.size(), 5);
    }

    #[test]
    fn lut_corner_matches_direct() {
        let lut = GamutLut::generate(5, 1.0);

        // Corner (1,1,1) should match direct computation
        let corner = lut.get(4, 4, 4);
        let direct = expand_oklch([1.0, 1.0, 1.0], 1.0);

        for i in 0..3 {
            assert!(
                (corner[i] - direct[i]).abs() < 1e-5,
                "corner[{i}] = {}, direct[{i}] = {}",
                corner[i],
                direct[i]
            );
        }
    }

    #[test]
    fn lut_interpolation_accuracy() {
        let lut = GamutLut::generate(17, 1.0);

        // Mid-point should be close to direct computation
        let mid = lut.lookup([0.5, 0.5, 0.5]);
        let direct = expand_oklch([0.5, 0.5, 0.5], 1.0);

        for i in 0..3 {
            assert!(
                (mid[i] - direct[i]).abs() < 0.01,
                "LUT interpolation error at mid-point: ch{i} lut={} direct={}",
                mid[i],
                direct[i]
            );
        }
    }

    #[test]
    fn lut_black_maps_to_black() {
        let lut = GamutLut::generate(17, 1.0);
        let black = lut.lookup([0.0, 0.0, 0.0]);
        for i in 0..3 {
            assert!(
                black[i].abs() < 1e-5,
                "black should map to black: ch{i} = {}",
                black[i]
            );
        }
    }

    #[test]
    fn lut_apply_row() {
        let lut = GamutLut::generate(17, 1.0);
        let mut row = vec![0.5f32, 0.3, 0.8, 0.9, 0.1, 0.2];
        let expected_0 = lut.lookup([0.5, 0.3, 0.8]);
        let expected_1 = lut.lookup([0.9, 0.1, 0.2]);
        lut.apply_row(&mut row, 3);
        for i in 0..3 {
            assert!((row[i] - expected_0[i]).abs() < 1e-6);
            assert!((row[i + 3] - expected_1[i]).abs() < 1e-6);
        }
    }

    // ====================================================================
    // GamutMLP tests
    // ====================================================================

    #[test]
    fn mlp_structure() {
        let mlp = GamutMLP::new();
        assert_eq!(mlp.weight_count(), 1283);
        assert_eq!(mlp.size_bytes(), 5132);
    }

    #[test]
    fn mlp_zero_weights_equals_p3_matrix() {
        // With zero weights, residual is zero, so output = sRGB→P3 matrix
        let mlp = GamutMLP::new();
        let rgb = [0.5, 0.3, 0.8];
        let output = mlp.forward(rgb);
        let expected = linear_srgb_to_linear_p3(rgb);

        for i in 0..3 {
            assert!(
                (output[i] - expected[i]).abs() < 1e-5,
                "zero-weight MLP ch{i}: {} vs expected {}",
                output[i],
                expected[i]
            );
        }
    }

    #[test]
    fn mlp_from_weights_roundtrip() {
        // Create an MLP, serialize to flat weights, reconstruct, verify
        let weights = vec![0.01f32; 1283];
        let mlp = GamutMLP::from_weights(&weights).unwrap();

        // Verify a weight was loaded correctly
        assert!((mlp.w1[0][0] - 0.01).abs() < 1e-6);
        assert!((mlp.b1[0] - 0.01).abs() < 1e-6);
        assert!((mlp.w2[0][0] - 0.01).abs() < 1e-6);
        assert!((mlp.b2[0] - 0.01).abs() < 1e-6);
        assert!((mlp.w3[0][0] - 0.01).abs() < 1e-6);
        assert!((mlp.b3[0] - 0.01).abs() < 1e-6);
    }

    #[test]
    fn mlp_from_weights_wrong_length() {
        assert!(GamutMLP::from_weights(&[0.0; 100]).is_err());
        assert!(GamutMLP::from_weights(&[0.0; 1284]).is_err());
    }

    #[test]
    fn mlp_apply_row() {
        let mlp = GamutMLP::new();
        let mut row = vec![0.5f32, 0.3, 0.8, 0.9, 0.1, 0.2];
        let expected_0 = mlp.forward([0.5, 0.3, 0.8]);
        let expected_1 = mlp.forward([0.9, 0.1, 0.2]);
        mlp.apply_row(&mut row, 3);
        for i in 0..3 {
            assert!((row[i] - expected_0[i]).abs() < 1e-6);
            assert!((row[i + 3] - expected_1[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn mlp_default_is_new() {
        let a = GamutMLP::default();
        let b = GamutMLP::new();
        let rgb = [0.4, 0.5, 0.6];
        let out_a = a.forward(rgb);
        let out_b = b.forward(rgb);
        for i in 0..3 {
            assert!((out_a[i] - out_b[i]).abs() < 1e-10);
        }
    }

    // ====================================================================
    // GamutExpandMethod tests
    // ====================================================================

    #[test]
    fn method_direct() {
        let method = GamutExpandMethod::Direct;
        let rgb = [0.5, 0.3, 0.8];
        let result = method.expand(rgb);
        let expected = linear_srgb_to_linear_p3(rgb);
        for i in 0..3 {
            assert!((result[i] - expected[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn method_oklch() {
        let method = GamutExpandMethod::Oklch { strength: 1.0 };
        let rgb = [0.5, 0.3, 0.8];
        let result = method.expand(rgb);
        let expected = expand_oklch(rgb, 1.0);
        for i in 0..3 {
            assert!((result[i] - expected[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn method_lut() {
        let lut = GamutLut::generate(17, 1.0);
        let method = GamutExpandMethod::Lut(lut.clone());
        let rgb = [0.5, 0.3, 0.8];
        let result = method.expand(rgb);
        let expected = lut.lookup(rgb);
        for i in 0..3 {
            assert!((result[i] - expected[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn method_mlp() {
        let mlp = GamutMLP::new();
        let method = GamutExpandMethod::Mlp(Box::new(mlp.clone()));
        let rgb = [0.5, 0.3, 0.8];
        let result = method.expand(rgb);
        let expected = mlp.forward(rgb);
        for i in 0..3 {
            assert!((result[i] - expected[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn method_default_is_oklch() {
        let method = GamutExpandMethod::default();
        assert!(
            matches!(method, GamutExpandMethod::Oklch { strength } if (strength - 1.0).abs() < 1e-6)
        );
    }

    // ====================================================================
    // Oklch roundtrip consistency
    // ====================================================================

    #[test]
    fn oklch_roundtrip() {
        // sRGB → Oklab → Oklch → Oklab → sRGB should be identity
        let test_colors: &[[f32; 3]] = &[
            [0.5, 0.3, 0.8],
            [0.1, 0.9, 0.2],
            [0.8, 0.8, 0.8],
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
        ];

        for &rgb in test_colors {
            let oklab = linear_srgb_to_oklab(rgb);
            let oklch = oklab_to_oklch(oklab);
            let oklab2 = oklch_to_oklab(oklch);
            let rgb2 = oklab_to_linear_srgb(oklab2);

            for i in 0..3 {
                assert!(
                    (rgb[i] - rgb2[i]).abs() < 1e-4,
                    "Oklch roundtrip error: rgb={rgb:?} -> rgb2={rgb2:?}"
                );
            }
        }
    }
}
