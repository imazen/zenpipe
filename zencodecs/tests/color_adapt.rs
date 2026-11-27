//! When the encoder can't take the caller's pixel format, the encode converts
//! the pixels to one it can (`adapt_for_encode_cow`). If that conversion
//! changes the transfer or primaries, the colour description the caller
//! passed describes the old pixels, not the written ones. The file must
//! describe what it holds: decoding it by its own description has to give
//! the colours the source described.
//!
//! Lossless targets only, so a difference is the description, not the codec.

#![cfg(all(feature = "png", feature = "cms"))]

use std::sync::Arc;

use zencodec::Metadata;
use zencodecs::{AllowedFormats, DecodeRequest, EncodeRequest, ImageFormat};
use zenpixels::{
    Cicp, ColorOrigin, ColorSpace, OriginKind, PeakSource, PixelBuf, PixelFormat, PixelView,
    Primaries, RenderingIntent, SampleType, Transfer,
};
use zenpixels_convert::{ConvertPolicy, OutputPlan, OutputSignaling};

const W: u32 = 6;
const H: u32 = 3;

/// The colours `buf` (with provenance `origin`) show as, in 8-bit sRGB. Converts
/// at the buffer's own sample type (CMS steps don't change depth), then
/// rounds to 8 bits.
fn as_srgb8(buf: PixelBuf, origin: ColorOrigin) -> Vec<u8> {
    let ty = buf.format().layout.sample;
    let dst = match ty {
        SampleType::U8 => PixelFormat::RGB8_SRGB,
        SampleType::U16 => PixelFormat::RGB16_SRGB,
        _ => PixelFormat::RGBF32_LINEAR.with_transfer(Transfer::Srgb),
    };
    let policy = ConvertPolicy::permissive().with_cms(Arc::new(zenpixels_convert::cms::MoxCms));
    let plan = OutputPlan::build(
        buf.format(),
        &dst,
        &policy,
        PeakSource::Declared,
        OutputSignaling::Strip,
    )
    .expect("plan to sRGB");
    let ready = plan.execute_view(buf.as_view(), &origin).expect("to sRGB");
    let s = ready.pixels.as_view();
    let mut v = Vec::new();
    for y in 0..s.height() {
        let row = s.row(y);
        for x in 0..(s.width() * 3) as usize {
            v.push(match ty {
                SampleType::U8 => row[x],
                SampleType::U16 => {
                    let u = u16::from_ne_bytes([row[2 * x], row[2 * x + 1]]);
                    ((u32::from(u) * 255 + 32767) / 65535) as u8
                }
                _ => {
                    let f = f32::from_ne_bytes(row[4 * x..4 * x + 4].try_into().unwrap());
                    (f.clamp(0.0, 1.0) * 255.0).round() as u8
                }
            });
        }
    }
    v
}

/// What the decoded file says its pixels are: the format/origin the
/// decoder attached to the buffer (for RGB output it describes the RGB
/// samples, matrix 0, not the coded YUV matrix `source_color` reports), else
/// `source_color`. A file with no colour description is read by convention:
/// float samples (PFM) are linear BT.709, integer samples sRGB.
fn declared_and_labelled(d: zencodecs::DecodeOutput) -> (PixelBuf, ColorOrigin) {
    let source_color = d.info().source_color.clone();
    let buf_origin = d.pixels().origin().cloned();
    let buf = d.into_buffer();
    let fmt = buf.format().clone();
    let alpha = fmt.color.alpha();

    // If the decoder attached a concrete ICC ColorSpace or non-assumed origin, keep it.
    if matches!(fmt.color, ColorSpace::Icc { .. }) {
        let origin = buf_origin.unwrap_or_else(|| source_color.authoritative_origin());
        return (buf, origin);
    }

    if let Some(icc) = &source_color.icc_profile
        && !matches!(buf_origin.as_ref().map(|o| o.kind), Some(OriginKind::Cicp))
    {
        let icc_cs = ColorSpace::Icc {
            profile: icc.clone(),
            color_channels: (fmt.layout.order.channels()
                - usize::from(
                    fmt.layout.order.has_alpha_slot() || fmt.layout.order.has_padding_slot(),
                )) as u8,
            alpha,
            intent: RenderingIntent::Perceptual,
        };
        let relabelled = buf.relabel_color(icc_cs).unwrap();
        return (relabelled, ColorOrigin::from_icc(icc.clone()));
    }

    if let Some(cicp) = source_color.cicp {
        let rgb_cicp = Cicp::new(
            cicp.color_primaries,
            cicp.transfer_characteristics,
            0,
            cicp.full_range,
        );
        let buf = if let Ok(cs) = rgb_cicp.to_color_space(alpha) {
            if matches!(
                fmt.color,
                ColorSpace::Rgb {
                    transfer: Transfer::Unknown,
                    ..
                } | ColorSpace::Gray {
                    transfer: Transfer::Unknown,
                    ..
                }
            ) {
                buf.relabel_color(cs).unwrap()
            } else {
                buf
            }
        } else {
            buf
        };
        return (buf, ColorOrigin::from_cicp(rgb_cicp));
    }

    if fmt.layout.sample == SampleType::F32 {
        (buf, ColorOrigin::from_cicp(Cicp::LINEAR_SRGB))
    } else {
        (buf, ColorOrigin::from_cicp(Cicp::SRGB))
    }
}

struct Case {
    name: &'static str,
    bytes: Vec<u8>,
    desc: PixelFormat,
    meta: Metadata,
    origin: ColorOrigin,
}

fn linear_f32() -> Case {
    let v: Vec<f32> = (0..W * H * 3)
        .map(|i| [0.02, 0.2, 0.5, 0.9][i as usize % 4])
        .collect();
    let linear = Cicp::new(1, 8, 0, true);
    Case {
        name: "linear f32 + linear CICP",
        bytes: v.iter().flat_map(|f| f.to_ne_bytes()).collect(),
        desc: PixelFormat::RGBF32_LINEAR,
        meta: Metadata::none().with_cicp(linear),
        origin: ColorOrigin::from_cicp(linear),
    }
}

fn display_p3_u8() -> Case {
    let v: Vec<u8> = (0..W * H)
        .flat_map(|i| [[250u8, 10, 10], [10, 220, 40], [30, 40, 230]][i as usize % 3])
        .collect();
    Case {
        name: "Display P3 RGB8 + P3 CICP",
        bytes: v,
        desc: PixelFormat::RGB8_SRGB.with_primaries(Primaries::DisplayP3),
        meta: Metadata::none().with_cicp(Cicp::DISPLAY_P3),
        origin: ColorOrigin::from_cicp(Cicp::DISPLAY_P3),
    }
}

fn linear_u16() -> Case {
    let v: Vec<u16> = (0..W * H * 3)
        .map(|i| [300u16, 9000, 30000, 60000][i as usize % 4])
        .collect();
    let linear = Cicp::new(1, 8, 0, true);
    Case {
        name: "linear u16 + linear CICP",
        bytes: v.iter().flat_map(|s| s.to_ne_bytes()).collect(),
        desc: PixelFormat::RGB16_SRGB.with_transfer(Transfer::Linear),
        meta: Metadata::none().with_cicp(linear),
        origin: ColorOrigin::from_cicp(linear),
    }
}

/// Baseline: sRGB 8-bit needs no conversion anywhere; a failure here is
/// the codec not being lossless, not the description.
fn srgb_u8() -> Case {
    let v: Vec<u8> = (0..W * H * 3).map(|i| (i * 13) as u8).collect();
    Case {
        name: "sRGB RGB8 + sRGB CICP",
        bytes: v,
        desc: PixelFormat::RGB8_SRGB,
        meta: Metadata::none().with_cicp(Cicp::SRGB),
        origin: ColorOrigin::from_cicp(Cicp::SRGB),
    }
}

fn lossless(f: ImageFormat) -> Option<EncodeRequest<'static>> {
    let req = EncodeRequest::new(f);
    match f {
        ImageFormat::Png
        | ImageFormat::Bmp
        | ImageFormat::Pnm
        | ImageFormat::Farbfeld
        | ImageFormat::Qoi
        | ImageFormat::Tga => Some(req),
        ImageFormat::WebP | ImageFormat::Jxl | ImageFormat::Avif => Some(req.with_lossless(true)),
        _ => None,
    }
}

#[test]
fn a_converted_encode_describes_the_written_pixels() {
    let formats = AllowedFormats::all();
    let mut bad = Vec::new();
    let mut checked = 0;
    for case in [srgb_u8(), linear_f32(), display_p3_u8(), linear_u16()] {
        let src = PixelBuf::from_bytes(case.bytes.clone(), W, H, case.desc.clone()).unwrap();
        let want = as_srgb8(src, case.origin.clone());
        for f in formats.encodable_formats() {
            let Some(req) = lossless(f) else { continue };
            if !formats.can_decode(f) {
                continue;
            }
            let bpp = case.desc.layout.bytes_per_pixel();
            let slice = PixelView::new(&case.bytes, W, H, W as usize * bpp, &case.desc).unwrap();
            let bytes = match req.with_metadata(case.meta.clone()).encode(slice, false) {
                Ok(o) => o.into_vec(),
                Err(e) => {
                    bad.push(format!("{} -> {f:?}: encode failed: {e}", case.name));
                    continue;
                }
            };
            let d = DecodeRequest::new(&bytes).decode_full_frame().unwrap();
            let label = format!(
                "{} -> {f:?} (decoded {:?}, declares {:?})",
                case.name,
                d.pixel_format(),
                d.info().source_color.cicp
            );
            let (buf, origin) = declared_and_labelled(d);
            let got = as_srgb8(buf, origin);
            checked += 1;
            let worst = want
                .iter()
                .zip(&got)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            if worst > 1 {
                bad.push(format!(
                    "{label}: off by up to {worst}: want {want:?} got {got:?}"
                ));
            }
        }
    }
    assert!(checked > 0);
    assert!(
        bad.is_empty(),
        "{} of {checked} files misdescribe their pixels:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
