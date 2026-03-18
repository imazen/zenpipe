use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;

/// Background mode for out-of-bounds pixels during geometric transforms.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WarpBackground {
    /// Clamp to nearest edge pixel (default, best for photos).
    Clamp,
    /// Set out-of-bounds pixels to black (L=0, a=0, b=0).
    /// Useful for documents where borders should be clean.
    Black,
}

/// Arbitrary geometric transform via 3×3 projective matrix.
///
/// Supports affine transforms (rotation, scale, shear, translation) and
/// perspective (homography) correction. The matrix maps **output** coordinates
/// to **source** coordinates (inverse mapping) for bilinear interpolation.
///
/// Key use cases:
/// - **Document deskew**: straighten scanned text (small rotation)
/// - **Photo straighten**: level the horizon
/// - **Perspective correction**: fix converging verticals (projective)
/// - **Arbitrary affine**: combine rotation + scale + shear
///
/// Output dimensions match the input (crop-to-fit). For small rotations
/// like deskew (< 5°), minimal content is lost.
///
/// # Matrix convention
///
/// The 3×3 matrix M maps output pixel (x', y') to source pixel (sx, sy):
///
/// ```text
/// [sx·w]   [m[0] m[1] m[2]]   [x']
/// [sy·w] = [m[3] m[4] m[5]] × [y']
/// [  w ]   [m[6] m[7] m[8]]   [ 1]
///
/// source_x = sx·w / w
/// source_y = sy·w / w
/// ```
///
/// For pure affine transforms, m\[6\]=0, m\[7\]=0, m\[8\]=1 (no perspective).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Warp {
    /// 3×3 transform matrix in row-major order.
    /// Maps output coordinates to source coordinates.
    pub matrix: [f32; 9],
    /// How to handle out-of-bounds source pixels.
    pub background: WarpBackground,
}

impl Default for Warp {
    fn default() -> Self {
        Self {
            // Identity matrix
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            background: WarpBackground::Clamp,
        }
    }
}

impl Warp {
    /// Rotate around the image center by the given angle.
    ///
    /// Positive angle = counterclockwise rotation of image content.
    /// For document deskew, a typical range is -5° to +5°.
    pub fn rotation(angle_degrees: f32, width: u32, height: u32) -> Self {
        let angle_rad = angle_degrees * core::f32::consts::PI / 180.0;
        let cos_a = angle_rad.cos();
        let sin_a = angle_rad.sin();
        let cx = (width as f32 - 1.0) * 0.5;
        let cy = (height as f32 - 1.0) * 0.5;

        // Inverse mapping: output → source
        // Translate center to origin, rotate by -angle, translate back.
        // Since cos(-a) = cos(a) and sin(-a) = -sin(a):
        Self {
            matrix: [
                cos_a,
                sin_a,
                cx - cx * cos_a - cy * sin_a,
                -sin_a,
                cos_a,
                cy + cx * sin_a - cy * cos_a,
                0.0,
                0.0,
                1.0,
            ],
            background: WarpBackground::Clamp,
        }
    }

    /// Deskew a document image by the given angle.
    ///
    /// Convenience wrapper around [`rotation`](Self::rotation) with
    /// [`WarpBackground::Black`] (clean borders for documents).
    pub fn deskew(angle_degrees: f32, width: u32, height: u32) -> Self {
        let mut warp = Self::rotation(angle_degrees, width, height);
        warp.background = WarpBackground::Black;
        warp
    }

    /// Construct from a raw 2×3 affine matrix.
    ///
    /// ```text
    /// [sx]   [a  b  tx]   [x']
    /// [sy] = [c  d  ty] × [y']
    ///                      [ 1]
    /// ```
    pub fn affine(a: f32, b: f32, tx: f32, c: f32, d: f32, ty: f32) -> Self {
        Self {
            matrix: [a, b, tx, c, d, ty, 0.0, 0.0, 1.0],
            background: WarpBackground::Clamp,
        }
    }

    /// Construct from a full 3×3 projective (homography) matrix.
    ///
    /// The matrix maps output coordinates to source coordinates.
    /// The last row enables perspective correction (non-zero m\[6\], m\[7\]).
    pub fn projective(matrix: [f32; 9]) -> Self {
        Self {
            matrix,
            background: WarpBackground::Clamp,
        }
    }

    /// Check if this transform is the identity (no-op).
    fn is_identity(&self) -> bool {
        let m = &self.matrix;
        (m[0] - 1.0).abs() < 1e-7
            && m[1].abs() < 1e-7
            && m[2].abs() < 1e-7
            && m[3].abs() < 1e-7
            && (m[4] - 1.0).abs() < 1e-7
            && m[5].abs() < 1e-7
            && m[6].abs() < 1e-7
            && m[7].abs() < 1e-7
            && (m[8] - 1.0).abs() < 1e-7
    }

    /// Check if this is a pure affine transform (no perspective).
    fn is_affine(&self) -> bool {
        self.matrix[6].abs() < 1e-7
            && self.matrix[7].abs() < 1e-7
            && (self.matrix[8] - 1.0).abs() < 1e-7
    }
}

impl Filter for Warp {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::ALL
    }

    fn is_neighborhood(&self) -> bool {
        true
    }

    fn neighborhood_radius(&self, width: u32, height: u32) -> u32 {
        // Warp needs access to the full image (any pixel can map anywhere)
        width.max(height)
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::Other
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        if self.is_identity() {
            return;
        }

        let w = planes.width;
        let h = planes.height;
        let n = (w as usize) * (h as usize);
        let m = &self.matrix;

        // Allocate output planes
        let mut dst_l = ctx.take_f32(n);
        let mut dst_a = ctx.take_f32(n);
        let mut dst_b = ctx.take_f32(n);

        let has_alpha = planes.alpha.is_some();
        let mut dst_alpha = if has_alpha {
            ctx.take_f32(n)
        } else {
            alloc::vec::Vec::new()
        };

        if self.is_affine() {
            // Fast path: no perspective division
            for dy in 0..h {
                for dx in 0..w {
                    let dxf = dx as f32;
                    let dyf = dy as f32;

                    let sx = m[0] * dxf + m[1] * dyf + m[2];
                    let sy = m[3] * dxf + m[4] * dyf + m[5];

                    let out_idx = (dy as usize) * (w as usize) + (dx as usize);
                    sample_bilinear_all(
                        &planes.l,
                        &planes.a,
                        &planes.b,
                        planes.alpha.as_deref(),
                        w,
                        h,
                        sx,
                        sy,
                        self.background,
                        &mut dst_l,
                        &mut dst_a,
                        &mut dst_b,
                        if has_alpha {
                            Some(&mut dst_alpha)
                        } else {
                            None
                        },
                        out_idx,
                    );
                }
            }
        } else {
            // Projective path: divide by w
            for dy in 0..h {
                for dx in 0..w {
                    let dxf = dx as f32;
                    let dyf = dy as f32;

                    let sx_w = m[0] * dxf + m[1] * dyf + m[2];
                    let sy_w = m[3] * dxf + m[4] * dyf + m[5];
                    let w_w = m[6] * dxf + m[7] * dyf + m[8];

                    let inv_w = if w_w.abs() > 1e-10 {
                        1.0 / w_w
                    } else {
                        1.0
                    };
                    let sx = sx_w * inv_w;
                    let sy = sy_w * inv_w;

                    let out_idx = (dy as usize) * (w as usize) + (dx as usize);
                    sample_bilinear_all(
                        &planes.l,
                        &planes.a,
                        &planes.b,
                        planes.alpha.as_deref(),
                        w,
                        h,
                        sx,
                        sy,
                        self.background,
                        &mut dst_l,
                        &mut dst_a,
                        &mut dst_b,
                        if has_alpha {
                            Some(&mut dst_alpha)
                        } else {
                            None
                        },
                        out_idx,
                    );
                }
            }
        }

        // Replace planes with warped result
        let old_l = core::mem::replace(&mut planes.l, dst_l);
        let old_a = core::mem::replace(&mut planes.a, dst_a);
        let old_b = core::mem::replace(&mut planes.b, dst_b);
        ctx.return_f32(old_l);
        ctx.return_f32(old_a);
        ctx.return_f32(old_b);

        if has_alpha {
            let old_alpha = core::mem::replace(planes.alpha.as_mut().unwrap(), dst_alpha);
            ctx.return_f32(old_alpha);
        }
    }
}

/// Bilinear interpolation of all planes at fractional source coordinates.
#[allow(clippy::too_many_arguments)]
fn sample_bilinear_all(
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    src_alpha: Option<&[f32]>,
    w: u32,
    h: u32,
    sx: f32,
    sy: f32,
    background: WarpBackground,
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    dst_alpha: Option<&mut alloc::vec::Vec<f32>>,
    out_idx: usize,
) {
    let w_s = w as usize;
    let wf = w as f32;
    let hf = h as f32;

    match background {
        WarpBackground::Clamp => {
            // Clamp source coordinates to valid range
            let sx_c = sx.clamp(0.0, wf - 1.0);
            let sy_c = sy.clamp(0.0, hf - 1.0);

            dst_l[out_idx] = sample_bilinear_plane(src_l, w_s, w, h, sx_c, sy_c);
            dst_a[out_idx] = sample_bilinear_plane(src_a, w_s, w, h, sx_c, sy_c);
            dst_b[out_idx] = sample_bilinear_plane(src_b, w_s, w, h, sx_c, sy_c);
            if let (Some(sa), Some(da)) = (src_alpha, dst_alpha) {
                da[out_idx] = sample_bilinear_plane(sa, w_s, w, h, sx_c, sy_c);
            }
        }
        WarpBackground::Black => {
            if sx < -0.5 || sx >= wf - 0.5 || sy < -0.5 || sy >= hf - 0.5 {
                // Out of bounds → black
                dst_l[out_idx] = 0.0;
                dst_a[out_idx] = 0.0;
                dst_b[out_idx] = 0.0;
                if let Some(da) = dst_alpha {
                    da[out_idx] = 0.0;
                }
            } else {
                let sx_c = sx.clamp(0.0, wf - 1.0);
                let sy_c = sy.clamp(0.0, hf - 1.0);
                dst_l[out_idx] = sample_bilinear_plane(src_l, w_s, w, h, sx_c, sy_c);
                dst_a[out_idx] = sample_bilinear_plane(src_a, w_s, w, h, sx_c, sy_c);
                dst_b[out_idx] = sample_bilinear_plane(src_b, w_s, w, h, sx_c, sy_c);
                if let (Some(sa), Some(da)) = (src_alpha, dst_alpha) {
                    da[out_idx] = sample_bilinear_plane(sa, w_s, w, h, sx_c, sy_c);
                }
            }
        }
    }
}

/// Bilinear interpolation on a single f32 plane.
fn sample_bilinear_plane(
    plane: &[f32],
    stride: usize,
    w: u32,
    h: u32,
    x: f32,
    y: f32,
) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    // Clamp to valid pixel range
    let x0c = x0.clamp(0, w as i32 - 1) as usize;
    let x1c = x1.clamp(0, w as i32 - 1) as usize;
    let y0c = y0.clamp(0, h as i32 - 1) as usize;
    let y1c = y1.clamp(0, h as i32 - 1) as usize;

    let p00 = plane[y0c * stride + x0c];
    let p10 = plane[y0c * stride + x1c];
    let p01 = plane[y1c * stride + x0c];
    let p11 = plane[y1c * stride + x1c];

    let top = p00 + (p10 - p00) * fx;
    let bot = p01 + (p11 - p01) * fx;
    top + (bot - top) * fy
}

static WARP_SCHEMA: FilterSchema = FilterSchema {
    name: "warp",
    label: "Warp",
    description: "Geometric transform (rotation, deskew, affine, perspective)",
    group: FilterGroup::Effects,
    params: &[
        ParamDesc {
            name: "angle",
            label: "Rotation Angle",
            description: "Rotation in degrees (positive = counterclockwise). Use rotation() or deskew() constructors.",
            kind: ParamKind::Float {
                min: -180.0,
                max: 180.0,
                default: 0.0,
                identity: 0.0,
                step: 0.1,
            },
            unit: "°",
            section: "Main",
            slider: SliderMapping::Linear,
        },
    ],
};

impl Describe for Warp {
    fn schema() -> &'static FilterSchema {
        &WARP_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "angle" => {
                // Extract angle from matrix (atan2 of rotation component)
                let angle_rad = self.matrix[3].atan2(self.matrix[0]);
                // Negate because matrix stores inverse mapping
                let angle_deg = -angle_rad * 180.0 / core::f32::consts::PI;
                Some(ParamValue::Float(angle_deg))
            }
            _ => None,
        }
    }

    fn set_param(&mut self, _name: &str, _value: ParamValue) -> bool {
        // Matrix-based parameters can't be set individually.
        // Use the constructors (rotation, deskew, affine, projective).
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::FilterContext;

    #[test]
    fn identity_is_noop() {
        let mut planes = OklabPlanes::new(16, 16);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = i as f32 / 256.0;
        }
        let original = planes.l.clone();
        Warp::default().apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, original);
    }

    #[test]
    fn zero_rotation_is_noop() {
        let mut planes = OklabPlanes::new(32, 32);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 * 0.0037).fract();
        }
        let original = planes.l.clone();
        let warp = Warp::rotation(0.0, 32, 32);
        warp.apply(&mut planes, &mut FilterContext::new());

        let mut max_err = 0.0f32;
        for (a, b) in planes.l.iter().zip(original.iter()) {
            max_err = max_err.max((a - b).abs());
        }
        assert!(
            max_err < 1e-5,
            "zero rotation should be near-identity, max_err={max_err}"
        );
    }

    #[test]
    fn rotation_360_is_near_identity() {
        let mut planes = OklabPlanes::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                let i = planes.index(x, y);
                planes.l[i] = 0.1 + 0.8 * (x as f32 / 31.0) * (y as f32 / 31.0);
            }
        }
        let original = planes.l.clone();

        // Apply 4 × 90° rotations (should return to start)
        let mut ctx = FilterContext::new();
        for _ in 0..4 {
            let warp = Warp::rotation(90.0, 32, 32);
            warp.apply(&mut planes, &mut ctx);
        }

        // Check interior pixels (corners lose precision due to repeated bilinear)
        let mut max_err = 0.0f32;
        for y in 4..28u32 {
            for x in 4..28u32 {
                let i = planes.index(x, y);
                let err = (planes.l[i] - original[i]).abs();
                max_err = max_err.max(err);
            }
        }
        assert!(
            max_err < 0.05,
            "4×90° rotation should be near-identity, interior max_err={max_err}"
        );
    }

    #[test]
    fn deskew_uses_black_background() {
        let warp = Warp::deskew(10.0, 100, 100);
        assert_eq!(warp.background, WarpBackground::Black);
    }

    #[test]
    fn small_rotation_preserves_center() {
        let mut planes = OklabPlanes::new(64, 64);
        // Put a known value at the center
        let center_idx = planes.index(32, 32);
        planes.l[center_idx] = 0.75;
        // Neighbors should also be ~0.75 for interpolation to work
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let i = planes.index((32 + dx) as u32, (32 + dy) as u32);
                planes.l[i] = 0.75;
            }
        }

        let warp = Warp::rotation(2.0, 64, 64); // Small rotation
        warp.apply(&mut planes, &mut FilterContext::new());

        let center_val = planes.l[planes.index(32, 32)];
        assert!(
            (center_val - 0.75).abs() < 0.01,
            "center should be preserved under small rotation, got {center_val}"
        );
    }

    #[test]
    fn affine_scale_works() {
        let mut planes = OklabPlanes::new(32, 32);
        planes.l.fill(0.5);
        // Scale 2x centered: inverse mapping = scale 0.5x
        // For output (x,y), source = (x*0.5 + 8, y*0.5 + 8)
        let warp = Warp::affine(0.5, 0.0, 8.0, 0.0, 0.5, 8.0);
        warp.apply(&mut planes, &mut FilterContext::new());
        // With constant input, output should still be constant
        for &v in &planes.l {
            assert!(
                (v - 0.5).abs() < 0.01,
                "constant image under scale should stay constant, got {v}"
            );
        }
    }

    #[test]
    fn handles_alpha() {
        let mut planes = OklabPlanes::with_alpha(16, 16);
        planes.alpha.as_mut().unwrap().fill(0.8);

        let warp = Warp::rotation(5.0, 16, 16);
        warp.apply(&mut planes, &mut FilterContext::new());

        // Center should still be ~0.8
        let center = planes.alpha.as_ref().unwrap()[planes.index(8, 8)];
        assert!(
            (center - 0.8).abs() < 0.05,
            "alpha center should be preserved, got {center}"
        );
    }
}
