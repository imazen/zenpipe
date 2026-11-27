//! Byte identity at the dispatched encoder/sink boundary on seeded RGB
//! and translucent RGBA input. Draining must never affect codec output.
#![cfg(all(feature = "jpeg", feature = "png"))]

use std::sync::{Arc, Mutex};
use zencodec::decode::{DecodeRowSink, RowStrip};
use zencodecs::{AllowedFormats, EncodeRequest, ImageFormat, TranscodeSink};
use zenpixels::{ColorOrigin, PeakSource, PixelFormat, PixelView};
use zenpixels_convert::{ConvertPolicy, OutputSignaling, adapt_for_encode};

const W: u32 = 67;
const H: u32 = 93;

fn pixels(alpha: bool) -> (Vec<u8>, PixelFormat) {
    let format = if alpha {
        PixelFormat::RGBA8_SRGB
    } else {
        PixelFormat::RGB8_SRGB
    };
    let bpp = format.layout.bytes_per_pixel();
    // Padding distinguishes row iteration from accidentally copying contiguous bytes.
    let stride = (W as usize + 3) * bpp;
    let mut data = vec![0xCD; stride * H as usize];
    let mut state = 0xD0CA_F00Du32;
    for y in 0..H as usize {
        for x in 0..W as usize {
            for c in 0..bpp {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                data[y * stride + x * bpp + c] = (state >> 24) as u8;
            }
        }
    }
    (data, format)
}

#[derive(Clone, Copy, Debug)]
enum Drain {
    Never,
    Writer,
    Uneven,
    Take,
}

fn streamed(
    format: ImageFormat,
    pixels: PixelView<'_>,
    mode: Drain,
) -> (Vec<u8>, zencodec::encode::OutputTiming) {
    let encoder = EncodeRequest::new(format)
        .with_quality(85.0)
        .build_streaming_encoder(W, H)
        .unwrap();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let mut sink = TranscodeSink::new(encoder.encoder, encoder.supported);
    if matches!(mode, Drain::Writer) {
        let out = writes.clone();
        sink = sink.with_writer(move |b| {
            out.lock().unwrap().extend_from_slice(b);
            Ok(())
        });
    }
    sink.begin(W, H, pixels.format().clone()).unwrap();
    let timing = sink.output_timing().unwrap();
    let mut output = Vec::new();
    let mut y = 0;
    let mut index = 0;
    while y < H {
        let rows = [1, 7, 3, 16][index % 4].min(H - y);
        let mut next = sink
            .provide_next_buffer(RowStrip::new(y, W, rows), pixels.format().clone())
            .unwrap();
        for r in 0..rows {
            next.row_mut(r).copy_from_slice(pixels.row(y + r));
        }
        y += rows;
        match mode {
            Drain::Uneven => {
                let n = [0, 1, 7, usize::MAX][index % 4].min(sink.pending_output().len());
                output.extend_from_slice(&sink.pending_output()[..n]);
                sink.consume_output(n);
            }
            Drain::Take => {
                if index % 2 == 0 {
                    let mut chunk = Vec::new();
                    sink.take_pending_output(&mut chunk);
                    output.extend_from_slice(&chunk);
                } else {
                    sink.take_pending_output(&mut output);
                }
            }
            _ => {}
        }
        index += 1;
    }
    sink.finish().unwrap();
    if matches!(mode, Drain::Uneven) {
        output.extend_from_slice(sink.pending_output());
        sink.consume_output(usize::MAX);
    } else if matches!(mode, Drain::Take) {
        sink.take_pending_output(&mut output);
    }
    let tail = sink.finish_encode().unwrap();
    if matches!(mode, Drain::Writer) {
        assert!(tail.is_empty());
        output = writes.lock().unwrap().clone();
    } else {
        output.extend_from_slice(tail.data());
    }
    (output, timing)
}

#[test]
fn every_dispatched_codec_keeps_bytes_across_sink_drains() {
    let mut failures = Vec::new();
    let mut saw_streaming = false;
    let mut saw_at_finish = false;
    for format in AllowedFormats::all().encodable_formats() {
        for alpha in [false, true] {
            if alpha && !format.supports_alpha() {
                continue;
            }
            let (data, desc) = pixels(alpha);
            let stride = (W as usize + 3) * desc.layout.bytes_per_pixel();
            let pixels = PixelView::new(&data, W, H, stride, &desc).unwrap();
            let one = EncodeRequest::new(format)
                .with_quality(85.0)
                .encode(pixels.clone(), alpha)
                .unwrap();
            let (undrained, timing) = streamed(format, pixels.clone(), Drain::Never);
            saw_streaming |= timing.streams();
            saw_at_finish |= !timing.streams();
            if undrained != one.data() {
                failures.push(format!(
                    "{format:?}/{desc:?}: one-shot differs from undrained begin/push/finish"
                ));
            }
            for mode in [Drain::Writer, Drain::Uneven, Drain::Take] {
                let (got, drained_timing) = streamed(format, pixels.clone(), mode);
                assert_eq!(drained_timing, timing);
                assert_eq!(
                    got, undrained,
                    "{format:?}/{desc:?}/{mode:?}: draining changed bytes"
                );
            }
            println!(
                "{format:?}/{desc:?}: {} bytes; one-shot identical={}",
                undrained.len(),
                undrained == one.data()
            );
        }
    }
    assert!(
        saw_streaming && saw_at_finish,
        "exercise both output timings"
    );
    assert!(
        failures.is_empty(),
        "one-shot byte identity failures: {failures:#?}"
    );
}

/// This gate is opt-in through `just test-dispatched-oracle`; missing tools
/// fail instead of skipping, and the ordinary byte gate has no tool dependency.
#[cfg(feature = "local-fixtures")]
#[test]
fn independent_decoders_read_every_streamed_format() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    fn run(program: &str, args: &[&str], bytes: &[u8]) -> Vec<u8> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("install the oracle required by this explicit local-fixtures gate");
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{program}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }
    for format in AllowedFormats::all().encodable_formats() {
        for alpha in [false, true] {
            if alpha && !format.supports_alpha() {
                continue;
            }
            let (data, desc) = pixels(alpha);
            let stride = (W as usize + 3) * desc.layout.bytes_per_pixel();
            let pixels = PixelView::new(&data, W, H, stride, &desc).unwrap();
            let (encoded, _) = streamed(format, pixels, Drain::Writer);
            let (coder, bytes) = match format {
                ImageFormat::Jxl => (
                    "PNM",
                    run(
                        "djxl",
                        &["-", "-", "--output_format", "ppm", "--quiet"],
                        &encoded,
                    ),
                ),
                ImageFormat::Jpeg => ("JPEG", encoded),
                ImageFormat::WebP => ("WEBP", encoded),
                ImageFormat::Gif => ("GIF", encoded),
                ImageFormat::Png => ("PNG", encoded),
                ImageFormat::Avif => ("AVIF", encoded),
                ImageFormat::Pnm => ("PNM", encoded),
                ImageFormat::Bmp => ("BMP", encoded),
                ImageFormat::Farbfeld => ("FARBFELD", encoded),
                ImageFormat::Tiff => ("TIFF", encoded),
                ImageFormat::Qoi => ("QOI", encoded),
                ImageFormat::Tga => ("TGA", encoded),
                ImageFormat::Hdr => ("HDR", encoded),
                other => panic!("add the independent decoder for {other:?}"),
            };
            let input = format!("{coder}:-");
            let dimensions = run("magick", &[&input, "-format", "%w %h", "info:"], &bytes);
            assert_eq!(String::from_utf8_lossy(&dimensions), format!("{W} {H}"));
            println!("independent decode: {format:?}/{desc:?}");
        }
    }
}

#[test]
fn output_limit_includes_bytes_already_sent_for_every_codec() {
    use zencodec::encode::{BoxedError, EncodeOutput};
    for format in AllowedFormats::all().encodable_formats() {
        let (data, descriptor) = pixels(false);
        let stride = (W as usize + 3) * descriptor.layout.bytes_per_pixel();
        let pixels = PixelView::new(&data, W, H, stride, &descriptor).unwrap();
        let base = zencodecs::Limits::none();
        let want = EncodeRequest::new(format)
            .with_quality(85.0)
            .with_limits(&base)
            .encode(pixels.clone(), false)
            .unwrap();
        for cap in [want.len(), want.len() - 1] {
            let limits = zencodecs::Limits::none().with_max_output(cap as u64);
            let se = EncodeRequest::new(format)
                .with_quality(85.0)
                .with_limits(&limits)
                .build_streaming_encoder(W, H)
                .unwrap();
            let bytes = Arc::new(Mutex::new(Vec::new()));
            let out = bytes.clone();
            let mut sink = TranscodeSink::new(se.encoder, se.supported).with_writer(move |b| {
                out.lock().unwrap().extend_from_slice(b);
                Ok(())
            });
            let result = (|| -> Result<EncodeOutput, BoxedError> {
                sink.begin(W, H, descriptor.clone())?;
                for y in (0..H).step_by(7) {
                    let rows = 7.min(H - y);
                    let mut next =
                        sink.provide_next_buffer(RowStrip::new(y, W, rows), descriptor.clone())?;
                    for r in 0..rows {
                        next.row_mut(r).copy_from_slice(pixels.row(y + r));
                    }
                }
                sink.finish()?;
                sink.finish_encode()
            })();
            assert!(
                bytes.lock().unwrap().len() <= cap,
                "{format:?} exceeded the transport byte limit"
            );
            if cap == want.len() {
                assert!(
                    result.is_ok(),
                    "{format:?} rejected an exact whole-file budget: {result:?}"
                );
                assert_eq!(
                    *bytes.lock().unwrap(),
                    want.data(),
                    "{format:?} changed bytes under the limit"
                );
            } else {
                assert!(
                    result.is_err(),
                    "{format:?} completed a file larger than the budget"
                );
            }
        }
    }
}

#[test]
fn tiny_output_budgets_allow_early_refusal_without_excess_writes() {
    use zencodec::encode::{BoxedError, EncodeOutput};
    for format in AllowedFormats::all().encodable_formats() {
        let (data, descriptor) = pixels(false);
        let stride = (W as usize + 3) * descriptor.layout.bytes_per_pixel();
        let pixels = PixelView::new(&data, W, H, stride, &descriptor).unwrap();
        let full = EncodeRequest::new(format)
            .with_quality(85.0)
            .encode(pixels.clone(), false)
            .unwrap();
        assert!(full.len() > 7, "{format:?} fixture fits a tiny budget");
        for cap in [0, 1, 7] {
            let limits = zencodecs::Limits::none().with_max_output(cap as u64);
            let bytes = Arc::new(Mutex::new(Vec::new()));
            let out = bytes.clone();
            let result = (|| -> Result<EncodeOutput, BoxedError> {
                let se = EncodeRequest::new(format)
                    .with_quality(85.0)
                    .with_limits(&limits)
                    .build_streaming_encoder(W, H)?;
                let mut sink = TranscodeSink::new(se.encoder, se.supported).with_writer(move |b| {
                    out.lock().unwrap().extend_from_slice(b);
                    Ok(())
                });
                sink.begin(W, H, descriptor.clone())?;
                for y in (0..H).step_by(7) {
                    let rows = 7.min(H - y);
                    let mut next =
                        sink.provide_next_buffer(RowStrip::new(y, W, rows), descriptor.clone())?;
                    for r in 0..rows {
                        next.row_mut(r).copy_from_slice(pixels.row(y + r));
                    }
                }
                sink.finish()?;
                sink.finish_encode()
            })();
            assert!(
                result.is_err(),
                "{format:?} completed with a {cap}-byte budget"
            );
            assert!(
                bytes.lock().unwrap().len() <= cap,
                "{format:?} exceeded its {cap}-byte transport budget"
            );
        }
    }
}

#[test]
fn empty_canvas_declaration_matches_the_native_one_shot_path() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    let mut failures = Vec::new();
    for format in AllowedFormats::all().encodable_formats() {
        let make = || {
            EncodeRequest::new(format)
                .build_streaming_encoder(W, H)
                .unwrap()
        };
        let supported = make().supported;
        let sample = PixelView::new(&[0, 0, 0], 1, 1, 3, &zenpixels::formats::RGB8_SRGB).unwrap();
        let (_, adapted) = adapt_for_encode(
            sample,
            &ColorOrigin::assumed(),
            supported,
            &ConvertPolicy::permissive(),
            PeakSource::Declared,
            OutputSignaling::Strip,
        )
        .unwrap();
        let descriptor = adapted.pixels.as_view().format().clone();
        for (width, height) in [(0, H), (W, 0)] {
            // zenpixels 0.5 forbids zero-dimension views at construction time,
            // so one-shot callers cannot pass an empty canvas; verify both
            // that PixelView::new rejects it and that streaming `begin` rejects
            // empty canvas declarations cleanly with typed InvalidInput without
            // leaving the encoder unusable.
            let empty_err = PixelView::new(
                &[],
                width,
                height,
                width as usize * descriptor.layout.bytes_per_pixel(),
                &descriptor,
            )
            .expect_err("zenpixels 0.5 must reject zero-dimension PixelView");
            assert_eq!(
                *empty_err.error(),
                zenpixels::BufferError::InvalidDimensions,
                "{format:?} {width}x{height}: unexpected PixelView error"
            );
            let mut enc = make().encoder;
            let begin = catch_unwind(AssertUnwindSafe(|| {
                enc.begin(width, height, descriptor.clone())
            }));
            match begin {
                Ok(Err(begin_err)) => {
                    let begin_err = begin_err
                        .downcast_ref::<zencodecs::CodecError>()
                        .expect("begin refusal retains its typed validation error");
                    assert!(
                        matches!(begin_err, zencodecs::CodecError::InvalidInput(_)),
                        "{format:?} {width}x{height}: expected InvalidInput, got {begin_err:?}"
                    );
                    assert!(
                        enc.begin(W, H, descriptor.clone()).is_ok(),
                        "failed begin changed {format:?}"
                    );
                }
                Ok(Ok(timing)) => {
                    assert_eq!(timing, zencodec::encode::OutputTiming::AT_FINISH);
                    assert!(enc.pending_output().is_empty());
                    if height == 0 {
                        assert!(
                            enc.finish().is_ok(),
                            "{format:?} {width}x{height}: accepted empty begin but failed finish"
                        );
                    }
                }
                Err(_) => failures.push(format!("{format:?} {width}x{height}: encoder panicked")),
            }
        }
    }
    assert!(failures.is_empty(), "empty canvas contract: {failures:#?}");
}

#[test]
fn every_dispatched_encoder_enforces_row_protocol() {
    for format in AllowedFormats::all().encodable_formats() {
        let make = || {
            EncodeRequest::new(format)
                .with_quality(85.0)
                .build_streaming_encoder(W, H)
                .unwrap()
        };
        let (data, descriptor) = pixels(false);
        let stride = (W as usize + 3) * descriptor.layout.bytes_per_pixel();
        let supported = make().supported;
        let view = PixelView::new(&data, W, H, stride, &descriptor).unwrap();
        let (_, adapted) = adapt_for_encode(
            view,
            &ColorOrigin::assumed(),
            supported,
            &ConvertPolicy::permissive(),
            PeakSource::Declared,
            OutputSignaling::Strip,
        )
        .unwrap();
        let pixels = adapted.pixels.as_view();
        let mut enc = make().encoder;
        enc.begin(W, H, pixels.format().clone()).unwrap();
        let pending = enc.pending_output().to_vec();
        assert!(
            enc.begin(W, H, pixels.format().clone()).is_err(),
            "{format:?} accepted a second begin"
        );
        assert_eq!(
            enc.pending_output(),
            pending,
            "{format:?} changed a rejected declaration"
        );
        assert!(
            enc.encode(pixels.clone()).is_err(),
            "{format:?} accepted encode after begin"
        );

        let other_descriptor = if *pixels.format() == PixelFormat::RGB8_SRGB {
            PixelFormat::RGBA8_SRGB
        } else {
            PixelFormat::RGB8_SRGB
        };
        for (width, descriptor) in [(W - 1, pixels.format().clone()), (W, other_descriptor)] {
            let data = vec![0; width as usize * descriptor.layout.bytes_per_pixel()];
            let wrong = PixelView::new(&data, width, 1, data.len(), &descriptor).unwrap();
            let mut enc = make().encoder;
            enc.begin(W, H, pixels.format().clone()).unwrap();
            assert!(
                enc.push_rows(wrong).is_err(),
                "{format:?} accepted a different row layout"
            );
            assert!(
                enc.pending_output().is_empty(),
                "{format:?} exposes a failed file"
            );
            assert!(
                enc.finish().is_err(),
                "{format:?} completed after a layout error"
            );
        }

        let mut enc = make().encoder;
        enc.begin(W, H, pixels.format().clone()).unwrap();
        assert!(enc.finish().is_err(), "{format:?} accepted missing rows");

        let mut enc = make().encoder;
        enc.begin(W, H, pixels.format().clone()).unwrap();
        enc.push_rows(pixels.clone()).unwrap();
        let extra =
            PixelView::new(pixels.row(0), W, 1, pixels.row(0).len(), pixels.format()).unwrap();
        assert!(
            enc.push_rows(extra).is_err(),
            "{format:?} accepted extra rows"
        );
        assert!(
            enc.pending_output().is_empty(),
            "{format:?} exposes output after failure"
        );
        assert!(
            enc.finish().is_err(),
            "{format:?} completed after a failed push"
        );
    }
}
