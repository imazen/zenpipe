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
//!
//! The HDR-to-GIF cell explicitly clips to sRGB8 in both paths: RGBE can
//! reconstruct SDR white above 1.0, whereas GIF's native float input requires
//! finite SDR in 0..1. The separate regression below keeps native refusal
//! covered; clipping is a caller choice, not an encoder default.

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

#[derive(Clone, Copy)]
enum InputPolicy {
    Native,
    ClipToSrgb8,
}

impl InputPolicy {
    fn for_pair(source: ImageFormat, target: ImageFormat) -> Self {
        if source == ImageFormat::Hdr && target == ImageFormat::Gif {
            Self::ClipToSrgb8
        } else {
            Self::Native
        }
    }

    fn supported(self, native: &'static [PixelDescriptor]) -> &'static [PixelDescriptor] {
        match self {
            Self::Native => native,
            Self::ClipToSrgb8 => {
                assert!(native.contains(&PixelDescriptor::RGB8_SRGB));
                &[PixelDescriptor::RGB8_SRGB]
            }
        }
    }
}

fn one_shot(src: &[u8], target: ImageFormat, input_policy: InputPolicy) -> Result<Vec<u8>, String> {
    let dec = DecodeRequest::new(src)
        .decode_full_frame()
        .map_err(|e| format!("decode: {e}"))?;
    let encode = |pixels| {
        EncodeRequest::new(target)
            .with_quality(85.0)
            .encode(pixels, dec.has_alpha())
            .map(|o| o.into_vec())
            .map_err(|e| format!("encode: {e}"))
    };
    match input_policy {
        InputPolicy::Native => encode(dec.pixels()),
        InputPolicy::ClipToSrgb8 => {
            let pixels = dec.pixels();
            let bytes = pixels.contiguous_bytes();
            // Select the same conversion that TranscodeSink applies per strip.
            let adapted = zenpixels_convert::adapt::adapt_for_encode_cow(
                &bytes,
                pixels.descriptor(),
                pixels.width(),
                pixels.rows(),
                pixels.width() as usize * pixels.descriptor().bytes_per_pixel(),
                &[PixelDescriptor::RGB8_SRGB],
            )
            .map_err(|e| format!("clip to sRGB8: {e}"))?;
            encode(adapted.as_slice())
        }
    }
}

fn streaming(
    src: &[u8],
    target: ImageFormat,
    input_policy: InputPolicy,
) -> Result<Vec<u8>, String> {
    let se = EncodeRequest::new(target)
        .with_quality(85.0)
        .build_streaming_encoder(W, H)
        .map_err(|e| format!("build: {e}"))?;
    let got: Arc<Mutex<Vec<u8>>> = Arc::default();
    let sink_out = got.clone();
    let mut sink = TranscodeSink::new(se.encoder, input_policy.supported(se.supported))
        .with_writer(move |b| {
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

#[test]
#[cfg(all(feature = "bitmaps-hdr", feature = "gif"))]
fn hdr_white_to_gif_requires_explicit_clipping_in_both_paths() {
    let white = vec![255; (W * H * 3) as usize];
    let pixels = PixelSlice::new(&white, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap();
    let source = EncodeRequest::new(ImageFormat::Hdr)
        .encode(pixels, false)
        .unwrap();
    let decoded = DecodeRequest::new(source.data())
        .decode_full_frame()
        .unwrap();
    assert_eq!(
        decoded.descriptor().channel_type(),
        zenpixels::ChannelType::F32
    );
    assert!(matches!(
        decoded.descriptor().layout(),
        zenpixels::ChannelLayout::Rgb | zenpixels::ChannelLayout::Rgba
    ));
    let pixels = decoded.pixels();
    // The materialized decoder may add opaque alpha; inspect RGB separately.
    for y in 0..H {
        for pixel in pixels
            .row(y)
            .chunks_exact(decoded.descriptor().bytes_per_pixel())
        {
            for sample in pixel[..12].as_chunks::<4>().0 {
                let value = f32::from_ne_bytes(*sample);
                assert!(value.is_finite() && value > 1.0);
            }
            if decoded.descriptor().layout() == zenpixels::ChannelLayout::Rgba {
                assert_eq!(f32::from_ne_bytes(pixel[12..16].try_into().unwrap()), 1.0);
            }
        }
    }

    for result in [
        one_shot(source.data(), ImageFormat::Gif, InputPolicy::Native),
        streaming(source.data(), ImageFormat::Gif, InputPolicy::Native),
    ] {
        let error = result.expect_err("native GIF input must refuse over-range RGBE white");
        assert!(error.contains("finite SDR in 0..1"), "{error}");
    }

    for bytes in [
        one_shot(source.data(), ImageFormat::Gif, InputPolicy::ClipToSrgb8).unwrap(),
        streaming(source.data(), ImageFormat::Gif, InputPolicy::ClipToSrgb8).unwrap(),
    ] {
        #[cfg(feature = "local-fixtures")]
        {
            use std::io::Write;
            use std::process::{Command, Stdio};
            let mut child = Command::new("magick")
                .args(["GIF:-", "-depth", "8", "RGB:-"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("install ImageMagick for the explicit HDR/GIF oracle gate");
            child.stdin.take().unwrap().write_all(&bytes).unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout.len(), (W * H * 3) as usize);
            assert!(output.stdout.iter().all(|&v| v == 255));
        }
        match decoded_pixels(&bytes).unwrap() {
            Pixels::Int(samples) => assert!(samples.iter().all(|&v| v == 65535)),
            Pixels::Float(_) => panic!("GIF must decode to integer samples"),
        }
    }
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
                let input_policy = InputPolicy::for_pair(s, t);
                let cell = match (
                    one_shot(&src, t, input_policy),
                    streaming(&src, t, input_policy),
                ) {
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
