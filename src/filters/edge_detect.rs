use crate::access::ChannelAccess;
use crate::blur::{GaussianKernel, gaussian_blur_plane};
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Edge detection mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EdgeMode {
    /// Sobel operator: directional 3×3 gradient (horizontal + vertical).
    /// Magnitude = sqrt(Gx² + Gy²). Good for edge detection and contours.
    Sobel,
    /// Laplacian operator: isotropic second-derivative 3×3 kernel.
    /// Detects edges and fine texture. More sensitive to noise than Sobel.
    Laplacian,
    /// Canny edge detector: Gaussian smoothing → Sobel gradient → non-maximum
    /// suppression → hysteresis thresholding. Produces thin, connected edges
    /// with fewer false detections than raw Sobel.
    ///
    /// SOTA classical edge detection. The `strength` parameter controls the
    /// Gaussian pre-blur sigma (higher = fewer noise edges, lower = more detail).
    /// Thresholds are derived automatically from the gradient histogram.
    Canny,
}

/// Edge detection on the L (lightness) channel.
///
/// Replaces L with gradient magnitude (Sobel/Laplacian) or binary edges (Canny),
/// normalized to [0, 1]. Chroma channels are zeroed to produce a grayscale
/// edge map.
///
/// Use cases:
/// - Mask generation for selective adjustments (edge-aware masks)
/// - Document analysis (text boundary detection, layout)
/// - Input for downstream algorithms (corner detection, line fitting)
///
/// # Canny mode
///
/// The Canny detector is the SOTA classical edge detection algorithm:
/// 1. **Gaussian blur** (sigma from `strength` param, default 1.0) to suppress noise
/// 2. **Sobel gradient** for magnitude and direction
/// 3. **Non-maximum suppression** — thin edges to 1-pixel width by suppressing
///    pixels that aren't local maxima along the gradient direction
/// 4. **Hysteresis thresholding** — connect strong edges through weak edges,
///    with thresholds auto-derived from the gradient distribution (Otsu-like)
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct EdgeDetect {
    /// Detection algorithm.
    pub mode: EdgeMode,
    /// For Sobel/Laplacian: output scaling factor (amplifies weak edges).
    /// For Canny: Gaussian pre-blur sigma (controls noise sensitivity).
    /// Default: 1.0. Range: 0.1–5.0.
    pub strength: f32,
}

impl Default for EdgeDetect {
    fn default() -> Self {
        Self {
            mode: EdgeMode::Sobel,
            strength: 1.0,
        }
    }
}

impl Filter for EdgeDetect {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, _width: u32, _height: u32) -> u32 {
        match self.mode {
            EdgeMode::Sobel | EdgeMode::Laplacian => 1,
            EdgeMode::Canny => (self.strength * 3.0).ceil() as u32 + 1,
        }
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::EdgeDetect
    }

    fn resize_phase(&self) -> crate::filter::ResizePhase {
        crate::filter::ResizePhase::PreResize
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        let w = planes.width as usize;
        let h = planes.height as usize;
        let n = w * h;

        let mut dst = ctx.take_f32(n);

        match self.mode {
            EdgeMode::Sobel => sobel(&planes.l, &mut dst, w, h, self.strength),
            EdgeMode::Laplacian => laplacian(&planes.l, &mut dst, w, h, self.strength),
            EdgeMode::Canny => canny(
                &planes.l,
                &mut dst,
                planes.width,
                planes.height,
                self.strength,
                ctx,
            ),
        }

        let old_l = core::mem::replace(&mut planes.l, dst);
        ctx.return_f32(old_l);

        // Zero chroma to produce grayscale edge map
        planes.a.fill(0.0);
        planes.b.fill(0.0);
    }
}

/// Sobel edge detection: gradient magnitude from 3×3 directional kernels.
///
/// Gx = [-1  0  1]    Gy = [-1 -2 -1]
///      [-2  0  2]         [ 0  0  0]
///      [-1  0  1]         [ 1  2  1]
fn sobel(src: &[f32], dst: &mut [f32], w: usize, h: usize, strength: f32) {
    for y in 0..h {
        for x in 0..w {
            let p = |dx: isize, dy: isize| -> f32 {
                let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                src[sy * w + sx]
            };

            let tl = p(-1, -1);
            let tc = p(0, -1);
            let tr = p(1, -1);
            let ml = p(-1, 0);
            let mr = p(1, 0);
            let bl = p(-1, 1);
            let bc = p(0, 1);
            let br = p(1, 1);

            let gx = -tl + tr - 2.0 * ml + 2.0 * mr - bl + br;
            let gy = -tl - 2.0 * tc - tr + bl + 2.0 * bc + br;

            let mag = (gx * gx + gy * gy).sqrt() * strength;
            dst[y * w + x] = mag.clamp(0.0, 1.0);
        }
    }
}

/// Laplacian edge detection: isotropic 3×3 second-derivative kernel.
///
/// Kernel = [ 0 -1  0]
///          [-1  4 -1]
///          [ 0 -1  0]
fn laplacian(src: &[f32], dst: &mut [f32], w: usize, h: usize, strength: f32) {
    for y in 0..h {
        for x in 0..w {
            let p = |dx: isize, dy: isize| -> f32 {
                let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                src[sy * w + sx]
            };

            let center = p(0, 0);
            let lap = -p(0, -1) - p(-1, 0) + 4.0 * center - p(1, 0) - p(0, 1);

            let mag = lap.abs() * strength;
            dst[y * w + x] = mag.clamp(0.0, 1.0);
        }
    }
}

// ─── Canny edge detector ────────────────────────────────────────────

/// Canny edge detection: the full pipeline.
///
/// 1. Gaussian blur (sigma = `strength`)
/// 2. Sobel gradient magnitude + direction
/// 3. Non-maximum suppression (thin to 1px edges)
/// 4. Hysteresis thresholding (connect strong edges through weak ones)
fn canny(
    src: &[f32],
    dst: &mut [f32],
    width: u32,
    height: u32,
    sigma: f32,
    ctx: &mut FilterContext,
) {
    let w = width as usize;
    let h = height as usize;
    let n = w * h;

    // Step 1: Gaussian blur to suppress noise
    let mut blurred = ctx.take_f32(n);
    if sigma >= 0.5 {
        let kernel = GaussianKernel::new(sigma);
        gaussian_blur_plane(src, &mut blurred, width, height, &kernel, ctx);
    } else {
        blurred.copy_from_slice(src);
    }

    // Step 2: Sobel gradient magnitude + direction
    let mut mag = ctx.take_f32(n);
    let mut dir = ctx.take_f32(n); // direction in [0, 4) quantized

    for y in 0..h {
        for x in 0..w {
            let p = |dx: isize, dy: isize| -> f32 {
                let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                blurred[sy * w + sx]
            };

            let tl = p(-1, -1);
            let tc = p(0, -1);
            let tr = p(1, -1);
            let ml = p(-1, 0);
            let mr = p(1, 0);
            let bl = p(-1, 1);
            let bc = p(0, 1);
            let br = p(1, 1);

            let gx = -tl + tr - 2.0 * ml + 2.0 * mr - bl + br;
            let gy = -tl - 2.0 * tc - tr + bl + 2.0 * bc + br;

            let idx = y * w + x;
            mag[idx] = (gx * gx + gy * gy).sqrt();

            // Quantize gradient direction to 4 orientations:
            // 0 = horizontal (compare left/right)
            // 1 = diagonal 45° (compare TL/BR)
            // 2 = vertical (compare top/bottom)
            // 3 = diagonal 135° (compare TR/BL)
            let angle = gy.atan2(gx); // -π to π
            let normalized = ((angle * 4.0 / core::f32::consts::PI) + 4.5) as usize % 4;
            dir[idx] = normalized as f32;
        }
    }

    ctx.return_f32(blurred);

    // Step 3: Non-maximum suppression — thin edges to 1px width
    let mut nms = ctx.take_f32(n);
    nms.fill(0.0);

    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let idx = y * w + x;
            let m = mag[idx];
            let d = dir[idx] as usize;

            // Compare with neighbors along gradient direction
            let (n1, n2) = match d {
                0 => (mag[idx - 1], mag[idx + 1]),                 // horizontal
                1 => (mag[(y - 1) * w + x - 1], mag[(y + 1) * w + x + 1]), // 45°
                2 => (mag[(y - 1) * w + x], mag[(y + 1) * w + x]),         // vertical
                _ => (mag[(y - 1) * w + x + 1], mag[(y + 1) * w + x - 1]), // 135°
            };

            // Keep pixel only if it's a local maximum along gradient direction
            if m >= n1 && m >= n2 {
                nms[idx] = m;
            }
        }
    }

    ctx.return_f32(dir);

    // Step 4: Compute thresholds from gradient histogram (Otsu-like auto threshold)
    let (low_thresh, high_thresh) = compute_canny_thresholds(&nms, n);

    // No edges if thresholds are zero (constant image)
    if high_thresh <= 0.0 {
        dst.fill(0.0);
        ctx.return_f32(nms);
        ctx.return_f32(mag);
        return;
    }

    // Step 5: Hysteresis thresholding — DFS from strong edges through weak ones
    // Mark: 0 = rejected, 1 = weak, 2 = strong
    let mut marks = ctx.take_f32(n);
    marks.fill(0.0);

    for i in 0..n {
        if nms[i] >= high_thresh {
            marks[i] = 2.0;
        } else if nms[i] >= low_thresh {
            marks[i] = 1.0;
        }
    }

    // Trace: promote weak edges connected to strong edges
    // Use iterative approach to avoid stack overflow on large images
    let mut changed = true;
    while changed {
        changed = false;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let idx = y * w + x;
                if marks[idx] != 1.0 {
                    continue;
                }
                // Check 8-connected neighbors for a strong edge
                let has_strong = marks[(y - 1) * w + x - 1] == 2.0
                    || marks[(y - 1) * w + x] == 2.0
                    || marks[(y - 1) * w + x + 1] == 2.0
                    || marks[y * w + x - 1] == 2.0
                    || marks[y * w + x + 1] == 2.0
                    || marks[(y + 1) * w + x - 1] == 2.0
                    || marks[(y + 1) * w + x] == 2.0
                    || marks[(y + 1) * w + x + 1] == 2.0;
                if has_strong {
                    marks[idx] = 2.0;
                    changed = true;
                }
            }
        }
    }

    // Output: strong edges = 1.0, everything else = 0.0
    for i in 0..n {
        dst[i] = if marks[i] == 2.0 { 1.0 } else { 0.0 };
    }

    ctx.return_f32(marks);
    ctx.return_f32(nms);
    ctx.return_f32(mag);
}

/// Minimum gradient magnitude to be considered an edge.
/// Anything below this is float noise, not a real edge.
const CANNY_MIN_GRADIENT: f32 = 0.005;

/// Auto-compute Canny thresholds from gradient magnitude distribution.
///
/// Uses a ratio-based approach on the maximum gradient:
/// - high = max_magnitude * 0.3 (strong edge threshold)
/// - low = high * 0.4 (weak edge threshold for hysteresis)
///
/// This is more robust than percentile-based methods because it adapts
/// to the actual contrast in the image rather than the number of edge pixels.
fn compute_canny_thresholds(nms: &[f32], n: usize) -> (f32, f32) {
    let mut max_mag = 0.0f32;
    for &v in &nms[..n] {
        if v > max_mag {
            max_mag = v;
        }
    }

    if max_mag <= CANNY_MIN_GRADIENT {
        return (0.0, 0.0);
    }

    let high = max_mag * 0.3;
    let low = high * 0.4;

    (low.max(CANNY_MIN_GRADIENT), high)
}

static EDGE_DETECT_SCHEMA: FilterSchema = FilterSchema {
    name: "edge_detect",
    label: "Edge Detect",
    description: "Sobel/Laplacian/Canny edge detection on L channel (produces grayscale edge map)",
    group: FilterGroup::Detail,
    params: &[
        ParamDesc {
            name: "mode",
            label: "Mode",
            description: "0 = Sobel, 1 = Laplacian, 2 = Canny (SOTA, thin connected edges)",
            kind: ParamKind::Int {
                min: 0,
                max: 2,
                default: 0,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
        },
        ParamDesc {
            name: "strength",
            label: "Strength",
            description: "Sobel/Laplacian: output scaling. Canny: Gaussian blur sigma (noise suppression).",
            kind: ParamKind::Float {
                min: 0.1,
                max: 5.0,
                default: 1.0,
                identity: 1.0,
                step: 0.1,
            },
            unit: "×",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for EdgeDetect {
    fn schema() -> &'static FilterSchema {
        &EDGE_DETECT_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "mode" => Some(ParamValue::Int(match self.mode {
                EdgeMode::Sobel => 0,
                EdgeMode::Laplacian => 1,
                EdgeMode::Canny => 2,
            })),
            "strength" => Some(ParamValue::Float(self.strength)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "mode" => {
                if let Some(v) = value.as_i32() {
                    self.mode = match v {
                        0 => EdgeMode::Sobel,
                        1 => EdgeMode::Laplacian,
                        _ => EdgeMode::Canny,
                    };
                    true
                } else {
                    false
                }
            }
            "strength" => {
                if let Some(v) = value.as_f32() {
                    self.strength = v;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;

    #[test]
    fn constant_plane_has_no_edges() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.5);
        EdgeDetect {
            mode: EdgeMode::Sobel,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.l {
            assert!(v.abs() < 1e-6, "constant plane should have zero edges, got {v}");
        }
    }

    #[test]
    fn detects_step_edge_sobel() {
        let mut planes = OklabPlanes::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.2 } else { 0.8 };
            }
        }
        EdgeDetect {
            mode: EdgeMode::Sobel,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        let edge = planes.l[planes.index(15, 16)];
        let interior = planes.l[planes.index(8, 16)];
        assert!(edge > 0.1, "edge pixel should have high gradient, got {edge}");
        assert!(
            interior < 0.01,
            "interior pixel should have near-zero gradient, got {interior}"
        );
    }

    #[test]
    fn detects_step_edge_laplacian() {
        let mut planes = OklabPlanes::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 16 { 0.2 } else { 0.8 };
            }
        }
        EdgeDetect {
            mode: EdgeMode::Laplacian,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        let edge = planes.l[planes.index(15, 16)];
        let interior = planes.l[planes.index(8, 16)];
        assert!(edge > 0.1, "edge pixel should be detected, got {edge}");
        assert!(
            interior < 0.01,
            "interior pixel should be near-zero, got {interior}"
        );
    }

    #[test]
    fn chroma_is_zeroed() {
        let mut planes = OklabPlanes::new(16, 16);
        planes.a.fill(0.1);
        planes.b.fill(-0.05);
        EdgeDetect::default().apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.a {
            assert!(v.abs() < 1e-6, "a should be zeroed, got {v}");
        }
        for &v in &planes.b {
            assert!(v.abs() < 1e-6, "b should be zeroed, got {v}");
        }
    }

    #[test]
    fn strength_amplifies_edges() {
        let mut planes_weak = OklabPlanes::new(32, 32);
        let mut planes_strong = OklabPlanes::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes_weak.index(x, y);
                let v = if x < 16 { 0.4 } else { 0.6 };
                planes_weak.l[i] = v;
                planes_strong.l[i] = v;
            }
        }

        let mut ctx = FilterContext::new();
        EdgeDetect {
            mode: EdgeMode::Sobel,
            strength: 1.0,
        }
        .apply(&mut planes_weak, &mut ctx);
        EdgeDetect {
            mode: EdgeMode::Sobel,
            strength: 3.0,
        }
        .apply(&mut planes_strong, &mut ctx);

        let weak_edge = planes_weak.l[planes_weak.index(15, 16)];
        let strong_edge = planes_strong.l[planes_strong.index(15, 16)];
        assert!(
            strong_edge > weak_edge,
            "strength should amplify: weak={weak_edge}, strong={strong_edge}"
        );
    }

    // ─── Canny tests ────────────────────────────────────────────────

    #[test]
    fn canny_constant_plane_no_edges() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.5);
        EdgeDetect {
            mode: EdgeMode::Canny,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());
        for &v in &planes.l {
            assert!(v.abs() < 1e-6, "Canny: constant plane should have no edges, got {v}");
        }
    }

    #[test]
    fn canny_detects_step_edge() {
        let mut planes = OklabPlanes::new(64, 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 32 { 0.2 } else { 0.8 };
            }
        }
        EdgeDetect {
            mode: EdgeMode::Canny,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        // Should have edges near x=31/32, not in interior
        let mut has_edge_at_boundary = false;
        let mut has_edge_at_interior = false;
        for y in 10..54u32 {
            if planes.l[planes.index(31, y)] > 0.5 || planes.l[planes.index(32, y)] > 0.5 {
                has_edge_at_boundary = true;
            }
            if planes.l[planes.index(16, y)] > 0.5 {
                has_edge_at_interior = true;
            }
        }
        assert!(has_edge_at_boundary, "Canny should detect boundary edge");
        assert!(!has_edge_at_interior, "Canny should not detect interior edges");
    }

    #[test]
    fn canny_edges_are_thin() {
        let mut planes = OklabPlanes::new(64, 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let i = planes.index(x, y);
                planes.l[i] = if x < 32 { 0.2 } else { 0.8 };
            }
        }
        EdgeDetect {
            mode: EdgeMode::Canny,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        // Count edge pixels in row 32 — should be 1 or 2 (thin edge)
        let mut edge_count = 0;
        for x in 0..64u32 {
            if planes.l[planes.index(x, 32)] > 0.5 {
                edge_count += 1;
            }
        }
        assert!(
            edge_count <= 3,
            "Canny edges should be thin (1-2px), got {edge_count} edge pixels"
        );
    }

    #[test]
    fn canny_output_is_binary() {
        let mut planes = OklabPlanes::new(64, 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let i = planes.index(x, y);
                // Gradient with some structure
                planes.l[i] = (x as f32 / 63.0) * (y as f32 / 63.0);
            }
        }
        EdgeDetect {
            mode: EdgeMode::Canny,
            strength: 1.0,
        }
        .apply(&mut planes, &mut FilterContext::new());

        // All values should be 0.0 or 1.0 (binary)
        for &v in &planes.l {
            assert!(
                v == 0.0 || v == 1.0,
                "Canny output should be binary, got {v}"
            );
        }
    }
}
