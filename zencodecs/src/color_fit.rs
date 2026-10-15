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
//! Two descriptions can't be converted, and are dropped with the pixels
//! unchanged: a profile that parses to no colour space at all (no decoder
//! can apply it either), and, without the `cms` feature, an ICC profile that
//! needs a full colour-management engine.

use alloc::sync::Arc;

use zencodec::Metadata;
use zencodec::encode::EncodeCapabilities;
use zenpixels::{Cicp, ColorPrimaries, PixelDescriptor, PixelSlice, TransferFunction};

/// What an encode does to fit its colour description to the target.
pub(crate) enum ColorFit {
    /// Encode the pixels and metadata unchanged.
    Keep,
    /// Encode the pixels unchanged with this metadata.
    Metadata(Metadata),
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
fn convert(from: Source, metadata: Metadata) -> ColorFit {
    #[cfg(not(feature = "cms"))]
    if matches!(from, Source::Icc(_)) {
        return ColorFit::Metadata(metadata);
    }
    ColorFit::ConvertToSrgb { from, metadata }
}

/// Decide how to fit `metadata`'s colour description to a target with
/// `caps`.
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
        if zenpixels::icc::is_common_srgb(icc) {
            return ColorFit::Metadata(out);
        }
        if let Some(id) = zenpixels::icc::identify_common(icc) {
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
        return ColorFit::Metadata(out);
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

/// The descriptor for pixels in `from`: a CICP source states primaries and
/// transfer, which the descriptor must agree with; an ICC source keeps the
/// pixels' own descriptor (the profile is the authority).
fn source_descriptor(desc: PixelDescriptor, from: &Source) -> PixelDescriptor {
    match from {
        Source::Cicp(c) => desc
            .with_primaries(ColorPrimaries::from_cicp(c.color_primaries).unwrap_or(desc.primaries))
            .with_transfer(
                TransferFunction::from_cicp(c.transfer_characteristics).unwrap_or(desc.transfer()),
            ),
        Source::Icc(_) => desc,
    }
}

/// Convert `pixels` from `from` to sRGB in the same pixel format.
pub(crate) fn convert_to_srgb(
    pixels: &PixelSlice<'_>,
    from: &Source,
) -> Result<zenpixels::PixelBuffer, alloc::string::String> {
    convert_to(pixels, from, Cicp::SRGB, pixels.descriptor())
}

/// Convert `pixels` from `from` to the colour space `to`, in `format`'s
/// pixel format.
fn convert_to(
    pixels: &PixelSlice<'_>,
    from: &Source,
    to: Cicp,
    format: PixelDescriptor,
) -> Result<zenpixels::PixelBuffer, alloc::string::String> {
    use alloc::string::ToString;
    let desc = source_descriptor(pixels.descriptor(), from);
    let bytes = pixels.contiguous_bytes().into_owned();
    let (context, origin) = match from {
        Source::Icc(icc) => (
            zenpixels::ColorContext::from_icc(icc.clone()),
            zenpixels::ColorOrigin::from_icc(icc.clone()),
        ),
        Source::Cicp(c) => (
            zenpixels::ColorContext::from_cicp(*c),
            zenpixels::ColorOrigin::from_cicp(*c),
        ),
    };
    let buffer = zenpixels::PixelBuffer::from_vec(bytes, pixels.width(), pixels.rows(), desc)
        .map_err(|e| e.to_string())?
        .with_color_context(Arc::new(context));
    #[cfg(feature = "cms")]
    let cms: Option<&dyn zenpixels_convert::cms::PluggableCms> =
        Some(&zenpixels_convert::cms_moxcms::MoxCms);
    #[cfg(not(feature = "cms"))]
    let cms: Option<&dyn zenpixels_convert::cms::PluggableCms> = None;
    let ready = zenpixels_convert::finalize_for_output_with(
        &buffer,
        &origin,
        zenpixels_convert::OutputProfile::Named(to),
        format.pixel_format(),
        cms,
    )
    .map_err(|e| e.to_string())?;
    Ok(ready.into_parts().0)
}

/// When the encoder can't take `pixels` as they are and the format it can
/// take (the one `adapt_for_encode_cow` would pick) has other primaries or
/// another transfer, converting there changes what colour the samples mean.
/// A colour description in `metadata` would then describe the source, not
/// the written pixels. So convert from the described colour to the target's
/// colour space here and describe the result by its CICP (dropping an ICC
/// profile that no longer applies). `None`: nothing to do, the pixels and
/// metadata stand as they are.
pub(crate) fn fit_to_supported(
    pixels: &PixelSlice<'_>,
    metadata: Option<&Metadata>,
    supported: &[PixelDescriptor],
) -> Result<Option<(zenpixels::PixelBuffer, Metadata)>, alloc::string::String> {
    let Some(meta) = metadata else {
        return Ok(None);
    };
    let source = match (&meta.icc_profile, meta.cicp) {
        (Some(icc), _) => Source::Icc(icc.clone()),
        (None, Some(c)) => Source::Cicp(c),
        (None, None) => return Ok(None),
    };
    let desc = pixels.descriptor();
    // `adapt_for_encode_cow` passes these through without converting.
    if supported.contains(&desc) || desc.transfer() == TransferFunction::Unknown {
        return Ok(None);
    }
    let Some(target) =
        zenpixels_convert::best_match(desc, supported, zenpixels_convert::ConvertIntent::Fastest)
    else {
        return Ok(None);
    };
    if target.primaries == desc.primaries && target.transfer() == desc.transfer() {
        return Ok(None);
    }
    let (Some(p), Some(t)) = (target.primaries.to_cicp(), target.transfer().to_cicp()) else {
        return Ok(None);
    };
    let target_cicp = Cicp::new(p, t, 0, true);
    // Without a CMS an ICC source can't be converted; it is dropped later
    // with the pixels unchanged, as in `plan`.
    #[cfg(not(feature = "cms"))]
    if matches!(source, Source::Icc(_)) {
        return Ok(None);
    }
    let converted = convert_to(pixels, &source, target_cicp, target)?;
    let mut out = meta.clone();
    out.icc_profile = None;
    out.cicp = Some(target_cicp);
    Ok(Some((converted, out)))
}
