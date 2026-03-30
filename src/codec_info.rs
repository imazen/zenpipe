//! Codec capability and format detection queries.
//!
//! Provides runtime-queryable information about available codecs,
//! supported formats, extensions, MIME types, and format detection
//! from peek buffers.

extern crate alloc;
extern crate std;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{Value, json};

/// Information about a single image format.
#[derive(Clone, Debug)]
pub struct FormatInfo {
    /// Format name (e.g., "jpeg", "png").
    pub name: String,
    /// Primary MIME type (e.g., "image/jpeg").
    pub mime_type: String,
    /// All accepted MIME types.
    pub mime_types: Vec<String>,
    /// Primary file extension (e.g., "jpg").
    pub extension: String,
    /// All accepted file extensions.
    pub extensions: Vec<String>,
    /// Whether this format supports alpha channels.
    pub supports_alpha: bool,
    /// Whether this format supports lossless compression.
    pub supports_lossless: bool,
    /// Whether this format supports animation/multi-frame.
    pub supports_animation: bool,
    /// Whether decoding is available (compiled in and enabled).
    pub can_decode: bool,
    /// Whether encoding is available (compiled in and enabled).
    pub can_encode: bool,
}

/// All known formats from zencodec.
const KNOWN_FORMATS: &[zencodec::ImageFormat] = &[
    zencodec::ImageFormat::Jpeg,
    zencodec::ImageFormat::Png,
    zencodec::ImageFormat::Gif,
    zencodec::ImageFormat::WebP,
    zencodec::ImageFormat::Avif,
    zencodec::ImageFormat::Jxl,
    zencodec::ImageFormat::Heic,
    zencodec::ImageFormat::Bmp,
    zencodec::ImageFormat::Tiff,
    zencodec::ImageFormat::Pnm,
    zencodec::ImageFormat::Farbfeld,
    zencodec::ImageFormat::Qoi,
    zencodec::ImageFormat::Hdr,
    zencodec::ImageFormat::Tga,
];

/// Query all available codecs and their capabilities.
///
/// Returns a list of format info structs describing every known format,
/// including which are available for decode/encode based on the provided
/// registry (which reflects compile-time feature gates + runtime config).
pub fn list_codecs(registry: &zencodecs::AllowedFormats) -> Vec<FormatInfo> {
    KNOWN_FORMATS
        .iter()
        .filter_map(|&fmt| {
            let def = fmt.definition()?;
            Some(FormatInfo {
                name: def.name.to_string(),
                mime_type: fmt.mime_type().to_string(),
                mime_types: fmt.mime_types().iter().map(|s| s.to_string()).collect(),
                extension: fmt.extension().to_string(),
                extensions: fmt.extensions().iter().map(|s| s.to_string()).collect(),
                supports_alpha: fmt.supports_alpha(),
                supports_lossless: fmt.supports_lossless(),
                supports_animation: fmt.supports_animation(),
                can_decode: registry.can_decode(fmt),
                can_encode: registry.can_encode(fmt),
            })
        })
        .collect()
}

/// Query codecs as JSON (for API endpoints).
pub fn list_codecs_json(registry: &zencodecs::AllowedFormats) -> Value {
    let codecs = list_codecs(registry);
    let entries: Vec<Value> = codecs
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "mime_type": f.mime_type,
                "mime_types": f.mime_types,
                "extension": f.extension,
                "extensions": f.extensions,
                "supports_alpha": f.supports_alpha,
                "supports_lossless": f.supports_lossless,
                "supports_animation": f.supports_animation,
                "can_decode": f.can_decode,
                "can_encode": f.can_encode,
            })
        })
        .collect();
    json!({ "codecs": entries })
}

/// Detect the image format from the first bytes of a file.
///
/// Only needs the first 16–32 bytes (the "peek buffer") to detect
/// most formats. Uses magic byte patterns — no full decode needed.
///
/// Returns `None` if the format is not recognized.
pub fn detect_format(peek: &[u8]) -> Option<FormatInfo> {
    let format_registry = zencodec::ImageFormatRegistry::common();
    let fmt = format_registry.detect(peek)?;
    let def = fmt.definition()?;
    let codec_registry = zencodecs::AllowedFormats::all();

    Some(FormatInfo {
        name: def.name.to_string(),
        mime_type: fmt.mime_type().to_string(),
        mime_types: fmt.mime_types().iter().map(|s| s.to_string()).collect(),
        extension: fmt.extension().to_string(),
        extensions: fmt.extensions().iter().map(|s| s.to_string()).collect(),
        supports_alpha: fmt.supports_alpha(),
        supports_lossless: fmt.supports_lossless(),
        supports_animation: fmt.supports_animation(),
        can_decode: codec_registry.can_decode(fmt),
        can_encode: codec_registry.can_encode(fmt),
    })
}

/// Detect format from peek bytes and return as JSON.
pub fn detect_format_json(peek: &[u8]) -> Value {
    match detect_format(peek) {
        Some(f) => json!({
            "detected": true,
            "name": f.name,
            "mime_type": f.mime_type,
            "extension": f.extension,
            "supports_alpha": f.supports_alpha,
            "supports_lossless": f.supports_lossless,
            "supports_animation": f.supports_animation,
            "can_decode": f.can_decode,
            "can_encode": f.can_encode,
        }),
        None => json!({
            "detected": false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_codecs_returns_known_formats() {
        let registry = zencodecs::AllowedFormats::all();
        let codecs = list_codecs(&registry);
        assert!(codecs.len() >= 10, "should have at least 10 known formats");

        let jpeg = codecs.iter().find(|c| c.name == "jpeg").unwrap();
        assert_eq!(jpeg.mime_type, "image/jpeg");
        assert!(jpeg.extensions.contains(&"jpg".to_string()));
        assert!(!jpeg.supports_alpha);
        assert!(!jpeg.supports_lossless);
    }

    #[test]
    fn list_codecs_json_has_codecs_array() {
        let registry = zencodecs::AllowedFormats::all();
        let json = list_codecs_json(&registry);
        assert!(json["codecs"].as_array().unwrap().len() >= 10);
    }

    #[test]
    fn detect_jpeg() {
        let jpeg_header = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46];
        let result = detect_format(&jpeg_header);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "jpeg");
    }

    #[test]
    fn detect_png() {
        let png_header = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let result = detect_format(&png_header);
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "png");
    }

    #[test]
    fn detect_unknown() {
        let garbage = [0x00, 0x01, 0x02, 0x03];
        let result = detect_format(&garbage);
        assert!(result.is_none());
    }

    #[test]
    fn detect_format_json_detected() {
        let png_header = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let json = detect_format_json(&png_header);
        assert_eq!(json["detected"], true);
        assert_eq!(json["name"], "png");
        assert_eq!(json["mime_type"], "image/png");
    }

    #[test]
    fn detect_format_json_unknown() {
        let garbage = [0x00, 0x01, 0x02, 0x03];
        let json = detect_format_json(&garbage);
        assert_eq!(json["detected"], false);
    }
}
