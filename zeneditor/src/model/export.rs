//! Export model — format, quality, dimensions, HDR handling, metadata, color space.
//!
//! SPEC.md §3 (export system), §3.5 (advanced preservation panel), §17 (color pipeline).

use serde::{Deserialize, Serialize};

/// Export settings for encoding the edited image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportModel {
    /// Target format (e.g. "jpeg", "webp", "png", "gif", "jxl", "avif").
    pub format: String,
    /// Export width (0 = source width).
    pub width: u32,
    /// Export height (0 = source height).
    pub height: u32,
    /// Format-specific options (quality, effort, lossless, near-lossless, subsampling, etc.).
    pub options: serde_json::Value,
    /// HDR / gain map handling (§3.5, §17.4).
    pub hdr_mode: HdrMode,
    /// What metadata to preserve in output (§3.5, §17.6).
    pub metadata_policy: MetadataPolicy,
    /// Output color space (§17.2, §17.7).
    pub colorspace: ColorspaceTarget,
    /// Transfer function — relevant when colorspace is Rec2020 (§17.2, §17.7).
    pub transfer_function: TransferFunction,
    /// Gamut handling for out-of-gamut colors (§17.7).
    pub gamut_handling: GamutHandling,
    /// Output bit depth (0 = auto / match source). §17.5.
    pub bit_depth: u8,
}

/// How to handle HDR content and gain maps.
///
/// SPEC.md §3.5: gain map options, §17.4: gain map pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HdrMode {
    /// Keep gain map if present, pass through HDR metadata.
    #[default]
    Preserve,
    /// Discard gain map, output SDR only.
    Strip,
    /// Apply gain map to produce HDR output.
    Tonemap,
    /// Reconstruct a gain map from an HDR source.
    Reconstruct,
}

/// What metadata to preserve in the encoded output.
///
/// SPEC.md §3.5: per-metadata-type checkboxes, all on by default.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataPolicy {
    /// Keep all metadata (default — no data loss unless explicit).
    #[default]
    PreserveAll,
    /// Strip all metadata.
    StripAll,
    /// Strip only specific kinds.
    Strip(Vec<MetadataKind>),
    /// Keep only specific kinds (strip everything else).
    Keep(Vec<MetadataKind>),
}

/// Individual metadata categories for selective preservation.
///
/// SPEC.md §3.5: EXIF, XMP, ICC, CICP checkboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataKind {
    Exif,
    Icc,
    Xmp,
    Cicp,
    GainMap,
}

/// Output color space target.
///
/// SPEC.md §17.2, §17.7: color space dropdown in export panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorspaceTarget {
    /// Match source color space (no unnecessary conversion).
    #[default]
    Auto,
    /// sRGB — widest compatibility, web default.
    Srgb,
    /// Display P3 — Apple ecosystem, wide gamut SDR.
    DisplayP3,
    /// Rec. 2020 — HDR workflows, broadcast. Requires TransferFunction selection.
    Rec2020,
}

/// Transfer function for HDR output (§17.2, §17.7).
///
/// Only meaningful when colorspace is Rec2020. Grayed out otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferFunction {
    /// Match source transfer function.
    #[default]
    Auto,
    /// sRGB gamma (~2.2). Standard for SDR.
    Srgb,
    /// Perceptual Quantizer (SMPTE ST 2084). HDR10.
    Pq,
    /// Hybrid Log-Gamma. HLG broadcast.
    Hlg,
}

/// How to handle out-of-gamut colors during color space conversion (§17.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GamutHandling {
    /// Perceptual — compress entire gamut to fit, preserves relationships.
    #[default]
    Perceptual,
    /// Relative colorimetric — clip out-of-gamut, preserve in-gamut accuracy.
    RelativeColorimetric,
    /// Absolute colorimetric — preserve exact colors, clip out-of-gamut.
    AbsoluteColorimetric,
}

impl Default for ExportModel {
    fn default() -> Self {
        Self {
            format: "jpeg".to_string(),
            width: 0,
            height: 0,
            options: serde_json::json!({"quality": 85}),
            hdr_mode: HdrMode::default(),
            metadata_policy: MetadataPolicy::default(),
            colorspace: ColorspaceTarget::default(),
            transfer_function: TransferFunction::default(),
            gamut_handling: GamutHandling::default(),
            bit_depth: 0,
        }
    }
}

impl ExportModel {
    /// The max dimension to pass to the pipeline (largest of width/height, or 0 for source size).
    pub fn max_dim(&self) -> u32 {
        self.width.max(self.height)
    }

    /// Set a single option by key.
    pub fn set_option(&mut self, key: &str, value: serde_json::Value) {
        if let Some(obj) = self.options.as_object_mut() {
            obj.insert(key.to_string(), value);
        }
    }

    /// Get the quality value (if set).
    pub fn quality(&self) -> Option<f32> {
        self.options
            .get("quality")
            .and_then(|v| v.as_f64())
            .map(|q| q as f32)
    }

    /// Get the effort value (if set).
    pub fn effort(&self) -> Option<u32> {
        self.options
            .get("effort")
            .and_then(|v| v.as_u64())
            .map(|e| e as u32)
    }

    /// Whether lossless encoding is requested.
    pub fn lossless(&self) -> bool {
        self.options
            .get("lossless")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    /// Whether a specific metadata kind should be preserved.
    pub fn should_preserve(&self, kind: MetadataKind) -> bool {
        match &self.metadata_policy {
            MetadataPolicy::PreserveAll => true,
            MetadataPolicy::StripAll => false,
            MetadataPolicy::Strip(kinds) => !kinds.contains(&kind),
            MetadataPolicy::Keep(kinds) => kinds.contains(&kind),
        }
    }
}
