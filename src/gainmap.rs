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
//! use [`DecodedGainMap::reconstruct_alternate`] without worrying about direction.
//!
//! # Gain map image codec
//!
//! The gain map image is encoded with the same codec as the base image
//! (JPEG in JPEG, AV1 in AVIF, JXL in JXL). Decoding and encoding the
//! gain map image is handled internally by the format-specific adapters.

use alloc::vec::Vec;

use crate::{CodecError, ImageFormat};

// Re-export the ISO 21496-1 metadata type from ultrahdr-core (via zenjpeg).
#[cfg(feature = "jpeg-ultrahdr")]
pub use zenjpeg::ultrahdr::GainMapMetadata;

/// Gain map extracted from a decoded image.
///
/// Format-agnostic: works for JPEG (UltraHDR), AVIF (tmap), and JXL (jhgm).
/// The gain map image has already been decoded from the container's embedded
/// format — `gain_map` contains raw pixel data.
#[derive(Clone, Debug)]
#[cfg(feature = "jpeg-ultrahdr")]
pub struct DecodedGainMap {
    /// The decoded gain map image pixels (grayscale or RGB u8).
    pub gain_map: GainMapImage,

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

/// Raw pixel data for a gain map image.
///
/// Gain maps are typically lower resolution than the base image.
/// They can be single-channel (luminance-only) or 3-channel (per-channel RGB).
#[derive(Clone, Debug)]
pub struct GainMapImage {
    /// Raw pixel bytes (u8 values, grayscale or RGB).
    pub data: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Number of channels: 1 (luminance) or 3 (per-channel RGB).
    pub channels: u8,
}

impl GainMapImage {
    /// Total number of pixels in the gain map.
    pub fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// Total byte length (should equal `data.len()` for valid images).
    pub fn expected_len(&self) -> u64 {
        self.pixel_count() * self.channels as u64
    }

    /// Validate that the data length matches dimensions and channel count.
    pub fn validate(&self) -> core::result::Result<(), CodecError> {
        if self.width == 0 || self.height == 0 {
            return Err(CodecError::InvalidInput(alloc::format!(
                "gain map has zero dimensions: {}x{}",
                self.width, self.height,
            )));
        }
        if self.channels != 1 && self.channels != 3 {
            return Err(CodecError::InvalidInput(alloc::format!(
                "gain map channels must be 1 or 3, got {}",
                self.channels,
            )));
        }
        let expected = self.expected_len();
        if self.data.len() as u64 != expected {
            return Err(CodecError::InvalidInput(alloc::format!(
                "gain map data length {} does not match {}x{}x{} = {}",
                self.data.len(),
                self.width,
                self.height,
                self.channels,
                expected,
            )));
        }
        Ok(())
    }
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
        gain_map: &'a GainMapImage,
        /// ISO 21496-1 metadata describing the mapping.
        metadata: &'a GainMapMetadata,
    },
}

#[cfg(feature = "jpeg-ultrahdr")]
impl DecodedGainMap {
    /// Reconstruct the HDR rendition from an SDR base image + gain map.
    ///
    /// Only valid when `base_is_hdr == false` (JPEG/AVIF direction).
    /// The base pixels must be sRGB u8 RGB (3 bytes/pixel) or RGBA (4 bytes/pixel).
    ///
    /// Returns linear f32 RGBA pixels (4 floats per pixel).
    ///
    /// `display_boost` controls HDR headroom: 1.0 = SDR, 4.0 = typical HDR.
    pub fn reconstruct_hdr(
        &self,
        base_sdr_pixels: &[u8],
        width: u32,
        height: u32,
        channels: u8,
        display_boost: f32,
    ) -> crate::error::Result<Vec<u8>> {
        use zenjpeg::ultrahdr::{
            HdrOutputFormat, UhdrColorGamut, UhdrColorTransfer, UhdrPixelFormat, UhdrRawImage,
            apply_gainmap,
        };
        use whereat::at;

        if self.base_is_hdr {
            return Err(at!(CodecError::InvalidInput(
                "reconstruct_hdr called but base is already HDR — use reconstruct_sdr".into(),
            )));
        }

        let pixel_format = match channels {
            3 => UhdrPixelFormat::Rgb8,
            4 => UhdrPixelFormat::Rgba8,
            _ => {
                return Err(at!(CodecError::InvalidInput(alloc::format!(
                    "reconstruct_hdr expects 3 or 4 channel base, got {channels}",
                ))));
            }
        };

        let sdr = UhdrRawImage::from_data(
            width,
            height,
            pixel_format,
            UhdrColorGamut::Bt709,
            UhdrColorTransfer::Srgb,
            base_sdr_pixels.to_vec(),
        )
        .map_err(|e| at!(CodecError::from_codec(self.source_format, e)))?;

        // Convert our GainMapImage to the GainMap type expected by apply_gainmap
        let core_gainmap = zenjpeg::ultrahdr::GainMap {
            width: self.gain_map.width,
            height: self.gain_map.height,
            channels: self.gain_map.channels,
            data: self.gain_map.data.clone(),
        };

        let hdr_result = apply_gainmap(
            &sdr,
            &core_gainmap,
            &self.metadata,
            display_boost,
            HdrOutputFormat::LinearFloat,
            zenjpeg::ultrahdr::Unstoppable,
        )
        .map_err(|e| at!(CodecError::from_codec(self.source_format, e)))?;

        Ok(hdr_result.data)
    }

    /// Reconstruct the SDR rendition from an HDR base image + gain map.
    ///
    /// Only valid when `base_is_hdr == true` (JXL direction).
    ///
    /// This is a placeholder — JXL gain map decode is not yet implemented.
    pub fn reconstruct_sdr(
        &self,
        _base_hdr_pixels: &[u8],
        _width: u32,
        _height: u32,
    ) -> crate::error::Result<Vec<u8>> {
        use whereat::at;

        if !self.base_is_hdr {
            return Err(at!(CodecError::InvalidInput(
                "reconstruct_sdr called but base is SDR — use reconstruct_hdr".into(),
            )));
        }

        // JXL inverse gain map application not yet implemented.
        Err(at!(CodecError::UnsupportedOperation {
            format: self.source_format,
            detail: "inverse gain map (HDR→SDR) not yet implemented",
        }))
    }

    /// Reconstruct the alternate rendition.
    ///
    /// - If base is SDR, reconstructs HDR (calls [`reconstruct_hdr`](Self::reconstruct_hdr)).
    /// - If base is HDR, reconstructs SDR (calls [`reconstruct_sdr`](Self::reconstruct_sdr)).
    ///
    /// `display_boost` is only used for SDR→HDR direction (ignored for HDR→SDR).
    pub fn reconstruct_alternate(
        &self,
        base_pixels: &[u8],
        width: u32,
        height: u32,
        channels: u8,
        display_boost: f32,
    ) -> crate::error::Result<Vec<u8>> {
        if self.base_is_hdr {
            self.reconstruct_sdr(base_pixels, width, height)
        } else {
            self.reconstruct_hdr(base_pixels, width, height, channels, display_boost)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_map_image_validate_ok() {
        let img = GainMapImage {
            data: vec![128; 4 * 4],
            width: 4,
            height: 4,
            channels: 1,
        };
        assert!(img.validate().is_ok());
    }

    #[test]
    fn gain_map_image_validate_rgb() {
        let img = GainMapImage {
            data: vec![128; 4 * 4 * 3],
            width: 4,
            height: 4,
            channels: 3,
        };
        assert!(img.validate().is_ok());
    }

    #[test]
    fn gain_map_image_validate_wrong_len() {
        let img = GainMapImage {
            data: vec![128; 10],
            width: 4,
            height: 4,
            channels: 1,
        };
        let err = img.validate().unwrap_err();
        assert!(
            matches!(err, CodecError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn gain_map_image_validate_zero_dim() {
        let img = GainMapImage {
            data: vec![],
            width: 0,
            height: 4,
            channels: 1,
        };
        let err = img.validate().unwrap_err();
        assert!(matches!(err, CodecError::InvalidInput(_)));
    }

    #[test]
    fn gain_map_image_validate_bad_channels() {
        let img = GainMapImage {
            data: vec![128; 16],
            width: 4,
            height: 4,
            channels: 2,
        };
        let err = img.validate().unwrap_err();
        assert!(matches!(err, CodecError::InvalidInput(_)));
    }

    #[test]
    fn gain_map_image_pixel_count() {
        let img = GainMapImage {
            data: vec![0; 320 * 240],
            width: 320,
            height: 240,
            channels: 1,
        };
        assert_eq!(img.pixel_count(), 76800);
        assert_eq!(img.expected_len(), 76800);
    }

    #[test]
    fn gain_map_image_expected_len_rgb() {
        let img = GainMapImage {
            data: vec![0; 10 * 10 * 3],
            width: 10,
            height: 10,
            channels: 3,
        };
        assert_eq!(img.expected_len(), 300);
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn decoded_gainmap_jpeg_sdr_base() {
        let gm = DecodedGainMap {
            gain_map: GainMapImage {
                data: vec![128; 4],
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
        assert!(gm.gain_map.validate().is_ok());
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn decoded_gainmap_jxl_hdr_base() {
        let gm = DecodedGainMap {
            gain_map: GainMapImage {
                data: vec![128; 4],
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
    fn reconstruct_hdr_rejects_hdr_base() {
        let gm = DecodedGainMap {
            gain_map: GainMapImage {
                data: vec![128; 4],
                width: 2,
                height: 2,
                channels: 1,
            },
            metadata: GainMapMetadata::default(),
            base_is_hdr: true,
            source_format: ImageFormat::Jxl,
        };
        let dummy_pixels = vec![128u8; 2 * 2 * 3];
        let err = gm.reconstruct_hdr(&dummy_pixels, 2, 2, 3, 4.0).unwrap_err();
        assert!(matches!(err.error(), CodecError::InvalidInput(_)));
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn reconstruct_sdr_rejects_sdr_base() {
        let gm = DecodedGainMap {
            gain_map: GainMapImage {
                data: vec![128; 4],
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
        let dummy_pixels = vec![128u8; 2 * 2 * 3];
        let err = gm.reconstruct_sdr(&dummy_pixels, 2, 2).unwrap_err();
        assert!(matches!(err.error(), CodecError::InvalidInput(_)));
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn reconstruct_sdr_unsupported_for_jxl() {
        let gm = DecodedGainMap {
            gain_map: GainMapImage {
                data: vec![128; 4],
                width: 2,
                height: 2,
                channels: 1,
            },
            metadata: GainMapMetadata::default(),
            base_is_hdr: true,
            source_format: ImageFormat::Jxl,
        };
        let dummy_pixels = vec![128u8; 2 * 2 * 16]; // f32 RGBA
        let err = gm.reconstruct_sdr(&dummy_pixels, 2, 2).unwrap_err();
        assert!(matches!(
            err.error(),
            CodecError::UnsupportedOperation { .. }
        ));
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn reconstruct_alternate_dispatches_correctly() {
        // SDR base → should try reconstruct_hdr (which may fail on tiny data, but the
        // dispatch direction is what we're testing)
        let gm_sdr = DecodedGainMap {
            gain_map: GainMapImage {
                data: vec![128; 4],
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
        // Provide valid sRGB u8 pixels: 2x2 RGB = 12 bytes
        let sdr_pixels = vec![128u8; 2 * 2 * 3];
        // This should succeed via reconstruct_hdr → apply_gainmap
        let result = gm_sdr.reconstruct_alternate(&sdr_pixels, 2, 2, 3, 4.0);
        assert!(result.is_ok(), "reconstruct_alternate SDR→HDR failed: {result:?}");

        // HDR base → should try reconstruct_sdr (which returns UnsupportedOperation)
        let gm_hdr = DecodedGainMap {
            gain_map: GainMapImage {
                data: vec![128; 4],
                width: 2,
                height: 2,
                channels: 1,
            },
            metadata: GainMapMetadata::default(),
            base_is_hdr: true,
            source_format: ImageFormat::Jxl,
        };
        let hdr_pixels = vec![0u8; 2 * 2 * 16];
        let err = gm_hdr
            .reconstruct_alternate(&hdr_pixels, 2, 2, 4, 4.0)
            .unwrap_err();
        assert!(matches!(
            err.error(),
            CodecError::UnsupportedOperation { .. }
        ));
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    #[test]
    fn gainmap_source_precomputed() {
        let img = GainMapImage {
            data: vec![200; 8 * 8],
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
}
