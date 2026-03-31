//! Identity passthrough Source that records pipeline metadata, optionally
//! dumps pixel data, and measures per-node execution timing.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::Source;
#[allow(unused_imports)]
use whereat::at;

use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};
use crate::trace::TraceEntry;

/// Identity passthrough Source that records metadata at a pipeline node boundary.
///
/// When pixel dump is enabled, accumulates all rows and writes on completion.
/// When timing is enabled, measures cumulative upstream pull duration.
/// The output is always identical to the input — this is a pure observer.
pub struct TracingSource {
    upstream: Box<dyn Source>,
    buf: StripBuf,
    /// Accumulated pixel data for dump (only allocated when dump is active).
    #[cfg(feature = "std")]
    dump_buf: Option<Vec<u8>>,
    #[cfg(feature = "std")]
    dump_path: Option<std::path::PathBuf>,
    /// Shared timing data (populated during execution, readable after).
    #[cfg(feature = "std")]
    timing: Option<alloc::sync::Arc<std::sync::Mutex<crate::trace::NodeTiming>>>,
    format: PixelFormat,
    width: u32,
    height: u32,
}

impl TracingSource {
    /// Wrap a source with tracing. The `entry` is already recorded in the trace.
    pub fn new(
        upstream: Box<dyn Source>,
        _entry: &TraceEntry,
        #[cfg(feature = "std")] dump_path: Option<std::path::PathBuf>,
    ) -> Self {
        let width = upstream.width();
        let height = upstream.height();
        let format = upstream.format();
        let buf = StripBuf::new(width, 16u32.min(height), format);

        Self {
            upstream,
            buf,
            #[cfg(feature = "std")]
            dump_buf: if dump_path.is_some() {
                Some(Vec::with_capacity(
                    width as usize * height as usize * format.bytes_per_pixel(),
                ))
            } else {
                None
            },
            #[cfg(feature = "std")]
            dump_path,
            #[cfg(feature = "std")]
            timing: None,
            format,
            width,
            height,
        }
    }

    /// Enable timing measurement with a shared timing handle.
    #[cfg(feature = "std")]
    pub fn with_timing(
        mut self,
        timing: alloc::sync::Arc<std::sync::Mutex<crate::trace::NodeTiming>>,
    ) -> Self {
        self.timing = Some(timing);
        self
    }
}

impl Source for TracingSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        // Measure upstream pull time if timing is enabled.
        #[cfg(feature = "std")]
        let start = self.timing.as_ref().map(|_| std::time::Instant::now());

        let upstream_strip = self.upstream.next()?;

        match upstream_strip {
            Some(strip) => {
                let rows = strip.rows();
                let stride = strip.stride();
                let data = strip.as_strided_bytes();

                // Record timing.
                #[cfg(feature = "std")]
                if let (Some(timing_arc), Some(start)) = (&self.timing, start) {
                    let mut t = timing_arc.lock().unwrap();
                    t.total_duration += start.elapsed();
                    t.strip_count += 1;
                    t.bytes_processed +=
                        self.width as u64 * rows as u64 * self.format.bytes_per_pixel() as u64;
                }

                // Accumulate for dump if active.
                #[cfg(feature = "std")]
                if let Some(ref mut dump) = self.dump_buf {
                    dump.extend_from_slice(data);
                }

                // Copy to our buffer for re-emission (row by row).
                self.buf.reset();
                self.buf.reconfigure(self.width, rows, self.format);
                for r in 0..rows {
                    let row_start = r as usize * stride;
                    let row_end = row_start + self.width as usize * self.format.bytes_per_pixel();
                    if row_end <= data.len() {
                        self.buf.push_row(&data[row_start..row_end]);
                    }
                }

                Ok(Some(self.buf.as_strip()))
            }
            None => {
                // Record final timing for the EOF pull.
                #[cfg(feature = "std")]
                if let (Some(timing_arc), Some(start)) = (&self.timing, start) {
                    let mut t = timing_arc.lock().unwrap();
                    t.total_duration += start.elapsed();
                }

                // Pipeline exhausted — write dump if active.
                #[cfg(feature = "std")]
                self.write_dump();

                Ok(None)
            }
        }
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

#[cfg(feature = "std")]
impl TracingSource {
    fn write_dump(&mut self) {
        let Some(ref path) = self.dump_path else {
            return;
        };
        let Some(ref data) = self.dump_buf else {
            return;
        };
        if data.is_empty() {
            return;
        }

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let bpp = self.format.bytes_per_pixel();
        let stride = self.width as usize * bpp;
        let actual_rows = data.len() / stride.max(1);

        let meta = format!(
            "{}x{} {} rows={} format={:?}\n",
            self.width, self.height, bpp, actual_rows, self.format
        );
        let meta_path = path.with_extension("meta.txt");
        let _ = std::fs::write(&meta_path, meta);
        let _ = std::fs::write(path, data);

        eprintln!("[trace] wrote {} bytes to {}", data.len(), path.display());
    }
}

#[cfg(feature = "std")]
impl Drop for TracingSource {
    fn drop(&mut self) {
        self.write_dump();
    }
}
