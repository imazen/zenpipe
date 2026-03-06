//! AVIF encode adapter using zenavif via trait interface.

use crate::CodecError;
use crate::ImageFormat;
use crate::config::CodecConfig;
use crate::limits::to_resource_limits;
use alloc::boxed::Box;
use zencodec_types::{EncodeJob as _, Encoder as _, EncoderConfig as _};

// ═══════════════════════════════════════════════════════════════════════
// Trait-based encoder dispatch
// ═══════════════════════════════════════════════════════════════════════

use crate::dispatch::{BuiltEncoder, EncodeParams};
use zenpixels::PixelDescriptor;

static AVIF_SUPPORTED: &[PixelDescriptor] = &[
    // SDR
    PixelDescriptor::RGB8_SRGB,
    PixelDescriptor::RGBA8_SRGB,
    PixelDescriptor::GRAY8_SRGB,
    PixelDescriptor::RGBF32_LINEAR,
    PixelDescriptor::RGBAF32_LINEAR,
    PixelDescriptor::GRAYF32_LINEAR,
    // f32 PQ BT.2020 (HDR)
    PixelDescriptor::RGBF32_LINEAR
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    PixelDescriptor::RGBAF32_LINEAR
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    // f32 HLG BT.2020 (HDR)
    PixelDescriptor::RGBF32_LINEAR
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    PixelDescriptor::RGBAF32_LINEAR
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    // 16-bit sRGB
    PixelDescriptor::RGB16_SRGB,
    PixelDescriptor::RGBA16_SRGB,
    // 16-bit PQ BT.2020
    PixelDescriptor::RGB16_SRGB
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    PixelDescriptor::RGBA16_SRGB
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    // 16-bit HLG BT.2020
    PixelDescriptor::RGB16_SRGB
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    PixelDescriptor::RGBA16_SRGB
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020),
    // 16-bit Display P3 sRGB transfer
    PixelDescriptor::RGB16_SRGB.with_primaries(zenpixels::ColorPrimaries::DisplayP3),
    PixelDescriptor::RGBA16_SRGB.with_primaries(zenpixels::ColorPrimaries::DisplayP3),
    // 16-bit PQ BT.2020 narrow range (broadcast HDR10)
    PixelDescriptor::RGB16_SRGB
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020)
        .with_signal_range(zenpixels::SignalRange::Narrow),
    PixelDescriptor::RGBA16_SRGB
        .with_transfer(zenpixels::TransferFunction::Pq)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020)
        .with_signal_range(zenpixels::SignalRange::Narrow),
    // 16-bit HLG BT.2020 narrow range (broadcast HLG)
    PixelDescriptor::RGB16_SRGB
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020)
        .with_signal_range(zenpixels::SignalRange::Narrow),
    PixelDescriptor::RGBA16_SRGB
        .with_transfer(zenpixels::TransferFunction::Hlg)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020)
        .with_signal_range(zenpixels::SignalRange::Narrow),
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
