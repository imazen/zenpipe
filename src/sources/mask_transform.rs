use alloc::boxed::Box;
use alloc::vec;

use crate::Source;
use crate::error::PipeError;
use crate::format::{self, PixelFormat};
use crate::strip::{Strip, StripBuf};

/// Applies a mask to a source without resizing.
///
/// Multiplies premultiplied RGBA rows by per-pixel mask values,
/// shaping the output's transparency (e.g., rounded corners → PNG/WebP).
///
/// Requires upstream format `RGBAF32_LINEAR_PREMUL`.
pub struct MaskTransformSource {
    upstream: Box<dyn Source>,
    mask: Box<dyn zenblend::mask::MaskSource + Send>,
    buf: StripBuf,
    mask_buf: Vec<f32>,
    y: u32,
}

impl MaskTransformSource {
    /// Create a mask transform source.
    ///
    /// `upstream` must produce `RGBAF32_LINEAR_PREMUL`.
    /// The `mask` is applied row-by-row, multiplying all 4 RGBA channels.
    pub fn new(
        upstream: Box<dyn Source>,
        mask: Box<dyn zenblend::mask::MaskSource + Send>,
    ) -> Result<Self, PipeError> {
        let fmt = format::RGBAF32_LINEAR_PREMUL;
        if upstream.format() != fmt {
            return Err(PipeError::FormatMismatch {
                expected: fmt,
                got: upstream.format(),
            });
        }
        let w = upstream.width();
        let h = upstream.height();
        let sh = 16u32.min(h);
        let mask_buf = vec![0.0f32; w as usize];
        Ok(Self {
            upstream,
            mask,
            buf: StripBuf::new(w, sh, fmt),
            mask_buf,
            y: 0,
        })
    }
}

impl Source for MaskTransformSource {
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            return Ok(None);
        };

        self.buf
            .reconfigure(strip.width(), strip.rows(), strip.descriptor());
        self.buf.reset();

        for r in 0..strip.rows() {
            let src_row = strip.row(r);
            self.buf.push_row(src_row);
            let dst_row = self.buf.row_mut(r);

            let fill = self.mask.fill_mask_row(&mut self.mask_buf, self.y);
            match fill {
                zenblend::mask::MaskFill::AllOpaque => {} // no-op, data already copied
                zenblend::mask::MaskFill::AllTransparent => {
                    dst_row.fill(0);
                }
                zenblend::mask::MaskFill::Partial => {
                    let row_f32: &mut [f32] = bytemuck::cast_slice_mut(dst_row);
                    zenblend::mask_row(row_f32, &self.mask_buf);
                }
            }
            self.y += 1;
        }

        Ok(Some(self.buf.as_strip()))
    }

    fn width(&self) -> u32 {
        self.upstream.width()
    }
    fn height(&self) -> u32 {
        self.upstream.height()
    }
    fn format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
}
