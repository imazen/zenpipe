//! Benchmark: Session caching speedup for interactive filter editing.
//!
//! Simulates an editor workflow:
//! 1. Decode a 4K JPEG
//! 2. Crop + resize (geometry prefix)
//! 3. Apply a filter (remove_alpha with varying matte — the suffix)
//! 4. Materialize and encode to BMP
//!
//! Compares full-pipeline execution vs session-cached suffix-only execution.
//!
//! Run: cargo bench --bench session_cache --features "zennode,std"

use std::borrow::Cow;
use std::sync::LazyLock;

use enough::Unstoppable;
use zenbench::Throughput;
use zenbench::black_box;
use zencodec::decode::{DecodeJob, DecoderConfig};
use zenjpeg::JpegDecoderConfig;
use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout};
use zenpixels::PixelDescriptor;

use zenpipe::Source;
use zenpipe::codec::DecoderSource;
use zenpipe::format::RGBA8_SRGB;
use zenpipe::orchestrate::{ProcessConfig, SourceImageInfo};
use zenpipe::session::Session;
use zenpipe::sources::MaterializedSource;

const SRC_W: u32 = 3840;
const SRC_H: u32 = 2160;

// ─── JPEG generation ───

/// 4K gradient JPEG, generated once and shared across all benchmarks.
static JPEG_4K: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let bpp = 4usize;
    let stride = SRC_W as usize * bpp;

    let config = EncoderConfig::ycbcr(85.0, ChromaSubsampling::Quarter)
        .progressive(false)
        .optimize_huffman(false);
    let mut enc = config
        .request()
        .encode_from_bytes(SRC_W, SRC_H, PixelLayout::Rgba8Srgb)
        .expect("encoder creation");

    let strip_h = 16u32;
    let mut strip_buf = vec![0u8; stride * strip_h as usize];

    let mut y = 0u32;
    while y < SRC_H {
        let rows = strip_h.min(SRC_H - y);
        for r in 0..rows {
            let row_start = r as usize * stride;
            for x in 0..SRC_W {
                let px = row_start + x as usize * bpp;
                strip_buf[px] = (x * 255 / SRC_W) as u8;
                strip_buf[px + 1] = ((y + r) * 255 / SRC_H) as u8;
                strip_buf[px + 2] = 128;
                strip_buf[px + 3] = 255;
            }
        }
        enc.push(
            &strip_buf[..rows as usize * stride],
            rows as usize,
            stride,
            Unstoppable,
        )
        .expect("push rows");
        y += rows;
    }

    enc.finish().expect("finish encode")
});

// ─── Helpers ───

fn decode_jpeg() -> Box<dyn Source> {
    let dec_config = JpegDecoderConfig::default();
    let job = dec_config.job();
    let decoder = job
        .dyn_streaming_decoder(
            Cow::Borrowed(JPEG_4K.as_slice()),
            &[PixelDescriptor::RGBA8_SRGB],
        )
        .expect("streaming decoder");
    Box::new(DecoderSource::new(decoder).expect("DecoderSource"))
}

fn source_info() -> SourceImageInfo {
    SourceImageInfo {
        width: SRC_W,
        height: SRC_H,
        format: RGBA8_SRGB,
        has_alpha: true,
        has_animation: false,
        has_gain_map: false,
        is_hdr: false,
        exif_orientation: 1,
        metadata: None,
    }
}

/// Geometry nodes: crop center 2560x1440, then resize to 1280x720.
fn geometry_nodes() -> Vec<Box<dyn zennode::NodeInstance>> {
    vec![
        Box::new(zenpipe::zennode_defs::Crop {
            x: 640,
            y: 360,
            w: 2560,
            h: 1440,
        }),
        Box::new(zenpipe::zennode_defs::Constrain {
            w: Some(1280),
            h: Some(720),
            mode: "fit".into(),
            ..Default::default()
        }),
    ]
}

/// Filter node: remove alpha with a specific matte color.
fn filter_node(r: u32, g: u32, b: u32) -> Box<dyn zennode::NodeInstance> {
    Box::new(zenpipe::zennode_defs::RemoveAlpha {
        matte_r: r,
        matte_g: g,
        matte_b: b,
    })
}

/// Drain a Source to MaterializedSource, then encode to BMP.
fn materialize_and_encode_bmp(source: Box<dyn Source>) -> Vec<u8> {
    let mat = MaterializedSource::from_source(source).expect("materialize");
    let w = mat.width();
    let h = mat.height();
    let fmt = mat.format();
    let data = mat.data();
    let bpp = fmt.bytes_per_pixel();
    let row_bytes = w as usize * bpp;
    // Strip alignment padding: BMP expects packed rows.
    let packed: Vec<u8> = (0..h)
        .flat_map(|y| &data[y as usize * mat.stride()..y as usize * mat.stride() + row_bytes])
        .copied()
        .collect();
    let layout = if bpp == 4 {
        zenbitmaps::PixelLayout::Rgba8
    } else {
        zenbitmaps::PixelLayout::Rgb8
    };
    if bpp == 4 {
        zenbitmaps::encode_bmp_rgba(&packed, w, h, layout, Unstoppable).expect("BMP encode")
    } else {
        zenbitmaps::encode_bmp(&packed, w, h, layout, Unstoppable).expect("BMP encode")
    }
}

/// Drain a Source fully (measure pipeline cost without BMP overhead).
fn drain(mut source: Box<dyn Source>) -> u32 {
    let mut total = 0u32;
    while let Some(strip) = source.next().expect("next") {
        total += strip.rows();
    }
    total
}

// ─── Benchmarks ───

zenbench::main!(|suite| {
    // Force JPEG generation before benchmarks start.
    eprintln!(
        "Generated 4K JPEG: {} KB ({SRC_W}x{SRC_H})",
        JPEG_4K.len() / 1024
    );

    // ─── Group 1: Full pipeline vs cached, drain only ───
    suite.compare("session_4k_drain", |group| {
        group.throughput(Throughput::Elements(1));
        group.throughput_unit("images");

        // Baseline: full pipeline every time (decode + crop + resize + remove_alpha).
        group.bench("full_pipeline", |b| {
            b.with_input(|| {
                let mut nodes = geometry_nodes();
                nodes.push(filter_node(255, 255, 255));
                nodes
            })
            .run(|nodes| {
                let source = decode_jpeg();
                let info = source_info();
                let config = ProcessConfig {
                    nodes: &nodes,
                    converters: &[],
                    hdr_mode: "sdr_only",
                    source_info: &info,
                    trace_config: None,
                };
                let output = zenpipe::orchestrate::stream(source, &config, None).expect("stream");
                black_box(drain(output.source))
            })
        });

        // Session-cached: first call is full, subsequent calls hit cache.
        // We prime the session outside the timed region, then measure
        // only the cached suffix execution.
        group.bench("cached_suffix", |b| {
            let mut session = Session::new(256 * 1024 * 1024);
            {
                let mut nodes = geometry_nodes();
                nodes.push(filter_node(255, 255, 255));
                let info = source_info();
                let config = ProcessConfig {
                    nodes: &nodes,
                    converters: &[],
                    hdr_mode: "sdr_only",
                    source_info: &info,
                    trace_config: None,
                };
                let source = decode_jpeg();
                let output = session
                    .stream(source, &config, None, 0xBE4C)
                    .expect("prime");
                drain(output.source);
            }
            assert_eq!(session.cache_len(), 1, "session should have 1 cached entry");

            let mut iter_counter = 0u32;
            b.iter(|| {
                iter_counter = iter_counter.wrapping_add(1);
                // Vary matte color each iteration — different suffix, same geometry.
                let r = (iter_counter * 37) & 0xFF;
                let g = (iter_counter * 73) & 0xFF;
                let b_val = (iter_counter * 113) & 0xFF;

                let mut nodes = geometry_nodes();
                nodes.push(filter_node(r, g, b_val));

                let info = source_info();
                let config = ProcessConfig {
                    nodes: &nodes,
                    converters: &[],
                    hdr_mode: "sdr_only",
                    source_info: &info,
                    trace_config: None,
                };

                // Source not needed for cache hit, but API requires one.
                let source = decode_jpeg();
                let output = session
                    .stream(source, &config, None, 0xBE4C)
                    .expect("cached stream");
                black_box(drain(output.source))
            })
        });
    });

    // ─── Group 2: Full pipeline vs cached, with BMP encode ───
    suite.compare("session_4k_bmp", |group| {
        group.throughput(Throughput::Elements(1));
        group.throughput_unit("images");

        // Full pipeline + BMP encode.
        group.bench("full_pipeline_bmp", |b| {
            b.with_input(|| {
                let mut nodes = geometry_nodes();
                nodes.push(filter_node(255, 255, 255));
                nodes
            })
            .run(|nodes| {
                let source = decode_jpeg();
                let info = source_info();
                let config = ProcessConfig {
                    nodes: &nodes,
                    converters: &[],
                    hdr_mode: "sdr_only",
                    source_info: &info,
                    trace_config: None,
                };
                let output = zenpipe::orchestrate::stream(source, &config, None).expect("stream");
                black_box(materialize_and_encode_bmp(output.source).len())
            })
        });

        // Session-cached + BMP encode.
        group.bench("cached_suffix_bmp", |b| {
            let mut session = Session::new(256 * 1024 * 1024);
            {
                let mut nodes = geometry_nodes();
                nodes.push(filter_node(255, 255, 255));
                let info = source_info();
                let config = ProcessConfig {
                    nodes: &nodes,
                    converters: &[],
                    hdr_mode: "sdr_only",
                    source_info: &info,
                    trace_config: None,
                };
                let source = decode_jpeg();
                let output = session
                    .stream(source, &config, None, 0xBE4C)
                    .expect("prime");
                drain(output.source);
            }

            let mut iter_counter = 0u32;
            b.iter(|| {
                iter_counter = iter_counter.wrapping_add(1);
                let r = (iter_counter * 37) & 0xFF;
                let g = (iter_counter * 73) & 0xFF;
                let b_val = (iter_counter * 113) & 0xFF;

                let mut nodes = geometry_nodes();
                nodes.push(filter_node(r, g, b_val));

                let info = source_info();
                let config = ProcessConfig {
                    nodes: &nodes,
                    converters: &[],
                    hdr_mode: "sdr_only",
                    source_info: &info,
                    trace_config: None,
                };

                let source = decode_jpeg();
                let output = session
                    .stream(source, &config, None, 0xBE4C)
                    .expect("cached stream");
                black_box(materialize_and_encode_bmp(output.source).len())
            })
        });
    });

    // ─── Group 3: 10 filter permutations (amortized session benefit) ───
    suite.compare("session_multi_filter", |group| {
        group.throughput(Throughput::Elements(10));
        group.throughput_unit("images");

        // 10 different filter configs, no session — full pipeline each time.
        group.bench("10x_full", |b| {
            b.iter(|| {
                let mut total = 0u32;
                for i in 0..10u32 {
                    let mut nodes = geometry_nodes();
                    nodes.push(filter_node(i * 25, 128, 255 - i * 25));

                    let info = source_info();
                    let config = ProcessConfig {
                        nodes: &nodes,
                        converters: &[],
                        hdr_mode: "sdr_only",
                        source_info: &info,
                        trace_config: None,
                    };
                    let source = decode_jpeg();
                    let output =
                        zenpipe::orchestrate::stream(source, &config, None).expect("stream");
                    total += drain(output.source);
                }
                black_box(total)
            })
        });

        // 10 different filter configs with session — first is full, rest are cached.
        group.bench("10x_cached", |b| {
            b.iter(|| {
                let mut session = Session::new(256 * 1024 * 1024);
                let mut total = 0u32;
                for i in 0..10u32 {
                    let mut nodes = geometry_nodes();
                    nodes.push(filter_node(i * 25, 128, 255 - i * 25));

                    let info = source_info();
                    let config = ProcessConfig {
                        nodes: &nodes,
                        converters: &[],
                        hdr_mode: "sdr_only",
                        source_info: &info,
                        trace_config: None,
                    };
                    let source = decode_jpeg();
                    let output = session
                        .stream(source, &config, None, 0xBE4C)
                        .expect("stream");
                    total += drain(output.source);
                }
                black_box(total)
            })
        });
    });
});
