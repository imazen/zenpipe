//! Zennode definitions for all codec, quantization, and quality-intent nodes.
//!
//! This module holds every encode/decode node schema that zencodecs owns.
//! Geometry, resize, and pipeline-level nodes live in zenpipe's own
//! `zennode_defs` module. Filter nodes live in zenfilters.
//!
//! The [`QualityIntentNode`] bridges zennode's parameter system with
//! zencodecs' [`CodecIntent`] for format selection and quality control.
//!
//! Feature-gated behind `feature = "zennode"`.

extern crate alloc;
use alloc::string::String;

use zennode::*;

// ═══════════════════════════════════════════════════════════════════════
//  CODEC — encode/decode nodes (schemas only, conversion in codec crates)
// ═══════════════════════════════════════════════════════════════════════

// ─── JPEG ───

/// JPEG encoder configuration as a self-documenting pipeline node.
///
/// Schema-only definition for pipeline registry. Conversion to native
/// zenjpeg config types happens in the bridge layer via `ParamMap`.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenjpeg.encode", group = Encode, role = Encode)]
#[node(tags("jpeg", "jpg", "encode", "lossy"))]
pub struct EncodeJpeg {
    #[param(range(0.0..=100.0), default = 85.0, step = 1.0)]
    #[param(section = "Quality", label = "Quality")]
    #[kv("jpeg.quality", "jpeg.q")]
    pub quality: Option<f32>,

    #[param(range(0..=2), default = 1)]
    #[param(section = "Quality", label = "Effort")]
    #[kv("jpeg.effort")]
    pub effort: Option<i32>,

    #[param(default = "ycbcr")]
    #[param(section = "Color", label = "Color Space")]
    #[kv("jpeg.colorspace")]
    pub color_space: Option<String>,

    #[param(default = "quarter")]
    #[param(section = "Color", label = "Chroma Subsampling")]
    #[kv("jpeg.subsampling", "jpeg.ss")]
    pub subsampling: Option<String>,

    #[param(default = "average")]
    #[param(section = "Color", label = "Chroma Downsampling")]
    #[kv("jpeg.chroma_method")]
    pub chroma_downsampling: Option<String>,

    #[param(default = "progressive")]
    #[param(section = "Encoding", label = "Scan Mode")]
    #[kv("jpeg.progressive", "jpeg.mode")]
    pub scan_mode: Option<String>,

    #[param(default = "jpegli")]
    #[param(section = "Encoding", label = "Quantization Tables")]
    #[kv("jpeg.tables")]
    pub quant_tables: Option<String>,

    #[param(default = true)]
    #[param(section = "Advanced")]
    #[kv("jpeg.deringing")]
    pub deringing: Option<bool>,

    #[param(default = true)]
    #[param(section = "Advanced", label = "Adaptive Quantization")]
    #[kv("jpeg.aq")]
    pub aq: Option<bool>,
}

/// Mozjpeg-compatible JPEG encoder configuration.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenjpeg.encode_mozjpeg", group = Encode, role = Encode)]
#[node(tags("jpeg", "jpg", "encode", "lossy", "mozjpeg", "compat"))]
pub struct EncodeMozjpeg {
    #[param(range(1.0..=100.0), default = 85.0, step = 1.0)]
    #[param(section = "Quality", label = "Quality")]
    #[kv("mozjpeg.quality", "mozjpeg.q")]
    pub quality: Option<f32>,

    #[param(range(0..=2), default = 1)]
    #[param(section = "Quality", label = "Effort")]
    #[kv("mozjpeg.effort")]
    pub effort: Option<i32>,

    #[param(default = "quarter")]
    #[param(section = "Color", label = "Chroma Subsampling")]
    #[kv("mozjpeg.subsampling", "mozjpeg.ss")]
    pub subsampling: Option<String>,
}

/// JPEG decoder configuration.
#[derive(Node, Clone, Debug)]
#[node(id = "zenjpeg.decode", group = Decode, role = Decode)]
#[node(tags("jpeg", "jpg", "decode"))]
pub struct DecodeJpeg {
    #[param(default = "balanced")]
    #[param(section = "Main", label = "Strictness")]
    #[kv("jpeg.strictness")]
    pub strictness: String,

    #[param(default = true)]
    #[param(section = "Main", label = "Auto Orient")]
    #[kv("jpeg.orient", "jpeg.auto_orient")]
    pub auto_orient: bool,

    #[param(range(0..=10000), default = 100)]
    #[param(unit = "MP", section = "Limits")]
    #[kv("jpeg.max_megapixels")]
    pub max_megapixels: Option<u32>,
}

impl Default for DecodeJpeg {
    fn default() -> Self {
        Self {
            strictness: String::from("balanced"),
            auto_orient: true,
            max_megapixels: None,
        }
    }
}

// ─── PNG ───

/// PNG encoding with quality, lossless mode, and compression options.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenpng.encode", group = Encode, role = Encode)]
#[node(tags("codec", "png", "lossless", "encode"))]
pub struct EncodePng {
    #[param(range(0..=100), default = 0, step = 1)]
    #[param(unit = "", section = "Main", label = "Quality")]
    #[kv("quality")]
    pub quality: Option<u32>,

    #[param(range(0..=100), default = 0, step = 1)]
    #[param(unit = "", section = "Main", label = "PNG Quality")]
    #[kv("png.quality")]
    pub png_quality: Option<u32>,

    #[param(range(0..=100), default = 0, step = 1)]
    #[param(unit = "", section = "Main", label = "Min Quality")]
    #[kv("png.min_quality")]
    pub min_quality: Option<u32>,

    #[param(default = true)]
    #[param(section = "Main")]
    #[kv("png.lossless")]
    pub lossless: Option<bool>,

    #[param(default = false)]
    #[param(section = "Advanced")]
    #[kv("png.max_deflate")]
    pub max_deflate: Option<bool>,
}

// ─── WebP ───

/// WebP lossy (VP8) encode node.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenwebp.encode_lossy", group = Encode, role = Encode)]
#[node(tags("webp", "lossy", "encode"))]
pub struct EncodeWebpLossy {
    #[param(range(0.0..=100.0), step = 1.0)]
    #[param(section = "Main", label = "Quality")]
    #[kv("webp.quality", "webp.q")]
    pub quality: Option<f32>,

    #[param(range(0.0..=100.0), step = 1.0)]
    #[param(section = "Main", label = "Effort")]
    #[kv("webp.effort")]
    pub effort: Option<f32>,

    #[param(section = "Main", label = "Preset")]
    #[kv("webp.preset")]
    pub preset: Option<String>,

    #[param(section = "Main", label = "Sharp YUV")]
    #[kv("webp.sharp_yuv")]
    pub sharp_yuv: Option<bool>,

    #[param(range(0..=100))]
    #[param(section = "Alpha", label = "Alpha Quality")]
    #[kv("webp.alpha_quality", "webp.aq")]
    pub alpha_quality: Option<u32>,

    #[param(section = "Target", label = "Target Size")]
    #[kv("webp.target_size")]
    pub target_size: Option<u32>,

    #[param(range(0.0..=100.0), step = 0.1)]
    #[param(section = "Target", label = "Target PSNR")]
    #[kv("webp.target_psnr")]
    pub target_psnr: Option<f32>,

    #[param(range(1..=4))]
    #[param(section = "Advanced", label = "Segments")]
    #[kv("webp.segments")]
    pub segments: Option<u32>,

    #[param(range(0..=100))]
    #[param(section = "Advanced", label = "SNS Strength")]
    #[kv("webp.sns")]
    pub sns_strength: Option<u32>,

    #[param(range(0..=100))]
    #[param(section = "Advanced", label = "Filter Strength")]
    #[kv("webp.filter")]
    pub filter_strength: Option<u32>,

    #[param(range(0..=7))]
    #[param(section = "Advanced", label = "Filter Sharpness")]
    #[kv("webp.sharpness")]
    pub filter_sharpness: Option<u32>,
}

/// WebP lossless (VP8L) encode node.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenwebp.encode_lossless", group = Encode, role = Encode)]
#[node(tags("webp", "lossless", "encode"))]
pub struct EncodeWebpLossless {
    #[param(range(0.0..=100.0), step = 1.0)]
    #[param(section = "Main", label = "Compression Effort")]
    #[kv("webp.effort")]
    pub effort: Option<f32>,

    #[param(range(0..=100))]
    #[param(section = "Main", label = "Near-Lossless")]
    #[kv("webp.near_lossless", "webp.nl")]
    pub near_lossless: Option<u32>,

    #[param(section = "Advanced")]
    #[kv("webp.exact")]
    pub exact: Option<bool>,

    #[param(range(0..=100))]
    #[param(section = "Alpha", label = "Alpha Quality")]
    #[kv("webp.alpha_quality", "webp.aq")]
    pub alpha_quality: Option<u32>,

    #[param(section = "Target", label = "Target Size")]
    #[kv("webp.target_size")]
    pub target_size: Option<u32>,
}

/// WebP decode node.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenwebp.decode", group = Decode, role = Decode)]
#[node(tags("webp", "decode"))]
pub struct DecodeWebp {
    #[param(section = "Main", label = "Upsampling")]
    #[kv("webp.upsampling")]
    pub upsampling: Option<String>,

    #[param(range(0..=100))]
    #[param(section = "Main", label = "Dithering Strength")]
    #[kv("webp.dithering", "webp.dither")]
    pub dithering_strength: Option<u32>,
}

// ─── GIF ───

/// GIF encoder settings.
#[derive(Node, Clone, Debug)]
#[node(id = "zengif.encode", group = Encode, role = Encode)]
#[node(tags("gif", "encode", "animation", "palette"))]
pub struct EncodeGif {
    #[param(range(1.0..=100.0), default = 80.0, step = 1.0)]
    #[param(section = "Quality", label = "Palette Quality")]
    #[kv("gif.quality")]
    pub quality: Option<f32>,

    #[param(range(0.0..=1.0), default = 0.5, step = 0.05)]
    #[param(section = "Quality", label = "Dithering")]
    #[kv("gif.dithering", "gif.dither")]
    pub dithering: Option<f32>,

    #[param(range(0.0..=255.0), default = 0.0, identity = 0.0, step = 1.0)]
    #[param(section = "Quality", label = "Lossy Tolerance")]
    #[kv("gif.lossy")]
    pub lossy_tolerance: Option<f32>,

    #[param(default = "auto")]
    #[param(section = "Advanced", label = "Quantizer")]
    #[kv("gif.quantizer")]
    pub quantizer: String,

    #[param(default = true)]
    #[param(section = "Animation", label = "Shared Palette")]
    #[kv("gif.shared_palette")]
    pub shared_palette: Option<bool>,

    #[param(range(0.0..=50.0), default = 5.0, step = 0.5)]
    #[param(section = "Animation", label = "Palette Error Threshold")]
    #[kv("gif.palette_threshold")]
    pub palette_error_threshold: Option<f32>,

    #[param(default = "infinite")]
    #[param(section = "Animation", label = "Loop")]
    #[kv("gif.loop")]
    pub loop_count: String,

    #[param(default = true)]
    #[param(section = "Advanced", label = "Transparency Optimization")]
    #[kv("gif.transparency")]
    pub transparency_optimization: Option<bool>,
}

impl Default for EncodeGif {
    fn default() -> Self {
        Self {
            quality: None,
            dithering: None,
            lossy_tolerance: None,
            quantizer: String::from("auto"),
            shared_palette: None,
            palette_error_threshold: None,
            loop_count: String::from("infinite"),
            transparency_optimization: None,
        }
    }
}

// ─── AVIF ───

/// AVIF encoding node.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenavif.encode", group = Encode, role = Encode)]
#[node(tags("avif", "encode", "av1"))]
pub struct AvifEncode {
    #[param(range(1.0..=100.0), default = 75.0, step = 1.0)]
    #[param(section = "Main", label = "Quality")]
    #[kv("avif.q", "avif.quality")]
    pub quality: Option<f32>,

    #[param(range(1..=10), default = 4)]
    #[param(section = "Main", label = "Speed")]
    #[kv("avif.speed")]
    pub speed: Option<u32>,

    #[param(range(0.0..=100.0), default = 0.0, identity = 0.0, step = 1.0)]
    #[param(section = "Main", label = "Alpha Quality")]
    #[kv("avif.alpha_quality", "avif.aq")]
    pub alpha_quality: Option<f32>,

    #[param(default = "auto")]
    #[param(section = "Main", label = "Bit Depth")]
    #[kv("avif.depth")]
    pub bit_depth: Option<String>,

    #[param(default = "444")]
    #[param(section = "Advanced", label = "Chroma Subsampling")]
    #[kv("avif.chroma")]
    pub chroma_subsampling: Option<String>,

    #[param(default = "ycbcr")]
    #[param(section = "Advanced", label = "Color Model")]
    #[kv("avif.color_model")]
    pub color_model: Option<String>,

    #[param(default = "clean")]
    #[param(section = "Advanced", label = "Alpha Mode")]
    #[kv("avif.alpha_mode")]
    pub alpha_mode: Option<String>,

    #[param(default = false)]
    #[param(section = "Advanced")]
    #[kv("avif.lossless")]
    pub lossless: Option<bool>,
}

// ─── JXL ───

/// JPEG XL encoder configuration.
#[derive(Node, Clone, Debug, Default)]
#[node(id = "zenjxl.encode", group = Encode, role = Encode)]
#[node(tags("jxl", "jpeg-xl", "encode", "lossy", "lossless", "hdr", "codec"))]
pub struct EncodeJxl {
    #[param(range(0..=100), default = 75, step = 1)]
    #[param(unit = "", section = "Quality", label = "Quality")]
    #[kv("quality")]
    pub quality: Option<i32>,

    #[param(range(0.0..=100.0), default = 75.0, identity = 75.0, step = 1.0)]
    #[param(unit = "", section = "Quality", label = "JXL Quality")]
    #[kv("jxl.quality", "jxl.q")]
    pub jxl_quality: Option<f32>,

    #[param(range(0.0..=25.0), default = 1.0, identity = 1.0, step = 0.1)]
    #[param(unit = "butteraugli", section = "Quality")]
    #[kv("jxl.distance", "jxl.d")]
    pub distance: Option<f32>,

    #[param(default = false)]
    #[param(section = "Mode")]
    #[kv("jxl.lossless")]
    pub lossless: Option<bool>,

    #[param(range(1..=10), default = 7)]
    #[param(section = "Speed", label = "Effort")]
    #[kv("jxl.effort", "jxl.e")]
    pub effort: Option<i32>,

    #[param(default = false)]
    #[param(section = "Advanced")]
    #[kv("jxl.noise")]
    pub noise: Option<bool>,
}

/// JPEG XL decoder configuration.
#[derive(Node, Clone, Debug)]
#[node(id = "zenjxl.decode", group = Decode, role = Decode)]
#[node(tags("jxl", "jpeg-xl", "decode", "codec"))]
pub struct DecodeJxl {
    #[param(default = true)]
    #[param(section = "Main")]
    #[kv("jxl.orient")]
    pub adjust_orientation: bool,

    #[param(range(0.0..=10000.0), default = 0.0, identity = 0.0, step = 100.0)]
    #[param(unit = "nits", section = "HDR")]
    #[kv("jxl.nits")]
    pub intensity_target: Option<f32>,
}

impl Default for DecodeJxl {
    fn default() -> Self {
        Self {
            adjust_orientation: true,
            intensity_target: None,
        }
    }
}

// ─── TIFF ───

/// TIFF encoding with compression and predictor options.
#[derive(Node, Clone, Debug)]
#[node(id = "zentiff.encode", group = Encode, role = Encode)]
#[node(tags("codec", "tiff", "lossless", "encode"))]
pub struct EncodeTiff {
    #[param(default = "lzw")]
    #[param(section = "Main", label = "Compression")]
    #[kv("tiff.compression")]
    pub compression: String,

    #[param(default = true)]
    #[param(section = "Main", label = "Predictor")]
    #[kv("tiff.predictor")]
    pub predictor: bool,
}

impl Default for EncodeTiff {
    fn default() -> Self {
        Self {
            compression: String::from("lzw"),
            predictor: true,
        }
    }
}

// ─── BMP ───

/// BMP encoding with bit depth selection.
#[derive(Node, Clone, Debug)]
#[node(id = "zenbitmaps.encode_bmp", group = Encode, role = Encode)]
#[node(tags("codec", "bmp", "lossless", "encode"))]
pub struct EncodeBmp {
    #[param(range(1..=32), default = 24, step = 8)]
    #[param(unit = "bits", section = "Main", label = "Bit Depth")]
    #[kv("bmp.bits", "bits")]
    pub bits: i32,
}

impl Default for EncodeBmp {
    fn default() -> Self {
        Self { bits: 24 }
    }
}

// ─── HEIC ───

/// HEIC/HEIF decode node.
#[derive(Node, Clone, Debug)]
#[node(id = "heic.decode", group = Decode, role = Decode)]
#[node(tags("heic", "heif", "hdr", "depth"))]
pub struct DecodeHeic {
    #[param(default = true)]
    #[param(section = "Supplements", label = "Extract Gain Map")]
    #[kv("heic.gain_map")]
    pub extract_gain_map: bool,

    #[param(default = false)]
    #[param(section = "Supplements", label = "Extract Depth Map")]
    #[kv("heic.depth")]
    pub extract_depth: bool,

    #[param(default = false)]
    #[param(section = "Supplements", label = "Extract Mattes")]
    #[kv("heic.mattes")]
    pub extract_mattes: bool,

    #[param(default = false)]
    #[param(section = "Main", label = "Decode Thumbnail")]
    #[kv("heic.thumbnail")]
    pub decode_thumbnail: bool,
}

impl Default for DecodeHeic {
    fn default() -> Self {
        Self {
            extract_gain_map: true,
            extract_depth: false,
            extract_mattes: false,
            decode_thumbnail: false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  QUANTIZATION
// ═══════════════════════════════════════════════════════════════════════

/// Palette quantization with perceptual masking.
#[derive(Node, Clone, Debug)]
#[node(id = "zenquant.quantize", group = Quantize, role = Quantize)]
#[node(tags("quantize", "palette", "indexed"))]
pub struct Quantize {
    #[param(range(2..=256), default = 256, step = 1)]
    #[param(unit = "colors", section = "Main", label = "Max Colors")]
    #[kv("quant.max_colors", "max_colors")]
    pub max_colors: i32,

    #[param(default = "best")]
    #[param(section = "Main", label = "Quality")]
    #[kv("quant.quality", "quality")]
    pub quality: String,

    #[param(range(0.0..=1.0), default = 0.5, identity = 0.5, step = 0.05)]
    #[param(unit = "", section = "Main", label = "Dithering")]
    #[kv("quant.dither_strength", "dither_strength")]
    pub dither_strength: f32,
}

impl Default for Quantize {
    fn default() -> Self {
        Self {
            max_colors: 256,
            quality: String::from("best"),
            dither_strength: 0.5,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  QUALITY INTENT — format selection and quality profile
// ═══════════════════════════════════════════════════════════════════════

use crate::ImageFormat;
use crate::format_set::FormatSet;
use crate::intent::{BoolKeep, CodecIntent, FormatChoice};
use crate::quality::QualityProfile;

/// Format selection and quality profile for encoding (zennode node).
///
/// This node controls output format selection and quality. It supports
/// both RIAPI querystring keys and JSON API fields, matching imageflow's
/// established `EncoderPreset::Auto` / `EncoderPreset::Format` ergonomics.
///
/// **RIAPI**: `?qp=high&accept.webp=true&accept.avif=true`
/// **JSON**: `{ "profile": "high", "allow_webp": true, "allow_avif": true }`
///
/// When `format` is empty (default), the pipeline auto-selects the best
/// format from the allowed set. When `format` is set (e.g., "jpeg"),
/// that format is used directly.
///
/// The `profile` field accepts both named presets and numeric values:
/// - Named: lowest, low, medium_low, medium, good, high, highest, lossless
/// - Numeric: 0-100 (mapped to codec-specific quality scales)
///
/// Convert to [`CodecIntent`] via [`to_codec_intent()`](QualityIntentNode::to_codec_intent).
#[derive(Node, Clone, Debug)]
#[node(id = "zencodecs.quality_intent", group = Encode, role = Encode)]
#[node(tags("quality", "auto", "format", "encode"))]
pub struct QualityIntentNode {
    /// Quality profile: named preset or numeric 0-100.
    ///
    /// Named presets: "lowest", "low", "medium_low", "medium",
    /// "good", "high", "highest", "lossless".
    /// Numeric: "0" to "100" (codec-specific mapping).
    #[param(default = "high")]
    #[param(section = "Main", label = "Quality Profile")]
    #[kv("qp")]
    pub profile: String,

    /// Explicit output format. Empty = auto-select from allowed formats.
    ///
    /// Values: "jpeg", "png", "webp", "gif", "avif", "jxl", "keep", or "".
    /// "keep" preserves the source format.
    #[param(default = "")]
    #[param(section = "Main", label = "Output Format")]
    #[kv("format")]
    pub format: String,

    /// Device pixel ratio for quality adjustment.
    ///
    /// Higher DPR screens tolerate lower quality (smaller pixels).
    /// Default 1.0 = no adjustment.
    #[param(range(0.5..=10.0), default = 1.0, identity = 1.0, step = 0.5)]
    #[param(unit = "\u{00d7}", section = "Main")]
    #[kv("qp.dpr", "qp.dppx", "dpr", "dppx")]
    pub dpr: f32,

    /// Global lossless preference. Empty = default (lossy).
    ///
    /// Accepts "true", "false", or "keep" (match source losslessness).
    #[param(default = "")]
    #[param(section = "Main")]
    #[kv("lossless")]
    pub lossless: String,

    /// Allow WebP output. Must be explicitly enabled.
    #[param(default = false)]
    #[param(section = "Allowed Formats")]
    #[kv("accept.webp")]
    pub allow_webp: bool,

    /// Allow AVIF output. Must be explicitly enabled.
    #[param(default = false)]
    #[param(section = "Allowed Formats")]
    #[kv("accept.avif")]
    pub allow_avif: bool,

    /// Allow JPEG XL output. Must be explicitly enabled.
    #[param(default = false)]
    #[param(section = "Allowed Formats")]
    #[kv("accept.jxl")]
    pub allow_jxl: bool,

    /// Allow non-sRGB color profiles in the output.
    #[param(default = false)]
    #[param(section = "Allowed Formats")]
    #[kv("accept.color_profiles")]
    pub allow_color_profiles: bool,
}

impl Default for QualityIntentNode {
    fn default() -> Self {
        Self {
            profile: String::from("high"),
            format: String::new(),
            dpr: 1.0,
            lossless: String::new(),
            allow_webp: false,
            allow_avif: false,
            allow_jxl: false,
            allow_color_profiles: false,
        }
    }
}

impl QualityIntentNode {
    /// Convert this node into a [`CodecIntent`] for use with zencodecs'
    /// format selection and encoding pipeline.
    pub fn to_codec_intent(&self) -> CodecIntent {
        let format = self.parse_format();
        let quality_profile = QualityProfile::parse(&self.profile);
        let quality_dpr = if (self.dpr - 1.0).abs() < f32::EPSILON {
            None
        } else {
            Some(self.dpr)
        };
        let lossless = self.parse_lossless();
        let allowed = self.build_format_set();

        // If qp is set but format is absent, default to Auto
        let format = format.or_else(|| {
            if quality_profile.is_some() {
                Some(FormatChoice::Auto)
            } else {
                None
            }
        });

        CodecIntent {
            format,
            quality_profile,
            quality_fallback: None,
            quality_dpr,
            lossless,
            allowed,
            hints: Default::default(),
            matte: None,
        }
    }

    /// Parse the `format` field into a [`FormatChoice`].
    fn parse_format(&self) -> Option<FormatChoice> {
        if self.format.is_empty() {
            return None;
        }
        Some(match self.format.to_ascii_lowercase().as_str() {
            "auto" => FormatChoice::Auto,
            "keep" => FormatChoice::Keep,
            "jpeg" | "jpg" => FormatChoice::Specific(ImageFormat::Jpeg),
            "png" => FormatChoice::Specific(ImageFormat::Png),
            "gif" => FormatChoice::Specific(ImageFormat::Gif),
            "webp" => FormatChoice::Specific(ImageFormat::WebP),
            "avif" => FormatChoice::Specific(ImageFormat::Avif),
            "jxl" => FormatChoice::Specific(ImageFormat::Jxl),
            "heic" => FormatChoice::Specific(ImageFormat::Heic),
            _ => FormatChoice::Auto,
        })
    }

    /// Parse the `lossless` field into a [`BoolKeep`].
    fn parse_lossless(&self) -> Option<BoolKeep> {
        if self.lossless.is_empty() {
            return None;
        }
        match self.lossless.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(BoolKeep::True),
            "false" | "0" | "no" => Some(BoolKeep::False),
            "keep" => Some(BoolKeep::Keep),
            _ => None,
        }
    }

    /// Build a [`FormatSet`] from the `allow_*` booleans.
    ///
    /// Web-safe formats (JPEG, PNG, GIF) are always included as the baseline.
    /// Modern formats (WebP, AVIF, JXL) must be explicitly enabled.
    fn build_format_set(&self) -> FormatSet {
        let mut set = FormatSet::web_safe();
        if self.allow_webp {
            set.insert(ImageFormat::WebP);
        }
        if self.allow_avif {
            set.insert(ImageFormat::Avif);
        }
        if self.allow_jxl {
            set.insert(ImageFormat::Jxl);
        }
        set
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  REGISTRATION
// ═══════════════════════════════════════════════════════════════════════

/// Register all zencodecs-owned node definitions with a registry.
///
/// This includes codec encode/decode, quantization, and quality-intent nodes.
pub fn register(registry: &mut NodeRegistry) {
    for node in ALL {
        registry.register(*node);
    }
}

/// All zencodecs zennode definitions.
pub static ALL: &[&dyn NodeDef] = &[
    // Codec — JPEG
    &ENCODE_JPEG_NODE,
    &ENCODE_MOZJPEG_NODE,
    &DECODE_JPEG_NODE,
    // Codec — PNG
    &ENCODE_PNG_NODE,
    // Codec — WebP
    &ENCODE_WEBP_LOSSY_NODE,
    &ENCODE_WEBP_LOSSLESS_NODE,
    &DECODE_WEBP_NODE,
    // Codec — GIF
    &ENCODE_GIF_NODE,
    // Codec — AVIF
    &AVIF_ENCODE_NODE,
    // Codec — JXL
    &ENCODE_JXL_NODE,
    &DECODE_JXL_NODE,
    // Codec — TIFF
    &ENCODE_TIFF_NODE,
    // Codec — BMP
    &ENCODE_BMP_NODE,
    // Codec — HEIC
    &DECODE_HEIC_NODE,
    // Quantization
    &QUANTIZE_NODE,
    // Quality intent
    &QUALITY_INTENT_NODE_NODE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_basics() {
        let schema = QUALITY_INTENT_NODE_NODE.schema();
        assert_eq!(schema.id, "zencodecs.quality_intent");
        assert_eq!(schema.group, NodeGroup::Encode);
        assert_eq!(schema.role, NodeRole::Encode);
        assert!(schema.tags.contains(&"quality"));
        assert!(schema.tags.contains(&"auto"));
        assert!(schema.tags.contains(&"format"));
        assert!(schema.tags.contains(&"encode"));

        let param_names: alloc::vec::Vec<&str> = schema.params.iter().map(|p| p.name).collect();
        assert!(param_names.contains(&"profile"));
        assert!(param_names.contains(&"format"));
        assert!(param_names.contains(&"dpr"));
        assert!(param_names.contains(&"lossless"));
        assert!(param_names.contains(&"allow_webp"));
        assert!(param_names.contains(&"allow_avif"));
        assert!(param_names.contains(&"allow_jxl"));
        assert!(param_names.contains(&"allow_color_profiles"));
    }

    #[test]
    fn default_values() {
        let node = QUALITY_INTENT_NODE_NODE.create_default().unwrap();
        assert_eq!(
            node.get_param("profile"),
            Some(ParamValue::Str("high".into()))
        );
        assert_eq!(
            node.get_param("format"),
            Some(ParamValue::Str(String::new()))
        );
        assert_eq!(node.get_param("dpr"), Some(ParamValue::F32(1.0)));
        assert_eq!(
            node.get_param("lossless"),
            Some(ParamValue::Str(String::new()))
        );
        assert_eq!(node.get_param("allow_webp"), Some(ParamValue::Bool(false)));
        assert_eq!(node.get_param("allow_avif"), Some(ParamValue::Bool(false)));
        assert_eq!(node.get_param("allow_jxl"), Some(ParamValue::Bool(false)));
        assert_eq!(
            node.get_param("allow_color_profiles"),
            Some(ParamValue::Bool(false))
        );
    }

    #[test]
    fn kv_keys_coverage() {
        let schema = QUALITY_INTENT_NODE_NODE.schema();

        let profile_param = schema.params.iter().find(|p| p.name == "profile").unwrap();
        assert_eq!(profile_param.kv_keys, &["qp"]);

        let format_param = schema.params.iter().find(|p| p.name == "format").unwrap();
        assert_eq!(format_param.kv_keys, &["format"]);

        let dpr_param = schema.params.iter().find(|p| p.name == "dpr").unwrap();
        assert!(dpr_param.kv_keys.contains(&"qp.dpr"));
        assert!(dpr_param.kv_keys.contains(&"dpr"));
        assert!(dpr_param.kv_keys.contains(&"dppx"));
    }

    #[test]
    fn kv_parsing_qp_with_accepts() {
        let mut kv = KvPairs::from_querystring("qp=medium&accept.webp=true&accept.avif=true");
        let node = QUALITY_INTENT_NODE_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(
            node.get_param("profile"),
            Some(ParamValue::Str("medium".into()))
        );
        assert_eq!(node.get_param("allow_webp"), Some(ParamValue::Bool(true)));
        assert_eq!(node.get_param("allow_avif"), Some(ParamValue::Bool(true)));
        assert_eq!(node.get_param("allow_jxl"), Some(ParamValue::Bool(false)));
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn kv_parsing_format_explicit() {
        let mut kv = KvPairs::from_querystring("format=webp&qp=good");
        let node = QUALITY_INTENT_NODE_NODE.from_kv(&mut kv).unwrap().unwrap();
        assert_eq!(
            node.get_param("format"),
            Some(ParamValue::Str("webp".into()))
        );
        assert_eq!(
            node.get_param("profile"),
            Some(ParamValue::Str("good".into()))
        );
        assert_eq!(kv.unconsumed().count(), 0);
    }

    #[test]
    fn kv_parsing_no_match() {
        let mut kv = KvPairs::from_querystring("w=800&h=600");
        let result = QUALITY_INTENT_NODE_NODE.from_kv(&mut kv).unwrap();
        assert!(result.is_none());
    }

    // ═══════════════════════════════════════════════════════════════════
    // to_codec_intent tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn to_codec_intent_default() {
        let node = QualityIntentNode::default();
        let intent = node.to_codec_intent();
        // profile="high" -> qp triggers auto
        assert_eq!(intent.format, Some(FormatChoice::Auto));
        assert_eq!(intent.quality_profile, Some(QualityProfile::High));
        assert!(intent.quality_dpr.is_none()); // dpr 1.0 -> None
        assert!(intent.lossless.is_none()); // empty string -> None
        // web_safe baseline
        assert!(intent.allowed.contains(ImageFormat::Jpeg));
        assert!(intent.allowed.contains(ImageFormat::Png));
        assert!(intent.allowed.contains(ImageFormat::Gif));
        assert!(!intent.allowed.contains(ImageFormat::WebP));
        assert!(!intent.allowed.contains(ImageFormat::Avif));
        assert!(!intent.allowed.contains(ImageFormat::Jxl));
    }

    #[test]
    fn to_codec_intent_with_format() {
        let node = QualityIntentNode {
            format: String::from("jpeg"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(
            intent.format,
            Some(FormatChoice::Specific(ImageFormat::Jpeg))
        );
    }

    #[test]
    fn to_codec_intent_format_keep() {
        let node = QualityIntentNode {
            format: String::from("keep"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.format, Some(FormatChoice::Keep));
    }

    #[test]
    fn to_codec_intent_dpr_adjustment() {
        let node = QualityIntentNode {
            dpr: 2.0,
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.quality_dpr, Some(2.0));
    }

    #[test]
    fn to_codec_intent_lossless_true() {
        let node = QualityIntentNode {
            lossless: String::from("true"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.lossless, Some(BoolKeep::True));
    }

    #[test]
    fn to_codec_intent_lossless_keep() {
        let node = QualityIntentNode {
            lossless: String::from("keep"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.lossless, Some(BoolKeep::Keep));
    }

    #[test]
    fn to_codec_intent_allowed_formats() {
        let node = QualityIntentNode {
            allow_webp: true,
            allow_avif: true,
            allow_jxl: true,
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert!(intent.allowed.contains(ImageFormat::WebP));
        assert!(intent.allowed.contains(ImageFormat::Avif));
        assert!(intent.allowed.contains(ImageFormat::Jxl));
        // web_safe still present
        assert!(intent.allowed.contains(ImageFormat::Jpeg));
        assert!(intent.allowed.contains(ImageFormat::Png));
        assert!(intent.allowed.contains(ImageFormat::Gif));
    }

    #[test]
    fn to_codec_intent_numeric_profile() {
        let node = QualityIntentNode {
            profile: String::from("55"),
            ..Default::default()
        };
        let intent = node.to_codec_intent();
        assert_eq!(intent.quality_profile, Some(QualityProfile::Medium));
    }

    #[test]
    fn downcast() {
        let node = QUALITY_INTENT_NODE_NODE.create_default().unwrap();
        let qi = node.as_any().downcast_ref::<QualityIntentNode>().unwrap();
        assert_eq!(qi.profile, "high");
        assert!(!qi.allow_webp);
    }

    #[test]
    fn all_codec_nodes_registered() {
        let mut r = NodeRegistry::new();
        register(&mut r);
        // Verify registration doesn't panic and all nodes are valid
        assert_eq!(ALL.len(), 16);
    }
}
