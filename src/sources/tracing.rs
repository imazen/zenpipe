//! Identity passthrough Source that records pipeline metadata and optionally
//! dumps pixel data to PNG16 files.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::Source;
use crate::error::PipeError;
use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};
use crate::trace::TraceEntry;

/// Identity passthrough Source that records metadata at a pipeline node boundary.
///
/// When pixel dump is enabled, accumulates all rows and writes to PNG16 on completion.
/// The output is always identical to the input — this is a pure observer.
pub struct TracingSource {
    upstream: Box<dyn Source>,
    buf: StripBuf,
    /// Accumulated pixel data for PNG16 dump (only allocated when dump is active).
    #[cfg(feature = "std")]
    dump_buf: Option<Vec<u8>>,
    #[cfg(feature = "std")]
    dump_path: Option<std::path::PathBuf>,
    format: PixelFormat,
    width: u32,
    height: u32,
    strip_height: u32,
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
        let strip_height = 16u32.min(height);
        let buf = StripBuf::new(width, strip_height, format);

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
            format,
            width,
            height,
            strip_height,
        }
    }
}

impl Source for TracingSource {
    fn next(&mut self) -> Result<Option<Strip<'_>>, PipeError> {
        let upstream_strip = self.upstream.next()?;

        match upstream_strip {
            Some(strip) => {
                let rows = strip.rows();
                let stride = strip.stride();
                let data = strip.as_strided_bytes();

                // Accumulate for dump if active
                #[cfg(feature = "std")]
                if let Some(ref mut dump) = self.dump_buf {
                    dump.extend_from_slice(data);
                }

                // Copy to our buffer for re-emission (row by row)
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
                // Pipeline exhausted — write dump if active
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
        let Some(ref path) = self.dump_path else { return };
        let Some(ref data) = self.dump_buf else { return };
        if data.is_empty() { return; }

        // Create parent directory
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Write raw pixel data as PNG via zenpng if available,
        // or as a raw .bin file as fallback.
        let bpp = self.format.bytes_per_pixel();
        let stride = self.width as usize * bpp;
        let actual_rows = data.len() / stride.max(1);

        // Write as raw RGBA8 PNG using lodepng-style encoding
        // For now, write as a simple raw dump with metadata sidecar
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
        // Write dump on drop if pipeline errored before exhausting
        self.write_dump();
    }
}
