//! `zenpipe info` and `zenpipe compare` subcommand implementations.
//!
//! All image operations go through zenpipe::job. The CLI only does file I/O
//! and output formatting.

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

        // Probe via ImageJob.
        let probe = zenpipe::job::ImageJob::new()
            .add_input(0, data.clone())
            .probe();

        match probe {
            Ok(info) => {
                let bpp = info.bits_per_pixel(data.len());

                if args.json {
                    json_results.push(serde_json::json!({
                        "file": path_str,
                        "format": info.format_name(),
                        "width": info.width,
                        "height": info.height,
                        "has_alpha": info.has_alpha,
                        "mime_type": info.mime_type,
                        "size_bytes": data.len(),
                        "bits_per_pixel": bpp,
                    }));
                } else {
                    println!(
                        "{}: {} {}x{}, {}",
                        path_str,
                        info.format_name(),
                        info.width,
                        info.height,
                        if info.has_alpha { "alpha" } else { "opaque" },
                    );
                    println!("  Size: {} ({:.2} bpp)", format_size(data.len()), bpp);
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

/// Run the `compare` subcommand via zenpipe::job::compare().
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

    match zenpipe::job::compare(&data_a, &data_b) {
        Ok(result) => {
            println!("A: {}x{}", result.width_a, result.height_a);
            println!("B: {}x{}", result.width_b, result.height_b);
            if result.dimensions_differ {
                eprintln!(
                    "warning: images have different dimensions — metrics may not be meaningful"
                );
            }
            if result.psnr.is_infinite() {
                println!("PSNR: inf (identical)");
            } else {
                println!("PSNR: {:.1} dB", result.psnr);
            }
            ExitCode::from(EXIT_SUCCESS)
        }
        Err(e) => {
            eprintln!("error: compare failed: {e}");
            ExitCode::from(EXIT_INPUT_ERROR)
        }
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
