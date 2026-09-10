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
//! 2026-06 HDR renderer needed one because it went through the `image` crate.
//!
//! That renderer's other warning still stands, and this tool works around it
//! rather than having been freed from it. zenresize's **u8** path dispatches on
//! the descriptor's transfer, so 8-bit sRGB resamples in correct linear light
//! for free. Its **u16** path does not: `resize_u16` linearizes with a
//! hardcoded sRGB curve (its own doc comment says so), which is wrong for
//! PQ/HLG. So 16-bit sources are linearized here with their own curve
//! (`zenresize::Pq`/`Hlg` through `TransferCurve`), resized as linear f32, and
//! re-encoded -- see `render`. Verified on the corpus HDR layer: PQ read from
//! cICP, 16-bit RGB out, cICP preserved byte-for-byte.
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

use zencodecs::{ColorEmitPolicy, DecodeRequest, EncodeRequest, ImageFormat};
use zenpixels::{ColorPrimaries, PixelSlice, TransferFunction};
use zenresize::{Bt709, Filter, Hlg, Pq, ResizeConfig, Resizer, Srgb, TransferCurve};

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

/// The source's real transfer curve. Refuses to guess: a wrong curve here
/// corrupts highlights silently, which is worse than failing.
fn curve_for(tf: TransferFunction) -> Result<Box<dyn TransferCurve<Luts = ()>>, String> {
    match tf {
        TransferFunction::Srgb => Ok(Box::new(Srgb)),
        TransferFunction::Pq => Ok(Box::new(Pq)),
        TransferFunction::Hlg => Ok(Box::new(Hlg)),
        TransferFunction::Bt709 => Ok(Box::new(Bt709)),
        other => Err(format!("no transfer curve for {other:?}")),
    }
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
    transfer_assumed: bool,
    color_untagged: bool,
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
    let (src_w, src_h) = (pixels.width(), pixels.rows());

    // Reconcile the descriptor with the source's own colour before doing
    // anything with it. A decoder can hand back pixels whose descriptor says
    // BT.709 while the container's cICP says Display-P3 -- measured on the
    // corpus HEICs -- and then the resample linearizes with one story while the
    // file gets written with the other. The container is the authority on what
    // the colour IS, so it wins here and descriptor and tag agree from this
    // point on.
    let mut descriptor = pixels.descriptor();
    // Only when the container ACTUALLY declares a colour. `color_primaries()`
    // returns a default rather than an Option, so overriding unconditionally
    // stamps BT.709 over a decoder-supplied Display-P3 descriptor whenever the
    // source carries its colour some other way (an ICC profile, say). Measured:
    // doing that dropped 18 of 44 Display-P3 tags in this corpus.
    if info_cicp.is_some() {
        let src_primaries = decoded.info().color_primaries();
        if descriptor.primaries != src_primaries {
            descriptor.primaries = src_primaries;
        }
    }
    let src_transfer = decoded.info().transfer_function();
    if descriptor.transfer == TransferFunction::Unknown && src_transfer != TransferFunction::Unknown
    {
        descriptor.transfer = src_transfer;
    }
    // Only after the container has had its say: an untagged JPEG genuinely IS
    // sRGB by web convention, and most of this corpus is untagged. zenresize
    // refuses to resample through an unknown transfer rather than guessing,
    // which is right, so the assumption is made here, explicitly, and recorded
    // per row -- an assumed transfer is a fact about the reference set a
    // consumer needs, not an implementation detail to bury.
    let transfer_assumed = descriptor.transfer == TransferFunction::Unknown
        || descriptor.primaries == ColorPrimaries::Unknown;
    if descriptor.transfer == TransferFunction::Unknown {
        descriptor.transfer = TransferFunction::Srgb;
    }
    if descriptor.primaries == ColorPrimaries::Unknown {
        descriptor.primaries = ColorPrimaries::Bt709;
    }

    // Whole blocks only — a partial MCU cannot have its AC cancelled.
    let crop_w = (src_w / ratio) * ratio;
    let crop_h = (src_h / ratio) * ratio;
    if crop_w == 0 || crop_h == 0 {
        return Err(format!(
            "source {src_w}x{src_h} is smaller than one {ratio}x{ratio} block"
        ));
    }
    let (out_w, out_h) = (crop_w / ratio, crop_h / ratio);

    // zenresize's u8 path dispatches on the descriptor's transfer, so 8-bit
    // sRGB resamples in correct linear light for free. Its u16 path does NOT:
    // `resize_u16` linearizes with a hardcoded sRGB curve, which is wrong for
    // PQ/HLG and would corrupt highlights — exactly the trap the 2026-06 HDR
    // renderer documented, and it is still open in zenresize 0.3.1.
    //
    // So anything that is not 8-bit is linearized here with its OWN curve,
    // resampled as linear f32, and re-encoded. That keeps one code path honest
    // for SDR and HDR instead of silently mis-resampling the HDR half.
    let src_bytes = pixels.as_strided_bytes();
    let bpp = descriptor.bytes_per_pixel();
    let channels = descriptor.channels();
    let is_u8 = descriptor.channel_type() == zenpixels::ChannelType::U8;

    let (resized, out_desc) = if is_u8 {
        let config = ResizeConfig::builder(src_w, src_h, out_w, out_h)
            .filter(filter)
            .format(descriptor)
            .crop(0, 0, crop_w, crop_h)
            .build();
        (Resizer::new(&config).resize(src_bytes), descriptor)
    } else {
        if descriptor.channel_type() != zenpixels::ChannelType::U16 {
            return Err(format!(
                "unsupported sample type {:?}; only U8 and U16 sources are handled",
                descriptor.channel_type()
            ));
        }
        let curve = curve_for(descriptor.transfer)?;
        let stride = pixels.stride();

        // u16 -> linear f32, via the source's real transfer curve.
        let mut lin = Vec::with_capacity((src_w as usize) * (src_h as usize) * channels);
        for y in 0..src_h as usize {
            let row = &src_bytes[y * stride..y * stride + (src_w as usize) * bpp];
            for c in row.chunks_exact(2) {
                let v = u16::from_ne_bytes([c[0], c[1]]) as f32 / 65535.0;
                lin.push(curve.to_linear(v));
            }
        }

        let lin_desc = match channels {
            3 => zenpixels::PixelDescriptor::RGBF32_LINEAR,
            4 => zenpixels::PixelDescriptor::RGBAF32_LINEAR,
            1 => zenpixels::PixelDescriptor::GRAYF32_LINEAR,
            n => return Err(format!("unsupported channel count {n}")),
        };
        let config = ResizeConfig::builder(src_w, src_h, out_w, out_h)
            .filter(filter)
            .format(lin_desc)
            .crop(0, 0, crop_w, crop_h)
            .build();
        let out_lin = Resizer::new(&config).resize_f32(&lin);

        // linear f32 -> u16, same curve back.
        let mut out16 = Vec::with_capacity(out_lin.len() * 2);
        for v in &out_lin {
            let e = (curve.from_linear(*v).clamp(0.0, 1.0) * 65535.0).round() as u16;
            out16.extend_from_slice(&e.to_ne_bytes());
        }
        (out16, descriptor)
    };

    let out_stride = (out_w as usize) * out_desc.bytes_per_pixel();
    let out_slice = PixelSlice::new(&resized, out_w, out_h, out_stride, out_desc)
        .map_err(|e| format!("wrap resized: {e}"))?;

    // Verbatim: emit the source's colour exactly as it came in. The default
    // policy negotiates towards sRGB/BT.709, which for a Display-P3 PQ source
    // means a gamut+tone conversion — it fails outright without a peak
    // luminance, and where it succeeds it would silently make the "reference"
    // a different image from the thing it references.
    let mut req = EncodeRequest::new(ImageFormat::Png)
        .with_lossless(true)
        .with_color_emit_policy(ColorEmitPolicy::Verbatim);
    if let Some(c) = info_cicp {
        req = req.with_cicp(Some(c));
    }
    let png = req
        .encode(out_slice, has_alpha)
        .map_err(|e| format!("encode: {e}"))?;

    // Verify what was actually written. The encoder negotiates the pixel format
    // against the codec's supported-descriptor list, and if that list does not
    // contain this descriptor the pixels are CONVERTED on the way out -- while
    // `with_cicp` still stamps the colour we asked for. That combination is a
    // silently mislabelled file: pixels in one encoding, a chunk claiming
    // another. For a reference set that is the worst possible failure, so the
    // output is decoded back and compared sample-for-sample against the buffer
    // handed in. Lossless PNG must round-trip exactly; anything else is a bug.
    let check = DecodeRequest::new(png.data())
        .decode_full_frame()
        .map_err(|e| format!("verify decode: {e}"))?;
    let check_px = check.pixels();
    // The pixels are the test. A byte-identical lossless round-trip proves the
    // encoder did not convert anything, whatever the container ended up tagged
    // as. A descriptor difference alone does not: an untagged PNG legitimately
    // reads back as `transfer: Unknown` because PNG has no chunk saying "sRGB"
    // unless one is written, and that is the normal case for 8-bit sRGB.
    let wrote = check_px.as_strided_bytes();
    if wrote.len() != resized.len() || wrote != resized.as_slice() {
        let diff = wrote
            .iter()
            .zip(resized.iter())
            .filter(|(a, b)| a != b)
            .count();
        return Err(format!(
            "write-verify: {} of {} bytes differ after a lossless round-trip — the \
             codec converted the pixels (asked for {:?}, read back {:?})",
            diff.max(1),
            resized.len(),
            out_desc,
            check_px.descriptor()
        ));
    }
    // Pixels survived. Whether the COLOUR survived is a separate fact, and for
    // anything non-sRGB it is load-bearing: PQ pixels in a container that does
    // not say PQ are a mislabelled file even though every byte is intact.
    let readback = check_px.descriptor();
    let color_kept =
        readback.transfer == out_desc.transfer && readback.primaries == out_desc.primaries;
    // sRGB/BT.709 is conventionally untagged; a PNG with no colour chunk means
    // exactly that, so an Unknown readback there is not a loss.
    let conventional_srgb = readback.transfer == TransferFunction::Unknown
        && out_desc.transfer == TransferFunction::Srgb
        && out_desc.primaries == ColorPrimaries::Bt709;
    // Some transfers have no cICP code point to be written as — `Gamma22` is
    // the one this corpus hits (2 of 507 sources). PNG could carry it in `gAMA`
    // instead, but zencodecs does not derive that, so the file genuinely cannot
    // describe itself. That is a recorded limitation, not a silent one: the row
    // carries `color_untagged`, and it is only tolerated when the transfer is
    // inexpressible AND the primaries survived. Anything else -- a lost PQ or
    // Display-P3 tag -- still fails.
    let inexpressible =
        out_desc.transfer.to_cicp().is_none() && readback.primaries == out_desc.primaries;
    let color_untagged = !color_kept;
    if !color_kept && !conventional_srgb && !inexpressible {
        return Err(format!(
            "write-verify: pixels are intact but the colour tag was lost — wrote \
             {:?}/{:?}, reads back {:?}/{:?}. A reference file that does not \
             describe its own colour is mislabelled.",
            out_desc.transfer, out_desc.primaries, readback.transfer, readback.primaries
        ));
    }

    Ok(Rendered {
        png: png.into_vec(),
        transfer_assumed,
        color_untagged,
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
        "src\tout\tkernel\tratio\tsrc_w\tsrc_h\tcrop_w\tcrop_h\tout_w\tout_h\ttransfer\ttransfer_assumed\tcolor_untagged\tcicp\tout_bytes\n",
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
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:?}\t{}\t{}\t{}\t{}\n",
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
                    r.transfer_assumed,
                    r.color_untagged,
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
