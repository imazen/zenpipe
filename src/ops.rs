//! Pixel operations that can be fused into a [`TransformSource`](crate::sources::TransformSource).
//!
//! Per-pixel ops are stateless and process strips independently — they need
//! no neighborhood context and can be fused into a single pass.

use crate::PixelFormat;

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

/// Fused sRGB u8 → linear f32 + premultiply.
///
/// Input: [`Rgba8`](PixelFormat::Rgba8) (4 bytes/px)
/// Output: [`Rgbaf32LinearPremul`](PixelFormat::Rgbaf32LinearPremul) (16 bytes/px)
pub struct SrgbToLinearPremul;

impl PixelOp for SrgbToLinearPremul {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(output);
        linear_srgb::default::srgb_u8_to_linear_premultiply_rgba_slice(input, out_f32);
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgba8
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32LinearPremul
    }
}

/// Fused unpremultiply + linear f32 → sRGB u8.
///
/// Input: [`Rgbaf32LinearPremul`](PixelFormat::Rgbaf32LinearPremul) (16 bytes/px)
/// Output: [`Rgba8`](PixelFormat::Rgba8) (4 bytes/px)
pub struct UnpremulLinearToSrgb;

impl PixelOp for UnpremulLinearToSrgb {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        let in_f32: &[f32] = bytemuck::cast_slice(input);
        linear_srgb::default::unpremultiply_linear_to_srgb_u8_rgba_slice(in_f32, output);
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32LinearPremul
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgba8
    }
}

/// Premultiply alpha in-place on f32 linear data.
///
/// Format: [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear) → [`Rgbaf32LinearPremul`](PixelFormat::Rgbaf32LinearPremul)
pub struct Premultiply;

impl PixelOp for Premultiply {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        output.copy_from_slice(input);
        garb::bytes::premultiply_alpha_f32(output).expect("buffer is pixel-aligned");
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Linear
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32LinearPremul
    }
}

/// Unpremultiply alpha in-place on f32 linear data.
///
/// Format: [`Rgbaf32LinearPremul`](PixelFormat::Rgbaf32LinearPremul) → [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear)
pub struct Unpremultiply;

impl PixelOp for Unpremultiply {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        output.copy_from_slice(input);
        garb::bytes::unpremultiply_alpha_f32(output).expect("buffer is pixel-aligned");
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32LinearPremul
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Linear
    }
}

/// sRGB u8 → linear f32 (straight alpha, no premultiply).
///
/// Input: [`Rgba8`](PixelFormat::Rgba8) (4 bytes/px)
/// Output: [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear) (16 bytes/px)
pub struct SrgbToLinear;

impl PixelOp for SrgbToLinear {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(output);
        linear_srgb::default::srgb_u8_to_linear_rgba_slice(input, out_f32);
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgba8
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Linear
    }
}

/// Linear f32 → sRGB u8 (straight alpha, no unpremultiply).
///
/// Input: [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear) (16 bytes/px)
/// Output: [`Rgba8`](PixelFormat::Rgba8) (4 bytes/px)
pub struct LinearToSrgb;

impl PixelOp for LinearToSrgb {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        let in_f32: &[f32] = bytemuck::cast_slice(input);
        linear_srgb::default::linear_to_srgb_u8_rgba_slice(in_f32, output);
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Linear
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgba8
    }
}

/// Linearize f32 sRGB channels in-place (straight alpha preserved).
///
/// Input: [`Rgbaf32Srgb`](PixelFormat::Rgbaf32Srgb)
/// Output: [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear)
pub struct LinearizeF32;

impl PixelOp for LinearizeF32 {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        output.copy_from_slice(input);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(output);
        linear_srgb::default::srgb_to_linear_rgba_slice(out_f32);
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Srgb
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Linear
    }
}

/// Delinearize f32 linear channels in-place (straight alpha preserved).
///
/// Input: [`Rgbaf32Linear`](PixelFormat::Rgbaf32Linear)
/// Output: [`Rgbaf32Srgb`](PixelFormat::Rgbaf32Srgb)
pub struct DelinearizeF32;

impl PixelOp for DelinearizeF32 {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        output.copy_from_slice(input);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(output);
        linear_srgb::default::linear_to_srgb_rgba_slice(out_f32);
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Linear
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Srgb
    }
}

/// Fused linearize + premultiply on f32 sRGB data.
///
/// Input: [`Rgbaf32Srgb`](PixelFormat::Rgbaf32Srgb)
/// Output: [`Rgbaf32LinearPremul`](PixelFormat::Rgbaf32LinearPremul)
pub struct SrgbF32ToLinearPremul;

impl PixelOp for SrgbF32ToLinearPremul {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        output.copy_from_slice(input);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(output);
        linear_srgb::default::srgb_to_linear_premultiply_rgba_slice(out_f32);
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Srgb
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32LinearPremul
    }
}

/// Fused unpremultiply + delinearize on f32 linear premul data.
///
/// Input: [`Rgbaf32LinearPremul`](PixelFormat::Rgbaf32LinearPremul)
/// Output: [`Rgbaf32Srgb`](PixelFormat::Rgbaf32Srgb)
pub struct UnpremulLinearToSrgbF32;

impl PixelOp for UnpremulLinearToSrgbF32 {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        output.copy_from_slice(input);
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(output);
        linear_srgb::default::unpremultiply_linear_to_srgb_rgba_slice(out_f32);
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32LinearPremul
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Srgb
    }
}

/// Normalize u8 to f32 (divide by 255). No gamma conversion.
///
/// Input: [`Rgba8`](PixelFormat::Rgba8)
/// Output: [`Rgbaf32Srgb`](PixelFormat::Rgbaf32Srgb)
pub struct NormalizeU8ToF32;

impl PixelOp for NormalizeU8ToF32 {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        let out_f32: &mut [f32] = bytemuck::cast_slice_mut(output);
        for (o, &i) in out_f32.iter_mut().zip(input.iter()) {
            *o = i as f32 / 255.0;
        }
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgba8
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Srgb
    }
}

/// Quantize f32 to u8 (multiply by 255 + round). No gamma conversion.
///
/// Input: [`Rgbaf32Srgb`](PixelFormat::Rgbaf32Srgb)
/// Output: [`Rgba8`](PixelFormat::Rgba8)
pub struct QuantizeF32ToU8;

impl PixelOp for QuantizeF32ToU8 {
    fn apply(&self, input: &[u8], output: &mut [u8], _width: u32, _height: u32) {
        let in_f32: &[f32] = bytemuck::cast_slice(input);
        for (o, &i) in output.iter_mut().zip(in_f32.iter()) {
            *o = (i * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }

    fn input_format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32Srgb
    }
    fn output_format(&self) -> PixelFormat {
        PixelFormat::Rgba8
    }
}
