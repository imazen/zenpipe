//! Spool a source to a temporary file and replay it — decode once, pull
//! any number of times, with one strip of RAM (zenpipe#24).
//!
//! The "analysis barrier without full materialization" primitive for
//! gigapixel inputs: a two-pass operation (statistics, then the real
//! pass) streams the decoded rows to disk on the first pull and reads
//! them back on every [`rewind`](TempFileSource::rewind). The OS page
//! cache decides how much of the file stays resident; this process holds
//! `strip_rows × width × bpp` bytes.
//!
//! Why a file and not `mmap`: zenpipe forbids `unsafe`, and every mmap
//! crate exposes the map as an `unsafe fn` (the file can change under the
//! mapping), so a memory-mapped [`MaterializedSource`](super::MaterializedSource)
//! is off the table. Operations that need random access to the whole
//! frame (`Analyze`, `CropWhitespace`, `EffectSource`) still materialize;
//! everything that can run as a second streaming pass can use this.
//!
//! The file lives in `ZENPIPE_SPOOL_DIR` when set, else
//! [`std::env::temp_dir`], and is removed on drop.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::Source;
use crate::error::{PipeError, PipeResult};
use crate::format::PixelFormat;
use crate::strip::{Strip, StripBuf};
use whereat::at;

static SPOOL_COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A source replayed from a temporary file. See the [module docs](self).
pub struct TempFileSource {
    file: File,
    path: PathBuf,
    width: u32,
    height: u32,
    format: PixelFormat,
    row_bytes: usize,
    strip_rows: u32,
    y: u32,
    row_scratch: Vec<u8>,
    buf: StripBuf,
    passes: u32,
}

impl TempFileSource {
    /// Directory spool files go to: `ZENPIPE_SPOOL_DIR`, else the OS temp dir.
    pub fn spool_dir() -> PathBuf {
        std::env::var_os("ZENPIPE_SPOOL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
    }

    /// Drain `upstream` into a spool file in [`spool_dir`](Self::spool_dir)
    /// and position at the first row. Strips come back `strip_rows` high.
    pub fn from_source(upstream: Box<dyn Source>, strip_rows: u32) -> PipeResult<Self> {
        Self::from_source_in(&Self::spool_dir(), upstream, strip_rows)
    }

    /// [`from_source`](Self::from_source) with an explicit directory.
    pub fn from_source_in(
        dir: &Path,
        mut upstream: Box<dyn Source>,
        strip_rows: u32,
    ) -> PipeResult<Self> {
        let width = upstream.width();
        let height = upstream.height();
        let format = upstream.format();
        if width == 0 || height == 0 || strip_rows == 0 {
            return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                "TempFileSource: empty geometry {width}x{height} / strip {strip_rows}"
            ))));
        }
        let row_bytes = crate::limits::checked_buffer_size(width, 1, format.bytes_per_pixel())?;
        let n = SPOOL_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = dir.join(alloc::format!(
            "zenpipe-spool-{}-{n}-{nanos}.raw",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| Self::io(e, &path, "create"))?;

        // Spool: packed rows, no stride padding.
        let mut rows_written = 0u32;
        {
            let mut out = BufWriter::with_capacity((row_bytes * 16).min(1 << 20), &file);
            while let Some(strip) = upstream.next()? {
                for r in 0..strip.rows() {
                    if rows_written >= height {
                        return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                            "TempFileSource: upstream produced more than {height} rows"
                        ))));
                    }
                    out.write_all(&strip.row(r)[..row_bytes])
                        .map_err(|e| Self::io(e, &path, "write"))?;
                    rows_written += 1;
                }
            }
            out.flush().map_err(|e| Self::io(e, &path, "flush"))?;
        }
        if rows_written != height {
            let _ = std::fs::remove_file(&path);
            return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                "TempFileSource: upstream produced {rows_written} of {height} rows"
            ))));
        }
        let strip_rows = strip_rows.min(height);
        Ok(Self {
            file,
            path,
            width,
            height,
            format,
            row_bytes,
            strip_rows,
            y: 0,
            row_scratch: vec![0u8; row_bytes],
            buf: StripBuf::new(width, strip_rows, format),
            passes: 0,
        })
    }

    /// Start the next pass from row 0.
    pub fn rewind(&mut self) {
        self.y = 0;
    }

    /// The spool file (removed when this source drops).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes the spool holds: `width × height × bpp`.
    pub fn bytes_on_disk(&self) -> u64 {
        self.row_bytes as u64 * u64::from(self.height)
    }

    /// Completed passes (a pass completes when `next()` returns `None`).
    pub fn passes(&self) -> u32 {
        self.passes
    }

    fn io(e: std::io::Error, path: &Path, what: &str) -> whereat::At<PipeError> {
        at!(PipeError::Op(alloc::format!(
            "TempFileSource: {what} {}: {e}",
            path.display()
        )))
    }
}

impl Source for TempFileSource {
    fn next(&mut self) -> PipeResult<Option<Strip<'_>>> {
        if self.y >= self.height {
            self.passes += 1;
            return Ok(None);
        }
        let rows = self.strip_rows.min(self.height - self.y);
        let offset = self.y as u64 * self.row_bytes as u64;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| Self::io(e, &self.path, "seek"))?;
        self.buf.reset();
        self.buf.reconfigure(self.width, rows, self.format);
        for _ in 0..rows {
            self.file
                .read_exact(&mut self.row_scratch)
                .map_err(|e| Self::io(e, &self.path, "read"))?;
            self.buf.push_row(&self.row_scratch);
        }
        self.y += rows;
        Ok(Some(self.buf.as_strip()))
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

impl Drop for TempFileSource {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format;
    use crate::sources::{CallbackSource, MaterializedSource};

    fn pattern(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| [i as u8, (i >> 8) as u8, (i * 3) as u8, 255])
            .collect()
    }

    fn drain(src: &mut dyn Source) -> (Vec<u8>, u32) {
        let w = src.width() as usize * 4;
        let mut out = Vec::new();
        let mut strips = 0;
        while let Some(s) = src.next().unwrap() {
            for r in 0..s.rows() {
                out.extend_from_slice(&s.row(r)[..w]);
            }
            strips += 1;
        }
        (out, strips)
    }

    #[test]
    fn spool_replays_the_source_any_number_of_times() {
        let (w, h) = (300u32, 200u32);
        let img = pattern(w, h);
        let src: Box<dyn Source> = Box::new(MaterializedSource::from_data(
            img.clone(),
            w,
            h,
            format::RGBA8_SRGB,
        ));
        let dir =
            std::env::temp_dir().join(alloc::format!("zenpipe-spool-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut spool = TempFileSource::from_source_in(&dir, src, 7).unwrap();
        let path = spool.path().to_path_buf();
        assert!(path.exists());
        assert_eq!(spool.bytes_on_disk(), u64::from(w * h * 4));
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            spool.bytes_on_disk()
        );
        assert_eq!((spool.width(), spool.height()), (w, h));

        let (first, strips) = drain(&mut spool);
        assert_eq!(first, img);
        assert_eq!(strips, h.div_ceil(7));
        assert_eq!(spool.passes(), 1);
        // Exhausted until rewound.
        assert!(spool.next().unwrap().is_none());
        spool.rewind();
        let (second, _) = drain(&mut spool);
        assert_eq!(second, img);
        assert_eq!(spool.passes(), 3);

        drop(spool);
        assert!(!path.exists(), "spool file removed on drop");
    }

    #[test]
    fn spool_works_from_a_row_generator_with_no_frame_in_memory() {
        // A callback source only ever holds one strip; the spool is the
        // first place the whole image exists — on disk.
        let (w, h) = (64u32, 33u32);
        let mut y = 0u32;
        let src: Box<dyn Source> = Box::new(CallbackSource::new(
            w,
            h,
            format::RGBA8_SRGB,
            5,
            move |buf| {
                if y >= h {
                    return Ok(false);
                }
                for (x, px) in buf[..w as usize * 4]
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .enumerate()
                {
                    *px = [x as u8, y as u8, 0, 255];
                }
                y += 1;
                Ok(true)
            },
        ));
        let mut spool = TempFileSource::from_source(src, 16).unwrap();
        let (bytes, strips) = drain(&mut spool);
        assert_eq!(strips, 3);
        assert_eq!(bytes.len(), (w * h * 4) as usize);
        assert_eq!(&bytes[(32 * 64 + 10) * 4..][..4], &[10, 32, 0, 255]);
    }

    #[test]
    fn spool_rejects_short_upstream_and_cleans_up() {
        let (w, h) = (8u32, 8u32);
        let mut y = 0u32;
        // Claims 8 rows, produces 5.
        let src: Box<dyn Source> = Box::new(CallbackSource::new(
            w,
            h,
            format::RGBA8_SRGB,
            4,
            move |_buf| {
                if y >= 5 {
                    return Ok(false);
                }
                y += 1;
                Ok(true)
            },
        ));
        let dir =
            std::env::temp_dir().join(alloc::format!("zenpipe-spool-short-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(TempFileSource::from_source_in(&dir, src, 4).is_err());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "no spool left behind"
        );
    }
}
