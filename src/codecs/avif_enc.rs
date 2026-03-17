//! AVIF encode adapter using zenavif via trait interface.

use crate::config::CodecConfig;
use zencodec::encode::EncoderConfig as _;

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams, build_from_config};

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(
        |p| build_encoding(p.quality, p.effort, p.codec_config),
        params,
    )
}

/// Build an AvifEncoderConfig from quality/effort/codec_config.
///
/// Uses `EncoderConfig` trait methods for generic params, with
/// codec_config taking priority for format-specific overrides.
pub(crate) fn build_encoding(
    quality: Option<f32>,
    effort: Option<u32>,
    codec_config: Option<&CodecConfig>,
) -> zenavif::AvifEncoderConfig {
    let mut enc = zenavif::AvifEncoderConfig::new();

    // Format-specific overrides from codec_config take priority
    if let Some(cfg) = codec_config {
        if let Some(q) = cfg.avif_quality {
            enc = enc.with_quality(q);
        } else if let Some(q) = quality {
            enc = enc.with_generic_quality(q);
        }
        // avif_speed is a direct speed value (higher = faster),
        // distinct from generic effort (higher = more work)
        if let Some(speed) = cfg.avif_speed {
            enc = enc.with_effort_u32(speed as u32);
        } else if let Some(e) = effort {
            enc = enc.with_generic_effort(e as i32);
        }
        if let Some(alpha_q) = cfg.avif_alpha_quality {
            enc = enc.with_alpha_quality(alpha_q);
        }
    } else {
        if let Some(q) = quality {
            enc = enc.with_generic_quality(q);
        }
        if let Some(e) = effort {
            enc = enc.with_generic_effort(e as i32);
        }
    }

    enc
}

/// Encode SDR pixels + precomputed gain map to AVIF with embedded tmap item.
///
/// The gain map pixels are first encoded as AV1, then the ISO 21496-1
/// metadata and AV1 gain map are embedded via zenavif-serialize's tmap support.
#[cfg(all(feature = "avif-encode", feature = "jpeg-ultrahdr"))]
pub(crate) fn encode_with_precomputed_gainmap(
    pixel_data: &[u8],
    width: u32,
    height: u32,
    descriptor: zenpixels::PixelDescriptor,
    quality: Option<f32>,
    effort: Option<u32>,
    codec_config: Option<&crate::config::CodecConfig>,
    gain_map: &crate::gainmap::GainMapImage,
    metadata: &crate::gainmap::GainMapMetadata,
    limits: Option<&crate::Limits>,
    stop: Option<&dyn crate::Stop>,
) -> crate::error::Result<crate::EncodeOutput> {
    use crate::{CodecError, ImageFormat};
    use whereat::at;

    gain_map.validate()?;

    // Step 1: Encode gain map pixels as a small AVIF to get AV1 bytes
    let gm_enc = zenavif::EncoderConfig::new(); // Default quality for gain map

    let gm_av1_data = if gain_map.channels == 1 {
        // Grayscale gain map — encode as grayscale AVIF
        // Convert to RGB (ravif doesn't have a direct gray path in the simple API)
        let rgb_pixels: alloc::vec::Vec<rgb::Rgb<u8>> = gain_map.data.iter()
            .map(|&v| rgb::Rgb { r: v, g: v, b: v })
            .collect();
        let img = imgref::ImgVec::new(rgb_pixels, gain_map.width as usize, gain_map.height as usize);
        let result = zenavif::encode_rgb8(img.as_ref(), &gm_enc, &enough::Unstoppable)
            .map_err(|e| at!(CodecError::from_codec(ImageFormat::Avif, e)))?;
        // Extract AV1 data from the AVIF file by re-parsing
        extract_av1_from_avif(&result.avif_file)?
    } else {
        // RGB gain map
        let rgb_pixels: &[rgb::Rgb<u8>] = bytemuck::cast_slice(&gain_map.data);
        let img = imgref::Img::new(rgb_pixels, gain_map.width as usize, gain_map.height as usize);
        let result = zenavif::encode_rgb8(img, &gm_enc, &enough::Unstoppable)
            .map_err(|e| at!(CodecError::from_codec(ImageFormat::Avif, e)))?;
        extract_av1_from_avif(&result.avif_file)?
    };

    // Step 2: Serialize ISO 21496-1 metadata
    let iso_metadata = zenjpeg::ultrahdr::serialize_iso21496(metadata);

    // Step 3: Build the main encoder with gain map attached
    let mut enc = build_encoding(quality, effort, codec_config);
    enc = enc.with_gain_map(
        gm_av1_data,
        gain_map.width,
        gain_map.height,
        8, // bit depth
        iso_metadata,
    );

    // Step 4: Encode the base image through the normal trait path
    use zencodec::encode::{EncodeJob as _, Encoder as _};
    let mut job = enc.job();
    if let Some(lim) = limits {
        job = job.with_limits(crate::limits::to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    job = job.with_canvas_size(width, height);

    let encoder = job.encoder()
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Avif, e)))?;

    let stride = width as usize * descriptor.bytes_per_pixel();
    let adapted = zenpixels_convert::adapt::adapt_for_encode(
        pixel_data,
        descriptor,
        width,
        height,
        stride,
        zenavif::AvifEncoderConfig::supported_descriptors(),
    )
    .map_err(|e| at!(CodecError::InvalidInput(alloc::format!("pixel format: {e}"))))?;

    let adapted_stride = adapted.width as usize * adapted.descriptor.bytes_per_pixel();
    let pixel_slice = zenpixels::PixelSlice::new(
        &adapted.data,
        adapted.width,
        adapted.rows,
        adapted_stride,
        adapted.descriptor,
    )
    .map_err(|e| at!(CodecError::InvalidInput(alloc::format!("pixel slice: {e}"))))?;

    encoder.encode(pixel_slice)
        .map_err(|e| at!(CodecError::from_codec(ImageFormat::Avif, e)))
}

/// Extract the primary item's AV1 data from an AVIF file.
#[cfg(all(feature = "avif-decode", feature = "jpeg-ultrahdr"))]
fn extract_av1_from_avif(avif_data: &[u8]) -> crate::error::Result<alloc::vec::Vec<u8>> {
    use crate::CodecError;
    use whereat::at;

    let parser = zenavif_parse::AvifParser::from_bytes(avif_data)
        .map_err(|e| at!(CodecError::InvalidInput(alloc::format!("parse gain map AVIF: {e}"))))?;
    let primary = parser.primary_data()
        .map_err(|e| at!(CodecError::InvalidInput(alloc::format!("extract AV1: {e}"))))?;
    Ok(primary.to_vec())
}
