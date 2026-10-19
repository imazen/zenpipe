//! Executable finite contract catalog. The catalog generates every ordered
//! source/destination route; refusals and next-contract gaps are explicit rows.
//! Set ZEN_CODEC_MATRIX_REPORT to save the TSV (also printed with --nocapture).
#![cfg(all(
    feature = "std",
    feature = "cms",
    feature = "jpeg",
    feature = "png",
    feature = "webp",
    feature = "gif-zenquant"
))]
use zencodec::encode::{DynEncoderConfig, EncoderConfig, Fidelity};
use zencodecs::{
    AllowedFormats, DecodeRequest, EncodeRequest, FormatDecision, ImageFormat, MetadataPolicy,
    PixelBufferConvertTypedExt, SupplementPolicy, TranscodeColor, TranscodeOptions,
};
use zenpixels::{PixelDescriptor, PixelSlice};

type InventoryCheck = dyn Fn(&[u8]) -> zencodec_testkit::Conformance;

struct Entry {
    name: &'static str,
    format: ImageFormat,
    exact_rgb8: bool,
    native_backend: bool,
    encoder: Box<dyn DynEncoderConfig>,
    inventory: Box<InventoryCheck>,
}
fn entry<E, D>(name: &'static str, exact_rgb8: bool, encoder: E, decoder: D) -> Entry
where
    E: EncoderConfig + DynEncoderConfig + 'static,
    D: zencodec::decode::DecoderConfig + 'static,
{
    Entry {
        name,
        format: <E as EncoderConfig>::format(),
        exact_rgb8,
        native_backend: false,
        encoder: Box::new(encoder),
        inventory: Box::new(move |bytes| {
            zencodec_testkit::check_inventory_dispatch(decoder.clone(), bytes)?;
            #[cfg(feature = "inventory")]
            zencodec_testkit::check_inventory_input_limit(decoder.clone(), bytes)?;
            Ok(())
        }),
    }
}
fn catalog() -> Vec<Entry> {
    let mut entries = vec![
        entry(
            "jpeg/default",
            false,
            zencodecs::JpegEncoderConfig::new(),
            zencodecs::JpegDecoderConfig::new(),
        ),
        entry(
            "png/lossless",
            true,
            zencodecs::PngEncoderConfig::new(),
            zencodecs::PngDecoderConfig::new(),
        ),
        entry(
            "webp/lossless",
            true,
            zencodecs::WebpEncoderConfig::lossless(),
            zencodecs::WebpDecoderConfig::new(),
        ),
        entry(
            "gif/zenquant",
            false,
            zencodecs::GifEncoderConfig::new(),
            zencodecs::GifDecoderConfig::new(),
        ),
    ];
    #[cfg(all(feature = "jxl-encode", feature = "jxl-decode"))]
    entries.push(entry(
        "jxl/lossless",
        true,
        zencodecs::JxlEncoderConfig::default().with_lossless(true),
        zencodecs::JxlDecoderConfig::default(),
    ));
    #[cfg(all(feature = "avif-encode", feature = "avif-decode"))]
    entries.push(entry(
        "avif/default",
        false,
        zenavif::AvifEncoderConfig::new().with_effort_u32(10),
        zenavif::AvifDecoderConfig::new(),
    ));
    #[cfg(feature = "bitmaps")]
    entries.extend([
        entry(
            "pnm",
            true,
            zenbitmaps::PnmEncoderConfig::new(),
            zenbitmaps::PnmDecoderConfig::new(),
        ),
        entry(
            "farbfeld",
            true,
            zenbitmaps::FarbfeldEncoderConfig::new(),
            zenbitmaps::FarbfeldDecoderConfig::new(),
        ),
    ]);
    #[cfg(feature = "bitmaps-bmp")]
    entries.push(entry(
        "bmp",
        true,
        zenbitmaps::BmpEncoderConfig::new(),
        zenbitmaps::BmpDecoderConfig::new(),
    ));
    #[cfg(feature = "bitmaps-qoi")]
    entries.push(entry(
        "qoi",
        true,
        zenbitmaps::QoiEncoderConfig::new(),
        zenbitmaps::QoiDecoderConfig::new(),
    ));
    #[cfg(feature = "bitmaps-tga")]
    entries.push(entry(
        "tga",
        true,
        zenbitmaps::TgaEncoderConfig::new(),
        zenbitmaps::TgaDecoderConfig::new(),
    ));
    #[cfg(feature = "bitmaps-hdr")]
    entries.push(entry(
        "hdr",
        false,
        zenbitmaps::HdrEncoderConfig::new(),
        zenbitmaps::HdrDecoderConfig::new(),
    ));
    #[cfg(feature = "tiff")]
    entries.push(entry(
        "tiff",
        true,
        zentiff::codec::TiffEncoderCodecConfig::new(),
        zentiff::codec::TiffDecoderCodecConfig::new(),
    ));
    #[cfg(feature = "avif-svt")]
    entries.push(avif_backend("avif/svt-420", zenavif::Av1Backend::Zenav1Svt));
    #[cfg(feature = "avif-aom")]
    entries.push(avif_backend("avif/aom-420", zenavif::Av1Backend::Zenav1Aom));
    entries
}

#[cfg(any(feature = "avif-svt", feature = "avif-aom"))]
fn avif_backend(name: &'static str, backend: zenavif::Av1Backend) -> Entry {
    let mut config = zenavif::AvifEncoderConfig::new();
    *config.inner_mut() = zenavif::EncoderConfig::new()
        .backend(backend)
        .chroma_subsampling(zenavif::EncodeChromaSubsampling::Yuv420)
        .speed(10)
        .threads(Some(1));
    let mut e = entry(name, false, config, zenavif::AvifDecoderConfig::new());
    e.native_backend = true;
    e
}

/// The name `ZEN_CODEC_MATRIX_ORACLE_FORMATS` uses for `format`.
fn oracle_name(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "jpeg",
        ImageFormat::Png => "png",
        ImageFormat::WebP => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Avif => "avif",
        ImageFormat::Jxl => "jxl",
        ImageFormat::Pnm => "pnm",
        ImageFormat::Farbfeld => "farbfeld",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Qoi => "qoi",
        ImageFormat::Tga => "tga",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Hdr => "hdr",
        _ => "unknown",
    }
}

fn external_rgb(format: ImageFormat, data: &[u8]) -> Result<Option<Vec<u8>>, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let Some(magick) = std::env::var_os("ZEN_CODEC_MATRIX_MAGICK") else {
        return Ok(None);
    };
    // The caller names the formats its installed readers decode (for example,
    // Ubuntu's ImageMagick 6 has no farbfeld or QOI delegate). Unset means all.
    if let Ok(list) = std::env::var("ZEN_CODEC_MATRIX_ORACLE_FORMATS")
        && !list
            .split(',')
            .any(|name| name.trim().eq_ignore_ascii_case(oracle_name(format)))
    {
        return Ok(None);
    }
    fn run(program: &std::ffi::OsStr, args: &[&str], input: &[u8]) -> Result<Vec<u8>, String> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input)
            .map_err(|e| e.to_string())?;
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        Ok(output.stdout)
    }
    let (kind, input) = if format == ImageFormat::Jxl {
        let djxl = std::env::var_os("ZEN_CODEC_MATRIX_DJXL")
            .ok_or("set ZEN_CODEC_MATRIX_DJXL for JXL oracle")?;
        // Files, not pipes: older djxl releases (Ubuntu's libjxl-tools) take
        // neither stdin/stdout nor `--output_format`; all infer PPM from the
        // output extension.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
        let input = dir.join(format!("oracle-{}-{n}.jxl", std::process::id()));
        let output = input.with_extension("ppm");
        std::fs::write(&input, data).map_err(|e| e.to_string())?;
        let decoded = run(
            &djxl,
            &[
                input.to_str().ok_or("non-UTF-8 temp path")?,
                output.to_str().ok_or("non-UTF-8 temp path")?,
            ],
            &[],
        )
        .and_then(|_| std::fs::read(&output).map_err(|e| e.to_string()));
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
        ("pnm:-", decoded?)
    } else {
        let kind = match format {
            ImageFormat::Jpeg => "jpeg:-",
            ImageFormat::Png => "png:-",
            ImageFormat::WebP => "webp:-",
            ImageFormat::Gif => "gif:-",
            ImageFormat::Avif => "avif:-",
            ImageFormat::Pnm => "pnm:-",
            ImageFormat::Farbfeld => "farbfeld:-",
            ImageFormat::Bmp => "bmp:-",
            ImageFormat::Qoi => "qoi:-",
            ImageFormat::Tga => "tga:-",
            ImageFormat::Tiff => "tiff:-",
            ImageFormat::Hdr => "hdr:-",
            _ => return Err("missing oracle mapping".into()),
        };
        (kind, data.to_vec())
    };
    run(&magick, &[kind, "-depth", "8", "rgb:-"], &input).map(Some)
}

struct Report {
    rows: Vec<String>,
    failures: Vec<String>,
}
impl Report {
    fn new() -> Self {
        Self {
            rows: vec!["operation\tsource\ttarget\tfixture\tlaw\tresult\tdetail".into()],
            failures: vec![],
        }
    }
    fn row(&mut self, [operation, source, target, fixture, law, result, detail]: [&str; 7]) {
        self.rows.push(format!(
            "{operation}\t{source}\t{target}\t{fixture}\t{law}\t{result}\t{}",
            detail.replace(['\n', '\r', '\t'], " ")
        ));
        if result == "FAIL" || result == "BLOCKED" {
            self.failures.push(self.rows.last().unwrap().clone());
        }
    }
    fn blocked_routes(&mut self, source: &Entry, entries: &[Entry], fixture: &str) {
        for destination in entries {
            self.row([
                "transcode",
                source.name,
                destination.name,
                fixture,
                "current/explicit-srgb8",
                "BLOCKED",
                "source encode/decode failed",
            ]);
        }
    }
    fn finish(self) {
        let text = self.rows.join("\n") + "\n";
        if let Some(path) = std::env::var_os("ZEN_CODEC_MATRIX_REPORT") {
            std::fs::write(path, &text).expect("write matrix report");
        }
        println!("{text}");
        assert!(self.failures.is_empty(), "{}", self.failures.join("\n"));
    }
}

fn rgb_bytes(width: u32, height: u32, padding: usize) -> (Vec<u8>, usize) {
    let stride = width as usize * 3 + padding;
    let mut bytes = vec![0xa5; stride * height as usize];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let p = y * stride + x * 3;
            bytes[p..p + 3].copy_from_slice(&[
                (x * 17 + y * 3) as u8,
                (x * 7 + y * 23) as u8,
                (x ^ y) as u8,
            ]);
        }
    }
    (bytes, stride)
}
fn decode_rgb(bytes: &[u8]) -> Result<Vec<rgb::Rgb<u8>>, String> {
    let image = DecodeRequest::new(bytes)
        .decode_full_frame()
        .map_err(|e| e.to_string())?;
    Ok(image.into_buffer().to_rgb8().as_imgref().buf().to_vec())
}

#[test]
fn exhaustive_registered_still_routes_and_fidelity_targets() {
    let entries = catalog();
    let mut report = Report::new();
    // This is a finite target-selection table, not a metric-score claim.
    let targets = [
        Fidelity::Lossless,
        Fidelity::codec_quality(80.0),
        Fidelity::ssim2(80.0),
        Fidelity::butteraugli(1.0),
        Fidelity::zensim_b(80.0),
    ];
    for entry in &entries {
        for target in targets {
            let query = entry.encoder.check_fidelity_target(target);
            let selected = entry.encoder.try_fidelity_job(target);
            let agrees = match (&query, &selected) {
                (Ok(()), Ok(_)) => true,
                (Err(a), Err(b)) => a == b,
                _ => false,
            };
            if let Ok(job) = selected {
                let (samples, stride) = rgb_bytes(7, 5, 6);
                let pixels =
                    PixelSlice::new(&samples, 7, 5, stride, PixelDescriptor::RGB8_SRGB).unwrap();
                let expected: Vec<_> = (0..5)
                    .flat_map(|y| {
                        pixels
                            .row(y)
                            .as_chunks::<3>()
                            .0
                            .iter()
                            .map(|p| rgb::Rgb::new(p[0], p[1], p[2]))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let result = job
                    .encode(pixels)
                    .map_err(|e| e.to_string())
                    .and_then(|out| {
                        let decoded = decode_rgb(out.data())?;
                        if decoded.len() != expected.len() {
                            return Err("wrong pixel count".into());
                        }
                        if target == Fidelity::Lossless && decoded != expected {
                            return Err("confirmed lossless target changed RGB8 samples".into());
                        }
                        Ok(())
                    });
                report.row([
                    "fidelity-encode",
                    "",
                    entry.name,
                    &format!("{target:?}"),
                    "current/rgb8-target-delivery",
                    if result.is_ok() { "PASS" } else { "FAIL" },
                    &result.err().unwrap_or_default(),
                ]);
            }
            report.row([
                "fidelity",
                "",
                entry.name,
                &format!("{target:?}"),
                "current/query-job-parity",
                if agrees { "PASS" } else { "FAIL" },
                &format!("{query:?}"),
            ]);
        }
        match entry.encoder.select_fidelity_job(&targets) {
            Ok((index, _)) => {
                let first = targets
                    .iter()
                    .position(|t| entry.encoder.check_fidelity_target(*t).is_ok());
                report.row([
                    "preference",
                    "",
                    entry.name,
                    "ordered-targets",
                    "current/first-confirmed",
                    if first == Some(index) { "PASS" } else { "FAIL" },
                    &format!("selected {index}"),
                ]);
            }
            Err(zencodec::encode::FidelitySelectionError::Rejected(reasons)) => report.row([
                "preference",
                "",
                entry.name,
                "ordered-targets",
                "current/explicit-refusal",
                if reasons.len() == targets.len() {
                    "EXPECTED_REJECTION"
                } else {
                    "FAIL"
                },
                &format!("{reasons:?}"),
            ]),
            Err(e) => report.row([
                "preference",
                "",
                entry.name,
                "ordered-targets",
                "current/explicit-refusal",
                "FAIL",
                &e.to_string(),
            ]),
        }
        report.row([
            "fidelity",
            "",
            entry.name,
            "required-pixel-fidelity",
            "next",
            "API_GAP",
            "configuration selection is not content-dependent admission",
        ]);
    }
    for (width, height, padding) in [(1, 1, 0), (7, 5, 12), (17, 9, 6)] {
        let fixture = format!("rgb8-{width}x{height}-pad{padding}");
        let (bytes, stride) = rgb_bytes(width, height, padding);
        let input =
            || PixelSlice::new(&bytes, width, height, stride, PixelDescriptor::RGB8_SRGB).unwrap();
        let expected: Vec<_> = (0..height)
            .flat_map(|y| {
                input()
                    .row(y)
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|p| rgb::Rgb::new(p[0], p[1], p[2]))
                    .collect::<Vec<_>>()
            })
            .collect();
        for source in &entries {
            let encoded = if source.native_backend {
                source
                    .encoder
                    .dyn_job()
                    .encode(input())
                    .map(|out| out.into_vec())
                    .map_err(|e| e.to_string())
            } else {
                EncodeRequest::new(source.format)
                    .with_lossless(source.exact_rgb8)
                    .with_effort(0)
                    .encode(input(), false)
                    .map(|out| out.data().to_vec())
                    .map_err(|e| e.to_string())
            };
            let encoded = match encoded {
                Ok(value) => value,
                Err(e) => {
                    report.row([
                        "encode",
                        "",
                        source.name,
                        &fixture,
                        "current/valid-rgb8",
                        "FAIL",
                        &e.to_string(),
                    ]);
                    report.blocked_routes(source, &entries, &fixture);
                    continue;
                }
            };
            let source_pixels = match decode_rgb(&encoded) {
                Ok(value) => value,
                Err(e) => {
                    report.row([
                        "decode",
                        source.name,
                        "",
                        &fixture,
                        "current/valid-output",
                        "FAIL",
                        &e,
                    ]);
                    report.blocked_routes(source, &entries, &fixture);
                    continue;
                }
            };
            let exact = !source.exact_rgb8 || source_pixels == expected;
            report.row([
                "roundtrip",
                source.name,
                source.name,
                &fixture,
                if source.exact_rgb8 {
                    "current/exact-rgb8"
                } else {
                    "current/decode-valid"
                },
                if exact && source_pixels.len() == expected.len() {
                    "PASS"
                } else {
                    "FAIL"
                },
                "",
            ]);
            let inspection = (source.inventory)(&encoded);
            report.row([
                "inventory",
                source.name,
                "",
                &fixture,
                if cfg!(feature = "inventory") {
                    "current/enabled-dispatch-and-input-limit"
                } else {
                    "current/disabled-typed-refusal"
                },
                if inspection.is_ok() { "PASS" } else { "FAIL" },
                &inspection.err().map(|e| e.to_string()).unwrap_or_default(),
            ]);
            let oracle = external_rgb(source.format, &encoded);
            let (status, detail) = match oracle {
                Ok(Some(pixels))
                    if pixels.len() == expected.len() * 3
                        && (!source.exact_rgb8
                            || pixels
                                == expected
                                    .iter()
                                    .flat_map(|p| [p.r, p.g, p.b])
                                    .collect::<Vec<_>>()) =>
                {
                    ("PASS", String::new())
                }
                Ok(Some(_)) => (
                    "FAIL",
                    "independent decoder returned wrong samples or pixel count".into(),
                ),
                Ok(None) => (
                    "UNEXECUTED",
                    "caller did not enable an external reader for this format".into(),
                ),
                Err(e) => ("FAIL", e),
            };
            report.row([
                "external-decode",
                source.name,
                "",
                &fixture,
                "current/independent-reader",
                status,
                &detail,
            ]);
            for destination in &entries {
                let mut decision = FormatDecision::for_format(destination.format);
                decision.lossless = destination.exact_rgb8;
                decision.quality.effort = Some(0);
                // Explicit sRGB conversion makes the conversion part of the
                // request. Preserve and HDR/carrier scenarios have separate tests.
                let opts = TranscodeOptions {
                    color: TranscodeColor::Srgb8,
                    metadata_policy: MetadataPolicy::ColorAndRotation,
                    supplements: SupplementPolicy::Strip,
                    ..Default::default()
                };
                let result = if destination.native_backend {
                    let samples: Vec<_> =
                        source_pixels.iter().flat_map(|p| [p.r, p.g, p.b]).collect();
                    let pixels = PixelSlice::new(
                        &samples,
                        width,
                        height,
                        width as usize * 3,
                        PixelDescriptor::RGB8_SRGB,
                    )
                    .unwrap();
                    destination
                        .encoder
                        .dyn_job()
                        .encode(pixels)
                        .map(|out| out.into_vec())
                        .map_err(|e| e.to_string())
                } else {
                    zencodecs::transcode(&encoded, &decision, &opts, &AllowedFormats::all())
                        .map(|out| out.data)
                        .map_err(|e| e.to_string())
                };
                let result = result.and_then(|out| {
                    let decoded = DecodeRequest::new(&out)
                        .decode_full_frame()
                        .map_err(|e| e.to_string())?;
                    if decoded.width() != width || decoded.height() != height {
                        return Err("wrong dimensions".into());
                    }
                    let actual = decoded.into_buffer().to_rgb8();
                    if destination.exact_rgb8
                        && *actual.as_imgref().buf() != source_pixels.as_slice()
                    {
                        return Err("lossless destination changed source RGB8 samples".into());
                    }
                    if let Some(pixels) = external_rgb(destination.format, &out)? {
                        if pixels.len() != expected.len() * 3 {
                            return Err("independent transcode decode has wrong pixel count".into());
                        }
                        if destination.exact_rgb8
                            && pixels
                                != source_pixels
                                    .iter()
                                    .flat_map(|p| [p.r, p.g, p.b])
                                    .collect::<Vec<_>>()
                        {
                            return Err("independent transcode decode changed RGB8 samples".into());
                        }
                    }
                    Ok(())
                });
                report.row([
                    if destination.native_backend {
                        "transcode-native-backend"
                    } else {
                        "transcode"
                    },
                    source.name,
                    destination.name,
                    &fixture,
                    "current/explicit-srgb8",
                    if result.is_ok() { "PASS" } else { "FAIL" },
                    &result.err().unwrap_or_default(),
                ]);
            }
        }
    }
    let routes = report
        .rows
        .iter()
        .filter(|row| {
            row.starts_with("transcode\t") || row.starts_with("transcode-native-backend\t")
        })
        .count();
    assert_eq!(
        routes,
        entries.len() * entries.len() * 3,
        "missing ordered routes"
    );
    let mut counts = std::collections::BTreeMap::new();
    for row in report.rows.iter().skip(1) {
        *counts
            .entry(row.split('\t').nth(5).unwrap())
            .or_insert(0usize) += 1;
    }
    println!(
        "matrix configurations={} routes={routes} outcomes={counts:?}",
        entries.len()
    );
    report.finish();
}

#[test]
fn animation_order_exact_timing_and_typed_refusal() {
    use zencodec::animation::FrameDuration;
    use zencodec_testkit::{ExpectedError, TimingCase, check_animation_cases};
    let a = [20, 80, 140, 255].repeat(16 * 8);
    let b = [180, 40, 100, 255].repeat(16 * 8);
    let a = PixelSlice::new(&a, 16, 8, 16 * 4, PixelDescriptor::RGBA8_SRGB).unwrap();
    let b = PixelSlice::new(&b, 16, 8, 16 * 4, PixelDescriptor::RGBA8_SRGB).unwrap();
    let rejected = ExpectedError::Unsupported(zencodec::UnsupportedOperation::AnimationTiming);
    let cases = [
        TimingCase {
            pixels: a.clone(),
            requested: FrameDuration::from_millis(10),
            expected: Ok((FrameDuration::from_millis(10), a.clone())),
        },
        TimingCase {
            pixels: a.clone(),
            requested: FrameDuration::new(1, 3).unwrap(),
            expected: Err(rejected),
        },
        TimingCase {
            pixels: b.clone(),
            requested: FrameDuration::from_millis(20),
            expected: Ok((FrameDuration::from_millis(20), b.clone())),
        },
    ];
    // GIF now supports exact rational timing on its centisecond grid.
    // Keep the same typed rejection and frame-preservation cases as WebP.
    assert!(<zencodecs::GifEncoderConfig as EncoderConfig>::capabilities().animation_timing());
    check_animation_cases(
        zencodecs::GifEncoderConfig::new(),
        zencodecs::GifDecoderConfig::new(),
        true,
        &cases,
    )
    .unwrap();
    check_animation_cases(
        zencodecs::WebpEncoderConfig::lossless(),
        zencodecs::WebpDecoderConfig::new(),
        true,
        &cases,
    )
    .unwrap();
    let thirds = [
        TimingCase {
            pixels: a.clone(),
            requested: FrameDuration::new(1, 3).unwrap(),
            expected: Ok((FrameDuration::new(1, 3).unwrap(), a.clone())),
        },
        TimingCase {
            pixels: a.clone(),
            requested: FrameDuration::new(1, 65537).unwrap(),
            expected: Err(rejected),
        },
        TimingCase {
            pixels: b.clone(),
            requested: FrameDuration::new(2, 3).unwrap(),
            expected: Ok((FrameDuration::new(2, 3).unwrap(), b.clone())),
        },
    ];
    check_animation_cases(
        zencodecs::PngEncoderConfig::new(),
        zencodecs::PngDecoderConfig::new(),
        true,
        &thirds,
    )
    .unwrap();
}

#[cfg(feature = "bitmaps")]
#[test]
fn structural_prefixes_distinguish_payload_from_trailer() {
    use zencodec_testkit::{ExpectedError, PrefixCase, PrefixOutcome, check_decode_prefix_cases};
    let mut bytes = b"P6\n2 1\n255\n".to_vec();
    let pixels = [20, 80, 140, 180, 40, 100];
    bytes.extend_from_slice(&pixels);
    let payload_end = bytes.len();
    bytes.extend_from_slice(&[0; 1024]);
    // Current PNM adapter preserves sample values without attaching a transfer.
    let expected = PixelSlice::new(
        &pixels,
        2,
        1,
        6,
        PixelDescriptor::RGB8_SRGB.with_transfer(zenpixels::TransferFunction::Unknown),
    )
    .unwrap();
    let cases = [
        PrefixCase {
            end: payload_end - 1,
            outcome: PrefixOutcome::Reject(ExpectedError::Category(
                zencodec::ErrorCategory::Image(zencodec::ImageError::UnexpectedEof),
            )),
        },
        PrefixCase {
            end: payload_end,
            outcome: PrefixOutcome::Accept(expected.clone()),
        },
        PrefixCase {
            end: bytes.len(),
            outcome: PrefixOutcome::Accept(expected),
        },
    ];
    check_decode_prefix_cases(zenbitmaps::PnmDecoderConfig::new(), &bytes, &cases).unwrap();
}
