//! Dimension effects for operations that change output size.
//!
//! [`DimensionEffect`] describes how an operation transforms dimensions,
//! enabling the pipeline planner to compute resize targets that account
//! for spatial transforms before and after resize.
//!
//! zenlayout provides built-in implementations for common effects
//! ([`RotateEffect`], [`PadEffect`], [`ExpandEffect`], [`TrimEffect`],
//! [`WarpEffect`]). Downstream crates can implement the trait for new
//! operations (lens distortion, content-aware resize, etc.) without
//! modifying zenlayout.

use alloc::boxed::Box;
use core::fmt::Debug;

use crate::constraint::CanvasColor;
#[allow(unused_imports)]
use crate::float_math::Float;
use crate::plan::RegionCoord;

// ── Trait ──

/// Describes how an operation changes output dimensions.
///
/// Used by [`Command::Effect`](crate::Command) in the pipeline planner.
/// Effects are processed in user-specified order — the planner tracks
/// dimensions through each step and adjusts the resize target accordingly.
pub trait DimensionEffect: Debug + Send + Sync {
    /// Output dimensions given input dimensions.
    ///
    /// Returns `None` for content-adaptive effects whose output depends on
    /// pixel analysis (e.g. auto-trim, auto-deskew). The planner treats
    /// `None` as an **analysis barrier** — it cannot plan through the effect
    /// without runtime data from an `Analyze` node.
    fn forward(&self, w: u32, h: u32) -> Option<(u32, u32)>;

    /// Required input dimensions for desired output.
    ///
    /// Returns `None` if non-invertible (content-adaptive operations) or
    /// if the inverse is ambiguous (e.g. expanded canvas at exactly 45°).
    fn inverse(&self, w: u32, h: u32) -> Option<(u32, u32)>;

    /// Map a point from input space to output space.
    ///
    /// Coordinates are in pixels relative to the top-left corner of the
    /// input. `in_w` and `in_h` are the input dimensions (needed because
    /// some effects centre their transform on the image).
    ///
    /// Returns `None` for content-adaptive effects.
    /// Default implementation scales linearly by the dimension ratio.
    fn forward_point(&self, x: f32, y: f32, in_w: u32, in_h: u32) -> Option<(f32, f32)> {
        let (out_w, out_h) = self.forward(in_w, in_h)?;
        Some((
            x * out_w as f32 / in_w.max(1) as f32,
            y * out_h as f32 / in_h.max(1) as f32,
        ))
    }

    /// Map a point from output space back to input space.
    ///
    /// Both source and output dimensions are passed as parameters so the
    /// effect can reconstruct centre-of-rotation etc. without relying on
    /// `inverse()` (which is ambiguous for some effects — e.g. expanded
    /// canvas at exactly 45°).
    ///
    /// Returns `None` for non-invertible (content-adaptive) effects.
    /// Default implementation scales linearly by the inverse dimension ratio.
    fn inverse_point(&self, x: f32, y: f32, in_w: u32, in_h: u32) -> Option<(f32, f32)> {
        let (out_w, out_h) = self.forward(in_w, in_h)?;
        Some((
            x * in_w as f32 / out_w.max(1) as f32,
            y * in_h as f32 / out_h.max(1) as f32,
        ))
    }

    /// Clone into a boxed trait object.
    fn clone_boxed(&self) -> Box<dyn DimensionEffect>;
}

impl Clone for Box<dyn DimensionEffect> {
    fn clone(&self) -> Self {
        self.clone_boxed()
    }
}

// ── Built-in effects ──

/// Rotation by an arbitrary angle.
///
/// For cardinal angles (90°/180°/270°), prefer composing into
/// [`Orientation`](crate::Orientation) instead — it's free (no resampling).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RotateEffect {
    /// Rotation angle in radians.
    pub angle_rad: f32,
    /// How rotation affects the output canvas.
    pub mode: RotateMode,
}

/// How rotation affects the output canvas.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RotateMode {
    /// Largest inscribed axis-aligned rectangle (photo straightening).
    /// Output shrinks. No fill color needed.
    InscribedCrop,
    /// Bounding box of rotated rectangle (document deskew).
    /// Output grows. Corner areas filled with color.
    Expand { color: CanvasColor },
    /// Crop to original dimensions (subtle correction).
    /// Output same size as input. Corners lost.
    CropToOriginal,
}

impl RotateEffect {
    /// Create a rotation effect from degrees.
    pub fn from_degrees(angle_degrees: f32, mode: RotateMode) -> Self {
        Self {
            angle_rad: angle_degrees.to_radians(),
            mode,
        }
    }
}

impl DimensionEffect for RotateEffect {
    fn forward(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        Some(match self.mode {
            RotateMode::InscribedCrop => inscribed_crop_dims(w, h, self.angle_rad),
            RotateMode::Expand { .. } => expanded_canvas_dims(w, h, self.angle_rad),
            RotateMode::CropToOriginal => (w, h),
        })
    }

    fn inverse(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        Some(match self.mode {
            RotateMode::InscribedCrop => inscribed_crop_inverse(w, h, self.angle_rad),
            RotateMode::Expand { .. } => expanded_canvas_inverse(w, h, self.angle_rad),
            RotateMode::CropToOriginal => (w, h),
        })
    }

    fn forward_point(&self, x: f32, y: f32, in_w: u32, in_h: u32) -> Option<(f32, f32)> {
        let (out_w, out_h) = self.forward(in_w, in_h)?;
        let fw = in_w as f32;
        let fh = in_h as f32;
        let ow = out_w as f32;
        let oh = out_h as f32;

        // Center of input and output
        let (cx_in, cy_in) = (fw / 2.0, fh / 2.0);
        let (cx_out, cy_out) = (ow / 2.0, oh / 2.0);

        // Rotate point around input center
        let (sin, cos) = (self.angle_rad.sin(), self.angle_rad.cos());
        let dx = x - cx_in;
        let dy = y - cy_in;
        let rx = dx * cos - dy * sin;
        let ry = dx * sin + dy * cos;

        // Translate to output center
        Some((rx + cx_out, ry + cy_out))
    }

    fn inverse_point(&self, x: f32, y: f32, in_w: u32, in_h: u32) -> Option<(f32, f32)> {
        let (out_w, out_h) = self.forward(in_w, in_h)?;
        let fw = in_w as f32;
        let fh = in_h as f32;
        let ow = out_w as f32;
        let oh = out_h as f32;

        let (cx_in, cy_in) = (fw / 2.0, fh / 2.0);
        let (cx_out, cy_out) = (ow / 2.0, oh / 2.0);

        // Inverse rotation (negate angle)
        let (sin, cos) = ((-self.angle_rad).sin(), (-self.angle_rad).cos());
        let dx = x - cx_out;
        let dy = y - cy_out;
        let rx = dx * cos - dy * sin;
        let ry = dx * sin + dy * cos;

        Some((rx + cx_in, ry + cy_in))
    }

    fn clone_boxed(&self) -> Box<dyn DimensionEffect> {
        Box::new(*self)
    }
}

/// Padding/border using percentage or pixel amounts.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PadEffect {
    pub top: RegionCoord,
    pub right: RegionCoord,
    pub bottom: RegionCoord,
    pub left: RegionCoord,
    pub color: CanvasColor,
}

impl PadEffect {
    /// Uniform padding as a percentage of content dimensions.
    pub fn percent(amount: f32, color: CanvasColor) -> Self {
        Self {
            top: RegionCoord::pct(amount),
            right: RegionCoord::pct(amount),
            bottom: RegionCoord::pct(amount),
            left: RegionCoord::pct(amount),
            color,
        }
    }

    /// Uniform padding in pixels.
    pub fn pixels(amount: u32, color: CanvasColor) -> Self {
        let px = amount as i32;
        Self {
            top: RegionCoord::px(px),
            right: RegionCoord::px(px),
            bottom: RegionCoord::px(px),
            left: RegionCoord::px(px),
            color,
        }
    }
}

impl DimensionEffect for PadEffect {
    fn forward(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        let left = self.left.resolve(w).max(0) as u32;
        let right = self.right.resolve(w).max(0) as u32;
        let top = self.top.resolve(h).max(0) as u32;
        let bottom = self.bottom.resolve(h).max(0) as u32;
        Some((w + left + right, h + top + bottom))
    }

    fn inverse(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        // Solve for in_w: out_w = in_w + left.resolve(in_w) + right.resolve(in_w)
        //                       = in_w * (1 + lp + rp) + lpx + rpx
        // Percent fraction (lp+rp) is always finite; pixel offsets are integers.
        let lp = (self.left.percent + self.right.percent) as f64;
        let tp = (self.top.percent + self.bottom.percent) as f64;
        let lpx = (self.left.pixels + self.right.pixels) as f64;
        let tpx = (self.top.pixels + self.bottom.pixels) as f64;
        // Reject degenerate cases: if percent = -1 the denominator is zero.
        let scale_w = 1.0 + lp;
        let scale_h = 1.0 + tp;
        if scale_w.abs() < 1e-6 || scale_h.abs() < 1e-6 {
            return None;
        }
        let in_w = ((w as f64 - lpx) / scale_w).round().max(0.0) as u32;
        let in_h = ((h as f64 - tpx) / scale_h).round().max(0.0) as u32;
        Some((in_w, in_h))
    }

    fn forward_point(&self, x: f32, y: f32, in_w: u32, in_h: u32) -> Option<(f32, f32)> {
        let left = self.left.resolve(in_w).max(0) as f32;
        let top = self.top.resolve(in_h).max(0) as f32;
        Some((x + left, y + top))
    }

    fn inverse_point(&self, x: f32, y: f32, in_w: u32, in_h: u32) -> Option<(f32, f32)> {
        let left = self.left.resolve(in_w).max(0) as f32;
        let top = self.top.resolve(in_h).max(0) as f32;
        Some((x - left, y - top))
    }

    fn clone_boxed(&self) -> Box<dyn DimensionEffect> {
        Box::new(*self)
    }
}

/// Canvas expansion by absolute pixel amounts.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ExpandEffect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl DimensionEffect for ExpandEffect {
    fn forward(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        Some((w + self.left + self.right, h + self.top + self.bottom))
    }

    fn inverse(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        Some((
            w.saturating_sub(self.left + self.right),
            h.saturating_sub(self.top + self.bottom),
        ))
    }

    fn forward_point(&self, x: f32, y: f32, _in_w: u32, _in_h: u32) -> Option<(f32, f32)> {
        Some((x + self.left as f32, y + self.top as f32))
    }

    fn inverse_point(&self, x: f32, y: f32, _in_w: u32, _in_h: u32) -> Option<(f32, f32)> {
        Some((x - self.left as f32, y - self.top as f32))
    }

    fn clone_boxed(&self) -> Box<dyn DimensionEffect> {
        Box::new(*self)
    }
}

/// Content-aware trim (**analysis barrier**).
///
/// Actual dimensions are determined at runtime by pixel analysis — the
/// planner cannot predict output size. Both `forward()` and `inverse()`
/// return `None`, signalling that the execution engine must resolve
/// dimensions via an `Analyze` node before planning can continue.
///
/// `estimated_margin_percent` is a **hint** for UI previews and fallback
/// estimates. It is NOT used by `forward()` — callers who want an
/// approximate preview can compute `(1 - 2*margin) * dim` themselves,
/// but the trait refuses to disguise it as exact.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TrimEffect {
    /// Expected margin to trim, as a fraction (0.05 = ~5% per side).
    /// This is a hint for UI preview — not used by the trait methods.
    pub estimated_margin_percent: f32,
}

impl TrimEffect {
    /// Estimated output dimensions (for UI previews only).
    ///
    /// This is NOT `forward()` — it's an explicit "I know this is a guess"
    /// API that callers opt into. The trait's `forward()` returns `None`.
    pub fn estimated_dims(&self, w: u32, h: u32) -> (u32, u32) {
        let scale = (1.0 - 2.0 * self.estimated_margin_percent).max(0.0);
        (
            (w as f32 * scale).round().max(1.0) as u32,
            (h as f32 * scale).round().max(1.0) as u32,
        )
    }
}

impl DimensionEffect for TrimEffect {
    fn forward(&self, _w: u32, _h: u32) -> Option<(u32, u32)> {
        None // Analysis barrier: actual trim depends on pixel content.
    }

    fn inverse(&self, _w: u32, _h: u32) -> Option<(u32, u32)> {
        None // Non-invertible: actual trim depends on content.
    }

    fn clone_boxed(&self) -> Box<dyn DimensionEffect> {
        Box::new(*self)
    }
}

// ── Warp / projective effects ──

/// How to choose output dimensions for a non-uniform spatial transform.
///
/// When a projective or affine transform has varying local scale (e.g.
/// perspective correction where the near edge has 2× the source pixels
/// of the far edge), this policy determines the output resolution.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ResolutionPolicy {
    /// Match the narrowest (lowest-density) edge: output resolution ensures
    /// no region is upsampled. The wide edge is downsampled. Produces the
    /// sharpest result at the cost of discarding source detail from the
    /// high-density region.
    ///
    /// Use for: document OCR, archival scans, text-heavy images.
    MatchNarrow,
    /// Match the widest (highest-density) edge: output resolution preserves
    /// all source detail. The narrow edge is upsampled (interpolated).
    /// Produces the largest output.
    ///
    /// Use for: photo editing where no source pixel should be discarded.
    MatchWide,
    /// Geometric mean of edge scales — a balanced middle ground.
    ///
    /// Use for: general-purpose perspective correction.
    MatchArea,
    /// Keep the same dimensions as input. Current zenfilters behavior.
    /// Ignores the transform's non-uniform scale.
    PreserveInput,
    /// Caller-specified exact dimensions.
    Custom(u32, u32),
}

/// Spatial warp via 3×3 projective matrix with resolution policy.
///
/// The matrix maps **output** coordinates to **source** coordinates (inverse
/// mapping), matching the convention used by zenfilters' `Warp::projective()`.
///
/// Unlike [`RotateEffect`] (which handles content coverage — inscribed crop
/// vs expand), `WarpEffect` handles **resolution**: when a transform has
/// non-uniform local scale (perspective, lens distortion), the policy
/// determines how to size the output.
///
/// # Matrix convention
///
/// ```text
/// [sx·w]   [m[0] m[1] m[2]]   [x']
/// [sy·w] = [m[3] m[4] m[5]] × [y']
/// [  w ]   [m[6] m[7] m[8]]   [ 1]
///
/// source_x = sx / w,  source_y = sy / w
/// ```
///
/// For pure affine transforms, `m[6] = m[7] = 0, m[8] = 1`.
#[derive(Clone, Debug, PartialEq)]
pub struct WarpEffect {
    /// 3×3 transform matrix (row-major), output → source.
    pub matrix: [f64; 9],
    /// Resolution policy for non-uniform scale.
    pub policy: ResolutionPolicy,
}

impl WarpEffect {
    /// Create from a 3×3 projective matrix and resolution policy.
    pub fn new(matrix: [f32; 9], policy: ResolutionPolicy) -> Self {
        Self {
            matrix: matrix.map(|v| v as f64),
            policy,
        }
    }

    /// Create from an f64 matrix.
    pub fn new_f64(matrix: [f64; 9], policy: ResolutionPolicy) -> Self {
        Self { matrix, policy }
    }
}

impl DimensionEffect for WarpEffect {
    fn forward(&self, w: u32, h: u32) -> Option<(u32, u32)> {
        Some(warp_output_dims(w, h, &self.matrix, self.policy))
    }

    fn inverse(&self, out_w: u32, out_h: u32) -> Option<(u32, u32)> {
        warp_inverse_dims(out_w, out_h, &self.matrix, self.policy)
    }

    fn forward_point(&self, x: f32, y: f32, in_w: u32, in_h: u32) -> Option<(f32, f32)> {
        let (out_w, out_h) = self.forward(in_w, in_h)?;
        // Forward point mapping: source → output via M⁻¹.
        // M maps output→source, so M⁻¹ maps source→output.
        // If output dims differ from input, scale the output coordinates.
        let inv = invert_3x3(&self.matrix)?;
        let (ox, oy) = apply_projective(&inv, x as f64, y as f64);
        // The matrix was built for output space (0..in_w, 0..in_h).
        // Scale to actual output dims.
        Some((
            (ox * out_w as f64 / in_w.max(1) as f64) as f32,
            (oy * out_h as f64 / in_h.max(1) as f64) as f32,
        ))
    }

    fn inverse_point(&self, x: f32, y: f32, in_w: u32, in_h: u32) -> Option<(f32, f32)> {
        let (out_w, out_h) = self.forward(in_w, in_h)?;
        // Inverse point: output → source via M.
        // Scale from actual output coords to matrix output space first.
        let mx = x as f64 * in_w as f64 / out_w.max(1) as f64;
        let my = y as f64 * in_h as f64 / out_h.max(1) as f64;
        let (sx, sy) = apply_projective(&self.matrix, mx, my);
        Some((sx as f32, sy as f32))
    }

    fn clone_boxed(&self) -> Box<dyn DimensionEffect> {
        Box::new(self.clone())
    }
}

/// Compute output dimensions for a 3×3 projective warp.
///
/// Maps the 4 output corners through the matrix M to get the source quad,
/// then computes edge lengths of the source quad to determine the local
/// sampling density in each direction. The policy picks output dims from
/// the per-axis scale factors.
///
/// # Arguments
/// - `w`, `h`: input (and reference output) dimensions
/// - `m`: 3×3 matrix (row-major), maps output → source
/// - `policy`: how to choose output resolution
pub fn warp_output_dims(w: u32, h: u32, m: &[f64; 9], policy: ResolutionPolicy) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    match policy {
        ResolutionPolicy::PreserveInput => (w, h),
        ResolutionPolicy::Custom(cw, ch) => (cw, ch),
        _ => {
            let fw = w as f64;
            let fh = h as f64;

            // Map the 4 output corners through M to get source corners.
            let src_tl = apply_projective(m, 0.0, 0.0);
            let src_tr = apply_projective(m, fw, 0.0);
            let src_br = apply_projective(m, fw, fh);
            let src_bl = apply_projective(m, 0.0, fh);

            // Source quad edge lengths (Euclidean distance).
            let top = dist_f64(src_tl, src_tr);
            let bottom = dist_f64(src_bl, src_br);
            let left = dist_f64(src_tl, src_bl);
            let right = dist_f64(src_tr, src_br);

            // Per-axis scale: ratio of source edge length to output edge length.
            // A scale > 1 means more source pixels than output pixels (downsampled).
            // A scale < 1 means fewer source pixels (upsampled → blur).
            let h_scales = (top / fw, bottom / fw);
            let v_scales = (left / fh, right / fh);

            let (h_scale, v_scale) = match policy {
                ResolutionPolicy::MatchNarrow => {
                    (h_scales.0.min(h_scales.1), v_scales.0.min(v_scales.1))
                }
                ResolutionPolicy::MatchWide => {
                    (h_scales.0.max(h_scales.1), v_scales.0.max(v_scales.1))
                }
                ResolutionPolicy::MatchArea => (
                    (h_scales.0 * h_scales.1).sqrt(),
                    (v_scales.0 * v_scales.1).sqrt(),
                ),
                _ => unreachable!(),
            };

            let out_w = (fw * h_scale).round().max(1.0) as u32;
            let out_h = (fh * v_scale).round().max(1.0) as u32;
            (out_w, out_h)
        }
    }
}

/// Inverse of warp output dims: what source dimensions produce `(out_w, out_h)`
/// after the warp with the given policy.
///
/// For `PreserveInput` and `Custom`, this is trivial. For scale-based policies,
/// we invert the scale factors.
fn warp_inverse_dims(
    out_w: u32,
    out_h: u32,
    m: &[f64; 9],
    policy: ResolutionPolicy,
) -> Option<(u32, u32)> {
    match policy {
        ResolutionPolicy::PreserveInput => Some((out_w, out_h)),
        ResolutionPolicy::Custom(_, _) => {
            // Custom output is independent of input — can't recover input from output.
            None
        }
        _ => {
            // For scale-based policies, forward(w, h) = (w * sx, h * sy).
            // To invert: w = out_w / sx, h = out_h / sy.
            // But sx and sy depend on w and h (the matrix is defined relative to input dims).
            // Use out_w, out_h as the initial reference dims for the scale computation
            // (since forward preserves aspect ratio approximately).
            let fw = out_w as f64;
            let fh = out_h as f64;

            let src_tl = apply_projective(m, 0.0, 0.0);
            let src_tr = apply_projective(m, fw, 0.0);
            let src_br = apply_projective(m, fw, fh);
            let src_bl = apply_projective(m, 0.0, fh);

            let top = dist_f64(src_tl, src_tr);
            let bottom = dist_f64(src_bl, src_br);
            let left = dist_f64(src_tl, src_bl);
            let right = dist_f64(src_tr, src_br);

            let h_scales = (top / fw, bottom / fw);
            let v_scales = (left / fh, right / fh);

            let (h_scale, v_scale) = match policy {
                ResolutionPolicy::MatchNarrow => {
                    (h_scales.0.min(h_scales.1), v_scales.0.min(v_scales.1))
                }
                ResolutionPolicy::MatchWide => {
                    (h_scales.0.max(h_scales.1), v_scales.0.max(v_scales.1))
                }
                ResolutionPolicy::MatchArea => (
                    (h_scales.0 * h_scales.1).sqrt(),
                    (v_scales.0 * v_scales.1).sqrt(),
                ),
                _ => unreachable!(),
            };

            if h_scale < 1e-9 || v_scale < 1e-9 {
                return None;
            }
            Some((
                (fw / h_scale).round().max(1.0) as u32,
                (fh / v_scale).round().max(1.0) as u32,
            ))
        }
    }
}

// ── 3×3 matrix helpers ──

/// Apply a 3×3 projective matrix to a point, returning (x, y) after division.
fn apply_projective(m: &[f64; 9], x: f64, y: f64) -> (f64, f64) {
    let w = m[6] * x + m[7] * y + m[8];
    if w.abs() < 1e-12 {
        return (0.0, 0.0); // degenerate
    }
    (
        (m[0] * x + m[1] * y + m[2]) / w,
        (m[3] * x + m[4] * y + m[5]) / w,
    )
}

/// Invert a 3×3 matrix. Returns `None` if singular.
fn invert_3x3(m: &[f64; 9]) -> Option<[f64; 9]> {
    let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
        + m[2] * (m[3] * m[7] - m[4] * m[6]);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    Some([
        (m[4] * m[8] - m[5] * m[7]) * inv_det,
        (m[2] * m[7] - m[1] * m[8]) * inv_det,
        (m[1] * m[5] - m[2] * m[4]) * inv_det,
        (m[5] * m[6] - m[3] * m[8]) * inv_det,
        (m[0] * m[8] - m[2] * m[6]) * inv_det,
        (m[2] * m[3] - m[0] * m[5]) * inv_det,
        (m[3] * m[7] - m[4] * m[6]) * inv_det,
        (m[1] * m[6] - m[0] * m[7]) * inv_det,
        (m[0] * m[4] - m[1] * m[3]) * inv_det,
    ])
}

/// Euclidean distance between two points.
fn dist_f64(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

// ── Pure math functions (rotation) ──
//
// All computation runs in f64 for sub-pixel precision and consistent
// rounding. Forward uses floor; inverse uses ceil and then iterates to
// find the exact preimage under forward. Monotonicity of the forward
// guarantees convergence in 0-2 iterations.

const FRAC_PI_2_F64: f64 = core::f64::consts::FRAC_PI_2;
const PI_F64: f64 = core::f64::consts::PI;

/// Normalize an angle to `[0, π/2]`. Inscribed crop and expanded canvas
/// dims are symmetric under sign flip and π/2-rotations (which swap axes
/// but we map via `θ mod π/2`).
fn fold_angle_quadrant(angle_rad: f32) -> f64 {
    // Fold to [0, π/2]. We do NOT fold across π/4 — output dims preserve
    // the original axis order so the caller sees the correct shape.
    (angle_rad as f64).abs() % FRAC_PI_2_F64
}

/// Scale factor for the aspect-ratio-preserving inscribed rectangle inside
/// a rotated w×h frame. Depends only on the aspect ratio and angle.
///
/// Derivation: the rotated rect is defined by |x*cos+y*sin| ≤ w/2 and
/// |−x*sin+y*cos| ≤ h/2. An axis-aligned rect with dims (s*w, s*h) has
/// corners at (±s*w/2, ±s*h/2). The binding corner gives two constraints:
///   s*(w*cos + h*sin) ≤ w  ⇒  s ≤ w/(w*cos+h*sin)
///   s*(w*sin + h*cos) ≤ h  ⇒  s ≤ h/(w*sin+h*cos)
/// The tighter of the two reduces to `short / (long*sin + short*cos)`.
fn inscribed_scale(w: f64, h: f64, theta: f64) -> f64 {
    let sin = theta.sin();
    let cos = theta.cos();
    let long = w.max(h);
    let short = w.min(h);
    short / (long * sin + short * cos)
}

/// Largest axis-aligned rectangle inside a rotated `w × h` frame,
/// preserving the original aspect ratio.
///
/// Returns `(crop_w, crop_h)`. For angle = 0, returns `(w, h)`.
/// Uses `floor` rounding for monotonic, roundtrip-stable results.
pub fn inscribed_crop_dims(w: u32, h: u32, angle_rad: f32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let theta = fold_angle_quadrant(angle_rad);
    if theta < 1e-12 {
        return (w, h);
    }
    let fw = w as f64;
    let fh = h as f64;
    let scale = inscribed_scale(fw, fh, theta);
    let crop_w = (fw * scale).floor().max(1.0) as u32;
    let crop_h = (fh * scale).floor().max(1.0) as u32;
    (crop_w, crop_h)
}

/// Bounding box of a rotated `w × h` rectangle.
///
/// Returns `(canvas_w, canvas_h)`. Always ≥ `(w, h)` for non-zero angles.
/// Uses `ceil` rounding.
pub fn expanded_canvas_dims(w: u32, h: u32, angle_rad: f32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let theta = (angle_rad as f64).abs() % PI_F64;
    if theta < 1e-12 {
        return (w, h);
    }
    // Fold to [0, π/2] — bounding box is symmetric across π/2.
    let theta = if theta > FRAC_PI_2_F64 {
        PI_F64 - theta
    } else {
        theta
    };
    let sin = theta.sin();
    let cos = theta.cos();
    let fw = w as f64;
    let fh = h as f64;
    let canvas_w = fw * cos + fh * sin;
    let canvas_h = fw * sin + fh * cos;
    (canvas_w.ceil() as u32, canvas_h.ceil() as u32)
}

/// Inverse of inscribed crop: source dimensions `(src_w, src_h)` such that
/// `inscribed_crop_dims(src_w, src_h, angle_rad) == (out_w, out_h)`.
///
/// Strategy: for each candidate `src_w` (starting at `out_w`, since forward
/// cannot grow width), binary-search for the smallest `src_h` where the
/// forward output height reaches `out_h`, then verify both axes match. This
/// is robust even for extreme aspect ratios where the analytic aspect-ratio
/// estimate breaks down.
pub fn inscribed_crop_inverse(out_w: u32, out_h: u32, angle_rad: f32) -> (u32, u32) {
    if out_w == 0 || out_h == 0 {
        return (0, 0);
    }
    let theta = fold_angle_quadrant(angle_rad);
    if theta < 1e-12 {
        return (out_w, out_h);
    }

    // Fast path: analytic estimate + small 2D search. Handles 99% of cases.
    let scale = inscribed_scale(out_w as f64, out_h as f64, theta);
    if scale > 0.0 {
        let est_w = ((out_w as f64) / scale).ceil() as u32;
        let est_h = ((out_h as f64) / scale).ceil() as u32;
        let mut best: Option<(u32, u32)> = None;
        for dw in -4i32..=4 {
            for dh in -4i32..=4 {
                let sw = (est_w as i32 + dw).max(1) as u32;
                let sh = (est_h as i32 + dh).max(1) as u32;
                if inscribed_crop_dims(sw, sh, angle_rad) == (out_w, out_h) {
                    let better = best.is_none_or(|(bw, bh)| sw + sh < bw + bh);
                    if better {
                        best = Some((sw, sh));
                    }
                }
            }
        }
        if let Some(ans) = best {
            return ans;
        }
    }

    // Slow path: robust 1D scan with binary search on src_h. Handles extreme
    // aspect ratios where the aspect-based estimate is badly wrong.
    let sw_max = out_w.saturating_add(64).max(out_w * 2);
    for sw in out_w..=sw_max {
        // Grow upper bound until forward.1 reaches out_h (or we give up).
        let mut hi: u32 = out_h.max(2);
        let mut growth_iters = 0;
        while inscribed_crop_dims(sw, hi, angle_rad).1 < out_h {
            if hi > 100_000_000 {
                break;
            }
            hi = hi.saturating_mul(2);
            growth_iters += 1;
            if growth_iters > 40 {
                break;
            }
        }
        if inscribed_crop_dims(sw, hi, angle_rad).1 < out_h {
            continue; // no sh reaches out_h for this sw
        }
        // Binary search smallest sh with fh >= out_h.
        let mut lo: u32 = 1;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if inscribed_crop_dims(sw, mid, angle_rad).1 >= out_h {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        // Scan a small window of sh values (all with fh == out_h due to floor)
        // looking for a matching fw.
        for sh in lo..lo.saturating_add(16) {
            let (fw, fh) = inscribed_crop_dims(sw, sh, angle_rad);
            if fh > out_h {
                break;
            }
            if fh == out_h && fw == out_w {
                return (sw, sh);
            }
        }
    }

    // Last-resort fallback.
    let scale = inscribed_scale(out_w as f64, out_h as f64, theta).max(1e-9);
    (
        ((out_w as f64) / scale).ceil() as u32,
        ((out_h as f64) / scale).ceil() as u32,
    )
}

/// Inverse of expanded canvas: source dimensions `(src_w, src_h)` such that
/// `expanded_canvas_dims(src_w, src_h, angle_rad) == (out_w, out_h)`.
///
/// At exactly 45° the inverse is not unique (any `(a, b)` with `a + b = K`
/// maps to the same square canvas). For all other angles the analytic
/// solution is near-exact and a small search refines to the exact preimage.
pub fn expanded_canvas_inverse(out_w: u32, out_h: u32, angle_rad: f32) -> (u32, u32) {
    if out_w == 0 || out_h == 0 {
        return (0, 0);
    }
    let theta = (angle_rad as f64).abs() % PI_F64;
    if theta < 1e-12 {
        return (out_w, out_h);
    }
    let theta = if theta > FRAC_PI_2_F64 {
        PI_F64 - theta
    } else {
        theta
    };
    let sin = theta.sin();
    let cos = theta.cos();
    let det = cos * cos - sin * sin;
    let fw = out_w as f64;
    let fh = out_h as f64;

    // Analytic estimate (exact when det is well-conditioned).
    let (est_w, est_h) = if det.abs() < 1e-6 {
        let side = ((fw + fh) / (2.0 * (sin + cos))).round() as u32;
        (side.max(1), side.max(1))
    } else {
        let sw = ((fw * cos - fh * sin) / det).round().max(1.0) as u32;
        let sh = ((fh * cos - fw * sin) / det).round().max(1.0) as u32;
        (sw, sh)
    };

    // 2D search around analytic estimate. Use larger window when det is small
    // (near 45° the analytic error can be multi-pixel due to ill-conditioning).
    let radius = if det.abs() < 1e-3 { 16i32 } else { 6 };
    let mut best: Option<(u32, u32)> = None;
    for dw in -radius..=radius {
        for dh in -radius..=radius {
            let sw = (est_w as i32 + dw).max(1) as u32;
            let sh = (est_h as i32 + dh).max(1) as u32;
            if expanded_canvas_dims(sw, sh, angle_rad) == (out_w, out_h) {
                let better = best.is_none_or(|(bw, bh)| sw + sh < bw + bh);
                if better {
                    best = Some((sw, sh));
                }
            }
        }
    }
    if let Some(ans) = best {
        return ans;
    }

    // Slow path for ill-conditioned cases (near 45° + extreme aspect ratios).
    //
    // The sum sw+sh is well-conditioned (determined by (ow+oh)/(sin+cos)).
    // The difference sw−sh is ill-conditioned (amplified by 1/det). We fix
    // the sum and scan along the difference direction.
    let sum_f64 = (fw + fh) / (sin + cos);
    let sum = sum_f64.round() as i64;
    let diff_est = if det.abs() > 1e-12 {
        ((fw - fh) / det).round() as i64
    } else {
        0
    };
    // Scan range proportional to 1/|det| (capped at 4096 to bound runtime).
    let diff_range = ((2.0 / det.abs().max(1e-12)).ceil() as i64).min(4096);
    let mut best2: Option<(u32, u32)> = None;
    for diff in (diff_est - diff_range)..=(diff_est + diff_range) {
        let sw2 = sum + diff;
        let sh2 = sum - diff;
        if sw2 < 2 || sh2 < 2 {
            continue;
        }
        // Try both floor and ceil halving (sum+diff may be odd).
        for &sw in &[(sw2 / 2) as u32, ((sw2 + 1) / 2) as u32] {
            for &sh in &[(sh2 / 2) as u32, ((sh2 + 1) / 2) as u32] {
                if sw == 0 || sh == 0 {
                    continue;
                }
                if expanded_canvas_dims(sw, sh, angle_rad) == (out_w, out_h) {
                    let better = best2.is_none_or(|(bw, bh)| sw + sh < bw + bh);
                    if better {
                        best2 = Some((sw, sh));
                    }
                }
            }
        }
    }
    best2.unwrap_or((est_w, est_h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inscribed_crop_zero_angle() {
        assert_eq!(inscribed_crop_dims(1000, 800, 0.0), (1000, 800));
    }

    #[test]
    fn inscribed_crop_small_angle() {
        let (w, h) = inscribed_crop_dims(1000, 800, 2.0_f32.to_radians());
        // At 2°, loss should be minimal (< 5%)
        assert!(w >= 950, "w={w}");
        assert!(h >= 760, "h={h}");
        assert!(w < 1000);
    }

    #[test]
    fn expanded_canvas_small_angle() {
        let (w, h) = expanded_canvas_dims(1000, 800, 2.0_f32.to_radians());
        // Canvas should grow
        assert!(w > 1000, "w={w}");
        assert!(h > 800, "h={h}");
        // But not by much at 2°
        assert!(w < 1050, "w={w}");
    }

    #[test]
    fn expanded_canvas_90_degrees() {
        let (w, h) = expanded_canvas_dims(1000, 800, core::f32::consts::FRAC_PI_2);
        // 90° rotation swaps dimensions
        assert!((w as i32 - 800).abs() <= 1, "w={w}");
        assert!((h as i32 - 1000).abs() <= 1, "h={h}");
    }

    #[test]
    fn inscribed_crop_inverse_roundtrip_exact() {
        // forward ∘ inverse ≡ identity exactly (no off-by-one)
        let (w, h) = (1000u32, 800u32);
        let angle = 10.0_f32.to_radians();
        let (cw, ch) = inscribed_crop_dims(w, h, angle);
        let (iw, ih) = inscribed_crop_inverse(cw, ch, angle);
        let (cw2, ch2) = inscribed_crop_dims(iw, ih, angle);
        assert_eq!((cw2, ch2), (cw, ch), "inverse must be an exact preimage");
    }

    #[test]
    fn expanded_canvas_inverse_roundtrip_exact() {
        let (w, h) = (1000u32, 800u32);
        let angle = 10.0_f32.to_radians();
        let (ew, eh) = expanded_canvas_dims(w, h, angle);
        let (iw, ih) = expanded_canvas_inverse(ew, eh, angle);
        let (ew2, eh2) = expanded_canvas_dims(iw, ih, angle);
        assert_eq!((ew2, eh2), (ew, eh), "inverse must be an exact preimage");
    }

    #[test]
    fn rotate_effect_inscribed_crop() {
        let effect = RotateEffect::from_degrees(15.0, RotateMode::InscribedCrop);
        let (w, h) = effect.forward(1000, 800).unwrap();
        assert!(w < 1000);
        assert!(h < 800);
        let (iw, ih) = effect.inverse(w, h).unwrap();
        // forward(inverse(out)) must equal out exactly
        assert_eq!(effect.forward(iw, ih).unwrap(), (w, h));
    }

    #[test]
    fn rotate_effect_expand() {
        let effect = RotateEffect::from_degrees(
            15.0,
            RotateMode::Expand {
                color: CanvasColor::Transparent,
            },
        );
        let (w, h) = effect.forward(1000, 800).unwrap();
        assert!(w > 1000, "w={w}");
        assert!(h > 800, "h={h}");
    }

    #[test]
    fn pad_effect_percent() {
        let effect = PadEffect::percent(0.1, CanvasColor::Transparent);
        let (w, h) = effect.forward(1000, 800).unwrap();
        // 10% padding on each side: 1000 + 100 + 100 = 1200
        assert_eq!(w, 1200);
        assert_eq!(h, 960);
    }

    #[test]
    fn pad_effect_inverse() {
        let effect = PadEffect::pixels(20, CanvasColor::Transparent);
        let (w, h) = effect.forward(1000, 800).unwrap();
        assert_eq!((w, h), (1040, 840));
        let (iw, ih) = effect.inverse(1040, 840).unwrap();
        assert_eq!((iw, ih), (1000, 800));
    }

    #[test]
    fn trim_is_analysis_barrier() {
        let effect = TrimEffect {
            estimated_margin_percent: 0.05,
        };
        // TrimEffect refuses to give concrete dimensions (analysis barrier).
        assert!(effect.forward(1000, 800).is_none());
        assert!(effect.inverse(900, 720).is_none());
        assert!(effect.forward_point(100.0, 100.0, 1000, 800).is_none());
        assert!(effect.inverse_point(100.0, 100.0, 1000, 800).is_none());
        // Estimated dims available via explicit opt-in method.
        let (ew, eh) = effect.estimated_dims(1000, 800);
        assert_eq!((ew, eh), (900, 720));
    }

    #[test]
    fn expand_effect() {
        let effect = ExpandEffect {
            left: 10,
            top: 20,
            right: 10,
            bottom: 20,
        };
        assert_eq!(effect.forward(100, 100), Some((120, 140)));
        assert_eq!(effect.inverse(120, 140), Some((100, 100)));
    }

    // ── Point mapping tests ──

    fn approx_eq(a: (f32, f32), b: (f32, f32), tol: f32) -> bool {
        (a.0 - b.0).abs() < tol && (a.1 - b.1).abs() < tol
    }

    #[test]
    fn rotate_point_center_stays() {
        // Center of image should stay at center after rotation
        let effect = RotateEffect::from_degrees(30.0, RotateMode::InscribedCrop);
        let (out_w, out_h) = effect.forward(1000, 800).unwrap();
        let p = effect.forward_point(500.0, 400.0, 1000, 800).unwrap();
        assert!(
            approx_eq(p, (out_w as f32 / 2.0, out_h as f32 / 2.0), 1.0),
            "center mapped to {p:?}, expected ~({}, {})",
            out_w as f32 / 2.0,
            out_h as f32 / 2.0
        );
    }

    #[test]
    fn rotate_point_roundtrip() {
        let effect = RotateEffect::from_degrees(
            15.0,
            RotateMode::Expand {
                color: CanvasColor::Transparent,
            },
        );
        let p = effect.forward_point(200.0, 300.0, 1000, 800).unwrap();
        let back = effect.inverse_point(p.0, p.1, 1000, 800).unwrap();
        assert!(
            approx_eq(back, (200.0, 300.0), 1e-3),
            "roundtrip: {back:?} expected (200, 300)"
        );
    }

    #[test]
    fn pad_point_shifts_by_padding() {
        let effect = PadEffect::pixels(20, CanvasColor::Transparent);
        let p = effect.forward_point(100.0, 50.0, 1000, 800).unwrap();
        assert_eq!(p, (120.0, 70.0)); // shifted by left=20, top=20
    }

    #[test]
    fn pad_point_inverse() {
        let effect = PadEffect::pixels(20, CanvasColor::Transparent);
        let back = effect.inverse_point(120.0, 70.0, 1000, 800).unwrap();
        assert!(approx_eq(back, (100.0, 50.0), 1e-4), "back={back:?}");
    }

    #[test]
    fn expand_point_shifts() {
        let effect = ExpandEffect {
            left: 10,
            top: 20,
            right: 10,
            bottom: 20,
        };
        assert_eq!(
            effect.forward_point(50.0, 50.0, 100, 100),
            Some((60.0, 70.0))
        );
        assert_eq!(
            effect.inverse_point(60.0, 70.0, 100, 100),
            Some((50.0, 50.0))
        );
    }

    // ── Brute-force roundtrip sweeps ──
    //
    // For the trait to be trustworthy, forward/inverse must satisfy a strict
    // pseudoinverse relation:
    //     forward(inverse(out)) == out                            (exact)
    //     forward(inverse(forward(src))) == forward(src)          (stable)
    // And point mapping must satisfy:
    //     inverse_point(forward_point(p)) ≈ p                     (within 1e-3 px)
    //
    // These sweeps sample many angles, aspect ratios, sizes, and point
    // locations — the equivalent of tracking a single red pixel through a
    // round-trip, without actually rendering anything.

    const SWEEP_DIMS: &[(u32, u32)] = &[
        (1, 1),
        (2, 2),
        (100, 100),
        (1000, 1000),
        (1920, 1080),
        (1080, 1920),
        (800, 600),
        (600, 800),
        (4000, 3000),
        (3000, 4000),
        (1, 1000),
        (1000, 1),
        (777, 333),
        (100, 31),
        (3, 7),
    ];

    const SWEEP_ANGLES_DEG: &[f32] = &[
        -89.0, -60.0, -45.0, -30.0, -15.0, -5.0, -2.0, -1.0, -0.1, 0.0, 0.1, 1.0, 2.0, 5.0, 7.5,
        10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 44.9, 45.0, 45.1, 50.0, 60.0, 75.0, 89.0, 89.9,
    ];

    #[test]
    fn inscribed_crop_forward_inverse_is_identity() {
        // forward(inverse(out)) must equal out exactly for every sample.
        for &(w, h) in SWEEP_DIMS {
            for &deg in SWEEP_ANGLES_DEG {
                let angle = deg.to_radians();
                let (ow, oh) = inscribed_crop_dims(w, h, angle);
                if ow == 0 || oh == 0 {
                    continue;
                }
                let (iw, ih) = inscribed_crop_inverse(ow, oh, angle);
                let (ow2, oh2) = inscribed_crop_dims(iw, ih, angle);
                assert_eq!(
                    (ow2, oh2),
                    (ow, oh),
                    "inscribed crop forward∘inverse not identity: \
                     src=({w},{h}) θ={deg}° out=({ow},{oh}) inv=({iw},{ih}) → ({ow2},{oh2})",
                );
            }
        }
    }

    #[test]
    fn inscribed_crop_inverse_forward_stable() {
        // inverse(forward(src)) may not equal src exactly (forward is many-to-one
        // due to floor), but forward(inverse(forward(src))) must match forward(src).
        for &(w, h) in SWEEP_DIMS {
            for &deg in SWEEP_ANGLES_DEG {
                let angle = deg.to_radians();
                let (ow, oh) = inscribed_crop_dims(w, h, angle);
                if ow == 0 || oh == 0 {
                    continue;
                }
                let (iw, ih) = inscribed_crop_inverse(ow, oh, angle);
                // And re-applying forward must give the same output.
                let (ow2, oh2) = inscribed_crop_dims(iw, ih, angle);
                assert_eq!((ow2, oh2), (ow, oh));
            }
        }
    }

    #[test]
    fn expanded_canvas_forward_inverse_is_identity() {
        for &(w, h) in SWEEP_DIMS {
            for &deg in SWEEP_ANGLES_DEG {
                let angle = deg.to_radians();
                let (ow, oh) = expanded_canvas_dims(w, h, angle);
                if ow == 0 || oh == 0 {
                    continue;
                }
                let (iw, ih) = expanded_canvas_inverse(ow, oh, angle);
                let (ow2, oh2) = expanded_canvas_dims(iw, ih, angle);
                assert_eq!(
                    (ow2, oh2),
                    (ow, oh),
                    "expanded canvas forward∘inverse not identity: \
                     src=({w},{h}) θ={deg}° out=({ow},{oh}) inv=({iw},{ih}) → ({ow2},{oh2})",
                );
            }
        }
    }

    #[test]
    fn expanded_canvas_inverse_forward_stable() {
        // At exactly 45° the inverse is not unique — any (a, b) with a+b=K maps
        // to the same square canvas, so the solver picks a square preimage that
        // may differ from the original. We only require that forward(inverse)
        // reproduces the target canvas.
        for &(w, h) in SWEEP_DIMS {
            for &deg in SWEEP_ANGLES_DEG {
                let angle = deg.to_radians();
                let (ow, oh) = expanded_canvas_dims(w, h, angle);
                if ow == 0 || oh == 0 {
                    continue;
                }
                let (iw, ih) = expanded_canvas_inverse(ow, oh, angle);
                let (ow2, oh2) = expanded_canvas_dims(iw, ih, angle);
                assert_eq!(
                    (ow2, oh2),
                    (ow, oh),
                    "expanded forward∘inverse not a fixed point: \
                     src=({w},{h}) θ={deg}° out=({ow},{oh}) inv=({iw},{ih}) → ({ow2},{oh2})",
                );
            }
        }
    }

    // ── Red-pixel point tracking sweep ──

    fn red_pixel_points(w: u32, h: u32) -> alloc::vec::Vec<(f32, f32)> {
        // Sample corners, edges, center, and an interior grid.
        let mut pts = alloc::vec::Vec::new();
        let fw = w as f32;
        let fh = h as f32;
        for &fx in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            for &fy in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
                pts.push((fx * fw, fy * fh));
            }
        }
        // Add a few fractional points to catch sub-pixel errors.
        pts.push((fw * 0.33, fh * 0.67));
        pts.push((fw * 0.61, fh * 0.18));
        pts.push((1.0, fh - 1.0));
        pts.push((fw - 1.0, 1.0));
        pts
    }

    /// Point distance for tracking tolerance assertions.
    fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    #[test]
    fn rotate_expand_point_roundtrip_sweep() {
        // forward_point then inverse_point must return the original coordinate.
        // This is the pure "track a red pixel" test — no rendering, just
        // coordinate tracking. Because the trait now takes in_w/in_h on both
        // directions, there's no dependence on inverse() (which is ambiguous
        // for expanded canvas at 45°), so all angles work.
        for &(w, h) in SWEEP_DIMS.iter().filter(|(w, h)| *w >= 4 && *h >= 4) {
            for &deg in SWEEP_ANGLES_DEG {
                let effect = RotateEffect::from_degrees(
                    deg,
                    RotateMode::Expand {
                        color: CanvasColor::Transparent,
                    },
                );
                let (ow, oh) = effect.forward(w, h).unwrap();
                if ow == 0 || oh == 0 {
                    continue;
                }
                for p in red_pixel_points(w, h) {
                    let q = effect.forward_point(p.0, p.1, w, h).unwrap();
                    let back = effect
                        .inverse_point(q.0, q.1, w, h)
                        .expect("expand inverse exists");
                    let d = dist(back, p);
                    assert!(
                        d < 1e-3,
                        "red-pixel roundtrip failed: src=({w},{h}) θ={deg}° \
                         p={p:?} → q={q:?} → back={back:?} dist={d}",
                    );
                }
            }
        }
    }

    #[test]
    fn rotate_inscribed_point_roundtrip_sweep() {
        // InscribedCrop: forward_point ∘ inverse_point is identity because
        // the rotation is centred and reversible even when the output canvas
        // is cropped. Points outside the crop may have negative or
        // out-of-bounds coords; that's fine — we still expect exact round-trip.
        for &(w, h) in SWEEP_DIMS.iter().filter(|(w, h)| *w >= 4 && *h >= 4) {
            for &deg in SWEEP_ANGLES_DEG {
                let effect = RotateEffect::from_degrees(deg, RotateMode::InscribedCrop);
                let (ow, oh) = effect.forward(w, h).unwrap();
                if ow == 0 || oh == 0 {
                    continue;
                }
                for p in red_pixel_points(w, h) {
                    let q = effect.forward_point(p.0, p.1, w, h).unwrap();
                    let back = effect.inverse_point(q.0, q.1, w, h).unwrap();
                    let d = dist(back, p);
                    assert!(
                        d < 1e-3,
                        "inscribed roundtrip failed: src=({w},{h}) θ={deg}° \
                         p={p:?} → q={q:?} → back={back:?} dist={d}",
                    );
                }
            }
        }
    }

    #[test]
    fn rotate_inscribed_center_stays_at_center() {
        // Center of source must map to center of output exactly.
        for &(w, h) in SWEEP_DIMS.iter().filter(|(w, h)| *w >= 4 && *h >= 4) {
            for &deg in SWEEP_ANGLES_DEG {
                let effect = RotateEffect::from_degrees(deg, RotateMode::InscribedCrop);
                let (ow, oh) = effect.forward(w, h).unwrap();
                if ow == 0 || oh == 0 {
                    continue;
                }
                let cx_in = w as f32 / 2.0;
                let cy_in = h as f32 / 2.0;
                let cx_out = ow as f32 / 2.0;
                let cy_out = oh as f32 / 2.0;
                let mapped = effect.forward_point(cx_in, cy_in, w, h).unwrap();
                let d = dist(mapped, (cx_out, cy_out));
                assert!(
                    d < 1e-4,
                    "center drifted: src=({w},{h}) θ={deg}° mapped={mapped:?} \
                     expected=({cx_out},{cy_out}) dist={d}",
                );
            }
        }
    }

    #[test]
    fn pad_effect_point_roundtrip_sweep() {
        use crate::plan::RegionCoord;
        let effects = [
            PadEffect::pixels(0, CanvasColor::Transparent),
            PadEffect::pixels(1, CanvasColor::Transparent),
            PadEffect::pixels(20, CanvasColor::Transparent),
            PadEffect::pixels(250, CanvasColor::Transparent),
            PadEffect::percent(0.0, CanvasColor::Transparent),
            PadEffect::percent(0.05, CanvasColor::Transparent),
            PadEffect::percent(0.25, CanvasColor::Transparent),
            // Asymmetric
            PadEffect {
                top: RegionCoord::px(10),
                right: RegionCoord::px(20),
                bottom: RegionCoord::px(30),
                left: RegionCoord::px(40),
                color: CanvasColor::Transparent,
            },
            // Mixed percentage + pixel
            PadEffect {
                top: RegionCoord::pct(0.1),
                right: RegionCoord::pct(0.05),
                bottom: RegionCoord::pct(0.1),
                left: RegionCoord::pct(0.05),
                color: CanvasColor::Transparent,
            },
        ];
        for effect in &effects {
            for &(w, h) in SWEEP_DIMS.iter().filter(|(w, h)| *w >= 4 && *h >= 4) {
                let _ = effect.forward(w, h).unwrap();
                for p in red_pixel_points(w, h) {
                    let q = effect.forward_point(p.0, p.1, w, h).unwrap();
                    let back = effect
                        .inverse_point(q.0, q.1, w, h)
                        .expect("pad inverse exists");
                    let d = dist(back, p);
                    assert!(
                        d < 1e-4,
                        "pad roundtrip failed: effect={effect:?} src=({w},{h}) \
                         p={p:?} → q={q:?} → back={back:?} dist={d}",
                    );
                }
            }
        }
    }

    #[test]
    fn expand_effect_point_roundtrip_sweep() {
        let effects = [
            ExpandEffect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            ExpandEffect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            },
            ExpandEffect {
                left: 500,
                top: 500,
                right: 500,
                bottom: 500,
            },
        ];
        for effect in &effects {
            for &(w, h) in SWEEP_DIMS {
                let (ow, oh) = effect.forward(w, h).unwrap();
                // ExpandEffect::forward/inverse are pure integer math — exact.
                if w + effect.left + effect.right == ow && h + effect.top + effect.bottom == oh {
                    let (iw, ih) = effect.inverse(ow, oh).unwrap();
                    assert_eq!((iw, ih), (w, h));
                }
                for p in red_pixel_points(w, h) {
                    let q = effect.forward_point(p.0, p.1, w, h).unwrap();
                    let back = effect.inverse_point(q.0, q.1, w, h).unwrap();
                    // ExpandEffect shifts by integer amounts — roundtrip is
                    // exact in the math, but f32 precision on large coords can
                    // produce ULP-level noise. Require ≤ 1e-4 px.
                    assert!(
                        dist(back, p) < 1e-4,
                        "expand point roundtrip: effect={effect:?} src=({w},{h}) p={p:?} back={back:?}",
                    );
                }
            }
        }
    }

    #[test]
    fn inscribed_crop_preserves_aspect_ratio() {
        // Critical invariant: inscribed crop must keep the aspect ratio
        // within 1 pixel (floor rounding is unavoidable).
        for &(w, h) in SWEEP_DIMS.iter().filter(|(w, h)| *w >= 50 && *h >= 50) {
            for &deg in SWEEP_ANGLES_DEG {
                let angle = deg.to_radians();
                let (ow, oh) = inscribed_crop_dims(w, h, angle);
                if ow == 0 || oh == 0 {
                    continue;
                }
                let src_aspect = w as f64 / h as f64;
                let out_aspect = ow as f64 / oh as f64;
                let rel_err = (src_aspect - out_aspect).abs() / src_aspect;
                // Up to ~1px floor rounding per axis means relative error
                // can be up to 1/min(ow, oh).
                let max_rel = 2.0 / (ow.min(oh) as f64);
                assert!(
                    rel_err < max_rel,
                    "aspect drifted: src=({w},{h})={src_aspect:.6} \
                     out=({ow},{oh})={out_aspect:.6} θ={deg}° err={rel_err}",
                );
            }
        }
    }

    #[test]
    fn inscribed_crop_is_monotonic_in_angle() {
        // For any fixed source, larger |angle| (within [0, π/4]) means
        // smaller inscribed crop. This is critical for inverse iteration.
        let (w, h) = (1000u32, 800u32);
        let angles: alloc::vec::Vec<f32> = (0..=45).map(|d| (d as f32).to_radians()).collect();
        let mut prev_area = u64::MAX;
        for &a in &angles {
            let (cw, ch) = inscribed_crop_dims(w, h, a);
            let area = cw as u64 * ch as u64;
            assert!(
                area <= prev_area,
                "inscribed crop not monotonic: angle={}° area={area} prev={prev_area}",
                a.to_degrees()
            );
            prev_area = area;
        }
    }

    #[test]
    fn expanded_canvas_is_monotonic_in_angle() {
        // Larger |angle| (within [0, π/4]) means a larger continuous canvas.
        // The ceil rounding at integer dims can dip by 1 px between adjacent
        // integer degrees, so the raw area test allows a 2-pixel slack against
        // the continuous growth — but on a coarser grid (every 5°) the
        // monotonicity is strict.
        let (w, h) = (1000u32, 800u32);
        let angles: alloc::vec::Vec<f32> = (0..=9).map(|i| (i as f32 * 5.0).to_radians()).collect();
        let mut prev_area = 0u64;
        for &a in &angles {
            let (cw, ch) = expanded_canvas_dims(w, h, a);
            let area = cw as u64 * ch as u64;
            assert!(
                area >= prev_area,
                "expanded canvas not monotonic on 5° grid: angle={}° area={area} prev={prev_area}",
                a.to_degrees()
            );
            prev_area = area;
        }
    }

    #[test]
    fn inscribed_crop_known_45_degrees() {
        // 45° on a unit square: inscribed is √2/2 ≈ 0.7071 of original.
        let (w, h) = inscribed_crop_dims(1000, 1000, core::f32::consts::FRAC_PI_4);
        let expected = (1000.0 / 2.0_f64.sqrt()).floor() as u32;
        assert_eq!(w, expected);
        assert_eq!(h, expected);
    }

    #[test]
    fn expanded_canvas_known_45_degrees() {
        // 45° on a square: canvas side is w*√2.
        let (w, h) = expanded_canvas_dims(1000, 1000, core::f32::consts::FRAC_PI_4);
        let expected = (1000.0 * 2.0_f64.sqrt()).ceil() as u32;
        assert_eq!(w, expected);
        assert_eq!(h, expected);
    }

    // ── WarpEffect tests ──

    /// Identity matrix → all policies give input dims.
    #[test]
    fn warp_identity_preserves_dims() {
        let identity = [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        for policy in [
            ResolutionPolicy::MatchNarrow,
            ResolutionPolicy::MatchWide,
            ResolutionPolicy::MatchArea,
            ResolutionPolicy::PreserveInput,
        ] {
            let effect = WarpEffect::new(identity, policy);
            assert_eq!(
                effect.forward(1920, 1080).unwrap(),
                (1920, 1080),
                "identity + {policy:?} should preserve dims"
            );
        }
    }

    /// Custom policy returns the specified dims regardless of matrix.
    #[test]
    fn warp_custom_policy() {
        let identity = [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let effect = WarpEffect::new(identity, ResolutionPolicy::Custom(800, 600));
        assert_eq!(effect.forward(1920, 1080).unwrap(), (800, 600));
    }

    /// Uniform 2× scale matrix → output is 2× input.
    #[test]
    fn warp_uniform_scale() {
        // M maps output(x,y) → source(2x, 2y): each output pixel covers 2×2 source.
        let m = [2.0f32, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0];
        let effect = WarpEffect::new(m, ResolutionPolicy::MatchArea);
        let (w, h) = effect.forward(1000, 800).unwrap();
        // Source quad edges are 2× output edges, so scale factor ≈ 2.
        assert!((w as f64 - 2000.0).abs() < 2.0, "w={w} expected ~2000");
        assert!((h as f64 - 1600.0).abs() < 2.0, "h={h} expected ~1600");
    }

    /// Perspective trapezoid: near edge wider than far edge.
    /// MatchNarrow should give smaller output than MatchWide.
    #[test]
    fn warp_perspective_narrow_vs_wide() {
        // Build a matrix that maps a rectangle to a trapezoid:
        // Top edge (far): 600px of source content
        // Bottom edge (near): 1000px of source content
        // This simulates looking down at a document.
        let src = [
            (200.0f64, 0.0),
            (800.0, 0.0),
            (1000.0, 1000.0),
            (0.0, 1000.0),
        ];
        let dst = [
            (0.0f64, 0.0),
            (1000.0, 0.0),
            (1000.0, 1000.0),
            (0.0, 1000.0),
        ];

        // Compute homography (dst→src) using the same DLT as zenfilters.
        let m = compute_test_homography(&src, &dst).expect("non-degenerate");

        let narrow = WarpEffect::new_f64(m, ResolutionPolicy::MatchNarrow);
        let wide = WarpEffect::new_f64(m, ResolutionPolicy::MatchWide);
        let area = WarpEffect::new_f64(m, ResolutionPolicy::MatchArea);

        let (nw, _nh) = narrow.forward(1000, 1000).unwrap();
        let (ww, _wh) = wide.forward(1000, 1000).unwrap();
        let (aw, _ah) = area.forward(1000, 1000).unwrap();

        assert!(
            nw < ww,
            "MatchNarrow width ({nw}) should be < MatchWide width ({ww})"
        );
        assert!(
            aw > nw && aw < ww,
            "MatchArea width ({aw}) should be between narrow ({nw}) and wide ({ww})"
        );

        // MatchNarrow: top edge is 600px of source in 1000px output → scale = 0.6.
        // So output should be ~600px wide.
        assert!(
            (nw as f64 - 600.0).abs() < 50.0,
            "MatchNarrow width {nw} should be near 600"
        );
        // MatchWide: bottom edge is 1000px → scale = 1.0 → output = 1000px.
        assert!(
            (ww as f64 - 1000.0).abs() < 50.0,
            "MatchWide width {ww} should be near 1000"
        );
    }

    /// WarpEffect point roundtrip: forward_point ∘ inverse_point ≈ identity.
    #[test]
    fn warp_point_roundtrip() {
        // Slight rotation matrix (5° around center of 1000×800).
        let angle = 5.0f64 * core::f64::consts::PI / 180.0;
        let cos = angle.cos();
        let sin = angle.sin();
        let cx = 499.5;
        let cy = 399.5;
        let m = [
            cos,
            sin,
            cx - cx * cos - cy * sin,
            -sin,
            cos,
            cy + cx * sin - cy * cos,
            0.0,
            0.0,
            1.0,
        ];
        let effect = WarpEffect::new_f64(m, ResolutionPolicy::PreserveInput);
        let pts = [
            (500.0f32, 400.0),
            (100.0, 200.0),
            (900.0, 50.0),
            (0.0, 799.0),
        ];
        for p in pts {
            let q = effect.forward_point(p.0, p.1, 1000, 800).unwrap();
            let back = effect.inverse_point(q.0, q.1, 1000, 800).unwrap();
            let d = dist(back, p);
            assert!(
                d < 0.1,
                "warp point roundtrip: p={p:?} → q={q:?} → back={back:?} dist={d}"
            );
        }
    }

    /// WarpEffect with PreserveInput + identity = RotateEffect with CropToOriginal.
    #[test]
    fn warp_rotation_matches_rotate_effect() {
        // Pure rotation, PreserveInput policy → same dims as CropToOriginal.
        let angle_deg = 15.0f32;
        let angle_rad = angle_deg * core::f32::consts::PI / 180.0;
        let cos = angle_rad.cos() as f64;
        let sin = angle_rad.sin() as f64;
        let cx = 499.5;
        let cy = 399.5;
        let m = [
            cos,
            sin,
            cx - cx * cos - cy * sin,
            -sin,
            cos,
            cy + cx * sin - cy * cos,
            0.0,
            0.0,
            1.0,
        ];

        let warp = WarpEffect::new_f64(m, ResolutionPolicy::PreserveInput);
        let rotate = RotateEffect::from_degrees(angle_deg, RotateMode::CropToOriginal);

        assert_eq!(
            warp.forward(1000, 800).unwrap(),
            rotate.forward(1000, 800).unwrap(),
            "WarpEffect(PreserveInput) should match RotateEffect(CropToOriginal)"
        );
    }

    /// Helper: compute 3×3 homography from 4 point correspondences (DLT).
    /// Mirrors zenfilters' compute_homography but in f64 for test precision.
    fn compute_test_homography(src: &[(f64, f64); 4], dst: &[(f64, f64); 4]) -> Option<[f64; 9]> {
        let mut a = [[0.0f64; 9]; 8];
        for i in 0..4 {
            let (xs, ys) = src[i];
            let (xd, yd) = dst[i];
            let r0 = i * 2;
            let r1 = i * 2 + 1;
            a[r0][0] = xd;
            a[r0][1] = yd;
            a[r0][2] = 1.0;
            a[r0][6] = -xd * xs;
            a[r0][7] = -yd * xs;
            a[r0][8] = xs;
            a[r1][3] = xd;
            a[r1][4] = yd;
            a[r1][5] = 1.0;
            a[r1][6] = -xd * ys;
            a[r1][7] = -yd * ys;
            a[r1][8] = ys;
        }
        for col in 0..8 {
            let mut max_row = col;
            let mut max_val = a[col][col].abs();
            for row in (col + 1)..8 {
                if a[row][col].abs() > max_val {
                    max_val = a[row][col].abs();
                    max_row = row;
                }
            }
            if max_val < 1e-12 {
                return None;
            }
            a.swap(col, max_row);
            let pivot = a[col][col];
            for row in (col + 1)..8 {
                let f = a[row][col] / pivot;
                for c in col..9 {
                    a[row][c] -= f * a[col][c];
                }
            }
        }
        let mut h = [0.0f64; 8];
        for col in (0..8).rev() {
            let mut s = a[col][8];
            for c in (col + 1)..8 {
                s -= a[col][c] * h[c];
            }
            h[col] = s / a[col][col];
        }
        Some([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0])
    }
}
