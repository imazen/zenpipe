//! JXL encode adapter — delegates to zenjxl via trait interface.

use crate::config::CodecConfig;
use crate::dispatch::{BuiltEncoder, EncodeParams, build_from_config};

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
