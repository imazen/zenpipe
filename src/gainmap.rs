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

use crate::CodecError;
#[cfg(feature = "jpeg-ultrahdr")]
use crate::ImageFormat;

// Re-export the ISO 21496-1 metadata type from ultrahdr-core (via zenjpeg).
#[cfg(feature = "jpeg-ultrahdr")]
pub use zenjpeg::ultrahdr::GainMapMetadata;

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
                self.width,
                self.height,
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
        use whereat::at;
        use zenjpeg::ultrahdr::{
            HdrOutputFormat, UhdrColorGamut, UhdrColorTransfer, UhdrPixelFormat, UhdrRawImage,
            apply_gainmap,
        };

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
    /// The base pixels must be linear f32 RGBA (16 bytes per pixel).
    ///
    /// Returns sRGB u8 RGBA pixels (4 bytes per pixel).
    ///
    /// This applies the inverse gain map formula:
    /// `sdr = (hdr + offset_hdr) / gain - offset_sdr`
    /// where gain is derived from the gain map LUT at weight=1.0.
    pub fn reconstruct_sdr(
        &self,
        base_hdr_pixels: &[u8],
        width: u32,
        height: u32,
    ) -> crate::error::Result<Vec<u8>> {
        use linear_srgb::default::linear_to_srgb_u8;
        use whereat::at;

        if !self.base_is_hdr {
            return Err(at!(CodecError::InvalidInput(
                "reconstruct_sdr called but base is SDR — use reconstruct_hdr".into(),
            )));
        }

        self.gain_map.validate()?;

        let pixel_count = width as usize * height as usize;
        let expected_bytes = pixel_count * 16; // f32 RGBA = 16 bytes/pixel
        if base_hdr_pixels.len() < expected_bytes {
            return Err(at!(CodecError::InvalidInput(alloc::format!(
                "HDR base too small: {} bytes for {}x{} f32 RGBA (need {})",
                base_hdr_pixels.len(),
                width,
                height,
                expected_bytes,
            ))));
        }

        let gm = &self.gain_map;
        let meta = &self.metadata;

        // Precompute per-channel gain map parameters
        let gamma = meta.gamma;
        let log_min = [
            meta.min_content_boost[0].ln(),
            meta.min_content_boost[1].ln(),
            meta.min_content_boost[2].ln(),
        ];
        let log_range = [
            meta.max_content_boost[0].ln() - log_min[0],
            meta.max_content_boost[1].ln() - log_min[1],
            meta.max_content_boost[2].ln() - log_min[2],
        ];

        let mut output = alloc::vec![0u8; pixel_count * 4]; // sRGB RGBA u8

        for y in 0..height {
            for x in 0..width {
                // Read HDR linear f32 pixel
                let px_idx = (y as usize * width as usize + x as usize) * 16;
                let r = f32::from_le_bytes([
                    base_hdr_pixels[px_idx],
                    base_hdr_pixels[px_idx + 1],
                    base_hdr_pixels[px_idx + 2],
                    base_hdr_pixels[px_idx + 3],
                ]);
                let g = f32::from_le_bytes([
                    base_hdr_pixels[px_idx + 4],
                    base_hdr_pixels[px_idx + 5],
                    base_hdr_pixels[px_idx + 6],
                    base_hdr_pixels[px_idx + 7],
                ]);
                let b = f32::from_le_bytes([
                    base_hdr_pixels[px_idx + 8],
                    base_hdr_pixels[px_idx + 9],
                    base_hdr_pixels[px_idx + 10],
                    base_hdr_pixels[px_idx + 11],
                ]);

                // Sample gain map with bilinear interpolation
                let gm_x = (x as f32 / width as f32) * gm.width as f32;
                let gm_y = (y as f32 / height as f32) * gm.height as f32;
                let gx0 = (gm_x.floor() as u32).min(gm.width.saturating_sub(1));
                let gy0 = (gm_y.floor() as u32).min(gm.height.saturating_sub(1));
                let gx1 = (gx0 + 1).min(gm.width.saturating_sub(1));
                let gy1 = (gy0 + 1).min(gm.height.saturating_sub(1));
                let fx = gm_x - gm_x.floor();
                let fy = gm_y - gm_y.floor();

                // Compute gain per channel (weight=1.0 for full SDR reconstruction)
                let gain = |byte_val: u8, ch: usize| -> f32 {
                    let normalized = byte_val as f32 / 255.0;
                    let linear = if gamma[ch] != 1.0 && gamma[ch] > 0.0 {
                        normalized.powf(1.0 / gamma[ch])
                    } else {
                        normalized
                    };
                    // log_gain at weight=1.0
                    (log_min[ch] + linear * log_range[ch]).exp()
                };

                let sample_channel = |ch: usize| -> f32 {
                    if gm.channels == 1 {
                        let g00 = gain(gm.data[(gy0 * gm.width + gx0) as usize], ch);
                        let g10 = gain(gm.data[(gy0 * gm.width + gx1) as usize], ch);
                        let g01 = gain(gm.data[(gy1 * gm.width + gx0) as usize], ch);
                        let g11 = gain(gm.data[(gy1 * gm.width + gx1) as usize], ch);
                        bilinear(g00, g10, g01, g11, fx, fy)
                    } else {
                        let i00 = (gy0 * gm.width + gx0) as usize * 3 + ch;
                        let i10 = (gy0 * gm.width + gx1) as usize * 3 + ch;
                        let i01 = (gy1 * gm.width + gx0) as usize * 3 + ch;
                        let i11 = (gy1 * gm.width + gx1) as usize * 3 + ch;
                        let g00 = gain(gm.data[i00], ch);
                        let g10 = gain(gm.data[i10], ch);
                        let g01 = gain(gm.data[i01], ch);
                        let g11 = gain(gm.data[i11], ch);
                        bilinear(g00, g10, g01, g11, fx, fy)
                    }
                };

                let gain_r = sample_channel(0);
                let gain_g = sample_channel(1);
                let gain_b = sample_channel(2);

                // Inverse gain map: sdr_linear = (hdr_linear + offset_hdr) / gain - offset_sdr
                let inv_r = if gain_r > 1e-10 {
                    (r + meta.offset_hdr[0]) / gain_r - meta.offset_sdr[0]
                } else {
                    0.0
                };
                let inv_g = if gain_g > 1e-10 {
                    (g + meta.offset_hdr[1]) / gain_g - meta.offset_sdr[1]
                } else {
                    0.0
                };
                let inv_b = if gain_b > 1e-10 {
                    (b + meta.offset_hdr[2]) / gain_b - meta.offset_sdr[2]
                } else {
                    0.0
                };

                // Convert linear → sRGB u8
                let out_idx = (y as usize * width as usize + x as usize) * 4;
                output[out_idx] = linear_to_srgb_u8(inv_r.max(0.0));
                output[out_idx + 1] = linear_to_srgb_u8(inv_g.max(0.0));
                output[out_idx + 2] = linear_to_srgb_u8(inv_b.max(0.0));
                output[out_idx + 3] = 255;
            }
        }

        Ok(output)
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

/// Bilinear interpolation.
#[cfg(feature = "jpeg-ultrahdr")]
#[inline(always)]
fn bilinear(v00: f32, v10: f32, v01: f32, v11: f32, fx: f32, fy: f32) -> f32 {
    let top = v00 * (1.0 - fx) + v10 * fx;
    let bottom = v01 * (1.0 - fx) + v11 * fx;
    top * (1.0 - fy) + bottom * fy
}

#[cfg(test)]
mod tests {
    use alloc::vec;

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
    fn reconstruct_sdr_produces_valid_output() {
        let gm = DecodedGainMap {
            gain_map: GainMapImage {
                data: vec![128; 4], // mid-gray gain map
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
            base_is_hdr: true,
            source_format: ImageFormat::Jxl,
        };
        // HDR linear f32 RGBA pixels: bright content
        let mut hdr_pixels = Vec::new();
        for _ in 0..4 {
            hdr_pixels.extend_from_slice(&1.5f32.to_le_bytes()); // R
            hdr_pixels.extend_from_slice(&1.0f32.to_le_bytes()); // G
            hdr_pixels.extend_from_slice(&0.5f32.to_le_bytes()); // B
            hdr_pixels.extend_from_slice(&1.0f32.to_le_bytes()); // A
        }
        let result = gm.reconstruct_sdr(&hdr_pixels, 2, 2).unwrap();
        assert_eq!(result.len(), 2 * 2 * 4); // sRGB RGBA u8
        assert!(
            result.iter().any(|&v| v > 0),
            "SDR should have non-zero pixels"
        );
        assert_eq!(result[3], 255); // alpha
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
        assert!(
            result.is_ok(),
            "reconstruct_alternate SDR→HDR failed: {result:?}"
        );

        // HDR base → should try reconstruct_sdr (now implemented)
        let gm_hdr = DecodedGainMap {
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
            base_is_hdr: true,
            source_format: ImageFormat::Jxl,
        };
        // HDR linear f32 RGBA pixels
        let mut hdr_pixels = Vec::new();
        for _ in 0..4 {
            hdr_pixels.extend_from_slice(&1.0f32.to_le_bytes());
            hdr_pixels.extend_from_slice(&0.8f32.to_le_bytes());
            hdr_pixels.extend_from_slice(&0.5f32.to_le_bytes());
            hdr_pixels.extend_from_slice(&1.0f32.to_le_bytes());
        }
        let result = gm_hdr.reconstruct_alternate(&hdr_pixels, 2, 2, 4, 4.0);
        assert!(
            result.is_ok(),
            "reconstruct_alternate HDR→SDR failed: {result:?}"
        );
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
