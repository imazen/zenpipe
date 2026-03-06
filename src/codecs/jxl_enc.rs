//! JXL encode adapter — delegates to zenjxl via trait interface.

use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use crate::{CodecError, ImageFormat};
use alloc::boxed::Box;

/// Build a JxlEncoderConfig from quality and effort.
///
/// Uses `EncoderConfig` trait methods — quality→distance mapping
/// is handled by zenjxl's `with_generic_quality()`.
fn build_encoding(quality: Option<f32>, effort: Option<u32>) -> zenjxl::JxlEncoderConfig {
    use zencodec_types::EncoderConfig;

    let mut enc = zenjxl::JxlEncoderConfig::default();
    if let Some(q) = quality {
        enc = enc.with_generic_quality(q);
    }
    if let Some(e) = effort {
        enc = enc.with_generic_effort(e as i32);
    }
    enc
}

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams};
use zenpixels::PixelDescriptor;

static JXL_SUPPORTED: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::BGRA8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    BuiltEncoder {
        encoder: Box::new(move |pixels| {
            let enc = build_encoding(params.quality, params.effort);
            let mut job = enc.job();
            if let Some(lim) = params.limits {
                job = job.with_limits(to_resource_limits(lim));
            }
            if let Some(meta) = params.metadata {
                job = job.with_metadata(meta);
            }
            if let Some(s) = params.stop {
                job = job.with_stop(s);
            }
            use zencodec_types::Encoder as _;
            job.encoder()
                .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))?
                .encode(pixels)
                .map_err(|e| CodecError::from_codec(ImageFormat::Jxl, e))
        }),
        supported: JXL_SUPPORTED,
    }
}
