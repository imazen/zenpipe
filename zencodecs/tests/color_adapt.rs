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
    Cicp, ColorContext, ColorOrigin, ColorPrimaries, PixelBuffer, PixelDescriptor, PixelFormat,
    PixelSlice, TransferFunction,
};
use zenpixels_convert::{OutputProfile, finalize_for_output_with};

const W: u32 = 6;
const H: u32 = 3;

/// The colours `buf` (described by `ctx`) show as, in 8-bit sRGB. Converts
/// at the buffer's own sample type (CMS steps don't change depth), then
/// rounds to 8 bits.
fn as_srgb8(buf: PixelBuffer, ctx: Option<ColorContext>) -> Vec<u8> {
    use zenpixels::ChannelType;
    let ty = buf.descriptor().channel_type();
    let format = match ty {
        ChannelType::U8 => PixelFormat::Rgb8,
        ChannelType::U16 => PixelFormat::Rgb16,
        _ => PixelFormat::RgbF32,
    };
    let buf = match ctx {
        Some(c) => buf.with_color_context(Arc::new(c)),
        None => buf,
    };
    let origin = ColorOrigin::from_cicp(Cicp::SRGB);
    let ready = finalize_for_output_with(
        &buf,
        &origin,
        OutputProfile::Named(Cicp::SRGB),
        format,
        Some(&zenpixels_convert::cms_moxcms::MoxCms),
    )
    .expect("to sRGB");
    let out = ready.into_parts().0;
    let s = out.as_slice();
    let mut v = Vec::new();
    for y in 0..s.rows() {
        let row = s.row(y);
        for x in 0..(s.width() * 3) as usize {
            v.push(match ty {
                ChannelType::U8 => row[x],
                ChannelType::U16 => {
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

/// What the decoded file says its pixels are: the colour context the
/// decoder attached to the buffer (for RGB output it describes the RGB
/// samples, matrix 0, not the coded YUV matrix `source_color` reports), else
/// `source_color`. A file with no colour description is read by convention:
/// float samples (PFM) are linear BT.709, integer samples sRGB.
fn declared(d: &zencodecs::DecodeOutput) -> Option<ColorContext> {
    if let Some(ctx) = d.pixels().color_context() {
        return Some(ColorContext::clone(ctx));
    }
    let c = &d.info().source_color;
    if let Some(icc) = &c.icc_profile {
        Some(ColorContext::from_icc(icc.clone()))
    } else if let Some(cicp) = c.cicp {
        Some(ColorContext::from_cicp(cicp))
    } else if d.descriptor().channel_type() == zenpixels::ChannelType::F32 {
        Some(ColorContext::from_cicp(Cicp::new(1, 8, 0, true)))
    } else {
        Some(ColorContext::from_cicp(Cicp::SRGB))
    }
}

/// The decoded buffer labelled with the transfer its description declares
/// (the conversion trusts the descriptor's transfer over the context).
fn labelled(d: zencodecs::DecodeOutput, ctx: &Option<ColorContext>) -> PixelBuffer {
    let tf = ctx
        .as_ref()
        .and_then(|c| c.cicp)
        .and_then(|c| TransferFunction::from_cicp(c.transfer_characteristics));
    let buf = d.into_buffer();
    match tf {
        Some(tf) if buf.descriptor().transfer() == TransferFunction::Unknown => {
            buf.with_transfer(tf)
        }
        _ => buf,
    }
}

struct Case {
    name: &'static str,
    bytes: Vec<u8>,
    desc: PixelDescriptor,
    meta: Metadata,
    ctx: ColorContext,
}

fn linear_f32() -> Case {
    let v: Vec<f32> = (0..W * H * 3)
        .map(|i| [0.02, 0.2, 0.5, 0.9][i as usize % 4])
        .collect();
    let linear = Cicp::new(1, 8, 0, true);
    Case {
        name: "linear f32 + linear CICP",
        bytes: v.iter().flat_map(|f| f.to_ne_bytes()).collect(),
        desc: PixelDescriptor::RGBF32_LINEAR,
        meta: Metadata::none().with_cicp(linear),
        ctx: ColorContext::from_cicp(linear),
    }
}

fn display_p3_u8() -> Case {
    let v: Vec<u8> = (0..W * H)
        .flat_map(|i| [[250u8, 10, 10], [10, 220, 40], [30, 40, 230]][i as usize % 3])
        .collect();
    Case {
        name: "Display P3 RGB8 + P3 CICP",
        bytes: v,
        desc: PixelDescriptor::RGB8_SRGB.with_primaries(ColorPrimaries::DisplayP3),
        meta: Metadata::none().with_cicp(Cicp::DISPLAY_P3),
        ctx: ColorContext::from_cicp(Cicp::DISPLAY_P3),
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
        desc: PixelDescriptor::RGB16_SRGB.with_transfer(TransferFunction::Linear),
        meta: Metadata::none().with_cicp(linear),
        ctx: ColorContext::from_cicp(linear),
    }
}

/// Baseline: sRGB 8-bit needs no conversion anywhere; a failure here is
/// the codec not being lossless, not the description.
fn srgb_u8() -> Case {
    let v: Vec<u8> = (0..W * H * 3).map(|i| (i * 13) as u8).collect();
    Case {
        name: "sRGB RGB8 + sRGB CICP",
        bytes: v,
        desc: PixelDescriptor::RGB8_SRGB,
        meta: Metadata::none().with_cicp(Cicp::SRGB),
        ctx: ColorContext::from_cicp(Cicp::SRGB),
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
        let src = PixelBuffer::from_vec(case.bytes.clone(), W, H, case.desc).unwrap();
        let want = as_srgb8(src, Some(case.ctx.clone()));
        for f in formats.encodable_formats() {
            let Some(req) = lossless(f) else { continue };
            if !formats.can_decode(f) {
                continue;
            }
            let bpp = case.desc.bytes_per_pixel();
            let slice = PixelSlice::new(&case.bytes, W, H, W as usize * bpp, case.desc).unwrap();
            let bytes = match req.with_metadata(case.meta.clone()).encode(slice, false) {
                Ok(o) => o.into_vec(),
                Err(e) => {
                    bad.push(format!("{} -> {f:?}: encode failed: {e}", case.name));
                    continue;
                }
            };
            let d = DecodeRequest::new(&bytes).decode_full_frame().unwrap();
            let ctx = declared(&d);
            let label = format!(
                "{} -> {f:?} (decoded {:?}, declares {:?})",
                case.name,
                d.descriptor(),
                d.info().source_color.cicp
            );
            let got = as_srgb8(labelled(d, &ctx), ctx);
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
