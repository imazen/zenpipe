//! End-to-end checks of the `zencodecs` binary (zenpipe#68).

use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zencodecs"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("zencodecs-cli");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir.join(name)
}

/// A 48×48 RGB PNG with enough texture that lossy WebP is not pixel-exact.
fn noisy_png() -> Vec<u8> {
    let (w, h) = (48u32, 48u32);
    let stride = w as usize * 3;
    let mut px = vec![0u8; stride * h as usize];
    let mut seed = 0x2545_F491u32;
    for p in px.as_chunks_mut::<3>().0 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        p[0] = seed as u8;
        p[1] = (seed >> 8) as u8;
        p[2] = (seed >> 16) as u8;
    }
    let ps = zenpixels::PixelSlice::new(&px, w, h, stride, zenpixels::PixelDescriptor::RGB8_SRGB)
        .unwrap();
    zencodecs::EncodeRequest::new(zencodecs::ImageFormat::Png)
        .with_lossless(true)
        .encode(ps, false)
        .expect("png encode")
        .into_vec()
}

fn run_convert(input: &PathBuf, output: &PathBuf, extra: &[&str]) -> Vec<u8> {
    let st = bin()
        .arg("convert")
        .arg(input)
        .arg(output)
        .arg("--quiet")
        .args(extra)
        .status()
        .expect("spawn zencodecs");
    assert!(st.success(), "convert {extra:?} failed: {st}");
    std::fs::read(output).expect("read output")
}

#[test]
fn lossless_if_cheaper_picks_by_size_factor() {
    let input = scratch("lic-in.png");
    std::fs::write(&input, noisy_png()).unwrap();

    let lossless = run_convert(&input, &scratch("lic-lossless.webp"), &["--lossless"]);
    let lossy = run_convert(&input, &scratch("lic-lossy.webp"), &["--quality", "75"]);
    assert_ne!(lossless, lossy, "fixture must separate lossless from lossy");

    // Generous factor: lossless is within budget → lossless bytes, verbatim.
    let kept = run_convert(
        &input,
        &scratch("lic-keep.webp"),
        &["--quality", "75", "--lossless-if-cheaper", "1000"],
    );
    assert_eq!(kept, lossless, "factor 1000 must keep the lossless encode");

    // Tiny factor: lossless can never be within budget → the lossy bytes.
    let fell = run_convert(
        &input,
        &scratch("lic-fall.webp"),
        &["--quality", "75", "--lossless-if-cheaper", "0.001"],
    );
    assert_eq!(
        fell, lossy,
        "factor 0.001 must fall back to the lossy encode"
    );
}

#[test]
fn lossless_if_cheaper_rejects_bad_factor_and_lossy_only_formats() {
    let input = scratch("lic-bad-in.png");
    std::fs::write(&input, noisy_png()).unwrap();
    let st = bin()
        .args(["convert"])
        .arg(&input)
        .arg(scratch("lic-bad.webp"))
        .args(["--lossless-if-cheaper", "0"])
        .status()
        .unwrap();
    assert!(!st.success(), "factor 0 must be rejected");
    let st = bin()
        .args(["convert"])
        .arg(&input)
        .arg(scratch("lic-bad.jpg"))
        .args(["--lossless-if-cheaper"])
        .status()
        .unwrap();
    assert!(!st.success(), "JPEG has no lossless mode");
}

#[test]
fn probe_emits_json() {
    let input = scratch("probe-in.png");
    std::fs::write(&input, noisy_png()).unwrap();
    let out = bin().arg("probe").arg(&input).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("\"format\":\"png\""), "{s}");
    assert!(s.contains("\"width\":48"), "{s}");
}
