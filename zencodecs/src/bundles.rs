//! Opt-in codec and media factory registration bundles for [`zencodec::CodecSet`].
//!
//! Registrations are split strictly by direction (`register_still_decoders`,
//! `register_still_encoders`, `register_media_demuxers`, `register_media_muxers`,
//! `register_video_decoders`, `register_video_encoders`) so read-only callers
//! never link unused encoder or muxer code.
//!
//! Zero concrete backends are enabled by default in `zencodecs`; callers opt into
//! individual features (`jpeg`, `png`, `webp`, `gif`, `avif-decode`, `avif-encode`,
//! `mp4`, `webm`, `ivf`, `av1`, `av1-encode`, `vp8`, `vp8-encode`, `vp9`, etc.).

#[cfg(feature = "media")]
use alloc::sync::Arc;
use zencodec::{CodecSet, RegistrationError, RegistrationId};

/// Standard reverse-DNS registration identifiers for built-in zen providers.
pub mod ids {
    /// Zen JPEG decoder/encoder (`zenjpeg`).
    pub const ZENJPEG: &str = "org.imazen.jpeg.zenjpeg";
    /// Pure-Rust MozJPEG encoder (`mozjpeg-rs`).
    pub const MOZJPEG_RS: &str = "org.imazen.jpeg.mozjpeg-rs";
    /// Zen PNG decoder/encoder (`zenpng`).
    pub const ZENPNG: &str = "org.imazen.png.zenpng";
    /// Zen WebP decoder/encoder (`zenwebp`).
    pub const ZENWEBP: &str = "org.imazen.webp.zenwebp";
    /// Zen GIF decoder/encoder (`zengif`).
    pub const ZENGIF: &str = "org.imazen.gif.zengif";
    /// Zen AVIF image decoder/encoder (`zenavif`).
    pub const ZENAVIF: &str = "org.imazen.avif.zenavif";
    /// Zen JXL decoder/encoder (`zenjxl`).
    pub const ZENJXL: &str = "org.imazen.jxl.zenjxl";
    /// Zen MP4 container demuxer/muxer (`zenmp4`).
    pub const ZENMP4: &str = "org.imazen.mp4.zenmp4";
    /// Zen WebM container demuxer/muxer (`zenwebm`).
    pub const ZENWEBM: &str = "org.imazen.webm.zenwebm";
    /// Zen IVF elementary container demuxer/muxer (`zenivf`).
    pub const ZENIVF: &str = "org.imazen.ivf.zenivf";
    /// AV1 video decoder (`zenavif` / `rav1d-safe`).
    pub const AV1_RAV1D: &str = "org.imazen.av1.rav1d";
    /// AV1 video encoder (`zenavif` / `zenrav1e`).
    pub const AV1_ZENRAV1E: &str = "org.imazen.av1.zenrav1e";
    /// VP8 video decoder/encoder (`zenvp8`).
    pub const VP8_ZENVP8: &str = "org.imazen.vp8.zenvp8";
    /// VP9 video decoder/encoder (`zenvp9` / `rusty_vp9`).
    pub const VP9_RUSTY_VP9: &str = "org.imazen.vp9.rusty_vp9";
    /// Pure-Rust ICC CMS backend (`moxcms`).
    pub const MOXCMS: &str = "org.imazen.cms.moxcms";
}

/// Register compiled-in zen still/animated image **decoders** (`jpeg`, `png`, `webp`,
/// `gif`, `avif-decode`, `jxl-decode`) into `set`.
///
/// Legacy bitmap formats (BMP, Farbfeld, PNM, QOI, TGA) are not registered by this bundle
/// and may be registered explicitly on `CodecSet` by the host if required.
/// Never references any image encoder or media muxer type.
#[allow(unused_mut)]
pub fn register_still_decoders(mut set: CodecSet) -> Result<CodecSet, RegistrationError> {
    #[cfg(feature = "jpeg")]
    {
        set = set.with_named_decoder(
            RegistrationId::new(ids::ZENJPEG)?,
            "zenjpeg-0.9",
            zenjpeg::JpegDecoderConfig::new(),
        )?;
    }
    #[cfg(feature = "png")]
    {
        set = set.with_named_decoder(
            RegistrationId::new(ids::ZENPNG)?,
            "zenpng-0.2",
            zenpng::PngDecoderConfig::new(),
        )?;
    }
    #[cfg(feature = "webp")]
    {
        set = set.with_named_decoder(
            RegistrationId::new(ids::ZENWEBP)?,
            "zenwebp-0.5",
            zenwebp::zencodec::WebpDecoderConfig::new(),
        )?;
    }
    #[cfg(feature = "gif")]
    {
        set = set.with_named_decoder(
            RegistrationId::new(ids::ZENGIF)?,
            "zengif-0.8",
            zengif::GifDecoderConfig::new(),
        )?;
    }
    #[cfg(feature = "avif-decode")]
    {
        set = set.with_named_decoder(
            RegistrationId::new(ids::ZENAVIF)?,
            "zenavif-0.2",
            zenavif::AvifDecoderConfig::new(),
        )?;
    }
    #[cfg(feature = "jxl-decode")]
    {
        set = set.with_named_decoder(
            RegistrationId::new(ids::ZENJXL)?,
            "zenjxl-0.3",
            zenjxl::JxlDecoderConfig::new(),
        )?;
    }
    Ok(set)
}

/// Register all compiled-in still/animated image **encoders** into `set`.
///
/// Never references any image decoder or media demuxer type.
#[allow(unused_mut)]
pub fn register_still_encoders(mut set: CodecSet) -> Result<CodecSet, RegistrationError> {
    #[cfg(feature = "jpeg")]
    {
        set = set.with_named_send_encoder(
            RegistrationId::new(ids::ZENJPEG)?,
            "zenjpeg-0.9",
            zenjpeg::JpegEncoderConfig::new(),
        )?;
    }
    #[cfg(feature = "png")]
    {
        set = set.with_named_send_encoder(
            RegistrationId::new(ids::ZENPNG)?,
            "zenpng-0.2",
            zenpng::PngEncoderConfig::new(),
        )?;
    }
    #[cfg(feature = "webp")]
    {
        set = set.with_named_send_encoder(
            RegistrationId::new(ids::ZENWEBP)?,
            "zenwebp-0.5",
            zenwebp::zencodec::WebpEncoderConfig::lossy(),
        )?;
    }
    #[cfg(feature = "gif")]
    {
        set = set.with_named_send_encoder(
            RegistrationId::new(ids::ZENGIF)?,
            "zengif-0.8",
            zengif::GifEncoderConfig::new(),
        )?;
    }
    #[cfg(feature = "avif-encode")]
    {
        set = set.with_named_send_encoder(
            RegistrationId::new(ids::ZENAVIF)?,
            "zenavif-0.2",
            zenavif::AvifEncoderConfig::new(),
        )?;
    }
    #[cfg(feature = "jxl-encode")]
    {
        set = set.with_named_send_encoder(
            RegistrationId::new(ids::ZENJXL)?,
            "zenjxl-0.3",
            zenjxl::JxlEncoderConfig::new(),
        )?;
    }
    Ok(set)
}

/// Register all compiled-in media container **demuxers** (`MP4`, `WebM`, `IVF`).
#[cfg(feature = "media")]
pub fn register_media_demuxers(mut set: CodecSet) -> Result<CodecSet, RegistrationError> {
    use zencodec::NamedFactory;
    use zencodec_media::registry::{ContainerId, DemuxRole};

    #[cfg(feature = "mp4")]
    {
        let f: Arc<dyn zencodec_media::registry::DemuxFactory> =
            Arc::new(zenmp4::Mp4DemuxFactory::new());
        set = set.with_role::<DemuxRole>(
            [ContainerId::Mp4],
            NamedFactory::new(RegistrationId::new(ids::ZENMP4)?, "zenmp4-0.1", f),
        )?;
    }
    #[cfg(feature = "webm")]
    {
        let f: Arc<dyn zencodec_media::registry::DemuxFactory> =
            Arc::new(zenwebm::WebmDemuxFactory::new());
        set = set.with_role::<DemuxRole>(
            [ContainerId::Webm],
            NamedFactory::new(RegistrationId::new(ids::ZENWEBM)?, "zenwebm-0.1", f),
        )?;
    }
    #[cfg(feature = "ivf")]
    {
        let f: Arc<dyn zencodec_media::registry::DemuxFactory> =
            Arc::new(zenivf::IvfDemuxFactory::new());
        set = set.with_role::<DemuxRole>(
            [ContainerId::Ivf],
            NamedFactory::new(RegistrationId::new(ids::ZENIVF)?, "zenivf-0.1", f),
        )?;
    }
    Ok(set)
}

#[cfg(all(feature = "media", feature = "webm"))]
fn builtin_webm_mux_singleton() -> &'static Arc<dyn zencodec_media::registry::MuxFactory> {
    static FACTORY: once_cell::race::OnceBox<Arc<dyn zencodec_media::registry::MuxFactory>> =
        once_cell::race::OnceBox::new();
    FACTORY.get_or_init(|| alloc::boxed::Box::new(Arc::new(zenwebm::WebmMuxFactory::new())))
}

#[cfg(all(feature = "media", feature = "ivf"))]
fn builtin_ivf_mux_singleton() -> &'static Arc<dyn zencodec_media::registry::MuxFactory> {
    static FACTORY: once_cell::race::OnceBox<Arc<dyn zencodec_media::registry::MuxFactory>> =
        once_cell::race::OnceBox::new();
    FACTORY.get_or_init(|| alloc::boxed::Box::new(Arc::new(zenivf::IvfMuxFactory::new())))
}

/// Returns `true` iff `factory` is literally the built-in `zenwebm::WebmMuxFactory` singleton
/// (checked by pointer identity in both `std` and `no_std + alloc`, never spoofable by a
/// third-party registration ID string).
#[cfg(feature = "media")]
pub(crate) fn is_builtin_webm_mux(factory: &Arc<dyn zencodec_media::registry::MuxFactory>) -> bool {
    #[cfg(feature = "webm")]
    {
        Arc::ptr_eq(factory, builtin_webm_mux_singleton())
    }
    #[cfg(not(feature = "webm"))]
    {
        let _ = factory;
        false
    }
}

/// Returns `true` iff `factory` is literally the built-in `zenivf::IvfMuxFactory` singleton
/// (checked by pointer identity in both `std` and `no_std + alloc`, never spoofable by a
/// third-party registration ID string).
#[cfg(feature = "media")]
pub(crate) fn is_builtin_ivf_mux(factory: &Arc<dyn zencodec_media::registry::MuxFactory>) -> bool {
    #[cfg(feature = "ivf")]
    {
        Arc::ptr_eq(factory, builtin_ivf_mux_singleton())
    }
    #[cfg(not(feature = "ivf"))]
    {
        let _ = factory;
        false
    }
}

/// Register all compiled-in media container **muxers** (`MP4`, `WebM`, `IVF`).
#[cfg(feature = "media")]
#[allow(unused_mut)]
pub fn register_media_muxers(mut set: CodecSet) -> Result<CodecSet, RegistrationError> {
    use zencodec::NamedFactory;
    use zencodec_media::registry::{ContainerId, MuxRole};

    #[cfg(feature = "mp4")]
    {
        let f: Arc<dyn zencodec_media::registry::MuxFactory> =
            Arc::new(zenmp4::Mp4MuxFactory::new());
        set = set.with_role::<MuxRole>(
            [ContainerId::Mp4],
            NamedFactory::new(RegistrationId::new(ids::ZENMP4)?, "zenmp4-0.1", f),
        )?;
    }
    #[cfg(feature = "webm")]
    {
        let f: Arc<dyn zencodec_media::registry::MuxFactory> =
            Arc::clone(builtin_webm_mux_singleton());
        set = set.with_role::<MuxRole>(
            [ContainerId::Webm],
            NamedFactory::new(RegistrationId::new(ids::ZENWEBM)?, "zenwebm-0.1", f),
        )?;
    }
    #[cfg(feature = "ivf")]
    {
        let f: Arc<dyn zencodec_media::registry::MuxFactory> =
            Arc::clone(builtin_ivf_mux_singleton());
        set = set.with_role::<MuxRole>(
            [ContainerId::Ivf],
            NamedFactory::new(RegistrationId::new(ids::ZENIVF)?, "zenivf-0.1", f),
        )?;
    }
    Ok(set)
}

/// Register all compiled-in video bitstream **decoders** (`AV1`, `VP8`, `VP9`).
#[cfg(feature = "media")]
#[allow(unused_mut)]
pub fn register_video_decoders(mut set: CodecSet) -> Result<CodecSet, RegistrationError> {
    use zencodec::NamedFactory;
    use zencodec_media::registry::VideoDecodeRole;
    use zencodec_media::track::CodecId as MediaCodecId;

    #[cfg(feature = "av1")]
    {
        let f: Arc<dyn zencodec_media::registry::VideoDecodeFactory> =
            Arc::new(zenavif::media::Av1DecodeFactory::new());
        set = set.with_role::<VideoDecodeRole>(
            [MediaCodecId::Av1],
            NamedFactory::new(RegistrationId::new(ids::AV1_RAV1D)?, "zenavif-rav1d-0.2", f),
        )?;
    }
    #[cfg(feature = "vp8")]
    {
        let f: Arc<dyn zencodec_media::registry::VideoDecodeFactory> =
            Arc::new(zenvp8::media::Vp8DecodeFactory);
        set = set.with_role::<VideoDecodeRole>(
            [MediaCodecId::Vp8],
            NamedFactory::new(RegistrationId::new(ids::VP8_ZENVP8)?, "zenvp8-0.1", f),
        )?;
    }
    #[cfg(feature = "vp9")]
    {
        let f: Arc<dyn zencodec_media::registry::VideoDecodeFactory> =
            Arc::new(zenvp9::media::Vp9DecodeFactory);
        set = set.with_role::<VideoDecodeRole>(
            [MediaCodecId::Vp9],
            NamedFactory::new(
                RegistrationId::new(ids::VP9_RUSTY_VP9)?,
                "zenvp9-rusty_vp9-0.3.0",
                f,
            ),
        )?;
    }
    Ok(set)
}

/// Register all compiled-in video bitstream **encoders** (`AV1`, `VP8`, `VP9`).
#[cfg(feature = "media")]
#[allow(unused_mut)]
pub fn register_video_encoders(mut set: CodecSet) -> Result<CodecSet, RegistrationError> {
    use zencodec::NamedFactory;
    use zencodec_media::registry::VideoEncodeRole;
    use zencodec_media::track::CodecId as MediaCodecId;

    #[cfg(feature = "av1-encode")]
    {
        let f: Arc<dyn zencodec_media::registry::VideoEncodeFactory> =
            Arc::new(zenavif::media::Av1EncodeFactory::new());
        set = set.with_role::<VideoEncodeRole>(
            [MediaCodecId::Av1],
            NamedFactory::new(
                RegistrationId::new(ids::AV1_ZENRAV1E)?,
                "zenavif-zenrav1e-0.2",
                f,
            ),
        )?;
    }
    #[cfg(feature = "vp8-encode")]
    {
        let f: Arc<dyn zencodec_media::registry::VideoEncodeFactory> =
            Arc::new(zenvp8::media::Vp8EncodeFactory::new());
        set = set.with_role::<VideoEncodeRole>(
            [MediaCodecId::Vp8],
            NamedFactory::new(RegistrationId::new(ids::VP8_ZENVP8)?, "zenvp8-enc-0.1", f),
        )?;
    }
    #[cfg(feature = "vp9-encode")]
    {
        let f: Arc<dyn zencodec_media::registry::VideoEncodeFactory> =
            Arc::new(zenvp9::media::Vp9EncodeFactory::default());
        set = set.with_role::<VideoEncodeRole>(
            [MediaCodecId::Vp9],
            NamedFactory::new(
                RegistrationId::new(ids::VP9_RUSTY_VP9)?,
                "zenvp9-rusty_vp9-enc-0.3.0",
                f,
            ),
        )?;
    }
    Ok(set)
}

/// Register every compiled-in still and media decoder/encoder/demuxer/muxer into `set`.
pub fn register_all_enabled(mut set: CodecSet) -> Result<CodecSet, RegistrationError> {
    set = register_still_decoders(set)?;
    set = register_still_encoders(set)?;
    #[cfg(feature = "media")]
    {
        set = register_media_demuxers(set)?;
        set = register_media_muxers(set)?;
        set = register_video_decoders(set)?;
        set = register_video_encoders(set)?;
    }
    Ok(set)
}

/// Convenience: construct a [`zenpixels_convert::ColorServices`] using `moxcms` when enabled.
#[cfg(feature = "cms")]
#[must_use]
pub fn moxcms_color_services() -> zenpixels_convert::ColorServices {
    use alloc::sync::Arc;
    use zenpixels_convert::{CmsBinding, CmsQuote, ColorServices, MoxCms};

    let binding = CmsBinding::new(
        ids::MOXCMS,
        "moxcms-0.8",
        Arc::new(MoxCms),
        CmsQuote::default(),
    );
    ColorServices::with_cms_binding(binding)
}

#[cfg(all(test, feature = "media", feature = "webm", feature = "ivf"))]
mod builtin_mux_identity_tests {
    use super::*;
    use zencodec::NamedFactory;
    use zencodec_media::registry::{ContainerId, MuxFactory, MuxRole};

    #[test]
    fn builtin_webm_and_ivf_mux_identity_works_without_std() {
        let set = register_media_muxers(CodecSet::new()).expect("register built-in muxers");
        let webm = set
            .factories::<MuxRole>(&ContainerId::Webm)
            .next()
            .expect("built-in webm factory");
        let ivf = set
            .factories::<MuxRole>(&ContainerId::Ivf)
            .next()
            .expect("built-in ivf factory");
        assert!(
            is_builtin_webm_mux(&webm.factory),
            "built-in WebM muxer must keep concrete singleton identity in both std and no_std"
        );
        assert!(
            is_builtin_ivf_mux(&ivf.factory),
            "built-in IVF muxer must keep concrete singleton identity in both std and no_std"
        );
        assert!(!is_builtin_webm_mux(&ivf.factory));
        assert!(!is_builtin_ivf_mux(&webm.factory));

        // A freshly wrapped factory registered under the same built-in ID must NOT match.
        let fresh_webm: Arc<dyn MuxFactory> = Arc::new(zenwebm::WebmMuxFactory::new());
        let fresh_ivf: Arc<dyn MuxFactory> = Arc::new(zenivf::IvfMuxFactory::new());
        let spoof_set = CodecSet::new()
            .with_role::<MuxRole>(
                [ContainerId::Webm],
                NamedFactory::new(RegistrationId::new(ids::ZENWEBM).unwrap(), "spoof", fresh_webm),
            )
            .unwrap()
            .with_role::<MuxRole>(
                [ContainerId::Ivf],
                NamedFactory::new(RegistrationId::new(ids::ZENIVF).unwrap(), "spoof", fresh_ivf),
            )
            .unwrap();
        let spoof_webm = spoof_set
            .factories::<MuxRole>(&ContainerId::Webm)
            .next()
            .unwrap();
        let spoof_ivf = spoof_set
            .factories::<MuxRole>(&ContainerId::Ivf)
            .next()
            .unwrap();
        assert!(!is_builtin_webm_mux(&spoof_webm.factory));
        assert!(!is_builtin_ivf_mux(&spoof_ivf.factory));
    }
}
