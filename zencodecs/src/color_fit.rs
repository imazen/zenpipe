//! Fitting an image's colour description to what the target format carries.
//!
//! The description an encode receives (an ICC profile or CICP code points in
//! [`Metadata`]) says what colour space the pixels are in. A format with no
//! carrier for it (GIF, PNM, BMP and the other raw formats carry neither) can
//! only be read as sRGB, so writing Display P3 pixels there unchanged would
//! show them with the wrong colours. Before the codec runs, the encode paths
//! fit the description to the target:
//!
//! - the target carries the description (an ICC carrier for an ICC profile,
//!   any carrier for CICP): nothing changes;
//! - the description is sRGB already: it is dropped where it can't be
//!   carried, and the pixels are written unchanged;
//! - an ICC profile that names a CICP colour space the target can carry as
//!   CICP is replaced by the CICP, pixels unchanged;
//! - otherwise the pixels are converted to the closest colour the target
//!   describes, sRGB, and the description is dropped.
//!
//! Strict color-fit admission belongs to this pipeline stage. Once admitted,
//! one-shot and streaming dispatch clear only `EncodePolicy::strict` before
//! native setup; color-carrier and metadata-emission requirements still go
//! through the native checked setter. A retained native job that bypasses this
//! fitting stage receives the complete policy unchanged.
//!
//! Two descriptions can't be converted, and are dropped with the pixels
//! unchanged: a profile that parses to no colour space at all (no decoder
//! can apply it either), and, without the `cms` feature, an ICC profile that
//! needs a full colour-management engine.

use alloc::sync::Arc;

use zencodec::Metadata;
#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
use zencodec::encode::EncodeCapabilities;
use zenpixels::{
    Cicp, ColorOrigin, ColorSpace, PeakSource, PixelBuf, PixelFormat, PixelView, Primaries,
    RenderingIntent, SignalRange, Transfer,
};

/// What an encode does to fit its colour description to the target.
#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
pub(crate) enum ColorFit {
    /// Encode the pixels and metadata unchanged.
    Keep,
    /// Encode the pixels unchanged with this metadata; nothing about the
    /// colour is lost (a redundant sRGB description dropped, or an ICC
    /// replaced by the CICP it names).
    Metadata(Metadata),
    /// Encode the pixels unchanged with this metadata, which drops a
    /// description the output cannot reproduce: a profile that describes no
    /// colour space, or (without `cms`) one only a CMS could convert.
    Dropped(Metadata),
    /// Convert the pixels from `from` to sRGB and encode with `metadata`.
    ConvertToSrgb { from: Source, metadata: Metadata },
}

/// The colour space the pixels are in, as the description gives it.
#[derive(Clone)]
pub(crate) enum Source {
    Icc(Arc<[u8]>),
    Cicp(Cicp),
}

fn is_srgb_cicp(c: Cicp) -> bool {
    c.color_primaries == 1 && c.transfer_characteristics == 13 && c.matrix_coefficients == 0
}

/// Convert to sRGB from `from`; without the `cms` feature an ICC source that
/// needs a colour-management engine is dropped instead (pixels unchanged).
#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
fn convert(from: Source, metadata: Metadata) -> ColorFit {
    #[cfg(not(feature = "cms"))]
    if matches!(from, Source::Icc(_)) {
        return ColorFit::Dropped(metadata);
    }
    ColorFit::ConvertToSrgb { from, metadata }
}

/// Decide how to fit `metadata`'s colour description to a target with
/// `caps`.
#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
pub(crate) fn plan(metadata: Option<&Metadata>, caps: &EncodeCapabilities) -> ColorFit {
    let Some(meta) = metadata else {
        return ColorFit::Keep;
    };
    let (icc_ok, cicp_ok) = (caps.icc(), caps.cicp());

    // An ICC profile, when present, is the authority for the pixels.
    if let Some(icc) = &meta.icc_profile {
        if icc_ok {
            return ColorFit::Keep;
        }
        let mut out = meta.clone();
        out.icc_profile = None;
        if !cicp_ok {
            out.cicp = None;
        }
        if zenpixels::icc::IccIdentification::is_common_srgb(icc) {
            return ColorFit::Metadata(out);
        }
        if let Some(id) = zenpixels::icc::IccIdentification::from_icc(icc) {
            if id.is_srgb() {
                return ColorFit::Metadata(out);
            }
            if let Some(cicp) = id.to_cicp() {
                if cicp_ok {
                    out.cicp = Some(cicp);
                    return ColorFit::Metadata(out);
                }
                let from =
                    if id.valid_use == zenpixels::icc::IdentificationUse::MatrixTrcSubstitution {
                        Source::Cicp(cicp)
                    } else {
                        Source::Icc(icc.clone())
                    };
                return convert(from, out);
            }
        }
        if zenpixels::icc::profile_color_space(icc).is_some() {
            return convert(Source::Icc(icc.clone()), out);
        }
        // The bytes describe no colour space.
        return ColorFit::Dropped(out);
    }

    // No ICC: CICP alone describes the pixels. An ICC carrier gets a profile
    // synthesised from it by the colour-emit resolver.
    if let Some(cicp) = meta.cicp
        && !cicp_ok
        && !icc_ok
    {
        let mut out = meta.clone();
        out.cicp = None;
        if is_srgb_cicp(cicp) {
            return ColorFit::Metadata(out);
        }
        return convert(Source::Cicp(cicp), out);
    }
    ColorFit::Keep
}

#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
impl ColorFit {
    /// Under [`EncodePolicy::strict`](zencodec::encode::EncodePolicy::strict),
    /// refuse a fit that loses colour: a conversion or a dropped description.
    pub(crate) fn check_strict(self, strict: bool) -> Result<Self, &'static str> {
        match self {
            ColorFit::ConvertToSrgb { .. } if strict => Err(
                "the target format cannot carry the source colour description \
                 (strict: refusing to convert the pixels to sRGB)",
            ),
            ColorFit::Dropped(_) if strict => Err(
                "the target format cannot carry the source colour description \
                 (strict: refusing to drop a profile that cannot be reproduced)",
            ),
            fit => Ok(fit),
        }
    }
}

fn primaries_and_transfer(fmt: &PixelFormat) -> Option<(Primaries, Transfer)> {
    match &fmt.color {
        ColorSpace::Rgb {
            primaries,
            transfer,
            ..
        } => Some((*primaries, *transfer)),
        ColorSpace::Gray { transfer, .. } => Some((Primaries::Bt709, *transfer)),
        _ => None,
    }
}

/// The format for pixels in `from`: a CICP source states primaries and
/// transfer, which the format must agree with; an ICC source attaches the
/// profile as the authoritative color space.
fn source_format(fmt: &PixelFormat, from: &Source) -> PixelFormat {
    match from {
        Source::Cicp(c) => {
            let alpha = fmt.color.alpha();
            let range = if c.full_range {
                SignalRange::Full
            } else {
                SignalRange::Narrow
            };
            let transfer = Transfer::from_cicp(c.transfer_characteristics)
                .unwrap_or_else(|| primaries_and_transfer(fmt).map_or(Transfer::Srgb, |(_, t)| t));
            if fmt.layout.order.is_gray_family() {
                PixelFormat::new(
                    fmt.layout,
                    ColorSpace::Gray {
                        transfer,
                        range,
                        alpha,
                        white: fmt.color.white(),
                    },
                )
            } else {
                let primaries = Primaries::from_cicp(c.color_primaries).unwrap_or_else(|| {
                    primaries_and_transfer(fmt).map_or(Primaries::Bt709, |(p, _)| p)
                });
                PixelFormat::new(
                    fmt.layout,
                    ColorSpace::Rgb {
                        primaries,
                        transfer,
                        range,
                        alpha,
                        white: fmt.color.white(),
                    },
                )
            }
        }
        Source::Icc(icc) => {
            let alpha = fmt.color.alpha();
            if fmt.layout.order.is_cmyk_family() {
                let inverted = matches!(fmt.color, ColorSpace::Cmyk { inverted: true, .. });
                PixelFormat::new(
                    fmt.layout,
                    ColorSpace::Cmyk {
                        inverted,
                        alpha,
                        icc: Some(icc.clone()),
                    },
                )
            } else {
                let color_channels = if fmt.layout.order.is_gray_family() {
                    1
                } else {
                    3
                };
                PixelFormat::new(
                    fmt.layout,
                    ColorSpace::Icc {
                        profile: icc.clone(),
                        color_channels,
                        alpha,
                        intent: RenderingIntent::RelativeColorimetric,
                    },
                )
            }
        }
    }
}

fn target_format_for_cicp(base: &PixelFormat, to: Cicp) -> PixelFormat {
    let alpha = base.color.alpha();
    let range = if to.full_range {
        SignalRange::Full
    } else {
        SignalRange::Narrow
    };
    let transfer = Transfer::from_cicp(to.transfer_characteristics).unwrap_or(Transfer::Srgb);
    if base.layout.order.is_gray_family() {
        PixelFormat::new(
            base.layout,
            ColorSpace::Gray {
                transfer,
                range,
                alpha,
                white: None,
            },
        )
    } else {
        let primaries = Primaries::from_cicp(to.color_primaries).unwrap_or(Primaries::Bt709);
        PixelFormat::new(
            base.layout,
            ColorSpace::Rgb {
                primaries,
                transfer,
                range,
                alpha,
                white: None,
            },
        )
    }
}

/// Convert `pixels` from `from` to sRGB in the same pixel layout.
#[cfg(any(
    feature = "jpeg",
    feature = "webp",
    feature = "gif",
    feature = "png",
    feature = "avif-encode",
    feature = "jxl-encode",
    feature = "bitmaps",
    feature = "tiff",
))]
pub(crate) fn convert_to_srgb(
    pixels: &PixelView<'_>,
    from: &Source,
) -> Result<PixelBuf, alloc::string::String> {
    convert_to(pixels, from, Cicp::SRGB, pixels.format())
}

/// Convert `pixels` from `from` to the colour space `to`, in `format`'s
/// pixel layout.
fn convert_to(
    pixels: &PixelView<'_>,
    from: &Source,
    to: Cicp,
    format: &PixelFormat,
) -> Result<PixelBuf, alloc::string::String> {
    use alloc::string::ToString;
    let src_fmt = source_format(pixels.format(), from);
    let dst_fmt = target_format_for_cicp(format, to);
    let origin = match from {
        Source::Icc(icc) => ColorOrigin::from_icc(icc.clone()),
        Source::Cicp(c) => ColorOrigin::from_cicp(*c),
    };
    let src_view = PixelView::new(
        pixels.as_strided_bytes(),
        pixels.width(),
        pixels.height(),
        pixels.stride_bytes(),
        &src_fmt,
    )
    .map_err(|e| e.to_string())?
    .with_origin(&origin)
    .with_hdr(pixels.hdr());
    #[allow(unused_mut)]
    let mut policy = zenpixels_convert::ConvertPolicy::permissive();
    #[cfg(feature = "cms")]
    {
        policy = policy.with_cms(Arc::new(zenpixels_convert::cms::MoxCms));
    }
    let plan = zenpixels_convert::OutputPlan::build(
        &src_fmt,
        &dst_fmt,
        &policy,
        PeakSource::Declared,
        zenpixels_convert::OutputSignaling::Strip,
    )
    .map_err(|e| e.to_string())?;
    let ready = plan
        .execute_view(src_view, &origin)
        .map_err(|e| e.to_string())?;
    ready.pixels.into_owned().map_err(|e| e.to_string())
}

/// When the encoder can't take `pixels` as they are and the format it can
/// take (the one `adapt_for_encode` would pick) has other primaries or
/// another transfer, converting there changes what colour the samples mean.
/// A colour description in `metadata` would then describe the source, not
/// the written pixels. So convert from the described colour to the target's
/// colour space here and describe the result by its CICP (dropping an ICC
/// profile that no longer applies). `None`: nothing to do, the pixels and
/// metadata stand as they are.
pub(crate) fn fit_to_supported(
    pixels: &PixelView<'_>,
    metadata: Option<&Metadata>,
    supported: &[PixelFormat],
) -> Result<Option<(PixelBuf, Metadata)>, alloc::string::String> {
    let Some(meta) = metadata else {
        return Ok(None);
    };
    let source = match (&meta.icc_profile, meta.cicp) {
        (Some(icc), _) => Source::Icc(icc.clone()),
        (None, Some(c)) => Source::Cicp(c),
        (None, None) => return Ok(None),
    };
    let desc = pixels.format();
    // `adapt_for_encode` passes these through without converting.
    if supported.contains(desc)
        || primaries_and_transfer(desc).is_some_and(|(_, t)| t == Transfer::Unknown)
    {
        return Ok(None);
    }
    #[allow(unused_mut)]
    let mut policy = zenpixels_convert::ConvertPolicy::permissive();
    #[cfg(feature = "cms")]
    {
        policy = policy.with_cms(Arc::new(zenpixels_convert::cms::MoxCms));
    }
    let Some(negotiated) = zenpixels_convert::Negotiate::select(desc, supported, &policy) else {
        return Ok(None);
    };
    let target = negotiated.format;
    if primaries_and_transfer(&target) == primaries_and_transfer(desc) {
        return Ok(None);
    }
    let Some((target_p, target_t)) = primaries_and_transfer(&target) else {
        return Ok(None);
    };
    let (Some(p), Some(t)) = (target_p.to_cicp(), target_t.to_cicp()) else {
        return Ok(None);
    };
    let target_cicp = Cicp::new(p, t, 0, true);
    // Without a CMS an ICC source can't be converted; it is dropped later
    // with the pixels unchanged, as in `plan`.
    #[cfg(not(feature = "cms"))]
    if matches!(source, Source::Icc(_)) {
        return Ok(None);
    }
    let converted = convert_to(pixels, &source, target_cicp, &target)?;
    let mut out = meta.clone();
    out.icc_profile = None;
    out.cicp = Some(target_cicp);
    Ok(Some((converted, out)))
}

/// Keep an EXIF ColorSpace tag in step with the colour being written: `1`
/// only for sRGB, `0xFFFF` otherwise. Readers that consult it treat `1` as
/// sRGB whatever profile is embedded (imazen/zencodec#132). A blob without
/// the tag is left alone.
pub(crate) fn exif_color_space(mut meta: Metadata) -> Metadata {
    let Some(exif) = meta.exif.as_deref() else {
        return meta;
    };
    let srgb = meta.icc_profile.as_deref().is_none_or(|icc| {
        zenpixels::icc::IccIdentification::is_common_srgb(icc)
            || zenpixels::icc::IccIdentification::from_icc(icc).is_some_and(|id| id.is_srgb())
    }) && meta.cicp.is_none_or(is_srgb_cicp);
    if let Some(fixed) = zencodec::helpers::set_exif_color_space(exif, srgb) {
        meta.exif = Some(fixed.into());
    }
    meta
}
