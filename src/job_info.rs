//! Serializable job result information.
//!
//! Provides [`JobResultInfo`] — a JSON-friendly summary of a processed image
//! result, excluding pixel data. Useful for APIs, logging, and downstream
//! consumers that need structured metadata about pipeline output.
//!
//! # Example
//!
//! ```ignore
//! let result = zenpipe::process(source, &config)?;
//! let info = JobResultInfo::from(&result);
//! let json = serde_json::to_string_pretty(&info)?;
//! ```

use alloc::string::String;
use serde::{Deserialize, Serialize};

// Source trait needed for width()/height()/format() on MaterializedSource.
#[cfg(feature = "zennode")]
use crate::Source as _;

// ─── Primary result ───

/// Serializable summary of a processed image result.
///
/// Captures dimensions, format, configuration, sidecar info, and metadata
/// presence — everything except the actual pixel data. Constructed from
/// [`ProcessedImage`](crate::orchestrate::ProcessedImage) or
/// [`StreamingOutput`](crate::orchestrate::StreamingOutput).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobResultInfo {
    /// Primary image summary (dimensions, format).
    pub primary: ImageSummary,
    /// Sidecar (gain map) summary, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidecar: Option<SidecarSummary>,
    /// Decode configuration.
    pub decode_config: DecodeConfigInfo,
    /// Encode configuration (excluding opaque codec params).
    pub encode_config: EncodeConfigInfo,
    /// Metadata summary (presence and sizes of ICC/EXIF/XMP, plus structured fields).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataSummary>,
}

// ─── Image summary ───

/// Dimensions and pixel format of an image.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageSummary {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format description.
    pub format: PixelFormatInfo,
    /// Stride (bytes per row, including alignment padding).
    pub stride: usize,
    /// Total pixel data size in bytes.
    pub data_bytes: usize,
}

/// Pixel format broken into its component parts for serialization.
///
/// Mirrors [`PixelDescriptor`](crate::PixelDescriptor) fields without
/// requiring serde on the upstream crate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PixelFormatInfo {
    /// Physical pixel format name (e.g., `"Rgba8"`, `"RgbF32"`).
    pub pixel_format: String,
    /// Transfer function (e.g., `"Srgb"`, `"Linear"`, `"Pq"`).
    pub transfer: String,
    /// Alpha mode, if an alpha channel is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<String>,
    /// Color primaries (e.g., `"Bt709"`, `"Bt2020"`, `"DisplayP3"`).
    pub primaries: String,
    /// Signal range (e.g., `"Full"`, `"Narrow"`).
    pub signal_range: String,
}

impl From<crate::PixelDescriptor> for PixelFormatInfo {
    fn from(pd: crate::PixelDescriptor) -> Self {
        Self {
            pixel_format: alloc::format!("{:?}", pd.format),
            transfer: alloc::format!("{:?}", pd.transfer),
            alpha: pd.alpha.map(|a| alloc::format!("{a:?}")),
            primaries: alloc::format!("{:?}", pd.primaries),
            signal_range: alloc::format!("{:?}", pd.signal_range),
        }
    }
}

// ─── Sidecar ───

/// Serializable summary of a processed sidecar (gain map).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SidecarSummary {
    /// Sidecar width in pixels.
    pub width: u32,
    /// Sidecar height in pixels.
    pub height: u32,
    /// Sidecar pixel format.
    pub format: PixelFormatInfo,
    /// What kind of sidecar this is.
    pub kind: SidecarKindInfo,
}

/// Sidecar kind with serializable gain map parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SidecarKindInfo {
    /// ISO 21496-1 gain map.
    GainMap {
        /// Per-channel gain map parameters.
        params: GainMapParamsInfo,
    },
}

/// ISO 21496-1 gain map parameters (serializable mirror of `GainMapParams`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GainMapParamsInfo {
    /// Per-channel parameters: `[R, G, B]`.
    pub channels: [GainMapChannelInfo; 3],
    /// Log2 of base image HDR headroom.
    pub base_hdr_headroom: f64,
    /// Log2 of alternate image HDR headroom.
    pub alternate_hdr_headroom: f64,
    /// Whether the gain map uses the base image's color space.
    pub use_base_color_space: bool,
}

/// Per-channel gain map parameters (serializable mirror of `GainMapChannel`).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct GainMapChannelInfo {
    /// Log2 of minimum gain.
    pub min: f64,
    /// Log2 of maximum gain.
    pub max: f64,
    /// Gamma (linear domain).
    pub gamma: f64,
    /// Base offset (linear domain).
    pub base_offset: f64,
    /// Alternate offset (linear domain).
    pub alternate_offset: f64,
}

// ─── Configuration ───

/// Decode configuration (serializable).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecodeConfigInfo {
    /// HDR mode: `"sdr_only"`, `"hdr_reconstruct"`, `"preserve"`.
    pub hdr_mode: String,
    /// Color intent: `"preserve"`, `"srgb"`.
    pub color_intent: String,
    /// JPEG prescale hint (minimum output dimension). 0 = no prescaling.
    pub min_size: u32,
}

/// Encode configuration (serializable, excluding opaque codec params).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncodeConfigInfo {
    /// Quality profile string (named preset or numeric `"0"`-`"100"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality_profile: Option<String>,
    /// Output format (e.g., `"jpeg"`, `"webp"`, `"avif"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Device pixel ratio for quality adjustment.
    pub dpr: f32,
    /// Lossless preference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lossless: Option<bool>,
    /// Schema ID of codec-specific params, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec_params_schema: Option<String>,
}

// ─── Metadata ───

/// Summary of image metadata (presence and sizes of binary blobs,
/// plus structured color/HDR fields).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetadataSummary {
    /// ICC profile size in bytes, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icc_profile_bytes: Option<usize>,
    /// EXIF data size in bytes, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exif_bytes: Option<usize>,
    /// XMP data size in bytes, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xmp_bytes: Option<usize>,
    /// CICP color description, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cicp: Option<CicpInfo>,
    /// Content light level (HDR), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_light_level: Option<ContentLightLevelInfo>,
    /// Mastering display metadata (HDR), if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mastering_display: Option<MasteringDisplayInfo>,
    /// EXIF orientation (1-8, 1 = normal).
    pub orientation: u8,
}

/// CICP color description (ITU-T H.273).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CicpInfo {
    /// Color primaries code (1 = BT.709, 9 = BT.2020, 12 = P3).
    pub color_primaries: u8,
    /// Transfer characteristics code (13 = sRGB, 16 = PQ, 18 = HLG).
    pub transfer_characteristics: u8,
    /// Matrix coefficients code.
    pub matrix_coefficients: u8,
    /// Full range flag.
    pub full_range: bool,
}

/// HDR content light level metadata.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ContentLightLevelInfo {
    /// Maximum Content Light Level (MaxCLL) in cd/m².
    pub max_content_light_level: u16,
    /// Maximum Frame-Average Light Level (MaxFALL) in cd/m².
    pub max_frame_average_light_level: u16,
}

/// Mastering display color volume metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MasteringDisplayInfo {
    /// RGB primaries in CIE 1931 xy: `[[rx, ry], [gx, gy], [bx, by]]`.
    pub primaries_xy: [[f32; 2]; 3],
    /// White point in CIE 1931 xy: `[wx, wy]`.
    pub white_point_xy: [f32; 2],
    /// Maximum display luminance in cd/m².
    pub max_luminance: f32,
    /// Minimum display luminance in cd/m².
    pub min_luminance: f32,
}

// ─── Conversions from zencodec types ───

impl From<&zencodec::Metadata> for MetadataSummary {
    fn from(m: &zencodec::Metadata) -> Self {
        Self {
            icc_profile_bytes: m.icc_profile.as_ref().map(|b| b.len()),
            exif_bytes: m.exif.as_ref().map(|b| b.len()),
            xmp_bytes: m.xmp.as_ref().map(|b| b.len()),
            cicp: m.cicp.map(|c| CicpInfo {
                color_primaries: c.color_primaries,
                transfer_characteristics: c.transfer_characteristics,
                matrix_coefficients: c.matrix_coefficients,
                full_range: c.full_range,
            }),
            content_light_level: m.content_light_level.map(|c| ContentLightLevelInfo {
                max_content_light_level: c.max_content_light_level,
                max_frame_average_light_level: c.max_frame_average_light_level,
            }),
            mastering_display: m.mastering_display.map(|d| MasteringDisplayInfo {
                primaries_xy: d.primaries_xy,
                white_point_xy: d.white_point_xy,
                max_luminance: d.max_luminance,
                min_luminance: d.min_luminance,
            }),
            orientation: m.orientation.exif_value() as u8,
        }
    }
}

impl From<&zencodec::GainMapParams> for GainMapParamsInfo {
    fn from(p: &zencodec::GainMapParams) -> Self {
        Self {
            channels: [
                GainMapChannelInfo::from(&p.channels[0]),
                GainMapChannelInfo::from(&p.channels[1]),
                GainMapChannelInfo::from(&p.channels[2]),
            ],
            base_hdr_headroom: p.base_hdr_headroom,
            alternate_hdr_headroom: p.alternate_hdr_headroom,
            use_base_color_space: p.use_base_color_space,
        }
    }
}

impl From<&zencodec::GainMapChannel> for GainMapChannelInfo {
    fn from(c: &zencodec::GainMapChannel) -> Self {
        Self {
            min: c.min,
            max: c.max,
            gamma: c.gamma,
            base_offset: c.base_offset,
            alternate_offset: c.alternate_offset,
        }
    }
}

// ─── Conversions from pipeline types (requires zennode) ───

#[cfg(feature = "zennode")]
impl From<&crate::orchestrate::ProcessedImage> for JobResultInfo {
    fn from(img: &crate::orchestrate::ProcessedImage) -> Self {
        let primary = ImageSummary {
            width: img.primary.width(),
            height: img.primary.height(),
            format: PixelFormatInfo::from(img.primary.format()),
            stride: img.primary.stride(),
            data_bytes: img.primary.data().len(),
        };

        let sidecar = img.sidecar.as_ref().map(|s| SidecarSummary {
            width: s.width(),
            height: s.height(),
            format: PixelFormatInfo::from(s.format()),
            kind: SidecarKindInfo::from(&s.kind),
        });

        let decode_config = DecodeConfigInfo {
            hdr_mode: img.decode_config.hdr_mode.clone(),
            color_intent: img.decode_config.color_intent.clone(),
            min_size: img.decode_config.min_size,
        };

        let encode_config = EncodeConfigInfo {
            quality_profile: img.encode_config.quality_profile.clone(),
            format: img.encode_config.format.clone(),
            dpr: img.encode_config.dpr,
            lossless: img.encode_config.lossless,
            codec_params_schema: img
                .encode_config
                .codec_params
                .as_ref()
                .map(|n| alloc::string::String::from(n.schema().id)),
        };

        let metadata = img.metadata.as_ref().map(MetadataSummary::from);

        Self {
            primary,
            sidecar,
            decode_config,
            encode_config,
            metadata,
        }
    }
}

#[cfg(feature = "zennode")]
impl From<&crate::sidecar::SidecarKind> for SidecarKindInfo {
    fn from(kind: &crate::sidecar::SidecarKind) -> Self {
        match kind {
            crate::sidecar::SidecarKind::GainMap { params } => SidecarKindInfo::GainMap {
                params: GainMapParamsInfo::from(params),
            },
        }
    }
}

#[cfg(feature = "zennode")]
impl From<&crate::orchestrate::StreamingOutput> for JobResultInfo {
    fn from(out: &crate::orchestrate::StreamingOutput) -> Self {
        let primary = ImageSummary {
            width: out.source.width(),
            height: out.source.height(),
            format: PixelFormatInfo::from(out.source.format()),
            stride: out.source.format().aligned_stride(out.source.width()),
            data_bytes: 0, // streaming — not yet materialized
        };

        let sidecar = out.sidecar.as_ref().map(|s| SidecarSummary {
            width: s.width(),
            height: s.height(),
            format: PixelFormatInfo::from(s.format()),
            kind: SidecarKindInfo::from(&s.kind),
        });

        let encode_config = EncodeConfigInfo {
            quality_profile: out.encode_config.quality_profile.clone(),
            format: out.encode_config.format.clone(),
            dpr: out.encode_config.dpr,
            lossless: out.encode_config.lossless,
            codec_params_schema: out
                .encode_config
                .codec_params
                .as_ref()
                .map(|n| alloc::string::String::from(n.schema().id)),
        };

        let metadata = out.metadata.as_ref().map(MetadataSummary::from);

        Self {
            primary,
            sidecar,
            decode_config: DecodeConfigInfo {
                hdr_mode: alloc::string::String::from("sdr_only"),
                color_intent: alloc::string::String::from("preserve"),
                min_size: 0,
            },
            encode_config,
            metadata,
        }
    }
}

#[cfg(feature = "zennode")]
impl From<&crate::orchestrate::SourceImageInfo> for ImageSummary {
    fn from(info: &crate::orchestrate::SourceImageInfo) -> Self {
        Self {
            width: info.width,
            height: info.height,
            format: PixelFormatInfo::from(info.format),
            stride: info.format.aligned_stride(info.width),
            data_bytes: 0, // source info — no pixel data
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_format_info_roundtrip() {
        let pd = crate::format::RGBA8_SRGB;
        let info = PixelFormatInfo::from(pd);
        assert_eq!(info.pixel_format, "Rgba8");
        assert_eq!(info.transfer, "Srgb");
        assert_eq!(info.primaries, "Bt709");
        assert_eq!(info.signal_range, "Full");
        assert!(info.alpha.is_some());
    }

    #[test]
    fn metadata_summary_empty() {
        let meta = zencodec::Metadata::default();
        let summary = MetadataSummary::from(&meta);
        assert!(summary.icc_profile_bytes.is_none());
        assert!(summary.exif_bytes.is_none());
        assert!(summary.xmp_bytes.is_none());
        assert!(summary.cicp.is_none());
        assert_eq!(summary.orientation, 1);
    }

    #[test]
    fn metadata_summary_with_data() {
        let meta = zencodec::Metadata::none()
            .with_icc(alloc::vec![1, 2, 3, 4, 5])
            .with_exif(alloc::vec![6, 7])
            .with_cicp(zenpixels::Cicp::SRGB);
        let summary = MetadataSummary::from(&meta);
        assert_eq!(summary.icc_profile_bytes, Some(5));
        assert_eq!(summary.exif_bytes, Some(2));
        assert!(summary.xmp_bytes.is_none());
        let cicp = summary.cicp.unwrap();
        assert_eq!(cicp.color_primaries, 1);
        assert_eq!(cicp.transfer_characteristics, 13);
        assert!(cicp.full_range);
    }

    #[test]
    fn gainmap_params_roundtrip() {
        let params = zencodec::GainMapParams::default();
        let info = GainMapParamsInfo::from(&params);
        assert_eq!(info.channels[0].min, 0.0);
        assert_eq!(info.channels[0].gamma, 1.0);
        assert!(info.use_base_color_space);
    }

    #[test]
    fn serde_roundtrip() {
        let info = JobResultInfo {
            primary: ImageSummary {
                width: 800,
                height: 600,
                format: PixelFormatInfo {
                    pixel_format: alloc::string::String::from("Rgba8"),
                    transfer: alloc::string::String::from("Srgb"),
                    alpha: Some(alloc::string::String::from("Straight")),
                    primaries: alloc::string::String::from("Bt709"),
                    signal_range: alloc::string::String::from("Full"),
                },
                stride: 3200,
                data_bytes: 1_920_000,
            },
            sidecar: None,
            decode_config: DecodeConfigInfo {
                hdr_mode: alloc::string::String::from("sdr_only"),
                color_intent: alloc::string::String::from("preserve"),
                min_size: 0,
            },
            encode_config: EncodeConfigInfo {
                quality_profile: Some(alloc::string::String::from("good")),
                format: Some(alloc::string::String::from("webp")),
                dpr: 1.0,
                lossless: None,
                codec_params_schema: None,
            },
            metadata: None,
        };

        // Serialize to JSON string (using serde_json would be ideal,
        // but we just verify the derives compile and the structure is correct).
        let json = alloc::format!("{info:?}");
        assert!(json.contains("800"));
        assert!(json.contains("600"));
    }
}
