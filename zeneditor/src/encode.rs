//! Encoding — render RGBA8 pixels to compressed image formats via zencodecs.

/// Result of encoding pixels to a specific format.
pub struct EncodeResult {
    pub data: Vec<u8>,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Parse a format string into a zencodecs ImageFormat.
#[cfg(feature = "encode")]
fn parse_image_format(format: &str) -> Result<zencodecs::ImageFormat, String> {
    match format {
        "jpeg" | "jpg" => Ok(zencodecs::ImageFormat::Jpeg),
        "webp" => Ok(zencodecs::ImageFormat::WebP),
        "png" => Ok(zencodecs::ImageFormat::Png),
        "gif" => Ok(zencodecs::ImageFormat::Gif),
        "jxl" => Ok(zencodecs::ImageFormat::Jxl),
        "avif" => Ok(zencodecs::ImageFormat::Avif),
        _ => Err(format!("Unsupported format: {format}")),
    }
}

/// Encode RGBA8 sRGB pixels via zencodecs EncodeRequest.
#[cfg(feature = "encode")]
pub(crate) fn encode_pixels(
    rgba_data: &[u8],
    width: u32,
    height: u32,
    format: &str,
    options: &serde_json::Value,
    metadata: Option<&zencodec::Metadata>,
) -> Result<EncodeResult, String> {
    let image_format = parse_image_format(format)?;

    let stride = width as usize * 4;
    let pixels = zenpixels::PixelSlice::new(
        rgba_data,
        width,
        height,
        stride,
        zenpixels::PixelDescriptor::RGBA8_SRGB,
    )
    .map_err(|e| format!("PixelSlice: {e}"))?;

    let quality = options
        .get("quality")
        .and_then(|v| v.as_f64())
        .map(|q| q as f32);
    let effort = options
        .get("effort")
        .and_then(|v| v.as_u64())
        .map(|e| e as u32);
    let lossless = options
        .get("lossless")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut req = zencodecs::EncodeRequest::new(image_format).with_lossless(lossless);
    if let Some(q) = quality {
        req = req.with_quality(q);
    }
    if let Some(e) = effort {
        req = req.with_effort(e);
    }
    if let Some(meta) = metadata {
        req = req.with_metadata(meta.clone());
    }

    let output = req
        .encode(pixels, false)
        .map_err(|e| format!("{format} encode: {e}"))?;
    let mime = image_format.mime_type();
    Ok(EncodeResult {
        data: output.into_vec(),
        mime,
        width,
        height,
    })
}
