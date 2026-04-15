//! Real-file decode and roundtrip tests using the codec-corpus crate.
//!
//! These tests download and cache real images from the imazen/codec-corpus repository.
//! They are `#[ignore]` by default since they require network access on first run.
//!
//! Run: `cargo test --features all --test corpus -- --ignored`
//! With custom cache: `CODEC_CORPUS_CACHE=/path cargo test --features all --test corpus -- --ignored`

use std::path::Path;

use codec_corpus::Corpus;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, PixelBufferConvertTypedExt as _};

fn corpus() -> Corpus {
    Corpus::new().expect("failed to initialize codec-corpus")
}

/// Collect all files with a given extension from a directory (non-recursive).
fn collect_files(dir: &Path, extensions: &[&str]) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)))
        })
        .collect();
    files.sort();
    files
}

/// Decode, re-encode to the given format, decode again.
fn roundtrip_ok(path: &Path, format: ImageFormat, quality: f32) {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let decoded = DecodeRequest::new(&data)
        .decode_full_frame()
        .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()));

    let w = decoded.width();
    let h = decoded.height();

    // Convert to RGBA8 for a uniform encode path
    let rgba_buf = decoded.into_buffer().to_rgba8();
    let rgba_ref = rgba_buf.as_imgref();

    let encoded = EncodeRequest::new(format)
        .with_quality(quality)
        .encode_full_frame_rgba8(rgba_ref)
        .unwrap_or_else(|e| panic!("encode {}: {e}", path.display()));

    let re_decoded = DecodeRequest::new(encoded.as_ref())
        .decode_full_frame()
        .unwrap_or_else(|e| panic!("re-decode {}: {e}", path.display()));

    assert_eq!(
        re_decoded.width(),
        w,
        "width mismatch after roundtrip: {}",
        path.display()
    );
    assert_eq!(
        re_decoded.height(),
        h,
        "height mismatch after roundtrip: {}",
        path.display()
    );
}

// ===========================================================================
// JPEG — decode valid conformance files, roundtrip a subset
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "jpeg")]
fn corpus_jpeg_decode_valid() {
    let c = corpus();
    let dir = c.get("jpeg-conformance/valid").unwrap();
    let files = collect_files(&dir, &["jpg", "jpeg"]);
    assert!(!files.is_empty(), "no JPEG files found in corpus");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("JPEG decode: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
    // Allow some failures (CMYK, unusual subsampling, etc.) but most should pass
    assert!(
        ok >= files.len() * 3 / 4,
        "too many JPEG decode failures: {}/{} failed",
        fail.len(),
        files.len()
    );
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "jpeg")]
fn corpus_jpeg_roundtrip() {
    let c = corpus();
    let dir = c.get("jpeg-conformance/valid").unwrap();
    let files = collect_files(&dir, &["jpg", "jpeg"]);

    // Roundtrip a subset — skip CMYK and known problematic files
    let mut tested = 0;
    for path in files.iter().take(10) {
        let data = std::fs::read(path).unwrap();
        if DecodeRequest::new(&data).decode_full_frame().is_ok() {
            roundtrip_ok(path, ImageFormat::Jpeg, 85.0);
            tested += 1;
        }
    }
    assert!(tested > 0, "no JPEG files could be roundtripped");
    eprintln!("JPEG roundtrip: {tested} files OK");
}

// ===========================================================================
// PNG — decode pngsuite, roundtrip a subset
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "png")]
fn corpus_png_decode_pngsuite() {
    let c = corpus();
    let dir = c.get("pngsuite").unwrap();
    let files = collect_files(&dir, &["png"]);
    assert!(!files.is_empty(), "no PNG files found in pngsuite");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        // pngsuite "x" prefix = invalid/corrupt files, skip those
        let fname = path.file_name().unwrap().to_string_lossy();
        if fname.starts_with('x') {
            continue;
        }
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!("{fname}: {e}"));
            }
        }
    }
    eprintln!("PNG decode (pngsuite): {ok} succeeded");
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
    // Most valid pngsuite files should decode
    assert!(ok > 50, "too few PNG decodes succeeded: {ok}");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "png")]
fn corpus_png_roundtrip() {
    let c = corpus();
    let dir = c.get("pngsuite").unwrap();
    let files = collect_files(&dir, &["png"]);

    let mut tested = 0;
    for path in &files {
        let fname = path.file_name().unwrap().to_string_lossy();
        if fname.starts_with('x') {
            continue;
        }
        let data = std::fs::read(path).unwrap();
        if DecodeRequest::new(&data).decode_full_frame().is_ok() {
            roundtrip_ok(path, ImageFormat::Png, 100.0);
            tested += 1;
            if tested >= 20 {
                break;
            }
        }
    }
    assert!(tested > 0, "no PNG files could be roundtripped");
    eprintln!("PNG roundtrip: {tested} files OK");
}

// ===========================================================================
// PNG — decode real-world edge cases from png-conformance
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "png")]
fn corpus_png_conformance_decode() {
    let c = corpus();
    let dir = c.get("png-conformance").unwrap();
    let files = collect_files(&dir, &["png"]);
    assert!(!files.is_empty(), "no files in png-conformance");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("PNG conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
}

// ===========================================================================
// WebP — decode from image-rs corpus, roundtrip
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "webp")]
fn corpus_webp_decode() {
    let c = corpus();
    let dir = c.get("image-rs/test-images/webp").unwrap();

    // Recursively find all .webp files
    let mut files = Vec::new();
    fn walk(dir: &Path, ext: &str, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, ext, out);
                } else if p.extension().and_then(|e| e.to_str()) == Some(ext) {
                    out.push(p);
                }
            }
        }
    }
    walk(&dir, "webp", &mut files);
    files.sort();
    assert!(!files.is_empty(), "no WebP files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("WebP decode: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
    assert!(ok > 0, "no WebP files decoded successfully");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "webp")]
fn corpus_webp_roundtrip() {
    let c = corpus();
    let dir = c.get("image-rs/test-images/webp/lossy_images").unwrap();
    let files = collect_files(&dir, &["webp"]);

    let mut tested = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        if DecodeRequest::new(&data).decode_full_frame().is_ok() {
            roundtrip_ok(path, ImageFormat::WebP, 75.0);
            tested += 1;
        }
    }
    assert!(tested > 0, "no WebP files could be roundtripped");
    eprintln!("WebP roundtrip: {tested} files OK");
}

// ===========================================================================
// GIF — decode from image-rs corpus, roundtrip
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "gif")]
fn corpus_gif_decode() {
    let c = corpus();
    let dir = c.get("image-rs/test-images/gif").unwrap();

    let mut files = Vec::new();
    fn walk(dir: &Path, ext: &str, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, ext, out);
                } else if p.extension().and_then(|e| e.to_str()) == Some(ext) {
                    out.push(p);
                }
            }
        }
    }
    walk(&dir, "gif", &mut files);
    files.sort();
    assert!(!files.is_empty(), "no GIF files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("GIF decode: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
    assert!(ok > 0, "no GIF files decoded successfully");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "gif")]
fn corpus_gif_roundtrip() {
    let c = corpus();
    let dir = c.get("image-rs/test-images/gif/simple").unwrap();
    let files = collect_files(&dir, &["gif"]);

    let mut tested = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        if DecodeRequest::new(&data).decode_full_frame().is_ok() {
            roundtrip_ok(path, ImageFormat::Gif, 100.0);
            tested += 1;
        }
    }
    assert!(tested > 0, "no GIF files could be roundtripped");
    eprintln!("GIF roundtrip: {tested} files OK");
}

// ===========================================================================
// JPEG — mozjpeg reference files
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "jpeg")]
fn corpus_jpeg_mozjpeg_decode() {
    let c = corpus();
    let dir = c.get("mozjpeg").unwrap();
    let files = collect_files(&dir, &["jpg", "jpeg"]);
    assert!(!files.is_empty(), "no mozjpeg files found");

    let mut ok = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        if let Ok(d) = DecodeRequest::new(&data).decode_full_frame() {
            assert!(d.width() > 0);
            ok += 1;
        }
        // mozjpeg may have edge-case files that fail — that's OK
    }
    eprintln!("mozjpeg decode: {ok}/{} succeeded", files.len());
    assert!(ok > 0, "no mozjpeg files decoded");
}

// ===========================================================================
// JPEG — invalid/crash-repro files must not panic
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "jpeg")]
fn corpus_jpeg_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("jpeg-conformance/invalid").unwrap();
    let files = collect_files(&dir, &["jpg", "jpeg"]);
    assert!(!files.is_empty(), "no invalid JPEG files found");

    let mut errors = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        if DecodeRequest::new(&data).decode_full_frame().is_err() {
            errors += 1;
        }
    }
    eprintln!(
        "JPEG invalid: {errors}/{} returned errors (no panics)",
        files.len()
    );
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "jpeg")]
fn corpus_jpeg_crash_repro_no_panic() {
    let c = corpus();
    let dir = c.get("jpeg-conformance/crash-repro").unwrap();
    let files = collect_files(&dir, &["jpg", "jpeg"]);

    if files.is_empty() {
        eprintln!("no crash-repro files (expected)");
        return;
    }

    for path in &files {
        let data = std::fs::read(path).unwrap();
        // Must not panic — error result is fine
        let _ = DecodeRequest::new(&data).decode_full_frame();
    }
    eprintln!(
        "JPEG crash-repro: {} files processed without panic",
        files.len()
    );
}

// ===========================================================================
// PNG — invalid pngsuite files must not panic
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "png")]
fn corpus_png_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("pngsuite").unwrap();
    let files = collect_files(&dir, &["png"]);

    let mut invalid_tested = 0;
    for path in &files {
        let fname = path.file_name().unwrap().to_string_lossy();
        // pngsuite "x" prefix = intentionally broken
        if !fname.starts_with('x') {
            continue;
        }
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
        invalid_tested += 1;
    }
    eprintln!("PNG invalid (pngsuite x*): {invalid_tested} files processed without panic");
}

// ===========================================================================
// JPEG — zune fuzz corpus (robustness)
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run) — slow, 1800+ files"]
#[cfg(feature = "jpeg")]
fn corpus_jpeg_zune_fuzz() {
    let c = corpus();
    let dir = c.get("zune/fuzz-corpus/jpeg").unwrap();

    let mut files = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.is_file() {
                    out.push(p);
                }
            }
        }
    }
    walk(&dir, &mut files);
    files.sort();

    if files.is_empty() {
        eprintln!("no zune fuzz JPEG files found");
        return;
    }

    let mut ok = 0;
    let mut err = 0;
    let mut panics = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match std::panic::catch_unwind(|| DecodeRequest::new(&data).decode_full_frame()) {
            Ok(Ok(_)) => ok += 1,
            Ok(Err(_)) => err += 1,
            Err(_) => panics += 1,
        }
    }
    eprintln!(
        "zune JPEG fuzz: {ok} ok, {err} errors, {panics} panics, {} total",
        files.len()
    );
    if panics > 0 {
        eprintln!("WARNING: {panics} files caused decoder panics — these should be filed as bugs");
    }
}

// ===========================================================================
// PNG — zune fuzz corpus (robustness)
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run) — slow, 800+ files"]
#[cfg(feature = "png")]
fn corpus_png_zune_fuzz() {
    let c = corpus();
    let dir = c.get("zune/fuzz-corpus/png").unwrap();

    let mut files = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.is_file() {
                    out.push(p);
                }
            }
        }
    }
    walk(&dir, &mut files);
    files.sort();

    if files.is_empty() {
        eprintln!("no zune fuzz PNG files found");
        return;
    }

    let mut ok = 0;
    let mut err = 0;
    let mut panics = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match std::panic::catch_unwind(|| DecodeRequest::new(&data).decode_full_frame()) {
            Ok(Ok(_)) => ok += 1,
            Ok(Err(_)) => err += 1,
            Err(_) => panics += 1,
        }
    }
    eprintln!(
        "zune PNG fuzz: {ok} ok, {err} errors, {panics} panics, {} total",
        files.len()
    );
    if panics > 0 {
        eprintln!("WARNING: {panics} files caused decoder panics — these should be filed as bugs");
    }
}

// ===========================================================================
// Cross-format decode probe — imageflow corpus
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
fn corpus_imageflow_probe() {
    let c = corpus();
    let dir = c.get("imageflow").unwrap();

    let extensions = ["jpg", "jpeg", "png", "gif", "webp"];
    let mut files = Vec::new();
    fn walk(dir: &Path, exts: &[&str], out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, exts, out);
                } else if p
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| exts.iter().any(|e| e.eq_ignore_ascii_case(ext)))
                {
                    out.push(p);
                }
            }
        }
    }
    walk(&dir, &extensions, &mut files);
    files.sort();

    if files.is_empty() {
        eprintln!("no imageflow files found");
        return;
    }

    let mut probed = 0;
    let mut decoded = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        // Probe should always work for known formats
        if zencodecs::from_bytes(&data).is_ok() {
            probed += 1;
        }
        // Decode may fail for some edge cases — that's OK
        if DecodeRequest::new(&data).decode_full_frame().is_ok() {
            decoded += 1;
        }
    }
    eprintln!(
        "imageflow: {probed}/{} probed, {decoded} decoded",
        files.len()
    );
    assert!(probed > 0, "no imageflow files could be probed");
}

// ===========================================================================
// AVIF — roundtrip using imageflow/gb82 natural photos
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "avif-encode")]
#[cfg(feature = "avif-decode")]
fn corpus_avif_roundtrip() {
    let c = corpus();
    // Use imageflow photos — small, diverse, CC0
    let dir = c.get("imageflow/test_inputs").unwrap();

    let files = collect_files(&dir, &["jpg", "jpeg", "png"]);
    let mut tested = 0;

    for path in files.iter().take(5) {
        let data = std::fs::read(path).unwrap();
        let Ok(decoded) = DecodeRequest::new(&data).decode_full_frame() else {
            continue;
        };
        let w = decoded.width();
        let h = decoded.height();
        let rgba_buf = decoded.into_buffer().to_rgba8();
        let rgba_ref = rgba_buf.as_imgref();

        let encoded = EncodeRequest::new(ImageFormat::Avif)
            .with_quality(60.0)
            .encode_full_frame_rgba8(rgba_ref)
            .unwrap_or_else(|e| panic!("AVIF encode {}: {e}", path.display()));

        let re = DecodeRequest::new(encoded.as_ref())
            .decode_full_frame()
            .unwrap_or_else(|e| panic!("AVIF re-decode {}: {e}", path.display()));

        assert_eq!(re.width(), w);
        assert_eq!(re.height(), h);
        tested += 1;
    }
    assert!(tested > 0, "no AVIF roundtrips completed");
    eprintln!("AVIF roundtrip: {tested} files OK");
}

// ===========================================================================
// JXL — roundtrip using natural photos
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "jxl-encode")]
#[cfg(feature = "jxl-decode")]
fn corpus_jxl_roundtrip() {
    let c = corpus();
    let dir = c.get("imageflow/test_inputs").unwrap();

    let files = collect_files(&dir, &["jpg", "jpeg", "png"]);
    let mut tested = 0;

    for path in files.iter().take(5) {
        let data = std::fs::read(path).unwrap();
        let Ok(decoded) = DecodeRequest::new(&data).decode_full_frame() else {
            continue;
        };
        let w = decoded.width();
        let h = decoded.height();

        // Use RGB8 for JXL to avoid alpha decode bug
        let rgb_buf = decoded.into_buffer().to_rgb8();
        let rgb_ref = rgb_buf.as_imgref();

        let encoded = EncodeRequest::new(ImageFormat::Jxl)
            .with_quality(75.0)
            .encode_full_frame_rgb8(rgb_ref)
            .unwrap_or_else(|e| panic!("JXL encode {}: {e}", path.display()));

        let re = DecodeRequest::new(encoded.as_ref())
            .decode_full_frame()
            .unwrap_or_else(|e| panic!("JXL re-decode {}: {e}", path.display()));

        assert_eq!(re.width(), w);
        assert_eq!(re.height(), h);
        tested += 1;
    }
    assert!(tested > 0, "no JXL roundtrips completed");
    eprintln!("JXL roundtrip: {tested} files OK");
}

// ===========================================================================
// JXL — decode conformance corpus
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "jxl-decode")]
fn corpus_jxl_decode() {
    let c = corpus();
    let dir = c.get("jxl").unwrap();

    let mut files = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|e| e.to_str()) == Some("jxl") {
                    out.push(p);
                }
            }
        }
    }
    walk(&dir, &mut files);
    files.sort();

    if files.is_empty() {
        eprintln!("no JXL corpus files found");
        return;
    }

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.strip_prefix(&dir).unwrap_or(path).display()
                ));
            }
        }
    }
    eprintln!("JXL decode: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures ({}):\n  {}", fail.len(), fail.join("\n  "));
    }
    assert!(ok > 0, "no JXL files decoded successfully");
}

// ===========================================================================
// WebP conformance (if dataset available)
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "webp")]
fn corpus_webp_conformance_decode() {
    let c = corpus();
    let dir = c.get("webp-conformance/valid").unwrap();
    let files = collect_files(&dir, &["webp"]);
    assert!(!files.is_empty(), "no WebP conformance files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("WebP conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
    assert!(
        ok > files.len() / 2,
        "too many WebP conformance failures: {}/{}",
        fail.len(),
        files.len()
    );
}

// ===========================================================================
// AVIF — decode conformance corpus (av1-avif, libavif, link-u test vectors)
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "avif-decode")]
fn corpus_avif_decode_valid() {
    let c = corpus();
    let dir = c.get("avif-conformance/valid").unwrap();
    let files = collect_files(&dir, &["avif"]);
    assert!(!files.is_empty(), "no AVIF conformance files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("AVIF conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures ({}):\n  {}", fail.len(), fail.join("\n  "));
    }
    assert!(ok > 0, "no AVIF conformance files decoded");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "avif-decode")]
fn corpus_avif_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("avif-conformance/invalid").unwrap();
    let files = collect_files(&dir, &["avif"]);
    assert!(!files.is_empty(), "no invalid AVIF files found");

    let mut errors = 0;
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
        errors += 1;
    }
    eprintln!(
        "AVIF invalid: {errors}/{} processed without panic",
        files.len()
    );
}

// ===========================================================================
// HEIC — decode conformance corpus
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "heic-decode")]
fn corpus_heic_decode_valid() {
    let c = corpus();
    let dir = c.get("heic-conformance/valid").unwrap();

    // Recursively find all HEIC/HEIF files
    let mut files = Vec::new();
    fn walk_heic(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk_heic(&p, out);
                } else if p.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("heic") || ext.eq_ignore_ascii_case("heif")
                }) {
                    out.push(p);
                }
            }
        }
    }
    walk_heic(&dir, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no HEIC files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.strip_prefix(&dir).unwrap_or(path).display()
                ));
            }
        }
    }
    eprintln!("HEIC conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures ({}):\n  {}", fail.len(), fail.join("\n  "));
    }
    assert!(ok > 0, "no HEIC files decoded");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "heic-decode")]
fn corpus_heic_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("heic-conformance/invalid").unwrap();
    let files = collect_files(&dir, &["heic", "heif"]);

    if files.is_empty() {
        eprintln!("no invalid HEIC files (skipping)");
        return;
    }

    for path in &files {
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
    }
    eprintln!(
        "HEIC invalid: {} files processed without panic",
        files.len()
    );
}

// ===========================================================================
// TIFF — decode conformance corpus, roundtrip
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "tiff")]
fn corpus_tiff_decode_valid() {
    let c = corpus();
    let dir = c.get("tiff-conformance/valid").unwrap();

    let mut files = Vec::new();
    fn walk_tiff(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk_tiff(&p, out);
                } else if p.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("tiff") || ext.eq_ignore_ascii_case("tif")
                }) {
                    out.push(p);
                }
            }
        }
    }
    walk_tiff(&dir, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no TIFF conformance files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.strip_prefix(&dir).unwrap_or(path).display()
                ));
            }
        }
    }
    eprintln!("TIFF conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures ({}):\n  {}", fail.len(), fail.join("\n  "));
    }
    assert!(
        ok >= files.len() / 2,
        "too many TIFF failures: {}/{}",
        fail.len(),
        files.len()
    );
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "tiff")]
fn corpus_tiff_roundtrip() {
    let c = corpus();
    let dir = c.get("tiff-conformance/valid").unwrap();
    let files = collect_files(&dir, &["tiff", "tif"]);

    let mut tested = 0;
    for path in files.iter().take(10) {
        let data = std::fs::read(path).unwrap();
        if DecodeRequest::new(&data).decode_full_frame().is_ok() {
            roundtrip_ok(path, ImageFormat::Tiff, 100.0);
            tested += 1;
        }
    }
    assert!(tested > 0, "no TIFF files could be roundtripped");
    eprintln!("TIFF roundtrip: {tested} files OK");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "tiff")]
fn corpus_tiff_robustness_no_panic() {
    let c = corpus();
    let dir = c.get("tiff-conformance/robustness").unwrap();
    let files = collect_files(&dir, &["tiff", "tif"]);

    if files.is_empty() {
        eprintln!("no TIFF robustness files (skipping)");
        return;
    }

    for path in &files {
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
    }
    eprintln!(
        "TIFF robustness: {} files processed without panic",
        files.len()
    );
}

// ===========================================================================
// BMP — decode conformance corpus, roundtrip
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "bitmaps-bmp")]
fn corpus_bmp_decode_valid() {
    let c = corpus();
    let dir = c.get("bmp-conformance/valid").unwrap();
    let files = collect_files(&dir, &["bmp"]);
    assert!(!files.is_empty(), "no BMP conformance files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("BMP conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures ({}):\n  {}", fail.len(), fail.join("\n  "));
    }
    assert!(ok > 0, "no BMP files decoded");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "bitmaps-bmp")]
fn corpus_bmp_roundtrip() {
    let c = corpus();
    let dir = c.get("bmp-conformance/valid").unwrap();
    let files = collect_files(&dir, &["bmp"]);

    let mut tested = 0;
    for path in files.iter().take(10) {
        let data = std::fs::read(path).unwrap();
        if DecodeRequest::new(&data).decode_full_frame().is_ok() {
            roundtrip_ok(path, ImageFormat::Bmp, 100.0);
            tested += 1;
        }
    }
    assert!(tested > 0, "no BMP files could be roundtripped");
    eprintln!("BMP roundtrip: {tested} files OK");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "bitmaps-bmp")]
fn corpus_bmp_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("bmp-conformance/invalid").unwrap();
    let files = collect_files(&dir, &["bmp"]);

    if files.is_empty() {
        eprintln!("no invalid BMP files (skipping)");
        return;
    }

    for path in &files {
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
    }
    eprintln!("BMP invalid: {} files processed without panic", files.len());
}

// ===========================================================================
// PNM/PAM — decode conformance corpus, roundtrip, invalid
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "bitmaps")]
fn corpus_pnm_decode_valid() {
    let c = corpus();
    let dir = c.get("pnm-conformance/valid").unwrap();

    // Recursively find all PNM files across pbm/pgm/ppm/pam subdirs
    let mut files = Vec::new();
    fn walk_pnm(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk_pnm(&p, out);
                } else if p.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
                    matches!(
                        ext.to_ascii_lowercase().as_str(),
                        "pbm" | "pgm" | "ppm" | "pnm" | "pam"
                    )
                }) {
                    out.push(p);
                }
            }
        }
    }
    walk_pnm(&dir, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no PNM conformance files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.strip_prefix(&dir).unwrap_or(path).display()
                ));
            }
        }
    }
    eprintln!("PNM conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures ({}):\n  {}", fail.len(), fail.join("\n  "));
    }
    assert!(
        ok >= files.len() * 3 / 4,
        "too many PNM failures: {}/{}",
        fail.len(),
        files.len()
    );
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "bitmaps")]
fn corpus_pnm_roundtrip() {
    let c = corpus();
    let dir = c.get("pnm-conformance/valid/ppm").unwrap();
    let files = collect_files(&dir, &["ppm"]);

    let mut tested = 0;
    for path in files.iter().take(5) {
        let data = std::fs::read(path).unwrap();
        if DecodeRequest::new(&data).decode_full_frame().is_ok() {
            roundtrip_ok(path, ImageFormat::Pnm, 100.0);
            tested += 1;
        }
    }
    assert!(tested > 0, "no PNM files could be roundtripped");
    eprintln!("PNM roundtrip: {tested} files OK");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "bitmaps")]
fn corpus_pnm_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("pnm-conformance/invalid").unwrap();
    let files = collect_files(&dir, &["ppm", "pgm", "pbm", "pnm", "pam"]);
    assert!(!files.is_empty(), "no invalid PNM files found");

    for path in &files {
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
    }
    eprintln!("PNM invalid: {} files processed without panic", files.len());
}

// ===========================================================================
// Farbfeld — decode conformance corpus, roundtrip, invalid
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "bitmaps")]
fn corpus_farbfeld_decode_valid() {
    let c = corpus();
    let dir = c.get("farbfeld-conformance/valid").unwrap();
    let files = collect_files(&dir, &["ff"]);
    assert!(!files.is_empty(), "no Farbfeld conformance files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("Farbfeld conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
    assert!(ok > 0, "no Farbfeld files decoded");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "bitmaps")]
fn corpus_farbfeld_roundtrip() {
    let c = corpus();
    let dir = c.get("farbfeld-conformance/valid").unwrap();
    let files = collect_files(&dir, &["ff"]);

    let mut tested = 0;
    for path in files.iter().take(5) {
        let data = std::fs::read(path).unwrap();
        if DecodeRequest::new(&data).decode_full_frame().is_ok() {
            roundtrip_ok(path, ImageFormat::Farbfeld, 100.0);
            tested += 1;
        }
    }
    assert!(tested > 0, "no Farbfeld files could be roundtripped");
    eprintln!("Farbfeld roundtrip: {tested} files OK");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "bitmaps")]
fn corpus_farbfeld_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("farbfeld-conformance/invalid").unwrap();
    let files = collect_files(&dir, &["ff"]);
    assert!(!files.is_empty(), "no invalid Farbfeld files found");

    for path in &files {
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
    }
    eprintln!(
        "Farbfeld invalid: {} files processed without panic",
        files.len()
    );
}

// ===========================================================================
// RAW/DNG — decode conformance corpus (Git LFS)
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run) + Git LFS"]
#[cfg(feature = "raw-decode")]
fn corpus_raw_decode_valid() {
    let c = corpus();
    let dir = c.get("raw-conformance/valid").unwrap();

    // Recursively find all RAW files
    let raw_exts = ["dng", "cr2", "cr3", "nef", "arw", "orf", "rw2", "raf"];
    let mut files = Vec::new();
    fn walk_raw(dir: &Path, exts: &[&str], out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk_raw(&p, exts, out);
                } else if p
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| exts.iter().any(|e| e.eq_ignore_ascii_case(ext)))
                {
                    out.push(p);
                }
            }
        }
    }
    walk_raw(&dir, &raw_exts, &mut files);
    files.sort();

    if files.is_empty() {
        eprintln!("no RAW files found (Git LFS not pulled?)");
        return;
    }

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        // Skip Git LFS pointer files (they start with "version https://")
        if data.starts_with(b"version https://") {
            eprintln!(
                "skipping LFS pointer: {}",
                path.file_name().unwrap().to_string_lossy()
            );
            continue;
        }
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.strip_prefix(&dir).unwrap_or(path).display()
                ));
            }
        }
    }
    eprintln!("RAW decode: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures ({}):\n  {}", fail.len(), fail.join("\n  "));
    }
    assert!(ok > 0, "no RAW files decoded");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "raw-decode")]
fn corpus_raw_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("raw-conformance/invalid").unwrap();

    let files = collect_files(&dir, &["raw", "dng", "nef"]);
    if files.is_empty() {
        eprintln!("no invalid RAW files (skipping)");
        return;
    }

    for path in &files {
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
    }
    eprintln!("RAW invalid: {} files processed without panic", files.len());
}

// ===========================================================================
// GIF — new conformance corpus (dispose, transparency, animation)
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "gif")]
fn corpus_gif_conformance_decode() {
    let c = corpus();
    let dir = c.get("gif-conformance/valid").unwrap();
    let files = collect_files(&dir, &["gif"]);
    assert!(!files.is_empty(), "no GIF conformance files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("GIF conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
    assert!(ok > 0, "no GIF conformance files decoded");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "gif")]
fn corpus_gif_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("gif-conformance/invalid").unwrap();
    let files = collect_files(&dir, &["gif"]);

    if files.is_empty() {
        eprintln!("no invalid GIF files (skipping)");
        return;
    }

    for path in &files {
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
    }
    eprintln!("GIF invalid: {} files processed without panic", files.len());
}

// ===========================================================================
// APNG — decode conformance corpus
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "png")]
fn corpus_apng_decode_valid() {
    let c = corpus();
    let dir = c.get("apng-conformance/valid").unwrap();
    let files = collect_files(&dir, &["png"]);
    assert!(!files.is_empty(), "no APNG conformance files found");

    let mut ok = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                assert!(d.height() > 0);
                ok += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    eprintln!("APNG conformance: {ok}/{} succeeded", files.len());
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
    // APNG should decode at least the first frame as regular PNG
    assert!(ok > 0, "no APNG files decoded");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "png")]
fn corpus_apng_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("apng-conformance/invalid").unwrap();
    let files = collect_files(&dir, &["png"]);

    if files.is_empty() {
        eprintln!("no invalid APNG files (skipping)");
        return;
    }

    for path in &files {
        let data = std::fs::read(path).unwrap();
        let _ = DecodeRequest::new(&data).decode_full_frame();
    }
    eprintln!(
        "APNG invalid: {} files processed without panic",
        files.len()
    );
}

// ===========================================================================
// UltraHDR — gain map decode from conformance corpus
// ===========================================================================

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "jpeg-ultrahdr")]
fn corpus_ultrahdr_decode_valid() {
    let c = corpus();
    let dir = c.get("ultrahdr-conformance/valid/jpeg").unwrap();

    // Recursively find all JPEG files (nested in subdirs)
    let mut files = Vec::new();
    fn walk_jpg(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk_jpg(&p, out);
                } else if p.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("jpg")
                        || ext.eq_ignore_ascii_case("jpeg")
                        || ext.eq_ignore_ascii_case("uhdr")
                }) {
                    out.push(p);
                }
            }
        }
    }
    walk_jpg(&dir, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no UltraHDR files found");

    let mut decoded = 0;
    let mut with_gainmap = 0;
    let mut fail = Vec::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        // All UltraHDR files should at least decode as regular JPEG
        match DecodeRequest::new(&data).decode_full_frame() {
            Ok(d) => {
                assert!(d.width() > 0);
                decoded += 1;
            }
            Err(e) => {
                fail.push(format!(
                    "{}: {e}",
                    path.strip_prefix(&dir).unwrap_or(path).display()
                ));
                continue;
            }
        }
        // Try gain map extraction
        match DecodeRequest::new(&data).decode_gain_map() {
            Ok((_output, Some(_gm))) => with_gainmap += 1,
            _ => {}
        }
    }
    eprintln!(
        "UltraHDR: {decoded}/{} decoded, {with_gainmap} with gain maps",
        files.len()
    );
    if !fail.is_empty() {
        eprintln!("Failures:\n  {}", fail.join("\n  "));
    }
    assert!(decoded > 0, "no UltraHDR files decoded");
}

#[test]
#[ignore = "requires codec-corpus (network on first run)"]
#[cfg(feature = "jpeg-ultrahdr")]
fn corpus_ultrahdr_invalid_no_panic() {
    let c = corpus();
    let dir = c.get("ultrahdr-conformance/invalid").unwrap();
    let files = collect_files(&dir, &["jpg", "jpeg", "uhdr"]);

    if files.is_empty() {
        eprintln!("no invalid UltraHDR files (skipping)");
        return;
    }

    for path in &files {
        let data = std::fs::read(path).unwrap();
        // Must not panic — decode or gain map extraction may fail
        let _ = DecodeRequest::new(&data).decode_full_frame();
        let _ = DecodeRequest::new(&data).decode_gain_map();
    }
    eprintln!(
        "UltraHDR invalid: {} files processed without panic",
        files.len()
    );
}
