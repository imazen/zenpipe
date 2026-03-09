use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::ops::PixelOp;
use crate::strip::StripRef;

/// Fuses per-pixel operations into a single-pass transform over strips.
///
/// Each call to [`next`](Source::next) pulls a strip from the upstream
/// source, then applies all queued [`PixelOp`]s in sequence. Adjacent
/// in-place ops share a buffer; format-changing ops use ping-pong buffers.
pub struct TransformSource {
    upstream: Box<dyn Source>,
    ops: Vec<Box<dyn PixelOp>>,
    /// Two ping-pong buffers for alternating input/output.
    buf_a: Vec<u8>,
    buf_b: Vec<u8>,
    output_format: PixelFormat,
}

impl TransformSource {
    /// Wrap an upstream source with zero ops (passthrough until ops are pushed).
    pub fn new(upstream: Box<dyn Source>) -> Self {
        let fmt = upstream.format();
        Self {
            upstream,
            ops: Vec::new(),
            buf_a: Vec::new(),
            buf_b: Vec::new(),
            output_format: fmt,
        }
    }

    /// Append a per-pixel operation to the fused chain.
    ///
    /// # Panics
    ///
    /// Panics if `op.input_format()` doesn't match the current output format.
    pub fn push(mut self, op: impl PixelOp + 'static) -> Self {
        assert_eq!(
            op.input_format(),
            self.output_format,
            "op input format {:?} doesn't match chain output {:?}",
            op.input_format(),
            self.output_format,
        );
        self.output_format = op.output_format();
        self.ops.push(Box::new(op));
        self
    }

    /// Append a boxed per-pixel operation to the fused chain.
    ///
    /// Like [`push`](Self::push) but accepts a pre-boxed trait object.
    pub fn push_boxed(mut self, op: Box<dyn PixelOp>) -> Self {
        assert_eq!(
            op.input_format(),
            self.output_format,
            "op input format {:?} doesn't match chain output {:?}",
            op.input_format(),
            self.output_format,
        );
        self.output_format = op.output_format();
        self.ops.push(op);
        self
    }
}

impl Source for TransformSource {
    fn next(&mut self) -> Result<Option<StripRef<'_>>, PipeError> {
        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            return Ok(None);
        };

        let width = strip.width;
        let height = strip.height;
        let y = strip.y;

        // Seed buf_a with upstream data
        self.buf_a.resize(strip.data.len(), 0);
        self.buf_a.copy_from_slice(strip.data);

        if self.ops.is_empty() {
            return Ok(Some(StripRef {
                data: &self.buf_a,
                width,
                height,
                stride: strip.stride,
                y,
                format: strip.format,
            }));
        }

        // current_is_a tracks which buffer holds the current data
        let mut current_is_a = true;

        for op in &self.ops {
            let out_size = op.output_format().row_bytes(width) * height as usize;

            if op.input_format() == op.output_format() {
                // In-place: read from current, write to other, then swap
                if current_is_a {
                    self.buf_b.resize(out_size, 0);
                    op.apply(&self.buf_a, &mut self.buf_b, width, height);
                    current_is_a = false;
                } else {
                    self.buf_a.resize(out_size, 0);
                    op.apply(&self.buf_b, &mut self.buf_a, width, height);
                    current_is_a = true;
                }
            } else {
                // Format change: write to other buffer
                if current_is_a {
                    self.buf_b.resize(out_size, 0);
                    op.apply(&self.buf_a, &mut self.buf_b, width, height);
                    current_is_a = false;
                } else {
                    self.buf_a.resize(out_size, 0);
                    op.apply(&self.buf_b, &mut self.buf_a, width, height);
                    current_is_a = true;
                }
            }
        }

        let stride = self.output_format.row_bytes(width);
        let data = if current_is_a {
            &self.buf_a
        } else {
            &self.buf_b
        };

        Ok(Some(StripRef {
            data,
            width,
            height,
            stride,
            y,
            format: self.output_format,
        }))
    }

    fn width(&self) -> u32 {
        self.upstream.width()
    }
    fn height(&self) -> u32 {
        self.upstream.height()
    }
    fn format(&self) -> PixelFormat {
        self.output_format
    }
}
