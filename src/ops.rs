//! Pixel operations that can be fused into a [`TransformSource`](crate::sources::TransformSource).
//!
//! Per-pixel ops are stateless and process strips independently — they need
//! no neighborhood context and can be fused into a single pass.
//!
//! # Generic conversion
//!
//! [`RowConverterOp`] wraps a [`zenpixels_convert::RowConverter`] as a
//! [`PixelOp`], supporting any format pair that zenpixels-convert can handle
//! (P3, BT.2020, PQ, HLG, etc.). Use [`RowConverterOp::must`] for common
//! conversions like `RGBA8_SRGB → RGBAF32_LINEAR`.

use crate::PixelFormat;
use crate::format::{self};

/// A per-pixel operation applied to strip data.
///
/// Operations declare their input and output formats. When input != output,
/// the transform source allocates a separate output buffer. When they match,
/// the operation runs in-place on the same buffer.
pub trait PixelOp: Send {
    /// Apply the operation. `input` and `output` may alias if formats match.
    fn apply(&mut self, input: &[u8], output: &mut [u8], width: u32, height: u32);

    /// Expected input format.
    fn input_format(&self) -> PixelFormat;

    /// Produced output format.
    fn output_format(&self) -> PixelFormat;

    /// If this op is a `RowConverterOp`, return a reference to its inner
    /// `RowConverter` for composition with adjacent converter ops.
    fn as_row_converter(&self) -> Option<&zenpixels_convert::RowConverter> {
        None
    }
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

    /// Create a conversion op with explicit policy options.
    ///
    /// Validates alpha/depth/luma policies before creating the plan. Returns
    /// `None` if forbidden by options or no path exists.
    pub fn new_explicit(
        from: PixelFormat,
        to: PixelFormat,
        options: &zenpixels_convert::policy::ConvertOptions,
    ) -> Option<Self> {
        let converter = zenpixels_convert::RowConverter::new_explicit(from, to, options).ok()?;
        Some(Self {
            converter,
            from,
            to,
        })
    }

    /// Create from a pre-built `RowConverter` with known format endpoints.
    pub fn from_converter(
        converter: zenpixels_convert::RowConverter,
        from: PixelFormat,
        to: PixelFormat,
    ) -> Self {
        Self {
            converter,
            from,
            to,
        }
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
    fn apply(&mut self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        let src_stride = self.from.aligned_stride(width);
        let dst_stride = self.to.aligned_stride(width);
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
    fn as_row_converter(&self) -> Option<&zenpixels_convert::RowConverter> {
        Some(&self.converter)
    }
}

// =========================================================================
// Alpha operations
// =========================================================================

/// Scale all channels (including alpha) by a factor in premultiplied linear space.
///
/// Used for opacity control on overlay/watermark sources.
///
/// Format: `RGBAF32_LINEAR_PREMUL` (in-place).
pub struct ScaleAlphaOp {
    opacity: f32,
}

impl ScaleAlphaOp {
    pub fn new(opacity: f32) -> Self {
        Self {
            opacity: opacity.clamp(0.0, 1.0),
        }
    }
}

impl PixelOp for ScaleAlphaOp {
    fn apply(&mut self, input: &[u8], output: &mut [u8], width: u32, height: u32) {
        let in_f32: &[f32] = bytemuck::cast_slice(input);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(output);
        let len = width as usize * height as usize * 4;
        for i in 0..len {
            out_f32[i] = in_f32[i] * self.opacity;
        }
    }

    fn input_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
    fn output_format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
}
