//! High-level convenience API for applying filters to `PixelBuffer`/`PixelSlice`.
//!
//! This module handles the full conversion pipeline:
//! 1. Convert input to linear f32 RGB(A) with the correct primaries
//! 2. Scatter to planar Oklab
//! 3. Apply filter stack
//! 4. Gather back to interleaved linear RGB
//! 5. Optionally convert back to original format
//!
//! Requires the `buffer` feature.

use whereat::{At, ResultAtExt, at};
use zenpixels::buffer::PixelBuffer;
use zenpixels::{
    AlphaMode, ChannelLayout, ChannelType, ColorPrimaries, PixelDescriptor, TransferFunction,
};
use zenpixels_convert::RowConverter;

use crate::context::FilterContext;
use crate::pipeline::{self, Pipeline, PipelineError};
use crate::planes::OklabPlanes;
use crate::scatter_gather::{
    gather_from_oklab, gather_oklab_to_srgb_u8, scatter_srgb_u8_to_oklab, scatter_to_oklab,
};

/// Error type for convenience layer operations.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConvenienceError {
    /// Filter pipeline error.
    Pipeline(PipelineError),
    /// Pixel format conversion error.
    Convert(zenpixels_convert::ConvertError),
    /// The input has unsupported primaries (Unknown).
    UnsupportedPrimaries(ColorPrimaries),
    /// The input format is not RGB-based (e.g., grayscale Oklab).
    UnsupportedLayout(ChannelLayout),
}

impl From<zenpixels_convert::ConvertError> for ConvenienceError {
    fn from(e: zenpixels_convert::ConvertError) -> Self {
        Self::Convert(e)
    }
}

impl core::fmt::Display for ConvenienceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pipeline(e) => write!(f, "pipeline: {e}"),
            Self::Convert(e) => write!(f, "convert: {e}"),
            Self::UnsupportedPrimaries(p) => write!(f, "unsupported primaries: {p:?}"),
            Self::UnsupportedLayout(l) => write!(f, "unsupported layout: {l:?}"),
        }
    }
}

impl core::error::Error for ConvenienceError {}

/// Determines the working primaries for the filter pipeline.
///
/// If the input has known primaries, use those. This preserves the gamut
/// without any cross-gamut conversion (BT.2020 stays BT.2020, P3 stays P3).
#[track_caller]
fn working_primaries(desc: PixelDescriptor) -> Result<ColorPrimaries, At<ConvenienceError>> {
    match desc.primaries {
        ColorPrimaries::Bt709 | ColorPrimaries::Bt2020 | ColorPrimaries::DisplayP3 => {
            Ok(desc.primaries)
        }
        other => Err(at!(ConvenienceError::UnsupportedPrimaries(other))),
    }
}

/// Build the intermediate linear f32 RGB(A) descriptor for the working space.
fn linear_f32_descriptor(has_alpha: bool, primaries: ColorPrimaries) -> PixelDescriptor {
    let layout = if has_alpha {
        ChannelLayout::Rgba
    } else {
        ChannelLayout::Rgb
    };
    let alpha = if has_alpha {
        Some(AlphaMode::Straight)
    } else {
        None
    };
    PixelDescriptor::new_full(
        ChannelType::F32,
        layout,
        alpha,
        TransferFunction::Linear,
        primaries,
    )
}

/// Apply a filter pipeline to a `PixelBuffer`, returning a new `PixelBuffer`.
///
/// The input buffer's `PixelDescriptor` provides all the metadata needed:
/// transfer function, primaries, alpha mode, and channel type. The pipeline
/// automatically handles:
/// - Linearization (sRGB/PQ/HLG/BT.709 → linear)
/// - HDR normalization (÷ reference_white)
/// - Scatter to planar Oklab
/// - Filter application
/// - Gather back to linear RGB
/// - × reference_white
/// - Convert back to original format (if `convert_back` is true)
///
/// When `convert_back` is false, the output is linear f32 RGB(A) in the
/// input's gamut. This is useful when the caller wants to do further
/// processing before final encoding.
#[track_caller]
pub fn apply_to_buffer(
    pipeline: &Pipeline,
    input: &PixelBuffer,
    convert_back: bool,
    ctx: &mut FilterContext,
) -> Result<PixelBuffer, At<ConvenienceError>> {
    let desc = input.descriptor();
    let width = input.width();
    let height = input.height();
    let primaries = working_primaries(desc).at()?;

    // Validate layout is RGB-based
    match desc.layout() {
        ChannelLayout::Rgb | ChannelLayout::Rgba | ChannelLayout::Bgra => {}
        other => return Err(at!(ConvenienceError::UnsupportedLayout(other))),
    }

    let has_alpha = desc.has_alpha();
    let channels = if has_alpha { 4u32 } else { 3u32 };

    // Step 1-3: Convert to linear f32, then scatter to planar Oklab
    let linear_desc = linear_f32_descriptor(has_alpha, primaries);
    let reference_white = desc.transfer().reference_white_nits();
    let m1 = zenpixels_convert::oklab::rgb_to_lms_matrix(primaries)
        .ok_or_else(|| at!(ConvenienceError::UnsupportedPrimaries(primaries)))?;
    let m1_inv = zenpixels_convert::oklab::lms_to_rgb_matrix(primaries)
        .ok_or_else(|| at!(ConvenienceError::UnsupportedPrimaries(primaries)))?;

    // Detect if we can use the fused sRGB u8 path (eliminates intermediate
    // linear f32 buffer — ~48MB savings at 2048² RGB).
    let use_fused = desc.channel_type() == ChannelType::U8
        && desc.transfer() == TransferFunction::Srgb
        && matches!(desc.layout(), ChannelLayout::Rgb | ChannelLayout::Rgba);

    let color_ctx = input.color_context().cloned();

    // Fast path: fused sRGB u8 roundtrip with L3-friendly strip processing.
    // Scatter/filter/gather in horizontal strips so planar data stays in L3.
    // Neighborhood filters are handled via overlapping halo rows per strip.
    if use_fused && convert_back {
        return apply_fused_stripped(
            pipeline,
            input,
            ctx,
            desc,
            width,
            height,
            channels,
            has_alpha,
            &m1,
            &m1_inv,
            color_ctx.as_ref(),
        )
        .at();
    }

    // Full-frame paths: neighborhood filters, non-fused formats, or !convert_back.
    let mut planes = if has_alpha {
        OklabPlanes::from_ctx_with_alpha(ctx, width, height)
    } else {
        OklabPlanes::from_ctx(ctx, width, height)
    };

    if use_fused {
        let input_bytes = copy_input_bytes_pooled(input, ctx);
        scatter_srgb_u8_to_oklab(&input_bytes, &mut planes, channels, &m1);
        ctx.return_u8(input_bytes);
    } else {
        let linear_bytes = convert_buffer_bytes_pooled(input, linear_desc, ctx)
            .map_err(|e| at!(e).map_error(|e| ConvenienceError::Convert(e.into_inner())))?;
        let linear_f32: &[f32] = bytemuck::cast_slice(&linear_bytes);
        scatter_to_oklab(linear_f32, &mut planes, channels, &m1, reference_white);
        ctx.return_u8(linear_bytes);
    }

    pipeline.apply_planar(&mut planes, ctx);

    if use_fused && convert_back {
        let total = (width as usize) * (height as usize) * (channels as usize);
        let mut output_bytes = ctx.take_u8(total);
        gather_oklab_to_srgb_u8(&planes, &mut output_bytes, channels, &m1_inv);
        planes.return_to_ctx(ctx);

        let mut output_buf =
            PixelBuffer::from_vec(output_bytes, width, height, desc).map_err(|_| {
                at!(ConvenienceError::Convert(
                    zenpixels_convert::ConvertError::AllocationFailed
                ))
            })?;
        if let Some(cc) = &color_ctx {
            output_buf = output_buf.with_color_context(alloc::sync::Arc::clone(cc));
        }
        Ok(output_buf)
    } else {
        let n = (width as usize) * (height as usize) * (channels as usize);
        let mut output_f32 = ctx.take_f32(n);
        gather_from_oklab(&planes, &mut output_f32, channels, &m1_inv, reference_white);
        planes.return_to_ctx(ctx);

        if convert_back && desc != linear_desc {
            let converter = RowConverter::new(linear_desc, desc)
                .map_err(|e| at!(e).map_error(|e| ConvenienceError::Convert(e.into_inner())))?;
            let dst_bpp = desc.bytes_per_pixel();
            let dst_stride = (width as usize) * dst_bpp;
            let total = dst_stride * height as usize;
            let mut final_bytes = ctx.take_u8(total);
            let src_bpp = linear_desc.bytes_per_pixel();
            let src_stride = (width as usize) * src_bpp;

            {
                let output_bytes: &[u8] = bytemuck::cast_slice(&output_f32);
                for y in 0..height {
                    let src_start = y as usize * src_stride;
                    let src_end = src_start + src_stride;
                    let dst_start = y as usize * dst_stride;
                    let dst_end = dst_start + dst_stride;
                    converter.convert_row(
                        &output_bytes[src_start..src_end],
                        &mut final_bytes[dst_start..dst_end],
                        width,
                    );
                }
            }

            ctx.return_f32(output_f32);
            let mut final_buf =
                PixelBuffer::from_vec(final_bytes, width, height, desc).map_err(|_| {
                    at!(ConvenienceError::Convert(
                        zenpixels_convert::ConvertError::AllocationFailed
                    ))
                })?;
            if let Some(cc) = &color_ctx {
                final_buf = final_buf.with_color_context(alloc::sync::Arc::clone(cc));
            }
            Ok(final_buf)
        } else {
            let output_bytes: &[u8] = bytemuck::cast_slice(&output_f32);
            let mut output_buf =
                PixelBuffer::from_vec(output_bytes.to_vec(), width, height, linear_desc).map_err(
                    |_| {
                        at!(ConvenienceError::Convert(
                            zenpixels_convert::ConvertError::AllocationFailed
                        ))
                    },
                )?;
            ctx.return_f32(output_f32);
            if let Some(cc) = &color_ctx {
                output_buf = output_buf.with_color_context(alloc::sync::Arc::clone(cc));
            }
            Ok(output_buf)
        }
    }
}

/// Fused sRGB u8 roundtrip with L3-friendly strip processing.
///
/// Processes the image in horizontal strips so that planar Oklab data
/// (L + a + b + optional alpha) stays in L3 cache during scatter → filter → gather.
/// Neighborhood filters are supported via overlapping halo rows per strip.
#[allow(clippy::too_many_arguments)]
#[track_caller]
fn apply_fused_stripped(
    pipeline: &Pipeline,
    input: &PixelBuffer,
    ctx: &mut FilterContext,
    desc: PixelDescriptor,
    width: u32,
    height: u32,
    channels: u32,
    has_alpha: bool,
    m1: &zenpixels_convert::gamut::GamutMatrix,
    m1_inv: &zenpixels_convert::gamut::GamutMatrix,
    color_ctx: Option<&alloc::sync::Arc<zenpixels::ColorContext>>,
) -> Result<PixelBuffer, At<ConvenienceError>> {
    let w = width as usize;
    let ch = channels as usize;
    let halo = pipeline.total_halo(width, height) as usize;
    let strip_h = pipeline::strip_height(width, has_alpha, halo);

    // Allocate extended-strip-sized input buffer and full-frame output
    let ext_strip_cap = (strip_h + 2 * halo) * w * ch;
    let mut strip_input = ctx.take_u8(ext_strip_cap);
    let total_out = w * (height as usize) * ch;
    let mut output_bytes = ctx.take_u8(total_out);

    let src_slice = input.as_slice();

    for y_start in (0..height as usize).step_by(strip_h) {
        let y_end = (y_start + strip_h).min(height as usize);
        let core_rows = y_end - y_start;

        // Extended region: core ± halo, clamped to image bounds
        let ext_y_start = y_start.saturating_sub(halo);
        let ext_y_end = (y_end + halo).min(height as usize);
        let ext_rows = ext_y_end - ext_y_start;
        let core_offset = y_start - ext_y_start;

        let ext_len = ext_rows * w * ch;

        // Copy extended rows from input (handles stride)
        for dy in 0..ext_rows {
            let row = src_slice.row((ext_y_start + dy) as u32);
            let row_start = dy * w * ch;
            strip_input[row_start..row_start + row.len()].copy_from_slice(row);
        }

        let mut planes = if has_alpha {
            OklabPlanes::from_ctx_with_alpha(ctx, width, ext_rows as u32)
        } else {
            OklabPlanes::from_ctx(ctx, width, ext_rows as u32)
        };

        scatter_srgb_u8_to_oklab(&strip_input[..ext_len], &mut planes, channels, m1);
        pipeline.apply_planar(&mut planes, ctx);

        // Gather only the core rows from the filtered extended strip
        let out_offset = y_start * w * ch;
        let core_len = core_rows * w * ch;
        let plane_start = core_offset * w;
        let plane_end = plane_start + core_rows * w;

        crate::simd::gather_oklab_to_srgb_u8(
            &planes.l[plane_start..plane_end],
            &planes.a[plane_start..plane_end],
            &planes.b[plane_start..plane_end],
            &mut output_bytes[out_offset..out_offset + core_len],
            channels,
            m1_inv,
        );

        // Alpha: copy core rows
        if ch == 4 {
            let n = core_rows * w;
            for i in 0..n {
                let a = planes
                    .alpha
                    .as_ref()
                    .map_or(1.0, |alpha| alpha[plane_start + i]);
                output_bytes[out_offset + i * ch + 3] = (a * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }
        }

        planes.return_to_ctx(ctx);
    }

    ctx.return_u8(strip_input);

    let mut buf = PixelBuffer::from_vec(output_bytes, width, height, desc).map_err(|_| {
        at!(ConvenienceError::Convert(
            zenpixels_convert::ConvertError::AllocationFailed
        ))
    })?;
    if let Some(cc) = color_ctx {
        buf = buf.with_color_context(alloc::sync::Arc::clone(cc));
    }
    Ok(buf)
}

/// Convert a PixelBuffer to the target descriptor using a pooled output buffer.
fn convert_buffer_bytes_pooled(
    input: &PixelBuffer,
    target: PixelDescriptor,
    ctx: &mut FilterContext,
) -> Result<alloc::vec::Vec<u8>, At<zenpixels_convert::ConvertError>> {
    let desc = input.descriptor();
    let width = input.width();
    let height = input.height();
    let dst_bpp = target.bytes_per_pixel();
    let dst_stride = (width as usize) * dst_bpp;
    let total = dst_stride * height as usize;
    let mut output = ctx.take_u8(total);

    if desc == target {
        // Direct copy from input into pooled buffer
        let src_slice = input.as_slice();
        for y in 0..height {
            let src_row = src_slice.row(y);
            let dst_start = y as usize * dst_stride;
            output[dst_start..dst_start + src_row.len()].copy_from_slice(src_row);
        }
    } else {
        let converter = RowConverter::new(desc, target).at()?;
        let src_slice = input.as_slice();
        for y in 0..height {
            let src_row = src_slice.row(y);
            let dst_start = y as usize * dst_stride;
            let dst_end = dst_start + dst_stride;
            converter.convert_row(src_row, &mut output[dst_start..dst_end], width);
        }
    }
    Ok(output)
}

/// Copy raw bytes from a PixelBuffer into a pooled contiguous buffer.
///
/// Used by the fused sRGB u8 path to avoid the RowConverter entirely.
fn copy_input_bytes_pooled(input: &PixelBuffer, ctx: &mut FilterContext) -> alloc::vec::Vec<u8> {
    let width = input.width();
    let height = input.height();
    let bpp = input.descriptor().bytes_per_pixel();
    let stride = (width as usize) * bpp;
    let total = stride * height as usize;
    let mut buf = ctx.take_u8(total);
    let src_slice = input.as_slice();
    for y in 0..height {
        let src_row = src_slice.row(y);
        let dst_start = y as usize * stride;
        buf[dst_start..dst_start + src_row.len()].copy_from_slice(src_row);
    }
    buf
}

/// Extension trait for [`Pipeline`] that adds buffer convenience methods.
pub trait PipelineBufferExt {
    /// Apply this pipeline to a `PixelBuffer`, converting back to original format.
    fn apply_buffer(
        &self,
        input: &PixelBuffer,
        ctx: &mut FilterContext,
    ) -> Result<PixelBuffer, At<ConvenienceError>>;

    /// Apply this pipeline to a `PixelBuffer`, returning linear f32 RGB(A).
    fn apply_buffer_linear(
        &self,
        input: &PixelBuffer,
        ctx: &mut FilterContext,
    ) -> Result<PixelBuffer, At<ConvenienceError>>;
}

impl PipelineBufferExt for Pipeline {
    #[track_caller]
    fn apply_buffer(
        &self,
        input: &PixelBuffer,
        ctx: &mut FilterContext,
    ) -> Result<PixelBuffer, At<ConvenienceError>> {
        apply_to_buffer(self, input, true, ctx).at()
    }

    #[track_caller]
    fn apply_buffer_linear(
        &self,
        input: &PixelBuffer,
        ctx: &mut FilterContext,
    ) -> Result<PixelBuffer, At<ConvenienceError>> {
        apply_to_buffer(self, input, false, ctx).at()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PipelineConfig;
    use crate::filters;
    use crate::gamut_map::GamutMapping;

    fn make_srgb_u8_buffer(width: u32, height: u32) -> PixelBuffer {
        let n = (width as usize) * (height as usize);
        let mut data = alloc::vec::Vec::with_capacity(n * 3);
        for i in 0..n {
            let t = i as f32 / n as f32;
            data.push((t * 200.0 + 30.0) as u8);
            data.push(((1.0 - t) * 180.0 + 40.0) as u8);
            data.push((t * 100.0 + 80.0) as u8);
        }
        PixelBuffer::from_vec(data, width, height, PixelDescriptor::RGB8_SRGB).unwrap()
    }

    fn make_srgba_u8_buffer(width: u32, height: u32) -> PixelBuffer {
        let n = (width as usize) * (height as usize);
        let mut data = alloc::vec::Vec::with_capacity(n * 4);
        for i in 0..n {
            let t = i as f32 / n as f32;
            data.push((t * 200.0 + 30.0) as u8);
            data.push(((1.0 - t) * 180.0 + 40.0) as u8);
            data.push((t * 100.0 + 80.0) as u8);
            data.push(200u8); // alpha
        }
        PixelBuffer::from_vec(data, width, height, PixelDescriptor::RGBA8_SRGB).unwrap()
    }

    fn make_p3_f32_buffer(width: u32, height: u32) -> PixelBuffer {
        let n = (width as usize) * (height as usize);
        let mut data = alloc::vec::Vec::with_capacity(n * 3 * 4);
        for i in 0..n {
            let t = i as f32 / n as f32;
            let r = (t * 0.6 + 0.2).to_le_bytes();
            let g = ((1.0 - t) * 0.5 + 0.25).to_le_bytes();
            let b = (t * 0.3 + 0.3).to_le_bytes();
            data.extend_from_slice(&r);
            data.extend_from_slice(&g);
            data.extend_from_slice(&b);
        }
        let desc = PixelDescriptor::RGBF32_LINEAR.with_primaries(ColorPrimaries::DisplayP3);
        PixelBuffer::from_vec(data, width, height, desc).unwrap()
    }

    #[test]
    fn srgb_u8_roundtrip_empty_pipeline() {
        let input = make_srgb_u8_buffer(32, 32);
        let pipeline = Pipeline::new(PipelineConfig {
            primaries: ColorPrimaries::Bt709,
            reference_white: 1.0,
            gamut_mapping: GamutMapping::Clip,
        })
        .unwrap();

        let output = apply_to_buffer(&pipeline, &input, true, &mut FilterContext::new()).unwrap();
        assert_eq!(output.descriptor(), input.descriptor());
        assert_eq!(output.width(), input.width());
        assert_eq!(output.height(), input.height());

        // Check roundtrip accuracy
        let src = input.copy_to_contiguous_bytes();
        let dst = output.copy_to_contiguous_bytes();
        let mut max_err = 0u8;
        for (a, b) in src.iter().zip(dst.iter()) {
            let err = (*a as i16 - *b as i16).unsigned_abs() as u8;
            max_err = max_err.max(err);
        }
        assert!(
            max_err <= 2,
            "sRGB u8 roundtrip max error: {max_err} (should be ≤2)"
        );
    }

    #[test]
    fn srgba_u8_roundtrip() {
        let input = make_srgba_u8_buffer(16, 16);
        let pipeline = Pipeline::new(PipelineConfig::default()).unwrap();

        let output = apply_to_buffer(&pipeline, &input, true, &mut FilterContext::new()).unwrap();
        assert_eq!(output.descriptor(), input.descriptor());

        let src = input.copy_to_contiguous_bytes();
        let dst = output.copy_to_contiguous_bytes();
        let mut max_err = 0u8;
        for (a, b) in src.iter().zip(dst.iter()) {
            let err = (*a as i16 - *b as i16).unsigned_abs() as u8;
            max_err = max_err.max(err);
        }
        assert!(max_err <= 2, "sRGBA u8 roundtrip max error: {max_err}");
    }

    #[test]
    fn p3_linear_f32_roundtrip() {
        let input = make_p3_f32_buffer(16, 16);
        let pipeline = Pipeline::new(PipelineConfig {
            primaries: ColorPrimaries::DisplayP3,
            reference_white: 1.0,
            gamut_mapping: GamutMapping::Clip,
        })
        .unwrap();

        let output = apply_to_buffer(&pipeline, &input, true, &mut FilterContext::new()).unwrap();
        assert_eq!(output.descriptor(), input.descriptor());

        let src_bytes = input.copy_to_contiguous_bytes();
        let dst_bytes = output.copy_to_contiguous_bytes();
        let src: &[f32] = bytemuck::cast_slice(&src_bytes);
        let dst: &[f32] = bytemuck::cast_slice(&dst_bytes);
        let mut max_err = 0.0f32;
        for (a, b) in src.iter().zip(dst.iter()) {
            max_err = max_err.max((a - b).abs());
        }
        assert!(max_err < 1e-3, "P3 f32 roundtrip max error: {max_err}");
    }

    #[test]
    fn apply_exposure_to_srgb_u8() {
        let input = make_srgb_u8_buffer(32, 32);
        let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
        pipeline.push(Box::new(filters::Exposure { stops: 0.5 }));

        let output = apply_to_buffer(&pipeline, &input, true, &mut FilterContext::new()).unwrap();
        assert_eq!(output.descriptor(), PixelDescriptor::RGB8_SRGB);

        // Output should be brighter on average
        let src = input.copy_to_contiguous_bytes();
        let dst = output.copy_to_contiguous_bytes();
        let src_avg: f32 = src.iter().map(|&v| v as f32).sum::<f32>() / src.len() as f32;
        let dst_avg: f32 = dst.iter().map(|&v| v as f32).sum::<f32>() / dst.len() as f32;
        assert!(
            dst_avg > src_avg,
            "exposure +0.5 should brighten: src_avg={src_avg}, dst_avg={dst_avg}"
        );
    }

    #[test]
    fn apply_linear_returns_f32() {
        let input = make_srgb_u8_buffer(8, 8);
        let pipeline = Pipeline::new(PipelineConfig::default()).unwrap();

        let output = apply_to_buffer(&pipeline, &input, false, &mut FilterContext::new()).unwrap();
        assert_eq!(output.descriptor().channel_type(), ChannelType::F32);
        assert_eq!(output.descriptor().transfer(), TransferFunction::Linear);
    }

    #[test]
    fn pipeline_ext_trait() {
        let input = make_srgb_u8_buffer(8, 8);
        let pipeline = Pipeline::new(PipelineConfig::default()).unwrap();

        let output = pipeline
            .apply_buffer(&input, &mut FilterContext::new())
            .unwrap();
        assert_eq!(output.descriptor(), PixelDescriptor::RGB8_SRGB);

        let linear = pipeline
            .apply_buffer_linear(&input, &mut FilterContext::new())
            .unwrap();
        assert_eq!(linear.descriptor().channel_type(), ChannelType::F32);
    }

    #[test]
    fn unknown_primaries_rejected() {
        let data = vec![128u8; 8 * 8 * 3];
        let desc = PixelDescriptor::RGB8_SRGB.with_primaries(ColorPrimaries::Unknown);
        let input = PixelBuffer::from_vec(data, 8, 8, desc).unwrap();
        let pipeline = Pipeline::new(PipelineConfig::default()).unwrap();

        let result = apply_to_buffer(&pipeline, &input, true, &mut FilterContext::new());
        assert!(result.is_err());
    }

    #[test]
    fn grayscale_layout_rejected() {
        let data = vec![128u8; 8 * 8];
        let input = PixelBuffer::from_vec(data, 8, 8, PixelDescriptor::GRAY8_SRGB).unwrap();
        let pipeline = Pipeline::new(PipelineConfig::default()).unwrap();

        let result = apply_to_buffer(&pipeline, &input, true, &mut FilterContext::new());
        assert!(result.is_err());
    }

    #[test]
    fn bt2020_pq_roundtrip() {
        // Simulate HDR PQ content in BT.2020
        let n = 16usize * 16;
        let mut data = alloc::vec::Vec::with_capacity(n * 3 * 4);
        for i in 0..n {
            let t = i as f32 / n as f32;
            // PQ f32 values are typically 0.0-1.0
            let r = (t * 0.5 + 0.2).to_le_bytes();
            let g = ((1.0 - t) * 0.4 + 0.1).to_le_bytes();
            let b = (t * 0.3 + 0.1).to_le_bytes();
            data.extend_from_slice(&r);
            data.extend_from_slice(&g);
            data.extend_from_slice(&b);
        }
        let desc = PixelDescriptor::new_full(
            ChannelType::F32,
            ChannelLayout::Rgb,
            None,
            TransferFunction::Pq,
            ColorPrimaries::Bt2020,
        );
        let input = PixelBuffer::from_vec(data, 16, 16, desc).unwrap();

        let pipeline = Pipeline::new(PipelineConfig {
            primaries: ColorPrimaries::Bt2020,
            reference_white: 203.0,
            gamut_mapping: GamutMapping::Clip,
        })
        .unwrap();

        let output = apply_to_buffer(&pipeline, &input, true, &mut FilterContext::new()).unwrap();
        assert_eq!(output.descriptor(), desc);

        let src_bytes = input.copy_to_contiguous_bytes();
        let dst_bytes = output.copy_to_contiguous_bytes();
        let src: &[f32] = bytemuck::cast_slice(&src_bytes);
        let dst: &[f32] = bytemuck::cast_slice(&dst_bytes);
        let mut max_err = 0.0f32;
        for (a, b) in src.iter().zip(dst.iter()) {
            max_err = max_err.max((a - b).abs());
        }
        assert!(max_err < 0.01, "BT.2020 PQ roundtrip max error: {max_err}");
    }

    #[test]
    fn multiple_filters_on_srgb() {
        let input = make_srgb_u8_buffer(32, 32);
        let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
        pipeline.push(Box::new(filters::Exposure { stops: 0.3 }));
        pipeline.push(Box::new(filters::Contrast { amount: 0.2 }));
        pipeline.push(Box::new(filters::Saturation { factor: 1.1 }));

        let output = pipeline
            .apply_buffer(&input, &mut FilterContext::new())
            .unwrap();
        assert_eq!(output.descriptor(), PixelDescriptor::RGB8_SRGB);
        assert_eq!(output.width(), 32);
        assert_eq!(output.height(), 32);
    }
}
