//! Every codec decode/encode path must complete inside a 512 KiB caller stack.
//!
//! Hosts that run codec work on bounded stacks (rayon workers sized below the
//! 2 MiB default, FFI/embedder threads, wasm) need a hard guarantee: a codec
//! call must not overflow ~512 KiB of stack. The zenavif-side twin
//! (`zenavif/tests/stack_headroom.rs`) covers each AV1 backend; this file
//! covers every OTHER codec route zencodecs exposes through
//! `DecodeRequest`/`EncodeRequest`.
//!
//! The historical failure pattern: `Box::new([elem; N])` materializes the
//! whole array on the stack before moving it to the heap — fixed by initial
//! heap allocation (`vec![..].into_boxed_slice().try_into()`, or a zeroed-box
//! helper for big structs).
//!
//! Each `#[test]` runs the codec call on a thread spawned with
//! `stack_size(ZENCODECS_STACK_KB)` (default 512). A stack overflow aborts the
//! process, so under `cargo test` one failing case kills the test binary —
//! run per-case (`cargo test --test stack_headroom <name>`) or under nextest
//! (process per test). `ZENCODECS_STACK_KB` exists for bisecting the true peak
//! when a case fails; the committed gate stays at 512.
//!
//! Input generation and corpus reads run on the parent thread: the contract
//! is about the codec call, not our fixtures.

use std::thread;

use imgref::{Img, ImgVec};
use rgb::{Rgb, Rgba};
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};

fn stack_size() -> usize {
    std::env::var("ZENCODECS_STACK_KB")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(512)
        * 1024
}

/// Run `f` on a thread whose stack is limited to `stack_size()` bytes.
fn on_limited_stack<F, T>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    thread::Builder::new()
        .stack_size(stack_size())
        .spawn(f)
        .expect("spawn limited-stack thread")
        .join()
        .unwrap_or_else(|e| {
            let msg = e
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| e.downcast_ref::<&'static str>().copied())
                .unwrap_or("<non-string panic>");
            panic!("codec call panicked on a limited stack: {msg}");
        })
}

/// Deterministic photo-ish content: gradients plus LCG noise so quantizers
/// and predictors do real work.
fn rgb_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let mut state = 0x2545_F491u32;
    let mut lcg = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 24) as u8
    };
    let mut buf = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let g = ((x * 255) / w.max(1)) as u8;
            let b = ((y * 255) / h.max(1)) as u8;
            let n = lcg() / 8;
            buf.push(Rgb {
                r: g.wrapping_add(n),
                g: b.wrapping_add(n / 2),
                b: ((u16::from(g) + u16::from(b)) / 2) as u8,
            });
        }
    }
    Img::new(buf, w, h)
}

fn rgba_image(w: usize, h: usize) -> ImgVec<Rgba<u8>> {
    let rgb = rgb_image(w, h);
    Img::new(
        rgb.pixels()
            .enumerate()
            .map(|(i, p)| Rgba {
                r: p.r,
                g: p.g,
                b: p.b,
                a: (i % 251) as u8,
            })
            .collect::<Vec<_>>(),
        w,
        h,
    )
}

/// Encode `img` as `format` (parent thread), decode the bytes on the limited
/// stack, then re-encode the decoded pixels on the limited stack — so one
/// test covers both directions of a codec.
fn roundtrip_limited(format: ImageFormat, w: usize, h: usize) {
    roundtrip_limited_as(format, w, h, None)
}

fn roundtrip_limited_as(format: ImageFormat, w: usize, h: usize, decode_as: Option<ImageFormat>) {
    let bytes = EncodeRequest::new(format)
        .with_quality(85.0)
        .encode_full_frame_rgb8(rgb_image(w, h).as_ref())
        .unwrap_or_else(|e| panic!("fixture encode {format:?}: {e}"));
    let decoded = on_limited_stack(move || {
        let req = DecodeRequest::new(bytes.as_ref());
        let req = match decode_as {
            Some(f) => req.with_format(f),
            None => req,
        };
        req.decode_full_frame().expect("decode")
    });
    assert!(decoded.width() > 0 && decoded.height() > 0);
    let re_encode = on_limited_stack(move || {
        EncodeRequest::new(format)
            .with_quality(85.0)
            .encode_full_frame_rgb8(rgb_image(w, h).as_ref())
            .expect("encode")
    });
    assert!(!re_encode.as_ref().is_empty());
}

fn decode_bytes_limited<T: AsRef<[u8]> + Send + 'static>(data: T) {
    let decoded = on_limited_stack(move || {
        DecodeRequest::new(data.as_ref())
            .decode_full_frame()
            .expect("decode")
    });
    assert!(decoded.width() > 0 && decoded.height() > 0);
}

fn corpus_dir(name: &str) -> std::path::PathBuf {
    codec_corpus::Corpus::new()
        .expect("codec-corpus init")
        .get(name)
        .unwrap_or_else(|e| panic!("codec-corpus {name}: {e}"))
}

fn first_file(dir: &std::path::Path, exts: &[&str]) -> Vec<u8> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        for e in std::fs::read_dir(&d)
            .unwrap_or_else(|e| panic!("read {}: {e}", d.display()))
            .flatten()
        {
            let p = e.path();
            if p.is_dir() {
                pending.push(p);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| exts.iter().any(|e| e.eq_ignore_ascii_case(x)))
            {
                files.push(p);
            }
        }
    }
    files.sort();
    let path = files
        .first()
        .unwrap_or_else(|| panic!("no {:?} files in {}", exts, dir.display()));
    std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Self-contained round trips: encode -> decode -> encode, both directions on
// the limited stack.
// ---------------------------------------------------------------------------

#[cfg(feature = "jpeg")]
#[test]
fn jpeg_roundtrip() {
    roundtrip_limited(ImageFormat::Jpeg, 256, 192);
    roundtrip_limited(ImageFormat::Jpeg, 1023, 767);
}

#[cfg(feature = "png")]
#[test]
fn png_roundtrip() {
    roundtrip_limited(ImageFormat::Png, 256, 192);
}

#[cfg(feature = "webp")]
#[test]
fn webp_roundtrip() {
    roundtrip_limited(ImageFormat::WebP, 256, 192);
}

#[cfg(feature = "gif")]
#[test]
fn gif_roundtrip() {
    roundtrip_limited(ImageFormat::Gif, 128, 96);
}

#[cfg(all(feature = "avif-decode", feature = "avif-encode"))]
#[test]
fn avif_roundtrip() {
    roundtrip_limited(ImageFormat::Avif, 256, 192);
}

#[cfg(all(feature = "jxl-decode", feature = "jxl-encode"))]
#[test]
fn jxl_roundtrip() {
    roundtrip_limited(ImageFormat::Jxl, 256, 192);
}

#[cfg(feature = "tiff")]
#[test]
fn tiff_roundtrip() {
    // The magic-byte sniffer misroutes TIFF into zenraw's rawler probe
    // (which rejects it as UnsupportedType), so force the format here.
    roundtrip_limited_as(ImageFormat::Tiff, 256, 192, Some(ImageFormat::Tiff));
}

#[cfg(feature = "bitmaps-bmp")]
#[test]
fn bmp_roundtrip() {
    roundtrip_limited(ImageFormat::Bmp, 256, 192);
}

#[cfg(feature = "bitmaps")]
#[test]
fn pnm_roundtrip() {
    roundtrip_limited(ImageFormat::Pnm, 256, 192);
}

#[cfg(feature = "bitmaps")]
#[test]
fn farbfeld_roundtrip() {
    // Farbfeld carries RGBA16; exercise the rgba input arm.
    let bytes = EncodeRequest::new(ImageFormat::Farbfeld)
        .encode_full_frame_rgba8(rgba_image(128, 96).as_ref())
        .expect("fixture encode farbfeld");
    decode_bytes_limited(bytes);
    let out = on_limited_stack(move || {
        EncodeRequest::new(ImageFormat::Farbfeld)
            .encode_full_frame_rgba8(rgba_image(128, 96).as_ref())
            .expect("encode")
    });
    assert!(!out.as_ref().is_empty());
}

// ---------------------------------------------------------------------------
// Decode-only paths — corpus fixtures.
// ---------------------------------------------------------------------------

#[cfg(feature = "heic-decode")]
#[test]
fn heic_decode() {
    let data = first_file(
        &corpus_dir("heic-conformance/valid"),
        &["heic", "heif"],
    );
    decode_bytes_limited(data);
}

#[cfg(feature = "raw-decode")]
#[test]
fn raw_decode() {
    let dir = corpus_dir("raw-conformance/valid/bayer");
    decode_bytes_limited(first_file(&dir, &["dng", "nef", "cr2", "arw"]));
}

#[cfg(feature = "pdf-decode")]
#[test]
fn pdf_decode() {
    let data = std::fs::read("tests/images/test.pdf").expect("test.pdf");
    decode_bytes_limited(data);
}
