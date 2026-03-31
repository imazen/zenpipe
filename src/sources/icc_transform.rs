//! Streaming ICC profile transform source.
//!
//! Applies an ICC-to-ICC color conversion row-by-row using any
//! [`ColorManagement`] backend (default: moxcms via `std` feature).
//!
//! # Usage
//!
//! ```ignore
//! use zenpipe::sources::IccTransformSource;
//! use zenpixels_convert::cms_moxcms::MoxCms;
//! use zenpixels_convert::cms::ColorManagement;
//!
//! let source = IccTransformSource::new(upstream, &src_icc, &dst_icc, &MoxCms)?;
//! ```

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use zenpixels_convert::cms::{ColorManagement, RowTransform};

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::Strip;

/// Streaming ICC profile transform.
///
/// Applies an ICC-to-ICC color conversion row-by-row using a pre-built
/// [`RowTransform`]. Pulls strips from upstream, transforms each row,
/// and yields the result.
///
/// The transform is created at construction time — no per-strip overhead
/// beyond the row-level conversion. Works with any [`ColorManagement`]
/// backend (moxcms, lcms2, etc.).
///
/// # Format constraints
///
/// The upstream pixel format must be compatible with the ICC transform.
/// The output pixel format is the same as the input — ICC transforms
/// change color values, not pixel layout.
pub struct IccTransformSource {
    upstream: Box<dyn Source>,
    transform: Box<dyn RowTransform>,
    /// ICC profile bytes for the output color space (for downstream consumers).
    dst_icc: Arc<[u8]>,
    /// Output buffer for transformed rows.
    buf: Vec<u8>,
    /// Pixel format (same as upstream — ICC doesn't change layout).
    format: PixelFormat,
}

impl IccTransformSource {
    /// Create a streaming ICC transform using any CMS backend.
    ///
    /// Parses both ICC profiles and builds a format-aware transform via the
    /// provided CMS. The transform runs at the native bit depth (u8, u16, or
    /// f32) of the upstream pixel format.
    ///
    /// # Errors
    ///
    /// Returns `PipeError::Op` if the ICC profiles can't be parsed or the
    /// CMS can't create a transform between them.
    pub fn new<C: ColorManagement>(
        upstream: Box<dyn Source>,
        src_icc: &[u8],
        dst_icc: &[u8],
        cms: &C,
    ) -> crate::PipeResult<Self> {
        let format = upstream.format();
        let pixel_format = format.pixel_format();

        let transform = cms
            .build_transform_for_format(src_icc, dst_icc, pixel_format, pixel_format)
            .map_err(|e| at!(PipeError::Op(alloc::format!("ICC transform failed: {e:?}"))))?;

        Ok(Self {
            upstream,
            transform,
            dst_icc: Arc::from(dst_icc),
            buf: Vec::new(),
            format,
        })
    }

    /// Create from a pre-built [`RowTransform`].
    ///
    /// Use this when you've already created the transform (e.g., from a
    /// cached CMS instance) and want to avoid re-parsing ICC profiles.
    pub fn from_transform(
        upstream: Box<dyn Source>,
        transform: Box<dyn RowTransform>,
        dst_icc: Arc<[u8]>,
    ) -> Self {
        let format = upstream.format();
        Self {
            upstream,
            transform,
            dst_icc,
            buf: Vec::new(),
            format,
        }
    }

    /// The destination ICC profile bytes.
    pub fn dst_icc(&self) -> &Arc<[u8]> {
        &self.dst_icc
    }
}

impl Source for IccTransformSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        use crate::strip::BufferResultExt as _;
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
                width,
            );
        }

        Ok(Some(Strip::new(
            &self.buf,
            width,
            height,
            stride,
            self.format,
        ).pipe_err()?))
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
