//! Pristine 1/N downscale — artifact-free PNG references from lossy corpus sources.
//!
//! # Why 1/8 specifically
//!
//! A JPEG 8x8 luma block, box-averaged down to a single pixel, **is** that
//! block's DC coefficient. Every AC coefficient — every ringing, blocking and
//! mosquito artifact the quantiser introduced — integrates to zero across the
//! block. So an exactly-aligned 1/8 box downscale removes luma artifacts by
//! construction rather than by heuristic, which is what makes the output usable
//! as a clean reference for measuring a restoration model.
//!
//! Two honest limits, recorded rather than hidden:
//!
//! - **Chroma survives.** At 4:2:0 the chroma planes are half-resolution, so one
//!   chroma 8x8 block spans 16x16 luma pixels; at 1/8 that leaves 2x2 chroma
//!   pixels per block and chroma AC is only partly cancelled. Full chroma
//!   cancellation needs 1/16, which costs more resolution than it is worth —
//!   chroma artifacts sit far below luma in both visual and metric weight.
//! - **Only the box kernel has the DC property.** Mitchell and Lanczos mix
//!   neighbouring blocks and trade exact artifact cancellation for less content
//!   aliasing. `--filter` exposes the choice so it can be settled by measurement
//!   instead of assertion; the kernel is a recorded axis in the variant manifest
//!   (VARIANTS-SPEC v2 §4).
//!
//! # Alignment
//!
//! Partial MCUs at the right/bottom edge are exactly where edge artifacts live,
//! and a block that is not whole cannot have its AC cancelled. The source is
//! therefore cropped to a whole multiple of N before resampling, dropping at
//! most N-1 pixels per axis. The crop is reported in the output row so the
//! discarded region is never silent.
//!
//! # Everything here is imazen-native
//!
//! zencodecs decode (jpeg/heic/png/avif/jxl, gain maps, cICP) -> zenresize
//! linear-light resample -> zencodecs PNG encode carrying cICP through. No
//! foreign imaging library, and in particular no cICP-splicing post-pass: the
//! 2026-06 HDR renderer needed one because it went through the `image` crate,
//! and zenresize then hardcoded the sRGB transfer. zenresize now carries real
//! `Pq`/`Hlg` curves and *rejects* an unknown transfer, so HDR resamples in
//! correct linear light on the same path as SDR.
//!
//! # Usage
//!
//! ```text
//! pristine_downscale --ratio 8 --filter box <src> <dst.png>
//! pristine_downscale --ratio 8 --filter box --tsv out.tsv <src...> --out-dir DIR
//! ```
//!
//! Emits one TSV row per file: src, out, kernel, ratio, source w/h, crop w/h,
//! out w/h, transfer, cicp, output size. Content hashing is the variant
//! generator's job (`make_variant_set.py` already hashes for `variants.tsv`),
//! so this tool does not duplicate it or pull a hash dependency.

use std::path::{Path, PathBuf};

use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};
use zenpixels::{PixelSlice, TransferFunction};
use zenresize::{Filter, ResizeConfig, Resizer};

fn filter_by_name(name: &str) -> Option<Filter> {
    Some(match name {
        "box" => Filter::Box,
        "mitchell" => Filter::Mitchell,
        "lanczos" | "lanczos3" => Filter::Lanczos,
        "catmullrom" => Filter::CatmullRom,
        "triangle" => Filter::Triangle,
        _ => return None,
    })
}

/// What one source produced, for the manifest row.
struct Rendered {
    png: Vec<u8>,
    src_w: u32,
    src_h: u32,
    crop_w: u32,
    crop_h: u32,
    out_w: u32,
    out_h: u32,
    transfer: TransferFunction,
    cicp: String,
}

fn render(src: &Path, ratio: u32, filter: Filter) -> Result<Rendered, String> {
    let data = std::fs::read(src).map_err(|e| format!("read: {e}"))?;
    let decoded = DecodeRequest::new(&data)
        .decode_full_frame()
        .map_err(|e| format!("decode: {e}"))?;

    let info_cicp = decoded.info().source_color.cicp;
    let transfer = decoded.info().source_color.transfer_function();
    let has_alpha = decoded.has_alpha();
    let pixels = decoded.pixels();
    let descriptor = pixels.descriptor();
    let (src_w, src_h) = (pixels.width(), pixels.rows());

    // Whole blocks only — a partial MCU cannot have its AC cancelled.
    let crop_w = (src_w / ratio) * ratio;
    let crop_h = (src_h / ratio) * ratio;
    if crop_w == 0 || crop_h == 0 {
        return Err(format!(
            "source {src_w}x{src_h} is smaller than one {ratio}x{ratio} block"
        ));
    }
    let (out_w, out_h) = (crop_w / ratio, crop_h / ratio);

    // The descriptor carries the transfer function, so zenresize linearizes with
    // the correct curve (sRGB / PQ / HLG) instead of assuming sRGB.
    let config = ResizeConfig::builder(src_w, src_h, out_w, out_h)
        .filter(filter)
        .format(descriptor)
        .crop(0, 0, crop_w, crop_h)
        .build();
    let resized = Resizer::new(&config).resize(pixels.as_strided_bytes());

    let out_stride = (out_w as usize) * descriptor.bytes_per_pixel();
    let out_slice = PixelSlice::new(&resized, out_w, out_h, out_stride, descriptor)
        .map_err(|e| format!("wrap resized: {e}"))?;

    let mut req = EncodeRequest::new(ImageFormat::Png).with_lossless(true);
    if let Some(c) = info_cicp {
        req = req.with_cicp(Some(c));
    }
    let png = req
        .encode(out_slice, has_alpha)
        .map_err(|e| format!("encode: {e}"))?;

    Ok(Rendered {
        png: png.into_vec(),
        src_w,
        src_h,
        crop_w,
        crop_h,
        out_w,
        out_h,
        transfer,
        cicp: match info_cicp {
            Some(c) => format!(
                "{}/{}/{}",
                c.color_primaries, c.transfer_characteristics, c.matrix_coefficients
            ),
            None => "-".to_string(),
        },
    })
}

fn main() -> std::process::ExitCode {
    let mut ratio: u32 = 8;
    let mut filter_name = "box".to_string();
    let mut out_dir: Option<PathBuf> = None;
    let mut tsv: Option<PathBuf> = None;
    let mut inputs: Vec<PathBuf> = Vec::new();
    let mut explicit_out: Option<PathBuf> = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ratio" => {
                i += 1;
                ratio = args[i].parse().expect("--ratio N");
            }
            "--filter" => {
                i += 1;
                filter_name = args[i].clone();
            }
            "--out-dir" => {
                i += 1;
                out_dir = Some(PathBuf::from(&args[i]));
            }
            "--tsv" => {
                i += 1;
                tsv = Some(PathBuf::from(&args[i]));
            }
            other => inputs.push(PathBuf::from(other)),
        }
        i += 1;
    }

    // Two-positional form: <src> <dst.png>
    if out_dir.is_none() && inputs.len() == 2 {
        explicit_out = Some(inputs.pop().unwrap());
    }
    if inputs.is_empty() {
        eprintln!("usage: pristine_downscale [--ratio N] [--filter box|mitchell|lanczos] \\");
        eprintln!("           <src> <dst.png>   |   --out-dir DIR [--tsv T] <src...>");
        return std::process::ExitCode::FAILURE;
    }
    let Some(filter) = filter_by_name(&filter_name) else {
        eprintln!("unknown --filter {filter_name}");
        return std::process::ExitCode::FAILURE;
    };
    if ratio < 2 {
        eprintln!("--ratio must be >= 2");
        return std::process::ExitCode::FAILURE;
    }

    let mut rows = String::from(
        "src\tout\tkernel\tratio\tsrc_w\tsrc_h\tcrop_w\tcrop_h\tout_w\tout_h\ttransfer\tcicp\tout_bytes\n",
    );
    let (mut ok, mut failed) = (0usize, 0usize);

    for src in &inputs {
        let dst = if let Some(ref d) = explicit_out {
            d.clone()
        } else {
            let stem = src.file_stem().unwrap_or_default().to_string_lossy();
            out_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(format!("{stem}.scale1of{ratio}.png"))
        };

        match render(src, ratio, filter) {
            Ok(r) => {
                if let Some(parent) = dst.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&dst, &r.png) {
                    eprintln!("FAIL {} — write: {e}", src.display());
                    failed += 1;
                    continue;
                }
                rows.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:?}\t{}\t{}\n",
                    src.display(),
                    dst.display(),
                    filter_name,
                    ratio,
                    r.src_w,
                    r.src_h,
                    r.crop_w,
                    r.crop_h,
                    r.out_w,
                    r.out_h,
                    r.transfer,
                    r.cicp,
                    r.png.len(),
                ));
                ok += 1;
            }
            Err(e) => {
                eprintln!("FAIL {} — {e}", src.display());
                failed += 1;
            }
        }
    }

    if let Some(path) = tsv {
        if let Err(e) = std::fs::write(&path, &rows) {
            eprintln!("could not write {}: {e}", path.display());
            return std::process::ExitCode::FAILURE;
        }
        eprintln!("wrote {}", path.display());
    }
    eprintln!("{ok} rendered, {failed} failed");

    // A partial run must not look like a clean one.
    if failed > 0 {
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
