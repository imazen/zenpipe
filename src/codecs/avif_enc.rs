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
