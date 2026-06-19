//! Format-agnostic gain map types and orchestration.
//!
//! Gain maps enable backward-compatible HDR in image files: a base image
//! (SDR or HDR) plus a secondary gain map image that maps between SDR and HDR
//! renditions. The gain map metadata follows ISO 21496-1, which is used by
//! JPEG (UltraHDR), AVIF (tmap), and JXL (jhgm).
//!
//! # Direction
//!
//! The gain map direction varies by format:
//! - **JPEG/AVIF**: base=SDR, gain map maps SDR→HDR (forward)
//! - **JXL**: base=HDR, gain map maps HDR→SDR (inverse)
//!
//! The [`DecodedGainMap::base_is_hdr`] flag tracks this so callers can
//! determine the correct reconstruction direction.
//!
//! # Gain map image codec
//!
//! The gain map image is encoded with the same codec as the base image
//! (JPEG in JPEG, AV1 in AVIF, JXL in JXL). Decoding and encoding the
//! gain map image is handled internally by the format-specific adapters.
//!
//! # Reconstruction
//!
//! To reconstruct HDR from an SDR base + gain map, use
//! [`ultrahdr_core::apply_gainmap()`] (re-exported via
//! [`zenjpeg::ultrahdr::apply_gainmap`]). That function provides LUT-optimized,
//! streaming-capable reconstruction — far better than reimplementing the math
//! in this crate.

use crate::ImageFormat;

// Re-export the ISO 21496-1 metadata type from ultrahdr-core (via zenjpeg).
#[cfg(feature = "jpeg-ultrahdr")]
pub use zenjpeg::ultrahdr::GainMapMetadata;

// Re-export the gain map pixel type from ultrahdr-core (via zenjpeg).
// This replaces the old `GainMapImage` type that was a duplicate.
#[cfg(feature = "jpeg-ultrahdr")]
pub use zenjpeg::ultrahdr::GainMap;

// Re-export zencodec gain map types.
pub use zencodec::gainmap::{GainMapChannel, GainMapParams, GainMapPresence};

/// Transcode a gain-map source (HEIC / Ultra-HDR JPEG) to a BT.2100 **PQ**
/// (ST 2084) PNG carrying `cICP` + `cLLI`.
///
/// Decodes the source's HDR reconstruction (linear float — full reconstruction
/// at the gain map's encoded maximum unless `target_headroom` caps it),
/// quantizes to `RGB16_BT2100_PQ` (the absolute-luminance anchor — BT.2408,
/// 203 nits — travels with the pixels via the buffer `ColorContext`), then
/// PNG-encodes with the resolved primaries, the PQ transfer, and the measured
/// content light level.
///
/// Returns `Ok(None)` when the source carries no gain map (nothing to
/// reconstruct — the caller should fall back to a plain SDR transcode).
///
/// Requires `png` plus a gain-map-capable decoder (`jpeg-ultrahdr` and/or
/// `heic-decode`). This is the rendition step `hdr-corpus-convert` needs from
/// the library so the tool can collapse to a CLI invocation (zenpipe#68).
#[cfg(all(
    feature = "png",
    any(feature = "jpeg-ultrahdr", feature = "heic-decode")
))]
pub fn transcode_to_hdr_pq_png(
    data: &[u8],
    registry: &crate::AllowedFormats,
    target_headroom: Option<f32>,
) -> crate::error::Result<Option<alloc::vec::Vec<u8>>> {
    use crate::CodecError;
    use whereat::{ResultAtExt, at};
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    use zencodec::{Cicp, ContentLightLevel, OrientationHint};
    use zenpixels::PixelDescriptor;

    // Decode the SDR base first: gates on gain-map presence (avoids a wasted
    // reconstruct on non-HDR input) and resolves the container primaries.
    let base = crate::DecodeRequest::new(data)
        .with_registry(registry)
        .decode()?;
    if !base.info().supplements.gain_map {
        return Ok(None);
    }
    // Resolved CICP primaries (1 BT.709/sRGB, 9 BT.2020, 12 Display P3),
    // defaulting to sRGB when unsignaled.
    let primaries = match base.info().source_color.cicp.map(|c| c.color_primaries) {
        Some(9) => 9,
        Some(12) => 12,
        _ => 1,
    };

    // Reconstruct HDR (linear float), display-oriented — PNG can't carry an
    // orientation tag, so the rotation must be baked into the pixels.
    let hdr = crate::DecodeRequest::new(data)
        .with_registry(registry)
        .with_orientation(OrientationHint::Correct)
        .reconstruct_hdr(target_headroom)
        .decode()?;

    // PQ (ST 2084) quantize. The diffuse-white anchor travels with the pixels
    // (`ColorContext.diffuse_white`), set by the reconstruction.
    let pq = zenpixels_convert::hdr::quantize_to(hdr.pixels(), PixelDescriptor::RGB16_BT2100_PQ)
        .map_err(|e| at!(CodecError::InvalidInput(alloc::format!("PQ quantize: {e}"))))?;

    // Content light level: per-pixel literal max (CTA-861.3-A stills). `measure`
    // returns None for non-float buffers (already filtered above by reconstruct).
    #[allow(deprecated)]
    let cll = ContentLightLevel::measure(hdr.pixels(), zenpixels::hdr::DiffuseWhite::BT2408);

    // PNG with cICP (resolved primaries + PQ transfer 16) and cLLI.
    let png = zenpng::PngEncoderConfig::new()
        .with_cicp(Some(Cicp::new(primaries, 16, 0, true)))
        .with_content_light_level(cll)
        .job()
        .encoder()
        .map_err_at(|e| CodecError::from_codec(crate::ImageFormat::Png, e))?
        .encode(pq.as_slice())
        .map_err_at(|e| CodecError::from_codec(crate::ImageFormat::Png, e))?
        .data()
        .to_vec();
    Ok(Some(png))
}

/// Gain map extracted from a decoded image.
///
/// Format-agnostic: works for JPEG (UltraHDR), AVIF (tmap), and JXL (jhgm).
/// The gain map image has already been decoded from the container's embedded
/// format — `gain_map` contains raw pixel data.
///
/// # Reconstruction
///
/// To reconstruct the alternate rendition, use the `gain_map` and `metadata`
/// fields directly with [`zenjpeg::ultrahdr::apply_gainmap()`]:
///
/// ```ignore
/// use zenjpeg::ultrahdr::{apply_gainmap, HdrOutputFormat, Unstoppable};
///
/// let hdr = apply_gainmap(&sdr_image, &decoded.gain_map, &decoded.metadata,
///     display_boost, HdrOutputFormat::LinearFloat, Unstoppable)?;
/// ```
#[derive(Clone, Debug)]
#[cfg(feature = "jpeg-ultrahdr")]
pub struct DecodedGainMap {
    /// The decoded gain map image pixels (grayscale or RGB u8).
    ///
    /// This is the `ultrahdr_core::GainMap` type — pass it directly to
    /// `apply_gainmap()` for HDR reconstruction.
    pub gain_map: GainMap,

    /// ISO 21496-1 gain map metadata describing how to apply the map.
    pub metadata: GainMapMetadata,

    /// Whether the base image is HDR.
    ///
    /// - `false` (JPEG/AVIF): base=SDR, gain map maps SDR→HDR
    /// - `true` (JXL): base=HDR, gain map maps HDR→SDR
    pub base_is_hdr: bool,

    /// Source format this gain map was extracted from.
    pub source_format: ImageFormat,
}

/// Source of gain map data for encoding.
///
/// When encoding an image with a gain map, you can either provide a
/// pre-computed gain map (for passthrough/transcode) or have the encoder
/// compute one from HDR source pixels.
#[cfg(feature = "jpeg-ultrahdr")]
pub enum GainMapSource<'a> {
    /// Pre-computed gain map (for passthrough/transcode).
    ///
    /// The encoder embeds this directly without recomputation. Useful when
    /// transcoding between formats or re-encoding with edits that don't
    /// affect the HDR mapping.
    Precomputed {
        /// The gain map image pixels.
        gain_map: &'a GainMap,
        /// ISO 21496-1 metadata describing the mapping.
        metadata: &'a GainMapMetadata,
    },
}

#[cfg(feature = "jpeg-ultrahdr")]
impl DecodedGainMap {
    /// The stored ISO 21496-1 parameters. [`GainMapMetadata`] is an alias for
    /// the canonical [`GainMapParams`] since ultrahdr-core 0.5.
    pub fn params(&self) -> GainMapParams {
        self.metadata.clone()
    }

    /// Build a [`GainMapInfo`](zencodec::GainMapInfo) describing this gain map
    /// (metadata + dimensions, no pixel data).
    pub fn to_gain_map_info(&self) -> zencodec::GainMapInfo {
        zencodec::GainMapInfo::new(
            self.params(),
            self.gain_map.width,
            self.gain_map.height,
            self.gain_map.channels,
        )
    }
}

// =========================================================================
// Unified gain map source decode
// =========================================================================

/// Decode a gain map from its raw encoded source.
///
/// Handles format-specific decode internally:
/// - **JPEG**: Complete JPEG file (MPF secondary image) — decoded via [`DecodeRequest`].
/// - **JXL**: Bare JXL codestream — decoded via [`DecodeRequest`] (zenjxl handles bare codestreams).
/// - **AVIF**: Raw AV1 OBUs (not a valid AVIF container) — decoded via `zenavif::decode_av1_obu`.
///
/// Enforces a recursion limit: `depth >= 1` is rejected to prevent gain maps
/// that themselves contain gain maps from causing unbounded recursion.
///
/// # Errors
///
/// Returns [`CodecError::InvalidInput`] if the recursion depth is exceeded.
/// Returns [`CodecError::UnsupportedFormat`] if the gain map format is not
/// compiled in or not supported for direct decode.
/// Format-specific codec errors are wrapped in [`CodecError::Codec`].
///
/// [`DecodeRequest`]: crate::DecodeRequest
pub fn decode_gain_map_source(
    source: &zencodec::gainmap::GainMapSource,
    limits: Option<&crate::Limits>,
    stop: Option<crate::StopToken>,
    registry: &crate::AllowedFormats,
) -> crate::error::Result<zencodec::gainmap::DecodedGainMap> {
    use alloc::string::ToString as _;
    use whereat::at;

    if source.depth >= 1 {
        return Err(at!(crate::CodecError::InvalidInput(
            "gain map recursion depth exceeded".to_string()
        )));
    }

    match source.format {
        // AVIF gain maps are raw AV1 OBUs, not a valid AVIF container.
        // DecodeRequest with format=Avif would try to parse an AVIF container
        // and fail, so we use the direct AV1 OBU decoder instead.
        #[cfg(feature = "avif-decode")]
        ImageFormat::Avif => decode_gain_map_av1_obu(source),

        #[cfg(not(feature = "avif-decode"))]
        ImageFormat::Avif => Err(at!(crate::CodecError::UnsupportedFormat(ImageFormat::Avif))),

        // JXL bare codestreams and JPEG complete files both work through
        // the standard DecodeRequest path.
        format => {
            let mut request = crate::DecodeRequest::new(&source.data)
                .with_format(format)
                .with_registry(registry);

            if let Some(lim) = limits {
                request = request.with_limits(lim);
            }
            if let Some(st) = stop {
                request = request.with_stop(st);
            }

            let output = request.decode_full_frame()?;

            Ok(zencodec::gainmap::DecodedGainMap::new(
                output.into_buffer(),
                source.metadata.clone(),
            ))
        }
    }
}

/// Decode raw AV1 OBUs into a gain map pixel buffer.
///
/// AVIF gain maps store raw AV1 OBUs rather than a complete AVIF container,
/// so we use `zenavif::decode_av1_obu` directly instead of the standard
/// decode pipeline.
#[cfg(feature = "avif-decode")]
fn decode_gain_map_av1_obu(
    source: &zencodec::gainmap::GainMapSource,
) -> crate::error::Result<zencodec::gainmap::DecodedGainMap> {
    use whereat::at;

    let (pixel_data, width, height, channels) = zenavif::decode_av1_obu(&source.data)
        .map_err(|e| at!(crate::CodecError::from_codec(ImageFormat::Avif, e)))?;

    // Build a PixelBuffer from the raw decoded bytes.
    let descriptor = match channels {
        1 => zenpixels::PixelDescriptor::GRAY8,
        3 => zenpixels::PixelDescriptor::RGB8_SRGB,
        4 => zenpixels::PixelDescriptor::RGBA8_SRGB,
        _ => {
            return Err(at!(crate::CodecError::InvalidInput(alloc::format!(
                "unexpected AV1 gain map channel count: {channels}"
            ))));
        }
    };

    let buffer =
        zenpixels::PixelBuffer::from_vec(pixel_data, width, height, descriptor).map_err(|_| {
            at!(crate::CodecError::InvalidInput(
                "failed to create PixelBuffer from AV1 gain map decode".into()
            ))
        })?;

    Ok(zencodec::gainmap::DecodedGainMap::new(
        buffer,
        source.metadata.clone(),
    ))
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    /// Build ISO 21496-1 params the way a JPEG UltraHDR decode would
    /// (SDR base, 2-stop alternate headroom). `GainMapParams` is
    /// `#[non_exhaustive]`, so construction goes through `default()`.
    #[cfg(feature = "jpeg-ultrahdr")]
    fn test_metadata() -> GainMapMetadata {
        let mut m = GainMapMetadata::default();
        m.channels = [GainMapChannel {
            min: 0.0,
            max: 2.0,
            gamma: 1.0,
            base_offset: 1.0 / 64.0,
            alternate_offset: 1.0 / 64.0,
        }; 3];
        m.base_hdr_headroom = 0.0;
        m.alternate_hdr_headroom = 2.0;
        m.use_base_color_space = true;
        m
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn decoded_gainmap_jpeg_sdr_base() {
        let gm = DecodedGainMap {
            gain_map: GainMap {
                data: alloc::vec![128; 4],
                width: 2,
                height: 2,
                channels: 1,
            },
            metadata: test_metadata(),
            base_is_hdr: false,
            source_format: ImageFormat::Jpeg,
        };
        assert!(!gm.base_is_hdr);
        assert_eq!(gm.source_format, ImageFormat::Jpeg);
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn decoded_gainmap_jxl_hdr_base() {
        let gm = DecodedGainMap {
            gain_map: GainMap {
                data: alloc::vec![128; 4],
                width: 2,
                height: 2,
                channels: 1,
            },
            metadata: GainMapMetadata::default(),
            base_is_hdr: true,
            source_format: ImageFormat::Jxl,
        };
        assert!(gm.base_is_hdr);
        assert_eq!(gm.source_format, ImageFormat::Jxl);
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn gainmap_source_precomputed() {
        let img = GainMap {
            data: alloc::vec![200; 8 * 8],
            width: 8,
            height: 8,
            channels: 1,
        };
        let meta = test_metadata();
        let source = GainMapSource::Precomputed {
            gain_map: &img,
            metadata: &meta,
        };
        match source {
            GainMapSource::Precomputed { gain_map, metadata } => {
                assert_eq!(gain_map.width, 8);
                assert_eq!(gain_map.height, 8);
                assert_eq!(gain_map.channels, 1);
                assert_eq!(metadata.channels[0].max, 2.0);
            }
        }
    }
}
