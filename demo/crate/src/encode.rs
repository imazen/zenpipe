//! Codec encoding — converts rendered RGBA8 pixels to encoded image bytes.
//!
//! Supports JPEG, WebP, PNG, and GIF via the zen codec crates.
//! Each codec is invoked through the [`zencodec::encode`] trait chain:
//! `EncoderConfig → EncodeJob → Encoder → encode(PixelSlice)`.

use zencodec::encode::{EncodeJob as _, EncodeOutput, Encoder as _, EncoderConfig as _};
use zenpixels::{PixelDescriptor, PixelSlice};

/// Encoded output with format metadata.
#[derive(Debug)]
pub struct EncodedImage {
    pub data: Vec<u8>,
    pub format: &'static str,
    pub mime: &'static str,
}

/// Encode RGBA8 sRGB pixels to the specified format.
///
/// `rgba_data` must be tightly-packed RGBA8 sRGB (`width * height * 4` bytes).
///
/// `options` is a JSON object with format-specific settings:
/// - `quality` (number, 0-100): encoding quality (JPEG, WebP, GIF)
/// - `effort` (number, 0-10): compression effort (PNG, WebP)
/// - `lossless` (bool): lossless mode (WebP, PNG)
pub fn encode(
    rgba_data: &[u8],
    width: u32,
    height: u32,
    format: &str,
    options: &serde_json::Value,
) -> Result<EncodedImage, String> {
    let stride = width as usize * 4;
    let pixels = PixelSlice::new(
        rgba_data,
        width,
        height,
        stride,
        PixelDescriptor::RGBA8_SRGB,
    )
    .map_err(|e| format!("PixelSlice: {e}"))?;

    match format {
        "jpeg" => encode_jpeg(pixels, options),
        "webp" => encode_webp(pixels, options),
        "png" => encode_png(pixels, options),
        "gif" => encode_gif(pixels, options),
        "jxl" => encode_jxl(pixels, options),
        "avif" => encode_avif(pixels, options),
        _ => Err(format!("Unsupported format: {format}")),
    }
}

fn into_encoded(output: EncodeOutput, format: &'static str, mime: &'static str) -> EncodedImage {
    EncodedImage {
        data: output.into_vec(),
        format,
        mime,
    }
}

fn encode_jpeg(pixels: PixelSlice<'_>, opts: &serde_json::Value) -> Result<EncodedImage, String> {
    let quality = opts.get("quality").and_then(|v| v.as_f64()).unwrap_or(85.0) as f32;

    let config =
        zenjpeg::JpegEncoderConfig::ycbcr(quality, zenjpeg::encoder::ChromaSubsampling::Quarter);

    let output = config
        .encode(pixels)
        .map_err(|e| format!("JPEG encode: {e}"))?;
    Ok(into_encoded(output, "jpeg", "image/jpeg"))
}

fn encode_webp(pixels: PixelSlice<'_>, opts: &serde_json::Value) -> Result<EncodedImage, String> {
    let quality = opts.get("quality").and_then(|v| v.as_f64()).unwrap_or(80.0) as f32;
    let lossless = opts
        .get("lossless")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let config = if lossless {
        zenwebp::zencodec::WebpEncoderConfig::lossless()
    } else {
        zenwebp::zencodec::WebpEncoderConfig::lossy().with_quality(quality)
    };

    let output = config
        .job()
        .encoder()
        .map_err(|e| format!("WebP encoder init: {e}"))?
        .encode(pixels)
        .map_err(|e| format!("WebP encode: {e}"))?;
    Ok(into_encoded(output, "webp", "image/webp"))
}

fn encode_png(pixels: PixelSlice<'_>, opts: &serde_json::Value) -> Result<EncodedImage, String> {
    let effort = opts.get("effort").and_then(|v| v.as_i64()).unwrap_or(5) as i32;

    let config = zenpng::PngEncoderConfig::new().with_generic_effort(effort);

    let output = config
        .job()
        .encoder()
        .map_err(|e| format!("PNG encoder init: {e}"))?
        .encode(pixels)
        .map_err(|e| format!("PNG encode: {e}"))?;
    Ok(into_encoded(output, "png", "image/png"))
}

fn encode_jxl(pixels: PixelSlice<'_>, opts: &serde_json::Value) -> Result<EncodedImage, String> {
    let quality = opts.get("quality").and_then(|v| v.as_f64()).unwrap_or(75.0) as f32;
    let effort = opts.get("effort").and_then(|v| v.as_i64()).unwrap_or(7) as i32;
    let lossless = opts
        .get("lossless")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut config = zenjxl::JxlEncoderConfig::new();
    config = config.with_generic_quality(quality);
    config = config.with_generic_effort(effort);
    if lossless {
        config = config.with_lossless(true);
    }

    let output = config
        .job()
        .encoder()
        .map_err(|e| format!("JXL encoder init: {e}"))?
        .encode(pixels)
        .map_err(|e| format!("JXL encode: {e}"))?;
    Ok(into_encoded(output, "jxl", "image/jxl"))
}

fn encode_avif(pixels: PixelSlice<'_>, opts: &serde_json::Value) -> Result<EncodedImage, String> {
    let quality = opts.get("quality").and_then(|v| v.as_f64()).unwrap_or(75.0) as f32;
    let effort = opts.get("effort").and_then(|v| v.as_i64()).unwrap_or(6) as i32;
    let lossless = opts
        .get("lossless")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut config = zenavif::AvifEncoderConfig::new().with_quality(quality);
    config = config.with_generic_effort(effort);
    if lossless {
        config = config.with_lossless(true);
    }

    let output = config
        .job()
        .encoder()
        .map_err(|e| format!("AVIF encoder init: {e}"))?
        .encode(pixels)
        .map_err(|e| format!("AVIF encode: {e}"))?;
    Ok(into_encoded(output, "avif", "image/avif"))
}

fn encode_gif(pixels: PixelSlice<'_>, opts: &serde_json::Value) -> Result<EncodedImage, String> {
    let quality = opts.get("quality").and_then(|v| v.as_f64()).unwrap_or(80.0) as f32;

    let config = zengif::GifEncoderConfig::new().with_quality(quality);

    let output = config
        .job()
        .encoder()
        .map_err(|e| format!("GIF encoder init: {e}"))?
        .encode(pixels)
        .map_err(|e| format!("GIF encode: {e}"))?;
    Ok(into_encoded(output, "gif", "image/gif"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rgba(w: u32, h: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for pixel in data.chunks_exact_mut(4) {
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
            pixel[3] = 255;
        }
        data
    }

    #[test]
    fn encode_jpeg_default() {
        let data = solid_rgba(64, 48, 128, 64, 200);
        let opts = serde_json::json!({});
        let result = encode(&data, 64, 48, "jpeg", &opts).unwrap();
        assert_eq!(result.format, "jpeg");
        assert_eq!(result.mime, "image/jpeg");
        assert!(result.data.len() > 100, "JPEG output too small");
        // JPEG magic bytes
        assert_eq!(&result.data[..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn encode_webp_lossy() {
        let data = solid_rgba(64, 48, 128, 64, 200);
        let opts = serde_json::json!({"quality": 75});
        let result = encode(&data, 64, 48, "webp", &opts).unwrap();
        assert_eq!(result.format, "webp");
        assert_eq!(result.mime, "image/webp");
        assert!(result.data.len() > 20, "WebP output too small");
        // RIFF header
        assert_eq!(&result.data[..4], b"RIFF");
    }

    #[test]
    fn encode_png_default() {
        let data = solid_rgba(64, 48, 128, 64, 200);
        let opts = serde_json::json!({});
        let result = encode(&data, 64, 48, "png", &opts).unwrap();
        assert_eq!(result.format, "png");
        assert_eq!(result.mime, "image/png");
        assert!(result.data.len() > 20, "PNG output too small");
        // PNG signature
        assert_eq!(&result.data[..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn encode_gif_default() {
        let data = solid_rgba(64, 48, 128, 64, 200);
        let opts = serde_json::json!({});
        let result = encode(&data, 64, 48, "gif", &opts).unwrap();
        assert_eq!(result.format, "gif");
        assert_eq!(result.mime, "image/gif");
        assert!(result.data.len() > 10, "GIF output too small");
        // GIF signature
        assert_eq!(&result.data[..3], b"GIF");
    }

    #[test]
    fn encode_jxl_default() {
        let data = solid_rgba(64, 48, 128, 64, 200);
        let opts = serde_json::json!({});
        let result = encode(&data, 64, 48, "jxl", &opts).unwrap();
        assert_eq!(result.format, "jxl");
        assert_eq!(result.mime, "image/jxl");
        assert!(result.data.len() > 10, "JXL output too small");
        // JXL signature: 0xFF 0x0A (naked codestream) or container
        assert!(
            result.data[..2] == [0xFF, 0x0A]
                || result.data[..12]
                    == [
                        0, 0, 0, 0x0C, 0x4A, 0x58, 0x4C, 0x20, 0x0D, 0x0A, 0x87, 0x0A
                    ],
            "Expected JXL magic bytes, got {:?}",
            &result.data[..12.min(result.data.len())]
        );
    }

    #[test]
    fn encode_jxl_lossless() {
        let data = solid_rgba(32, 32, 100, 150, 200);
        let opts = serde_json::json!({"lossless": true});
        let result = encode(&data, 32, 32, "jxl", &opts).unwrap();
        assert_eq!(result.format, "jxl");
        assert!(result.data.len() > 10);
    }

    #[test]
    fn encode_avif_default() {
        let data = solid_rgba(64, 48, 128, 64, 200);
        let opts = serde_json::json!({});
        let result = encode(&data, 64, 48, "avif", &opts).unwrap();
        assert_eq!(result.format, "avif");
        assert_eq!(result.mime, "image/avif");
        assert!(result.data.len() > 10, "AVIF output too small");
    }

    #[test]
    fn encode_unsupported_format() {
        let data = solid_rgba(8, 8, 128, 128, 128);
        let opts = serde_json::json!({});
        let result = encode(&data, 8, 8, "bmp", &opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported"));
    }
}
