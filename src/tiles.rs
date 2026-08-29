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
//!    source strip cascades down to the apex.
//!
//! Buffer bytes ≈ `Σ_levels w_level × (tile_size + 2·overlap) × bpp`
//! ≈ `2 × w × (tile_size + 2·overlap) × bpp` (geometric sum) plus one
//! tile-row scratch — a formula, see
//! [`TilePyramidSink::buffer_bytes_estimate`].
//!
//! Measured (2026-08-28, `examples/tile_pyramid_mem.rs`, release, Apple
//! M4 Pro / macOS 26.5, `/usr/bin/time -l` maximum resident set size,
//! RGBA8, DZI 254/1, rows generated on the fly so the source holds no
//! frame; the runtime baseline at 256×16 is 1.8 MB):
//!
//! | image         | levels | tiles | max RSS  | formula  |
//! |---------------|--------|-------|----------|----------|
//! | 10 000 × 1000 |   15   |   229 |  38.3 MB |  31.1 MB |
//! | 40 000 × 1000 |   17   |   879 | 124.8 MB | 123.7 MB |
//! | 100 000 × 600 |   18   |  1785 | 298.1 MB | 308.9 MB |
//!
//! RSS does not depend on the height (the sink never holds more than
//! `tile_size + 2·overlap + 1` rows per level), so a gigapixel 100 000 px
//! wide image stays under 300 MB of sink buffers. Re-measure on your
//! platform before quoting a number for a deployment.
//!
//! # Layouts and stores (`std`)
//!
//! [`PyramidWriter`] is the [`TileWriter`] that turns tiles into files: a
//! [`TileLayout`] names them ([`DziLayout`], [`Iiif3Layout`],
//! [`GoogleMapsLayout`], [`ZoomifyLayout`]) and writes the descriptor
//! (`.dzi`, `info.json`, `ImageProperties.xml`), a [`TileStore`] persists
//! them ([`FsStore`], [`ZipStore`], [`MemoryStore`]), and a caller-supplied
//! encoder turns pixels into bytes (any `zencodecs::EncodeRequest`; the
//! sink has no codec dependency). Tile rows are encoded in parallel with
//! [`PyramidWriter::with_threads`] and near-background tiles skipped with
//! [`PyramidWriter::with_skip_blanks`].
//!
//! Each layout needs a matching [`PyramidGeometry`]: DZI and IIIF halve to
//! 1×1 ([`TilePyramidConfig::dzi`] / [`iiif`](TilePyramidConfig::iiif)),
//! Zoomify stops at the first level that fits one tile
//! ([`zoomify`](TilePyramidConfig::zoomify)), Google Maps pads the image
//! into a `tile × 2^k` square and stops at one tile
//! ([`google_maps`](TilePyramidConfig::google_maps)). [`PyramidWriter`]
//! rejects a mismatched pairing in [`TileWriter::begin`].
//!
//! Not yet: tiled-TIFF / mmap input, temp-file materialization for
//! analysis barriers, column-parallel execution, PMTiles.
//!
//! # Levels
//!
//! Level numbers count up from the apex: level 0 is the smallest level
//! (1×1 for DZI/IIIF, the one-tile level for Zoomify/Google), `levels - 1`
//! is the full-resolution image; level `k` has `ceil(w / 2^(n-k))` ×
//! `ceil(h / 2^(n-k))` pixels. Tile `(col, row)` of a level covers columns
//! `[col·T − o, (col+1)·T + o)` and the same for rows, clamped to the
//! level — DZI's "overlap on every interior edge".

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::error::{PipeError, PipeResult};
use crate::format::PixelFormat;
use crate::strip::Strip;
use whereat::at;

/// How far the pyramid goes and whether the canvas is padded.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyramidGeometry {
    /// Ceil-halve until the apex is 1×1 (Deep Zoom, IIIF).
    ToOnePixel,
    /// Ceil-halve until both dimensions fit in one tile (Zoomify).
    ToOneTile,
    /// Pad the image (top-left aligned, `background` fill, `bpp` bytes of
    /// it used) into a `tile_size × 2^k` square that contains it, then halve
    /// until one tile (Google Maps XYZ: every tile is complete).
    PaddedSquare {
        /// Fill bytes for the padding, one per channel (first `bpp` used).
        background: [u8; 4],
    },
}

/// Pyramid geometry: tile size, overlap and level policy (DZI defaults
/// 254 / 1 / to 1×1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilePyramidConfig {
    /// Tile edge in pixels, before overlap (DZI: 254 so tiles are ≤ 256).
    pub tile_size: u32,
    /// Extra pixels on every interior tile edge (DZI: 1).
    pub overlap: u32,
    /// Level policy.
    pub geometry: PyramidGeometry,
}

impl Default for TilePyramidConfig {
    fn default() -> Self {
        Self::dzi()
    }
}

impl TilePyramidConfig {
    /// `tile_size` × `tile_size` tiles with `overlap` pixels on interior
    /// edges, halving to 1×1.
    pub fn new(tile_size: u32, overlap: u32) -> Self {
        Self {
            tile_size,
            overlap,
            geometry: PyramidGeometry::ToOnePixel,
        }
    }

    /// Deep Zoom defaults: 254 px tiles, 1 px overlap, levels to 1×1.
    pub fn dzi() -> Self {
        Self::new(254, 1)
    }

    /// IIIF Image API 3.0 static tiles: 512 px, no overlap, levels to 1×1.
    pub fn iiif() -> Self {
        Self::new(512, 0)
    }

    /// Zoomify: 256 px, no overlap, levels down to the first one-tile level.
    pub fn zoomify() -> Self {
        Self {
            tile_size: 256,
            overlap: 0,
            geometry: PyramidGeometry::ToOneTile,
        }
    }

    /// Google Maps XYZ: 256 px, no overlap, image padded into a
    /// `256 × 2^k` square with `background`, levels down to one tile.
    pub fn google_maps(background: [u8; 4]) -> Self {
        Self {
            tile_size: 256,
            overlap: 0,
            geometry: PyramidGeometry::PaddedSquare { background },
        }
    }

    /// Override the level policy.
    pub fn with_geometry(mut self, geometry: PyramidGeometry) -> Self {
        self.geometry = geometry;
        self
    }
}

/// One tile handed to a [`TileWriter`]: packed pixels (stride = `width × bpp`).
#[derive(Clone, Copy, Debug)]
pub struct TileRef<'a> {
    /// Level number (0 = apex, `levels - 1` = full resolution).
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

impl TileRef<'_> {
    /// Whether every channel of every pixel is within `threshold` of
    /// `background` (the first `bpp` bytes are compared) — a tile a viewer
    /// can substitute with a flat fill.
    pub fn is_blank(&self, background: &[u8], threshold: u8) -> bool {
        let bpp = self.format.bytes_per_pixel();
        if background.len() < bpp {
            return false;
        }
        let bg = &background[..bpp];
        self.data
            .chunks_exact(bpp)
            .all(|px| px.iter().zip(bg).all(|(&a, &b)| a.abs_diff(b) <= threshold))
    }
}

/// Dimensions of one pyramid level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelInfo {
    /// Level number (0 = apex).
    pub level: u32,
    /// Level width in pixels.
    pub width: u32,
    /// Level height in pixels.
    pub height: u32,
}

impl LevelInfo {
    /// Tile columns × rows at `tile_size`.
    pub fn tile_grid(&self, tile_size: u32) -> (u32, u32) {
        (
            self.width.div_ceil(tile_size.max(1)),
            self.height.div_ceil(tile_size.max(1)),
        )
    }
}

/// Receives tiles as the pyramid is generated.
pub trait TileWriter: Send {
    /// Called once before the first tile with the pyramid geometry
    /// (`levels[0]` is the apex, the last entry the full image).
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

    /// Store / encode one complete tile row (all columns of one `(level,
    /// row)`, left to right). The default forwards to
    /// [`write_tile`](Self::write_tile); writers that encode tiles in
    /// parallel override this.
    fn write_tile_row(&mut self, tiles: &[TileRef<'_>]) -> PipeResult<()> {
        for t in tiles {
            self.write_tile(*t)?;
        }
        Ok(())
    }

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
}

/// Streaming tile pyramid [`Sink`](crate::Sink). See the [module docs](self).
pub struct TilePyramidSink<W: TileWriter> {
    /// `levels[0]` is the full-resolution level; the cascade flows to the end.
    levels: Vec<Level>,
    writer: W,
    config: TilePyramidConfig,
    format: PixelFormat,
    bpp: usize,
    /// Source image size.
    width: u32,
    height: u32,
    /// Canvas size (== image size unless the geometry pads).
    canvas_w: u32,
    canvas_h: u32,
    rows_consumed: u32,
    /// Scratch for one complete tile row: `cols` tiles of the largest size.
    row_scratch: Vec<u8>,
    finished: bool,
}

/// Level dimensions, full resolution first, for `(w, h)` under `geometry`.
fn level_dims(w: u32, h: u32, tile: u32, geometry: PyramidGeometry) -> Vec<(u32, u32)> {
    let mut dims = vec![(w, h)];
    loop {
        let &(cw, ch) = dims.last().unwrap();
        let done = match geometry {
            PyramidGeometry::ToOnePixel => cw == 1 && ch == 1,
            PyramidGeometry::ToOneTile | PyramidGeometry::PaddedSquare { .. } => {
                cw <= tile && ch <= tile
            }
        };
        if done {
            break;
        }
        dims.push((cw.div_ceil(2), ch.div_ceil(2)));
    }
    dims
}

impl<W: TileWriter> TilePyramidSink<W> {
    /// Build a sink for a `width × height` image in `format` (8-bit
    /// channels, 1–4 bytes per pixel).
    ///
    /// Errors: zero dimensions, `tile_size == 0`, `overlap >= tile_size`,
    /// a non-8-bit format, or a padded canvas that would overflow `u32`.
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

        let (canvas_w, canvas_h) = match config.geometry {
            PyramidGeometry::PaddedSquare { .. } => {
                let mut s = config.tile_size;
                while s < width.max(height) {
                    s = s.checked_mul(2).ok_or_else(|| {
                        at!(PipeError::LimitExceeded(alloc::format!(
                            "TilePyramidSink: padded square for {width}x{height} overflows u32"
                        )))
                    })?;
                }
                (s, s)
            }
            _ => (width, height),
        };

        let dims = level_dims(canvas_w, canvas_h, config.tile_size, config.geometry);
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
            })
            .collect();

        let edge = config.tile_size as usize + 2 * config.overlap as usize;
        let cols = canvas_w.div_ceil(config.tile_size) as usize;
        let tile_bytes = crate::limits::checked_buffer_size(edge as u32, edge as u32, bpp)?;
        let row_scratch = vec![
            0u8;
            tile_bytes.checked_mul(cols).ok_or_else(|| at!(
                PipeError::LimitExceeded(String::from(
                    "TilePyramidSink: tile-row scratch overflows usize"
                ))
            ))?
        ];

        Ok(Self {
            levels,
            writer,
            config,
            format,
            bpp,
            width,
            height,
            canvas_w,
            canvas_h,
            rows_consumed: 0,
            row_scratch,
            finished: false,
        })
    }

    /// Level geometry, apex first.
    pub fn level_infos(&self) -> Vec<LevelInfo> {
        self.levels.iter().rev().map(|l| l.info).collect()
    }

    /// Number of levels.
    pub fn level_count(&self) -> u32 {
        self.levels.len() as u32
    }

    /// Canvas size the pyramid is built from — the image size, or the
    /// padded square for [`PyramidGeometry::PaddedSquare`].
    pub fn canvas_size(&self) -> (u32, u32) {
        (self.canvas_w, self.canvas_h)
    }

    /// Upper bound on the bytes this sink keeps queued, from the formula in
    /// the [module docs](self) (row queues + pending rows + tile-row
    /// scratch). A formula, not a measurement.
    pub fn buffer_bytes_estimate(&self) -> u64 {
        let rows = u64::from(self.config.tile_size) + 2 * u64::from(self.config.overlap) + 1;
        let queues: u64 = self
            .levels
            .iter()
            .map(|l| u64::from(l.info.width) * rows * self.bpp as u64)
            .sum();
        queues + self.row_scratch.len() as u64
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
        let lw = self.levels[li].info.width;
        debug_assert_eq!(row.len(), lw as usize * bpp);
        {
            let level = &self.levels[li];
            if level.next_row >= level.info.height {
                return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                    "TilePyramidSink: level {} received more than {} rows",
                    level.info.level,
                    level.info.height
                ))));
            }
        }
        let idx = self.levels[li].next_row;
        // Rows pair (0,1), (2,3), … into the next level: the odd row of a pair
        // shrinks with its predecessor, read straight out of the queue.
        // `emit_ready_tile_rows` never drops the newest row, which is what
        // makes that predecessor still be there — so no copy of every even row
        // is needed (it used to cost one full-width allocation + memcpy per
        // even row per level).
        let mut shrunk = if has_next && !idx.is_multiple_of(2) {
            Some(vec![0u8; lw.div_ceil(2) as usize * bpp])
        } else {
            None
        };
        {
            let level = &mut self.levels[li];
            let base = level.first_row;
            level.rows.push_back(row);
            level.next_row += 1;
            if let Some(out) = shrunk.as_mut() {
                debug_assert!(idx > base, "shrink pair dropped: {idx} <= {base}");
                let prev = &level.rows[(idx - 1 - base) as usize];
                let cur = &level.rows[(idx - base) as usize];
                shrink_rows_into(out, prev, cur, lw, bpp, alpha);
            }
        }

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
            // Drop rows the next tile row can't reference — but never the
            // newest one: `push_row` pairs it with the row that follows to
            // shrink into the next level without copying it (and `finish`
            // pairs it with itself when the level's height is odd).
            let level = &mut self.levels[li];
            let keep_from = (r + 1)
                .saturating_mul(t)
                .saturating_sub(o)
                .min(level.next_row.saturating_sub(1));
            while level.first_row < keep_from && !level.rows.is_empty() {
                level.rows.pop_front();
                level.first_row += 1;
            }
            level.next_tile_row += 1;
        }
        Ok(())
    }

    /// Cut tile row `r` of level `li` into tiles and hand the whole row to
    /// the writer.
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
        let mut scratch = core::mem::take(&mut self.row_scratch);
        // Pack every tile of the row back to back in the scratch.
        let mut spans: Vec<(u32, u32, usize, usize)> = Vec::with_capacity(cols as usize);
        let mut pos = 0usize;
        {
            let level = &self.levels[li];
            for c in 0..cols {
                let x0 = c.saturating_mul(t).saturating_sub(o);
                let x1 = (c + 1).saturating_mul(t).saturating_add(o).min(lw);
                let tw = x1 - x0;
                let row_bytes = tw as usize * bpp;
                let start = pos;
                for y in y0..y1 {
                    let src = &level.rows[(y - level.first_row) as usize];
                    let s = x0 as usize * bpp;
                    scratch[pos..pos + row_bytes].copy_from_slice(&src[s..s + row_bytes]);
                    pos += row_bytes;
                }
                spans.push((c, tw, start, pos));
            }
        }
        let tiles: Vec<TileRef<'_>> = spans
            .iter()
            .map(|&(c, tw, start, end)| TileRef {
                level: lvl,
                col: c,
                row: r,
                width: tw,
                height: th,
                format: self.format,
                data: &scratch[start..end],
            })
            .collect();
        let written = self.writer.write_tile_row(&tiles);
        drop(tiles);
        self.row_scratch = scratch;
        written
    }

    /// A packed canvas row for image row `y` (padded on the right when the
    /// geometry pads).
    fn canvas_row(&self, src: &[u8]) -> Vec<u8> {
        let bpp = self.bpp;
        let img = self.width as usize * bpp;
        if self.canvas_w == self.width {
            // `to_vec` allocates without zeroing first — the copy is the
            // only pass over the bytes.
            return src[..img].to_vec();
        }
        let mut row = self.background_row();
        row[..img].copy_from_slice(&src[..img]);
        row
    }

    fn background_row(&self) -> Vec<u8> {
        let bpp = self.bpp;
        let bg = match self.config.geometry {
            PyramidGeometry::PaddedSquare { background } => background,
            _ => [0; 4],
        };
        let mut row = vec![0u8; self.canvas_w as usize * bpp];
        for px in row.chunks_exact_mut(bpp) {
            px.copy_from_slice(&bg[..bpp]);
        }
        row
    }
}

/// 2×2 box shrink of two adjacent rows into one row of `ceil(w / 2)`
/// pixels. Odd widths replicate the last column. RGBA with alpha is
/// averaged alpha-weighted (color premultiplied, alpha plain mean) so
/// transparent pixels don't bleed their color; other layouts are a plain
/// per-channel rounded mean.
fn shrink_rows_into(out: &mut [u8], a: &[u8], b: &[u8], w: u32, bpp: usize, alpha: bool) {
    let w = w as usize;
    let out_w = w.div_ceil(2);
    debug_assert_eq!(out.len(), out_w * bpp);
    // Output pixels backed by a real 2-wide pair. Only an odd `w` leaves a
    // tail pixel, which replicates the last column.
    let full = w / 2;
    if full > 0 {
        let (head, tail_a, tail_b) = (
            &mut out[..full * bpp],
            &a[..full * 2 * bpp],
            &b[..full * 2 * bpp],
        );
        match (alpha, bpp) {
            // Fully opaque rows take the plain path: it is *bit-identical*
            // there (with every alpha 255, `a_sum` is 1020 and
            // `(255·S + 510) / 1020` reduces exactly to `(S + 2) / 4`), and it
            // skips three integer divides per output pixel — the alpha-
            // weighted path's dominant cost. One linear scan of two rows buys
            // that for the common case; see the equivalence test.
            (true, 4) if rows_are_opaque(tail_a, tail_b) => {
                shrink_pairs_plain::<4>(head, tail_a, tail_b);
            }
            (true, 4) => shrink_pairs_rgba(head, tail_a, tail_b),
            (_, 1) => shrink_pairs_plain::<1>(head, tail_a, tail_b),
            (_, 2) => shrink_pairs_plain::<2>(head, tail_a, tail_b),
            (_, 3) => shrink_pairs_plain::<3>(head, tail_a, tail_b),
            (_, 4) => shrink_pairs_plain::<4>(head, tail_a, tail_b),
            // `TilePyramidSink::new` rejects everything outside 1..=4 bpp.
            _ => unreachable!("bpp {bpp} outside 1..=4"),
        }
    }
    if w % 2 == 1 {
        // The last column is replicated, so both samples of the pair are it.
        let s = (w - 1) * bpp;
        let (pa, pb) = (&a[s..s + bpp], &b[s..s + bpp]);
        let dst = &mut out[(out_w - 1) * bpp..];
        if alpha && bpp == 4 {
            let q = [pa, pa, pb, pb];
            let a_sum: u32 = q.iter().map(|p| u32::from(p[3])).sum();
            for c in 0..3 {
                let num: u32 = q.iter().map(|p| u32::from(p[c]) * u32::from(p[3])).sum();
                dst[c] = (num + a_sum / 2).checked_div(a_sum).unwrap_or(0) as u8;
            }
            dst[3] = ((a_sum + 2) / 4) as u8;
        } else {
            for c in 0..bpp {
                let sum = 2 * u32::from(pa[c]) + 2 * u32::from(pb[c]);
                dst[c] = ((sum + 2) / 4) as u8;
            }
        }
    }
}

/// Allocating wrapper around [`shrink_rows_into`] — tests and the
/// non-recycled paths.
#[cfg(test)]
fn shrink_rows(a: &[u8], b: &[u8], w: u32, bpp: usize, alpha: bool) -> Vec<u8> {
    let mut out = vec![0u8; w.div_ceil(2) as usize * bpp];
    shrink_rows_into(&mut out, a, b, w, bpp, alpha);
    out
}

/// Every RGBA8 pixel of both rows has alpha 255.
fn rows_are_opaque(a: &[u8], b: &[u8]) -> bool {
    let (apx, _) = a.as_chunks::<4>();
    let (bpx, _) = b.as_chunks::<4>();
    apx.iter().chain(bpx).all(|p| p[3] == 255)
}

/// Plain per-channel rounded mean of a 2×2 block, `N` bytes per pixel.
///
/// `N` is a const so every index is inside a fixed-size array — no bounds
/// checks in the loop body, and the channel loop unrolls into something LLVM
/// can vectorize.
fn shrink_pairs_plain<const N: usize>(out: &mut [u8], a: &[u8], b: &[u8]) {
    let (dsts, _) = out.as_chunks_mut::<N>();
    let (apx, _) = a.as_chunks::<N>();
    let (bpx, _) = b.as_chunks::<N>();
    let (apairs, _) = apx.as_chunks::<2>();
    let (bpairs, _) = bpx.as_chunks::<2>();
    for ((dst, ap), bp) in dsts.iter_mut().zip(apairs).zip(bpairs) {
        for c in 0..N {
            let sum = u32::from(ap[0][c])
                + u32::from(ap[1][c])
                + u32::from(bp[0][c])
                + u32::from(bp[1][c]);
            dst[c] = ((sum + 2) / 4) as u8;
        }
    }
}

/// Alpha-weighted mean of a 2×2 RGBA block: color is averaged premultiplied
/// and alpha plainly, so fully transparent pixels don't bleed their color.
fn shrink_pairs_rgba(out: &mut [u8], a: &[u8], b: &[u8]) {
    let (dsts, _) = out.as_chunks_mut::<4>();
    let (apx, _) = a.as_chunks::<4>();
    let (bpx, _) = b.as_chunks::<4>();
    let (apairs, _) = apx.as_chunks::<2>();
    let (bpairs, _) = bpx.as_chunks::<2>();
    for ((dst, ap), bp) in dsts.iter_mut().zip(apairs).zip(bpairs) {
        let q = [ap[0], ap[1], bp[0], bp[1]];
        let a_sum =
            u32::from(q[0][3]) + u32::from(q[1][3]) + u32::from(q[2][3]) + u32::from(q[3][3]);
        let half = a_sum / 2;
        for c in 0..3 {
            let num = u32::from(q[0][c]) * u32::from(q[0][3])
                + u32::from(q[1][c]) * u32::from(q[1][3])
                + u32::from(q[2][c]) * u32::from(q[2][3])
                + u32::from(q[3][c]) * u32::from(q[3][3]);
            dst[c] = (num + half).checked_div(a_sum).unwrap_or(0) as u8;
        }
        dst[3] = ((a_sum + 2) / 4) as u8;
    }
}

impl<W: TileWriter> crate::Sink for TilePyramidSink<W> {
    fn consume(&mut self, strip: &Strip<'_>) -> PipeResult<()> {
        if self.finished {
            return Err(at!(PipeError::Op(String::from(
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
        for r in 0..strip.rows() {
            if self.rows_consumed >= self.height {
                return Err(at!(PipeError::DimensionMismatch(alloc::format!(
                    "TilePyramidSink: more than {} rows received",
                    self.height
                ))));
            }
            let row = self.canvas_row(strip.row(r));
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
        // Padded geometry: the rows below the image are background.
        if self.canvas_h > self.height {
            let bg = self.background_row();
            for _ in self.height..self.canvas_h {
                self.push_row(0, bg.clone())?;
            }
        }
        // Odd heights: the last row pairs with itself, level by level (each
        // flush may leave an unpaired row one level down). Levels are walked
        // top down so level `li` has received all its rows before it is read.
        for li in 0..self.levels.len().saturating_sub(1) {
            let level = &self.levels[li];
            if level.next_row.is_multiple_of(2) {
                continue;
            }
            let (lw, base, idx) = (level.info.width, level.first_row, level.next_row - 1);
            let mut s = vec![0u8; lw.div_ceil(2) as usize * self.bpp];
            {
                let last = &self.levels[li].rows[(idx - base) as usize];
                shrink_rows_into(&mut s, last, last, lw, self.bpp, self.format.has_alpha());
            }
            self.push_row(li + 1, s)?;
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

// ─── Layouts ───

/// Names tiles and writes the descriptor for one viewer format. Paths are
/// `/`-separated and relative to the [`TileStore`] root.
pub trait TileLayout: Send {
    /// Reject a geometry the viewer format cannot read.
    fn validate(&self, _levels: &[LevelInfo], _config: TilePyramidConfig) -> PipeResult<()> {
        Ok(())
    }

    /// Relative path for the tile at `key`. `levels` is apex-first,
    /// `image` the source size (before padding).
    fn tile_path(
        &self,
        key: TileKey,
        tile: (u32, u32),
        levels: &[LevelInfo],
        config: TilePyramidConfig,
        image: (u32, u32),
    ) -> String;

    /// Descriptor file(s) as `(relative path, bytes)`, written at finish.
    fn descriptors(
        &self,
        levels: &[LevelInfo],
        config: TilePyramidConfig,
        image: (u32, u32),
        tiles_written: u64,
    ) -> Vec<(String, Vec<u8>)>;
}

/// Deep Zoom: `{name}.dzi` + `{name}_files/{level}/{col}_{row}.{ext}`
/// (OpenSeadragon). Needs [`PyramidGeometry::ToOnePixel`].
#[derive(Clone, Debug)]
pub struct DziLayout {
    /// Base name (`{name}.dzi`, `{name}_files/`).
    pub name: String,
    /// Tile extension recorded in the descriptor (`jpeg`, `png`, `webp`).
    pub ext: String,
}

impl DziLayout {
    /// `{name}.dzi` with `{ext}` tiles.
    pub fn new(name: impl Into<String>, ext: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ext: ext.into(),
        }
    }
}

impl TileLayout for DziLayout {
    fn validate(&self, _levels: &[LevelInfo], config: TilePyramidConfig) -> PipeResult<()> {
        if config.geometry != PyramidGeometry::ToOnePixel {
            return Err(at!(PipeError::Op(String::from(
                "DziLayout needs PyramidGeometry::ToOnePixel (TilePyramidConfig::dzi)"
            ))));
        }
        Ok(())
    }

    fn tile_path(
        &self,
        (level, col, row): TileKey,
        _tile: (u32, u32),
        _levels: &[LevelInfo],
        _config: TilePyramidConfig,
        _image: (u32, u32),
    ) -> String {
        alloc::format!("{}_files/{level}/{col}_{row}.{}", self.name, self.ext)
    }

    fn descriptors(
        &self,
        levels: &[LevelInfo],
        config: TilePyramidConfig,
        _image: (u32, u32),
        _tiles_written: u64,
    ) -> Vec<(String, Vec<u8>)> {
        let full = levels.last().copied().unwrap_or(LevelInfo {
            level: 0,
            width: 0,
            height: 0,
        });
        let xml = alloc::format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Image xmlns=\"http://schemas.microsoft.com/deepzoom/2008\" Format=\"{}\" Overlap=\"{}\" TileSize=\"{}\">\n  <Size Height=\"{}\" Width=\"{}\"/>\n</Image>\n",
            self.ext,
            config.overlap,
            config.tile_size,
            full.height,
            full.width
        );
        vec![(alloc::format!("{}.dzi", self.name), xml.into_bytes())]
    }
}

/// IIIF Image API 3.0 level-0 static tiles:
/// `{id}/{x},{y},{w},{h}/{tw},{th}/0/default.{ext}` + `{id}/info.json`.
/// Region coordinates are full-resolution pixels, size the tile's own
/// pixels (libvips `dzsave --layout iiif3` convention). Needs overlap 0 and
/// [`PyramidGeometry::ToOnePixel`].
#[derive(Clone, Debug)]
pub struct Iiif3Layout {
    /// Directory / service id segment (`{id}/info.json`).
    pub id: String,
    /// Tile extension (`jpg`, `png`, `webp`).
    pub ext: String,
    /// Absolute service id written to `info.json` (`"id"`); defaults to
    /// `id` when `None`.
    pub service_id: Option<String>,
}

impl Iiif3Layout {
    /// Tiles under `{id}/`, `{ext}` tiles.
    pub fn new(id: impl Into<String>, ext: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ext: ext.into(),
            service_id: None,
        }
    }

    /// The absolute `id` URL to record in `info.json`.
    pub fn with_service_id(mut self, service_id: impl Into<String>) -> Self {
        self.service_id = Some(service_id.into());
        self
    }
}

impl TileLayout for Iiif3Layout {
    fn validate(&self, _levels: &[LevelInfo], config: TilePyramidConfig) -> PipeResult<()> {
        if config.overlap != 0 || config.geometry != PyramidGeometry::ToOnePixel {
            return Err(at!(PipeError::Op(String::from(
                "Iiif3Layout needs overlap 0 and PyramidGeometry::ToOnePixel (TilePyramidConfig::iiif)"
            ))));
        }
        Ok(())
    }

    fn tile_path(
        &self,
        (level, col, row): TileKey,
        (tw, th): (u32, u32),
        levels: &[LevelInfo],
        config: TilePyramidConfig,
        (img_w, img_h): (u32, u32),
    ) -> String {
        let n = levels.len() as u32 - 1;
        let scale = 1u64 << (n - level);
        let t = u64::from(config.tile_size);
        let x = u64::from(col) * t * scale;
        let y = u64::from(row) * t * scale;
        let w = (t * scale).min(u64::from(img_w).saturating_sub(x));
        let h = (t * scale).min(u64::from(img_h).saturating_sub(y));
        alloc::format!(
            "{}/{x},{y},{w},{h}/{tw},{th}/0/default.{}",
            self.id,
            self.ext
        )
    }

    fn descriptors(
        &self,
        levels: &[LevelInfo],
        config: TilePyramidConfig,
        (img_w, img_h): (u32, u32),
        _tiles_written: u64,
    ) -> Vec<(String, Vec<u8>)> {
        let n = levels.len() as u32 - 1;
        let sizes: Vec<String> = levels
            .iter()
            .map(|l| alloc::format!("{{\"width\":{},\"height\":{}}}", l.width, l.height))
            .collect();
        let scales: Vec<String> = (0..=n).map(|k| alloc::format!("{}", 1u64 << k)).collect();
        let id = self.service_id.as_deref().unwrap_or(&self.id);
        let json = alloc::format!(
            "{{\"@context\":\"http://iiif.io/api/image/3/context.json\",\"id\":\"{}\",\"type\":\"ImageService3\",\"protocol\":\"http://iiif.io/api/image\",\"profile\":\"level0\",\"width\":{},\"height\":{},\"sizes\":[{}],\"tiles\":[{{\"width\":{},\"height\":{},\"scaleFactors\":[{}]}}],\"preferredFormats\":[\"{}\"]}}\n",
            json_escape(id),
            img_w,
            img_h,
            sizes.join(","),
            config.tile_size,
            config.tile_size,
            scales.join(","),
            self.ext
        );
        vec![(alloc::format!("{}/info.json", self.id), json.into_bytes())]
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&alloc::format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Google Maps XYZ: `{z}/{y}/{x}.{ext}`, `z = 0` the single apex tile.
/// Needs [`PyramidGeometry::PaddedSquare`] and overlap 0 (every tile is a
/// complete `tile_size` square). No descriptor.
#[derive(Clone, Debug)]
pub struct GoogleMapsLayout {
    /// Tile extension.
    pub ext: String,
}

impl GoogleMapsLayout {
    /// `{z}/{y}/{x}.{ext}` tiles.
    pub fn new(ext: impl Into<String>) -> Self {
        Self { ext: ext.into() }
    }
}

impl TileLayout for GoogleMapsLayout {
    fn validate(&self, _levels: &[LevelInfo], config: TilePyramidConfig) -> PipeResult<()> {
        if config.overlap != 0 || !matches!(config.geometry, PyramidGeometry::PaddedSquare { .. }) {
            return Err(at!(PipeError::Op(String::from(
                "GoogleMapsLayout needs overlap 0 and PyramidGeometry::PaddedSquare (TilePyramidConfig::google_maps)"
            ))));
        }
        Ok(())
    }

    fn tile_path(
        &self,
        (level, col, row): TileKey,
        _tile: (u32, u32),
        _levels: &[LevelInfo],
        _config: TilePyramidConfig,
        _image: (u32, u32),
    ) -> String {
        alloc::format!("{level}/{row}/{col}.{}", self.ext)
    }

    fn descriptors(
        &self,
        _levels: &[LevelInfo],
        _config: TilePyramidConfig,
        _image: (u32, u32),
        _tiles_written: u64,
    ) -> Vec<(String, Vec<u8>)> {
        Vec::new()
    }
}

/// Zoomify: `TileGroup{n}/{level}-{col}-{row}.{ext}` + `ImageProperties.xml`,
/// tiles numbered sequentially from the apex level in raster order and
/// grouped 256 per `TileGroup`. Needs overlap 0 and
/// [`PyramidGeometry::ToOneTile`].
#[derive(Clone, Debug)]
pub struct ZoomifyLayout {
    /// Tile extension.
    pub ext: String,
}

impl ZoomifyLayout {
    /// `TileGroup{n}/{level}-{col}-{row}.{ext}` tiles.
    pub fn new(ext: impl Into<String>) -> Self {
        Self { ext: ext.into() }
    }

    /// Sequential tile number: every tile of the levels below `level`, then
    /// raster order within the level.
    fn tile_number((level, col, row): TileKey, levels: &[LevelInfo], tile: u32) -> u64 {
        let mut n = 0u64;
        for l in levels {
            let (cols, rows) = l.tile_grid(tile);
            if l.level < level {
                n += u64::from(cols) * u64::from(rows);
            } else if l.level == level {
                n += u64::from(row) * u64::from(cols) + u64::from(col);
            }
        }
        n
    }
}

impl TileLayout for ZoomifyLayout {
    fn validate(&self, _levels: &[LevelInfo], config: TilePyramidConfig) -> PipeResult<()> {
        if config.overlap != 0 || config.geometry != PyramidGeometry::ToOneTile {
            return Err(at!(PipeError::Op(String::from(
                "ZoomifyLayout needs overlap 0 and PyramidGeometry::ToOneTile (TilePyramidConfig::zoomify)"
            ))));
        }
        Ok(())
    }

    fn tile_path(
        &self,
        key: TileKey,
        _tile: (u32, u32),
        levels: &[LevelInfo],
        config: TilePyramidConfig,
        _image: (u32, u32),
    ) -> String {
        let n = Self::tile_number(key, levels, config.tile_size);
        let (level, col, row) = key;
        alloc::format!("TileGroup{}/{level}-{col}-{row}.{}", n / 256, self.ext)
    }

    fn descriptors(
        &self,
        levels: &[LevelInfo],
        config: TilePyramidConfig,
        (img_w, img_h): (u32, u32),
        _tiles_written: u64,
    ) -> Vec<(String, Vec<u8>)> {
        let total: u64 = levels
            .iter()
            .map(|l| {
                let (c, r) = l.tile_grid(config.tile_size);
                u64::from(c) * u64::from(r)
            })
            .sum();
        let xml = alloc::format!(
            "<IMAGE_PROPERTIES WIDTH=\"{img_w}\" HEIGHT=\"{img_h}\" NUMTILES=\"{total}\" NUMIMAGES=\"1\" VERSION=\"1.8\" TILESIZE=\"{}\"/>\n",
            config.tile_size
        );
        vec![(String::from("ImageProperties.xml"), xml.into_bytes())]
    }
}

// ─── Stores ───

/// Persists named byte blobs (tiles, descriptors).
pub trait TileStore: Send {
    /// Store `bytes` at the `/`-separated relative `path`.
    fn put(&mut self, path: &str, bytes: &[u8]) -> PipeResult<()>;

    /// Flush / close (zip central directory, etc.).
    fn finish(&mut self) -> PipeResult<()> {
        Ok(())
    }
}

/// In-memory store: `path → bytes` (tests, small pyramids).
#[derive(Default)]
pub struct MemoryStore {
    /// Everything stored, by path.
    pub files: alloc::collections::BTreeMap<String, Vec<u8>>,
}

impl TileStore for MemoryStore {
    fn put(&mut self, path: &str, bytes: &[u8]) -> PipeResult<()> {
        self.files.insert(String::from(path), bytes.to_vec());
        Ok(())
    }
}

/// Filesystem store rooted at a directory; parent directories are created
/// on demand.
#[cfg(feature = "std")]
pub struct FsStore {
    root: std::path::PathBuf,
    made: std::collections::HashSet<std::path::PathBuf>,
}

#[cfg(feature = "std")]
impl FsStore {
    /// Store under `root`.
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            root: root.into(),
            made: std::collections::HashSet::new(),
        }
    }

    /// The root directory.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}

#[cfg(feature = "std")]
impl TileStore for FsStore {
    fn put(&mut self, path: &str, bytes: &[u8]) -> PipeResult<()> {
        let full = self.root.join(path);
        if let Some(parent) = full.parent()
            && !self.made.contains(parent)
        {
            std::fs::create_dir_all(parent).map_err(|e| {
                at!(PipeError::Op(alloc::format!(
                    "FsStore: create {}: {e}",
                    parent.display()
                )))
            })?;
            self.made.insert(parent.to_path_buf());
        }
        std::fs::write(&full, bytes).map_err(|e| {
            at!(PipeError::Op(alloc::format!(
                "FsStore: write {}: {e}",
                full.display()
            )))
        })
    }
}

/// Streaming zip store (entries stored uncompressed — tiles are already
/// compressed) for single-file CDN / object-storage deployment. Writes
/// sequentially to any [`Write`](std::io::Write); ZIP64 records are emitted
/// when the archive has more than 65 535 entries or exceeds 4 GiB. Entries
/// larger than 4 GiB are rejected.
#[cfg(feature = "std")]
pub struct ZipStore<W: std::io::Write + Send> {
    out: W,
    offset: u64,
    /// `(name, crc32, size, local header offset)`.
    entries: Vec<(String, u32, u32, u64)>,
    finished: bool,
}

#[cfg(feature = "std")]
impl<W: std::io::Write + Send> ZipStore<W> {
    /// Write the archive to `out`.
    pub fn new(out: W) -> Self {
        Self {
            out,
            offset: 0,
            entries: Vec::new(),
            finished: false,
        }
    }

    /// Entries written so far.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Finish (if not already) and return the writer.
    pub fn into_inner(mut self) -> PipeResult<W> {
        TileStore::finish(&mut self)?;
        Ok(self.out)
    }

    fn io(e: std::io::Error) -> whereat::At<PipeError> {
        at!(PipeError::Op(alloc::format!("ZipStore: {e}")))
    }

    fn write(&mut self, bytes: &[u8]) -> PipeResult<()> {
        self.out.write_all(bytes).map_err(Self::io)?;
        self.offset += bytes.len() as u64;
        Ok(())
    }
}

#[cfg(feature = "std")]
impl<W: std::io::Write + Send> TileStore for ZipStore<W> {
    fn put(&mut self, path: &str, bytes: &[u8]) -> PipeResult<()> {
        if self.finished {
            return Err(at!(PipeError::Op(String::from(
                "ZipStore: put after finish"
            ))));
        }
        let size = u32::try_from(bytes.len()).map_err(|_| {
            at!(PipeError::LimitExceeded(alloc::format!(
                "ZipStore: entry {path} is {} bytes; entries over 4 GiB are not supported",
                bytes.len()
            )))
        })?;
        let name = path.as_bytes();
        if name.len() > u16::MAX as usize {
            return Err(at!(PipeError::Op(String::from(
                "ZipStore: path longer than 65535 bytes"
            ))));
        }
        let crc = crc32(bytes);
        let header_offset = self.offset;
        let mut h = Vec::with_capacity(30 + name.len());
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes()); // local file header
        h.extend_from_slice(&20u16.to_le_bytes()); // version needed
        h.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 names
        h.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        h.extend_from_slice(&0u16.to_le_bytes()); // mod time
        h.extend_from_slice(&0x21u16.to_le_bytes()); // mod date (1980-01-01)
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&size.to_le_bytes()); // compressed
        h.extend_from_slice(&size.to_le_bytes()); // uncompressed
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // extra len
        h.extend_from_slice(name);
        self.write(&h)?;
        self.write(bytes)?;
        self.entries
            .push((String::from(path), crc, size, header_offset));
        Ok(())
    }

    fn finish(&mut self) -> PipeResult<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let cd_start = self.offset;
        let entries = core::mem::take(&mut self.entries);
        for (name, crc, size, off) in &entries {
            let name = name.as_bytes();
            let zip64 = *off >= 0xFFFF_FFFF;
            let mut c = Vec::with_capacity(46 + name.len() + 12);
            c.extend_from_slice(&0x0201_4b50u32.to_le_bytes()); // central dir header
            c.extend_from_slice(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes()); // made by
            c.extend_from_slice(&(if zip64 { 45u16 } else { 20u16 }).to_le_bytes()); // needed
            c.extend_from_slice(&0x0800u16.to_le_bytes());
            c.extend_from_slice(&0u16.to_le_bytes());
            c.extend_from_slice(&0u16.to_le_bytes());
            c.extend_from_slice(&0x21u16.to_le_bytes());
            c.extend_from_slice(&crc.to_le_bytes());
            c.extend_from_slice(&size.to_le_bytes());
            c.extend_from_slice(&size.to_le_bytes());
            c.extend_from_slice(&(name.len() as u16).to_le_bytes());
            c.extend_from_slice(&(if zip64 { 12u16 } else { 0u16 }).to_le_bytes()); // extra len
            c.extend_from_slice(&0u16.to_le_bytes()); // comment len
            c.extend_from_slice(&0u16.to_le_bytes()); // disk
            c.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            c.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            c.extend_from_slice(&(if zip64 { 0xFFFF_FFFFu32 } else { *off as u32 }).to_le_bytes());
            c.extend_from_slice(name);
            if zip64 {
                c.extend_from_slice(&0x0001u16.to_le_bytes()); // zip64 extra
                c.extend_from_slice(&8u16.to_le_bytes());
                c.extend_from_slice(&off.to_le_bytes());
            }
            self.write(&c)?;
        }
        let cd_size = self.offset - cd_start;
        let count = entries.len() as u64;
        let need64 = count > 0xFFFF || cd_size >= 0xFFFF_FFFF || cd_start >= 0xFFFF_FFFF;
        if need64 {
            let z64_off = self.offset;
            let mut z = Vec::with_capacity(56 + 20);
            z.extend_from_slice(&0x0606_4b50u32.to_le_bytes()); // zip64 EOCD
            z.extend_from_slice(&44u64.to_le_bytes()); // size of remaining record
            z.extend_from_slice(&45u16.to_le_bytes());
            z.extend_from_slice(&45u16.to_le_bytes());
            z.extend_from_slice(&0u32.to_le_bytes()); // this disk
            z.extend_from_slice(&0u32.to_le_bytes()); // cd disk
            z.extend_from_slice(&count.to_le_bytes());
            z.extend_from_slice(&count.to_le_bytes());
            z.extend_from_slice(&cd_size.to_le_bytes());
            z.extend_from_slice(&cd_start.to_le_bytes());
            z.extend_from_slice(&0x0706_4b50u32.to_le_bytes()); // zip64 EOCD locator
            z.extend_from_slice(&0u32.to_le_bytes());
            z.extend_from_slice(&z64_off.to_le_bytes());
            z.extend_from_slice(&1u32.to_le_bytes());
            self.write(&z)?;
        }
        let mut e = Vec::with_capacity(22);
        e.extend_from_slice(&0x0605_4b50u32.to_le_bytes()); // EOCD
        e.extend_from_slice(&0u16.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        let c16 = if count > 0xFFFF {
            0xFFFFu16
        } else {
            count as u16
        };
        e.extend_from_slice(&c16.to_le_bytes());
        e.extend_from_slice(&c16.to_le_bytes());
        e.extend_from_slice(&(cd_size.min(0xFFFF_FFFF) as u32).to_le_bytes());
        e.extend_from_slice(&(cd_start.min(0xFFFF_FFFF) as u32).to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        self.write(&e)?;
        self.out.flush().map_err(Self::io)
    }
}

/// CRC-32 (IEEE 802.3), as zip needs.
///
/// Slicing-by-8: consumes 8 bytes per iteration from eight 256-entry tables
/// instead of one byte from one. Tile bytes are the whole payload of a
/// [`ZipStore`] pyramid, and the byte-at-a-time form was measured as *all* of
/// zip's overhead over [`FsStore`] (10 000 × 1000 DZI: 0.105 s vs 0.024 s for
/// the same 229 tiles). The 8 KiB of tables is built once, on first use.
#[cfg(feature = "std")]
fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[[u32; 256]; 8]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [[0u32; 256]; 8];
        for (i, e) in t[0].iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        for n in 1..8 {
            for i in 0..256 {
                let prev = t[n - 1][i];
                t[n][i] = (prev >> 8) ^ t[0][(prev & 0xFF) as usize];
            }
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    let (blocks, tail) = data.as_chunks::<8>();
    for b in blocks {
        crc ^= u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        crc = t[7][(crc & 0xFF) as usize]
            ^ t[6][((crc >> 8) & 0xFF) as usize]
            ^ t[5][((crc >> 16) & 0xFF) as usize]
            ^ t[4][(crc >> 24) as usize]
            ^ t[3][b[4] as usize]
            ^ t[2][b[5] as usize]
            ^ t[1][b[6] as usize]
            ^ t[0][b[7] as usize];
    }
    for &b in tail {
        crc = t[0][((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

// ─── PyramidWriter ───

/// Per-tile encoder used by [`PyramidWriter`]: tile in, encoded bytes out.
/// `Sync` so tile rows can be encoded on several threads.
#[cfg(feature = "std")]
pub type TileEncoder = alloc::sync::Arc<dyn Fn(TileRef<'_>) -> PipeResult<Vec<u8>> + Send + Sync>;

/// [`TileWriter`] that encodes tiles with a caller-supplied encoder and
/// persists them through a [`TileLayout`] into a [`TileStore`].
#[cfg(feature = "std")]
pub struct PyramidWriter<L: TileLayout, S: TileStore> {
    layout: L,
    store: S,
    encode: TileEncoder,
    threads: usize,
    skip_blanks: Option<([u8; 4], u8)>,
    levels: Vec<LevelInfo>,
    config: TilePyramidConfig,
    image: (u32, u32),
    tiles_written: u64,
    tiles_skipped: u64,
}

#[cfg(feature = "std")]
impl<L: TileLayout, S: TileStore> PyramidWriter<L, S> {
    /// Encode every tile with `encode` (e.g. a `zencodecs::EncodeRequest`),
    /// name it by `layout`, persist it in `store`. Sequential by default.
    pub fn new(
        layout: L,
        store: S,
        encode: impl Fn(TileRef<'_>) -> PipeResult<Vec<u8>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            layout,
            store,
            encode: alloc::sync::Arc::new(encode),
            threads: 1,
            skip_blanks: None,
            levels: Vec::new(),
            config: TilePyramidConfig::default(),
            image: (0, 0),
            tiles_written: 0,
            tiles_skipped: 0,
        }
    }

    /// Encode each tile row on up to `threads` scoped threads (`0` means
    /// [`std::thread::available_parallelism`]). Store writes stay on the
    /// calling thread, in raster order.
    pub fn with_threads(mut self, threads: usize) -> Self {
        self.threads = if threads == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        } else {
            threads
        };
        self
    }

    /// Don't store tiles whose every channel is within `threshold` of
    /// `background` (see [`TileRef::is_blank`]); the viewer shows its own
    /// fill for the missing tile.
    pub fn with_skip_blanks(mut self, background: [u8; 4], threshold: u8) -> Self {
        self.skip_blanks = Some((background, threshold));
        self
    }

    /// Tiles stored so far.
    pub fn tiles_written(&self) -> u64 {
        self.tiles_written
    }

    /// Tiles skipped as blank so far.
    pub fn tiles_skipped(&self) -> u64 {
        self.tiles_skipped
    }

    /// Borrow the store (read back a [`MemoryStore`], the [`FsStore`] root).
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Take the store out after [`TileWriter::finish`].
    pub fn into_store(self) -> S {
        self.store
    }

    /// The layout.
    pub fn layout(&self) -> &L {
        &self.layout
    }

    fn put_encoded(&mut self, tile: &TileRef<'_>, bytes: &[u8]) -> PipeResult<()> {
        let path = self.layout.tile_path(
            (tile.level, tile.col, tile.row),
            (tile.width, tile.height),
            &self.levels,
            self.config,
            self.image,
        );
        self.store.put(&path, bytes)?;
        self.tiles_written += 1;
        Ok(())
    }
}

#[cfg(feature = "std")]
impl<L: TileLayout, S: TileStore> TileWriter for PyramidWriter<L, S> {
    fn begin(
        &mut self,
        levels: &[LevelInfo],
        config: TilePyramidConfig,
        _format: PixelFormat,
    ) -> PipeResult<()> {
        self.layout.validate(levels, config)?;
        self.levels = levels.to_vec();
        self.config = config;
        Ok(())
    }

    fn write_tile(&mut self, tile: TileRef<'_>) -> PipeResult<()> {
        if let Some((bg, thr)) = self.skip_blanks
            && tile.is_blank(&bg, thr)
        {
            self.tiles_skipped += 1;
            return Ok(());
        }
        let bytes = (self.encode)(tile)?;
        self.put_encoded(&tile, &bytes)
    }

    fn write_tile_row(&mut self, tiles: &[TileRef<'_>]) -> PipeResult<()> {
        // Blank filtering first (cheap, single thread).
        let keep: Vec<&TileRef<'_>> = tiles
            .iter()
            .filter(|t| match self.skip_blanks {
                Some((bg, thr)) if t.is_blank(&bg, thr) => {
                    self.tiles_skipped += 1;
                    false
                }
                _ => true,
            })
            .collect();
        if self.threads <= 1 || keep.len() <= 1 {
            for t in keep {
                let bytes = (self.encode)(*t)?;
                self.put_encoded(t, &bytes)?;
            }
            return Ok(());
        }
        // Parallel encode: `threads` workers pull tiles by index; results
        // land in a per-tile slot so store writes stay in raster order.
        let workers = self.threads.min(keep.len());
        let next = core::sync::atomic::AtomicUsize::new(0);
        let slots: Vec<std::sync::Mutex<Option<PipeResult<Vec<u8>>>>> = (0..keep.len())
            .map(|_| std::sync::Mutex::new(None))
            .collect();
        let encode = &self.encode;
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        if i >= keep.len() {
                            break;
                        }
                        let r = encode(*keep[i]);
                        *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
                    }
                });
            }
        });
        let results = slots
            .into_iter()
            .map(|m| m.into_inner().unwrap_or_else(|e| e.into_inner()));
        for (t, r) in keep.iter().zip(results) {
            let bytes = r.ok_or_else(|| {
                at!(PipeError::Op(String::from(
                    "PyramidWriter: tile encode produced no result"
                )))
            })??;
            self.put_encoded(t, &bytes)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> PipeResult<()> {
        for (path, bytes) in
            self.layout
                .descriptors(&self.levels, self.config, self.image, self.tiles_written)
        {
            self.store.put(&path, &bytes)?;
        }
        self.store.finish()
    }
}

#[cfg(feature = "std")]
impl<L: TileLayout, S: TileStore> PyramidWriter<L, S> {
    /// Record the source image size (before padding) for layouts whose
    /// descriptor / paths need it (IIIF regions, Zoomify properties).
    /// [`TilePyramidSink::with_writer_image_size`] calls this.
    pub fn set_image_size(&mut self, width: u32, height: u32) {
        self.image = (width, height);
    }
}

#[cfg(feature = "std")]
impl<L: TileLayout, S: TileStore> TilePyramidSink<PyramidWriter<L, S>> {
    /// Build a sink whose [`PyramidWriter`] knows the source image size.
    pub fn with_pyramid_writer(
        width: u32,
        height: u32,
        format: PixelFormat,
        config: TilePyramidConfig,
        mut writer: PyramidWriter<L, S>,
    ) -> PipeResult<Self> {
        writer.set_image_size(width, height);
        Self::new(width, height, format, config, writer)
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

    /// Reference: full-image shrink chain over `geometry`.
    fn reference_levels(
        mut img: Vec<u8>,
        mut w: u32,
        mut h: u32,
        tile: u32,
        geometry: PyramidGeometry,
    ) -> Vec<StoredTile> {
        let mut out = vec![(w, h, img.clone())];
        loop {
            let done = match geometry {
                PyramidGeometry::ToOnePixel => w == 1 && h == 1,
                _ => w <= tile && h <= tile,
            };
            if done {
                break;
            }
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

    fn feed<W: TileWriter>(
        sink: &mut TilePyramidSink<W>,
        img: &[u8],
        w: u32,
        h: u32,
        strip_rows: u32,
    ) {
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
        feed(&mut sink, &img, w, h, strip_rows);
        (sink.into_writer(), img)
    }

    /// Pad `img` (w×h) into a canvas `cw×ch` with `bg`.
    fn padded(img: &[u8], w: u32, h: u32, cw: u32, ch: u32, bg: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::with_capacity((cw * ch * 4) as usize);
        for y in 0..ch {
            for x in 0..cw {
                if x < w && y < h {
                    let i = ((y * w + x) * 4) as usize;
                    out.extend_from_slice(&img[i..i + 4]);
                } else {
                    out.extend_from_slice(&bg);
                }
            }
        }
        out
    }

    fn check(w: u32, h: u32, cfg: TilePyramidConfig, strip_rows: u32) {
        let (writer, img) = run(w, h, cfg, strip_rows);
        assert!(writer.finished());
        let (cw, ch, canvas) = match cfg.geometry {
            PyramidGeometry::PaddedSquare { background } => {
                let mut s = cfg.tile_size;
                while s < w.max(h) {
                    s *= 2;
                }
                (s, s, padded(&img, w, h, s, s, background))
            }
            _ => (w, h, img),
        };
        let levels = reference_levels(canvas, cw, ch, cfg.tile_size, cfg.geometry);
        let n = levels.len() as u32 - 1;
        assert_eq!(
            writer.levels.len(),
            levels.len(),
            "level count {w}x{h} {cfg:?}"
        );
        let (aw, ah) = (levels.last().unwrap().0, levels.last().unwrap().1);
        assert_eq!(
            writer.levels[0],
            LevelInfo {
                level: 0,
                width: aw,
                height: ah
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
    fn one_tile_and_padded_square_geometries_match_reference() {
        let zoomify = TilePyramidConfig::zoomify().with_geometry(PyramidGeometry::ToOneTile);
        let small = TilePyramidConfig {
            tile_size: 32,
            ..zoomify
        };
        check(300, 200, small, 7);
        check(31, 31, small, 31); // already one tile: a single level
        check(33, 5, small, 2);
        let google = TilePyramidConfig {
            tile_size: 32,
            overlap: 0,
            geometry: PyramidGeometry::PaddedSquare {
                background: [9, 8, 7, 255],
            },
        };
        check(300, 200, google, 7); // → 512² canvas, 5 levels
        check(32, 32, google, 32); // exactly one tile
        check(33, 1, google, 1); // → 64² canvas
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

        // Zoomify: stop at the first level that fits one 256 tile → 1000×40
        // halves to 500, 250 → 3 levels.
        let z = TilePyramidSink::new(
            1000,
            40,
            crate::format::RGBA8_SRGB,
            TilePyramidConfig::zoomify(),
            MemoryTileWriter::default(),
        )
        .unwrap();
        assert_eq!(z.level_count(), 3);
        assert_eq!(z.level_infos()[0].width, 250);
        // Google: 1000 → 1024 canvas, levels 1024/512/256 → 3 levels.
        let g = TilePyramidSink::new(
            1000,
            40,
            crate::format::RGBA8_SRGB,
            TilePyramidConfig::google_maps([0; 4]),
            MemoryTileWriter::default(),
        )
        .unwrap();
        assert_eq!(g.canvas_size(), (1024, 1024));
        assert_eq!(g.level_count(), 3);
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

    #[test]
    fn blank_detection_compares_first_bpp_channels_within_threshold() {
        let data = [10, 20, 30, 255, 12, 18, 31, 255];
        let t = TileRef {
            level: 0,
            col: 0,
            row: 0,
            width: 2,
            height: 1,
            format: crate::format::RGBA8_SRGB,
            data: &data,
        };
        assert!(t.is_blank(&[11, 19, 30, 255], 2));
        assert!(t.is_blank(&[11, 19, 30, 255], 1));
        assert!(!t.is_blank(&[11, 19, 30, 255], 0));
        assert!(!t.is_blank(&[11, 19, 60, 255], 2));
        assert!(!t.is_blank(&[11, 19], 2), "background shorter than bpp");
    }

    #[test]
    fn zoomify_tile_numbering_is_sequential_from_the_apex() {
        // Apex 1×1 tiles, then a 2×2 level, then a 3×2 level.
        let levels = [
            LevelInfo {
                level: 0,
                width: 200,
                height: 100,
            },
            LevelInfo {
                level: 1,
                width: 400,
                height: 300,
            },
            LevelInfo {
                level: 2,
                width: 700,
                height: 500,
            },
        ];
        let n = |k| ZoomifyLayout::tile_number(k, &levels, 256);
        assert_eq!(n((0, 0, 0)), 0);
        assert_eq!(n((1, 0, 0)), 1);
        assert_eq!(n((1, 1, 0)), 2);
        assert_eq!(n((1, 0, 1)), 3);
        assert_eq!(n((1, 1, 1)), 4);
        assert_eq!(n((2, 0, 0)), 5);
        assert_eq!(n((2, 2, 1)), 5 + 3 + 2);
        let l = ZoomifyLayout::new("jpg");
        let cfg = TilePyramidConfig::zoomify();
        assert_eq!(
            l.tile_path((2, 2, 1), (188, 244), &levels, cfg, (700, 500)),
            "TileGroup0/2-2-1.jpg"
        );
        let (path, xml) = l.descriptors(&levels, cfg, (700, 500), 11).remove(0);
        assert_eq!(path, "ImageProperties.xml");
        let xml = String::from_utf8(xml).unwrap();
        assert!(
            xml.contains("NUMTILES=\"11\"") && xml.contains("WIDTH=\"700\""),
            "{xml}"
        );
    }

    #[test]
    fn iiif3_paths_are_full_resolution_regions() {
        // 700×500 at 256 tiles → levels 700, 350, 175, 88, 44, 22, 11, 6, 3, 2, 1.
        let dims = level_dims(700, 500, 256, PyramidGeometry::ToOnePixel);
        let n = dims.len() as u32 - 1;
        let levels: Vec<LevelInfo> = dims
            .iter()
            .rev()
            .enumerate()
            .map(|(i, &(w, h))| LevelInfo {
                level: i as u32,
                width: w,
                height: h,
            })
            .collect();
        let l = Iiif3Layout::new("img", "jpg");
        let cfg = TilePyramidConfig {
            tile_size: 256,
            ..TilePyramidConfig::iiif()
        };
        // Full res, second column: x = 256, width clipped to 700 - 512 = 188.
        assert_eq!(
            l.tile_path((n, 2, 1), (188, 244), &levels, cfg, (700, 500)),
            "img/512,256,188,244/188,244/0/default.jpg"
        );
        // One level down (scale 2): tile (1,0) covers 512..700 → w 188 at
        // full res, tile itself 94 px wide.
        assert_eq!(
            l.tile_path((n - 1, 1, 0), (94, 250), &levels, cfg, (700, 500)),
            "img/512,0,188,500/94,250/0/default.jpg"
        );
        let (path, json) = l.descriptors(&levels, cfg, (700, 500), 0).remove(0);
        assert_eq!(path, "img/info.json");
        let json = String::from_utf8(json).unwrap();
        assert!(json.contains("\"width\":700,\"height\":500"), "{json}");
        assert!(
            json.contains("\"scaleFactors\":[1,2,4,8,16,32,64,128,256,512,1024]"),
            "{json}"
        );
        assert!(json.contains("{\"width\":1,\"height\":1}"), "{json}");
    }

    #[test]
    fn layouts_reject_mismatched_geometry() {
        let levels = [LevelInfo {
            level: 0,
            width: 1,
            height: 1,
        }];
        assert!(
            DziLayout::new("a", "png")
                .validate(&levels, TilePyramidConfig::zoomify())
                .is_err()
        );
        assert!(
            Iiif3Layout::new("a", "png")
                .validate(&levels, TilePyramidConfig::dzi())
                .is_err()
        );
        assert!(
            GoogleMapsLayout::new("png")
                .validate(&levels, TilePyramidConfig::iiif())
                .is_err()
        );
        assert!(
            ZoomifyLayout::new("png")
                .validate(&levels, TilePyramidConfig::google_maps([0; 4]))
                .is_err()
        );
        assert!(
            DziLayout::new("a", "png")
                .validate(&levels, TilePyramidConfig::dzi())
                .is_ok()
        );
        assert!(
            Iiif3Layout::new("a", "png")
                .validate(&levels, TilePyramidConfig::iiif())
                .is_ok()
        );
        assert!(
            GoogleMapsLayout::new("png")
                .validate(&levels, TilePyramidConfig::google_maps([0; 4]))
                .is_ok()
        );
        assert!(
            ZoomifyLayout::new("png")
                .validate(&levels, TilePyramidConfig::zoomify())
                .is_ok()
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn crc32_matches_known_vectors() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    /// The slicing-by-8 CRC must equal the textbook byte-at-a-time form at
    /// every length, including the 0..8-byte tails either side of a block.
    #[cfg(feature = "std")]
    #[test]
    fn crc32_matches_byte_at_a_time_at_every_length() {
        fn reference(data: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for &b in data {
                let mut c = (crc ^ u32::from(b)) & 0xFF;
                for _ in 0..8 {
                    c = if c & 1 != 0 {
                        0xEDB8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
                crc = c ^ (crc >> 8);
            }
            !crc
        }
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let data: Vec<u8> = (0..300)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 32) as u8
            })
            .collect();
        for len in 0..=data.len() {
            assert_eq!(
                crc32(&data[..len]),
                reference(&data[..len]),
                "crc32 diverged at len {len}"
            );
        }
    }

    /// The straightforward per-output-pixel form `shrink_rows` was before the
    /// fixed-array rewrite. Kept as the oracle for
    /// [`shrink_rows_matches_reference_bit_for_bit`] — the fast paths must be
    /// byte-identical, not merely close.
    fn shrink_rows_reference(a: &[u8], b: &[u8], w: u32, bpp: usize, alpha: bool) -> Vec<u8> {
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

    /// Every (width, bpp, alpha) the sink can hand `shrink_rows` must produce
    /// exactly the reference bytes — including the odd-width replicated
    /// column, fully transparent blocks (`a_sum == 0`), and saturated alpha.
    #[test]
    fn shrink_rows_matches_reference_bit_for_bit() {
        // Deterministic pseudo-random bytes, plus the 0 / 255 corners that
        // exercise the alpha-weighted divide.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut byte = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            match state % 16 {
                0 => 0u8,
                1 => 255,
                _ => (state >> 24) as u8,
            }
        };
        // `opaque` forces every alpha byte to 255 so the RGBA8 fast path (which
        // is only reached when both rows are fully opaque) is exercised too.
        for &bpp in &[1usize, 2, 3, 4] {
            for alpha in [false, true] {
                for opaque in [false, true] {
                    for w in 1u32..=33 {
                        let n = w as usize * bpp;
                        let mut a: Vec<u8> = (0..n).map(|_| byte()).collect();
                        let mut b: Vec<u8> = (0..n).map(|_| byte()).collect();
                        if opaque && bpp == 4 {
                            for px in a.as_chunks_mut::<4>().0 {
                                px[3] = 255;
                            }
                            for px in b.as_chunks_mut::<4>().0 {
                                px[3] = 255;
                            }
                        }
                        assert_eq!(
                            shrink_rows(&a, &b, w, bpp, alpha),
                            shrink_rows_reference(&a, &b, w, bpp, alpha),
                            "shrink_rows diverged at w={w} bpp={bpp} alpha={alpha} opaque={opaque}"
                        );
                    }
                }
                // A wide row so the chunked path runs many iterations.
                let w = 1024u32;
                let n = w as usize * bpp;
                let a: Vec<u8> = (0..n).map(|_| byte()).collect();
                let b: Vec<u8> = (0..n).map(|_| byte()).collect();
                assert_eq!(
                    shrink_rows(&a, &b, w, bpp, alpha),
                    shrink_rows_reference(&a, &b, w, bpp, alpha),
                    "shrink_rows diverged at w=1024 bpp={bpp} alpha={alpha}"
                );
            }
        }
    }
}
