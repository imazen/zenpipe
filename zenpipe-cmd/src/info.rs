//! `zenpipe info` and `zenpipe compare` subcommand implementations.

use std::process::ExitCode;

use crate::args::{CompareArgs, InfoArgs};
use crate::error::{EXIT_INPUT_ERROR, EXIT_SUCCESS};

/// Run the `info` subcommand.
pub fn run_info(args: InfoArgs) -> ExitCode {
    let mut any_failed = false;

    // Expand globs in file arguments.
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for pattern in &args.files {
        if pattern.contains('*') || pattern.contains('?') {
            match glob::glob(pattern) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        if entry.is_file() {
                            paths.push(entry);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("error: invalid glob '{pattern}': {e}");
                    any_failed = true;
                }
            }
        } else {
            paths.push(std::path::PathBuf::from(pattern));
        }
    }

    let mut json_results: Vec<serde_json::Value> = Vec::new();

    for path in &paths {
        let path_str = path.to_string_lossy();
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: {path_str}: {e}");
                any_failed = true;
                continue;
            }
        };

        match zencodecs::from_bytes(&data) {
            Ok(info) => {
                if args.json {
                    let obj = serde_json::json!({
                        "file": path_str,
                        "format": format!("{:?}", info.format),
                        "width": info.width,
                        "height": info.height,
                        "has_alpha": info.has_alpha,
                        "mime_type": info.format.mime_type(),
                        "size_bytes": data.len(),
                        "bits_per_pixel": if info.width > 0 && info.height > 0 {
                            (data.len() as f64 * 8.0) / (info.width as f64 * info.height as f64)
                        } else { 0.0 },
                    });
                    json_results.push(obj);
                } else {
                    let bpp = if info.width > 0 && info.height > 0 {
                        (data.len() as f64 * 8.0) / (info.width as f64 * info.height as f64)
                    } else {
                        0.0
                    };
                    println!(
                        "{}: {} {}x{}, {}",
                        path_str,
                        format_name(info.format),
                        info.width,
                        info.height,
                        if info.has_alpha { "alpha" } else { "opaque" },
                    );
                    println!("  Size: {} ({:.2} bpp)", format_size(data.len()), bpp,);
                    let exif_val = info.orientation.to_exif();
                    if exif_val != 1 {
                        println!("  EXIF orientation: {exif_val}");
                    }
                    // Show gain map presence.
                    let gm = &info.gain_map;
                    println!(
                        "  Gain map: {}",
                        match gm {
                            zencodec::gainmap::GainMapPresence::Absent => "none",
                            zencodec::gainmap::GainMapPresence::Unknown => "unknown",
                            zencodec::gainmap::GainMapPresence::Available(_) => "present",
                            _ => "unknown",
                        }
                    );
                }
            }
            Err(e) => {
                eprintln!("error: {path_str}: {e}");
                any_failed = true;
            }
        }
    }

    if args.json {
        if json_results.len() == 1 {
            println!(
                "{}",
                serde_json::to_string_pretty(&json_results[0]).unwrap()
            );
        } else {
            println!("{}", serde_json::to_string_pretty(&json_results).unwrap());
        }
    }

    if any_failed {
        ExitCode::from(EXIT_INPUT_ERROR)
    } else {
        ExitCode::from(EXIT_SUCCESS)
    }
}

/// Run the `compare` subcommand.
pub fn run_compare(args: CompareArgs) -> ExitCode {
    let data_a = match std::fs::read(&args.a) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {}: {e}", args.a);
            return ExitCode::from(EXIT_INPUT_ERROR);
        }
    };
    let data_b = match std::fs::read(&args.b) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {}: {e}", args.b);
            return ExitCode::from(EXIT_INPUT_ERROR);
        }
    };

    // Decode both images to RGBA8 via zencodecs.
    let decoded_a = match zencodecs::DecodeRequest::new(&data_a).decode_full_frame() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {}: decode failed: {e}", args.a);
            return ExitCode::from(EXIT_INPUT_ERROR);
        }
    };
    let decoded_b = match zencodecs::DecodeRequest::new(&data_b).decode_full_frame() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {}: decode failed: {e}", args.b);
            return ExitCode::from(EXIT_INPUT_ERROR);
        }
    };

    let (wa, ha) = (decoded_a.width(), decoded_a.height());
    let (wb, hb) = (decoded_b.width(), decoded_b.height());

    println!("A: {}x{}", wa, ha);
    println!("B: {}x{}", wb, hb);

    if wa != wb || ha != hb {
        eprintln!("warning: images have different dimensions — metrics may not be meaningful");
    }

    // Compute PSNR (simple, always available).
    let pixels_a = decoded_a.pixels().as_strided_bytes();
    let pixels_b = decoded_b.pixels().as_strided_bytes();

    let min_len = pixels_a.len().min(pixels_b.len());
    if min_len > 0 {
        let mse: f64 = pixels_a[..min_len]
            .iter()
            .zip(&pixels_b[..min_len])
            .map(|(&a, &b)| {
                let diff = a as f64 - b as f64;
                diff * diff
            })
            .sum::<f64>()
            / min_len as f64;

        if mse == 0.0 {
            println!("PSNR: inf (identical)");
        } else {
            let psnr = 10.0 * (255.0_f64 * 255.0 / mse).log10();
            println!("PSNR: {psnr:.1} dB");
        }
    }

    // Note: SSIMULACRA2 and Butteraugli require the fast-ssim2 and butteraugli
    // crates respectively. We compute PSNR as a baseline metric that's always
    // available. Full perceptual metrics can be added when those deps are wired.
    eprintln!("note: SSIMULACRA2 and Butteraugli metrics require additional dependencies");

    ExitCode::from(EXIT_SUCCESS)
}

fn format_name(fmt: zencodecs::ImageFormat) -> &'static str {
    match fmt {
        zencodecs::ImageFormat::Jpeg => "JPEG",
        zencodecs::ImageFormat::Png => "PNG",
        zencodecs::ImageFormat::WebP => "WebP",
        zencodecs::ImageFormat::Gif => "GIF",
        zencodecs::ImageFormat::Avif => "AVIF",
        zencodecs::ImageFormat::Jxl => "JPEG XL",
        zencodecs::ImageFormat::Heic => "HEIC",
        zencodecs::ImageFormat::Bmp => "BMP",
        zencodecs::ImageFormat::Tiff => "TIFF",
        _ => "Unknown",
    }
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
