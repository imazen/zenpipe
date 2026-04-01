use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::format::PixelFormat;
use crate::ops::PixelOp;
use crate::strip::Strip;

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
    pub fn push(self, op: impl PixelOp + 'static) -> Self {
        self.push_boxed(Box::new(op))
    }

    /// Append a boxed per-pixel operation to the fused chain.
    ///
    /// Like [`push`](Self::push) but accepts a pre-boxed trait object.
    /// If both this op and the previous op are `RowConverterOp`s,
    /// they are composed into a single converter (eliminating one
    /// intermediate buffer allocation per strip).
    pub fn push_boxed(mut self, op: Box<dyn PixelOp>) -> Self {
        assert_eq!(
            op.input_format(),
            self.output_format,
            "op input format {} doesn't match chain output {}",
            op.input_format(),
            self.output_format,
        );

        // Try to compose with the last op if both are RowConverters.
        if let Some(new_rc) = op.as_row_converter() {
            if let Some(last) = self.ops.last() {
                if let Some(prev_rc) = last.as_row_converter() {
                    if let Some(composed) = prev_rc.compose(new_rc) {
                        let from = self.ops.last().unwrap().input_format();
                        let to = op.output_format();
                        self.ops.pop();
                        if composed.is_identity() {
                            // Total cancellation — no op needed.
                            self.output_format = from;
                        } else {
                            let composed_op =
                                crate::ops::RowConverterOp::from_converter(composed, from, to);
                            self.output_format = to;
                            self.ops.push(Box::new(composed_op));
                        }
                        return self;
                    }
                }
            }
        }

        self.output_format = op.output_format();
        self.ops.push(op);
        self
    }
}

impl Source for TransformSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        use crate::strip::BufferResultExt as _;
        let strip = self.upstream.next()?;
        let Some(strip) = strip else {
            return Ok(None);
        };

        let width = strip.width();
        let height = strip.rows();

        if self.ops.is_empty() {
            // No ops — copy to buf_a for lifetime management.
            self.buf_a.resize(strip.as_strided_bytes().len(), 0);
            self.buf_a.copy_from_slice(strip.as_strided_bytes());
            return Ok(Some(
                Strip::new(
                    &self.buf_a,
                    width,
                    height,
                    strip.stride(),
                    self.output_format,
                )
                .pipe_err()?,
            ));
        }

        // Apply first op directly from strip.as_strided_bytes() → buf_b, skipping the
        // buf_a copy. This saves one full memcpy per strip.
        let first_op = &mut self.ops[0];
        let out_size = first_op.output_format().aligned_stride(width) * height as usize;
        self.buf_b.resize(out_size, 0);
        first_op.apply(strip.as_strided_bytes(), &mut self.buf_b, width, height);
        let mut current_is_a = false;

        // Remaining ops ping-pong between buf_a and buf_b.
        for op in &mut self.ops[1..] {
            let out_size = op.output_format().aligned_stride(width) * height as usize;
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

        let stride = self.output_format.aligned_stride(width);
        let data = if current_is_a {
            &self.buf_a
        } else {
            &self.buf_b
        };

        Ok(Some(
            Strip::new(data, width, height, stride, self.output_format).pipe_err()?,
        ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{RGB8_SRGB, RGBA8_SRGB};
    use crate::ops::RowConverterOp;
    use crate::sources::materialize::MaterializedSource;

    #[test]
    fn passthrough_no_ops() {
        let w = 2u32;
        let h = 2u32;
        let fmt = RGBA8_SRGB;
        let stride = fmt.aligned_stride(w); // 8

        // Two rows of distinct RGBA pixels.
        #[rustfmt::skip]
        let data: Vec<u8> = vec![
            // row 0: red, green
            255, 0, 0, 255,   0, 255, 0, 255,
            // row 1: blue, white
            0, 0, 255, 255,   255, 255, 255, 255,
        ];

        let upstream = MaterializedSource::from_data(data.clone(), w, h, fmt).with_strip_height(h);

        let mut ts = TransformSource::new(Box::new(upstream));

        assert_eq!(ts.format(), fmt);
        assert_eq!(ts.width(), w);
        assert_eq!(ts.height(), h);

        let strip = ts.next().unwrap().unwrap();
        assert_eq!(strip.rows(), 2);
        assert_eq!(strip.width(), 2);
        // Verify pixel data matches.
        assert_eq!(&strip.row(0)[..stride], &data[..stride]);
        assert_eq!(&strip.row(1)[..stride], &data[stride..stride * 2]);

        // No more strips.
        assert!(ts.next().unwrap().is_none());
    }

    #[test]
    fn rgba8_to_rgb8_conversion() {
        let w = 2u32;
        let h = 1u32;
        let fmt_in = RGBA8_SRGB;

        // Two RGBA pixels: red opaque, green opaque.
        #[rustfmt::skip]
        let data: Vec<u8> = vec![
            255, 0, 0, 255,   0, 255, 0, 255,
        ];

        let upstream = MaterializedSource::from_data(data, w, h, fmt_in).with_strip_height(h);

        let converter = RowConverterOp::must(fmt_in, RGB8_SRGB);
        let mut ts = TransformSource::new(Box::new(upstream)).push(converter);

        assert_eq!(ts.format(), RGB8_SRGB);

        let strip = ts.next().unwrap().unwrap();
        assert_eq!(strip.rows(), 1);
        assert_eq!(strip.width(), 2);

        let row = strip.row(0);
        let rgb_stride = RGB8_SRGB.aligned_stride(w); // 6
        // First pixel: R=255, G=0, B=0
        assert_eq!(row[0], 255);
        assert_eq!(row[1], 0);
        assert_eq!(row[2], 0);
        // Second pixel: R=0, G=255, B=0
        assert_eq!(row[3], 0);
        assert_eq!(row[4], 255);
        assert_eq!(row[5], 0);

        let _ = rgb_stride; // used for documentation

        assert!(ts.next().unwrap().is_none());
    }
}
