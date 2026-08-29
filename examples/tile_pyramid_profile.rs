//! Allocation + wall-clock profile of the tile pyramid (zenpipe#24).
//!
//! Complements `examples/tile_pyramid_mem.rs` (which measures process RSS
//! only) by adding a counting global allocator, so a run reports both the
//! *peak live heap* the sink actually holds and the *allocation churn* it
//! generates — numbers `/usr/bin/time -l` cannot separate, and the portable
//! stand-in for heaptrack on non-Linux hosts.
//!
//! ```text
//! cargo build --release --example tile_pyramid_profile
//! target/release/examples/tile_pyramid_profile --width 40000 --height 1000 \
//!     --layout dzi --store null --threads 1 --encode raw
//! ```
//!
//! `--tsv-header` prints the column names; every run prints one TSV row, so
//! a grid is just a shell loop appending to one file.
//!
//! Numbers reported per run:
//! - `peak_live_mb` — high-water mark of `allocated - freed` (heap only,
//!   excludes the binary, stacks, and allocator slack; compare with RSS).
//! - `allocs` / `alloc_mb` — total allocation calls and bytes requested,
//!   i.e. churn. Two runs with the same peak but a 10× allocs gap differ in
//!   malloc pressure, not footprint.
//! - `wall_s` — end-to-end `execute()`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use zenpipe::sources::CallbackSource;
use zenpipe::tiles::{
    DziLayout, FsStore, GoogleMapsLayout, Iiif3Layout, LevelInfo, MemoryStore, PyramidWriter,
    TilePyramidConfig, TilePyramidSink, TileRef, TileStore, TileWriter, ZipStore, ZoomifyLayout,
};
use zenpipe::{PipeResult, Source, execute, format};

// ─── counting allocator ───

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counting;

impl Counting {
    fn record_alloc(size: usize) {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(size, Ordering::Relaxed);
        let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        PEAK.fetch_max(live, Ordering::Relaxed);
    }
}

// SAFETY-free: every method forwards to `System`; the counters are plain
// relaxed atomics that never allocate, so no reentrancy is possible.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            Self::record_alloc(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            Self::record_alloc(layout.size());
        }
        p
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            if new_size > layout.size() {
                let grew = new_size - layout.size();
                ALLOC_BYTES.fetch_add(grew, Ordering::Relaxed);
                let live = LIVE.fetch_add(grew, Ordering::Relaxed) + grew;
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn snapshot() -> (usize, usize, usize) {
    (
        ALLOCS.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
        PEAK.load(Ordering::Relaxed),
    )
}

/// Zero the churn counters and re-arm the peak at whatever is live now, so
/// setup (e.g. synthesizing a JPEG to decode) is excluded from the churn but
/// buffers still held across the run stay counted in the peak.
fn reset_counters() {
    ALLOCS.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
}

// ─── writers ───

/// Discards tiles after touching them: isolates the sink's own cost from
/// any store or encoder.
struct NullWriter {
    tiles: u64,
    checksum: u64,
    levels: usize,
}

impl TileWriter for NullWriter {
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
        self.checksum = self
            .checksum
            .wrapping_mul(31)
            .wrapping_add(u64::from(tile.data[0]));
        Ok(())
    }
}

/// Discards the *stored* bytes but runs layout naming: isolates encode cost
/// from store cost.
#[derive(Default)]
struct NullStore {
    puts: u64,
    bytes: u64,
}

impl TileStore for NullStore {
    fn put(&mut self, path: &str, bytes: &[u8]) -> PipeResult<()> {
        self.puts += 1;
        self.bytes += bytes.len() as u64 + path.len() as u64;
        Ok(())
    }
}

// ─── encoders ───

/// "Encode" = copy the tile with a tiny header. Measures the copy + Vec
/// allocation every real encoder also pays, without the codec.
fn raw_encode(t: TileRef<'_>) -> PipeResult<Vec<u8>> {
    let mut v = Vec::with_capacity(t.data.len() + 8);
    v.extend_from_slice(&t.width.to_le_bytes());
    v.extend_from_slice(&t.height.to_le_bytes());
    v.extend_from_slice(t.data);
    Ok(v)
}

/// Real JPEG encode at q80 4:2:0 — what a production DZI actually costs.
fn jpeg_encode(t: TileRef<'_>) -> PipeResult<Vec<u8>> {
    use enough::Unstoppable;
    use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout};
    let stride = t.width as usize * t.format.bytes_per_pixel();
    let mut enc = EncoderConfig::ycbcr(80.0, ChromaSubsampling::Quarter)
        .progressive(false)
        .optimize_huffman(false)
        .request()
        .encode_from_bytes(t.width, t.height, PixelLayout::Rgba8Srgb)
        .map_err(|e| zenpipe::PipeError::Op(format!("jpeg encoder: {e}")))?;
    enc.push(t.data, t.height as usize, stride, Unstoppable)
        .map_err(|e| zenpipe::PipeError::Op(format!("jpeg push: {e}")))?;
    Ok(enc
        .finish()
        .map_err(|e| zenpipe::PipeError::Op(format!("jpeg finish: {e}")))?)
}

// ─── driver ───

struct Args {
    width: u32,
    height: u32,
    tile: u32,
    layout: String,
    store: String,
    threads: usize,
    encode: String,
    source: String,
    /// Re-run the whole pyramid this many times (for `sample`/`dtrace`
    /// attach windows); only the last run's counters are printed.
    repeat: usize,
}

fn parse_args() -> Option<Args> {
    let mut a = Args {
        width: 10_000,
        height: 1_000,
        tile: 254,
        layout: "dzi".into(),
        store: "null".into(),
        threads: 1,
        encode: "raw".into(),
        source: "callback".into(),
        repeat: 1,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        if k == "--tsv-header" {
            println!(
                "width\theight\ttile\tlayout\tstore\tthreads\tencode\tsource\tlevels\ttiles\twall_s\tallocs\talloc_mb\tpeak_live_mb\testimate_mb"
            );
            return None;
        }
        let v = it.next().unwrap_or_else(|| panic!("{k} needs a value"));
        match k.as_str() {
            "--width" => a.width = v.parse().expect("width"),
            "--height" => a.height = v.parse().expect("height"),
            "--tile" => a.tile = v.parse().expect("tile"),
            "--layout" => a.layout = v,
            "--store" => a.store = v,
            "--threads" => a.threads = v.parse().expect("threads"),
            "--encode" => a.encode = v,
            "--source" => a.source = v,
            "--repeat" => a.repeat = v.parse().expect("repeat"),
            other => panic!("unknown flag {other}"),
        }
    }
    Some(a)
}

/// Build the source named by `--source`, then reset the churn counters so
/// only the pyramid pass is measured.
///
/// The four modes are the four *input classes* a tile pyramid sees:
/// - `callback` — a perfectly streaming source (the sink's own floor).
/// - `jpeg` — a real streaming codec (`zenjpeg`'s row-level decoder through
///   [`zenpipe::codec::DecoderSource`]): what JPEG / PNG / WebP / GIF cost.
/// - `materialized` — a whole decoded frame in RAM: what `job.rs` falls back
///   to for every codec whose `streaming_decoder` is `Unsupported`
///   (JXL, TIFF, AVIF, HEIC, RAW today).
/// - `spool` — the frame written to a `TempFileSource` and replayed: what an
///   analysis barrier costs when the decode itself cannot be repeated.
fn build_source(a: &Args) -> Box<dyn Source> {
    let src: Box<dyn Source> = match a.source.as_str() {
        "callback" => synthetic_source(a.width, a.height),
        "materialized" => {
            let mut data = vec![0u8; a.width as usize * a.height as usize * 4];
            fill_frame(&mut data, a.width, a.height);
            Box::new(zenpipe::sources::MaterializedSource::from_data(
                data,
                a.width,
                a.height,
                format::RGBA8_SRGB,
            ))
        }
        "spool" => {
            let inner = synthetic_source(a.width, a.height);
            let dir = scratch_dir();
            Box::new(
                zenpipe::sources::TempFileSource::from_source_in(&dir, inner, 16)
                    .expect("spool source"),
            )
        }
        "jpeg" => jpeg_source(a.width, a.height),
        other => panic!("unknown source {other}"),
    };
    reset_counters();
    src
}

fn fill_frame(data: &mut [u8], width: u32, height: u32) {
    let row_bytes = width as usize * 4;
    for y in 0..height {
        let row = &mut data[y as usize * row_bytes..(y as usize + 1) * row_bytes];
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            *px = [
                (x as u32 * 7) as u8,
                (y * 13) as u8,
                ((x as u32 ^ y) * 3) as u8,
                255,
            ];
        }
    }
}

/// Encode a synthetic JPEG (excluded from the measurement) and hand back a
/// row-level streaming decoder source over it.
fn jpeg_source(width: u32, height: u32) -> Box<dyn Source> {
    use enough::Unstoppable;
    use std::borrow::Cow;
    use zencodec::decode::{DecodeJob, DecoderConfig};
    use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout};
    use zenpixels::PixelDescriptor;

    let stride = width as usize * 4;
    let mut enc = EncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter)
        .progressive(false)
        .optimize_huffman(false)
        .request()
        .encode_from_bytes(width, height, PixelLayout::Rgba8Srgb)
        .expect("jpeg encoder");
    let strip_h = 16u32;
    let mut strip = vec![0u8; stride * strip_h as usize];
    let mut y = 0u32;
    while y < height {
        let rows = strip_h.min(height - y);
        for r in 0..rows {
            let row = &mut strip[r as usize * stride..(r as usize + 1) * stride];
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                *px = [
                    (x as u32 * 7) as u8,
                    ((y + r) * 13) as u8,
                    ((x as u32 ^ (y + r)) * 3) as u8,
                    255,
                ];
            }
        }
        enc.push(
            &strip[..rows as usize * stride],
            rows as usize,
            stride,
            Unstoppable,
        )
        .expect("jpeg push");
        y += rows;
    }
    let bytes = enc.finish().expect("jpeg finish");
    let decoder = zenjpeg::JpegDecoderConfig::default()
        .job()
        .dyn_streaming_decoder(Cow::Owned(bytes), &[PixelDescriptor::RGBA8_SRGB])
        .expect("jpeg streaming decoder");
    Box::new(zenpipe::codec::DecoderSource::new(decoder).expect("DecoderSource"))
}

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::var("TILE_PROFILE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("tile_pyramid_profile"));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Rows generated on the fly — the source never holds the frame, so every
/// byte measured belongs to the sink.
fn synthetic_source(width: u32, height: u32) -> Box<dyn Source> {
    let row_bytes = width as usize * 4;
    let mut y = 0u32;
    Box::new(CallbackSource::new(
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
    ))
}

fn config_for(layout: &str, tile: u32) -> TilePyramidConfig {
    let base = match layout {
        "dzi" => TilePyramidConfig::dzi(),
        "iiif" => TilePyramidConfig::iiif(),
        "zoomify" => TilePyramidConfig::zoomify(),
        "gmaps" => TilePyramidConfig::google_maps([0, 0, 0, 255]),
        other => panic!("unknown layout {other}"),
    };
    if tile == 0 {
        // `--tile 0` keeps each layout's own convention (DZI 254/1, IIIF
        // 512/0, Zoomify and Google Maps 256/0) — the only apples-to-apples
        // way to compare layouts.
        base
    } else {
        TilePyramidConfig {
            tile_size: tile,
            ..base
        }
    }
}

/// Run the pyramid and return `(levels, tiles, wall_seconds)`.
fn run<W: TileWriter>(a: &Args, sink: &mut TilePyramidSink<W>) -> (u32, std::time::Duration) {
    let mut src = build_source(a);
    let levels = sink.level_count();
    let t = std::time::Instant::now();
    execute(src.as_mut(), sink).expect("pyramid");
    (levels, t.elapsed())
}

fn emit(a: &Args, levels: u32, tiles: u64, wall: std::time::Duration, estimate: u64) {
    let (allocs, alloc_bytes, peak) = snapshot();
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{levels}\t{tiles}\t{:.3}\t{allocs}\t{:.1}\t{:.1}\t{:.1}",
        a.width,
        a.height,
        a.tile,
        a.layout,
        a.store,
        a.threads,
        a.encode,
        a.source,
        wall.as_secs_f64(),
        alloc_bytes as f64 / 1e6,
        peak as f64 / 1e6,
        estimate as f64 / 1e6,
    );
}

fn with_store<S: TileStore + 'static>(a: &Args, store: S) {
    let cfg = config_for(&a.layout, a.tile);
    let enc: fn(TileRef<'_>) -> PipeResult<Vec<u8>> = match a.encode.as_str() {
        "raw" => raw_encode,
        "jpeg" => jpeg_encode,
        other => panic!("unknown encode {other}"),
    };
    macro_rules! go {
        ($layout:expr) => {{
            let writer = PyramidWriter::new($layout, store, enc).with_threads(a.threads);
            let mut sink = TilePyramidSink::with_pyramid_writer(
                a.width,
                a.height,
                format::RGBA8_SRGB,
                cfg,
                writer,
            )
            .expect("sink");
            let estimate = sink.buffer_bytes_estimate();
            let (levels, wall) = run(a, &mut sink);
            let w = sink.into_writer();
            emit(a, levels, w.tiles_written(), wall, estimate);
        }};
    }
    match a.layout.as_str() {
        "dzi" => go!(DziLayout::new("img", "bin")),
        "iiif" => go!(Iiif3Layout::new("img", "bin")),
        "zoomify" => go!(ZoomifyLayout::new("bin")),
        "gmaps" => go!(GoogleMapsLayout::new("bin")),
        other => panic!("unknown layout {other}"),
    }
}

fn main() {
    let Some(a) = parse_args() else { return };
    for _ in 1..a.repeat {
        run_once(&a);
        reset_counters();
    }
    run_once(&a);
}

fn run_once(a: &Args) {
    if a.store == "sink-only" {
        // No layout / store / encoder at all: the sink's own floor.
        let cfg = config_for(&a.layout, a.tile);
        let mut sink = TilePyramidSink::new(
            a.width,
            a.height,
            format::RGBA8_SRGB,
            cfg,
            NullWriter {
                tiles: 0,
                checksum: 0,
                levels: 0,
            },
        )
        .expect("sink");
        let estimate = sink.buffer_bytes_estimate();
        let (levels, wall) = run(a, &mut sink);
        let w = sink.into_writer();
        std::hint::black_box(w.checksum);
        emit(a, levels, w.tiles, wall, estimate);
        return;
    }

    match a.store.as_str() {
        "null" => with_store(a, NullStore::default()),
        "mem" => with_store(a, MemoryStore::default()),
        "fs" => {
            let dir = std::env::var("TILE_PROFILE_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir().join("tile_pyramid_profile"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            with_store(a, FsStore::new(&dir));
        }
        "zip" => {
            let dir = std::env::var("TILE_PROFILE_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::env::temp_dir().join("tile_pyramid_profile"));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            let f = std::fs::File::create(dir.join("pyramid.zip")).expect("zip file");
            with_store(a, ZipStore::new(std::io::BufWriter::new(f)));
        }
        other => panic!("unknown store {other}"),
    }
}
