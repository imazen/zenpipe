//! Peak-memory probe for `TilePyramidSink` (zenpipe#24).
//!
//! Streams a synthetic `width × height` RGBA8 image (rows generated on the
//! fly — the source never holds the frame) through a DZI pyramid sink whose
//! writer only counts tiles, so the process RSS is the sink's own buffers
//! plus the runtime. Run under a memory profiler:
//!
//! ```text
//! cargo build --release --example tile_pyramid_mem
//! /usr/bin/time -l  target/release/examples/tile_pyramid_mem 40000 2000   # macOS: "maximum resident set size"
//! /usr/bin/time -v  target/release/examples/tile_pyramid_mem 40000 2000   # Linux: "Maximum resident set size"
//! heaptrack        target/release/examples/tile_pyramid_mem 40000 2000
//! ```
//!
//! Prints the tile count and `buffer_bytes_estimate()` so the measured
//! peak can be compared with the formula.

use zenpipe::sources::CallbackSource;
use zenpipe::tiles::{LevelInfo, TilePyramidConfig, TilePyramidSink, TileRef, TileWriter};
use zenpipe::{PipeResult, Source, execute, format};

struct Counting {
    tiles: u64,
    bytes: u64,
    levels: usize,
}

impl TileWriter for Counting {
    fn begin(
        &mut self,
        levels: &[LevelInfo],
        _config: TilePyramidConfig,
        _format: format::PixelFormat,
    ) -> PipeResult<()> {
        self.levels = levels.len();
        Ok(())
    }
    fn write_tile(&mut self, tile: TileRef<'_>) -> PipeResult<()> {
        self.tiles += 1;
        self.bytes += tile.data.len() as u64;
        // Touch the bytes so nothing is optimized away.
        self.bytes ^= u64::from(tile.data[0]) << 40;
        Ok(())
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let width: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let height: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1_000);
    let tile: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(254);

    let row_bytes = width as usize * 4;
    let mut y = 0u32;
    let mut src: Box<dyn Source> = Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if y >= height {
                return Ok(false);
            }
            for (x, px) in buf[..row_bytes]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .enumerate()
            {
                *px = [
                    (x as u32 * 7) as u8,
                    (y * 13) as u8,
                    ((x as u32 ^ y) * 3) as u8,
                    255,
                ];
            }
            y += 1;
            Ok(true)
        },
    ));
    let cfg = TilePyramidConfig {
        tile_size: tile,
        ..TilePyramidConfig::dzi()
    };
    let mut sink = TilePyramidSink::new(
        width,
        height,
        format::RGBA8_SRGB,
        cfg,
        Counting {
            tiles: 0,
            bytes: 0,
            levels: 0,
        },
    )
    .expect("sink");
    let estimate = sink.buffer_bytes_estimate();
    let t = std::time::Instant::now();
    execute(src.as_mut(), &mut sink).expect("pyramid");
    let secs = t.elapsed().as_secs_f64();
    let w = sink.into_writer();
    println!(
        "{width}x{height} RGBA8, tile {tile}: {} levels, {} tiles, {:.1} MB of tile pixels in {secs:.2}s; buffer_bytes_estimate = {:.1} MB",
        w.levels,
        w.tiles,
        (w.bytes & 0xFF_FFFF_FFFF) as f64 / 1e6,
        estimate as f64 / 1e6
    );
}
