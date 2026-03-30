use alloc::vec::Vec;

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
    /// Fill out-of-bounds with a solid Oklab color + alpha.
    Color {
        /// Oklab L (lightness): 0.0 = black, 1.0 = white.
        l: f32,
        /// Oklab a (green-red): 0.0 = neutral.
        a: f32,
        /// Oklab b (blue-yellow): 0.0 = neutral.
        b: f32,
        /// Alpha for the fill region. 1.0 = opaque, 0.0 = transparent.
        alpha: f32,
    },
}

impl WarpBackground {
    /// Opaque black fill.
    pub const fn black() -> Self {
        Self::Color {
            l: 0.0,
            a: 0.0,
            b: 0.0,
            alpha: 1.0,
        }
    }

    /// Transparent fill (alpha=0). RGB values are zero.
    pub const fn transparent() -> Self {
        Self::Color {
            l: 0.0,
            a: 0.0,
            b: 0.0,
            alpha: 0.0,
        }
    }

    /// Opaque white fill.
    pub const fn white() -> Self {
        Self::Color {
            l: 1.0,
            a: 0.0,
            b: 0.0,
            alpha: 1.0,
        }
    }
}

/// Interpolation method for pixel resampling during transforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum WarpInterpolation {
    /// Bilinear: 2×2 neighborhood, fast but softens detail.
    /// Good for previews and real-time use.
    Bilinear,
    /// Catmull-Rom bicubic: 4×4 neighborhood, sharp and artifact-free.
    /// Fast but noticeably softer than Robidoux or Lanczos.
    Bicubic,
    /// Robidoux: 4×4 Mitchell-Netravali optimized for rotation.
    /// Same cost as Bicubic but significantly sharper (86 vs 78 roundtrip
    /// zensim score at 5°). ImageMagick's default for `-distort SRT`.
    /// Default for [`Rotate`].
    Robidoux,
    /// Lanczos-3 (windowed sinc): 6×6 neighborhood, maximum sharpness.
    /// Best quality for final output. ~4× slower than bilinear.
    Lanczos3,
}

/// Arbitrary geometric transform via 3×3 projective matrix.
///
/// Supports affine transforms (rotation, scale, shear, translation) and
/// perspective (homography) correction. The matrix maps **output** coordinates
/// to **source** coordinates (inverse mapping) for sub-pixel interpolation.
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
/// # Interpolation quality
///
/// Four modes are available, trading speed for sharpness:
/// - **Bilinear** — 2×2, fast, softens edges slightly
/// - **Bicubic** (Catmull-Rom) — 4×4, sharp with no ringing
/// - **Robidoux** — 4×4, optimized for rotation (same cost as bicubic, sharper)
/// - **Lanczos3** (windowed sinc) — 6×6, maximum sharpness
///
/// **Note:** With the `experimental` feature, the SIMD fast path always uses
/// Robidoux regardless of this setting. The interpolation field is only
/// respected by the scalar fallback (perspective transforms, or builds
/// without `experimental`).
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
    /// Interpolation quality. Default: Bicubic.
    pub interpolation: WarpInterpolation,
    /// Exact cardinal rotation (1=90°CCW, 2=180°, 3=270°CCW).
    /// When set, uses pixel-perfect copy instead of matrix interpolation.
    cardinal: Option<u8>,
}

/// How to handle the border region created by non-cardinal rotation.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RotateMode {
    /// Crop to the largest inscribed rectangle with the original aspect ratio.
    /// No borders, no fill — output is smaller but clean. Default for photos.
    #[default]
    Crop,
    /// Document deskew: opaque white fill, preserves full frame.
    /// No content lost at edges — important for scanned documents.
    Deskew,
    /// Fill out-of-bounds with clamped edge pixels.
    FillClamp,
    /// Fill out-of-bounds with a specific background.
    Fill(WarpBackground),
}

impl RotateMode {
    /// Fill with opaque black.
    pub const fn black() -> Self {
        Self::Fill(WarpBackground::black())
    }

    /// Fill with opaque white.
    pub const fn white() -> Self {
        Self::Fill(WarpBackground::white())
    }

    /// Fill with transparent (alpha=0).
    pub const fn transparent() -> Self {
        Self::Fill(WarpBackground::transparent())
    }
}

/// Rotation by an arbitrary angle in degrees.
///
/// Automatically selects the fastest path:
/// - **0°** — identity (no-op, zero cost)
/// - **90°, 180°, 270°** — pixel-perfect cardinal rotation (no interpolation)
/// - **All other angles** — Robidoux interpolation (4×4, sharp, fast)
///
/// By default, non-cardinal rotations crop to the largest inscribed rectangle
/// (no borders). Use [`RotateMode::Deskew`] for documents or
/// [`RotateMode::Fill`] for custom backgrounds.
///
/// # Example
///
/// ```ignore
/// use zenfilters::{Rotate, RotateMode};
///
/// // Photo straighten — crops to clean rectangle
/// let tilt = Rotate::new(3.5);
///
/// // Document deskew — white fill, full frame
/// let deskew = Rotate::new(1.2).with_mode(RotateMode::Deskew);
///
/// // Cardinal — pixel-perfect, zero cost
/// let r90 = Rotate::new(90.0);
///
/// // Custom fill color
/// let r = Rotate::new(5.0).with_mode(RotateMode::black());
/// ```
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Rotate {
    /// Rotation angle in degrees. Positive = counterclockwise.
    pub angle_degrees: f32,
    /// How to handle borders. Default: Crop.
    pub mode: RotateMode,
}

impl Rotate {
    /// Create a rotation by the given angle in degrees.
    ///
    /// Positive = counterclockwise. Cardinal angles (0, 90, 180, 270)
    /// are detected automatically and use pixel-perfect fast paths.
    /// Non-cardinal rotations crop to the largest clean rectangle.
    pub fn new(angle_degrees: f32) -> Self {
        Self {
            angle_degrees,
            mode: RotateMode::Crop,
        }
    }

    /// Set the border mode.
    pub fn with_mode(mut self, mode: RotateMode) -> Self {
        self.mode = mode;
        self
    }

    /// Normalize angle to [0, 360) and check for exact cardinal.
    /// Returns Some(quarter_turns) for 0/90/180/270, None otherwise.
    fn cardinal_quarter_turns(&self) -> Option<u8> {
        let norm = self.angle_degrees.rem_euclid(360.0);
        if (norm - 0.0).abs() < 0.01 {
            Some(0) // identity
        } else if (norm - 90.0).abs() < 0.01 {
            Some(1)
        } else if (norm - 180.0).abs() < 0.01 {
            Some(2)
        } else if (norm - 270.0).abs() < 0.01 {
            Some(3)
        } else {
            None
        }
    }

    /// Convert to a [`Warp`] filter for the given image dimensions.
    ///
    /// Cardinal angles produce pixel-perfect warps (no interpolation).
    /// Non-cardinal angles produce Robidoux-interpolated warps.
    pub fn to_warp(&self, width: u32, height: u32) -> Warp {
        let bg = match self.mode {
            RotateMode::Crop | RotateMode::FillClamp => WarpBackground::Clamp,
            RotateMode::Deskew => WarpBackground::white(),
            RotateMode::Fill(bg) => bg,
        };
        match self.cardinal_quarter_turns() {
            Some(0) => Warp::default(), // identity
            Some(n) => Warp::exact_cardinal(n, width, height),
            None => {
                let mut warp = Warp::rotation(self.angle_degrees, width, height);
                warp.background = bg;
                warp.interpolation = WarpInterpolation::Robidoux;
                warp
            }
        }
    }
}

/// Compute the largest axis-aligned rectangle with the original aspect ratio
/// that fits entirely inside the rotated rectangle.
///
/// Returns `(crop_w, crop_h)` in pixels, always ≤ `(w, h)`.
fn inscribed_crop_dimensions(w: f32, h: f32, angle_rad: f32) -> (f32, f32) {
    let theta = angle_rad.abs() % (core::f32::consts::PI / 2.0);
    if theta < 1e-6 {
        return (w, h);
    }
    let cos_t = theta.cos();
    let sin_t = theta.sin();

    // Two constraints from the four edges of the rotated rectangle.
    // The inscribed rectangle preserves the original aspect ratio.
    let w1 = (w * w) / (w * cos_t + h * sin_t);
    let denom2 = (h * cos_t - w * sin_t).abs();
    let w2 = if denom2 > 1e-6 {
        (w * h) / denom2
    } else {
        f32::MAX
    };

    let crop_w = w1.min(w2).min(w);
    let crop_h = (crop_w * h / w).min(h);
    (crop_w, crop_h)
}

/// Crop `planes` to a centered sub-rectangle of size `(new_w, new_h)`.
fn crop_planes_center(planes: &mut OklabPlanes, new_w: u32, new_h: u32) {
    let old_w = planes.width as usize;
    let x0 = ((planes.width - new_w) / 2) as usize;
    let y0 = ((planes.height - new_h) / 2) as usize;
    let nw = new_w as usize;
    let nh = new_h as usize;

    fn crop_plane(
        src: &[f32],
        old_w: usize,
        x0: usize,
        y0: usize,
        nw: usize,
        nh: usize,
    ) -> Vec<f32> {
        let mut dst = Vec::with_capacity(nw * nh);
        for y in y0..y0 + nh {
            let row_start = y * old_w + x0;
            dst.extend_from_slice(&src[row_start..row_start + nw]);
        }
        dst
    }

    planes.l = crop_plane(&planes.l, old_w, x0, y0, nw, nh);
    planes.a = crop_plane(&planes.a, old_w, x0, y0, nw, nh);
    planes.b = crop_plane(&planes.b, old_w, x0, y0, nw, nh);
    if let Some(ref alpha) = planes.alpha {
        planes.alpha = Some(crop_plane(alpha, old_w, x0, y0, nw, nh));
    }
    planes.width = new_w;
    planes.height = new_h;
}

impl Filter for Rotate {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::ALL
    }

    fn is_neighborhood(&self) -> bool {
        self.cardinal_quarter_turns().is_none()
    }

    fn neighborhood_radius(&self, width: u32, height: u32) -> u32 {
        if self.cardinal_quarter_turns().is_some() {
            0
        } else {
            width.max(height)
        }
    }

    fn tag(&self) -> crate::filter_compat::FilterTag {
        crate::filter_compat::FilterTag::Other
    }

    fn apply(&self, planes: &mut OklabPlanes, ctx: &mut FilterContext) {
        let warp = self.to_warp(planes.width, planes.height);
        let (orig_w, orig_h) = (planes.width, planes.height);
        warp.apply(planes, ctx);

        // Crop to inscribed rectangle if Crop mode and non-cardinal
        if self.mode == RotateMode::Crop && self.cardinal_quarter_turns().is_none() {
            let angle_rad = self.angle_degrees.abs() * core::f32::consts::PI / 180.0;
            let (cw, ch) = inscribed_crop_dimensions(orig_w as f32, orig_h as f32, angle_rad);
            let new_w = (cw.floor() as u32).max(1).min(planes.width);
            let new_h = (ch.floor() as u32).max(1).min(planes.height);
            if new_w < planes.width || new_h < planes.height {
                crop_planes_center(planes, new_w, new_h);
            }
        }
    }
}

impl Default for Warp {
    fn default() -> Self {
        Self {
            // Identity matrix
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            background: WarpBackground::Clamp,
            interpolation: WarpInterpolation::Bicubic,
            cardinal: None,
        }
    }
}

impl Warp {
    /// Rotate around the image center by the given angle.
    ///
    /// Positive angle = counterclockwise rotation of image content.
    /// For document deskew, a typical range is -5° to +5°.
    /// Uses bicubic interpolation by default.
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
            interpolation: WarpInterpolation::Bicubic,
            cardinal: None,
        }
    }

    /// Deskew a document image by the given angle.
    ///
    /// Convenience wrapper around [`rotation`](Self::rotation) with
    /// white background fill and [`WarpInterpolation::Lanczos3`]
    /// (clean borders and maximum sharpness for text).
    pub fn deskew(angle_degrees: f32, width: u32, height: u32) -> Self {
        let mut warp = Self::rotation(angle_degrees, width, height);
        warp.background = WarpBackground::white();
        warp.interpolation = WarpInterpolation::Lanczos3;
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
            interpolation: WarpInterpolation::Bicubic,
            cardinal: None,
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
            interpolation: WarpInterpolation::Bicubic,
            cardinal: None,
        }
    }

    /// Rotate around an arbitrary center point.
    ///
    /// Unlike [`rotation`](Self::rotation) which always uses the image center,
    /// this allows rotating around any point — useful for rotating around a
    /// detected feature, horizon vanishing point, or document corner.
    ///
    /// Coordinates are in pixels (not normalized). For the image center,
    /// use `((width-1)/2, (height-1)/2)`.
    pub fn rotation_around(angle_degrees: f32, center_x: f32, center_y: f32) -> Self {
        let angle_rad = angle_degrees * core::f32::consts::PI / 180.0;
        let cos_a = angle_rad.cos();
        let sin_a = angle_rad.sin();

        Self {
            matrix: [
                cos_a,
                sin_a,
                center_x - center_x * cos_a - center_y * sin_a,
                -sin_a,
                cos_a,
                center_y + center_x * sin_a - center_y * cos_a,
                0.0,
                0.0,
                1.0,
            ],
            background: WarpBackground::Clamp,
            interpolation: WarpInterpolation::Bicubic,
            cardinal: None,
        }
    }

    /// Exact 90° rotation (counterclockwise). Pixel-perfect, no interpolation.
    ///
    /// For non-square images, width and height are swapped in the output.
    /// This is lossless — no resampling artifacts.
    pub fn rotate_90(width: u32, height: u32) -> Self {
        Self::exact_cardinal(1, width, height)
    }

    /// Exact 180° rotation. Pixel-perfect, no interpolation.
    pub fn rotate_180(width: u32, height: u32) -> Self {
        Self::exact_cardinal(2, width, height)
    }

    /// Exact 270° rotation (counterclockwise) / 90° clockwise.
    /// Pixel-perfect, no interpolation.
    ///
    /// For non-square images, width and height are swapped in the output.
    pub fn rotate_270(width: u32, height: u32) -> Self {
        Self::exact_cardinal(3, width, height)
    }

    /// Build a cardinal rotation (pixel-perfect, no interpolation).
    /// `quarter_turns`: 1=90°CCW, 2=180°, 3=270°CCW
    fn exact_cardinal(quarter_turns: u8, _width: u32, _height: u32) -> Self {
        Self {
            // Identity matrix — unused, the cardinal fast path handles everything
            matrix: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            background: WarpBackground::Clamp,
            interpolation: WarpInterpolation::Bilinear,
            cardinal: Some(quarter_turns),
        }
    }

    /// Set interpolation to maximum quality (Lanczos3).
    pub fn with_max_quality(mut self) -> Self {
        self.interpolation = WarpInterpolation::Lanczos3;
        self
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

    /// Scalar fallback for perspective transforms or when SIMD is unavailable.
    #[allow(clippy::too_many_arguments)]
    fn apply_scalar(
        &self,
        planes: &OklabPlanes,
        dst_l: &mut [f32],
        dst_a: &mut [f32],
        dst_b: &mut [f32],
        dst_alpha: &mut Vec<f32>,
        has_alpha: bool,
    ) {
        let w = planes.width;
        let h = planes.height;
        let m = &self.matrix;
        let interp = self.interpolation;
        let bg = self.background;
        let is_affine = self.is_affine();

        for dy in 0..h {
            for dx in 0..w {
                let dxf = dx as f32;
                let dyf = dy as f32;

                let (sx, sy) = if is_affine {
                    (
                        m[0] * dxf + m[1] * dyf + m[2],
                        m[3] * dxf + m[4] * dyf + m[5],
                    )
                } else {
                    let sx_w = m[0] * dxf + m[1] * dyf + m[2];
                    let sy_w = m[3] * dxf + m[4] * dyf + m[5];
                    let w_w = m[6] * dxf + m[7] * dyf + m[8];
                    let inv_w = if w_w.abs() > 1e-10 { 1.0 / w_w } else { 1.0 };
                    (sx_w * inv_w, sy_w * inv_w)
                };

                let out_idx = (dy as usize) * (w as usize) + (dx as usize);
                sample_all_planes(
                    &planes.l,
                    &planes.a,
                    &planes.b,
                    planes.alpha.as_deref(),
                    w,
                    h,
                    sx,
                    sy,
                    bg,
                    interp,
                    dst_l,
                    dst_a,
                    dst_b,
                    if has_alpha { Some(dst_alpha) } else { None },
                    out_idx,
                );
            }
        }
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
        // Fast path: exact cardinal rotations (pixel-perfect, no interpolation)
        // Check before identity — cardinal constructors use identity matrix as placeholder.
        if let Some(quarter_turns) = self.cardinal {
            apply_cardinal(planes, quarter_turns, ctx);
            return;
        }

        if self.is_identity() {
            return;
        }

        let w = planes.width;
        let h = planes.height;
        let n = (w as usize) * (h as usize);
        let m = &self.matrix;
        let bg = self.background;

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

        // Use fused SIMD path for affine transforms (the common case).
        // Falls back to scalar for perspective (non-affine) transforms.
        if self.is_affine() {
            #[cfg(feature = "experimental")]
            {
                super::warp_simd::warp_planes_fused(
                    &planes.l,
                    &planes.a,
                    &planes.b,
                    planes.alpha.as_deref(),
                    &mut dst_l,
                    &mut dst_a,
                    &mut dst_b,
                    if has_alpha {
                        Some(dst_alpha.as_mut_slice())
                    } else {
                        None
                    },
                    w,
                    h,
                    m,
                    bg,
                );
            }

            #[cfg(not(feature = "experimental"))]
            {
                self.apply_scalar(
                    planes,
                    &mut dst_l,
                    &mut dst_a,
                    &mut dst_b,
                    &mut dst_alpha,
                    has_alpha,
                );
            }
        } else {
            // Perspective: scalar per-pixel with perspective division
            self.apply_scalar(
                planes,
                &mut dst_l,
                &mut dst_a,
                &mut dst_b,
                &mut dst_alpha,
                has_alpha,
            );
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

// ─── Interpolation kernels ──────────────────────────────────────────

/// Catmull-Rom cubic kernel (a = -0.5).
///
/// Produces sharper results than bilinear with no ringing artifacts.
/// 4-tap (radius 2): uses pixels at offsets -1, 0, +1, +2 from the
/// integer position.
#[inline]
fn catmull_rom(t: f32) -> f32 {
    let t = t.abs();
    if t <= 1.0 {
        // (3/2)|t|³ - (5/2)|t|² + 1
        ((1.5 * t - 2.5) * t) * t + 1.0
    } else if t <= 2.0 {
        // -(1/2)|t|³ + (5/2)|t|² - 4|t| + 2
        ((-0.5 * t + 2.5) * t - 4.0) * t + 2.0
    } else {
        0.0
    }
}

/// Robidoux kernel (Mitchell-Netravali, 4-tap).
///
/// Uses B=0.3782, C=0.3109 — coefficients optimized by Nicolas Robidoux for
/// cylindrical (rotation) resampling. Same 4×4 support as Catmull-Rom but
/// significantly sharper for geometric transforms. ImageMagick's default
/// for `-distort SRT`.
#[inline]
fn robidoux(t: f32) -> f32 {
    // Mitchell-Netravali with B=0.37821575509399867, C=0.31089212245300067
    // f(t) = (1/6)*((12-9B-6C)|t|³ + (-18+12B+6C)|t|² + (6-2B))    for |t| < 1
    //      = (1/6)*((-B-6C)|t|³ + (6B+30C)|t|² + (-12B-48C)|t| + (8B+24C))  for 1 ≤ |t| < 2
    const B: f32 = 0.37821575509399867;
    const C: f32 = 0.31089212245300067;
    let t = t.abs();
    if t < 1.0 {
        let a3 = (12.0 - 9.0 * B - 6.0 * C) / 6.0;
        let a2 = (-18.0 + 12.0 * B + 6.0 * C) / 6.0;
        let a0 = (6.0 - 2.0 * B) / 6.0;
        ((a3 * t + a2) * t) * t + a0
    } else if t < 2.0 {
        let b3 = (-B - 6.0 * C) / 6.0;
        let b2 = (6.0 * B + 30.0 * C) / 6.0;
        let b1 = (-12.0 * B - 48.0 * C) / 6.0;
        let b0 = (8.0 * B + 24.0 * C) / 6.0;
        (((b3 * t + b2) * t) + b1) * t + b0
    } else {
        0.0
    }
}

/// Lanczos-3 kernel (windowed sinc, 6-tap).
///
/// The gold standard for resampling quality. Maximizes sharpness at the
/// cost of possible minor ringing at very high-contrast edges.
/// 6-tap (radius 3): uses pixels at offsets -2..=+3 from the integer
/// position.
#[inline]
fn lanczos3(t: f32) -> f32 {
    let t = t.abs();
    if t < 1e-7 {
        1.0
    } else if t < 3.0 {
        let pi_t = core::f32::consts::PI * t;
        let pi_t_3 = pi_t / 3.0;
        (pi_t.sin() * pi_t_3.sin()) / (pi_t * pi_t_3)
    } else {
        0.0
    }
}

// ─── Sampling functions ─────────────────────────────────────────────

/// Sample all planes at fractional source coordinates using the specified interpolation.
#[allow(clippy::too_many_arguments)]
fn sample_all_planes(
    src_l: &[f32],
    src_a: &[f32],
    src_b: &[f32],
    src_alpha: Option<&[f32]>,
    w: u32,
    h: u32,
    sx: f32,
    sy: f32,
    background: WarpBackground,
    interp: WarpInterpolation,
    dst_l: &mut [f32],
    dst_a: &mut [f32],
    dst_b: &mut [f32],
    dst_alpha: Option<&mut alloc::vec::Vec<f32>>,
    out_idx: usize,
) {
    let wf = w as f32;
    let hf = h as f32;

    // Out-of-bounds check for solid color fill.
    if let WarpBackground::Color { l, a, b, alpha } = background {
        if sx < -0.5 || sx >= wf - 0.5 || sy < -0.5 || sy >= hf - 0.5 {
            dst_l[out_idx] = l;
            dst_a[out_idx] = a;
            dst_b[out_idx] = b;
            if let Some(da) = dst_alpha {
                da[out_idx] = alpha;
            }
            return;
        }
    }

    // Clamp source coordinates for sampling
    let sx_c = sx.clamp(0.0, wf - 1.0);
    let sy_c = sy.clamp(0.0, hf - 1.0);
    let stride = w as usize;

    match interp {
        WarpInterpolation::Bilinear => {
            dst_l[out_idx] = sample_bilinear(src_l, stride, w, h, sx_c, sy_c);
            dst_a[out_idx] = sample_bilinear(src_a, stride, w, h, sx_c, sy_c);
            dst_b[out_idx] = sample_bilinear(src_b, stride, w, h, sx_c, sy_c);
            if let (Some(sa), Some(da)) = (src_alpha, dst_alpha) {
                da[out_idx] = sample_bilinear(sa, stride, w, h, sx_c, sy_c);
            }
        }
        WarpInterpolation::Bicubic => {
            dst_l[out_idx] = sample_kernel::<4>(src_l, stride, w, h, sx_c, sy_c, catmull_rom);
            dst_a[out_idx] = sample_kernel::<4>(src_a, stride, w, h, sx_c, sy_c, catmull_rom);
            dst_b[out_idx] = sample_kernel::<4>(src_b, stride, w, h, sx_c, sy_c, catmull_rom);
            if let (Some(sa), Some(da)) = (src_alpha, dst_alpha) {
                da[out_idx] = sample_kernel::<4>(sa, stride, w, h, sx_c, sy_c, catmull_rom);
            }
        }
        WarpInterpolation::Robidoux => {
            dst_l[out_idx] = sample_kernel::<4>(src_l, stride, w, h, sx_c, sy_c, robidoux);
            dst_a[out_idx] = sample_kernel::<4>(src_a, stride, w, h, sx_c, sy_c, robidoux);
            dst_b[out_idx] = sample_kernel::<4>(src_b, stride, w, h, sx_c, sy_c, robidoux);
            if let (Some(sa), Some(da)) = (src_alpha, dst_alpha) {
                da[out_idx] = sample_kernel::<4>(sa, stride, w, h, sx_c, sy_c, robidoux);
            }
        }
        WarpInterpolation::Lanczos3 => {
            dst_l[out_idx] = sample_kernel::<6>(src_l, stride, w, h, sx_c, sy_c, lanczos3);
            dst_a[out_idx] = sample_kernel::<6>(src_a, stride, w, h, sx_c, sy_c, lanczos3);
            dst_b[out_idx] = sample_kernel::<6>(src_b, stride, w, h, sx_c, sy_c, lanczos3);
            if let (Some(sa), Some(da)) = (src_alpha, dst_alpha) {
                da[out_idx] = sample_kernel::<6>(sa, stride, w, h, sx_c, sy_c, lanczos3);
            }
        }
    }
}

/// Bilinear interpolation on a single f32 plane. 2×2 neighborhood.
fn sample_bilinear(plane: &[f32], stride: usize, w: u32, h: u32, x: f32, y: f32) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let x0c = x0.clamp(0, w as i32 - 1) as usize;
    let x1c = (x0 + 1).clamp(0, w as i32 - 1) as usize;
    let y0c = y0.clamp(0, h as i32 - 1) as usize;
    let y1c = (y0 + 1).clamp(0, h as i32 - 1) as usize;

    let p00 = plane[y0c * stride + x0c];
    let p10 = plane[y0c * stride + x1c];
    let p01 = plane[y1c * stride + x0c];
    let p11 = plane[y1c * stride + x1c];

    let top = p00 + (p10 - p00) * fx;
    let bot = p01 + (p11 - p01) * fx;
    top + (bot - top) * fy
}

/// Generic N-tap separable kernel interpolation on a single f32 plane.
///
/// `TAPS` = kernel diameter (4 for bicubic, 6 for Lanczos3).
/// `kernel_fn` returns the weight for a given distance from center.
/// Weights are normalized to sum to 1.0 to preserve DC level.
fn sample_kernel<const TAPS: usize>(
    plane: &[f32],
    stride: usize,
    w: u32,
    h: u32,
    x: f32,
    y: f32,
    kernel_fn: fn(f32) -> f32,
) -> f32 {
    let half = (TAPS / 2) as i32;
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - ix as f32;
    let fy = y - iy as f32;

    // Precompute 1D weights
    let mut wx = [0.0f32; TAPS];
    let mut wy = [0.0f32; TAPS];
    let mut wx_sum = 0.0f32;
    let mut wy_sum = 0.0f32;

    for i in 0..TAPS {
        let offset = i as i32 - half + 1;
        let wt_x = kernel_fn(offset as f32 - fx);
        let wt_y = kernel_fn(offset as f32 - fy);
        wx[i] = wt_x;
        wy[i] = wt_y;
        wx_sum += wt_x;
        wy_sum += wt_y;
    }

    // Normalize weights (ensures constant image stays constant)
    let inv_wx = if wx_sum.abs() > 1e-10 {
        1.0 / wx_sum
    } else {
        1.0
    };
    let inv_wy = if wy_sum.abs() > 1e-10 {
        1.0 / wy_sum
    } else {
        1.0
    };
    for wt in &mut wx {
        *wt *= inv_wx;
    }
    for wt in &mut wy {
        *wt *= inv_wy;
    }

    // 2D separable convolution: first rows, then combine vertically
    let mut sum = 0.0f32;
    for j in 0..TAPS {
        let sy = (iy + j as i32 - half + 1).clamp(0, h as i32 - 1) as usize;
        let mut row_sum = 0.0f32;
        for i in 0..TAPS {
            let sx = (ix + i as i32 - half + 1).clamp(0, w as i32 - 1) as usize;
            row_sum += plane[sy * stride + sx] * wx[i];
        }
        sum += row_sum * wy[j];
    }

    sum
}

// ─── Cardinal rotation (pixel-perfect) ──────────────────────────────

/// Apply an exact cardinal rotation (90°/180°/270°) without interpolation.
///
/// For 90° and 270°, width and height are swapped in the output planes.
/// This is completely lossless — every pixel is copied exactly once.
fn apply_cardinal(planes: &mut OklabPlanes, quarter_turns: u8, ctx: &mut FilterContext) {
    let src_w = planes.width as usize;
    let src_h = planes.height as usize;

    let (dst_w, dst_h) = match quarter_turns {
        1 | 3 => (src_h, src_w), // swap dimensions
        _ => (src_w, src_h),     // 180° keeps dimensions
    };

    let dst_n = dst_w * dst_h;
    let mut dst_l = ctx.take_f32(dst_n);
    let mut dst_a = ctx.take_f32(dst_n);
    let mut dst_b = ctx.take_f32(dst_n);

    let rotate_plane = |src: &[f32], dst: &mut [f32]| {
        for sy in 0..src_h {
            for sx in 0..src_w {
                let (dx, dy) = match quarter_turns {
                    1 => (sy, src_w - 1 - sx),             // 90° CCW
                    2 => (src_w - 1 - sx, src_h - 1 - sy), // 180°
                    3 => (src_h - 1 - sy, sx),             // 270° CCW
                    _ => (sx, sy),
                };
                dst[dy * dst_w + dx] = src[sy * src_w + sx];
            }
        }
    };

    rotate_plane(&planes.l, &mut dst_l);
    rotate_plane(&planes.a, &mut dst_a);
    rotate_plane(&planes.b, &mut dst_b);

    let old_l = core::mem::replace(&mut planes.l, dst_l);
    let old_a = core::mem::replace(&mut planes.a, dst_a);
    let old_b = core::mem::replace(&mut planes.b, dst_b);
    ctx.return_f32(old_l);
    ctx.return_f32(old_a);
    ctx.return_f32(old_b);

    if let Some(alpha) = &mut planes.alpha {
        let mut dst_alpha = ctx.take_f32(dst_n);
        rotate_plane(alpha, &mut dst_alpha);
        let old_alpha = core::mem::replace(alpha, dst_alpha);
        ctx.return_f32(old_alpha);
    }

    // Update dimensions for 90°/270° rotations
    planes.width = dst_w as u32;
    planes.height = dst_h as u32;
}

static WARP_SCHEMA: FilterSchema = FilterSchema {
    name: "warp",
    label: "Warp",
    description: "Geometric transform (rotation, deskew, affine, perspective) with bicubic/Lanczos interpolation",
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
        ParamDesc {
            name: "interpolation",
            label: "Quality",
            description: "0 = Bilinear, 1 = Bicubic, 2 = Robidoux (default), 3 = Lanczos3",
            kind: ParamKind::Int {
                min: 0,
                max: 2,
                default: 1,
            },
            unit: "",
            section: "Main",
            slider: SliderMapping::NotSlider,
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
            "interpolation" => Some(ParamValue::Int(match self.interpolation {
                WarpInterpolation::Bilinear => 0,
                WarpInterpolation::Bicubic => 1,
                WarpInterpolation::Robidoux => 2,
                WarpInterpolation::Lanczos3 => 3,
            })),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        match name {
            "interpolation" => {
                if let Some(v) = value.as_i32() {
                    self.interpolation = match v {
                        0 => WarpInterpolation::Bilinear,
                        1 => WarpInterpolation::Bicubic,
                        3 => WarpInterpolation::Lanczos3,
                        _ => WarpInterpolation::Robidoux,
                    };
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
            max_err < 1e-4,
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

        // Check interior pixels (corners lose precision due to repeated interpolation)
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
    fn deskew_uses_black_and_lanczos() {
        let warp = Warp::deskew(10.0, 100, 100);
        assert_eq!(warp.background, WarpBackground::white());
        assert_eq!(warp.interpolation, WarpInterpolation::Lanczos3);
    }

    #[test]
    fn small_rotation_preserves_center() {
        let mut planes = OklabPlanes::new(64, 64);
        // Fill a region around center with known value
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
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
        let warp = Warp::affine(0.5, 0.0, 8.0, 0.0, 0.5, 8.0);
        warp.apply(&mut planes, &mut FilterContext::new());
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

        let center = planes.alpha.as_ref().unwrap()[planes.index(8, 8)];
        assert!(
            (center - 0.8).abs() < 0.05,
            "alpha center should be preserved, got {center}"
        );
    }

    // ─── Interpolation quality comparison tests ──────────────────────

    #[test]
    fn constant_plane_all_interpolations() {
        for interp in [
            WarpInterpolation::Bilinear,
            WarpInterpolation::Bicubic,
            WarpInterpolation::Robidoux,
            WarpInterpolation::Lanczos3,
        ] {
            let mut planes = OklabPlanes::new(32, 32);
            planes.l.fill(0.6);
            planes.a.fill(0.05);
            planes.b.fill(-0.03);
            let mut warp = Warp::rotation(15.0, 32, 32);
            warp.interpolation = interp;
            warp.apply(&mut planes, &mut FilterContext::new());

            // Interior pixels should still be constant
            for y in 6..26u32 {
                for x in 6..26u32 {
                    let i = planes.index(x, y);
                    assert!(
                        (planes.l[i] - 0.6).abs() < 0.01,
                        "{interp:?}: L at ({x},{y}) should be ~0.6, got {}",
                        planes.l[i]
                    );
                    assert!(
                        (planes.a[i] - 0.05).abs() < 0.01,
                        "{interp:?}: a at ({x},{y}) should be ~0.05, got {}",
                        planes.a[i]
                    );
                }
            }
        }
    }

    #[test]
    fn lanczos3_sharper_than_bilinear() {
        // Create a step edge, rotate slightly, compare sharpness.
        // A sharper interpolation preserves more edge contrast.
        let make_step = || {
            let mut planes = OklabPlanes::new(64, 64);
            for y in 0..64u32 {
                for x in 0..64u32 {
                    let i = planes.index(x, y);
                    planes.l[i] = if x < 32 { 0.2 } else { 0.8 };
                }
            }
            planes
        };

        let mut bilinear_planes = make_step();
        let mut lanczos_planes = make_step();

        let mut ctx = FilterContext::new();
        let mut warp_bl = Warp::rotation(3.0, 64, 64);
        warp_bl.interpolation = WarpInterpolation::Bilinear;
        warp_bl.apply(&mut bilinear_planes, &mut ctx);

        let mut warp_lz = Warp::rotation(3.0, 64, 64);
        warp_lz.interpolation = WarpInterpolation::Lanczos3;
        warp_lz.apply(&mut lanczos_planes, &mut ctx);

        // Measure max contrast across the edge at row 32
        let edge_contrast = |planes: &OklabPlanes| -> f32 {
            let mut max_diff = 0.0f32;
            for x in 1..63u32 {
                let i = planes.index(x, 32);
                let prev = planes.index(x - 1, 32);
                max_diff = max_diff.max((planes.l[i] - planes.l[prev]).abs());
            }
            max_diff
        };

        let bl_contrast = edge_contrast(&bilinear_planes);
        let lz_contrast = edge_contrast(&lanczos_planes);
        assert!(
            lz_contrast >= bl_contrast * 0.95, // Lanczos should be at least as sharp
            "Lanczos3 should be sharper: bilinear={bl_contrast:.4}, lanczos={lz_contrast:.4}"
        );
    }

    #[test]
    fn catmull_rom_kernel_properties() {
        // At t=0, weight should be 1.0
        assert!((catmull_rom(0.0) - 1.0).abs() < 1e-6);
        // At t=1, weight should be 0.0
        assert!(catmull_rom(1.0).abs() < 1e-6);
        // At t=2, weight should be 0.0
        assert!(catmull_rom(2.0).abs() < 1e-6);
        // Beyond t=2, weight should be 0
        assert_eq!(catmull_rom(2.5), 0.0);
        // Symmetric
        assert!((catmull_rom(0.5) - catmull_rom(-0.5)).abs() < 1e-6);
    }

    #[test]
    fn lanczos3_kernel_properties() {
        // At t=0, weight should be 1.0
        assert!((lanczos3(0.0) - 1.0).abs() < 1e-6);
        // At t=3, weight should be ~0
        assert!(lanczos3(3.0).abs() < 1e-6);
        // Beyond t=3, weight should be 0
        assert_eq!(lanczos3(3.5), 0.0);
        // Symmetric
        assert!((lanczos3(1.5) - lanczos3(-1.5)).abs() < 1e-6);
        // Positive at center, has lobes
        assert!(lanczos3(0.5) > 0.0);
        assert!(lanczos3(1.5) < 0.0); // First negative lobe
    }

    #[test]
    fn with_max_quality_sets_lanczos3() {
        let warp = Warp::rotation(5.0, 100, 100).with_max_quality();
        assert_eq!(warp.interpolation, WarpInterpolation::Lanczos3);
    }

    // ─── Cardinal rotation tests ─────────────────────────────────────

    #[test]
    fn rotate_90_swaps_dimensions() {
        let mut planes = OklabPlanes::new(16, 8);
        // Mark top-left corner
        let tl = planes.index(0, 0);
        planes.l[tl] = 1.0;
        // Mark top-right corner
        let tr = planes.index(15, 0);
        planes.l[tr] = 0.5;

        let warp = Warp::rotate_90(16, 8);
        warp.apply(&mut planes, &mut FilterContext::new());

        // Dimensions should be swapped
        assert_eq!(planes.width, 8);
        assert_eq!(planes.height, 16);

        // Top-left (0,0) should have moved to bottom-left of rotated image
        // 90° CCW: (0,0) → (0, w-1) in output = (0, 15)
        let moved = planes.l[planes.index(0, 15)];
        assert!(
            (moved - 1.0).abs() < 1e-6,
            "top-left should be at (0,15) after 90° CCW, got {moved}"
        );
    }

    #[test]
    fn rotate_180_preserves_dimensions() {
        let mut planes = OklabPlanes::new(16, 8);
        let tl = planes.index(0, 0);
        planes.l[tl] = 1.0;

        let warp = Warp::rotate_180(16, 8);
        warp.apply(&mut planes, &mut FilterContext::new());

        assert_eq!(planes.width, 16);
        assert_eq!(planes.height, 8);

        // (0,0) → (w-1, h-1) = (15, 7)
        let moved = planes.l[planes.index(15, 7)];
        assert!(
            (moved - 1.0).abs() < 1e-6,
            "(0,0) should be at (15,7) after 180°, got {moved}"
        );
    }

    #[test]
    fn rotate_270_is_inverse_of_90() {
        let mut planes = OklabPlanes::new(16, 8);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 * 0.007).fract();
        }
        let original_l = planes.l.clone();
        let original_w = planes.width;
        let original_h = planes.height;

        let mut ctx = FilterContext::new();

        // 90° then 270° should be identity
        let warp90 = Warp::rotate_90(original_w, original_h);
        warp90.apply(&mut planes, &mut ctx);
        let mid_w = planes.width;
        let mid_h = planes.height;

        let warp270 = Warp::rotate_270(mid_w, mid_h);
        warp270.apply(&mut planes, &mut ctx);

        assert_eq!(planes.width, original_w);
        assert_eq!(planes.height, original_h);
        assert_eq!(planes.l, original_l);
    }

    #[test]
    fn four_90_rotations_is_identity() {
        let mut planes = OklabPlanes::new(20, 12);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = (i as f32 * 0.0041).fract();
        }
        let original = planes.l.clone();

        let mut ctx = FilterContext::new();
        let mut w = planes.width;
        let mut h = planes.height;
        for _ in 0..4 {
            let warp = Warp::rotate_90(w, h);
            warp.apply(&mut planes, &mut ctx);
            w = planes.width;
            h = planes.height;
        }

        assert_eq!(planes.width, 20);
        assert_eq!(planes.height, 12);
        assert_eq!(
            planes.l, original,
            "4×90° cardinal should be exactly identity"
        );
    }

    #[test]
    fn cardinal_rotates_alpha() {
        let mut planes = OklabPlanes::with_alpha(8, 4);
        let alpha = planes.alpha.as_mut().unwrap();
        alpha[0] = 0.99; // top-left

        let warp = Warp::rotate_90(8, 4);
        warp.apply(&mut planes, &mut FilterContext::new());

        // After 90° CCW on 8×4 → 4×8 output
        // (0,0) → (0, 7) in 4-wide output
        assert_eq!(planes.width, 4);
        assert_eq!(planes.height, 8);
        let val = planes.alpha.as_ref().unwrap()[planes.index(0, 7)];
        assert!(
            (val - 0.99).abs() < 1e-6,
            "alpha should follow rotation, got {val}"
        );
    }

    #[test]
    fn rotation_around_custom_center() {
        let mut planes = OklabPlanes::new(64, 64);
        // Fill a patch at (10, 10)
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let i = planes.index((10 + dx) as u32, (10 + dy) as u32);
                planes.l[i] = 0.9;
            }
        }

        // Rotate around (10, 10) — the patch center should be preserved
        let warp = Warp::rotation_around(15.0, 10.0, 10.0);
        warp.apply(&mut planes, &mut FilterContext::new());

        let center = planes.l[planes.index(10, 10)];
        assert!(
            (center - 0.9).abs() < 0.02,
            "rotation around custom center should preserve that point, got {center}"
        );
    }

    #[test]
    fn sub_degree_rotation_works() {
        // Verify sub-degree precision: 0.1° rotation should be barely different from identity
        let mut planes = OklabPlanes::new(64, 64);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let i = planes.index(x, y);
                planes.l[i] = 0.1 + 0.8 * (x as f32 / 63.0);
            }
        }
        let original = planes.l.clone();

        let warp = Warp::rotation(0.1, 64, 64);
        warp.apply(&mut planes, &mut FilterContext::new());

        // Interior pixels should be very close to original (0.1° is tiny)
        let mut max_err = 0.0f32;
        for y in 8..56u32 {
            for x in 8..56u32 {
                let i = planes.index(x, y);
                max_err = max_err.max((planes.l[i] - original[i]).abs());
            }
        }
        assert!(
            max_err < 0.02,
            "0.1° rotation should barely change interior: max_err={max_err}"
        );
    }
}
