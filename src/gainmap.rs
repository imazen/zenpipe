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

#[cfg(feature = "jpeg-ultrahdr")]
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

/// Convert [`GainMapParams`] (log2 domain) → [`GainMapMetadata`] (linear domain).
///
/// Applies `2^x` to gains and headroom fields. Gamma and offsets pass through directly.
#[cfg(feature = "jpeg-ultrahdr")]
pub fn params_to_metadata(p: &GainMapParams) -> GainMapMetadata {
    GainMapMetadata {
        min_content_boost: [
            2.0f32.powf(p.channels[0].min as f32),
            2.0f32.powf(p.channels[1].min as f32),
            2.0f32.powf(p.channels[2].min as f32),
        ],
        max_content_boost: [
            2.0f32.powf(p.channels[0].max as f32),
            2.0f32.powf(p.channels[1].max as f32),
            2.0f32.powf(p.channels[2].max as f32),
        ],
        gamma: [
            p.channels[0].gamma as f32,
            p.channels[1].gamma as f32,
            p.channels[2].gamma as f32,
        ],
        offset_sdr: [
            p.channels[0].base_offset as f32,
            p.channels[1].base_offset as f32,
            p.channels[2].base_offset as f32,
        ],
        offset_hdr: [
            p.channels[0].alternate_offset as f32,
            p.channels[1].alternate_offset as f32,
            p.channels[2].alternate_offset as f32,
        ],
        hdr_capacity_min: 2.0f32.powf(p.base_hdr_headroom as f32),
        hdr_capacity_max: 2.0f32.powf(p.alternate_hdr_headroom as f32),
        use_base_color_space: p.use_base_color_space,
    }
}

/// Convert [`GainMapMetadata`] (linear domain) → [`GainMapParams`] (log2 domain).
///
/// Applies `log2(x)` to gains and headroom fields. Gamma and offsets pass through directly.
#[cfg(feature = "jpeg-ultrahdr")]
pub fn metadata_to_params(m: &GainMapMetadata) -> GainMapParams {
    let mut channels = [GainMapChannel::default(); 3];
    for i in 0..3 {
        channels[i].min = (m.min_content_boost[i] as f64).log2();
        channels[i].max = (m.max_content_boost[i] as f64).log2();
        channels[i].gamma = m.gamma[i] as f64;
        channels[i].base_offset = m.offset_sdr[i] as f64;
        channels[i].alternate_offset = m.offset_hdr[i] as f64;
    }
    let mut params = GainMapParams::default();
    params.channels = channels;
    params.base_hdr_headroom = (m.hdr_capacity_min as f64).log2();
    params.alternate_hdr_headroom = (m.hdr_capacity_max as f64).log2();
    params.use_base_color_space = m.use_base_color_space;
    params
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
    /// Convert the stored linear-domain metadata to the canonical
    /// log2-domain [`GainMapParams`].
    pub fn params(&self) -> GainMapParams {
        metadata_to_params(&self.metadata)
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

#[cfg(test)]
mod tests {
    use super::*;

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
            metadata: GainMapMetadata {
                max_content_boost: [4.0; 3],
                min_content_boost: [1.0; 3],
                gamma: [1.0; 3],
                offset_sdr: [1.0 / 64.0; 3],
                offset_hdr: [1.0 / 64.0; 3],
                hdr_capacity_min: 1.0,
                hdr_capacity_max: 4.0,
                use_base_color_space: true,
            },
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
        let meta = GainMapMetadata {
            max_content_boost: [4.0; 3],
            min_content_boost: [1.0; 3],
            gamma: [1.0; 3],
            offset_sdr: [1.0 / 64.0; 3],
            offset_hdr: [1.0 / 64.0; 3],
            hdr_capacity_min: 1.0,
            hdr_capacity_max: 4.0,
            use_base_color_space: true,
        };
        let source = GainMapSource::Precomputed {
            gain_map: &img,
            metadata: &meta,
        };
        match source {
            GainMapSource::Precomputed { gain_map, metadata } => {
                assert_eq!(gain_map.width, 8);
                assert_eq!(gain_map.height, 8);
                assert_eq!(gain_map.channels, 1);
                assert_eq!(metadata.max_content_boost[0], 4.0);
            }
        }
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn params_metadata_roundtrip() {
        let meta = GainMapMetadata {
            max_content_boost: [4.0; 3],
            min_content_boost: [1.0; 3],
            gamma: [1.0; 3],
            offset_sdr: [1.0 / 64.0; 3],
            offset_hdr: [1.0 / 64.0; 3],
            hdr_capacity_min: 1.0,
            hdr_capacity_max: 4.0,
            use_base_color_space: true,
        };
        let params = metadata_to_params(&meta);
        let meta2 = params_to_metadata(&params);
        for i in 0..3 {
            assert!((meta.max_content_boost[i] - meta2.max_content_boost[i]).abs() < 0.01);
            assert!((meta.min_content_boost[i] - meta2.min_content_boost[i]).abs() < 0.01);
            assert!((meta.gamma[i] - meta2.gamma[i]).abs() < 0.01);
        }
    }
}
