//! HDR → SDR tone mapping for targets that carry neither HDR nor CICP
//! (feature `tonemap`).
//!
//! [`crate::color_fit`] converts pixels to sRGB when a target can't carry
//! their colour, but a PQ or HLG source can't be converted that way: the
//! samples go far past SDR white, and `zenpixels-convert` refuses the
//! conversion without a peak (`HdrSourceRequiresPeak`). Writing the HDR code
//! values as they are would show them as sRGB, much too dark and flat. This
//! module tone-maps them instead.
//!
//! # When it runs
//!
//! The request explicitly selects [`ToneMapPolicy::Bt2408`]. The source colour is PQ or HLG, read from the metadata's CICP, from an
//! ICC profile that names such a CICP, or (with no colour description at
//! all) from the pixel descriptor's transfer. The target declares neither
//! `EncodeCapabilities::hdr()` nor `cicp()`. That covers JPEG, WebP, GIF,
//! BMP and TIFF; PNG (cICP), AVIF, JPEG XL, HEIC and PNM (which writes
//! float PFM) keep the HDR pixels and their description.
//!
//! # Operator
//!
//! ITU-R BT.2408 EETF ([`zentone::Bt2408Tonemapper`], YRGB space with the
//! source primaries' luma), mapping the content peak onto a 203-nit display
//! peak: BT.2408 HDR reference white is SDR white (sRGB 255). Then the
//! source primaries convert to BT.709 with a hue-preserving soft clip, and
//! the result is sRGB-encoded 8-bit.
//!
//! PQ samples decode to absolute luminance (1.0 = 10 000 nits) and are scaled
//! so that 1.0 is the content peak, the unit the EETF works in. HLG is
//! scene-referred: it goes through the BT.2100 OOTF for the 1000-nit
//! reference display, which then is its content peak.
//!
//! # Content peak (PQ)
//!
//! The source's MaxCLL, else the mastering display's maximum luminance, else
//! 1000 nits, identically for whole images and every streaming strip. Pixel
//! measurements never alter rendering. Peaks are clamped to 203–10,000 nits.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use zencodec::Metadata;
use zencodec::encode::EncodeCapabilities;
use zenpixels::{
    AlphaMode, ChannelLayout, ChannelType, Cicp, ColorPrimaries, PixelBuffer, PixelDescriptor,
    PixelSlice, SignalRange, TransferFunction,
};
use zentone::ToneMap as _;

/// Per-request permission to render HDR pixels for an SDR-only target.
/// Compiling the `tonemap` feature only makes this capability available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToneMapPolicy {
    /// Keep the existing color fitting contract, including its errors.
    #[default]
    Preserve,
    /// BT.2408 EETF onto 203-nit SDR white, followed by BT.709 gamut
    /// conversion, soft clipping and sRGB 8-bit encoding. PQ peak is MaxCLL,
    /// else mastering-display maximum, else 1000 nits; both encode paths use
    /// the same fallback. HLG uses the 1000-nit BT.2100 reference display.
    /// Strict encode policy still refuses HDR rendering.
    Bt2408,
}

// Inputs accepted by the tone-map adapter, before the codec's own format
// negotiation. Advertising only native SDR formats would make TranscodeSink
// try HDR-to-SDR conversion before this adapter can apply the request policy.
pub(crate) const STREAMING_INPUTS: &[PixelDescriptor] = &streaming_inputs();

const fn streaming_inputs() -> [PixelDescriptor; 50] {
    let mut out = [PixelDescriptor::RGB8_SRGB; 50];
    out[1] = PixelDescriptor::RGBA8_SRGB;
    let formats = [
        PixelDescriptor::RGBF32_LINEAR,
        PixelDescriptor::RGBAF32_LINEAR,
        PixelDescriptor::RGB16_SRGB,
        PixelDescriptor::RGBA16_SRGB,
    ];
    let transfers = [
        TransferFunction::Pq,
        TransferFunction::Hlg,
        TransferFunction::Linear,
        TransferFunction::Srgb,
    ];
    let primaries = [
        ColorPrimaries::Bt709,
        ColorPrimaries::DisplayP3,
        ColorPrimaries::Bt2020,
    ];
    let mut i = 2;
    let mut f = 0;
    while f < formats.len() {
        let mut t = 0;
        while t < transfers.len() {
            let mut p = 0;
            while p < primaries.len() {
                out[i] = formats[f]
                    .with_transfer(transfers[t])
                    .with_primaries(primaries[p]);
                i += 1;
                p += 1;
            }
            t += 1;
        }
        f += 1;
    }
    out
}

/// SDR white in nits: BT.2408 HDR reference white, mapped to sRGB 255.
pub(crate) const SDR_WHITE_NITS: f32 = 203.0;
/// PQ content peak when nothing better is known: the HDR10 baseline.
pub(crate) const DEFAULT_PQ_PEAK_NITS: f32 = 1000.0;
/// HLG reference display peak (BT.2100), the peak after the OOTF.
pub(crate) const HLG_REFERENCE_PEAK_NITS: f32 = 1000.0;

/// Display P3 luma coefficients (D65), for the EETF's YRGB signal.
const LUMA_P3: [f32; 3] = [0.228_975, 0.691_739, 0.079_287];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HdrTransfer {
    Pq,
    Hlg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HdrPrimaries {
    Bt709,
    DisplayP3,
    Bt2020,
}

/// The colour an HDR source is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HdrSource {
    pub(crate) transfer: HdrTransfer,
    pub(crate) primaries: HdrPrimaries,
}

impl HdrSource {
    /// `Some` for a PQ (16) or HLG (18) transfer. Primaries other than
    /// BT.709, Display P3 or BT.2020 are kept as an error for the conversion
    /// to report, so the source is never silently treated as SDR.
    fn from_cicp(c: Cicp) -> Option<Result<Self, String>> {
        let transfer = match c.transfer_characteristics {
            16 => HdrTransfer::Pq,
            18 => HdrTransfer::Hlg,
            _ => return None,
        };
        if c.matrix_coefficients != 0 || !c.full_range {
            return Some(Err(
                "tone mapping needs full-range RGB CICP (matrix 0)".into()
            ));
        }
        Some(match c.color_primaries {
            1 => Ok(Self {
                transfer,
                primaries: HdrPrimaries::Bt709,
            }),
            12 => Ok(Self {
                transfer,
                primaries: HdrPrimaries::DisplayP3,
            }),
            9 => Ok(Self {
                transfer,
                primaries: HdrPrimaries::Bt2020,
            }),
            p => Err(alloc::format!(
                "tone mapping supports BT.709, Display P3 and BT.2020 primaries, not CICP primaries {p}"
            )),
        })
    }

    fn from_descriptor(d: PixelDescriptor) -> Option<Result<Self, String>> {
        let transfer = match d.transfer() {
            TransferFunction::Pq => HdrTransfer::Pq,
            TransferFunction::Hlg => HdrTransfer::Hlg,
            _ => return None,
        };
        Some(match d.primaries {
            ColorPrimaries::Bt709 => Ok(Self {
                transfer,
                primaries: HdrPrimaries::Bt709,
            }),
            ColorPrimaries::DisplayP3 => Ok(Self {
                transfer,
                primaries: HdrPrimaries::DisplayP3,
            }),
            ColorPrimaries::Bt2020 => Ok(Self {
                transfer,
                primaries: HdrPrimaries::Bt2020,
            }),
            p => Err(alloc::format!(
                "tone mapping supports BT.709, Display P3 and BT.2020 primaries, not {p:?}"
            )),
        })
    }
}

/// A decision to tone-map an encode to sRGB.
#[derive(Clone, Debug)]
pub(crate) struct ToneMapPlan {
    /// The source colour; `None` takes it from the pixels' descriptor (the
    /// metadata describes no colour).
    pub(crate) source: Option<Result<HdrSource, String>>,
    /// The content peak the metadata gives (MaxCLL, else mastering peak).
    pub(crate) metadata_peak_nits: Option<f32>,
    /// The metadata to encode with: the colour description and HDR
    /// luminance metadata removed, since the pixels are sRGB now.
    pub(crate) metadata: Option<Metadata>,
    strict: bool,
    icc_authority: bool,
}

impl ToneMapPlan {
    /// The source for `desc`'s pixels, or `None` when they aren't HDR.
    pub(crate) fn source_for(&self, desc: PixelDescriptor) -> Option<Result<HdrSource, String>> {
        let source = match &self.source {
            Some(s) => Some(s.clone()),
            None if self.icc_authority => {
                // An unknown ICC remains authoritative. A PQ/HLG descriptor
                // cannot authorize replacing its transform with guessed CICP.
                HdrSource::from_descriptor(desc).map(|_| Err(
                    "HDR ICC profile cannot be rendered by the supported matrix/TRC tone-map path".into(),
                ))
            }
            None => HdrSource::from_descriptor(desc),
        };
        if source.is_some() && self.strict {
            return Some(Err("strict encode policy refuses HDR tone mapping".into()));
        }
        source
    }

    /// Shared content peak for every encode path and chunk size.
    pub(crate) fn peak(&self, source: HdrSource) -> f32 {
        match source.transfer {
            HdrTransfer::Hlg => HLG_REFERENCE_PEAK_NITS,
            HdrTransfer::Pq => self.metadata_peak_nits.unwrap_or(DEFAULT_PQ_PEAK_NITS),
        }
    }
}

/// Tone-map a one-shot encode's `pixels` when [`plan`] says so and they are
/// HDR: the sRGB pixels and the metadata to encode with. `None`: encode as
/// they are.
pub(crate) fn one_shot(
    pixels: &PixelSlice<'_>,
    metadata: Option<&Metadata>,
    caps: &EncodeCapabilities,
    policy: ToneMapPolicy,
    strict: bool,
) -> Result<Option<(PixelBuffer, Option<Metadata>)>, String> {
    let Some(plan) = plan(metadata, caps, policy, strict) else {
        return Ok(None);
    };
    let Some(source) = plan.source_for(pixels.descriptor()) else {
        return Ok(None);
    };
    let source = source?;
    let peak = plan.peak(source);
    let buf = tone_map_to_srgb8(pixels, source, peak)?;
    Ok(Some((buf, plan.metadata)))
}

/// Whether a target with `caps` gets tone-mapped pixels: it can carry
/// neither HDR nor CICP.
pub(crate) fn target_needs_tone_map(caps: &EncodeCapabilities) -> bool {
    !caps.hdr() && !caps.cicp()
}

/// Decide whether to tone-map an encode with `metadata` to a target with
/// `caps`. `None`: the target carries HDR, or the metadata describes a
/// colour that isn't HDR (the pixels' descriptor is then not consulted:
/// the metadata is the authority).
pub(crate) fn plan(
    metadata: Option<&Metadata>,
    caps: &EncodeCapabilities,
    policy: ToneMapPolicy,
    strict: bool,
) -> Option<ToneMapPlan> {
    if policy == ToneMapPolicy::Preserve || !target_needs_tone_map(caps) {
        return None;
    }
    let source = match metadata {
        Some(meta) => match (&meta.icc_profile, meta.cicp) {
            (Some(icc), _) => {
                if zenpixels::icc::is_common_srgb(icc) {
                    return None;
                }
                #[cfg(feature = "cms")]
                if crate::cms::is_srgb_icc_structural(icc) {
                    return None;
                }
                match zenpixels::icc::identify_common(icc)
                    .and_then(|id| id.to_cicp().map(|c| (id, c)))
                {
                    Some((id, cicp)) => {
                        let source = HdrSource::from_cicp(cicp)?;
                        if id.valid_use != zenpixels::icc::IdentificationUse::MatrixTrcSubstitution
                        {
                            Some(Err("HDR ICC profile requires a full color transform; CICP substitution is not valid".into()))
                        } else {
                            Some(source)
                        }
                    }
                    None => {
                        // A CICP tag alone does not prove the ICC transform
                        // is substitutable. It can establish an HDR refusal.
                        zenpixels::icc::extract_cicp(icc).and_then(HdrSource::from_cicp)
                            .map(|_| Err("HDR ICC profile requires a full color transform; CICP substitution is not valid".into()))
                    }
                }
            }
            (None, Some(cicp)) => Some(HdrSource::from_cicp(cicp)?),
            (None, None) => None,
        },
        None => None,
    };
    let metadata_peak_nits = metadata.and_then(metadata_peak_nits);
    let metadata_has_icc = metadata.is_some_and(|m| m.icc_profile.is_some());
    // With no color description the descriptor is resolved when pixels
    // arrive. Do not retain HDR luminance metadata on the possible SDR result;
    // non-color metadata remains intact.
    let metadata = metadata.map(|m| {
        let mut out = m.clone();
        if source.is_some() {
            out.icc_profile = None;
            out.cicp = None;
        }
        out.content_light_level = None;
        out.mastering_display = None;
        out
    });
    Some(ToneMapPlan {
        source,
        metadata_peak_nits,
        metadata,
        strict,
        icc_authority: metadata_has_icc,
    })
}

/// MaxCLL when present and non-zero, else the mastering display's peak.
fn metadata_peak_nits(meta: &Metadata) -> Option<f32> {
    let cll = meta
        .content_light_level
        .map(|c| c.max_content_light_level)
        .filter(|&m| m > 0)
        .map(f32::from);
    let mastering = meta
        .mastering_display
        .map(|m| m.max_luminance)
        .filter(|m| m.is_finite() && *m > 0.0);
    cll.or(mastering).map(sanitize_peak)
}

/// Keep a peak inside the range the EETF handles: at least SDR white (a
/// dimmer peak passes through unchanged anyway) and at most PQ's 10 000.
fn sanitize_peak(nits: f32) -> f32 {
    if nits.is_finite() {
        nits.clamp(SDR_WHITE_NITS, 10_000.0)
    } else {
        DEFAULT_PQ_PEAK_NITS
    }
}

/// How [`tone_map_to_srgb8`] reads a row: f32 or U16 samples, RGB or RGBA.
#[derive(Clone, Copy)]
struct SampleLayout {
    float: bool,
    alpha: bool,
    opaque: bool,
}

impl SampleLayout {
    fn of(desc: PixelDescriptor) -> Result<Self, String> {
        let float = match desc.channel_type() {
            ChannelType::F32 => true,
            ChannelType::U16 => false,
            t => {
                return Err(alloc::format!(
                    "tone mapping reads f32 or U16 samples, not {t:?}"
                ));
            }
        };
        let alpha = match desc.layout() {
            ChannelLayout::Rgb => false,
            ChannelLayout::Rgba => true,
            l => return Err(alloc::format!("tone mapping reads RGB or RGBA, not {l:?}")),
        };
        if desc.alpha() == Some(AlphaMode::Premultiplied) {
            return Err("tone mapping needs straight alpha, not premultiplied".to_string());
        }
        if desc.signal_range != SignalRange::Full {
            return Err("tone mapping needs full-range samples".to_string());
        }
        let opaque = matches!(desc.alpha(), Some(AlphaMode::Undefined | AlphaMode::Opaque));
        Ok(Self {
            float,
            alpha,
            opaque,
        })
    }

    fn channels(self) -> usize {
        if self.alpha { 4 } else { 3 }
    }

    /// Read one row into normalised code values (`rgb`) and alpha in 0..=1.
    /// Byte order is native, as zenpixels stores samples.
    fn read_row(self, row: &[u8], rgb: &mut [[f32; 3]], alpha: &mut [f32]) {
        let ch = self.channels();
        let size = if self.float { 4 } else { 2 };
        let sample = |px: &[u8], c: usize| -> f32 {
            let b = &px[c * size..(c + 1) * size];
            if self.float {
                f32::from_ne_bytes([b[0], b[1], b[2], b[3]])
            } else {
                f32::from(u16::from_ne_bytes([b[0], b[1]])) / 65_535.0
            }
        };
        for (x, px) in row.chunks_exact(ch * size).enumerate() {
            rgb[x] = [sample(px, 0), sample(px, 1), sample(px, 2)];
            if self.alpha {
                alpha[x] = if self.opaque {
                    1.0
                } else {
                    sample(px, 3).clamp(0.0, 1.0)
                };
            }
        }
    }
}

/// Tone-map HDR `pixels` in `source` colour with content peak `peak_nits`
/// to sRGB 8-bit (RGB8, or RGBA8 with straight alpha).
pub(crate) fn tone_map_to_srgb8(
    pixels: &PixelSlice<'_>,
    source: HdrSource,
    peak_nits: f32,
) -> Result<PixelBuffer, String> {
    let layout = SampleLayout::of(pixels.descriptor())?;
    let width = pixels.width() as usize;
    let rows = pixels.rows();
    let out_ch = layout.channels();
    let out_desc = if layout.alpha {
        PixelDescriptor::RGBA8_SRGB
    } else {
        PixelDescriptor::RGB8_SRGB
    };
    let len = width
        .checked_mul(rows as usize)
        .and_then(|n| n.checked_mul(out_ch))
        .ok_or_else(|| "tone-mapped image size overflows".to_string())?;
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| alloc::format!("allocating {len} bytes for the tone-mapped image"))?;
    out.resize(len, 0);

    let peak = match source.transfer {
        HdrTransfer::Pq => sanitize_peak(peak_nits),
        HdrTransfer::Hlg => HLG_REFERENCE_PEAK_NITS,
    };
    let luma = match source.primaries {
        HdrPrimaries::Bt709 => zentone::LUMA_BT709,
        HdrPrimaries::DisplayP3 => LUMA_P3,
        HdrPrimaries::Bt2020 => zentone::LUMA_BT2020,
    };
    let matrix = match source.primaries {
        HdrPrimaries::Bt709 => None,
        HdrPrimaries::DisplayP3 => Some(&zentone::gamut::P3_TO_BT709),
        HdrPrimaries::Bt2020 => Some(&zentone::gamut::BT2020_TO_BT709),
    };
    let curve = zentone::Bt2408Tonemapper::with_luma(peak, SDR_WHITE_NITS, luma);
    let hlg_gamma = zentone::hlg::hlg_system_gamma(HLG_REFERENCE_PEAK_NITS);
    // PQ decodes to 1.0 = 10 000 nits; the EETF reads 1.0 as the content
    // peak. HLG after the OOTF is 1.0 = the reference display peak, which is
    // its content peak.
    let pq_scale = 10_000.0 / peak;

    let mut rgb = vec![[0.0f32; 3]; width];
    let mut alpha = vec![0.0f32; if layout.alpha { width } else { 0 }];
    let mut srgb = vec![[0u8; 3]; width];
    for (y, dst) in (0..rows).zip(out.chunks_exact_mut(width * out_ch)) {
        layout.read_row(pixels.row(y), &mut rgb, &mut alpha);
        for p in rgb.iter_mut() {
            for c in p.iter_mut() {
                *c = if c.is_finite() {
                    c.clamp(0.0, 1.0)
                } else {
                    0.0
                };
            }
        }
        match source.transfer {
            HdrTransfer::Pq => {
                linear_srgb::default::pq_to_linear_slice(rgb.as_flattened_mut());
                for c in rgb.as_flattened_mut() {
                    *c *= pq_scale;
                }
            }
            HdrTransfer::Hlg => {
                linear_srgb::default::hlg_to_linear_slice(rgb.as_flattened_mut());
                zentone::hlg::hlg_ootf_row_simd(&mut rgb, hlg_gamma);
            }
        }
        curve.map_strip_simd(&mut rgb);
        if let Some(m) = matrix {
            zentone::gamut::apply_matrix_row_simd(m, &mut rgb);
        }
        zentone::gamut::soft_clip_row_simd(&mut rgb);
        linear_srgb::default::linear_to_srgb_u8_slice(rgb.as_flattened(), srgb.as_flattened_mut());
        if layout.alpha {
            for ((o, s), a) in dst.as_chunks_mut::<4>().0.iter_mut().zip(&srgb).zip(&alpha) {
                o[..3].copy_from_slice(s);
                o[3] = (a * 255.0 + 0.5) as u8;
            }
        } else {
            dst.copy_from_slice(srgb.as_flattened());
        }
    }
    PixelBuffer::from_vec(out, pixels.width(), rows, out_desc).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pq_code(nits: f32) -> f32 {
        linear_srgb::tf::linear_to_pq(nits / 10_000.0)
    }

    fn gray_pq_row(nits: &[f32]) -> Vec<u8> {
        nits.iter()
            .flat_map(|&n| {
                let c = pq_code(n);
                [c, c, c]
            })
            .flat_map(f32::to_ne_bytes)
            .collect()
    }

    fn tone_map_gray(nits: &[f32], peak: f32) -> Vec<u8> {
        let bytes = gray_pq_row(nits);
        let desc = PixelDescriptor::RGBF32_LINEAR.with_transfer(TransferFunction::Pq);
        let slice = PixelSlice::new(&bytes, nits.len() as u32, 1, bytes.len(), desc).unwrap();
        let src = HdrSource {
            transfer: HdrTransfer::Pq,
            primaries: HdrPrimaries::Bt709,
        };
        let out = tone_map_to_srgb8(&slice, src, peak).unwrap();
        out.as_slice().row(0).to_vec()
    }

    #[test]
    fn pq_gray_ramp_is_monotonic_and_reaches_white_at_the_peak() {
        let nits = [0.0, 1.0, 10.0, 50.0, 100.0, 203.0, 400.0, 1000.0];
        let out = tone_map_gray(&nits, 1000.0);
        let grays: Vec<u8> = out.as_chunks::<3>().0.iter().map(|p| p[0]).collect();
        for p in out.as_chunks::<3>().0 {
            assert!(p[0] == p[1] && p[1] == p[2], "gray stays neutral: {p:?}");
        }
        for w in grays.windows(2) {
            assert!(w[0] < w[1], "ramp must increase: {grays:?}");
        }
        assert_eq!(grays[0], 0);
        assert_eq!(grays[7], 255, "the content peak maps to SDR white");
        assert!(
            grays[5] >= 200,
            "203-nit reference white is near 255: {grays:?}"
        );
    }

    #[test]
    fn a_peak_above_the_content_is_respected() {
        // With a 4000-nit peak, 1000 nits is no longer white.
        let out = tone_map_gray(&[1000.0, 4000.0], 4000.0);
        assert!(out[0] < 255 && out[3] == 255, "{out:?}");
    }

    #[test]
    fn unsupported_samples_are_refused() {
        let bytes = [0u8; 12];
        let desc = PixelDescriptor::RGB8_SRGB.with_transfer(TransferFunction::Pq);
        let slice = PixelSlice::new(&bytes, 4, 1, 12, desc).unwrap();
        let src = HdrSource {
            transfer: HdrTransfer::Pq,
            primaries: HdrPrimaries::Bt2020,
        };
        assert!(tone_map_to_srgb8(&slice, src, 1000.0).is_err());
    }
    #[test]
    fn descriptor_only_conversion_clears_hdr_metadata() {
        let bytes = gray_pq_row(&[203.0, 400.0]);
        let desc = PixelDescriptor::RGBF32_LINEAR.with_transfer(TransferFunction::Pq);
        let slice = PixelSlice::new(&bytes, 2, 1, bytes.len(), desc).unwrap();
        for described in [false, true] {
            let mut meta = Metadata::none();
            meta.content_light_level = Some(zenpixels::hdr::ContentLightLevel::new(400, 100));
            meta.mastering_display = Some(zenpixels::hdr::MasteringDisplay::HDR10_REFERENCE);
            meta.xmp = Some(alloc::sync::Arc::from(&b"keep this XMP"[..]));
            if described {
                meta.cicp = Some(Cicp::new(1, 16, 0, true));
            }
            let caps = EncodeCapabilities::new();
            let (_, output) = one_shot(&slice, Some(&meta), &caps, ToneMapPolicy::Bt2408, false)
                .unwrap()
                .unwrap();
            let output = output.unwrap();
            assert!(output.icc_profile.is_none());
            assert!(output.cicp.is_none());
            assert!(output.content_light_level.is_none());
            assert!(output.mastering_display.is_none());
            assert_eq!(output.xmp, meta.xmp);
            // This is the metadata stamped onto the streaming codec job.
            let streamed = plan(Some(&meta), &caps, ToneMapPolicy::Bt2408, false)
                .unwrap()
                .metadata
                .unwrap();
            assert!(streamed.content_light_level.is_none());
            assert!(streamed.mastering_display.is_none());
            assert_eq!(streamed.xmp, meta.xmp);
        }
    }
    #[cfg(feature = "cms")]
    #[test]
    fn authoritative_icc_prevents_descriptor_hdr_fallback() {
        let mut meta = Metadata::none().with_icc(crate::cms::srgb_icc_profile());
        meta.cicp = Some(Cicp::new(9, 16, 0, true));
        assert!(
            plan(
                Some(&meta),
                &EncodeCapabilities::new(),
                ToneMapPolicy::Bt2408,
                false
            )
            .is_none()
        );
    }
    #[test]
    fn u16_rgba_and_float_rgb_respect_stride_and_strip_boundaries() {
        let source = HdrSource {
            transfer: HdrTransfer::Pq,
            primaries: HdrPrimaries::Bt2020,
        };
        for rgba_u16 in [false, true] {
            let desc = if rgba_u16 {
                PixelDescriptor::RGBA16_SRGB.with_transfer(TransferFunction::Pq)
            } else {
                PixelDescriptor::RGBF32_LINEAR.with_transfer(TransferFunction::Pq)
            }
            .with_primaries(ColorPrimaries::Bt2020);
            let row: Vec<u8> = [50.0, 203.0, 400.0, 2000.0]
                .into_iter()
                .flat_map(|n| {
                    let c = pq_code(n);
                    if rgba_u16 {
                        [
                            ((c * 65535.0).round() as u16),
                            ((c * 65535.0).round() as u16),
                            ((c * 65535.0).round() as u16),
                            32768,
                        ]
                        .into_iter()
                        .flat_map(u16::to_ne_bytes)
                        .collect::<Vec<_>>()
                    } else {
                        [c; 3]
                            .into_iter()
                            .flat_map(f32::to_ne_bytes)
                            .collect::<Vec<_>>()
                    }
                })
                .collect();
            let padding = 2 * desc.bytes_per_pixel();
            let stride = row.len() + padding;
            let mut padded = row.clone();
            padded.extend_from_slice(&vec![0xab; padding]);
            padded.extend_from_slice(&row); // final row has no padding
            let pixels = PixelSlice::new(&padded, 4, 2, stride, desc).unwrap();
            let whole = tone_map_to_srgb8(&pixels, source, 1000.0).unwrap();
            let single = PixelSlice::new(&row, 4, 1, row.len(), desc).unwrap();
            let strip = tone_map_to_srgb8(&single, source, 1000.0).unwrap();
            for y in 0..2 {
                assert_eq!(whole.as_slice().row(y), strip.as_slice().row(0));
            }
            if rgba_u16 {
                for p in whole.as_slice().row(0).as_chunks::<4>().0 {
                    assert_eq!(p[3], 128);
                }
            }
            if rgba_u16 {
                for alpha in [AlphaMode::Undefined, AlphaMode::Opaque] {
                    let ignored =
                        PixelSlice::new(&padded, 4, 2, stride, desc.with_alpha(Some(alpha)))
                            .unwrap();
                    let opaque = tone_map_to_srgb8(&ignored, source, 1000.0).unwrap();
                    for y in 0..2 {
                        for p in opaque.as_slice().row(y).as_chunks::<4>().0 {
                            assert_eq!(p[3], 255);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn unknown_icc_never_authorizes_descriptor_hdr_substitution() {
        let mut meta = Metadata::none().with_icc(b"unidentified ICC".to_vec());
        meta.cicp = Some(Cicp::new(9, 16, 0, true));
        let plan = plan(
            Some(&meta),
            &EncodeCapabilities::new(),
            ToneMapPolicy::Bt2408,
            false,
        )
        .unwrap();
        assert!(
            plan.source_for(PixelDescriptor::RGBF32_LINEAR.with_transfer(TransferFunction::Pq))
                .unwrap()
                .is_err()
        );
        // Secondary HDR CICP does not override the unknown ICC for SDR pixels.
        assert!(plan.source_for(PixelDescriptor::RGB8_SRGB).is_none());
    }
}
