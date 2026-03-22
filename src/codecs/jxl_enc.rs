//! JXL encode adapter — delegates to zenjxl via trait interface.

use crate::config::CodecConfig;
use crate::dispatch::{BuiltEncoder, EncodeParams, StreamingEncoder, build_from_config};

/// Build a JxlEncoderConfig from encoding params.
fn build_encoding(
    quality: Option<f32>,
    effort: Option<u32>,
    _codec_config: Option<&CodecConfig>,
) -> zenjxl::JxlEncoderConfig {
    use zencodec::encode::EncoderConfig;

    let mut enc = zenjxl::JxlEncoderConfig::default();
    if let Some(q) = quality {
        enc = enc.with_generic_quality(q);
    }
    if let Some(e) = effort {
        enc = enc.with_generic_effort(e as i32);
    }
    enc
}

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    build_from_config(
        |p| build_encoding(p.quality, p.effort, p.codec_config),
        params,
    )
}

pub(crate) fn build_streaming<'a>(
    params: EncodeParams<'a>,
) -> crate::error::Result<StreamingEncoder<'a>> {
    crate::dispatch::build_streaming_from_config(
        |p| build_encoding(p.quality, p.effort, p.codec_config),
        params,
    )
}

/// Encode base image pixels + precomputed gain map to JXL with embedded jhgm box.
///
/// The gain map image is encoded as a bare JXL codestream (lossless), then
/// wrapped in a [`GainMapBundle`](zenjxl::GainMapBundle) with serialized
/// ISO 21496-1 metadata. The bundle is attached to the encoder config so the
/// output JXL file contains a `jhgm` container box.
#[cfg(all(
    feature = "jxl-encode",
    feature = "jxl-decode",
    feature = "jpeg-ultrahdr"
))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_with_precomputed_gainmap(
    pixel_data: &[u8],
    width: u32,
    height: u32,
    descriptor: zenpixels::PixelDescriptor,
    quality: Option<f32>,
    gain_map: &crate::gainmap::GainMap,
    metadata: &crate::gainmap::GainMapMetadata,
    stop: Option<&dyn crate::Stop>,
) -> crate::error::Result<crate::encode::EncodeOutput> {
    use imgref::{Img, ImgExt as _};
    use rgb::{Gray, Rgb};
    use whereat::at;
    use zencodec::ImageFormat;
    use zenjxl::GainMapBundle;

    let _ = stop; // JXL encoder doesn't support cancellation yet

    // 2. Encode the gain map as a bare JXL codestream (lossless for quality)
    let lossless_config = zenjxl::LosslessConfig::default();
    let gain_map_codestream = match gain_map.channels {
        1 => {
            let gray_pixels: &[Gray<u8>] = bytemuck::cast_slice(&gain_map.data);
            let img = Img::new(
                gray_pixels,
                gain_map.width as usize,
                gain_map.height as usize,
            );
            zenjxl::encode_gray8_lossless(img.as_ref(), &lossless_config)
                .map_err(|e| at!(crate::CodecError::from_codec(ImageFormat::Jxl, e)))?
        }
        3 => {
            let rgb_pixels: &[Rgb<u8>] = bytemuck::cast_slice(&gain_map.data);
            let img = Img::new(
                rgb_pixels,
                gain_map.width as usize,
                gain_map.height as usize,
            );
            zenjxl::encode_rgb8_lossless(img.as_ref(), &lossless_config)
                .map_err(|e| at!(crate::CodecError::from_codec(ImageFormat::Jxl, e)))?
        }
        _ => {
            return Err(at!(crate::CodecError::InvalidInput(alloc::format!(
                "gain map channels must be 1 or 3, got {}",
                gain_map.channels,
            ))));
        }
    };

    // 3. Serialize ISO 21496-1 metadata
    let iso_bytes = zenjpeg::ultrahdr::serialize_iso21496(metadata);

    // 4. Build GainMapBundle and serialize
    let bundle = GainMapBundle {
        metadata: iso_bytes,
        color_encoding: None,
        alt_icc_compressed: None,
        gain_map_codestream,
    };
    let jhgm_payload = bundle.serialize();

    // 5. Build encoder config with gain map attached
    let mut enc = build_encoding(quality, None, None);
    enc = enc.with_gain_map(jhgm_payload);

    // 6. Encode the base image through the normal trait-based path
    use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
    let job = enc.job();
    let encoder = job
        .encoder()
        .map_err(|e| at!(crate::CodecError::from_codec(ImageFormat::Jxl, e)))?;

    // Negotiate pixel format: adapt input to what JXL encoder supports
    let stride = width as usize * descriptor.bytes_per_pixel();
    let supported = zenjxl::JxlEncoderConfig::supported_descriptors();

    let adapted = zenpixels_convert::adapt::adapt_for_encode(
        pixel_data, descriptor, width, height, stride, supported,
    )
    .map_err(|e| {
        at!(crate::CodecError::InvalidInput(alloc::format!(
            "pixel format negotiation for JXL gain map encode: {e}"
        )))
    })?;

    let adapted_stride = adapted.width as usize * adapted.descriptor.bytes_per_pixel();
    let pixel_slice = zenpixels::PixelSlice::new(
        &adapted.data,
        adapted.width,
        adapted.rows,
        adapted_stride,
        adapted.descriptor,
    )
    .map_err(|e| {
        at!(crate::CodecError::InvalidInput(alloc::format!(
            "pixel slice for JXL gain map encode: {e}"
        )))
    })?;

    let output = encoder
        .encode(pixel_slice)
        .map_err(|e| at!(crate::CodecError::from_codec(ImageFormat::Jxl, e)))?;

    Ok(output)
}
