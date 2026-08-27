//! Tile pyramid generation from the streaming strip pipeline (zenpipe#24).
//!
//! [`TilePyramidSink`] consumes full-width strips top to bottom and emits
//! every tile of every pyramid level in that single pass, with RAM bounded
//! by the image *width*, never its height (the libvips `dzsave` shape):
//!
//! 1. Each level keeps a queue of at most `tile_size + 2 × overlap` rows.
//! 2. When the rows a tile row needs have arrived, that tile row is cut
//!    into tiles and handed to the [`TileWriter`]; rows the next tile row
//!    no longer needs are dropped.
//! 3. Every pair of rows is 2×2 box-shrunk into the next level, so one
//!    source strip cascades down to the 1×1 apex.
//!
//! Buffer bytes ≈ `Σ_levels w_level × (tile_size + 2·overlap) × bpp`
//! ≈ `2 × w × (tile_size + 2·overlap) × bpp` (geometric sum) plus one tile
//! scratch — a formula, see [`TilePyramidSink::buffer_bytes_estimate`];
//! measure with heaptrack before quoting a number for a deployment.
//!
//! **This chunk**: Deep Zoom (DZI) level numbering and layout, 2×2 mean
//! shrink (alpha-weighted for RGBA, last row/column replicated for odd
//! sizes), 8-bit-channel formats (1–4 bytes/pixel), sequential tile
//! encoding through a caller-supplied writer, an in-memory writer for
//! tests and a DZI filesystem writer (`std`). Not yet: IIIF / Google Maps /
//! Zoomify layouts, parallel tile encoding, blank-tile skipping, zip
//! output, tiled-TIFF input.
//!
//! # Levels
//!
//! DZI level `n` is the full-resolution image with
//! `n = ceil(log2(max(w, h)))`; level `k` has `ceil(w / 2^(n-k))` ×
//! `ceil(h / 2^(n-k))` pixels; level 0 is 1×1. Tile `(col, row)` of a level
//! covers columns `[col·T − o, (col+1)·T + o)` and the same for rows,
//! clamped to the level — DZI's "overlap on every interior edge".

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use crate::error::{PipeError, PipeResult};
use crate::format::PixelFormat;
use crate::strip::Strip;
use whereat::at;

/// Pyramid geometry: tile size and overlap (DZI defaults 254 / 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilePyramidConfig {
    /// Tile edge in pixels, before overlap (DZI: 254 so tiles are ≤ 256).
    pub tile_size: u32,
    /// Extra pixels on every interior tile edge (DZI: 1).
    pub overlap: u32,
}

impl Default for TilePyramidConfig {
    fn default() -> Self {
        Self {
            tile_size: 254,
            overlap: 1,
        }
    }
}

impl TilePyramidConfig {
    /// `tile_size` × `tile_size` tiles with `overlap` pixels on interior edges.
    pub fn new(tile_size: u32, overlap: u32) -> Self {
        Self { tile_size, overlap }
    }
}

/// One tile handed to a [`TileWriter`]: packed pixels (stride = `width × bpp`).
#[derive(Clone, Copy, Debug)]
pub struct TileRef<'a> {
    /// DZI level number (0 = 1×1 apex, `levels - 1` = full resolution).
    pub level: u32,
    /// Tile column within the level.
    pub col: u32,
    /// Tile row within the level.
    pub row: u32,
    /// Tile width in pixels (edge tiles are smaller; includes overlap).
    pub width: u32,
    /// Tile height in pixels.
    pub height: u32,
    /// Pixel format of `data`.
    pub format: PixelFormat,
    /// Packed pixel bytes, `width × height × bpp`.
    pub data: &'a [u8],
}

/// Dimensions of one pyramid level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelInfo {
    /// DZI level number.
    pub level: u32,
    /// Level width in pixels.
    pub width: u32,
    /// Level height in pixels.
    pub height: u32,
}

/// Receives tiles as the pyramid is generated.
pub trait TileWriter: Send {
    /// Called once before the first tile with the pyramid geometry
    /// (`levels[0]` is the 1×1 apex, the last entry the full image).
    fn begin(
        &mut self,
        _levels: &[LevelInfo],
        _config: TilePyramidConfig,
        _format: PixelFormat,
    ) -> PipeResult<()> {
        Ok(())
    }

    /// Store / encode one tile. Tiles arrive in raster order within a tile
    /// row; tile rows of different levels interleave as the cascade fires.
    fn write_tile(&mut self, tile: TileRef<'_>) -> PipeResult<()>;

    /// Called once after the last tile.
    fn finish(&mut self) -> PipeResult<()> {
        Ok(())
    }
}

/// `(level, col, row)` tile address.
pub type TileKey = (u32, u32, u32);
/// `(width, height, packed pixels)` as stored by [`MemoryTileWriter`].
pub type StoredTile = (u32, u32, Vec<u8>);

/// Collects every tile in memory, keyed `(level, col, row)` — for tests
/// and small pyramids.
#[derive(Default)]
pub struct MemoryTileWriter {
    /// `(level, col, row)` → `(width, height, packed pixels)`.
    pub tiles: alloc::collections::BTreeMap<TileKey, StoredTile>,
    /// Geometry from [`TileWriter::begin`].
    pub levels: Vec<LevelInfo>,
    finished: bool,
}

impl MemoryTileWriter {
    /// Whether [`TileWriter::finish`] ran.
    pub fn finished(&self) -> bool {
        self.finished
    }
}

impl TileWriter for MemoryTileWriter {
    fn begin(
        &mut self,
        levels: &[LevelInfo],
        _config: TilePyramidConfig,
        _format: PixelFormat,
    ) -> PipeResult<()> {
        self.levels = levels.to_vec();
        Ok(())
    }

    fn write_tile(&mut self, tile: TileRef<'_>) -> PipeResult<()> {
        self.tiles.insert(
            (tile.level, tile.col, tile.row),
            (tile.width, tile.height, tile.data.to_vec()),
        );
        Ok(())
    }

    fn finish(&mut self) -> PipeResult<()> {
        self.finished = true;
        Ok(())
    }
}

/// Per-level rolling state.
struct Level {
    info: LevelInfo,
    /// Queued rows (packed), `rows[0]` is absolute row `first_row`.
    rows: VecDeque<Vec<u8>>,
    first_row: u32,
    /// Absolute index of the next row to arrive.
    next_row: u32,
    /// Next tile row to emit.
    next_tile_row: u32,
    /// An odd row waiting for its pair before shrinking into the next level.
    pending: Option<Vec<u8>>,
}

/// Streaming tile pyramid [`Sink`](crate::Sink). See the [module docs](self).
pub struct TilePyramidSink<W: TileWriter> {
    /// `levels[0]` is the full-resolution level; the cascade flows to the end.
    levels: Vec<Level>,
    writer: W,
    config: TilePyramidConfig,
    format: PixelFormat,
    bpp: usize,
    width: u32,
    height: u32,
    rows_consumed: u32,
    tile_scratch: Vec<u8>,
    finished: bool,
}

impl<W: TileWriter> TilePyramidSink<W> {
    /// Build a sink for a `width × height` image in `format` (8-bit
    /// channels, 1–4 bytes per pixel).
    ///
    /// Errors: zero dimensions, `tile_size == 0`, `overlap >= tile_size`,
    /// or a non-8-bit format.
    pub fn new(
        width: u32,
        height: u32,
        format: PixelFormat,
        config: TilePyramidConfig,
        writer: W,
    ) -> PipeResult<Self> {
        if width == 0 || height == 0 {
            return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                "TilePyramidSink: empty image {width}x{height}"
            ))));
        }
        if config.tile_size == 0 || config.overlap >= config.tile_size {
            return Err(at!(PipeError::Op(alloc::format!(
                "TilePyramidSink: tile_size {} must be > overlap {}",
                config.tile_size,
                config.overlap
            ))));
        }
        let bpp = format.bytes_per_pixel();
        if !(1..=4).contains(&bpp) {
            return Err(at!(PipeError::Op(alloc::format!(
                "TilePyramidSink: only 8-bit-channel formats (1-4 bytes/pixel) are supported, got {bpp} bytes/pixel"
            ))));
        }

        // Level dims by repeated ceil-halving down to 1×1; DZI numbers the
        // apex 0 and the full image `n`.
        let mut dims = vec![(width, height)];
        while let Some(&(w, h)) = dims.last() {
            if w == 1 && h == 1 {
                break;
            }
            dims.push((w.div_ceil(2), h.div_ceil(2)));
        }
        let n = dims.len() as u32 - 1;
        let levels = dims
            .iter()
            .enumerate()
            .map(|(i, &(w, h))| Level {
                info: LevelInfo {
                    level: n - i as u32,
                    width: w,
                    height: h,
                },
                rows: VecDeque::new(),
                first_row: 0,
                next_row: 0,
                next_tile_row: 0,
                pending: None,
            })
            .collect();

        let edge = config.tile_size as usize + 2 * config.overlap as usize;
        let tile_scratch =
            vec![0u8; crate::limits::checked_buffer_size(edge as u32, edge as u32, bpp)?];

        Ok(Self {
            levels,
            writer,
            config,
            format,
            bpp,
            width,
            height,
            rows_consumed: 0,
            tile_scratch,
            finished: false,
        })
    }

    /// Level geometry, apex first (DZI order).
    pub fn level_infos(&self) -> Vec<LevelInfo> {
        self.levels.iter().rev().map(|l| l.info).collect()
    }

    /// Number of DZI levels (`ceil(log2(max(w, h))) + 1`).
    pub fn level_count(&self) -> u32 {
        self.levels.len() as u32
    }

    /// Upper bound on the bytes this sink keeps queued, from the formula in
    /// the [module docs](self) (row queues + pending rows + tile scratch).
    /// A formula, not a measurement.
    pub fn buffer_bytes_estimate(&self) -> u64 {
        let rows = u64::from(self.config.tile_size) + 2 * u64::from(self.config.overlap) + 1;
        let queues: u64 = self
            .levels
            .iter()
            .map(|l| u64::from(l.info.width) * rows * self.bpp as u64)
            .sum();
        queues + self.tile_scratch.len() as u64
    }

    /// Borrow the writer (e.g. to read back an in-memory writer's tiles).
    pub fn writer(&self) -> &W {
        &self.writer
    }

    /// Take the writer out after [`finish`](crate::Sink::finish).
    pub fn into_writer(self) -> W {
        self.writer
    }

    /// Push one packed row into level `li`, emit any tile rows that
    /// became complete, and cascade the shrink into level `li + 1`.
    fn push_row(&mut self, li: usize, row: Vec<u8>) -> PipeResult<()> {
        let bpp = self.bpp;
        let has_next = li + 1 < self.levels.len();
        let alpha = self.format.has_alpha();
        let shrunk = {
            let level = &mut self.levels[li];
            let lw = level.info.width;
            debug_assert_eq!(row.len(), lw as usize * bpp);
            if level.next_row >= level.info.height {
                return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                    "TilePyramidSink: level {} received more than {} rows",
                    level.info.level,
                    level.info.height
                ))));
            }
            // Shrink pairing for the next level (before the row moves).
            let shrunk = if has_next {
                match level.pending.take() {
                    None => {
                        level.pending = Some(row.clone());
                        None
                    }
                    Some(prev) => Some(shrink_rows(&prev, &row, lw, bpp, alpha)),
                }
            } else {
                None
            };
            level.rows.push_back(row);
            level.next_row += 1;
            shrunk
        };

        self.emit_ready_tile_rows(li)?;

        if let Some(s) = shrunk {
            self.push_row(li + 1, s)?;
        }
        Ok(())
    }

    /// Emit every tile row of level `li` whose rows have all arrived.
    fn emit_ready_tile_rows(&mut self, li: usize) -> PipeResult<()> {
        let t = self.config.tile_size;
        let o = self.config.overlap;
        loop {
            let level = &self.levels[li];
            let lh = level.info.height;
            let r = level.next_tile_row;
            if r.saturating_mul(t) >= lh {
                break;
            }
            let row_end = (r + 1).saturating_mul(t).saturating_add(o).min(lh);
            if level.next_row < row_end {
                break;
            }
            self.emit_tile_row(li, r)?;
            // Drop rows the next tile row can't reference.
            let keep_from = (r + 1).saturating_mul(t).saturating_sub(o);
            let level = &mut self.levels[li];
            while level.first_row < keep_from && !level.rows.is_empty() {
                level.rows.pop_front();
                level.first_row += 1;
            }
            level.next_tile_row += 1;
        }
        Ok(())
    }

    /// Cut tile row `r` of level `li` into tiles and hand them to the writer.
    fn emit_tile_row(&mut self, li: usize, r: u32) -> PipeResult<()> {
        let t = self.config.tile_size;
        let o = self.config.overlap;
        let bpp = self.bpp;
        let (lw, lh, lvl) = {
            let l = &self.levels[li].info;
            (l.width, l.height, l.level)
        };
        let y0 = r.saturating_mul(t).saturating_sub(o);
        let y1 = (r + 1).saturating_mul(t).saturating_add(o).min(lh);
        let th = y1 - y0;
        let cols = lw.div_ceil(t);
        for c in 0..cols {
            let x0 = c.saturating_mul(t).saturating_sub(o);
            let x1 = (c + 1).saturating_mul(t).saturating_add(o).min(lw);
            let tw = x1 - x0;
            let row_bytes = tw as usize * bpp;
            let mut scratch = core::mem::take(&mut self.tile_scratch);
            {
                let level = &self.levels[li];
                for (i, y) in (y0..y1).enumerate() {
                    let src = &level.rows[(y - level.first_row) as usize];
                    let s = x0 as usize * bpp;
                    scratch[i * row_bytes..(i + 1) * row_bytes]
                        .copy_from_slice(&src[s..s + row_bytes]);
                }
            }
            let written = self.writer.write_tile(TileRef {
                level: lvl,
                col: c,
                row: r,
                width: tw,
                height: th,
                format: self.format,
                data: &scratch[..th as usize * row_bytes],
            });
            self.tile_scratch = scratch;
            written?;
        }
        Ok(())
    }
}

/// 2×2 box shrink of two adjacent rows into one row of `ceil(w / 2)`
/// pixels. Odd widths replicate the last column. RGBA with alpha is
/// averaged alpha-weighted (color premultiplied, alpha plain mean) so
/// transparent pixels don't bleed their color; other layouts are a plain
/// per-channel rounded mean.
fn shrink_rows(a: &[u8], b: &[u8], w: u32, bpp: usize, alpha: bool) -> Vec<u8> {
    let out_w = w.div_ceil(2) as usize;
    let mut out = vec![0u8; out_w * bpp];
    let last = w as usize - 1;
    for x in 0..out_w {
        let x0 = 2 * x;
        let x1 = (2 * x + 1).min(last);
        let p = [
            &a[x0 * bpp..x0 * bpp + bpp],
            &a[x1 * bpp..x1 * bpp + bpp],
            &b[x0 * bpp..x0 * bpp + bpp],
            &b[x1 * bpp..x1 * bpp + bpp],
        ];
        let dst = &mut out[x * bpp..x * bpp + bpp];
        if alpha && bpp == 4 {
            let a_sum: u32 = p.iter().map(|q| u32::from(q[3])).sum();
            for c in 0..3 {
                let num: u32 = p.iter().map(|q| u32::from(q[c]) * u32::from(q[3])).sum();
                dst[c] = (num + a_sum / 2).checked_div(a_sum).unwrap_or(0) as u8;
            }
            dst[3] = ((a_sum + 2) / 4) as u8;
        } else {
            for c in 0..bpp {
                let sum: u32 = p.iter().map(|q| u32::from(q[c])).sum();
                dst[c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    out
}

impl<W: TileWriter> crate::Sink for TilePyramidSink<W> {
    fn consume(&mut self, strip: &Strip<'_>) -> PipeResult<()> {
        if self.finished {
            return Err(at!(PipeError::Op(alloc::string::String::from(
                "TilePyramidSink: consume after finish"
            ))));
        }
        if strip.width() != self.width || strip.descriptor() != self.format {
            return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                "TilePyramidSink: strip {}x? {:?} does not match {}x{} {:?}",
                strip.width(),
                strip.descriptor(),
                self.width,
                self.height,
                self.format
            ))));
        }
        if self.rows_consumed == 0 && strip.rows() > 0 {
            let infos = self.level_infos();
            self.writer.begin(&infos, self.config, self.format)?;
        }
        let row_bytes = self.width as usize * self.bpp;
        for r in 0..strip.rows() {
            if self.rows_consumed >= self.height {
                return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                    "TilePyramidSink: more than {} rows received",
                    self.height
                ))));
            }
            let row = strip.row(r)[..row_bytes].to_vec();
            self.push_row(0, row)?;
            self.rows_consumed += 1;
        }
        Ok(())
    }

    fn finish(&mut self) -> PipeResult<()> {
        if self.finished {
            return Ok(());
        }
        if self.rows_consumed != self.height {
            return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                "TilePyramidSink: finished after {} of {} rows",
                self.rows_consumed,
                self.height
            ))));
        }
        // Odd heights: the last row pairs with itself, level by level (each
        // flush may leave a pending row one level down).
        for li in 0..self.levels.len() {
            if let Some(prev) = self.levels[li].pending.take() {
                let lw = self.levels[li].info.width;
                let s = shrink_rows(&prev, &prev, lw, self.bpp, self.format.has_alpha());
                self.push_row(li + 1, s)?;
            }
        }
        // Every level must have emitted all its tile rows.
        for l in &self.levels {
            let expected = l.info.height.div_ceil(self.config.tile_size);
            if l.next_tile_row != expected || l.next_row != l.info.height {
                return Err(at!(PipeError::Op(alloc::format!(
                    "TilePyramidSink: level {} incomplete ({} of {} rows, {} of {} tile rows)",
                    l.info.level,
                    l.next_row,
                    l.info.height,
                    l.next_tile_row,
                    expected
                ))));
            }
        }
        self.finished = true;
        self.writer.finish()
    }
}

/// Per-tile encoder used by [`DziFsWriter`]: tile in, encoded bytes out.
#[cfg(feature = "std")]
pub type TileEncoder = Box<dyn FnMut(TileRef<'_>) -> PipeResult<Vec<u8>> + Send>;

/// Deep Zoom filesystem layout: `{dir}/{name}.dzi` + `{dir}/{name}_files/
/// {level}/{col}_{row}.{ext}`, tiles encoded by a caller-supplied closure
/// (e.g. a `zencodecs::EncodeRequest` per tile).
#[cfg(feature = "std")]
pub struct DziFsWriter {
    dir: std::path::PathBuf,
    name: alloc::string::String,
    ext: alloc::string::String,
    encode: TileEncoder,
    tiles_written: u64,
}

#[cfg(feature = "std")]
impl DziFsWriter {
    /// Write `{name}.dzi` and `{name}_files/` under `dir`; `ext` is the tile
    /// extension (`jpeg`, `png`, `webp`) recorded in the descriptor;
    /// `encode` turns a tile into that format's bytes.
    pub fn new(
        dir: impl Into<std::path::PathBuf>,
        name: impl Into<alloc::string::String>,
        ext: impl Into<alloc::string::String>,
        encode: impl FnMut(TileRef<'_>) -> PipeResult<Vec<u8>> + Send + 'static,
    ) -> Self {
        Self {
            dir: dir.into(),
            name: name.into(),
            ext: ext.into(),
            encode: Box::new(encode),
            tiles_written: 0,
        }
    }

    /// Tiles written so far.
    pub fn tiles_written(&self) -> u64 {
        self.tiles_written
    }

    /// Path of the `.dzi` descriptor.
    pub fn descriptor_path(&self) -> std::path::PathBuf {
        self.dir.join(alloc::format!("{}.dzi", self.name))
    }

    fn io(e: std::io::Error, what: &str) -> whereat::At<PipeError> {
        at!(PipeError::Op(alloc::format!("DziFsWriter: {what}: {e}")))
    }
}

#[cfg(feature = "std")]
impl TileWriter for DziFsWriter {
    fn begin(
        &mut self,
        levels: &[LevelInfo],
        config: TilePyramidConfig,
        _format: PixelFormat,
    ) -> PipeResult<()> {
        let full = levels.last().ok_or_else(|| {
            at!(PipeError::Op(alloc::string::String::from(
                "DziFsWriter: no levels"
            )))
        })?;
        let files = self.dir.join(alloc::format!("{}_files", self.name));
        std::fs::create_dir_all(&files).map_err(|e| Self::io(e, "create _files dir"))?;
        for l in levels {
            std::fs::create_dir_all(files.join(l.level.to_string()))
                .map_err(|e| Self::io(e, "create level dir"))?;
        }
        let xml = alloc::format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Image xmlns=\"http://schemas.microsoft.com/deepzoom/2008\" Format=\"{}\" Overlap=\"{}\" TileSize=\"{}\">\n  <Size Height=\"{}\" Width=\"{}\"/>\n</Image>\n",
            self.ext,
            config.overlap,
            config.tile_size,
            full.height,
            full.width
        );
        std::fs::write(self.descriptor_path(), xml).map_err(|e| Self::io(e, "write .dzi"))
    }

    fn write_tile(&mut self, tile: TileRef<'_>) -> PipeResult<()> {
        let bytes = (self.encode)(tile)?;
        let path = self
            .dir
            .join(alloc::format!("{}_files", self.name))
            .join(tile.level.to_string())
            .join(alloc::format!("{}_{}.{}", tile.col, tile.row, self.ext));
        std::fs::write(&path, bytes).map_err(|e| Self::io(e, "write tile"))?;
        self.tiles_written += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sink;
    use crate::strip::StripBuf;

    /// Deterministic RGBA image with varying alpha.
    fn image(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[
                    (x * 7) as u8,
                    (y * 13) as u8,
                    ((x ^ y) * 3) as u8,
                    if (x / 5 + y / 7) % 3 == 0 {
                        0
                    } else {
                        255 - ((x + y) % 40) as u8
                    },
                ]);
            }
        }
        v
    }

    /// Reference: full-image shrink chain.
    fn reference_levels(mut img: Vec<u8>, mut w: u32, mut h: u32) -> Vec<StoredTile> {
        let mut out = vec![(w, h, img.clone())];
        while !(w == 1 && h == 1) {
            let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
            let mut next = Vec::with_capacity((nw * nh * 4) as usize);
            for y in 0..nh {
                let y0 = (2 * y) as usize;
                let y1 = ((2 * y + 1).min(h - 1)) as usize;
                let a = &img[y0 * w as usize * 4..(y0 + 1) * w as usize * 4];
                let b = &img[y1 * w as usize * 4..(y1 + 1) * w as usize * 4];
                next.extend_from_slice(&shrink_rows(a, b, w, 4, true));
            }
            img = next;
            w = nw;
            h = nh;
            out.push((w, h, img.clone()));
        }
        out
    }

    fn run(w: u32, h: u32, cfg: TilePyramidConfig, strip_rows: u32) -> (MemoryTileWriter, Vec<u8>) {
        let img = image(w, h);
        let mut sink = TilePyramidSink::new(
            w,
            h,
            crate::format::RGBA8_SRGB,
            cfg,
            MemoryTileWriter::default(),
        )
        .unwrap();
        let mut y = 0;
        while y < h {
            let rows = strip_rows.min(h - y);
            let mut buf = StripBuf::new(w, rows, crate::format::RGBA8_SRGB);
            for r in 0..rows {
                let s = ((y + r) * w * 4) as usize;
                buf.push_row(&img[s..s + (w * 4) as usize]);
            }
            sink.consume(&buf.as_strip()).unwrap();
            y += rows;
        }
        sink.finish().unwrap();
        (sink.into_writer(), img)
    }

    fn check(w: u32, h: u32, cfg: TilePyramidConfig, strip_rows: u32) {
        let (writer, img) = run(w, h, cfg, strip_rows);
        assert!(writer.finished());
        let levels = reference_levels(img, w, h);
        let n = levels.len() as u32 - 1;
        assert_eq!(writer.levels.len(), levels.len());
        assert_eq!(
            writer.levels[0],
            LevelInfo {
                level: 0,
                width: 1,
                height: 1
            }
        );
        let (t, o) = (cfg.tile_size, cfg.overlap);
        let mut seen = 0;
        for (i, (lw, lh, px)) in levels.iter().enumerate() {
            let level = n - i as u32;
            let (cols, rows) = (lw.div_ceil(t), lh.div_ceil(t));
            for c in 0..cols {
                for r in 0..rows {
                    let (tw, th, data) = writer.tiles.get(&(level, c, r)).unwrap_or_else(|| {
                        panic!("missing tile L{level} ({c},{r}) for {w}x{h} {cfg:?}")
                    });
                    let x0 = (c * t).saturating_sub(o);
                    let x1 = ((c + 1) * t + o).min(*lw);
                    let y0 = (r * t).saturating_sub(o);
                    let y1 = ((r + 1) * t + o).min(*lh);
                    assert_eq!((*tw, *th), (x1 - x0, y1 - y0), "L{level} ({c},{r}) dims");
                    for yy in y0..y1 {
                        let src = &px[((yy * lw + x0) * 4) as usize..((yy * lw + x1) * 4) as usize];
                        let got =
                            &data[((yy - y0) * tw * 4) as usize..((yy - y0 + 1) * tw * 4) as usize];
                        assert_eq!(got, src, "L{level} ({c},{r}) row {yy} for {w}x{h} {cfg:?}");
                    }
                    seen += 1;
                }
            }
        }
        assert_eq!(seen, writer.tiles.len(), "no extra tiles");
    }

    #[test]
    fn dzi_pyramid_matches_full_image_reference() {
        check(300, 200, TilePyramidConfig::new(64, 1), 3);
        check(300, 200, TilePyramidConfig::new(64, 0), 16);
        check(257, 129, TilePyramidConfig::new(7, 2), 5);
        check(1, 1, TilePyramidConfig::default(), 1);
        check(2, 3, TilePyramidConfig::new(2, 1), 1);
        check(1000, 40, TilePyramidConfig::new(254, 1), 11);
    }

    #[test]
    fn level_numbering_is_deep_zoom() {
        let sink = TilePyramidSink::new(
            1000,
            40,
            crate::format::RGBA8_SRGB,
            TilePyramidConfig::default(),
            MemoryTileWriter::default(),
        )
        .unwrap();
        // ceil(log2(1000)) = 10 → 11 levels, full image is level 10.
        assert_eq!(sink.level_count(), 11);
        let infos = sink.level_infos();
        assert_eq!(
            infos[10],
            LevelInfo {
                level: 10,
                width: 1000,
                height: 40
            }
        );
        assert_eq!(
            infos[9],
            LevelInfo {
                level: 9,
                width: 500,
                height: 20
            }
        );
        assert_eq!(
            infos[0],
            LevelInfo {
                level: 0,
                width: 1,
                height: 1
            }
        );
        assert!(sink.buffer_bytes_estimate() > 0);
    }

    #[test]
    fn shrink_is_alpha_weighted_and_replicates_odd_columns() {
        // Transparent red must not tint an opaque blue neighbour.
        let a = [255, 0, 0, 0, 0, 0, 255, 255, 9, 9, 9, 255];
        let b = [255, 0, 0, 0, 0, 0, 255, 255, 9, 9, 9, 255];
        let out = shrink_rows(&a, &b, 3, 4, true);
        assert_eq!(&out[..4], &[0, 0, 255, 128]);
        // Odd width: last column pairs with itself.
        assert_eq!(&out[4..8], &[9, 9, 9, 255]);
        // Plain mean without alpha.
        let out = shrink_rows(&[10, 20, 30, 40], &[50, 60, 70, 80], 2, 2, false);
        assert_eq!(
            out,
            vec![(10 + 30 + 50 + 70 + 2) / 4, (20 + 40 + 60 + 80 + 2) / 4]
        );
    }

    #[test]
    fn rejects_bad_geometry_and_row_counts() {
        let f = crate::format::RGBA8_SRGB;
        assert!(
            TilePyramidSink::new(
                0,
                5,
                f,
                TilePyramidConfig::default(),
                MemoryTileWriter::default()
            )
            .is_err()
        );
        assert!(
            TilePyramidSink::new(
                5,
                5,
                f,
                TilePyramidConfig::new(4, 4),
                MemoryTileWriter::default()
            )
            .is_err()
        );
        assert!(
            TilePyramidSink::new(
                5,
                5,
                crate::format::RGBAF32_LINEAR,
                TilePyramidConfig::default(),
                MemoryTileWriter::default()
            )
            .is_err()
        );
        let mut sink = TilePyramidSink::new(
            4,
            4,
            f,
            TilePyramidConfig::new(2, 0),
            MemoryTileWriter::default(),
        )
        .unwrap();
        assert!(sink.finish().is_err(), "finish before all rows");
    }
}
