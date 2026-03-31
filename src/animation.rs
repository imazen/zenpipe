//! Animation support — frame-by-frame decode, process, and encode.
//!
//! Wraps zencodec's `DynAnimationFrameDecoder` / `DynAnimationFrameEncoder` for
//! frame-at-a-time animation processing through zenpipe's streaming pipeline.
//!
//! # Architecture
//!
//! Animation processing decomposes into:
//! 1. **Decode** one composited frame at a time (`FrameSource`)
//! 2. **Process** each frame through a per-frame pipeline (resize, filter, etc.)
//! 3. **Encode** each processed frame into the output animation (`FrameSink`)
//!
//! The [`transcode`] function ties these together for the common case.
//!
//! # Example
//!
//! ```ignore
//! use zenpipe::animation::{FrameSource, FrameSink, transcode};
//!
//! // Decode animated GIF, resize each frame to 200×200, encode as animated WebP
//! transcode(
//!     gif_decoder,
//!     webp_encoder,
//!     |frame_source, frame_index| {
//!         // Build per-frame pipeline
//!         let mut g = PipelineGraph::new();
//!         let src = g.add_node(NodeOp::Source);
//!         let resize = g.add_node(NodeOp::Resize { w: 200, h: 200 });
//!         let out = g.add_node(NodeOp::Output);
//!         g.add_edge(src, resize, EdgeKind::Input);
//!         g.add_edge(resize, out, EdgeKind::Input);
//!         let mut sources = HashMap::new();
//!         sources.insert(src, frame_source);
//!         g.compile(sources)
//!     },
//! )?;
//! ```

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use zencodec::decode::DynAnimationFrameDecoder;
use zencodec::encode::{DynAnimationFrameEncoder, EncodeOutput};
use zenpixels::{PixelDescriptor, PixelSlice};

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::Strip;

// =========================================================================
// Frame metadata
// =========================================================================

/// Metadata for a single animation frame.
#[derive(Debug, Clone, Copy)]
pub struct FrameInfo {
    /// Frame index (0-based).
    pub index: u32,
    /// Frame duration in milliseconds.
    pub duration_ms: u32,
}

// =========================================================================
// FrameSource — wraps DynAnimationFrameDecoder
// =========================================================================

/// A [`Source`] that yields one animation frame's pixel data as strips.
///
/// Wraps a [`DynAnimationFrameDecoder`] and renders one frame at a time.
/// Each frame is materialized internally (animation decoders produce
/// full-canvas composited frames), then replayed as strips.
///
/// Call [`advance_frame()`](Self::advance_frame) to move to the next frame.
/// Returns `None` from [`next()`](Source::next) when the current frame is
/// exhausted. Returns `None` from `advance_frame()` when all frames are done.
pub struct FrameSource {
    decoder: Box<dyn DynAnimationFrameDecoder>,
    /// Current frame's pixel data.
    data: Vec<u8>,
    width: u32,
    height: u32,
    format: PixelFormat,
    stride: usize,
    strip_height: u32,
    /// Current y position within the frame.
    y: u32,
    /// Current frame metadata.
    frame_info: Option<FrameInfo>,
    /// Whether the decoder has been exhausted.
    done: bool,
}

impl FrameSource {
    /// Create a frame source from an animation decoder.
    ///
    /// The first frame is automatically loaded. If the animation has zero
    /// frames, `frame_info()` returns `None`.
    pub fn new(decoder: Box<dyn DynAnimationFrameDecoder>) -> crate::PipeResult<Self> {
        let info = decoder.info().clone();
        let w = info.width;
        let h = info.height;
        let format = PixelDescriptor::RGBA8_SRGB; // animation decoders typically produce RGBA8

        let mut source = Self {
            decoder,
            data: Vec::new(),
            width: w,
            height: h,
            format,
            stride: format.aligned_stride(w),
            strip_height: 16.min(h),
            y: 0,
            frame_info: None,
            done: false,
        };

        // Load the first frame.
        source.load_next_frame()?;

        Ok(source)
    }

    /// Current frame metadata, or `None` if no frame is loaded.
    pub fn frame_info(&self) -> Option<FrameInfo> {
        self.frame_info
    }

    /// Animation loop count from the container.
    ///
    /// `Some(0)` = loop forever, `Some(n)` = play n times, `None` = unknown.
    pub fn loop_count(&self) -> Option<u32> {
        self.decoder.loop_count()
    }

    /// Total number of frames, if known without decoding all of them.
    pub fn frame_count(&self) -> Option<u32> {
        self.decoder.frame_count()
    }

    /// Advance to the next frame. Returns `true` if a new frame was loaded,
    /// `false` if the animation is finished.
    ///
    /// After calling this, [`next()`](Source::next) yields strips from the
    /// new frame. Resets the y position to 0.
    pub fn advance_frame(&mut self) -> crate::PipeResult<bool> {
        if self.done {
            return Ok(false);
        }
        self.load_next_frame()?;
        Ok(self.frame_info.is_some())
    }

    fn load_next_frame(&mut self) -> crate::PipeResult<()> {
        let frame = self
            .decoder
            .render_next_frame_owned(None)
            .map_err(|e| at!(PipeError::Op(e.to_string())))?;

        match frame {
            Some(owned) => {
                let pixels = owned.pixels();
                let w = pixels.width();
                let h = pixels.rows();
                let desc = pixels.descriptor();

                self.width = w;
                self.height = h;
                self.format = desc;
                self.stride = desc.aligned_stride(w);
                self.strip_height = 16.min(h);

                // Copy frame data into our buffer.
                let total = self.stride * h as usize;
                self.data.resize(total, 0);
                for r in 0..h {
                    let src_row = pixels.row(r);
                    let dst_start = r as usize * self.stride;
                    self.data[dst_start..dst_start + self.stride]
                        .copy_from_slice(&src_row[..self.stride]);
                }

                self.y = 0;
                self.frame_info = Some(FrameInfo {
                    index: owned.frame_index(),
                    duration_ms: owned.duration_ms(),
                });
            }
            None => {
                self.done = true;
                self.frame_info = None;
                self.data.clear();
            }
        }

        Ok(())
    }
}

impl Source for FrameSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        use crate::strip::BufferResultExt as _;
        if self.frame_info.is_none() || self.y >= self.height {
            return Ok(None);
        }

        let rows = self.strip_height.min(self.height - self.y);
        let start = self.y as usize * self.stride;
        let end = start + rows as usize * self.stride;

        self.y += rows;

        Ok(Some(Strip::new(
            &self.data[start..end],
            self.width,
            rows,
            self.stride,
            self.format,
        ).pipe_err()?))
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn format(&self) -> PixelFormat {
        self.format
    }
}

// =========================================================================
// FrameSink — wraps DynAnimationFrameEncoder
// =========================================================================

/// A [`Sink`](crate::Sink) that accumulates one frame's strips, then pushes
/// the complete frame to a [`DynAnimationFrameEncoder`].
///
/// Call [`finish_frame()`](Self::finish_frame) after pipeline execution to
/// push the accumulated frame with its duration. Call [`finish()`](Self::finish_animation)
/// after all frames to finalize the encoded output.
pub struct FrameSink {
    encoder: Option<Box<dyn DynAnimationFrameEncoder>>,
    output: Option<EncodeOutput>,
    /// Accumulated pixel data for the current frame.
    frame_buf: Vec<u8>,
    /// Expected frame dimensions.
    width: u32,
    height: u32,
    /// Rows accumulated so far.
    rows_accumulated: u32,
    /// Pixel format of the current frame.
    format: PixelFormat,
}

impl FrameSink {
    /// Create a frame sink wrapping an animation encoder.
    ///
    /// `width` and `height` are the canvas dimensions for the animation.
    /// `format` is the pixel format that strips will arrive in (typically RGBA8_SRGB).
    pub fn new(
        encoder: Box<dyn DynAnimationFrameEncoder>,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Self {
        let stride = format.aligned_stride(width);
        Self {
            encoder: Some(encoder),
            output: None,
            frame_buf: vec![0u8; stride * height as usize],
            width,
            height,
            rows_accumulated: 0,
            format,
        }
    }

    /// Reset for a new frame (clears accumulated pixel data).
    pub fn begin_frame(&mut self) {
        self.rows_accumulated = 0;
    }

    /// Push the accumulated frame to the encoder with the given duration.
    ///
    /// Must be called after all strips for one frame have been consumed
    /// via the `Sink` trait.
    pub fn finish_frame(&mut self, duration_ms: u32) -> crate::PipeResult<()> {
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| at!(PipeError::Op("encoder already finished".to_string())))?;

        let stride = self.format.aligned_stride(self.width);
        let total = stride * self.rows_accumulated as usize;

        let pixels = PixelSlice::new(
            &self.frame_buf[..total],
            self.width,
            self.rows_accumulated,
            stride,
            self.format,
        )
        .map_err(|e| at!(PipeError::Op(alloc::format!("PixelSlice construction failed: {e}"))))?;

        encoder
            .push_frame(pixels, duration_ms, None)
            .map_err(|e| at!(PipeError::Op(e.to_string())))?;

        self.rows_accumulated = 0;
        Ok(())
    }

    /// Finalize the animation and return the encoded output.
    pub fn finish_animation(mut self) -> crate::PipeResult<EncodeOutput> {
        let encoder = self
            .encoder
            .take()
            .ok_or_else(|| at!(PipeError::Op("encoder already finished".to_string())))?;

        encoder
            .finish(None)
            .map_err(|e| at!(PipeError::Op(e.to_string())))
    }

    /// Take the encoded output (if finish_animation was called via trait).
    pub fn take_output(&mut self) -> Option<EncodeOutput> {
        self.output.take()
    }
}

impl crate::Sink for FrameSink {
    fn consume(&mut self, strip: &Strip<'_>) -> crate::PipeResult<()> {
        let stride = self.format.aligned_stride(self.width);
        for r in 0..strip.rows() {
            if self.rows_accumulated >= self.height {
                return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                    "frame sink received more than {} rows",
                    self.height
                ))));
            }
            let src_row = strip.row(r);
            let dst_start = self.rows_accumulated as usize * stride;
            self.frame_buf[dst_start..dst_start + stride].copy_from_slice(&src_row[..stride]);
            self.rows_accumulated += 1;
        }
        Ok(())
    }

    fn finish(&mut self) -> crate::PipeResult<()> {
        // No-op: frame completion is handled by finish_frame().
        // Animation finalization is handled by finish_animation().
        Ok(())
    }
}

// =========================================================================
// transcode() — high-level animation transcoding
// =========================================================================

/// Transcode an animation: decode each frame, process it through a
/// caller-built pipeline, and encode into the output format.
///
/// `build_pipeline` is called once per frame with:
/// - `Box<dyn Source>` — the decoded frame as a source of strips
/// - `u32` — the frame index (0-based)
///
/// It must return a `Box<dyn Source>` that will be drained into the encoder.
///
/// # Example
///
/// ```ignore
/// let output = transcode(
///     gif_decoder,
///     webp_encoder,
///     200, 200,          // output canvas dimensions
///     format::RGBA8_SRGB,
///     |frame_src, _idx| {
///         // Resize each frame to 200×200
///         let mut g = PipelineGraph::new();
///         // ... build graph ...
///         g.compile(sources)
///     },
/// )?;
/// ```
pub fn transcode(
    decoder: Box<dyn DynAnimationFrameDecoder>,
    encoder: Box<dyn DynAnimationFrameEncoder>,
    out_width: u32,
    out_height: u32,
    out_format: PixelFormat,
    mut build_pipeline: impl FnMut(Box<dyn Source>, u32) -> crate::PipeResult<Box<dyn Source>>,
) -> crate::PipeResult<EncodeOutput> {
    let mut frame_source = FrameSource::new(decoder)?;
    let mut frame_sink = FrameSink::new(encoder, out_width, out_height, out_format);

    while let Some(info) = frame_source.frame_info() {
        // Build per-frame pipeline.
        // We need to give the pipeline a Source. Since FrameSource holds the
        // current frame data, we create a MemSource from its current state.
        let frame_data = frame_source.data.clone();
        let w = frame_source.width;
        let h = frame_source.height;
        let fmt = frame_source.format;
        let mem = crate::sources::MaterializedSource::from_data(frame_data, w, h, fmt);

        let mut pipeline = build_pipeline(Box::new(mem), info.index)?;

        // Drain pipeline into frame sink.
        frame_sink.begin_frame();
        crate::execute(pipeline.as_mut(), &mut frame_sink)?;
        frame_sink.finish_frame(info.duration_ms)?;

        // Advance to next frame.
        if !frame_source.advance_frame()? {
            break;
        }
    }

    frame_sink.finish_animation()
}
