use alloc::boxed::Box;
use alloc::vec;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::{self, PixelFormat};
use crate::strip::{Strip, StripBuf};

/// Compositing of two strip sources with configurable blend mode.
///
/// Pulls synchronized strips from a background and foreground source,
/// composites foreground over background in premultiplied linear f32
/// space, and outputs the result.
///
/// Both sources must produce `RGBAF32_LINEAR_PREMUL`
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
    blend_mode: zenblend::BlendMode,
}

impl CompositeSource {
    /// Create a source-over composite of `foreground` over `background`.
    ///
    /// Both must produce `RGBAF32_LINEAR_PREMUL`.
    pub fn over(
        background: Box<dyn Source>,
        foreground: Box<dyn Source>,
    ) -> crate::PipeResult<Self> {
        Self::over_at(background, foreground, 0, 0)
    }

    /// Set the blend mode (default: `SrcOver`).
    pub fn with_blend_mode(mut self, mode: zenblend::BlendMode) -> Self {
        self.blend_mode = mode;
        self
    }

    /// Composite `foreground` at offset `(fg_x, fg_y)` within `background`.
    pub fn over_at(
        background: Box<dyn Source>,
        foreground: Box<dyn Source>,
        fg_x: u32,
        fg_y: u32,
    ) -> crate::PipeResult<Self> {
        let fmt = format::RGBAF32_LINEAR_PREMUL;
        if background.format() != fmt {
            return Err(at!(PipeError::FormatMismatch {
                expected: fmt,
                got: background.format(),
            }));
        }
        if foreground.format() != fmt {
            return Err(at!(PipeError::FormatMismatch {
                expected: fmt,
                got: foreground.format(),
            }));
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
            blend_mode: zenblend::BlendMode::SrcOver,
        })
    }
}

impl Source for CompositeSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        if self.y >= self.height {
            return Ok(None);
        }

        let rows_wanted = self.strip_height.min(self.height - self.y);
        self.buf
            .reconfigure(self.width, rows_wanted, format::RGBAF32_LINEAR_PREMUL);
        self.buf.reset();

        // Pull background strip
        let bg_strip = self.background.next()?;
        let Some(bg_strip) = bg_strip else {
            return Ok(None);
        };

        // Check if foreground overlaps this strip's y range
        let strip_y_end = self.y + bg_strip.rows();
        let fg_overlaps = self.y < self.fg_y + self.fg_h && strip_y_end > self.fg_y;

        if !fg_overlaps {
            // No foreground — pass through background
            for r in 0..bg_strip.rows() {
                self.buf.push_row(bg_strip.row(r));
            }
        } else {
            // Pull foreground strip
            let fg_strip = self.foreground.next()?;

            for r in 0..bg_strip.rows() {
                let abs_y = self.y + r;
                let bg_row = bg_strip.row(r);

                let has_fg =
                    abs_y >= self.fg_y && abs_y < self.fg_y + self.fg_h && fg_strip.is_some();

                if has_fg {
                    let fg = fg_strip.as_ref().unwrap();
                    let fg_local_y = abs_y - self.fg_y;
                    if fg_local_y < fg.rows() {
                        let fg_row = fg.row(fg_local_y);
                        let stride = self.buf.stride();
                        let mut out_row = vec![0u8; stride];
                        composite_row_over(
                            bg_row,
                            fg_row,
                            &mut out_row,
                            self.fg_x as usize,
                            self.width as usize,
                            self.blend_mode,
                        );
                        self.buf.push_row(&out_row);
                        continue;
                    }
                }

                self.buf.push_row(bg_row);
            }
        }

        self.y += self.buf.rows_filled();
        Ok(Some(self.buf.as_strip()))
    }

    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn format(&self) -> PixelFormat {
        format::RGBAF32_LINEAR_PREMUL
    }
}

/// Blend one row: fg placed at `fg_x_offset` within bg, result written to out.
fn composite_row_over(
    bg: &[u8],
    fg: &[u8],
    out: &mut [u8],
    fg_x_offset: usize,
    bg_width: usize,
    mode: zenblend::BlendMode,
) {
    let bg_f32: &[f32] = bytemuck::cast_slice(bg);
    let fg_f32: &[f32] = bytemuck::cast_slice(fg);
    let out_f32: &mut [f32] = bytemuck::cast_slice_mut(out);

    // Start with background
    out_f32.copy_from_slice(bg_f32);

    // Determine the overlapping pixel range
    let fg_pixels = fg_f32.len() / 4;
    let blend_end = (fg_x_offset + fg_pixels).min(bg_width);
    if fg_x_offset >= blend_end {
        return;
    }
    let blend_pixels = blend_end - fg_x_offset;

    // Copy fg into the overlap region of out, then blend in-place
    let dst_start = fg_x_offset * 4;
    let dst_end = dst_start + blend_pixels * 4;
    let fg_end = blend_pixels * 4;

    // fg goes into out[dst_start..dst_end], bg is already there.
    // zenblend::blend_row(fg, bg, mode) writes result into fg.
    // We need: out = blend(fg, bg_at_offset).
    // Strategy: save bg portion, copy fg into out, blend out over saved bg.
    // But that's more copies. Simpler: copy fg, blend bg underneath.

    // For SrcOver: out = fg + bg * (1 - fg.a)
    // zenblend expects fg in first arg (modified in-place), bg in second.
    // We want: result = blend(fg_slice, bg_slice_at_offset)
    // Copy fg into the overlap region, then blend bg underneath.
    let bg_portion: alloc::vec::Vec<f32> = out_f32[dst_start..dst_end].to_vec();
    out_f32[dst_start..dst_end].copy_from_slice(&fg_f32[..fg_end]);
    zenblend::blend_row(&mut out_f32[dst_start..dst_end], &bg_portion, mode);
}
