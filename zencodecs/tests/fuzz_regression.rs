//! Fuzz crash regression suite — replays every committed seed on stable.
//!
//! `fuzz/regression/` holds minimized inputs that once crashed a fuzz target
//! and have since been fixed. Replaying them needs neither nightly nor
//! `cargo-fuzz`: this is a plain `cargo test` that drives the same entry
//! points the targets drive, so the seeds gate every CI run instead of only
//! the hand-run fuzzing sessions.
//!
//! Every seed goes through EVERY entry point, not just the one that found it.
//! The seeds are format-detected bytes, not target-specific fixtures, and a
//! bug reached from one dispatch path is usually reachable from the others —
//! `fuzz/regression/fuzz_depthmap/` is the standing example: depth-map decode
//! was removed in 2026-06-25, so that seed has no target of its own left, but
//! its bytes still exercise format detection and the decode dispatcher.
//!
//! To add a seed: drop the (preferably `cargo fuzz tmin`-minimized) file into
//! `fuzz/regression/<target>/` with a `crash-<sha>` name. Nothing else.
//!
//! Keep the limits below in sync with `fuzz/fuzz_targets/*.rs` — they are
//! deliberately tighter than production defaults so a decompression bomb
//! reports as a rejected input rather than as a timeout.

use std::fs;
use std::path::{Path, PathBuf};

use zencodecs::{AllowedFormats, DecodeRequest, Limits};

/// Mirrors the `Limits` every `zencodecs/fuzz` target builds.
fn fuzz_limits() -> Limits {
    Limits::none()
        .with_max_width(4096)
        .with_max_height(4096)
        .with_max_pixels(4_000_000)
        .with_max_memory(64 * 1024 * 1024)
        .with_max_frames(50)
}

fn regression_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fuzz/regression")
}

/// Every file under `fuzz/regression/`, paired with the directory that names
/// the target it was found by (kept only for failure messages).
fn seeds() -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, target: &str, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(target)
                    .to_string();
                walk(&path, &name, out);
            } else if path.is_file() {
                out.push((target.to_string(), path));
            }
        }
    }

    let mut out = Vec::new();
    walk(&regression_dir(), "regression", &mut out);
    out.sort();
    out
}

// ── entry points, one per `zencodecs/fuzz` target ────────────────────────

/// `fuzz_decode`
fn run_decode_full_frame(data: &[u8]) {
    let limits = fuzz_limits();
    let _ = DecodeRequest::new(data)
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .decode_full_frame();
}

/// `fuzz_animation`
fn run_animation(data: &[u8]) {
    let limits = fuzz_limits();
    let decoder = DecodeRequest::new(data)
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .animation_frame_decoder();

    if let Ok(mut dec) = decoder {
        for _ in 0..100 {
            match dec.render_next_frame_owned(None) {
                Ok(Some(_frame)) => {}
                Ok(None) | Err(_) => break,
            }
        }
    }
}

/// `fuzz_gainmap` — only reachable with the UltraHDR stack compiled in.
#[cfg(feature = "jpeg-ultrahdr")]
fn run_gain_map(data: &[u8]) {
    let limits = fuzz_limits();
    let _ = DecodeRequest::new(data)
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .decode_gain_map();
}

#[cfg(not(feature = "jpeg-ultrahdr"))]
fn run_gain_map(_data: &[u8]) {}

/// `fuzz_push_decode` — the streaming path, which is separate code from
/// `decode_full_frame`. Sink logic mirrors the fuzz target's `CountingSink`.
struct CountingSink {
    buf: Vec<u8>,
    rows: u32,
    max_rows: u32,
    width: u32,
    bpp: usize,
}

impl zencodec::decode::DecodeRowSink for CountingSink {
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: zenpixels::PixelDescriptor,
    ) -> Result<(), zencodec::decode::SinkError> {
        if height > self.max_rows || width > 1024 {
            return Err("dimensions exceed fuzz limit".into());
        }
        self.width = width;
        self.bpp = descriptor.bytes_per_pixel();
        let strip_bytes = width as usize * self.bpp * 16;
        self.buf.resize(strip_bytes.min(1024 * 1024), 0);
        Ok(())
    }

    fn provide_next_buffer(
        &mut self,
        _y: u32,
        height: u32,
        width: u32,
        descriptor: zenpixels::PixelDescriptor,
    ) -> Result<zenpixels::PixelSliceMut<'_>, zencodec::decode::SinkError> {
        self.rows = self.rows.saturating_add(height);
        if self.rows > self.max_rows {
            return Err("row count exceeds fuzz limit".into());
        }
        let stride = width as usize * self.bpp;
        let needed = stride * height as usize;
        if needed > self.buf.len() {
            self.buf.resize(needed, 0);
        }
        zenpixels::PixelSliceMut::new(&mut self.buf[..needed], width, height, stride, descriptor)
            .map_err(|e| -> zencodec::decode::SinkError { format!("{e}").into() })
    }

    fn finish(&mut self) -> Result<(), zencodec::decode::SinkError> {
        Ok(())
    }
}

fn run_push_decode(data: &[u8]) {
    let limits = fuzz_limits();
    let mut sink = CountingSink {
        buf: Vec::new(),
        rows: 0,
        max_rows: 1024,
        width: 0,
        bpp: 4,
    };
    let _ = DecodeRequest::new(data)
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .push_decode(&mut sink);
}

// ── the gate ─────────────────────────────────────────────────────────────

/// The suite must never silently pass because the seed directory moved or
/// emptied out. No `|| true`-shaped escape hatch anywhere in this file.
#[test]
fn regression_corpus_is_present() {
    let dir = regression_dir();
    assert!(
        dir.is_dir(),
        "{} is missing — the regression seeds are committed, not generated",
        dir.display()
    );
    assert!(
        !seeds().is_empty(),
        "{} contains no seed files; a suite that tests nothing is worse than \
         one that fails",
        dir.display()
    );
}

#[test]
fn regression_seeds_do_not_panic() {
    for (target, path) in seeds() {
        let data = fs::read(&path)
            .unwrap_or_else(|e| panic!("failed to read seed {}: {e}", path.display()));
        // Panics abort the test with the seed already named by the harness
        // output; each helper is intentionally infallible-by-ignoring-Err,
        // because the contract under test is "no panic / no hang", not
        // "decodes successfully".
        eprintln!("replaying {target}: {}", path.display());
        run_decode_full_frame(&data);
        run_animation(&data);
        run_gain_map(&data);
        run_push_decode(&data);
    }
}
