//! Transcode API and streaming decode→encode bridge.
//!
//! ## Transcode function
//!
//! [`transcode()`] provides a high-level one-call transcode: decode the input,
//! re-encode to the target format specified by a [`FormatDecision`], and return
//! the encoded bytes. Metadata (EXIF, ICC, XMP) is roundtripped by default.
//!
//! ## TranscodeSink
//!
//! [`TranscodeSink`] is the low-level streaming bridge. It implements
//! [`DecodeRowSink`] and forwards decoded strips directly to an encoder's
//! `push_rows()`, converting pixel formats per-strip via `adapt_for_encode_cow`.
//! No full-image buffer is ever allocated by the sink — only a strip-sized
//! conversion buffer when the decoded pixel format doesn't match the
//! encoder's native format.
//!
//! Codecs that need the full image (WebP, AVIF) buffer internally in their
//! `push_rows()` implementation. That's the codec's concern, not the
//! pipeline's.

use alloc::boxed::Box;
use alloc::vec::Vec;

use enough::Stop as _;
use zencodec::MetadataPolicy;
use zencodec::decode::{DecodeRowSink, RowStrip, SinkError};
use zencodec::encode::{DynEncoder, EncodeOutput};
use zenpixels::{PixelFormat, PixelView, PixelViewMut};

use crate::decision::FormatDecision;
use crate::error::Result;
use crate::{AllowedFormats, CodecError, ImageFormat};
use whereat::at;

// ═══════════════════════════════════════════════════════════════════════
// TranscodeOptions, SupplementPolicy, SupplementSet
// ═══════════════════════════════════════════════════════════════════════

/// Policy for animated inputs passed to [`transcode()`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnimationPolicy {
    /// Refuse animated inputs with [`CodecError::UnsupportedOperation`] rather
    /// than silently dropping frames (default).
    #[default]
    Refuse,
    /// Decode and transcode only the first frame of an animated input.
    FirstFrame,
}

/// Options controlling a transcode operation.
///
/// Controls metadata roundtrip, supplement handling, resource limits,
/// cancellation/progress pulse, animation policy, tone mapping, and alpha
/// compositing.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TranscodeOptions {
    /// Metadata to embed in the output (EXIF, ICC, XMP).
    ///
    /// - `None` (default): extract metadata from the source and roundtrip it.
    /// - `Some(meta)`: use the provided metadata instead of the source's.
    pub metadata: Option<zencodec::Metadata>,

    /// Pixel color policy. Metadata overrides do not perform color conversion.
    pub color: TranscodeColor,

    /// Retention policy applied to the embedded metadata.
    ///
    /// Defaults to [`MetadataPolicy::PreserveExact`] (verbatim roundtrip, with a
    /// stale EXIF orientation tag reconciled against the authoritative field).
    /// Set [`MetadataPolicy::Web`] to strip GPS/camera/timestamps/XMP for web
    /// publishing while keeping orientation, rights, and color signaling.
    pub metadata_policy: MetadataPolicy,

    /// How to handle container supplements (gain maps, depth maps, etc.)
    /// during transcode.
    pub supplements: SupplementPolicy,

    /// Matte color for alpha compositing when encoding to a format without
    /// alpha (e.g., RGBA source → JPEG output).
    ///
    /// Overrides [`FormatDecision::matte`]. When both are `None`, no explicit
    /// matte compositing is forced.
    pub matte: Option<[u8; 3]>,

    /// Whether to bake EXIF/container orientation into the output pixels.
    ///
    /// Defaults to [`OrientationHint::Preserve`] (the orientation stays in the
    /// metadata tag). Set [`OrientationHint::Correct`] to bake it — required for
    /// targets that can't carry an orientation tag (e.g. PNG) and the right
    /// choice for display-ready output. Only affects the pixel-decode path
    /// (JPEG sources); coefficient-domain transcodes are unaffected.
    pub orientation: zencodec::OrientationHint,

    /// Resource limits forwarded to both decode and encode requests.
    pub limits: Option<crate::Limits>,

    /// Cancellation/progress pulse forwarded to decode and encode requests.
    pub pulse: Option<crate::SharedPulse>,

    /// Policy for animated inputs (default: [`AnimationPolicy::Refuse`]).
    pub animation: AnimationPolicy,

    /// Explicit HDR→SDR tone-mapping policy.
    ///
    /// When [`TranscodeColor::Srgb8`] encounters an HDR source:
    /// - `None` or `Some(ToneMapPolicy::Preserve)` refuses the conversion.
    /// - `Some(ToneMapPolicy::Bt2408)` tone-maps HDR to sRGB8 via BT.2408.
    ///
    /// Also forwarded to the internal [`EncodeRequest`](crate::EncodeRequest)
    /// when `Some`.
    #[cfg(feature = "tonemap")]
    pub tone_map: Option<crate::ToneMapPolicy>,
}

impl Default for TranscodeOptions {
    fn default() -> Self {
        Self {
            metadata: None,
            color: TranscodeColor::Preserve,
            // No implicit privacy choice: roundtrip verbatim by default. Callers
            // publishing to the web should set `MetadataPolicy::Web`.
            metadata_policy: MetadataPolicy::PreserveExact,
            supplements: SupplementPolicy::default(),
            matte: None,
            // No implicit pixel rewrite: preserve the tag by default. Callers
            // targeting PNG or wanting display-ready output set `Correct`.
            orientation: zencodec::OrientationHint::Preserve,
            limits: None,
            pulse: None,
            animation: AnimationPolicy::Refuse,
            #[cfg(feature = "tonemap")]
            tone_map: None,
        }
    }
}

impl TranscodeOptions {
    /// Create default transcode options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Override output metadata instead of roundtripping source metadata.
    pub fn with_metadata(mut self, metadata: zencodec::Metadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Set pixel color policy (`Preserve` or `Srgb8`).
    pub fn with_color(mut self, color: TranscodeColor) -> Self {
        self.color = color;
        self
    }

    /// Set metadata retention policy.
    pub fn with_metadata_policy(mut self, policy: MetadataPolicy) -> Self {
        self.metadata_policy = policy;
        self
    }

    /// Set container supplement handling policy.
    pub fn with_supplements(mut self, supplements: SupplementPolicy) -> Self {
        self.supplements = supplements;
        self
    }

    /// Set matte color for compositing alpha onto opaque target formats.
    pub fn with_matte(mut self, matte: [u8; 3]) -> Self {
        self.matte = Some(matte);
        self
    }

    /// Set decode orientation handling hint.
    pub fn with_orientation(mut self, orientation: zencodec::OrientationHint) -> Self {
        self.orientation = orientation;
        self
    }

    /// Set resource limits for decode and encode.
    pub fn with_limits(mut self, limits: crate::Limits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Set cancellation/progress pulse for decode and encode.
    pub fn with_pulse(mut self, pulse: crate::SharedPulse) -> Self {
        self.pulse = Some(pulse);
        self
    }

    /// Set animation handling policy (default: [`AnimationPolicy::Refuse`]).
    pub fn with_animation(mut self, animation: AnimationPolicy) -> Self {
        self.animation = animation;
        self
    }

    /// Set explicit HDR→SDR tone-mapping policy.
    #[cfg(feature = "tonemap")]
    pub fn with_tone_map_policy(mut self, policy: crate::ToneMapPolicy) -> Self {
        self.tone_map = Some(policy);
        self
    }
}

/// Color conversion requested by a transcode caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum TranscodeColor {
    /// Keep the decoded pixel colors and precision. A target that cannot carry
    /// the source color description may reject the request (for example GIF).
    #[default]
    Preserve,
    /// Convert SDR pixels through their current ICC/CICP description to straight
    /// RGBA8 sRGB. Requires `cms`. Replaces output ICC/CICP/HDR signaling, while
    /// retaining unrelated metadata. PQ/HLG require an explicit rendering policy
    /// and are rejected; preserving a gain map across this conversion is rejected.
    Srgb8,
}

/// What to do with container supplements (gain maps, depth maps, etc.)
/// during transcode.
#[derive(Clone, Copy, Debug, Default)]
pub enum SupplementPolicy {
    /// Roundtrip all supplements the target format supports.
    ///
    /// Gain maps, depth maps, and auxiliary images are extracted from the
    /// source container and re-embedded in the output container.
    /// Supplements that the target format can't represent are silently dropped.
    #[default]
    Preserve,

    /// Strip all supplements. Output contains only the primary image + metadata.
    Strip,

    /// Preserve only specific supplement types.
    Only(SupplementSet),
}

/// Bitflag set of supplement types.
///
/// Used with [`SupplementPolicy::Only`] to selectively preserve supplements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupplementSet(u32);

impl SupplementSet {
    /// UltraHDR / ISO 21496-1 gain map.
    pub const GAIN_MAP: Self = Self(1);
    /// Depth / disparity map.
    pub const DEPTH_MAP: Self = Self(2);
    /// Embedded thumbnail.
    pub const THUMBNAIL: Self = Self(4);

    /// Check whether a specific supplement type is in this set.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Combine two supplement sets (union).
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Check whether this set is empty.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOr for SupplementSet {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TranscodeOutput
// ═══════════════════════════════════════════════════════════════════════

/// The result of a [`transcode()`] operation.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TranscodeOutput {
    /// The encoded image bytes.
    pub data: Vec<u8>,
    /// The output format.
    pub format: ImageFormat,
    /// The MIME type of the output format.
    pub mime_type: &'static str,
}

/// Composite an alpha-bearing buffer onto an opaque RGB8 matte color, returning
/// an opaque `RGB8_SRGB` buffer.
fn composite_matte(buffer: zenpixels::PixelBuf, matte: [u8; 3]) -> Result<zenpixels::PixelBuf> {
    let rgba = if *buffer.format() == PixelFormat::RGBA8_SRGB {
        buffer
    } else {
        zenpixels_convert::convert_to_buf(
            &buffer.as_view(),
            &PixelFormat::RGBA8_SRGB,
            &zenpixels_convert::ConvertPolicy::permissive(),
        )
        .map_err(|e| {
            at!(CodecError::InvalidInput(alloc::format!(
                "matte composite: {e}"
            )))
        })?
    };
    let width = rgba.width();
    let height = rgba.height();
    let view = rgba.as_view();
    let out_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| {
            at!(CodecError::InvalidInput(
                "matte output size overflows".into()
            ))
        })?;
    let mut out = Vec::new();
    out.try_reserve_exact(out_len)
        .map_err(|_| at!(CodecError::Oom))?;
    let [mr, mg, mb] = [
        u16::from(matte[0]),
        u16::from(matte[1]),
        u16::from(matte[2]),
    ];
    for y in 0..height {
        let row = view.row(y);
        for px in row.as_chunks::<4>().0 {
            let a = u16::from(px[3]);
            let inv = 255 - a;
            out.push(((u16::from(px[0]) * a + mr * inv + 127) / 255) as u8);
            out.push(((u16::from(px[1]) * a + mg * inv + 127) / 255) as u8);
            out.push(((u16::from(px[2]) * a + mb * inv + 127) / 255) as u8);
        }
    }
    zenpixels::PixelBuf::from_bytes(out, width, height, PixelFormat::RGB8_SRGB).map_err(|e| {
        at!(CodecError::InvalidInput(alloc::format!(
            "matte buffer: {e}"
        )))
    })
}

// ═══════════════════════════════════════════════════════════════════════
// transcode() — high-level one-call transcode
// ═══════════════════════════════════════════════════════════════════════

/// Transcode an image: decode from `data`, re-encode to the format and
/// quality specified by `decision`.
///
/// This is the primary transcode entry point. Metadata (EXIF, ICC, XMP)
/// is roundtripped from the source unless overridden in `opts`.
///
/// Animated inputs are refused by default ([`AnimationPolicy::Refuse`]) so
/// frames are never silently dropped; opt in with [`AnimationPolicy::FirstFrame`]
/// to transcode frame 0, or use [`EncodeRequest::animation_frame_encoder`](crate::EncodeRequest::animation_frame_encoder)
/// to preserve animation.
///
/// # Example
///
/// ```rust,ignore
/// use zencodecs::{transcode, TranscodeOptions, FormatDecision, AllowedFormats};
/// use zencodecs::ImageFormat;
///
/// let decision = FormatDecision::for_format_quality(ImageFormat::WebP, 80.0);
///
/// let output = zencodecs::transcode(
///     &jpeg_bytes,
///     &decision,
///     &TranscodeOptions::default(),
///     &AllowedFormats::all(),
/// )?;
/// assert_eq!(output.format, ImageFormat::WebP);
/// ```
pub fn transcode(
    data: &[u8],
    decision: &FormatDecision,
    opts: &TranscodeOptions,
    registry: &AllowedFormats,
) -> Result<TranscodeOutput> {
    if let Some(ref pulse) = opts.pulse {
        pulse.check().map_err(|r| at!(CodecError::Stopped(r)))?;
    }

    // Probe unconditionally before decode so animated inputs are caught up front
    // under `AnimationPolicy::Refuse` (issue #111) and metadata is reused without
    // probing twice.
    let probed = match crate::info::from_bytes_with_registry(data, registry) {
        Ok(info) => {
            if opts.animation == AnimationPolicy::Refuse && info.is_animation() {
                return Err(at!(CodecError::UnsupportedOperation {
                    format: info.format,
                    detail: "animated input refused by AnimationPolicy::Refuse; \
                             use AnimationPolicy::FirstFrame or an animation encoder",
                }));
            }
            Some(info)
        }
        Err(e)
            if matches!(
                e.error(),
                CodecError::DisabledFormat(_)
                    | CodecError::UnrecognizedFormat
                    | CodecError::UnsupportedFormat(_)
            ) =>
        {
            return Err(e);
        }
        Err(_) => None,
    };

    // Determine whether we need gain map data from the decode side.
    let wants_gain_map = match opts.supplements {
        SupplementPolicy::Preserve => true,
        SupplementPolicy::Only(set) => set.contains(SupplementSet::GAIN_MAP),
        SupplementPolicy::Strip => false,
    };

    let build_decode = || {
        let mut req = crate::DecodeRequest::new(data)
            .with_registry(*registry)
            .with_orientation(opts.orientation);
        if let Some(limits) = opts.limits {
            req = req.with_limits(limits);
        }
        if let Some(ref pulse) = opts.pulse {
            req = req.with_pulse(pulse.clone());
        }
        req
    };

    // Step 1: Decode the source image (full materialization for now). When the
    // caller preserves supplements and the build has gain-map support, also
    // recover the decoded gain map so it can be re-embedded (passthrough) below.
    #[cfg(feature = "jpeg-ultrahdr")]
    let (decoded, gain_map) = if wants_gain_map {
        build_decode().decode_gain_map()?
    } else {
        (build_decode().decode_full_frame()?, None)
    };
    #[cfg(not(feature = "jpeg-ultrahdr"))]
    let decoded = {
        let _ = wants_gain_map;
        build_decode().decode_full_frame()?
    };

    // Second animation check against the decoded ImageInfo (catches formats
    // whose header probe cannot determine frame count until decoding).
    if opts.animation == AnimationPolicy::Refuse && decoded.info().is_animation() {
        return Err(at!(CodecError::UnsupportedOperation {
            format: decoded.info().format,
            detail: "animated input refused by AnimationPolicy::Refuse; \
                     use AnimationPolicy::FirstFrame or an animation encoder",
        }));
    }

    // Carry the resolved source color through to the encoder so it emits the right
    // cICP. Unspecified-nclx + ICC ⇒ Display P3 resolves into
    // `source_color`, NOT the EXIF/ICC metadata roundtripped below, and the pixel
    // descriptor's color-space enum can't represent an arbitrary ICC-resolved primary
    // set — so without this explicit carrier a wide-gamut source→PNG is silently
    // mislabeled sRGB. `None` for sRGB-class sources (the encoder's default).
    #[allow(unused_mut)] // Mutated by the optional CMS conversion.
    let mut src_cicp = decoded.info().source_color.cicp;
    #[allow(unused_variables)] // Used only by the optional cms conversion.
    let source_color = decoded.info().source_color.clone();

    // Step 2: Determine metadata to embed
    #[allow(unused_mut)] // Mutated by the optional CMS conversion.
    let mut metadata = match opts.metadata.clone() {
        Some(m) => m,
        None => {
            let mut m = match probed {
                Some(info) => info.metadata(),
                None => zencodec::Metadata::none(),
            };
            // The probe reports the source's orientation, but the decode may
            // already have applied it (`OrientationHint::Correct`). What the
            // encode must still apply is what the decode reports; carrying the
            // probe's value would rotate the pixels twice.
            m.orientation = decoded.info().orientation;
            m
        }
    };

    let buffer = decoded.into_buffer();
    let mut buffer = match opts.color {
        TranscodeColor::Preserve => buffer,
        TranscodeColor::Srgb8 => {
            #[cfg(not(feature = "cms"))]
            return Err(at!(CodecError::UnsupportedOperation {
                format: decision.format,
                detail: "SDR sRGB conversion requires the cms feature",
            }));
            #[cfg(feature = "cms")]
            {
                let src_fmt = zencodec::helpers::pixel_format_for_decoded_pixels(
                    buffer.format().layout,
                    &source_color,
                    None,
                )
                .map_err(|e| at!(CodecError::ColorManagement(alloc::format!("{e}"))))?;
                let is_hdr = matches!(
                    src_fmt.color,
                    zenpixels::ColorSpace::Rgb { transfer, .. }
                        | zenpixels::ColorSpace::Gray { transfer, .. }
                        if transfer.is_hdr()
                ) || matches!(
                    buffer.format().color,
                    zenpixels::ColorSpace::Rgb { transfer, .. }
                        | zenpixels::ColorSpace::Gray { transfer, .. }
                        if transfer.is_hdr()
                );
                let hdr_input = is_hdr
                    || src_cicp.is_some_and(|c| matches!(c.transfer_characteristics, 16 | 18));
                #[cfg(feature = "jpeg-ultrahdr")]
                if gain_map.is_some() {
                    return Err(at!(CodecError::ColorManagement(
                        "sRGB conversion cannot preserve an unchanged gain map; select SupplementPolicy::Strip".into()
                    )));
                }
                if hdr_input {
                    #[cfg(feature = "tonemap")]
                    if let Some(crate::ToneMapPolicy::Bt2408) = opts.tone_map {
                        let relabeled = buffer
                            .relabel_format(src_fmt)
                            .map_err(|e| at!(CodecError::ColorManagement(alloc::format!("{e}"))))?;
                        let toned = crate::tonemap::one_shot(
                            &relabeled.as_view(),
                            Some(&metadata),
                            &zencodec::encode::EncodeCapabilities::new(),
                            crate::ToneMapPolicy::Bt2408,
                            false,
                        )
                        .map_err(|e| at!(CodecError::ColorManagement(e)))?
                        .ok_or_else(|| {
                            at!(CodecError::ColorManagement(
                                "HDR-to-SDR tone mapping could not determine an HDR source color"
                                    .into()
                            ))
                        })?
                        .0;
                        let rgba = if *toned.format() == PixelFormat::RGBA8_SRGB {
                            toned
                        } else {
                            zenpixels_convert::convert_to_buf(
                                &toned.as_view(),
                                &PixelFormat::RGBA8_SRGB,
                                &zenpixels_convert::ConvertPolicy::permissive(),
                            )
                            .map_err(|e| at!(CodecError::ColorManagement(alloc::format!("{e}"))))?
                        };
                        metadata.icc_profile = None;
                        metadata.cicp = Some(zencodec::Cicp::SRGB);
                        metadata.content_light_level = None;
                        metadata.mastering_display = None;
                        metadata.diffuse_white = None;
                        src_cicp = metadata.cicp;
                        rgba
                    } else {
                        return Err(at!(CodecError::ColorManagement(
                            "HDR-to-SDR requires an explicit display and tone-map policy".into()
                        )));
                    }
                    #[cfg(not(feature = "tonemap"))]
                    return Err(at!(CodecError::ColorManagement(
                        "HDR-to-SDR requires an explicit display and tone-map policy".into()
                    )));
                } else {
                    // Relabel the decoded buffer with the authoritative source color
                    // (ICC or CICP per `color_authority`) so the CMS converts e.g.
                    // embedded Adobe RGB instead of assuming sRGB, and so malformed
                    // ICC bytes fail here instead of being silently dropped.
                    // Only normalize Unknown transfer/primaries for genuinely untagged
                    // sources (no ICC and no CICP), never when an attached ICC failed
                    // header identification.
                    let src_fmt =
                        if source_color.icc_profile.is_none() && source_color.cicp.is_none() {
                            crate::dispatch::normalize_unknown_format(&src_fmt).unwrap_or(src_fmt)
                        } else {
                            src_fmt
                        };
                    let buffer = buffer
                        .relabel_format(src_fmt.clone())
                        .map_err(|e| at!(CodecError::ColorManagement(alloc::format!("{e}"))))?;
                    let policy = zenpixels_convert::ConvertPolicy::permissive()
                        .with_cms(alloc::sync::Arc::new(zenpixels_convert::cms::MoxCms));
                    let plan = zenpixels_convert::OutputPlan::build(
                        &src_fmt,
                        &PixelFormat::RGBA8_SRGB,
                        &policy,
                        zenpixels::PeakSource::Declared,
                        zenpixels_convert::OutputSignaling::Strip,
                    )
                    .map_err(|e| at!(CodecError::ColorManagement(alloc::format!("{e}"))))?;
                    let ready = plan
                        .execute_buf(buffer)
                        .map_err(|e| at!(CodecError::ColorManagement(alloc::format!("{e}"))))?;
                    metadata.icc_profile = None;
                    metadata.cicp = Some(zencodec::Cicp::SRGB);
                    metadata.content_light_level = None;
                    metadata.mastering_display = None;
                    metadata.diffuse_white = None;
                    src_cicp = metadata.cicp;
                    ready
                        .pixels
                        .into_owned()
                        .map_err(|e| at!(CodecError::ColorManagement(alloc::format!("{e}"))))?
                }
            }
        }
    };

    // Step 3: Encode to the target format
    let format = decision.format;
    if !registry.can_encode(format) {
        return Err(at!(CodecError::DisabledFormat(format)));
    }

    // Apply explicit matte compositing when targeting an opaque format with alpha pixels.
    let has_alpha = matches!(
        buffer.format().color.alpha(),
        zenpixels::AlphaMode::Straight | zenpixels::AlphaMode::Premultiplied
    );
    if has_alpha
        && !format.supports_alpha()
        && let Some(matte) = opts.matte.or(decision.matte)
    {
        buffer = composite_matte(buffer, matte)?;
    }

    // Resolve per-codec hint overrides from `decision.hints`.
    let quality = decision
        .hints
        .get("quality")
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(decision.quality.quality);
    let effort = decision
        .hints
        .get("effort")
        .and_then(|s| s.parse::<u32>().ok())
        .or_else(|| {
            if format == ImageFormat::Avif {
                decision
                    .hints
                    .get("speed")
                    .and_then(|s| s.parse::<u32>().ok())
                    .map(|s| 10u32.saturating_sub(s))
            } else {
                None
            }
        })
        .or(decision.quality.effort);
    let lossless = decision
        .hints
        .get("lossless")
        .and_then(|s| match s.as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        })
        .unwrap_or(decision.lossless);

    // Build the encode request from the decision + options
    let mut request = crate::EncodeRequest::new(format)
        .with_quality(quality)
        .with_metadata(metadata)
        .with_metadata_policy(opts.metadata_policy)
        .with_cicp(src_cicp)
        .with_registry(*registry);

    if lossless {
        request = request.with_lossless(true);
    }
    if let Some(effort) = effort {
        request = request.with_effort(effort);
    }
    if let Some(limits) = opts.limits {
        request = request.with_limits(limits);
    }
    if let Some(ref pulse) = opts.pulse {
        request = request.with_pulse(pulse.clone());
    }
    #[cfg(feature = "tonemap")]
    if let Some(policy) = opts.tone_map {
        request = request.with_tone_map_policy(policy);
    }

    // Passthrough re-embed: carry the source gain map into the target container
    // unchanged. `DecodedGainMap.gain_map` / `.metadata` are already exactly the
    // types `GainMapSource::Precomputed` borrows (`ultrahdr_core::GainMap` and
    // the `zencodec::GainMapParams` alias) — no conversion, no recomputation.
    // A target encoder without gain-map support ignores it.
    #[cfg(feature = "jpeg-ultrahdr")]
    if let Some(ref dgm) = gain_map {
        request = request.with_gain_map(crate::gainmap::GainMapSource::Precomputed {
            gain_map: &dgm.gain_map,
            metadata: &dgm.metadata,
        });
    }

    let encode_output = request.encode(buffer.as_view())?;

    Ok(TranscodeOutput {
        data: encode_output.into_vec(),
        format,
        mime_type: format.mime_type(),
    })
}

// ═══════════════════════════════════════════════════════════════════════
// transcode_to_quality — hit a zensim Profile-A QualityTarget, one-shot
// ═══════════════════════════════════════════════════════════════════════

/// What a transcode quality target means relative to the (already lossy) source,
/// in **zensim Profile-A** units (0–100, higher = closer to the original).
///
/// The source is itself lossy, so the two readings differ — see
/// [`TRANSCODE-GRID.md`](https://github.com/imazen/zenpipe/blob/main/zencodecs/TRANSCODE-GRID.md).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QualityTarget {
    /// zensim-A vs the **original** image (the default meaning). Because the source
    /// already discarded detail, the achievable target is clamped to the source's
    /// inferred quality-vs-original floor — the output is never promised more
    /// fidelity than the source itself has. One number, comparable across sources.
    Absolute(f32),
    /// zensim-A vs the **decoded source pixels** — a cap on generation loss. No
    /// floor model; not comparable across sources of different starting quality.
    Relative(f32),
}

impl QualityTarget {
    /// The bare zensim-A score (0–100), regardless of reference.
    pub fn score(self) -> f32 {
        match self {
            Self::Absolute(t) | Self::Relative(t) => t,
        }
    }
}

/// Transcode `data` to `target` at the **smallest byte size meeting a zensim
/// Profile-A [`QualityTarget`]**, using the best method for the source→dest pair.
///
/// **One-shot** — the result is produced by either a coefficient-domain transcode
/// or a single picker-predicted encode; there is no bisection search.
///
/// 1. **Coefficient-domain** (no pixel round-trip, no generation loss) where the
///    pair shares a DCT representation:
///    - **JPEG→JPEG** — [`zenjpeg::recompress`] (re-quantize coefficients).
///    - **JPEG→JXL** — [`zenjxl::jpeg_lossy`] (coarsen + lossless floor; needs
///      `transcode-iqa`).
///    - **JXL→JPEG** — [`reconstruct_jpeg_from_jxl`] when the JXL is a JBRD
///      transcode (byte-exact); otherwise falls through to the JPEG picker.
/// 2. **One-shot picker** otherwise — decode the source once, ask the dest codec's
///    learned picker for the knob that lands on `target`, and encode once. See
///    the `TRANSCODE-GRID.md` doc for the per-dest predictor.
pub fn transcode_to_quality(
    data: &[u8],
    target: ImageFormat,
    quality: QualityTarget,
    opts: &TranscodeOptions,
    registry: &AllowedFormats,
) -> Result<TranscodeOutput> {
    if let Some(ref pulse) = opts.pulse {
        pulse.check().map_err(|r| at!(CodecError::Stopped(r)))?;
    }
    let source =
        crate::info::detect_format(data).ok_or_else(|| at!(CodecError::UnrecognizedFormat))?;

    // 1. Coefficient-domain native paths — no pixel round-trip. These bypass
    // `DecodeRequest`/`EncodeRequest` entirely (raw byte-level recompression),
    // so — unlike the generic `transcode()` above, which gets registry
    // enforcement for free through those types — each arm must check the
    // registry itself. Same authority as the generic path: `DisabledFormat`
    // on the offending side, not a silent fall-through to the (stub)
    // one-shot picker below.
    match (source, target) {
        #[cfg(feature = "jpeg")]
        (ImageFormat::Jpeg, ImageFormat::Jpeg) => {
            if !registry.can_decode(source) {
                return Err(at!(CodecError::DisabledFormat(source)));
            }
            if !registry.can_encode(target) {
                return Err(at!(CodecError::DisabledFormat(target)));
            }
            return recompress_jpeg_to_jpeg(data, quality);
        }
        #[cfg(all(feature = "jpeg", feature = "transcode-iqa", feature = "jxl-decode"))]
        (ImageFormat::Jpeg, ImageFormat::Jxl) => {
            if !registry.can_decode(source) {
                return Err(at!(CodecError::DisabledFormat(source)));
            }
            // This path produces JXL via the coefficient-domain recompressor
            // (`transcode-iqa` + `jxl-decode`), independent of the `jxl-encode`
            // feature that gates the *generic* pixel-level JXL encoder (and
            // thus `COMPILED_ENCODE`'s Jxl bit, which `can_encode` checks).
            // Only enforce the registry's Jxl-encode toggle when `jxl-encode`
            // is *also* compiled — i.e. when `can_encode(Jxl)` is tracking a
            // real, caller-disableable capability, not an untracked one that
            // would always read `false` here regardless of the registry.
            if cfg!(feature = "jxl-encode") && !registry.can_encode(target) {
                return Err(at!(CodecError::DisabledFormat(target)));
            }
            return recompress_jpeg_to_jxl(data, quality);
        }
        #[cfg(feature = "jxl-jpeg-reconstruct")]
        (ImageFormat::Jxl, ImageFormat::Jpeg) => {
            // `jxl-jpeg-reconstruct` reads the JBRD box directly via
            // zenjxl-decoder, independent of the generic `jxl-decode`
            // pixel-decode path (and thus `COMPILED_DECODE`'s Jxl bit) — same
            // "only enforce when meaningfully tracked" reasoning as above.
            if cfg!(feature = "jxl-decode") && !registry.can_decode(source) {
                return Err(at!(CodecError::DisabledFormat(source)));
            }
            // Reconstruction returns the ORIGINAL JPEG bytes verbatim (no
            // zenjpeg encode involved), so only enforce when the generic
            // `jpeg` feature is also compiled.
            if cfg!(feature = "jpeg") && !registry.can_encode(target) {
                return Err(at!(CodecError::DisabledFormat(target)));
            }
            // Byte-exact iff the JXL is a JBRD JPEG transcode; otherwise fall
            // through to the one-shot JPEG picker on the decoded pixels.
            if let Some(jpeg) = reconstruct_jpeg_from_jxl(data)? {
                return Ok(TranscodeOutput {
                    data: jpeg,
                    format: ImageFormat::Jpeg,
                    mime_type: ImageFormat::Jpeg.mime_type(),
                });
            }
        }
        _ => {}
    }

    // 2. One-shot picker: decode source → dest picker → encode once.
    transcode_via_picker(data, target, quality, opts, registry)
}

/// One-shot picker dispatch: decode the source to pixels once, ask the dest
/// codec's learned picker for the knob that lands on `target`, and encode once.
///
/// Wired column-by-column (feature-aware pickers first); see the build order in
/// `TRANSCODE-GRID.md`.
fn transcode_via_picker(
    data: &[u8],
    target: ImageFormat,
    quality: QualityTarget,
    opts: &TranscodeOptions,
    registry: &AllowedFormats,
) -> Result<TranscodeOutput> {
    let _ = (data, quality, opts, registry);
    Err(at!(CodecError::UnsupportedOperation {
        format: target,
        detail: "one-shot picker not yet wired for this dest. Planned predictors: \
                 AVIF (zenavif auto_tune), JPEG (zenjpeg encode/picker), WebP \
                 (zenwebp picker), JXL (calibrated_jxl_quality). See \
                 TRANSCODE-GRID.md.",
    }))
}

/// JPEG→JPEG: coefficient-domain recompression to a zensim Profile-A target.
/// Returns the source bytes unchanged when recompression wouldn't beat it
/// (`RecompressResult::NoOp`), preserving the no-size-regression invariant.
///
/// `zenjpeg::recompress` targets absolute zensim-A vs the original and floor-clamps
/// to the source, so [`QualityTarget::Absolute`] is exact; a
/// [`Relative`](QualityTarget::Relative) target is driven at the same numeric level
/// (re-quantizing the same coefficients in place).
#[cfg(feature = "jpeg")]
fn recompress_jpeg_to_jpeg(data: &[u8], quality: QualityTarget) -> Result<TranscodeOutput> {
    use zenjpeg::recompress::{RecompressOptions, recompress};
    let result = recompress(data, &RecompressOptions::new(quality.score())).map_err(|e| {
        at!(CodecError::InvalidInput(alloc::format!(
            "jpeg recompress: {e}"
        )))
    })?;
    let bytes = result
        .output_bytes()
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| data.to_vec());
    Ok(TranscodeOutput {
        data: bytes,
        format: ImageFormat::Jpeg,
        mime_type: ImageFormat::Jpeg.mime_type(),
    })
}

/// JPEG→JXL: hand zenjxl's coefficient-domain recompressor a **zensim Profile-A**
/// scorer and let it hit `quality`, with the lossless transcode as the floor.
/// Uses the existing [`zenjxl::jpeg_lossy`] system — no decode→re-encode here.
/// JPEG is opaque, so RGB8 scoring is exact.
///
/// The [`QualityTarget`] maps onto zenjxl's native target:
/// [`Absolute`](QualityTarget::Absolute) → `Inferred` (predict the source floor vs
/// the original; if the source quality can't be read, fall back to a relative
/// target at the same level); [`Relative`](QualityTarget::Relative) → `Relative`
/// (vs the source's own decoded pixels).
#[cfg(all(feature = "transcode-iqa", feature = "jxl-decode"))]
fn recompress_jpeg_to_jxl(data: &[u8], quality: QualityTarget) -> Result<TranscodeOutput> {
    use zenjxl::jpeg_lossy::{
        InferredMetric, JpegRecompressMethod, QualityTarget as JxlTarget,
        recompress_jpeg_lossy_target,
    };
    use zensim::{Zensim, ZensimProfile};

    // RelativeScorer = Fn(ref_rgb8, dist_rgb8, w, h) -> f32 over the packed RGB8
    // buffers zenjxl decodes internally. Higher zensim-A = better.
    // Profile A is deprecated in zensim 0.3, but the thresholds this loop
    // targets were calibrated on it; moving to B is a recalibration.
    #[allow(deprecated)]
    let metric = Zensim::new(ZensimProfile::A);
    let scorer = move |r: &[u8], d: &[u8], w: u32, h: u32| -> f32 {
        let (pw, ph) = (w as usize, h as usize);
        let rs = zensim::RgbSlice::new(bytemuck::cast_slice(r), pw, ph);
        let ds = zensim::RgbSlice::new(bytemuck::cast_slice(d), pw, ph);
        metric
            .compute(&rs, &ds)
            .map(|x| x.score() as f32)
            .unwrap_or(0.0)
    };

    let jxl_target = match quality {
        // Absolute: aim vs the original via the source's inferred floor. If the
        // source quality can't be read (adaptive-quant / jpegli sources), fall
        // back to the same level taken relative to the source's own pixels.
        QualityTarget::Absolute(t) => {
            JxlTarget::inferred_preliminary(data, InferredMetric::ZensimA, t).unwrap_or(
                JxlTarget::Relative {
                    level: t,
                    higher_is_better: true,
                },
            )
        }
        QualityTarget::Relative(t) => JxlTarget::Relative {
            level: t,
            higher_is_better: true,
        },
    };
    let result =
        recompress_jpeg_lossy_target(data, JpegRecompressMethod::Auto, jxl_target, &scorer, 7);
    // Auto = min(coarsen, re-encode); never larger/worse than the lossless floor.
    let bytes = result.map_err(|e| {
        at!(CodecError::InvalidInput(alloc::format!(
            "jpeg→jxl recompress: {e}"
        )))
    })?;
    Ok(TranscodeOutput {
        data: bytes,
        format: ImageFormat::Jxl,
        mime_type: ImageFormat::Jxl.mime_type(),
    })
}

// ═══════════════════════════════════════════════════════════════════════
// Lossless byte-exact JPEG↔JXL transcode (JBRD / brunsli-parity)
// ═══════════════════════════════════════════════════════════════════════

/// Losslessly transcode a baseline/extended JPEG to JPEG XL so the **exact
/// original JPEG bitstream** can later be recovered with
/// [`reconstruct_jpeg_from_jxl`] (the brunsli-equivalent JBRD path). The JXL is
/// typically ~20% smaller than the source JPEG and carries the reconstruction
/// data; no pixels are re-encoded and no quality is lost.
///
/// `effort` (1–10) trades encode time for output size; it does **not** affect the
/// reconstructed bytes — the round-trip is byte-exact at every effort.
///
/// Returns [`CodecError::InvalidInput`] for JPEGs outside the JBRD boundary (CMYK,
/// arithmetic coding, 12-bit, chroma sampling factors > 2). For those, fall back
/// to a lossy route ([`transcode_to_quality`]) or keep the source JPEG.
///
/// Requires the `jpeg-jxl-transcode` feature.
#[cfg(feature = "jpeg-jxl-transcode")]
pub fn transcode_jpeg_to_jxl_lossless(jpeg: &[u8], effort: u8) -> Result<TranscodeOutput> {
    use zenjxl::LosslessConfig;
    let data = LosslessConfig::new()
        .with_effort(effort)
        .encode_jpeg_transcode(jpeg)
        .map_err(|e| {
            at!(CodecError::InvalidInput(alloc::format!(
                "jpeg→jxl lossless transcode: {e}"
            )))
        })?;
    Ok(TranscodeOutput {
        data,
        format: ImageFormat::Jxl,
        mime_type: ImageFormat::Jxl.mime_type(),
    })
}

/// Reconstruct the **byte-exact original JPEG** from a JPEG XL produced by a
/// lossless JPEG transcode (a JBRD reconstruction box is present) — the inverse
/// of [`transcode_jpeg_to_jxl_lossless`]. Pure Rust; no external `djxl`.
///
/// - `Ok(Some(jpeg))` — the JXL carried reconstruction data; the bytes are the
///   exact original JPEG.
/// - `Ok(None)` — a valid JXL that is **not** a JPEG transcode (no JBRD box), so
///   there is no original JPEG to recover. Decode it as pixels instead (e.g. via
///   [`DecodeRequest`](crate::DecodeRequest)).
///
/// Requires the `jxl-jpeg-reconstruct` feature (decode-side only — independent of
/// the JPEG/JXL encoders).
#[cfg(feature = "jxl-jpeg-reconstruct")]
pub fn reconstruct_jpeg_from_jxl(jxl: &[u8]) -> Result<Option<Vec<u8>>> {
    zenjxl_decoder::reconstruct_jpeg(jxl).map_err(|e| {
        at!(CodecError::InvalidInput(alloc::format!(
            "jxl→jpeg reconstruct: {e}"
        )))
    })
}

// ═══════════════════════════════════════════════════════════════════════
// TranscodeSink — streaming decode→encode bridge
// ═══════════════════════════════════════════════════════════════════════

/// Streaming transcode sink: forwards decoded strips to an encoder.
///
/// Created via [`TranscodeSink::new`] with a [`StreamingEncoder`] from
/// [`EncodeRequest::build_streaming_encoder`].
///
/// On a decoder, encoder, or writer error after bytes have been sent, abort
/// the transport and discard the partial file. Never call `finish_encode`
/// after `push_decode` fails. A failure while forwarding a strip, or a writer
/// failure, releases the encoder and writer; a transport borrowed by the writer
/// must be aborted by its owner.
///
/// Native encoder errors retain their typed causes through declaration, row
/// delivery and finalization. Inspect them with [`zencodec::find_cause`] rather
/// than parsing their messages. A buffered encoder may observe cancellation
/// at finalization instead of while accepting a strip; row input alone does
/// not promise immediate output or a cancellation-latency bound.
///
/// [`StreamingEncoder`]: crate::dispatch::StreamingEncoder
/// [`EncodeRequest::build_streaming_encoder`]: crate::EncodeRequest::build_streaming_encoder
///
/// # Example
///
/// ```rust,ignore
/// // Build the encoder
/// let se = EncodeRequest::new(ImageFormat::Jpeg)
///     .with_quality(85.0)
///     .build_streaming_encoder(width, height)?;
///
/// // Create sink and decode through it
/// let mut sink = TranscodeSink::new(se.encoder, se.supported);
/// DecodeRequest::new(data).push_decode(&mut sink)?;
///
/// // Finalize
/// let output = sink.finish_encode()?;
/// ```
pub struct TranscodeSink<'a> {
    encoder: Option<Box<dyn DynEncoder + 'a>>,
    supported: &'static [PixelFormat],
    /// What the encoder's `begin` reported (set when the decoder declared
    /// the image).
    timing: Option<zencodec::encode::OutputTiming>,
    /// Receives encoded bytes as soon as the encoder has them.
    writer: Option<TranscodeWriter<'a>>,
    /// Scratch buffer for receiving decoded rows from the decoder.
    /// The decoder writes into this via `provide_next_buffer`, and
    /// we forward it to the encoder on the *next* call (or on finish).
    strip_buf: Vec<u8>,
    /// Metadata for the pending (written but not yet forwarded) strip.
    pending: Option<PendingStrip>,
}

/// Receives encoded bytes as they become available (see
/// [`TranscodeSink::with_writer`]).
pub type TranscodeWriter<'a> =
    Box<dyn FnMut(&[u8]) -> core::result::Result<(), SinkError> + Send + 'a>;

/// Metadata for a strip that the decoder has written but we haven't
/// forwarded to the encoder yet.
struct PendingStrip {
    width: u32,
    height: u32,
    format: PixelFormat,
}

impl<'a> TranscodeSink<'a> {
    /// Create a new streaming transcode sink.
    ///
    /// `encoder` — the `DynEncoder` to push strips into.
    /// `supported` — the encoder's supported pixel formats
    ///   (from `EncoderConfig::supported_pixel_formats()`).
    pub fn new(encoder: Box<dyn DynEncoder + 'a>, supported: &'static [PixelFormat]) -> Self {
        Self {
            encoder: Some(encoder),
            supported,
            timing: None,
            writer: None,
            strip_buf: Vec::new(),
            pending: None,
        }
    }

    /// Send encoded bytes to `writer` as soon as the encoder has them: after
    /// every strip and, at [`finish_encode`](Self::finish_encode), the rest
    /// (which then returns an empty output). The decoder's `begin` declares
    /// the image to the encoder, so an encoder whose
    /// [`OutputTiming`](zencodec::encode::OutputTiming) streams writes its
    /// header before the first row.
    pub fn with_writer(
        mut self,
        writer: impl FnMut(&[u8]) -> core::result::Result<(), SinkError> + Send + 'a,
    ) -> Self {
        self.writer = Some(Box::new(writer));
        self
    }

    /// What the encoder's `begin` reported, once the decoder has declared
    /// the image.
    pub fn output_timing(&self) -> Option<zencodec::encode::OutputTiming> {
        self.timing
    }

    /// Encoded bytes ready to send (see [`DynEncoder::pending_output`]);
    /// always empty with a writer attached, which takes them at once.
    pub fn pending_output(&self) -> &[u8] {
        self.encoder.as_ref().map_or(&[], |e| e.pending_output())
    }

    /// Mark the first `n` pending bytes as sent.
    pub fn consume_output(&mut self, n: usize) {
        if let Some(e) = self.encoder.as_mut() {
            e.consume_output(n);
        }
    }

    /// Append every pending byte to `into` and mark it sent (see
    /// [`DynEncoder::take_output`]): with `into` empty, encoders that keep
    /// their output in an `OutputBuffer` hand over the buffer itself.
    pub fn take_pending_output(&mut self, into: &mut Vec<u8>) {
        if let Some(e) = self.encoder.as_mut() {
            e.take_output(into);
        }
    }

    /// Hand every pending byte to the writer, if there is one.
    fn drain(&mut self) -> core::result::Result<(), SinkError> {
        let result =
            if let (Some(writer), Some(encoder)) = (self.writer.as_mut(), self.encoder.as_mut()) {
                let pending = encoder.pending_output();
                if !pending.is_empty() {
                    let n = pending.len();
                    writer(pending).map(|()| encoder.consume_output(n))
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            };
        if result.is_err() {
            self.abort();
        }
        result
    }

    fn abort(&mut self) {
        self.encoder = None;
        self.writer = None;
        self.pending = None;
    }

    /// Finalize encoding and return the output.
    ///
    /// Must be called after `push_decode` completes (which calls
    /// `DecodeRowSink::finish` internally). Consumes the encoder
    /// via `DynEncoder::finish()`.
    pub fn finish_encode(
        mut self,
    ) -> core::result::Result<EncodeOutput, Box<dyn core::error::Error + Send + Sync>> {
        let encoder =
            self.encoder
                .take()
                .ok_or_else(|| -> Box<dyn core::error::Error + Send + Sync> {
                    "encoder already finished".into()
                })?;
        let mut output = encoder.finish()?;
        if let Some(writer) = self.writer.as_mut() {
            writer(output.data())?;
            // The bytes went to the writer; keep what describes them.
            output = EncodeOutput::new(Vec::new(), output.format())
                .with_mime_type(output.mime_type())
                .with_extension(output.extension());
        }
        Ok(output)
    }

    /// Forward the pending strip (if any) to the encoder.
    fn flush_pending(&mut self) -> core::result::Result<(), SinkError> {
        if self.encoder.is_none() {
            return Err("encoder already finished or failed".into());
        }
        let result = self.flush_pending_inner();
        if result.is_err() {
            self.abort();
        }
        result
    }

    fn flush_pending_inner(&mut self) -> core::result::Result<(), SinkError> {
        let pending = match self.pending.take() {
            Some(p) => p,
            None => return Ok(()),
        };

        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| -> SinkError { "encoder already finished".into() })?;

        let stride = (pending.width as usize)
            .checked_mul(pending.format.layout.bytes_per_pixel())
            .ok_or_else(|| -> SinkError { "strip stride overflow".into() })?;
        let data_len = stride
            .checked_mul(pending.height as usize)
            .ok_or_else(|| -> SinkError { "strip byte length overflow".into() })?;
        let strip_data = &self.strip_buf[..data_len];
        let view = PixelView::new(
            strip_data,
            pending.width,
            pending.height,
            stride,
            &pending.format,
        )
        .map_err(|e| -> SinkError { alloc::format!("pixel view: {e}").into() })?;

        // Adapt pixel format per-strip — zero-copy when format already matches
        let view = crate::dispatch::normalize_unknown_for_encode(view, self.supported);
        let (_, adapted) = zenpixels_convert::adapt_for_encode(
            view,
            &zenpixels::ColorOrigin::assumed(),
            self.supported,
            &zenpixels_convert::ConvertPolicy::permissive(),
            zenpixels::PeakSource::Declared,
            zenpixels_convert::OutputSignaling::Strip,
        )
        .map_err(|e| -> SinkError { alloc::format!("adapt: {e}").into() })?;

        encoder.push_rows(adapted.pixels.as_view())?;
        self.drain()
    }
}

impl DecodeRowSink for TranscodeSink<'_> {
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
    ) -> core::result::Result<(), SinkError> {
        if self.timing.is_some() || self.pending.is_some() {
            return Err("begin after an earlier declaration or rows".into());
        }
        self.pending = None;
        self.strip_buf.clear();
        // Declare the image to the encoder in the format the strips will
        // arrive in after adaptation: adapt one pixel through the same
        // function the strips go through, so the two cannot disagree.
        let one = alloc::vec![0u8; pixel_format.layout.bytes_per_pixel()];
        let one_view = PixelView::new(&one, 1, 1, one.len(), &pixel_format)
            .map_err(|e| -> SinkError { alloc::format!("pixel view: {e}").into() })?;
        let one_view = crate::dispatch::normalize_unknown_for_encode(one_view, self.supported);
        let (_, adapted) = zenpixels_convert::adapt_for_encode(
            one_view,
            &zenpixels::ColorOrigin::assumed(),
            self.supported,
            &zenpixels_convert::ConvertPolicy::permissive(),
            zenpixels::PeakSource::Declared,
            zenpixels_convert::OutputSignaling::Strip,
        )
        .map_err(|e| -> SinkError { alloc::format!("adapt: {e}").into() })?;
        let target = adapted.pixels.as_view().format().clone();
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| -> SinkError { "encoder already finished".into() })?;
        let timing = encoder.begin(width, height, target)?;
        self.timing = Some(timing);
        self.drain()
    }

    fn provide_next_buffer(
        &mut self,
        strip: RowStrip,
        pixel_format: PixelFormat,
    ) -> core::result::Result<PixelViewMut<'_>, SinkError> {
        // The previous buffer (if any) has been fully written by the decoder.
        // Forward it to the encoder before providing the next buffer.
        self.flush_pending()?;

        let width = strip.width;
        let height = strip.height;
        let stride = (width as usize)
            .checked_mul(pixel_format.layout.bytes_per_pixel())
            .ok_or_else(|| -> SinkError { "strip stride overflow".into() })?;
        let needed = stride
            .checked_mul(height as usize)
            .ok_or_else(|| -> SinkError { "strip byte length overflow".into() })?;

        // Resize strip_buf for this strip
        self.strip_buf.resize(needed, 0);
        let pending = self.pending.insert(PendingStrip {
            width,
            height,
            format: pixel_format,
        });

        PixelViewMut::new(
            &mut self.strip_buf[..needed],
            width,
            height,
            stride,
            &pending.format,
        )
        .map_err(|e| -> SinkError { alloc::format!("pixel view: {e}").into() })
    }

    fn finish(&mut self) -> core::result::Result<(), SinkError> {
        // Forward the last strip
        self.flush_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcode_sink_construction() {
        // Verify the type compiles and basic construction works.
        // Full integration requires a real encoder, tested in integration tests.
        assert!(core::mem::size_of::<TranscodeSink<'_>>() > 0);
    }

    #[test]
    fn supplement_set_operations() {
        let set = SupplementSet::GAIN_MAP | SupplementSet::DEPTH_MAP;
        assert!(set.contains(SupplementSet::GAIN_MAP));
        assert!(set.contains(SupplementSet::DEPTH_MAP));
        assert!(!set.contains(SupplementSet::THUMBNAIL));

        let empty = SupplementSet(0);
        assert!(empty.is_empty());
        assert!(!set.is_empty());

        assert_eq!(
            SupplementSet::GAIN_MAP.union(SupplementSet::THUMBNAIL),
            SupplementSet::GAIN_MAP | SupplementSet::THUMBNAIL
        );
    }

    #[test]
    fn transcode_options_default() {
        let opts = TranscodeOptions::default();
        assert!(opts.metadata.is_none());
        assert!(opts.matte.is_none());
        assert!(matches!(opts.supplements, SupplementPolicy::Preserve));
        // Verbatim by default — no implicit privacy choice.
        assert!(matches!(
            opts.metadata_policy,
            MetadataPolicy::PreserveExact
        ));
    }

    #[cfg(feature = "jxl-jpeg-reconstruct")]
    #[test]
    fn reconstruct_rejects_non_transcode_input() {
        // Garbage / non-JXL input must not panic: it returns an error or Ok(None),
        // never bogus JPEG bytes.
        let r = reconstruct_jpeg_from_jxl(&[0xFF, 0x0A, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05]);
        assert!(matches!(r, Err(_) | Ok(None)));
    }

    #[cfg(feature = "jpeg-jxl-transcode")]
    #[test]
    fn jpeg_jxl_lossless_roundtrips_or_rejects() {
        // Brunsli-equivalent contract: a lossless JPEG→JXL transcode either cleanly
        // rejects (outside the JBRD boundary) or reconstructs the ORIGINAL JPEG
        // byte-for-byte. Never silent corruption.
        let jpeg: &[u8] = include_bytes!("../tests/images/ultrahdr_sample.jpg");
        // Outside the JBRD boundary (multi-image / unsupported sampling, etc.) a
        // clean rejection is contract-correct — never a corrupt result — so only
        // the Ok arm carries assertions.
        if let Ok(out) = transcode_jpeg_to_jxl_lossless(jpeg, 7) {
            assert_eq!(out.format, ImageFormat::Jxl);
            let recon = reconstruct_jpeg_from_jxl(&out.data)
                .expect("reconstruct must not error on our own transcode")
                .expect("our lossless transcode embeds a JBRD reconstruction box");
            assert_eq!(
                recon, jpeg,
                "lossless JPEG→JXL→JPEG must reconstruct byte-for-byte"
            );
        }
    }

    /// Round-trip: encode a tiny JPEG, transcode to WebP, verify output.
    #[cfg(all(feature = "jpeg", feature = "webp"))]
    #[test]
    fn transcode_jpeg_to_webp() {
        use crate::quality::QualityIntent;

        // Create a small test image
        let img = imgref::ImgVec::new(
            alloc::vec![
                rgb::Rgb {
                    r: 128u8,
                    g: 64,
                    b: 32
                };
                10 * 10
            ],
            10,
            10,
        );

        // Encode to JPEG first
        let jpeg_output = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(75.0)
            .encode(PixelView::from_imgref(img.as_ref(), zenpixels::ColorSpace::SRGB).unwrap())
            .unwrap();
        assert!(!jpeg_output.data().is_empty());

        // Now transcode JPEG → WebP
        let decision = FormatDecision {
            format: ImageFormat::WebP,
            quality: QualityIntent::from_quality(80.0),
            lossless: false,
            hints: Default::default(),
            matte: None,
            trace: alloc::vec::Vec::new(),
        };

        let output = transcode(
            jpeg_output.data(),
            &decision,
            &TranscodeOptions::default(),
            &AllowedFormats::all(),
        )
        .unwrap();

        assert_eq!(output.format, ImageFormat::WebP);
        assert_eq!(output.mime_type, "image/webp");
        assert!(!output.data.is_empty());

        // Verify we can decode the transcoded output
        let decoded = crate::DecodeRequest::new(&output.data)
            .decode_full_frame()
            .unwrap();
        assert_eq!(decoded.width(), 10);
        assert_eq!(decoded.height(), 10);
    }

    /// JPEG→JXL via zenjxl's `jpeg_lossy` coefficient-domain recompressor with a
    /// zensim-A target. Exercises the existing system end-to-end and verifies a
    /// decodable JXL at the source dimensions — no zencodecs-side re-encode loop.
    #[cfg(all(feature = "transcode-iqa", feature = "jpeg", feature = "jxl-decode"))]
    #[test]
    fn transcode_to_quality_jpeg_to_jxl() {
        // Gradient + XOR high-frequency content so the source JPEG has real
        // structure for the recompressor to act on.
        let (w, h) = (64usize, 64usize);
        let mut px = alloc::vec::Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                px.push(rgb::Rgb {
                    r: (x * 4) as u8,
                    g: (y * 4) as u8,
                    b: ((x ^ y) * 4) as u8,
                });
            }
        }
        let img = imgref::ImgVec::new(px, w, h);
        let jpeg = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(90.0)
            .encode(PixelView::from_imgref(img.as_ref(), zenpixels::ColorSpace::SRGB).unwrap())
            .unwrap();

        let out = transcode_to_quality(
            jpeg.data(),
            ImageFormat::Jxl,
            QualityTarget::Absolute(85.0),
            &TranscodeOptions::default(),
            &AllowedFormats::all(),
        )
        .unwrap();
        assert_eq!(out.format, ImageFormat::Jxl);
        assert!(!out.data.is_empty());

        let decoded = crate::DecodeRequest::new(&out.data)
            .decode_full_frame()
            .unwrap();
        assert_eq!(decoded.width() as usize, w);
        assert_eq!(decoded.height() as usize, h);
    }

    /// Bug #7 regression: the coefficient-domain fast paths in
    /// `transcode_to_quality` used to bypass `AllowedFormats` entirely (no
    /// `DecodeRequest`/`EncodeRequest` in the loop to enforce it for free).
    /// A registry that disables JPEG must reject the JPEG→JPEG recompress
    /// fast path, not silently recompress anyway.
    #[cfg(feature = "jpeg")]
    #[test]
    fn transcode_to_quality_respects_disabled_registry() {
        let img = imgref::ImgVec::new(
            alloc::vec![
                rgb::Rgb {
                    r: 200u8,
                    g: 100,
                    b: 50
                };
                8 * 8
            ],
            8,
            8,
        );
        let jpeg = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(90.0)
            .encode(PixelView::from_imgref(img.as_ref(), zenpixels::ColorSpace::SRGB).unwrap())
            .unwrap();

        let result = transcode_to_quality(
            jpeg.data(),
            ImageFormat::Jpeg,
            QualityTarget::Absolute(70.0),
            &TranscodeOptions::default(),
            &AllowedFormats::none(),
        );

        assert!(
            matches!(
                result.as_ref().map_err(|e| e.error()),
                Err(CodecError::DisabledFormat(_))
            ),
            "AllowedFormats::none() must reject the JPEG->JPEG coefficient-domain \
             fast path, not silently recompress: {result:?}"
        );
    }

    /// Round-trip: encode a tiny image, transcode keeping the same format.
    #[cfg(feature = "jpeg")]
    #[test]
    fn transcode_jpeg_to_jpeg() {
        use crate::quality::QualityIntent;

        let img = imgref::ImgVec::new(
            alloc::vec![
                rgb::Rgb {
                    r: 200u8,
                    g: 100,
                    b: 50
                };
                8 * 8
            ],
            8,
            8,
        );

        let jpeg_output = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(90.0)
            .encode(PixelView::from_imgref(img.as_ref(), zenpixels::ColorSpace::SRGB).unwrap())
            .unwrap();

        let decision = FormatDecision {
            format: ImageFormat::Jpeg,
            quality: QualityIntent::from_quality(70.0),
            lossless: false,
            hints: Default::default(),
            matte: None,
            trace: alloc::vec::Vec::new(),
        };

        let output = transcode(
            jpeg_output.data(),
            &decision,
            &TranscodeOptions::default(),
            &AllowedFormats::all(),
        )
        .unwrap();

        assert_eq!(output.format, ImageFormat::Jpeg);
        assert_eq!(output.mime_type, "image/jpeg");
        assert!(!output.data.is_empty());

        // Lower quality should produce fewer bytes
        assert!(
            output.data.len() <= jpeg_output.data().len(),
            "q70 ({}) should be <= q90 ({})",
            output.data.len(),
            jpeg_output.data().len(),
        );
    }

    /// Verify probe() returns correct info for a JPEG.
    #[cfg(feature = "jpeg")]
    #[test]
    fn probe_jpeg_returns_correct_info() {
        let img = imgref::ImgVec::new(
            alloc::vec![
                rgb::Rgb {
                    r: 100u8,
                    g: 150,
                    b: 200
                };
                16 * 12
            ],
            16,
            12,
        );
        let jpeg_output = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(80.0)
            .encode(PixelView::from_imgref(img.as_ref(), zenpixels::ColorSpace::SRGB).unwrap())
            .unwrap();

        let info = crate::probe(jpeg_output.data(), &AllowedFormats::all()).unwrap();
        assert_eq!(info.width, 16);
        assert_eq!(info.height, 12);
        assert_eq!(info.format, ImageFormat::Jpeg);
        assert!(!info.has_alpha);
    }

    /// Verify probe() returns correct info for a PNG.
    #[cfg(feature = "png")]
    #[test]
    fn probe_png_returns_correct_info() {
        // Create a minimal PNG via the png crate
        let mut buf = alloc::vec::Vec::new();
        let mut encoder = png::Encoder::new(&mut buf, 20, 15);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&alloc::vec![128u8; 20 * 15 * 4])
            .unwrap();
        writer.finish().unwrap();

        let info = crate::probe(&buf, &AllowedFormats::all()).unwrap();
        assert_eq!(info.width, 20);
        assert_eq!(info.height, 15);
        assert_eq!(info.format, ImageFormat::Png);
        assert!(info.has_alpha);
    }
}
