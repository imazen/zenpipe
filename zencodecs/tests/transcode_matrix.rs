//! N × M transcode matrix: every format zencodecs can both encode and decode,
//! as an opaque RGB8 source and (where the format carries alpha) a translucent
//! RGBA8 source, into every format it can encode, two ways:
//!
//! - one-shot: `DecodeRequest::decode_full_frame` then `EncodeRequest::encode`;
//! - streaming: `DecodeRequest::push_decode` into a `TranscodeSink` built from
//!   `EncodeRequest::build_streaming_encoder`, collecting what its writer gets.
//!
//! Both paths must succeed, and their outputs must decode to identical pixels
//! at the source's size: zencodec's cross-path contract (the testkit's
//! `check_cross_path_pixel_equivalence`) is pixel identity. Byte identity is
//! reported per cell; a one-shot encode may apply a lossless reduction it can
//! only choose after seeing the whole image (PNM writing RGB for opaque RGBA),
//! which a streaming encode cannot, so bytes may differ where pixels may not.
//! The matrix covers whatever codec features are enabled and prints its table.

// Needs at least two codecs; the CI run with `--no-default-features` has none.
#![cfg(all(feature = "jpeg", feature = "png"))]

use std::sync::{Arc, Mutex};

use zencodecs::{AllowedFormats, DecodeRequest, EncodeRequest, ImageFormat, TranscodeSink};
use zenpixels::{PixelDescriptor, PixelSlice};

/// Odd sizes, so chroma subsampling, block and strip edges are partial.
const W: u32 = 67;
const H: u32 = 45;

fn noise(x: u32, y: u32) -> u8 {
    let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    (h >> 24) as u8
}

/// A gradient with a little texture; `alpha` adds a translucent ramp and a
/// fully transparent block.
fn source_pixels(alpha: bool) -> (Vec<u8>, PixelDescriptor) {
    let bpp = if alpha { 4 } else { 3 };
    let mut px = Vec::with_capacity((W * H) as usize * bpp);
    for y in 0..H {
        for x in 0..W {
            px.push((x * 255 / W) as u8 ^ (noise(x, y) & 0x0F));
            px.push((y * 255 / H) as u8);
            px.push(((x + y) * 2) as u8);
            if alpha {
                let a = if (10..25).contains(&x) && (5..15).contains(&y) {
                    0
                } else {
                    (64 + (x * 191 / W)) as u8
                };
                px.push(a);
            }
        }
    }
    let desc = if alpha {
        PixelDescriptor::RGBA8_SRGB
    } else {
        PixelDescriptor::RGB8_SRGB
    };
    (px, desc)
}

fn encode_source(format: ImageFormat, alpha: bool) -> Result<Vec<u8>, String> {
    let (px, desc) = source_pixels(alpha);
    let slice = PixelSlice::new(&px, W, H, W as usize * desc.bytes_per_pixel(), desc)
        .map_err(|e| format!("{e:?}"))?;
    EncodeRequest::new(format)
        .with_quality(92.0)
        .encode(slice, alpha)
        .map(|o| o.into_vec())
        .map_err(|e| format!("{e}"))
}

fn one_shot(src: &[u8], target: ImageFormat) -> Result<Vec<u8>, String> {
    let dec = DecodeRequest::new(src)
        .decode_full_frame()
        .map_err(|e| format!("decode: {e}"))?;
    EncodeRequest::new(target)
        .with_quality(85.0)
        .encode(dec.pixels(), dec.has_alpha())
        .map(|o| o.into_vec())
        .map_err(|e| format!("encode: {e}"))
}

fn streaming(src: &[u8], target: ImageFormat) -> Result<Vec<u8>, String> {
    let se = EncodeRequest::new(target)
        .with_quality(85.0)
        .build_streaming_encoder(W, H)
        .map_err(|e| format!("build: {e}"))?;
    let got: Arc<Mutex<Vec<u8>>> = Arc::default();
    let sink_out = got.clone();
    let mut sink = TranscodeSink::new(se.encoder, se.supported).with_writer(move |b| {
        sink_out.lock().unwrap().extend_from_slice(b);
        Ok(())
    });
    DecodeRequest::new(src)
        .push_decode(&mut sink)
        .map_err(|e| format!("push_decode: {e}"))?;
    sink.finish_encode().map_err(|e| format!("finish: {e}"))?;
    let bytes = core::mem::take(&mut *got.lock().unwrap());
    Ok(bytes)
}

/// Decoded pixels in a comparable form: integer samples widened to RGBA16
/// (8-bit samples ×257, gray replicated, missing alpha opaque), so a lossless
/// reduction (RGBA to RGB, RGB to gray, 16 to 8 bits) compares equal to what
/// it reduced; float samples kept as exact RGBA f32 bit patterns.
#[derive(PartialEq)]
enum Pixels {
    Int(Vec<u16>),
    Float(Vec<u32>),
}

impl Pixels {
    fn differing(&self, other: &Pixels) -> usize {
        match (self, other) {
            (Pixels::Int(a), Pixels::Int(b)) => a.iter().zip(b).filter(|(x, y)| x != y).count(),
            (Pixels::Float(a), Pixels::Float(b)) => a.iter().zip(b).filter(|(x, y)| x != y).count(),
            _ => usize::MAX,
        }
    }
}

fn decoded_pixels(bytes: &[u8]) -> Result<Pixels, String> {
    use zenpixels::{ChannelLayout, ChannelType};
    let d = DecodeRequest::new(bytes)
        .decode_full_frame()
        .map_err(|e| format!("decode: {e}"))?;
    if (d.width(), d.height()) != (W, H) {
        return Err(format!("decodes at {}x{}", d.width(), d.height()));
    }
    let desc = d.descriptor();
    let px = d.pixels();
    let n = desc.layout().channels();
    let float = desc.channel_type() == ChannelType::F32;
    let (opaque, mut out) = if float {
        (1.0f32.to_bits(), Vec::with_capacity((W * H) as usize * 4))
    } else {
        (65535, Vec::with_capacity((W * H) as usize * 4))
    };
    for y in 0..H {
        let row = px.row(y);
        for x in 0..W as usize {
            let sample = |c: usize| -> Result<u32, String> {
                let i = x * n + c;
                match desc.channel_type() {
                    ChannelType::U8 => Ok(u32::from(row[i]) * 257),
                    ChannelType::U16 => {
                        Ok(u32::from(u16::from_ne_bytes([row[i * 2], row[i * 2 + 1]])))
                    }
                    ChannelType::F32 => Ok(u32::from_ne_bytes([
                        row[i * 4],
                        row[i * 4 + 1],
                        row[i * 4 + 2],
                        row[i * 4 + 3],
                    ])),
                    t => Err(format!("unhandled channel type {t:?}")),
                }
            };
            let rgba = match desc.layout() {
                ChannelLayout::Gray => [sample(0)?, sample(0)?, sample(0)?, opaque],
                ChannelLayout::GrayAlpha => [sample(0)?, sample(0)?, sample(0)?, sample(1)?],
                ChannelLayout::Rgb => [sample(0)?, sample(1)?, sample(2)?, opaque],
                ChannelLayout::Rgba => [sample(0)?, sample(1)?, sample(2)?, sample(3)?],
                ChannelLayout::Bgra => [sample(2)?, sample(1)?, sample(0)?, sample(3)?],
                l => return Err(format!("unhandled layout {l:?}")),
            };
            out.extend_from_slice(&rgba);
        }
    }
    Ok(if float {
        Pixels::Float(out)
    } else {
        Pixels::Int(out.into_iter().map(|v| v as u16).collect())
    })
}

#[derive(Debug)]
#[allow(dead_code)] // the fields are reported through `Debug`
enum Cell {
    /// Same bytes (so same pixels).
    Same(usize),
    /// Different bytes that decode to the same pixels.
    SamePixels {
        one_shot: usize,
        streaming: usize,
    },
    /// Different pixels: a contract violation.
    PixelsDiffer {
        one_shot: usize,
        streaming: usize,
        samples_differ: usize,
    },
    OneShotFailed(String),
    StreamingFailed(String),
    BothFailed(String, String),
    BadOutput(String),
}

#[test]
fn every_decodable_format_transcodes_to_every_encodable_format_both_ways() {
    let formats = AllowedFormats::all();
    let encodable: Vec<ImageFormat> = formats.encodable_formats().collect();
    let sources: Vec<ImageFormat> = encodable
        .iter()
        .copied()
        .filter(|f| formats.can_decode(*f))
        .collect();
    assert!(
        sources.len() >= 2,
        "the matrix needs at least two formats: {sources:?}"
    );

    let mut table: Vec<(String, ImageFormat, Cell)> = Vec::new();
    let mut src_failures = Vec::new();
    for &s in &sources {
        for alpha in [false, true] {
            if alpha && !s.supports_alpha() {
                continue;
            }
            let label = format!("{s:?}/{}", if alpha { "rgba8" } else { "rgb8" });
            let src = match encode_source(s, alpha) {
                Ok(b) => b,
                Err(e) => {
                    src_failures.push(format!("{label}: {e}"));
                    continue;
                }
            };
            for &t in &encodable {
                let cell = match (one_shot(&src, t), streaming(&src, t)) {
                    (Ok(a), Ok(b)) => match (decoded_pixels(&a), decoded_pixels(&b)) {
                        (Ok(_), Ok(_)) if a == b => Cell::Same(a.len()),
                        (Ok(pa), Ok(pb)) if pa == pb => Cell::SamePixels {
                            one_shot: a.len(),
                            streaming: b.len(),
                        },
                        (Ok(pa), Ok(pb)) => Cell::PixelsDiffer {
                            one_shot: a.len(),
                            streaming: b.len(),
                            samples_differ: pa.differing(&pb),
                        },
                        (Err(e), _) | (_, Err(e)) => Cell::BadOutput(e),
                    },
                    (Err(a), Ok(_)) => Cell::OneShotFailed(a),
                    (Ok(_), Err(b)) => Cell::StreamingFailed(b),
                    (Err(a), Err(b)) => Cell::BothFailed(a, b),
                };
                table.push((label.clone(), t, cell));
            }
        }
    }

    let bad: Vec<_> = table
        .iter()
        .filter(|(_, _, c)| !matches!(c, Cell::Same(_) | Cell::SamePixels { .. }))
        .collect();
    let same_bytes = table
        .iter()
        .filter(|(_, _, c)| matches!(c, Cell::Same(_)))
        .count();
    println!(
        "transcode matrix: {} sources x {} targets = {} cells: {} same bytes, {} same pixels only, {} failed",
        table
            .iter()
            .map(|r| &r.0)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        encodable.len(),
        table.len(),
        same_bytes,
        table.len() - same_bytes - bad.len(),
        bad.len()
    );
    for (s, t, c) in &table {
        println!("  {s:>14} -> {t:<8?} {c:?}");
    }
    assert!(
        src_failures.is_empty(),
        "could not build sources: {src_failures:#?}"
    );
    assert!(
        bad.is_empty(),
        "{} of {} cells failed: {bad:#?}",
        bad.len(),
        table.len()
    );
}
