//! Benchmark encode/decode across codecs with real images.
//!
//! Reads PNG/JPEG files from disk, decodes, and re-encodes to each format.
//!
//! Native:  `cargo run --example bench_wasm --release --features std -- <image_paths...>`
//! WASM:    Build with --target wasm32-wasip1 --release --features std
//!          `wasmtime --dir /path target/.../bench_wasm.wasm -- <image_paths...>`

use std::time::Instant;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, PixelBufferConvertTypedExt as _};

const ITERS: u32 = 3;

fn bench<F: FnMut() -> R, R>(label: &str, iters: u32, mut f: F) -> R {
    // Warmup
    let result = f();

    let start = Instant::now();
    for _ in 1..iters {
        let _ = f();
    }
    let elapsed = start.elapsed();
    let per_iter = elapsed / iters;
    println!("  {label}: {per_iter:?} ({iters} iters)");
    result
}

fn bench_image(path: &str) {
    let data = std::fs::read(path).expect("failed to read file");
    let filename = path.rsplit('/').next().unwrap_or(path);
    let file_size = data.len();

    println!(
        "--- {filename} ({:.1} MB) ---",
        file_size as f64 / 1_048_576.0
    );

    // Decode
    let decoded = bench("Decode", ITERS, || {
        DecodeRequest::new(&data).decode().unwrap()
    });

    let w = decoded.width();
    let h = decoded.height();
    let mpx = (w as f64 * h as f64) / 1_000_000.0;
    println!("  {w}x{h} ({mpx:.1} MP) {:?}", decoded.format());

    let rgb = decoded.into_buffer().to_rgb8();
    let img = rgb.as_imgref();

    // Encode to each format
    for (name, format) in [
        ("JPEG q80", ImageFormat::Jpeg),
        ("WebP q80", ImageFormat::WebP),
        ("PNG", ImageFormat::Png),
        ("GIF", ImageFormat::Gif),
    ] {
        let quality = match format {
            ImageFormat::Png | ImageFormat::Gif => None,
            _ => Some(80.0),
        };
        let result = bench(&format!("Encode {name}"), ITERS, || {
            let mut req = EncodeRequest::new(format);
            if let Some(q) = quality {
                req = req.with_quality(q);
            }
            req.encode_rgb8(img)
        });
        match result {
            Ok(encoded) => {
                let ratio = encoded.len() as f64 / (w * h * 3) as f64 * 100.0;
                println!("    -> {} bytes ({ratio:.1}% of raw)", encoded.len());
            }
            Err(e) => println!("    FAILED: {e}"),
        }
    }

    // Decode the encoded formats back
    println!("  Re-decode:");
    for (name, format) in [
        ("JPEG", ImageFormat::Jpeg),
        ("WebP", ImageFormat::WebP),
        ("PNG", ImageFormat::Png),
    ] {
        let quality = if format == ImageFormat::Png {
            None
        } else {
            Some(80.0)
        };
        let mut req = EncodeRequest::new(format);
        if let Some(q) = quality {
            req = req.with_quality(q);
        }
        let encoded = match req.encode_rgb8(img) {
            Ok(e) => e,
            Err(e) => {
                println!("  Decode {name}: SKIP (encode failed: {e})");
                continue;
            }
        };
        match bench(&format!("Decode {name}"), ITERS, || {
            DecodeRequest::new(encoded.data()).decode()
        }) {
            Ok(_) => {}
            Err(e) => println!("    FAILED: {e}"),
        }
    }

    println!();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("Usage: bench_wasm <image_path> [image_path...]");
        std::process::exit(1);
    }

    for path in &args {
        bench_image(path);
    }
}
