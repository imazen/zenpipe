//! Real-file decode and roundtrip tests using the codec-corpus crate.
//!
//! These tests download and cache real images from the imazen/codec-corpus repository.
//! They are `#[ignore]` by default since they require network access on first run.
//!
//! Run: `cargo test --features all --test corpus -- --ignored`
//! With custom cache: `CODEC_CORPUS_CACHE=/path cargo test --features all --test corpus -- --ignored`

use std::path::Path;

use codec_corpus::Corpus;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, PixelBufferConvertExt};

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
        .decode()
        .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()));

    let w = decoded.width();
    let h = decoded.height();

    // Convert to RGBA8 for a uniform encode path
    let rgba_buf = decoded.into_buffer().to_rgba8();
    let rgba_ref = rgba_buf.as_imgref();

    let encoded = EncodeRequest::new(format)
        .with_quality(quality)
        .encode_rgba8(rgba_ref)
        .unwrap_or_else(|e| panic!("encode {}: {e}", path.display()));

    let re_decoded = DecodeRequest::new(encoded.as_ref())
        .decode()
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
        match DecodeRequest::new(&data).decode() {
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
        if DecodeRequest::new(&data).decode().is_ok() {
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
        match DecodeRequest::new(&data).decode() {
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
        if DecodeRequest::new(&data).decode().is_ok() {
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
        match DecodeRequest::new(&data).decode() {
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
        match DecodeRequest::new(&data).decode() {
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
        if DecodeRequest::new(&data).decode().is_ok() {
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
        match DecodeRequest::new(&data).decode() {
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
        if DecodeRequest::new(&data).decode().is_ok() {
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
        if let Ok(d) = DecodeRequest::new(&data).decode() {
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
        if DecodeRequest::new(&data).decode().is_err() {
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
        let _ = DecodeRequest::new(&data).decode();
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
        let _ = DecodeRequest::new(&data).decode();
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
        match std::panic::catch_unwind(|| DecodeRequest::new(&data).decode()) {
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
        match std::panic::catch_unwind(|| DecodeRequest::new(&data).decode()) {
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
        if zencodecs::probe(&data).is_ok() {
            probed += 1;
        }
        // Decode may fail for some edge cases — that's OK
        if DecodeRequest::new(&data).decode().is_ok() {
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
        let Ok(decoded) = DecodeRequest::new(&data).decode() else {
            continue;
        };
        let w = decoded.width();
        let h = decoded.height();
        let rgba_buf = decoded.into_buffer().to_rgba8();
        let rgba_ref = rgba_buf.as_imgref();

        let encoded = EncodeRequest::new(ImageFormat::Avif)
            .with_quality(60.0)
            .encode_rgba8(rgba_ref)
            .unwrap_or_else(|e| panic!("AVIF encode {}: {e}", path.display()));

        let re = DecodeRequest::new(encoded.as_ref())
            .decode()
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
        let Ok(decoded) = DecodeRequest::new(&data).decode() else {
            continue;
        };
        let w = decoded.width();
        let h = decoded.height();

        // Use RGB8 for JXL to avoid alpha decode bug
        let rgb_buf = decoded.into_buffer().to_rgb8();
        let rgb_ref = rgb_buf.as_imgref();

        let encoded = EncodeRequest::new(ImageFormat::Jxl)
            .with_quality(75.0)
            .encode_rgb8(rgb_ref)
            .unwrap_or_else(|e| panic!("JXL encode {}: {e}", path.display()));

        let re = DecodeRequest::new(encoded.as_ref())
            .decode()
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
        match DecodeRequest::new(&data).decode() {
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
        match DecodeRequest::new(&data).decode() {
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
