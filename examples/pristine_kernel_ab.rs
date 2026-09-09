//! Which resampling kernel actually makes a 1/N downscale artifact-free?
//!
//! `pristine_downscale` exposes `--filter` rather than hardcoding one, because
//! the choice is a real trade and the workspace rule is that a constant landing
//! in a dataset gets measured, not asserted. This is that measurement.
//!
//! The claim under test: at exactly 1/8, a **box** kernel aligned to the JPEG
//! block grid averages each 8x8 luma block to its DC coefficient, so every AC
//! coefficient — every ringing and blocking artifact — cancels exactly. Mitchell
//! and Lanczos mix neighbouring blocks, so they should cancel *less*. Against
//! that, box is a poor antialiaser and should lose content fidelity.
//!
//! So two numbers per kernel, and they pull in opposite directions:
//!
//! - **rejection** = ssim2( down_K(clean), down_K(jpeg_of_clean) ). How much of
//!   the codec damage survives the downscale. Higher is better; 100 means the
//!   downscaled JPEG is indistinguishable from the downscaled original, which is
//!   exactly what "artifact-free reference" has to mean.
//! - **fidelity** = ssim2( down_lanczos(clean), down_K(clean) ). How far K's view
//!   of the *undamaged* source drifts from a properly antialiased one. Lanczos
//!   scores 100 by construction — it is the yardstick here, not a winner.
//!
//! Sources must be PNG with no JPEG history, or the "clean" side is not clean.
//!
//! Quality grid runs q10..q95 step 5: the low-q half is where artifacts are
//! structural and where the kernels should separate, and the workspace sweep
//! discipline forbids a grid that is denser at high q than low q.
//!
//! Usage:
//!   pristine_kernel_ab --ratio 8 --tsv out.tsv <clean.png...>

use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, PixelBufferConvertExt};
use zenpixels::{PixelDescriptor, PixelSlice};
use zenresize::{Filter, ResizeConfig, Resizer};

const KERNELS: &[(&str, Filter)] = &[
    ("box", Filter::Box),
    ("mitchell", Filter::Mitchell),
    ("lanczos", Filter::Lanczos),
];

/// Decode any supported source to packed RGB8 bytes.
fn to_rgb8_bytes(data: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let decoded = DecodeRequest::new(data)
        .decode_full_frame()
        .map_err(|e| format!("decode: {e}"))?;
    let (w, h) = (decoded.width(), decoded.height());
    // Untyped conversion to a packed RGB8/sRGB buffer — one descriptor-driven
    // step, so a 16-bit or YCbCr source lands here already normalised.
    let buf = decoded
        .into_buffer()
        .convert_to(PixelDescriptor::RGB8_SRGB)
        .map_err(|e| format!("to rgb8: {e}"))?;
    let px = buf
        .as_contiguous_pixels::<rgb::Rgb<u8>>()
        .ok_or("converted buffer is not contiguous RGB8")?;
    let mut out = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for p in px {
        out.extend_from_slice(&[p.r, p.g, p.b]);
    }
    Ok((out, w, h))
}

/// Crop to whole blocks, then downscale by `ratio` with `filter`.
fn downscale(
    rgb: &[u8],
    w: u32,
    h: u32,
    ratio: u32,
    filter: Filter,
) -> Result<(Vec<u8>, u32, u32), String> {
    let (crop_w, crop_h) = ((w / ratio) * ratio, (h / ratio) * ratio);
    if crop_w == 0 || crop_h == 0 {
        return Err("source smaller than one block".into());
    }
    let (ow, oh) = (crop_w / ratio, crop_h / ratio);
    let desc = PixelDescriptor::RGB8_SRGB;
    let config = ResizeConfig::builder(w, h, ow, oh)
        .filter(filter)
        .format(desc)
        .crop(0, 0, crop_w, crop_h)
        .build();
    Ok((Resizer::new(&config).resize(rgb), ow, oh))
}

fn ssim2(a: &[u8], b: &[u8], w: u32, h: u32) -> Result<f64, String> {
    let pack =
        |v: &[u8]| -> Vec<[u8; 3]> { v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect() };
    let (pa, pb) = (pack(a), pack(b));
    let ia = imgref::Img::new(pa, w as usize, h as usize);
    let ib = imgref::Img::new(pb, w as usize, h as usize);
    fast_ssim2::compute_ssimulacra2(ia.as_ref(), ib.as_ref()).map_err(|e| format!("ssim2: {e}"))
}

fn main() -> std::process::ExitCode {
    let mut ratio: u32 = 8;
    let mut tsv: Option<String> = None;
    let mut inputs: Vec<String> = Vec::new();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ratio" => {
                i += 1;
                ratio = args[i].parse().expect("--ratio N");
            }
            "--tsv" => {
                i += 1;
                tsv = Some(args[i].clone());
            }
            other => inputs.push(other.to_string()),
        }
        i += 1;
    }
    if inputs.is_empty() {
        eprintln!("usage: pristine_kernel_ab [--ratio N] --tsv OUT <clean.png...>");
        return std::process::ExitCode::FAILURE;
    }

    let qs: Vec<u32> = (10..=95).step_by(5).collect();
    let mut rows = String::from("src\tq\tkernel\trejection_ssim2\tfidelity_ssim2\n");
    let (mut ok, mut failed) = (0usize, 0usize);

    for src in &inputs {
        let Ok(data) = std::fs::read(src) else {
            eprintln!("FAIL {src} — unreadable");
            failed += 1;
            continue;
        };
        let (clean, w, h) = match to_rgb8_bytes(&data) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("FAIL {src} — {e}");
                failed += 1;
                continue;
            }
        };

        // Fidelity yardstick: a well-antialiased view of the undamaged source.
        let Ok((ref_lanczos, ow, oh)) = downscale(&clean, w, h, ratio, Filter::Lanczos) else {
            eprintln!("FAIL {src} — reference downscale");
            failed += 1;
            continue;
        };

        // down_K(clean) per kernel — reused across every q.
        let mut clean_down = Vec::new();
        for (name, f) in KERNELS {
            match downscale(&clean, w, h, ratio, *f) {
                Ok((px, _, _)) => clean_down.push((*name, px)),
                Err(e) => eprintln!("FAIL {src} {name} — {e}"),
            }
        }

        for &q in &qs {
            // `encode` consumes the slice, so it is rebuilt per q — the wrap is a
            // bounds check over borrowed bytes, not a copy.
            let slice =
                match PixelSlice::new(&clean, w, h, (w as usize) * 3, PixelDescriptor::RGB8_SRGB) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("FAIL {src} — wrap: {e}");
                        failed += 1;
                        continue;
                    }
                };
            let Ok(jpg) = EncodeRequest::new(ImageFormat::Jpeg)
                .with_quality(q as f32)
                .encode(slice, false)
            else {
                eprintln!("FAIL {src} q{q} — encode");
                failed += 1;
                continue;
            };
            let (damaged, dw, dh) = match to_rgb8_bytes(jpg.data()) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("FAIL {src} q{q} — redecode: {e}");
                    failed += 1;
                    continue;
                }
            };
            if (dw, dh) != (w, h) {
                eprintln!("FAIL {src} q{q} — roundtrip changed dims");
                failed += 1;
                continue;
            }

            for (name, f) in KERNELS {
                let Ok((dmg_down, _, _)) = downscale(&damaged, dw, dh, ratio, *f) else {
                    failed += 1;
                    continue;
                };
                let Some((_, cln_down)) = clean_down.iter().find(|(n, _)| n == name) else {
                    continue;
                };
                let rejection = match ssim2(cln_down, &dmg_down, ow, oh) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("FAIL {src} q{q} {name} — {e}");
                        failed += 1;
                        continue;
                    }
                };
                let fidelity = match ssim2(&ref_lanczos, cln_down, ow, oh) {
                    Ok(v) => v,
                    Err(_) => f64::NAN,
                };
                rows.push_str(&format!(
                    "{src}\t{q}\t{name}\t{rejection:.4}\t{fidelity:.4}\n"
                ));
                ok += 1;
            }
        }
        eprintln!("done {src}");
    }

    if let Some(path) = tsv {
        if let Err(e) = std::fs::write(&path, &rows) {
            eprintln!("could not write {path}: {e}");
            return std::process::ExitCode::FAILURE;
        }
        eprintln!("wrote {path}");
    }
    eprintln!("{ok} cells, {failed} failures");
    if failed > 0 {
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
