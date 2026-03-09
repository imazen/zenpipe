use alloc::boxed::Box;
use alloc::vec;

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::{StripBuf, StripRef};

/// Porter-Duff source-over compositing of two strip sources.
///
/// Pulls synchronized strips from a background and foreground source,
/// composites foreground over background in premultiplied linear f32
/// space, and outputs the result.
///
/// Both sources must produce [`Rgbaf32LinearPremul`](PixelFormat::Rgbaf32LinearPremul)
/// and the background dimensions determine output dimensions.
pub struct CompositeSource {
    background: Box<dyn Source>,
    foreground: Box<dyn Source>,
    fg_x: u32,
    fg_y: u32,
    fg_h: u32,
    width: u32,
    height: u32,
    strip_height: u32,
    buf: StripBuf,
    y: u32,
}

impl CompositeSource {
    /// Create a source-over composite of `foreground` over `background`.
    ///
    /// Both must produce `Rgbaf32LinearPremul`.
    pub fn over(
        background: Box<dyn Source>,
        foreground: Box<dyn Source>,
    ) -> Result<Self, PipeError> {
        Self::over_at(background, foreground, 0, 0)
    }

    /// Composite `foreground` at offset `(fg_x, fg_y)` within `background`.
    pub fn over_at(
        background: Box<dyn Source>,
        foreground: Box<dyn Source>,
        fg_x: u32,
        fg_y: u32,
    ) -> Result<Self, PipeError> {
        let fmt = PixelFormat::Rgbaf32LinearPremul;
        if background.format() != fmt {
            return Err(PipeError::FormatMismatch {
                expected: fmt,
                got: background.format(),
            });
        }
        if foreground.format() != fmt {
            return Err(PipeError::FormatMismatch {
                expected: fmt,
                got: foreground.format(),
            });
        }

        let w = background.width();
        let h = background.height();
        let fg_h = foreground.height();
        let sh = 16u32.min(h);

        Ok(Self {
            background,
            foreground,
            fg_x,
            fg_y,
            fg_h,
            width: w,
            height: h,
            strip_height: sh,
            buf: StripBuf::new(w, sh, fmt),
            y: 0,
        })
    }
}

impl Source for CompositeSource {
    fn next(&mut self) -> Result<Option<StripRef<'_>>, PipeError> {
        if self.y >= self.height {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.height - self.y);
        self.buf
            .reconfigure(self.width, rows_wanted, PixelFormat::Rgbaf32LinearPremul);
        self.buf.reset(self.y);

        // Pull background strip
        let bg_strip = self.background.next()?;
        let Some(bg_strip) = bg_strip else {
            return Ok(None);
        };

        // Check if foreground overlaps this strip's y range
        let strip_y_end = self.y + bg_strip.height;
        let fg_overlaps = self.y < self.fg_y + self.fg_h && strip_y_end > self.fg_y;

        if !fg_overlaps {
            // No foreground — pass through background
            for r in 0..bg_strip.height {
                self.buf.push_row(bg_strip.row(r));
            }
        } else {
            // Pull foreground strip
            let fg_strip = self.foreground.next()?;

            for r in 0..bg_strip.height {
                let abs_y = self.y + r;
                let bg_row = bg_strip.row(r);

                let has_fg =
                    abs_y >= self.fg_y && abs_y < self.fg_y + self.fg_h && fg_strip.is_some();

                if has_fg {
                    let fg = fg_strip.as_ref().unwrap();
                    let fg_local_y = abs_y - self.fg_y;
                    if fg_local_y >= fg.y && fg_local_y < fg.y + fg.height {
                        let fg_row = fg.row(fg_local_y - fg.y);
                        let stride = self.buf.stride();
                        let mut out_row = vec![0u8; stride];
                        composite_row_over(
                            bg_row,
                            fg_row,
                            &mut out_row,
                            self.fg_x as usize,
                            self.width as usize,
                        );
                        self.buf.push_row(&out_row);
                        continue;
                    }
                }

                self.buf.push_row(bg_row);
            }
        }

        self.y += self.buf.rows_filled();
        Ok(Some(self.buf.as_ref()))
    }

    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn format(&self) -> PixelFormat {
        PixelFormat::Rgbaf32LinearPremul
    }
}

/// Porter-Duff source-over for one row: out = fg + bg * (1 - fg.alpha)
fn composite_row_over(bg: &[u8], fg: &[u8], out: &mut [u8], fg_x_offset: usize, bg_width: usize) {
    let bg_f32: &[f32] = bytemuck::cast_slice(bg);
    let fg_f32: &[f32] = bytemuck::cast_slice(fg);
    let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out);

    out_f32.copy_from_slice(bg_f32);

    let fg_pixels = fg_f32.len() / 4;
    for px in 0..fg_pixels {
        let dst_px = fg_x_offset + px;
        if dst_px >= bg_width {
            break;
        }
        let si = px * 4;
        let di = dst_px * 4;
        let inv_a = 1.0 - fg_f32[si + 3];
        out_f32[di] = fg_f32[si] + out_f32[di] * inv_a;
        out_f32[di + 1] = fg_f32[si + 1] + out_f32[di + 1] * inv_a;
        out_f32[di + 2] = fg_f32[si + 2] + out_f32[di + 2] * inv_a;
        out_f32[di + 3] = fg_f32[si + 3] + out_f32[di + 3] * inv_a;
    }
}
