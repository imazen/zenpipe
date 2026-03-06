//! AVIF encode adapter using zenavif via trait interface.

use crate::CodecError;
use crate::ImageFormat;
use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use alloc::boxed::Box;

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams};
use zenpixels::PixelDescriptor;

static AVIF_SUPPORTED: &[PixelDescriptor] = &[
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
];

pub(crate) fn build_trait_encoder<'a>(params: EncodeParams<'a>) -> BuiltEncoder<'a> {
    BuiltEncoder {
        encoder: Box::new(move |pixels| {
            let enc = build_encoding(params.quality, params.effort, params.codec_config);
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
                .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))?
                .encode(pixels)
                .map_err(|e| CodecError::from_codec(ImageFormat::Avif, e))
        }),
        supported: AVIF_SUPPORTED,
    }
}

/// Build an AvifEncoderConfig from quality/effort/codec_config.
///
/// Uses `EncoderConfig` trait methods for generic params, with
/// codec_config taking priority for format-specific overrides.
fn build_encoding(
    quality: Option<f32>,
    effort: Option<u32>,
    codec_config: Option<&CodecConfig>,
) -> zenavif::AvifEncoderConfig {
    use zencodec_types::EncoderConfig;

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
