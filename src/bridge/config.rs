//! Decode and encode configuration types extracted from zenode node params.

use alloc::boxed::Box;
use alloc::string::{String, ToString};

use zenode::NodeInstance;

/// Decode-time configuration extracted from the Decode node's params.
///
/// Provides convenient typed access to decode settings without requiring
/// callers to downcast the node instance or read params individually.
#[derive(Clone, Debug)]
pub struct DecodeConfig {
    /// HDR mode: `"sdr_only"`, `"hdr_reconstruct"`, `"preserve"`.
    pub hdr_mode: String,
    /// Color intent: `"preserve"`, `"srgb"`.
    pub color_intent: String,
    /// JPEG prescale hint (minimum output dimension). 0 = no prescaling.
    pub min_size: u32,
}

impl Default for DecodeConfig {
    fn default() -> Self {
        Self {
            hdr_mode: String::from("sdr_only"),
            color_intent: String::from("preserve"),
            min_size: 0,
        }
    }
}

impl DecodeConfig {
    /// Extract decode configuration from a list of decode-phase nodes.
    ///
    /// Reads params from the first node with schema ID `"zenode.decode"`.
    /// If no such node is found, returns defaults.
    pub fn from_nodes(nodes: &[Box<dyn NodeInstance>]) -> Self {
        for node in nodes {
            if node.schema().id == "zenode.decode" {
                return Self::from_node(node.as_ref());
            }
        }
        Self::default()
    }

    /// Extract decode configuration from a single decode node.
    fn from_node(node: &dyn NodeInstance) -> Self {
        let hdr_mode = node
            .get_param("hdr_mode")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| String::from("sdr_only"));

        let color_intent = node
            .get_param("color_intent")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| String::from("preserve"));

        let min_size = node
            .get_param("min_size")
            .and_then(|v| v.as_u32())
            .unwrap_or(0);

        Self {
            hdr_mode,
            color_intent,
            min_size,
        }
    }
}

/// Encode configuration extracted from encode-phase nodes.
///
/// Reads quality intent and per-codec params from the encode node list.
/// Handles both `"zenode.quality_intent"` and `"zencodecs.quality_intent"`
/// schema IDs so callers don't need to know where QualityIntent is defined.
pub struct EncodeConfig {
    /// Quality profile string (from QualityIntent node, if present).
    ///
    /// Named presets: `"lowest"`, `"low"`, `"medium_low"`, `"medium"`,
    /// `"good"`, `"high"`, `"highest"`, `"lossless"`. Or numeric `"0"`-`"100"`.
    pub quality_profile: Option<String>,
    /// Output format string (from QualityIntent node).
    ///
    /// `""` = auto-select, `"jpeg"`, `"png"`, `"webp"`, `"avif"`, `"jxl"`, `"keep"`.
    pub format: Option<String>,
    /// Device pixel ratio for quality adjustment.
    pub dpr: f32,
    /// Lossless preference.
    pub lossless: Option<bool>,
    /// Per-codec params from an explicit encode node (e.g., `zenjpeg.encode`).
    ///
    /// Stored as the raw node instance for downstream code to downcast
    /// via [`NodeInstance::as_any()`].
    pub codec_params: Option<Box<dyn NodeInstance>>,
}

impl core::fmt::Debug for EncodeConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EncodeConfig")
            .field("quality_profile", &self.quality_profile)
            .field("format", &self.format)
            .field("dpr", &self.dpr)
            .field("lossless", &self.lossless)
            .field(
                "codec_params",
                &self.codec_params.as_ref().map(|n| n.schema().id),
            )
            .finish()
    }
}

impl Clone for EncodeConfig {
    fn clone(&self) -> Self {
        Self {
            quality_profile: self.quality_profile.clone(),
            format: self.format.clone(),
            dpr: self.dpr,
            lossless: self.lossless,
            codec_params: self.codec_params.as_ref().map(|n| n.clone_boxed()),
        }
    }
}

impl Default for EncodeConfig {
    fn default() -> Self {
        Self {
            quality_profile: None,
            format: None,
            dpr: 1.0,
            lossless: None,
            codec_params: None,
        }
    }
}

impl EncodeConfig {
    /// Extract encode configuration from a list of encode-phase nodes.
    ///
    /// Looks for:
    /// - `"zenode.quality_intent"` or `"zencodecs.quality_intent"` for quality/format settings
    /// - Any other encode-phase node as codec-specific params
    pub fn from_nodes(nodes: &[Box<dyn NodeInstance>]) -> Self {
        let mut config = Self::default();

        for node in nodes {
            let id = node.schema().id;
            if id == "zenode.quality_intent" || id == "zencodecs.quality_intent" {
                config.quality_profile = node
                    .get_param("profile")
                    .and_then(|v| v.as_str().map(|s| s.to_string()));

                config.format = node.get_param("format").and_then(|v| {
                    let s = v.as_str()?.to_string();
                    if s.is_empty() { None } else { Some(s) }
                });

                config.dpr = node
                    .get_param("dpr")
                    .and_then(|v| v.as_f32())
                    .unwrap_or(1.0);

                config.lossless = node.get_param("lossless").and_then(|v| v.as_bool());
            } else {
                // Any other encode-phase node is treated as codec-specific config.
                config.codec_params = Some(node.clone_boxed());
            }
        }

        config
    }
}
