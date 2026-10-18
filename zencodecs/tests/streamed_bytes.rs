//! Byte identity at the dispatched encoder/sink boundary on seeded RGB
//! and translucent RGBA input. Draining must never affect codec output.
#![cfg(all(feature = "jpeg", feature = "png"))]

use std::sync::{Arc, Mutex};
use zencodec::decode::DecodeRowSink;
use zencodecs::{AllowedFormats, EncodeRequest, ImageFormat, TranscodeSink};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 67;
const H: u32 = 93;

fn pixels(alpha: bool) -> (Vec<u8>, PixelDescriptor) {
    let descriptor = if alpha {
        PixelDescriptor::RGBA8_SRGB
    } else {
        PixelDescriptor::RGB8_SRGB
    };
    // Padding distinguishes row iteration from accidentally copying contiguous bytes.
    let stride = (W as usize + 3) * descriptor.bytes_per_pixel();
    let mut data = vec![0xCD; stride * H as usize];
    let mut state = 0xD0CA_F00Du32;
    for y in 0..H as usize {
        for x in 0..W as usize {
            for c in 0..descriptor.bytes_per_pixel() {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                data[y * stride + x * descriptor.bytes_per_pixel() + c] = (state >> 24) as u8;
            }
        }
    }
    (data, descriptor)
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
    pixels: PixelSlice<'_>,
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
    sink.begin(W, H, pixels.descriptor()).unwrap();
    let timing = sink.output_timing().unwrap();
    let mut output = Vec::new();
    let mut y = 0;
    let mut index = 0;
    while y < H {
        let rows = [1, 7, 3, 16][index % 4].min(H - y);
        let mut next = sink
            .provide_next_buffer(y, rows, W, pixels.descriptor())
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
            let stride = (W as usize + 3) * desc.bytes_per_pixel();
            let pixels = PixelSlice::new(&data, W, H, stride, desc).unwrap();
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
            let stride = (W as usize + 3) * desc.bytes_per_pixel();
            let pixels = PixelSlice::new(&data, W, H, stride, desc).unwrap();
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
        let stride = (W as usize + 3) * descriptor.bytes_per_pixel();
        let pixels = PixelSlice::new(&data, W, H, stride, descriptor).unwrap();
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
                sink.begin(W, H, descriptor)?;
                for y in (0..H).step_by(7) {
                    let rows = 7.min(H - y);
                    let mut next = sink.provide_next_buffer(y, rows, W, descriptor)?;
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
        let stride = (W as usize + 3) * descriptor.bytes_per_pixel();
        let pixels = PixelSlice::new(&data, W, H, stride, descriptor).unwrap();
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
                sink.begin(W, H, descriptor)?;
                for y in (0..H).step_by(7) {
                    let rows = 7.min(H - y);
                    let mut next = sink.provide_next_buffer(y, rows, W, descriptor)?;
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
