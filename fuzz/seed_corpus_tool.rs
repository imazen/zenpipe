//! Cross-platform fuzz corpus seeder using codec-corpus.
//!
//! Downloads conformance test images for all supported formats and copies
//! them to fuzz/corpus/seed/. Works on Linux, macOS, and Windows.
//!
//! Usage: cargo run --manifest-path fuzz/Cargo.toml --bin seed_corpus_tool

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let corpus = codec_corpus::Corpus::new().expect("failed to init codec-corpus cache");
    let seed_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus").join("seed");

    let datasets: &[(&str, &str)] = &[
        // (corpus path, local subdirectory)
        ("jpeg-conformance", "jpeg"),
        ("mozjpeg", "jpeg"),
        ("png-conformance", "png"),
        ("pngsuite", "png"),
        ("gif-conformance", "gif"),
        ("webp-conformance", "webp"),
        ("avif-conformance", "avif"),
        ("jxl", "jxl"),
        ("heic-conformance", "heic"),
        ("bmp-conformance", "bitmaps"),
        ("pnm-conformance", "bitmaps"),
        ("farbfeld-conformance", "bitmaps"),
        ("qoi-benchmark", "qoi"),
        ("tiff-conformance", "tiff"),
        ("ultrahdr-conformance", "jpeg"),
        ("zune", "mixed"),
        ("image-rs", "mixed"),
    ];

    let mut total = 0usize;

    for (dataset, subdir) in datasets {
        let dst = seed_dir.join(subdir);
        fs::create_dir_all(&dst).ok();

        match corpus.get(dataset) {
            Ok(src) => {
                let count = copy_image_files(&src, &dst, 500);
                if count > 0 {
                    println!("  {subdir}: {count} files from {dataset}");
                    total += count;
                }
            }
            Err(e) => {
                eprintln!("  WARNING: {dataset}: {e}");
            }
        }
    }

    // Build mixed/ from all subdirs
    let mixed = seed_dir.join("mixed");
    fs::create_dir_all(&mixed).ok();
    let mut mixed_count = 0;
    for subdir in ["jpeg", "png", "gif", "webp", "avif", "jxl", "heic", "bitmaps", "qoi", "tiff"]
    {
        let src = seed_dir.join(subdir);
        if src.is_dir() {
            mixed_count += copy_image_files(&src, &mixed, 10);
        }
    }
    println!("  mixed: {mixed_count} files");
    total += mixed_count;

    println!("\nDone. Total seed files: {total}");
}

/// Copy up to `limit` image files from `src` to `dst` (non-recursive for
/// the top level, recursive into immediate subdirs). Skips files already
/// present. Returns count of files copied.
fn copy_image_files(src: &Path, dst: &Path, limit: usize) -> usize {
    let mut count = 0;
    let Ok(entries) = fs::read_dir(src) else {
        return 0;
    };
    for entry in entries {
        if count >= limit {
            break;
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            // Recurse one level
            count += copy_image_files(&path, dst, limit - count);
        } else if path.is_file() {
            let Some(name) = path.file_name() else {
                continue;
            };
            let dest = dst.join(name);
            if !dest.exists() {
                if fs::copy(&path, &dest).is_ok() {
                    count += 1;
                }
            }
        }
    }
    count
}
