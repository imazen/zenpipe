//! Quality calibration harness for zen encoders.
//!
//! Encodes each image from a corpus at multiple quality levels per codec,
//! then measures SSIMULACRA2 to build quality→SSIM2 curves.
//!
//! Usage:
//!   cargo run --release --features calibrate --example quality_calibrate -- \
//!     --corpus ~/work/codec-corpus/CID22/CID22-512/training \
//!     --output /mnt/v/output/quality-calibrate
//!
//! Output: one CSV per codec + a summary CSV with median SSIM2 per quality level.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use imgref::{Img, ImgRef, ImgVec};
use rayon::prelude::*;
use zencodec::encode::EncoderConfig;

use zencodecs::DecodeRequest;

// bytemuck is a dependency of zencodecs, available transitively
extern crate bytemuck;

/// Quality levels to sweep. Denser at the high end where users care most.
const QUALITY_LEVELS: &[f32] = &[
    5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 72.0, 75.0,
    78.0, 80.0, 82.0, 85.0, 87.0, 90.0, 92.0, 95.0, 97.0, 99.0,
];

/// A single measurement: one image at one quality for one codec.
#[derive(Clone)]
struct Measurement {
    codec: &'static str,
    image_name: String,
    generic_quality: f32,
    ssim2: f64,
    file_size: usize,
    width: u32,
    height: u32,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let corpus_dir = args
        .iter()
        .position(|a| a == "--corpus")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap())
                .join("work/codec-corpus/CID22/CID22-512/training")
        });

    let output_dir = args
        .iter()
        .position(|a| a == "--output")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/v/output/quality-calibrate"));

    let max_images: usize = args
        .iter()
        .position(|a| a == "--max-images")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);

    fs::create_dir_all(&output_dir).expect("create output dir");

    // Collect source images
    let mut image_paths: Vec<PathBuf> = fs::read_dir(&corpus_dir)
        .expect("read corpus dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| matches!(ext.to_str(), Some("png" | "jpg" | "jpeg")))
        })
        .collect();
    image_paths.sort();
    image_paths.truncate(max_images);

    eprintln!(
        "Corpus: {} ({} images)",
        corpus_dir.display(),
        image_paths.len()
    );
    eprintln!("Output: {}", output_dir.display());
    eprintln!(
        "Quality levels: {} ({:?}...{:?})",
        QUALITY_LEVELS.len(),
        QUALITY_LEVELS.first(),
        QUALITY_LEVELS.last()
    );

    let start = Instant::now();

    // Load and decode all source images to RGB8
    eprintln!("Loading source images...");
    let sources: Vec<(String, ImgVec<[u8; 3]>)> = image_paths
        .par_iter()
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().to_string();
            let data = fs::read(path).ok()?;
            let decoded = DecodeRequest::new(&data).decode().ok()?;
            let rgb = decode_output_to_rgb8(&decoded)?;
            Some((name, rgb))
        })
        .collect();

    eprintln!(
        "Loaded {} images in {:.1}s",
        sources.len(),
        start.elapsed().as_secs_f64()
    );

    // Run codecs — libjpeg-turbo first as the reference anchor
    let codecs: Vec<(&str, Box<dyn Fn(f32) -> Option<Codec> + Send + Sync>)> = vec![
        (
            "libjpeg-turbo",
            Box::new(|q| Some(Codec::LibjpegTurbo(q)))
                as Box<dyn Fn(f32) -> Option<Codec> + Send + Sync>,
        ),
        #[cfg(feature = "jpeg")]
        (
            "jpeg",
            Box::new(|q| Some(Codec::Jpeg(q))) as Box<dyn Fn(f32) -> Option<Codec> + Send + Sync>,
        ),
        #[cfg(feature = "webp")]
        (
            "webp",
            Box::new(|q| Some(Codec::Webp(q))) as Box<dyn Fn(f32) -> Option<Codec> + Send + Sync>,
        ),
        #[cfg(feature = "avif-encode")]
        (
            "avif",
            Box::new(|q| Some(Codec::Avif(q))) as Box<dyn Fn(f32) -> Option<Codec> + Send + Sync>,
        ),
        #[cfg(feature = "jxl-encode")]
        (
            "jxl",
            Box::new(|q| Some(Codec::Jxl(q))) as Box<dyn Fn(f32) -> Option<Codec> + Send + Sync>,
        ),
    ];

    for (codec_name, make_codec) in &codecs {
        eprintln!("\n=== {} ===", codec_name);
        let codec_start = Instant::now();

        let measurements: Mutex<Vec<Measurement>> = Mutex::new(Vec::new());
        let done_count = std::sync::atomic::AtomicUsize::new(0);
        let total = sources.len() * QUALITY_LEVELS.len();

        sources.par_iter().for_each(|(name, source_rgb)| {
            // Precompute SSIM2 reference once per image
            let source_ref = imgref_u8_to_ssim2_ref(source_rgb.as_ref());
            let source_ref = match source_ref {
                Some(r) => r,
                None => return,
            };

            for &q in QUALITY_LEVELS {
                let codec = match make_codec(q) {
                    Some(c) => c,
                    None => continue,
                };

                let result = encode_and_measure(
                    codec_name,
                    name,
                    source_rgb.as_ref(),
                    &source_ref,
                    q,
                    &codec,
                );

                if let Some(m) = result {
                    measurements.lock().unwrap().push(m);
                }

                let count = done_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if count % 100 == 0 {
                    eprint!(
                        "\r  {}/{} ({:.0}%)",
                        count,
                        total,
                        count as f64 / total as f64 * 100.0
                    );
                }
            }
        });
        eprintln!(
            "\r  Done: {} measurements in {:.1}s",
            measurements.lock().unwrap().len(),
            codec_start.elapsed().as_secs_f64()
        );

        // Write per-codec CSV
        let measurements = measurements.into_inner().unwrap();
        write_codec_csv(&output_dir, codec_name, &measurements);
    }

    // Write summary CSV (median SSIM2 per codec per quality level)
    write_summary_csv(
        &output_dir,
        &codecs.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
    );

    eprintln!("\nTotal time: {:.1}s", start.elapsed().as_secs_f64());
    eprintln!("Results in: {}", output_dir.display());
}

/// Codec variants for encoding.
enum Codec {
    /// libjpeg-turbo baseline (reference anchor)
    LibjpegTurbo(f32),
    #[cfg(feature = "jpeg")]
    Jpeg(f32),
    #[cfg(feature = "webp")]
    Webp(f32),
    #[cfg(feature = "avif-encode")]
    Avif(f32),
    #[cfg(feature = "jxl-encode")]
    Jxl(f32),
}

/// Encode an image, decode it back, measure SSIM2.
fn encode_and_measure(
    codec_name: &'static str,
    image_name: &str,
    source: ImgRef<[u8; 3]>,
    ssim2_ref: &fast_ssim2::Ssimulacra2Reference,
    quality: f32,
    codec: &Codec,
) -> Option<Measurement> {
    let w = source.width() as u32;
    let h = source.height() as u32;

    // Encode using the codec's with_generic_quality
    let encoded = match codec {
        Codec::LibjpegTurbo(q) => encode_with_libjpeg_turbo(source, *q as i32),
        #[cfg(feature = "jpeg")]
        Codec::Jpeg(q) => {
            let config = zenjpeg::JpegEncoderConfig::new().with_generic_quality(*q);
            encode_rgb8_with_config(&config, source)
        }
        #[cfg(feature = "webp")]
        Codec::Webp(q) => {
            let config = zenwebp::WebpEncoderConfig::lossy().with_generic_quality(*q);
            encode_rgb8_with_config(&config, source)
        }
        #[cfg(feature = "avif-encode")]
        Codec::Avif(q) => {
            let config = zenavif::AvifEncoderConfig::new()
                .with_generic_quality(*q)
                .with_generic_effort(0); // fastest (speed 10); effort 0 = least effort = fastest
            encode_rgb8_with_config(&config, source)
        }
        #[cfg(feature = "jxl-encode")]
        Codec::Jxl(q) => {
            let config = zenjxl::JxlEncoderConfig::new().with_generic_quality(*q);
            encode_rgb8_with_config(&config, source)
        }
    };

    let encoded = match encoded {
        Ok(data) => data,
        Err(e) => {
            eprintln!("  WARN: {} q={} {}: {}", codec_name, quality, image_name, e);
            return None;
        }
    };

    let file_size = encoded.len();

    // Decode back to RGB8
    // Use libjpeg-turbo's own decoder for libjpeg-turbo codec (pure roundtrip),
    // otherwise use zencodecs' format-matched decoder.
    let decoded_rgb = if matches!(codec, Codec::LibjpegTurbo(_)) {
        decode_with_libjpeg_turbo(&encoded)?
    } else {
        let decoded = DecodeRequest::new(&encoded).decode().ok()?;
        decode_output_to_rgb8(&decoded)?
    };

    // Measure SSIM2 using precomputed reference
    let ssim2 = measure_ssim2_with_ref(ssim2_ref, decoded_rgb.as_ref())?;

    Some(Measurement {
        codec: codec_name,
        image_name: image_name.to_string(),
        generic_quality: quality,
        ssim2,
        file_size,
        width: w,
        height: h,
    })
}

/// Encode RGB8 pixels using libjpeg-turbo directly.
fn encode_with_libjpeg_turbo(img: ImgRef<[u8; 3]>, quality: i32) -> Result<Vec<u8>, String> {
    let w = img.width();
    let h = img.height();
    // Flatten [u8; 3] pixels into contiguous &[u8]
    let pixels: Vec<u8> = img
        .pixels()
        .flat_map(|p| {
            let [r, g, b] = p;
            [r, g, b]
        })
        .collect();
    let tj_image = turbojpeg::Image {
        pixels: pixels.as_slice(),
        width: w,
        pitch: w * 3,
        height: h,
        format: turbojpeg::PixelFormat::RGB,
    };
    let mut compressor = turbojpeg::Compressor::new().map_err(|e| format!("{e}"))?;
    compressor
        .set_quality(quality.clamp(1, 100))
        .map_err(|e| format!("{e}"))?;
    compressor
        .set_subsamp(turbojpeg::Subsamp::Sub2x2)
        .map_err(|e| format!("{e}"))?;
    compressor
        .compress_to_vec(tj_image)
        .map_err(|e| format!("{e}"))
}

/// Decode JPEG bytes using libjpeg-turbo and return RGB8 ImgVec.
fn decode_with_libjpeg_turbo(data: &[u8]) -> Option<ImgVec<[u8; 3]>> {
    let image = turbojpeg::decompress(data, turbojpeg::PixelFormat::RGB).ok()?;
    let w = image.width;
    let h = image.height;
    let pitch = image.pitch;
    let mut rgb_data = Vec::with_capacity(w * h);
    for y in 0..h {
        let row_start = y * pitch;
        for x in 0..w {
            let i = row_start + x * 3;
            rgb_data.push([image.pixels[i], image.pixels[i + 1], image.pixels[i + 2]]);
        }
    }
    Some(ImgVec::new(rgb_data, w, h))
}

/// Encode RGB8 pixels using a concrete EncoderConfig.
fn encode_rgb8_with_config<C>(config: &C, img: ImgRef<[u8; 3]>) -> Result<Vec<u8>, String>
where
    C: zencodecs::AnyEncoder,
{
    // Convert [u8; 3] to rgb::Rgb<u8>
    let pixels: Vec<rgb::Rgb<u8>> = img
        .pixels()
        .map(|p| rgb::Rgb {
            r: p[0],
            g: p[1],
            b: p[2],
        })
        .collect();
    let rgba_pixels: Vec<rgb::Rgba<u8>> = pixels
        .iter()
        .map(|p| rgb::Rgba {
            r: p.r,
            g: p.g,
            b: p.b,
            a: 255,
        })
        .collect();
    let rgba_img = Img::new(rgba_pixels.as_slice(), img.width(), img.height());

    let output = config
        .encode_srgba8_imgref(rgba_img, true)
        .map_err(|e| format!("{e}"))?;
    Ok(output.into_vec())
}

/// Convert a DecodeOutput to an ImgVec<[u8; 3]>.
fn decode_output_to_rgb8(output: &zencodecs::DecodeOutput) -> Option<ImgVec<[u8; 3]>> {
    let w = output.width() as usize;
    let h = output.height() as usize;
    let pixels = output.pixels();
    let bytes = pixels.contiguous_bytes();
    let desc = pixels.descriptor();
    let bpp = desc.bytes_per_pixel();
    let channel_type = desc.channel_type();

    use zenpixels::ChannelType;
    match (channel_type, bpp) {
        (ChannelType::U8, 3) => {
            // RGB8
            let data: Vec<[u8; 3]> = bytes.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
            (data.len() == w * h).then(|| ImgVec::new(data, w, h))
        }
        (ChannelType::U8, 4) => {
            // RGBA8 -> RGB8 (drop alpha)
            let data: Vec<[u8; 3]> = bytes.chunks_exact(4).map(|c| [c[0], c[1], c[2]]).collect();
            (data.len() == w * h).then(|| ImgVec::new(data, w, h))
        }
        (ChannelType::U8, 1) => {
            // Gray8 -> RGB8
            let data: Vec<[u8; 3]> = bytes.iter().map(|&g| [g, g, g]).collect();
            (data.len() == w * h).then(|| ImgVec::new(data, w, h))
        }
        (ChannelType::U16, 6) => {
            // RGB16 -> RGB8 (shift down)
            let data: Vec<[u8; 3]> = bytes
                .chunks_exact(6)
                .map(|c| {
                    let r = u16::from_ne_bytes([c[0], c[1]]);
                    let g = u16::from_ne_bytes([c[2], c[3]]);
                    let b = u16::from_ne_bytes([c[4], c[5]]);
                    [(r >> 8) as u8, (g >> 8) as u8, (b >> 8) as u8]
                })
                .collect();
            (data.len() == w * h).then(|| ImgVec::new(data, w, h))
        }
        (ChannelType::U16, 8) => {
            // RGBA16 -> RGB8 (shift down, drop alpha)
            let data: Vec<[u8; 3]> = bytes
                .chunks_exact(8)
                .map(|c| {
                    let r = u16::from_ne_bytes([c[0], c[1]]);
                    let g = u16::from_ne_bytes([c[2], c[3]]);
                    let b = u16::from_ne_bytes([c[4], c[5]]);
                    [(r >> 8) as u8, (g >> 8) as u8, (b >> 8) as u8]
                })
                .collect();
            (data.len() == w * h).then(|| ImgVec::new(data, w, h))
        }
        (ChannelType::F32, 12) => {
            // RGB f32 -> RGB8 (clamp + scale)
            let floats: &[f32] = bytemuck::cast_slice(&bytes[..w * h * 12]);
            let data: Vec<[u8; 3]> = floats
                .chunks_exact(3)
                .map(|c| {
                    [
                        (c[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        (c[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        (c[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                    ]
                })
                .collect();
            (data.len() == w * h).then(|| ImgVec::new(data, w, h))
        }
        _ => {
            eprintln!(
                "  WARN: unsupported pixel format {:?} bpp={} for SSIM2 measurement",
                channel_type, bpp
            );
            None
        }
    }
}

/// Create an SSIM2 reference from an sRGB u8 image.
fn imgref_u8_to_ssim2_ref(img: ImgRef<[u8; 3]>) -> Option<fast_ssim2::Ssimulacra2Reference> {
    fast_ssim2::Ssimulacra2Reference::new(img).ok()
}

/// Measure SSIM2 between a precomputed reference and a distorted image.
fn measure_ssim2_with_ref(
    reference: &fast_ssim2::Ssimulacra2Reference,
    distorted: ImgRef<[u8; 3]>,
) -> Option<f64> {
    reference.compare(distorted).ok()
}

/// Write per-codec CSV.
fn write_codec_csv(output_dir: &Path, codec: &str, measurements: &[Measurement]) {
    let path = output_dir.join(format!("{}_quality_sweep.csv", codec));
    let mut f = fs::File::create(&path).expect("create csv");
    writeln!(
        f,
        "codec,image,generic_quality,ssim2,file_size,width,height,bpp"
    )
    .unwrap();

    let mut sorted = measurements.to_vec();
    sorted.sort_by(|a, b| {
        a.image_name
            .cmp(&b.image_name)
            .then(a.generic_quality.partial_cmp(&b.generic_quality).unwrap())
    });

    for m in &sorted {
        let pixels = m.width as f64 * m.height as f64;
        let bpp = m.file_size as f64 * 8.0 / pixels;
        writeln!(
            f,
            "{},{},{},{:.4},{},{},{},{:.4}",
            m.codec, m.image_name, m.generic_quality, m.ssim2, m.file_size, m.width, m.height, bpp
        )
        .unwrap();
    }

    eprintln!("  Wrote {}", path.display());
}

/// Write summary CSV: median SSIM2 per codec per quality level.
fn write_summary_csv(output_dir: &Path, codec_names: &[&str]) {
    let path = output_dir.join("quality_calibration_summary.csv");
    let mut f = fs::File::create(&path).expect("create summary csv");

    // Header
    write!(f, "generic_quality").unwrap();
    for codec in codec_names {
        write!(
            f,
            ",{}_median_ssim2,{}_p25_ssim2,{}_p75_ssim2,{}_median_bpp",
            codec, codec, codec, codec
        )
        .unwrap();
    }
    writeln!(f).unwrap();

    // For each quality level, read codec CSVs and compute stats
    for &q in QUALITY_LEVELS {
        write!(f, "{}", q).unwrap();

        for codec in codec_names {
            let csv_path = output_dir.join(format!("{}_quality_sweep.csv", codec));
            let (median_ssim2, p25, p75, median_bpp) = if csv_path.exists() {
                compute_stats_for_quality(&csv_path, q)
            } else {
                (f64::NAN, f64::NAN, f64::NAN, f64::NAN)
            };
            write!(
                f,
                ",{:.4},{:.4},{:.4},{:.4}",
                median_ssim2, p25, p75, median_bpp
            )
            .unwrap();
        }
        writeln!(f).unwrap();
    }

    eprintln!("  Wrote summary: {}", path.display());
}

/// Read a codec CSV and compute median/p25/p75 SSIM2 and median bpp for a given quality.
fn compute_stats_for_quality(csv_path: &Path, quality: f32) -> (f64, f64, f64, f64) {
    let contents = fs::read_to_string(csv_path).unwrap_or_default();
    let mut ssim2_values: Vec<f64> = Vec::new();
    let mut bpp_values: Vec<f64> = Vec::new();

    for line in contents.lines().skip(1) {
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 8 {
            continue;
        }
        let q: f32 = cols[2].parse().unwrap_or(-1.0);
        if (q - quality).abs() < 0.01 {
            if let Ok(ssim2) = cols[3].parse::<f64>() {
                ssim2_values.push(ssim2);
            }
            if let Ok(bpp) = cols[7].parse::<f64>() {
                bpp_values.push(bpp);
            }
        }
    }

    if ssim2_values.is_empty() {
        return (f64::NAN, f64::NAN, f64::NAN, f64::NAN);
    }

    ssim2_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    bpp_values.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let median = percentile(&ssim2_values, 50);
    let p25 = percentile(&ssim2_values, 25);
    let p75 = percentile(&ssim2_values, 75);
    let median_bpp = percentile(&bpp_values, 50);

    (median, p25, p75, median_bpp)
}

fn percentile(sorted: &[f64], pct: u32) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = (sorted.len() as f64 * pct as f64 / 100.0).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
