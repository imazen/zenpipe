//! Pixel operations that can be fused into a [`TransformSource`](crate::sources::TransformSource).
//!
//! Per-pixel ops are stateless and process strips independently — they need
//! no neighborhood context and can be fused into a single pass.
//!
//! # Generic conversion
//!
//! [`RowConverterOp`] wraps a [`zenpixels_convert::RowConverter`] as a
//! [`PixelOp`], supporting any format pair that zenpixels-convert can handle
//! (P3, BT.2020, PQ, HLG, etc.). The named structs below are convenience
//! aliases for common sRGB conversions.

use crate::PixelFormat;
use crate::format::{self, PixelFormatExt};

/// A per-pixel operation applied to strip data.
///
/// Operations declare their input and output formats. When input != output,
/// the transform source allocates a separate output buffer. When they match,
/// the operation runs in-place on the same buffer.
pub trait PixelOp: Send {
    /// Apply the operation. `input` and `output` may alias if formats match.
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32);

    /// Expected input format.
    fn input_format(&self) -> PixelFormat;

    /// Produced output format.
    fn output_format(&self) -> PixelFormat;
}

// =========================================================================
// Generic RowConverter-based op
// =========================================================================

/// A [`PixelOp`] wrapping [`zenpixels_convert::RowConverter`].
///
/// Handles any format conversion that zenpixels-convert supports, including
/// P3, BT.2020, PQ, HLG, depth changes, alpha mode changes, and gamut
/// mapping — all via pre-computed conversion plans with no per-row allocation.
pub struct RowConverterOp {
    converter: zenpixels_convert::RowConverter,
    from: PixelFormat,
    to: PixelFormat,
}

impl RowConverterOp {
    /// Create a conversion op between any two formats.
    ///
    /// Returns `None` if no conversion path exists.
    pub fn new(from: PixelFormat, to: PixelFormat) -> Option<Self> {
        let converter = zenpixels_convert::RowConverter::new(from, to).ok()?;
        Some(Self {
            converter,
            from,
            to,
        })
    }

    /// Create a conversion op, panicking if no path exists.
    #[track_caller]
    pub fn must(from: PixelFormat, to: PixelFormat) -> Self {
        Self::new(from, to).unwrap_or_else(|| {
            panic!("no conversion path from {from} to {to}");
        })
    }
}

impl PixelOp for RowConverterOp {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        let src_stride = self.from.row_bytes(width);
        let dst_stride = self.to.row_bytes(width);
        for r in 0..height {
            let src_start = r as usize * src_stride;
            let dst_start = r as usize * dst_stride;
            self.converter.convert_row(
                &input[src_start..src_start + src_stride],
                &mut output[dst_start..dst_start + dst_stride],
                width,
            );
        }
    }

    fn input_format(&self) -> PixelFormat {
        self.from
    }
    fn output_format(&self) -> PixelFormat {
        self.to
    }
}

// =========================================================================
// Named convenience ops (backward compatible)
// =========================================================================

/// Fused sRGB u8 → linear f32 + premultiply.
///
/// Input: `RGBA8_SRGB` (4 bytes/px)
/// Output: `RGBAF32_LINEAR_PREMUL` (16 bytes/px)
pub struct SrgbToLinearPremul;

impl PixelOp for SrgbToLinearPremul {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBA8_SRGB, format::RGBAF32_LINEAR_PREMUL)
            .apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBA8_SRGB
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
}

/// Fused unpremultiply + linear f32 → sRGB u8.
///
/// Input: `RGBAF32_LINEAR_PREMUL` (16 bytes/px)
/// Output: `RGBA8_SRGB` (4 bytes/px)
pub struct UnpremulLinearToSrgb;

impl PixelOp for UnpremulLinearToSrgb {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBAF32_LINEAR_PREMUL, format::RGBA8_SRGB)
            .apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBA8_SRGB
    }
}

/// Premultiply alpha in-place on f32 linear data.
///
/// Format: `RGBAF32_LINEAR` → `RGBAF32_LINEAR_PREMUL`
pub struct Premultiply;

impl PixelOp for Premultiply {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBAF32_LINEAR, format::RGBAF32_LINEAR_PREMUL)
            .apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
}

/// Unpremultiply alpha in-place on f32 linear data.
///
/// Format: `RGBAF32_LINEAR_PREMUL` → `RGBAF32_LINEAR`
pub struct Unpremultiply;

impl PixelOp for Unpremultiply {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBAF32_LINEAR_PREMUL, format::RGBAF32_LINEAR)
            .apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
}

/// sRGB u8 → linear f32 (straight alpha, no premultiply).
///
/// Input: `RGBA8_SRGB` (4 bytes/px)
/// Output: `RGBAF32_LINEAR` (16 bytes/px)
pub struct SrgbToLinear;

impl PixelOp for SrgbToLinear {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBA8_SRGB, format::RGBAF32_LINEAR).apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBA8_SRGB
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
}

/// Linear f32 → sRGB u8 (straight alpha, no unpremultiply).
///
/// Input: `RGBAF32_LINEAR` (16 bytes/px)
/// Output: `RGBA8_SRGB` (4 bytes/px)
pub struct LinearToSrgb;

impl PixelOp for LinearToSrgb {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBAF32_LINEAR, format::RGBA8_SRGB).apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBA8_SRGB
    }
}

/// Linearize f32 sRGB channels in-place (straight alpha preserved).
///
/// Input: `RGBAF32_SRGB`
/// Output: `RGBAF32_LINEAR`
pub struct LinearizeF32;

impl PixelOp for LinearizeF32 {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBAF32_SRGB, format::RGBAF32_LINEAR).apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_SRGB
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
}

/// Delinearize f32 linear channels in-place (straight alpha preserved).
///
/// Input: `RGBAF32_LINEAR`
/// Output: `RGBAF32_SRGB`
pub struct DelinearizeF32;

impl PixelOp for DelinearizeF32 {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBAF32_LINEAR, format::RGBAF32_SRGB).apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_SRGB
    }
}

/// Fused linearize + premultiply on f32 sRGB data.
///
/// Input: `RGBAF32_SRGB`
/// Output: `RGBAF32_LINEAR_PREMUL`
pub struct SrgbF32ToLinearPremul;

impl PixelOp for SrgbF32ToLinearPremul {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBAF32_SRGB, format::RGBAF32_LINEAR_PREMUL)
            .apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_SRGB
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
}

/// Fused unpremultiply + delinearize on f32 linear premul data.
///
/// Input: `RGBAF32_LINEAR_PREMUL`
/// Output: `RGBAF32_SRGB`
pub struct UnpremulLinearToSrgbF32;

impl PixelOp for UnpremulLinearToSrgbF32 {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBAF32_LINEAR_PREMUL, format::RGBAF32_SRGB)
            .apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_SRGB
    }
}

/// Normalize u8 to f32 (divide by 255). No gamma conversion.
///
/// Input: `RGBA8_SRGB`
/// Output: `RGBAF32_SRGB`
pub struct NormalizeU8ToF32;

impl PixelOp for NormalizeU8ToF32 {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBA8_SRGB, format::RGBAF32_SRGB).apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBA8_SRGB
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_SRGB
    }
}

/// Quantize f32 to u8 (multiply by 255 + round). No gamma conversion.
///
/// Input: `RGBAF32_SRGB`
/// Output: `RGBA8_SRGB`
pub struct QuantizeF32ToU8;

impl PixelOp for QuantizeF32ToU8 {
    fn apply(&self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        static_op(format::RGBAF32_SRGB, format::RGBA8_SRGB).apply(input, output, width, height);
    }
    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_SRGB
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBA8_SRGB
    }
}

// =========================================================================
// Helper: create a RowConverterOp for named ops at call time
// =========================================================================

fn static_op(from: PixelFormat, to: PixelFormat) -> RowConverterOp {
    RowConverterOp::must(from, to)
}
