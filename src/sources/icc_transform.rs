//! Streaming ICC profile transform source.
//!
//! Applies an ICC-to-ICC color conversion row-by-row using moxcms.
//! Requires the `cms` feature.
//!
//! # Usage
//!
//! ```ignore
//! use zenpipe::sources::IccTransformSource;
//! use zenpixels_convert::cms_moxcms::MoxCms;
//!
//! let source = IccTransformSource::new(upstream, &src_icc, &dst_icc)?;
//! ```

use alloc::sync::Arc;
use alloc::vec::Vec;

use moxcms::{ColorProfile, Layout, TransformExecutor, TransformOptions};

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::Strip;

/// Map a zenpixels PixelFormat to a moxcms Layout.
fn pixel_format_to_layout(format: zenpixels_convert::PixelFormat) -> Layout {
    use zenpixels_convert::PixelFormat as PF;
    match format {
        PF::Rgb8 | PF::Rgb16 | PF::RgbF32 => Layout::Rgb,
        PF::Rgba8 | PF::Rgba16 | PF::RgbaF32 => Layout::Rgba,
        PF::Gray8 | PF::Gray16 | PF::GrayF32 => Layout::Gray,
        PF::GrayA8 | PF::GrayA16 | PF::GrayAF32 => Layout::GrayAlpha,
        _ => Layout::Rgba, // fallback for Bgra, Oklab, etc.
    }
}

/// Internal wrapper around moxcms transform executors at different bit depths.
///
/// All variants contain `Arc<dyn TransformExecutor<T> + Send + Sync>`,
/// which makes this enum `Send + Sync`.
enum MoxTransform {
    U8(Arc<dyn TransformExecutor<u8> + Send + Sync>),
    U16(Arc<dyn TransformExecutor<u16> + Send + Sync>),
    F32(Arc<dyn TransformExecutor<f32> + Send + Sync>),
}

impl MoxTransform {
    fn transform_row(&self, src: &[u8], dst: &mut [u8]) {
        match self {
            Self::U8(xform) => {
                xform
                    .transform(src, dst)
                    .expect("moxcms u8 transform: buffer size mismatch");
            }
            Self::U16(xform) => {
                let src_u16: &[u16] = bytemuck::cast_slice(src);
                let dst_u16: &mut [u16] = bytemuck::cast_slice_mut(dst);
                xform
                    .transform(src_u16, dst_u16)
                    .expect("moxcms u16 transform: buffer size mismatch");
            }
            Self::F32(xform) => {
                let src_f32: &[f32] = bytemuck::cast_slice(src);
                let dst_f32: &mut [f32] = bytemuck::cast_slice_mut(dst);
                xform
                    .transform(src_f32, dst_f32)
                    .expect("moxcms f32 transform: buffer size mismatch");
            }
        }
    }
}

/// Streaming ICC profile transform.
///
/// Applies an ICC-to-ICC color conversion row-by-row using moxcms.
/// Pulls strips from upstream, transforms each row, and yields the result.
///
/// The transform is created at construction time from the source and
/// destination ICC profile bytes. No per-strip overhead beyond the
/// row-level color conversion.
///
/// # Format constraints
///
/// The upstream pixel format must be RGB/RGBA/Gray/GrayAlpha at u8, u16,
/// or f32. The output pixel format is the same as the input — ICC transforms
/// change color values, not pixel layout.
pub struct IccTransformSource {
    upstream: Box<dyn Source>,
    transform: MoxTransform,
    /// ICC profile bytes for the output color space (for downstream consumers).
    dst_icc: Arc<[u8]>,
    /// Output buffer for transformed rows.
    buf: Vec<u8>,
    /// Pixel format (same as upstream — ICC doesn't change layout).
    format: PixelFormat,
}

impl IccTransformSource {
    /// Create a streaming ICC transform.
    ///
    /// Parses both ICC profiles and builds a format-aware transform. The
    /// transform runs at the native bit depth (u8, u16, or f32) of the
    /// upstream pixel format.
    ///
    /// # Errors
    ///
    /// Returns `PipeError::Op` if the ICC profiles can't be parsed or moxcms
    /// can't create a transform between them.
    pub fn new(
        upstream: Box<dyn Source>,
        src_icc: &[u8],
        dst_icc: &[u8],
    ) -> Result<Self, PipeError> {
        let format = upstream.format();
        let pixel_format = format.pixel_format();

        let src_profile = ColorProfile::new_from_slice(src_icc)
            .map_err(|e| PipeError::Op(alloc::format!("failed to parse source ICC: {e}")))?;
        let dst_profile = ColorProfile::new_from_slice(dst_icc)
            .map_err(|e| PipeError::Op(alloc::format!("failed to parse dest ICC: {e}")))?;

        let layout = pixel_format_to_layout(pixel_format);
        let opts = TransformOptions::default();

        let depth = format.channel_type();
        let transform = match depth {
            zenpixels_convert::ChannelType::U8 => {
                let xform = src_profile
                    .create_transform_8bit(layout, &dst_profile, layout, opts)
                    .map_err(|e| PipeError::Op(alloc::format!("ICC u8 transform: {e}")))?;
                MoxTransform::U8(xform)
            }
            zenpixels_convert::ChannelType::U16 | zenpixels_convert::ChannelType::F16 => {
                let xform = src_profile
                    .create_transform_16bit(layout, &dst_profile, layout, opts)
                    .map_err(|e| PipeError::Op(alloc::format!("ICC u16 transform: {e}")))?;
                MoxTransform::U16(xform)
            }
            _ => {
                let xform = src_profile
                    .create_transform_f32(layout, &dst_profile, layout, opts)
                    .map_err(|e| PipeError::Op(alloc::format!("ICC f32 transform: {e}")))?;
                MoxTransform::F32(xform)
            }
        };

        Ok(Self {
            upstream,
            transform,
            dst_icc: Arc::from(dst_icc),
            buf: Vec::new(),
            format,
        })
    }

    /// The destination ICC profile bytes.
    pub fn dst_icc(&self) -> &Arc<[u8]> {
        &self.dst_icc
    }
}

impl Source for IccTransformSource {
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            return Ok(None);
        };

        let width = strip.width();
        let height = strip.rows();
        let stride = self.format.aligned_stride(width);
        let total_bytes = stride * height as usize;

        self.buf.resize(total_bytes, 0);

        for r in 0..height {
            let src_start = r as usize * strip.stride();
            let dst_start = r as usize * stride;
            self.transform.transform_row(
                &strip.as_strided_bytes()[src_start..src_start + stride],
                &mut self.buf[dst_start..dst_start + stride],
            );
        }

        Ok(Some(Strip::new(
            &self.buf,
            width,
            height,
            stride,
            self.format,
        )?))
    }

    fn width(&self) -> u32 {
        self.upstream.width()
    }

    fn height(&self) -> u32 {
        self.upstream.height()
    }

    fn format(&self) -> PixelFormat {
        self.format
    }
}
