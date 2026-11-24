use std::io::Cursor;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

const MAX_PIXELS: u64 = 64_000_000;
const MAX_SIDE: u32 = 16_384;
const MAX_FRAMES: usize = 256;
const MAX_INPUT_BYTES: usize = 128 * 1024 * 1024;
const MAX_CODEC_MEMORY: usize = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct DecodeOptions {
    /// When true, also probe Level 5 compliance where supported.
    probe_level5: bool,
    /// When true, apply external moxcms ICC->sRGB conversion for jxl-rs when an ICC profile is embedded.
    apply_external_cms_for_jxl_rs: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DecodeReport {
    decoder: String,
    version: String,
    ok: bool,
    error: Option<String>,
    width: u32,
    height: u32,
    frame_count: u32,
    frame_durations_ms: Vec<u32>,
    loop_count: Option<u32>,
    has_alpha: bool,
    has_icc: bool,
    is_cmyk: bool,
    uses_original_profile: bool,
    extra_channels: u32,
    orientation: u8,
    cms_used: String,
    level5_accepted: Option<bool>,
    warnings: Vec<String>,
}

impl DecodeReport {
    fn error(decoder: &str, version: &str, err: impl Into<String>) -> Self {
        Self {
            decoder: decoder.to_string(),
            version: version.to_string(),
            ok: false,
            error: Some(err.into()),
            width: 0,
            height: 0,
            frame_count: 0,
            frame_durations_ms: Vec::new(),
            loop_count: None,
            has_alpha: false,
            has_icc: false,
            is_cmyk: false,
            uses_original_profile: false,
            extra_channels: 0,
            orientation: 1,
            cms_used: "none".to_string(),
            level5_accepted: None,
            warnings: Vec::new(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct DecoderOutput {
    json: String,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl DecoderOutput {
    pub fn json(&self) -> String {
        self.json.clone()
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

fn check_dims(width: usize, height: usize) -> Result<(u32, u32), String> {
    if width == 0 || height == 0 {
        return Err("Decoded dimensions are zero.".into());
    }
    let w = u32::try_from(width).map_err(|_| "Decoded width overflows u32.".to_string())?;
    let h = u32::try_from(height).map_err(|_| "Decoded height overflows u32.".to_string())?;
    if w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(format!(
            "Decoded dimensions ({w}x{h}) exceed 16384 px / 64 MP safety limit."
        ));
    }
    Ok((w, h))
}

fn unpremultiply_rgba8(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        if let Some(a) = NonZeroU32::new(u32::from(px[3])) {
            if a.get() < 255 {
                for v in &mut px[..3] {
                    *v = ((u32::from(*v) * 255 + a.get() / 2) / a.get()).min(255) as u8;
                }
            }
        }
    }
}

fn apply_moxcms_icc_to_srgb_rgba8(
    icc: &[u8],
    rgba: &mut [u8],
    is_cmyk: bool,
) -> Result<String, String> {
    let src_profile = moxcms::ColorProfile::new_from_slice(icc)
        .map_err(|e| format!("moxcms parse ICC failed: {e:?}"))?;
    let dst_profile = moxcms::ColorProfile::new_srgb();
    let opts = moxcms::TransformOptions::default();

    if is_cmyk {
        // JXL CMYK convention: 0 = full ink, 255 = no ink.
        // moxcms standard ICC CMYK expects 0 = no ink, 255 = full ink.
        let mut cmyk_in = Vec::with_capacity(rgba.len());
        for px in rgba.chunks_exact(4) {
            cmyk_in.push(255u8.saturating_sub(px[0]));
            cmyk_in.push(255u8.saturating_sub(px[1]));
            cmyk_in.push(255u8.saturating_sub(px[2]));
            cmyk_in.push(255u8.saturating_sub(px[3]));
        }
        let xform = src_profile
            .create_transform_8bit(
                moxcms::Layout::Rgba,
                &dst_profile,
                moxcms::Layout::Rgba,
                opts,
            )
            .map_err(|e| format!("moxcms CMYK->sRGB transform failed: {e:?}"))?;
        let mut out = vec![0u8; rgba.len()];
        xform
            .transform(&cmyk_in, &mut out)
            .map_err(|e| format!("moxcms CMYK transform execution failed: {e:?}"))?;
        for px in out.chunks_exact_mut(4) {
            px[3] = 255;
        }
        rgba.copy_from_slice(&out);
        Ok("moxcms (external CMYK->sRGB)".to_string())
    } else if src_profile.color_space == moxcms::DataColorSpace::Rgb {
        let xform = src_profile
            .create_transform_8bit(
                moxcms::Layout::Rgba,
                &dst_profile,
                moxcms::Layout::Rgba,
                opts,
            )
            .map_err(|e| format!("moxcms RGB->sRGB transform failed: {e:?}"))?;
        let mut out = vec![0u8; rgba.len()];
        xform
            .transform(rgba, &mut out)
            .map_err(|e| format!("moxcms RGB transform execution failed: {e:?}"))?;
        // Preserve original alpha channel byte-for-byte
        for (dst_px, src_px) in out.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            dst_px[3] = src_px[3];
        }
        rgba.copy_from_slice(&out);
        Ok("moxcms (external ICC->sRGB)".to_string())
    } else {
        Ok("none (non-RGB/CMYK ICC)".to_string())
    }
}

// ── 1. zenjxl-decoder (imazen pure-Rust SIMD decoder) ────────────────────────

const ZENJXL_VERSION: &str = "zenjxl-decoder 0.4.0 (025852d)";

fn zenjxl_limits(
    level: zenjxl_decoder::api::JxlCodestreamLevel,
) -> zenjxl_decoder::api::JxlDecoderLimits {
    let mut lim = zenjxl_decoder::api::JxlDecoderLimits::restrictive()
        .with_max_pixels(MAX_PIXELS as usize)
        .with_max_extra_channels(256)
        .with_max_tree_size(1 << 22)
        .with_max_reference_frames(4)
        .with_max_memory_bytes(MAX_CODEC_MEMORY as u64);
    lim.max_patches = None;
    lim.max_spline_points = Some(1 << 20);
    lim.max_codestream_level = level;
    lim
}

fn zenjxl_complete<T, U>(
    res: zenjxl_decoder::api::Result<zenjxl_decoder::api::ProcessingResult<T, U>>,
) -> Result<T, String> {
    match res.map_err(|e| e.to_string())? {
        zenjxl_decoder::api::ProcessingResult::Complete { result } => Ok(result),
        zenjxl_decoder::api::ProcessingResult::NeedsMoreInput { size_hint, .. } => Err(format!(
            "Truncated JPEG XL stream (needs {size_hint} more bytes)."
        )),
    }
}

fn run_zenjxl(bytes: &[u8], opts: &DecodeOptions) -> (DecodeReport, Vec<u8>) {
    use zenjxl_decoder::api::{
        ExtraChannel, JxlCodestreamLevel, JxlColorProfile, JxlDecoder, JxlDecoderOptions,
        JxlOutputBuffer, JxlPixelFormat, MoxCms,
    };

    let level5_accepted = if opts.probe_level5 {
        Some(
            zenjxl_decoder::read_header_with(bytes, zenjxl_limits(JxlCodestreamLevel::Level5))
                .is_ok(),
        )
    } else {
        None
    };

    let dec_opts = JxlDecoderOptions::default()
        .with_limits(zenjxl_limits(JxlCodestreamLevel::Level10))
        .with_cms(Box::new(MoxCms))
        .with_coalescing(true)
        .with_adjust_orientation(true)
        .with_skip_preview(true)
        .with_dither_u8(false);
    // Keep track of whether CMS succeeded
    let mut input = bytes;
    let mut decoder = match zenjxl_complete(JxlDecoder::new(dec_opts).process(&mut input)) {
        Ok(d) => d,
        Err(e) => {
            let mut r = DecodeReport::error("zenjxl-decoder", ZENJXL_VERSION, e);
            r.level5_accepted = level5_accepted;
            return (r, Vec::new());
        }
    };

    let info = decoder.basic_info().clone();
    let (width, height) = match check_dims(info.size.0, info.size.1) {
        Ok(d) => d,
        Err(e) => {
            let mut r = DecodeReport::error("zenjxl-decoder", ZENJXL_VERSION, e);
            r.level5_accepted = level5_accepted;
            return (r, Vec::new());
        }
    };

    let has_icc = matches!(decoder.embedded_color_profile(), JxlColorProfile::Icc(_));
    let is_cmyk = decoder.embedded_color_profile().is_cmyk()
        || info
            .extra_channels
            .iter()
            .any(|ch| ch.ec_type == ExtraChannel::Black);
    let has_alpha = info
        .extra_channels
        .iter()
        .any(|ch| ch.ec_type == ExtraChannel::Alpha);
    let alpha_associated = info
        .extra_channels
        .iter()
        .any(|ch| ch.ec_type == ExtraChannel::Alpha && ch.alpha_associated);
    let loop_count = info.animation.as_ref().map(|a| a.num_loops);
    let orientation = info.intrinsic_orientation as u8;
    let extra_channels = info.extra_channels.len() as u32;
    let uses_original_profile = info.uses_original_profile;

    decoder.set_pixel_format(JxlPixelFormat::rgba8(info.extra_channels.len()));

    let frame_bytes = (width as usize) * (height as usize) * 4;
    let mut first_pixels = Vec::new();
    let mut scratch = Vec::new();
    let mut frame_count = 0u32;
    let mut durations_ms = Vec::new();
    let mut warnings = Vec::new();

    while decoder.has_more_frames() {
        if frame_count as usize >= MAX_FRAMES {
            warnings.push(format!("Stopped after {MAX_FRAMES} frames (frame cap)."));
            break;
        }
        let frame = match zenjxl_complete(decoder.process(&mut input)) {
            Ok(f) => f,
            Err(e) => {
                let mut r = DecodeReport::error("zenjxl-decoder", ZENJXL_VERSION, e);
                r.width = width;
                r.height = height;
                r.frame_count = frame_count;
                r.frame_durations_ms = durations_ms;
                r.loop_count = loop_count;
                r.has_alpha = has_alpha;
                r.has_icc = has_icc;
                r.is_cmyk = is_cmyk;
                r.uses_original_profile = uses_original_profile;
                r.extra_channels = extra_channels;
                r.orientation = orientation;
                r.cms_used = "moxcms".into();
                r.level5_accepted = level5_accepted;
                r.warnings = warnings;
                return (r, Vec::new());
            }
        };
        let hdr = frame.frame_header();
        if info.animation.is_some() {
            durations_ms.push(hdr.duration.unwrap_or(0.0).round() as u32);
        }
        let buf_target = if frame_count == 0 {
            first_pixels.resize(frame_bytes, 0);
            &mut first_pixels
        } else {
            scratch.resize(frame_bytes, 0);
            &mut scratch
        };
        let mut bufs = [JxlOutputBuffer::new(
            buf_target,
            height as usize,
            (width as usize) * 4,
        )];
        decoder = match zenjxl_complete(frame.process(&mut input, &mut bufs)) {
            Ok(d) => d,
            Err(e) => {
                let mut r = DecodeReport::error("zenjxl-decoder", ZENJXL_VERSION, e);
                r.width = width;
                r.height = height;
                r.frame_count = frame_count;
                r.frame_durations_ms = durations_ms;
                r.loop_count = loop_count;
                r.has_alpha = has_alpha;
                r.has_icc = has_icc;
                r.is_cmyk = is_cmyk;
                r.uses_original_profile = uses_original_profile;
                r.extra_channels = extra_channels;
                r.orientation = orientation;
                r.cms_used = "moxcms".into();
                r.level5_accepted = level5_accepted;
                r.warnings = warnings;
                return (r, Vec::new());
            }
        };
        frame_count += 1;
    }

    if frame_count == 0 || first_pixels.len() != frame_bytes {
        let mut r = DecodeReport::error(
            "zenjxl-decoder",
            ZENJXL_VERSION,
            "zenjxl-decoder produced no visible frames.",
        );
        r.level5_accepted = level5_accepted;
        return (r, Vec::new());
    }

    if alpha_associated {
        unpremultiply_rgba8(&mut first_pixels);
    }

    let report = DecodeReport {
        decoder: "zenjxl-decoder".to_string(),
        version: ZENJXL_VERSION.to_string(),
        ok: true,
        error: None,
        width,
        height,
        frame_count,
        frame_durations_ms: durations_ms,
        loop_count,
        has_alpha,
        has_icc,
        is_cmyk,
        uses_original_profile,
        extra_channels,
        orientation,
        cms_used: "moxcms".to_string(),
        level5_accepted,
        warnings,
    };
    (report, first_pixels)
}

// ── 2. jxl-rs (official libjxl/jxl-rs 0.8.0 crate) ───────────────────────────

const JXL_RS_VERSION: &str = "jxl-rs 0.8.0";

fn jxl_rs_probe_level5(bytes: &[u8]) -> bool {
    let mut opts = jxl::api::JxlDecoderOptions::default();
    opts.max_profile_level = jxl::api::ProfileLevel::Main5;
    opts.scan_frames_only = true;
    opts.sample_limit = Some((MAX_PIXELS as usize).saturating_mul(4));
    let mut dec = jxl::api::JxlDecoder::new(opts);
    let mut input: &[u8] = bytes;
    loop {
        match dec.process(&mut input, None, None) {
            Ok(jxl::api::Event::BasicInfo)
            | Ok(jxl::api::Event::FrameHeader)
            | Ok(jxl::api::Event::FrameComplete { .. }) => continue,
            Ok(jxl::api::Event::Complete) => return true,
            Ok(jxl::api::Event::NeedMoreInput { .. }) | Err(_) => return false,
        }
    }
}

fn run_jxl_rs(bytes: &[u8], opts: &DecodeOptions) -> (DecodeReport, Vec<u8>) {
    use jxl::api::{
        AlphaMode, Event, ExtraChannel, JxlColorProfile, JxlColorType, JxlDataFormat, JxlDecoder,
        JxlDecoderOptions, JxlOutputBuffer, JxlPixelFormat, ProfileLevel,
    };

    let level5_accepted = if opts.probe_level5 {
        Some(jxl_rs_probe_level5(bytes))
    } else {
        None
    };

    let mut dec_opts = JxlDecoderOptions::default();
    dec_opts.adjust_orientation = true;
    dec_opts.coalescing = true;
    dec_opts.skip_preview = true;
    dec_opts.alpha_mode = AlphaMode::Unpremultiplied;
    dec_opts.max_profile_level = ProfileLevel::Main10;
    dec_opts.sample_limit = Some((MAX_PIXELS as usize).saturating_mul(4));

    let mut dec = JxlDecoder::new(dec_opts);
    let mut input: &[u8] = bytes;

    let mut width = 0u32;
    let mut height = 0u32;
    let mut frame_count = 0u32;
    let mut durations_ms = Vec::new();
    let mut loop_count = None;
    let mut has_alpha = false;
    let mut has_icc = false;
    let mut is_cmyk = false;
    let mut uses_original_profile = false;
    let mut extra_channels = 0u32;
    let mut orientation = 1u8;
    let mut embedded_icc: Option<Vec<u8>> = None;
    let mut is_animated = false;
    let mut warnings = Vec::new();

    let mut first_pixels = Vec::new();
    let mut scratch = Vec::new();
    let mut frame_bytes = 0usize;

    loop {
        let mut active_buf: Option<[JxlOutputBuffer<'_>; 1]> =
            if frame_bytes > 0 && frame_count < MAX_FRAMES as u32 {
                let target = if frame_count == 0 {
                    if first_pixels.len() != frame_bytes {
                        first_pixels.resize(frame_bytes, 0);
                    }
                    &mut first_pixels
                } else {
                    if scratch.len() != frame_bytes {
                        scratch.resize(frame_bytes, 0);
                    }
                    &mut scratch
                };
                Some([JxlOutputBuffer::new(
                    target,
                    height as usize,
                    (width as usize) * 4,
                )])
            } else {
                None
            };

        let event = match dec.process(&mut input, active_buf.as_mut().map(|b| &mut b[..]), None) {
            Ok(ev) => ev,
            Err(e) => {
                let mut r = DecodeReport::error("jxl-rs", JXL_RS_VERSION, e.to_string());
                r.width = width;
                r.height = height;
                r.frame_count = frame_count;
                r.frame_durations_ms = durations_ms;
                r.loop_count = loop_count;
                r.has_alpha = has_alpha;
                r.has_icc = has_icc;
                r.is_cmyk = is_cmyk;
                r.uses_original_profile = uses_original_profile;
                r.extra_channels = extra_channels;
                r.orientation = orientation;
                r.level5_accepted = level5_accepted;
                r.warnings = warnings;
                return (r, Vec::new());
            }
        };

        match event {
            Event::NeedMoreInput { size_hint } => {
                let mut r = DecodeReport::error(
                    "jxl-rs",
                    JXL_RS_VERSION,
                    format!("Truncated JPEG XL stream (needs {size_hint} more bytes)."),
                );
                r.width = width;
                r.height = height;
                r.frame_count = frame_count;
                r.level5_accepted = level5_accepted;
                return (r, Vec::new());
            }
            Event::BasicInfo => {
                let info = match dec.basic_info() {
                    Some(i) => i.clone(),
                    None => {
                        let mut r = DecodeReport::error(
                            "jxl-rs",
                            JXL_RS_VERSION,
                            "BasicInfo event fired without basic_info().",
                        );
                        r.level5_accepted = level5_accepted;
                        return (r, Vec::new());
                    }
                };
                match check_dims(info.size.0, info.size.1) {
                    Ok((w, h)) => {
                        width = w;
                        height = h;
                    }
                    Err(e) => {
                        let mut r = DecodeReport::error("jxl-rs", JXL_RS_VERSION, e);
                        r.level5_accepted = level5_accepted;
                        return (r, Vec::new());
                    }
                }
                orientation = info.orientation as u8;
                extra_channels = info.extra_channels.len() as u32;
                uses_original_profile = info.uses_original_profile;
                has_alpha = info
                    .extra_channels
                    .iter()
                    .any(|c| c.ec_type == ExtraChannel::Alpha);
                let has_black = info
                    .extra_channels
                    .iter()
                    .any(|c| c.ec_type == ExtraChannel::Black);
                if let Some(anim) = &info.animation {
                    is_animated = true;
                    loop_count = Some(anim.num_loops);
                }
                if let Some(JxlColorProfile::Icc(icc)) = dec.embedded_color_profile() {
                    has_icc = true;
                    embedded_icc = Some(icc.clone());
                }
                is_cmyk = has_black;

                let color_type = if is_cmyk {
                    JxlColorType::Cmyk
                } else {
                    JxlColorType::Rgba
                };
                let pixel_format = JxlPixelFormat {
                    color_type,
                    color_data_format: Some(JxlDataFormat::U8 { bit_depth: 8 }),
                    extra_channel_format: vec![None; info.extra_channels.len()],
                };
                if let Err(e) = dec.set_pixel_format(pixel_format) {
                    let mut r = DecodeReport::error("jxl-rs", JXL_RS_VERSION, e.to_string());
                    r.width = width;
                    r.height = height;
                    r.level5_accepted = level5_accepted;
                    return (r, Vec::new());
                }
                frame_bytes = (width as usize) * (height as usize) * 4;
            }
            Event::FrameHeader => {
                if let Some(fh) = dec.frame_header() {
                    if is_animated {
                        durations_ms.push(fh.duration.unwrap_or(0.0).round() as u32);
                    }
                }
            }
            Event::FrameComplete { has_more_frames } => {
                frame_count += 1;
                if frame_count as usize >= MAX_FRAMES && has_more_frames {
                    warnings.push(format!("Stopped after {MAX_FRAMES} frames (frame cap)."));
                    break;
                }
            }
            Event::Complete => break,
        }
    }

    if frame_count == 0 || first_pixels.len() != frame_bytes {
        let mut r = DecodeReport::error(
            "jxl-rs",
            JXL_RS_VERSION,
            "jxl-rs produced no visible frames.",
        );
        r.level5_accepted = level5_accepted;
        return (r, Vec::new());
    }

    let mut cms_used = "built-in".to_string();
    let apply_ext_cms = opts.apply_external_cms_for_jxl_rs.unwrap_or(true);
    if apply_ext_cms {
        if let Some(icc) = &embedded_icc {
            match apply_moxcms_icc_to_srgb_rgba8(icc, &mut first_pixels, is_cmyk) {
                Ok(note) => cms_used = note,
                Err(warn) => {
                    cms_used = "none (ICC skipped)".to_string();
                    warnings.push(warn);
                }
            }
        }
    } else if embedded_icc.is_some() {
        cms_used = "none (raw ICC samples)".to_string();
    }

    if level5_accepted == Some(false) {
        warnings.push("Requires Level 10 (rejected under strict Main Profile Level 5).".into());
    }

    let report = DecodeReport {
        decoder: "jxl-rs".to_string(),
        version: JXL_RS_VERSION.to_string(),
        ok: true,
        error: None,
        width,
        height,
        frame_count,
        frame_durations_ms: durations_ms,
        loop_count,
        has_alpha,
        has_icc,
        is_cmyk,
        uses_original_profile,
        extra_channels,
        orientation,
        cms_used,
        level5_accepted,
        warnings,
    };
    (report, first_pixels)
}

// ── 3. jxl-oxide (tirr-c pure-Rust decoder 0.12.6 + Moxcms) ──────────────────

const JXL_OXIDE_VERSION: &str = "jxl-oxide 0.12.6";

#[inline]
fn f32_to_u8(v: f32) -> u8 {
    if !v.is_finite() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn run_jxl_oxide(bytes: &[u8]) -> (DecodeReport, Vec<u8>) {
    use jxl_oxide::{EnumColourEncoding, JxlImage, PixelFormat, RenderingIntent};

    let mut image = match JxlImage::builder().read(Cursor::new(bytes)) {
        Ok(img) => img,
        Err(e) => {
            return (
                DecodeReport::error("jxl-oxide", JXL_OXIDE_VERSION, e.to_string()),
                Vec::new(),
            );
        }
    };

    let width = image.width();
    let height = image.height();
    if let Err(e) = check_dims(width as usize, height as usize) {
        return (
            DecodeReport::error("jxl-oxide", JXL_OXIDE_VERSION, e),
            Vec::new(),
        );
    }

    let hdr = image.image_header();
    let meta = &hdr.metadata;
    let orientation = meta.orientation as u8;
    let extra_channels = meta.ec_info.len() as u32;
    let uses_original_profile = !meta.xyb_encoded;
    let alpha_associated = meta
        .ec_info
        .iter()
        .any(|ec| ec.is_alpha() && ec.alpha_associated() == Some(true));
    let has_icc = image.original_icc().is_some();
    let is_cmyk = matches!(image.pixel_format(), PixelFormat::Cmyk | PixelFormat::Cmyka);
    let has_alpha = matches!(
        image.pixel_format(),
        PixelFormat::Graya | PixelFormat::Rgba | PixelFormat::Cmyka
    );
    let anim_info = meta
        .animation
        .as_ref()
        .map(|a| (a.tps_numerator, a.tps_denominator, a.num_loops));
    let loop_count = anim_info.map(|(_, _, loops)| loops);

    let mut warnings = Vec::new();
    let mut cms_used = "moxcms".to_string();
    // Ask jxl-oxide to render in sRGB so embedded ICC & CMYK profiles convert via Moxcms.
    image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));

    let num_keyframes = image.num_loaded_keyframes();
    if num_keyframes == 0 {
        return (
            DecodeReport::error(
                "jxl-oxide",
                JXL_OXIDE_VERSION,
                "jxl-oxide found 0 keyframes.",
            ),
            Vec::new(),
        );
    }

    let mut frame_count = 0u32;
    let mut durations_ms = Vec::new();
    let mut first_pixels = Vec::new();
    let out_fmt = image.pixel_format();

    for idx in 0..num_keyframes {
        if idx >= MAX_FRAMES {
            warnings.push(format!("Stopped after {MAX_FRAMES} frames (frame cap)."));
            break;
        }
        let render = match image.render_frame(idx) {
            Ok(r) => r,
            Err(e) => {
                let mut r = DecodeReport::error("jxl-oxide", JXL_OXIDE_VERSION, e.to_string());
                r.width = width;
                r.height = height;
                r.frame_count = frame_count;
                r.frame_durations_ms = durations_ms;
                r.loop_count = loop_count;
                r.has_alpha = has_alpha;
                r.has_icc = has_icc;
                r.is_cmyk = is_cmyk;
                r.uses_original_profile = uses_original_profile;
                r.extra_channels = extra_channels;
                r.orientation = orientation;
                r.cms_used = cms_used;
                r.warnings = warnings;
                return (r, Vec::new());
            }
        };
        if let Some((tps_num, tps_den, _)) = anim_info {
            let ms = if tps_num > 0 {
                ((render.duration() as f64) * (tps_den as f64) * 1000.0 / (tps_num as f64)).round()
                    as u32
            } else {
                0
            };
            durations_ms.push(ms);
        }
        if idx == 0 {
            let fb = render.image_all_channels();
            let rw = fb.width();
            let rh = fb.height();
            let ch = fb.channels();
            let buf = fb.buf();
            if rw != width as usize || rh != height as usize {
                warnings.push(format!(
                    "Rendered frame 0 dimensions ({rw}x{rh}) differ from header ({width}x{height})."
                ));
            }
            let pixel_count = rw * rh;
            first_pixels.resize(pixel_count * 4, 255);
            match out_fmt {
                PixelFormat::Gray => {
                    for i in 0..pixel_count {
                        let y = f32_to_u8(buf[i * ch]);
                        let o = i * 4;
                        first_pixels[o] = y;
                        first_pixels[o + 1] = y;
                        first_pixels[o + 2] = y;
                        first_pixels[o + 3] = 255;
                    }
                }
                PixelFormat::Graya => {
                    for i in 0..pixel_count {
                        let y = f32_to_u8(buf[i * ch]);
                        let a = if ch > 1 {
                            f32_to_u8(buf[i * ch + 1])
                        } else {
                            255
                        };
                        let o = i * 4;
                        first_pixels[o] = y;
                        first_pixels[o + 1] = y;
                        first_pixels[o + 2] = y;
                        first_pixels[o + 3] = a;
                    }
                }
                PixelFormat::Rgb => {
                    for i in 0..pixel_count {
                        let base = i * ch;
                        let o = i * 4;
                        first_pixels[o] = f32_to_u8(buf[base]);
                        first_pixels[o + 1] = f32_to_u8(buf[base + 1]);
                        first_pixels[o + 2] = f32_to_u8(buf[base + 2]);
                        first_pixels[o + 3] = 255;
                    }
                }
                PixelFormat::Rgba => {
                    for i in 0..pixel_count {
                        let base = i * ch;
                        let o = i * 4;
                        first_pixels[o] = f32_to_u8(buf[base]);
                        first_pixels[o + 1] = f32_to_u8(buf[base + 1]);
                        first_pixels[o + 2] = f32_to_u8(buf[base + 2]);
                        first_pixels[o + 3] = if ch > 3 {
                            f32_to_u8(buf[base + 3])
                        } else {
                            255
                        };
                    }
                }
                PixelFormat::Cmyk | PixelFormat::Cmyka => {
                    // Fallback if CMYK remained unconverted: naive CMYK->RGB approximation + warning
                    cms_used = "fallback-cmyk".to_string();
                    warnings.push("jxl-oxide output remained in CMYK pixel format; applied naive CMYK->sRGB preview.".into());
                    for i in 0..pixel_count {
                        let base = i * ch;
                        let c = buf[base].clamp(0.0, 1.0);
                        let m = buf[base + 1].clamp(0.0, 1.0);
                        let y = buf[base + 2].clamp(0.0, 1.0);
                        let k = if ch > 3 {
                            buf[base + 3].clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        let o = i * 4;
                        first_pixels[o] = f32_to_u8(c * k);
                        first_pixels[o + 1] = f32_to_u8(m * k);
                        first_pixels[o + 2] = f32_to_u8(y * k);
                        first_pixels[o + 3] = if matches!(out_fmt, PixelFormat::Cmyka) && ch > 4 {
                            f32_to_u8(buf[base + 4])
                        } else {
                            255
                        };
                    }
                }
            }
            // Unpremultiply if alpha is associated in metadata
            if alpha_associated {
                unpremultiply_rgba8(&mut first_pixels);
            }
        }
        frame_count += 1;
    }

    let report = DecodeReport {
        decoder: "jxl-oxide".to_string(),
        version: JXL_OXIDE_VERSION.to_string(),
        ok: true,
        error: None,
        width,
        height,
        frame_count,
        frame_durations_ms: durations_ms,
        loop_count,
        has_alpha,
        has_icc,
        is_cmyk,
        uses_original_profile,
        extra_channels,
        orientation,
        cms_used,
        level5_accepted: None,
        warnings,
    };
    (report, first_pixels)
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn decoders_catalog_json() -> String {
    serde_json::json!([
        {
            "id": "zenjxl-decoder",
            "label": "zenjxl-decoder",
            "version": ZENJXL_VERSION,
            "engine": "Rust WASM",
            "supportsLevel5Probe": true,
        },
        {
            "id": "jxl-rs",
            "label": "jxl-rs",
            "version": JXL_RS_VERSION,
            "engine": "Rust WASM",
            "supportsLevel5Probe": true,
        },
        {
            "id": "jxl-oxide",
            "label": "jxl-oxide",
            "version": JXL_OXIDE_VERSION,
            "engine": "Rust WASM",
            "supportsLevel5Probe": false,
        }
    ])
    .to_string()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn decode_with_decoder(
    decoder_id: &str,
    bytes: &[u8],
    options_json: &str,
) -> Result<DecoderOutput, String> {
    if bytes.is_empty() {
        return Err("Input JPEG XL buffer is empty.".into());
    }
    if bytes.len() > MAX_INPUT_BYTES {
        return Err("Input JPEG XL exceeds 128 MiB limit.".into());
    }
    let opts: DecodeOptions = if options_json.trim().is_empty() {
        DecodeOptions::default()
    } else {
        serde_json::from_str(options_json)
            .map_err(|e| format!("Invalid decode options JSON: {e}"))?
    };

    let (report, pixels) = match decoder_id {
        "zenjxl-decoder" => run_zenjxl(bytes, &opts),
        "jxl-rs" => run_jxl_rs(bytes, &opts),
        "jxl-oxide" => run_jxl_oxide(bytes),
        other => return Err(format!("Unknown Rust decoder '{other}'.")),
    };

    let json = serde_json::to_string(&report).map_err(|e| e.to_string())?;
    Ok(DecoderOutput {
        json,
        width: report.width,
        height: report.height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jxl_encoder::{AnimationFrame, AnimationParams, LosslessConfig, LossyConfig, PixelLayout};

    fn sample_rgba(w: usize, h: usize) -> Vec<u8> {
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                out[i] = ((x * 7 + 31) & 0xff) as u8;
                out[i + 1] = ((y * 9 + 17) & 0xff) as u8;
                out[i + 2] = (((x ^ y) * 5 + 40) & 0xff) as u8;
                out[i + 3] = if (x + y) % 5 == 0 { 180 } else { 255 };
            }
        }
        out
    }

    #[test]
    fn all_three_rust_decoders_match_lossless_modular_exactly() {
        let (w, h) = (37u32, 29u32);
        let rgba = sample_rgba(w as usize, h as usize);
        let jxl_bytes = LosslessConfig::new()
            .with_effort(3)
            .encode(&rgba, w, h, PixelLayout::Rgba8)
            .expect("encode lossless");

        for id in ["zenjxl-decoder", "jxl-rs", "jxl-oxide"] {
            let out = decode_with_decoder(id, &jxl_bytes, r#"{"probeLevel5":true}"#).expect(id);
            let meta: serde_json::Value = serde_json::from_str(&out.json()).unwrap();
            assert_eq!(meta["ok"], true, "{id} failed: {}", out.json());
            assert_eq!(out.width(), w, "{id} width");
            assert_eq!(out.height(), h, "{id} height");
            assert_eq!(meta["frameCount"], 1, "{id} frameCount");
            assert_eq!(out.into_pixels(), rgba, "{id} lossless pixel mismatch");
        }
    }

    #[test]
    fn all_three_rust_decoders_decode_vardct_and_animation() {
        let (w, h) = (48u32, 32u32);
        let f0 = sample_rgba(w as usize, h as usize);
        let mut f1 = f0.clone();
        for px in f1.chunks_exact_mut(4) {
            px[0] = px[0].wrapping_add(40);
            px[3] = 255;
        }
        // VarDCT lossy
        let vardct = LossyConfig::new(1.5)
            .with_effort(3)
            .encode(&f1, w, h, PixelLayout::Rgba8)
            .expect("encode vardct");
        for id in ["zenjxl-decoder", "jxl-rs", "jxl-oxide"] {
            let out = decode_with_decoder(id, &vardct, "{}").expect(id);
            let meta: serde_json::Value = serde_json::from_str(&out.json()).unwrap();
            assert_eq!(meta["ok"], true, "{id} vardct failed: {}", out.json());
            assert_eq!(out.into_pixels().len(), (w * h * 4) as usize);
        }

        // Animated 2-frame lossless
        let params = AnimationParams {
            tps_numerator: 1000,
            tps_denominator: 1,
            num_loops: 3,
            ..AnimationParams::default()
        };
        let frames = [AnimationFrame::new(&f0, 120), AnimationFrame::new(&f1, 240)];
        let anim = LosslessConfig::new()
            .with_effort(3)
            .encode_animation(w, h, PixelLayout::Rgba8, &params, &frames)
            .expect("encode animation");
        for id in ["zenjxl-decoder", "jxl-rs", "jxl-oxide"] {
            let out = decode_with_decoder(id, &anim, "{}").expect(id);
            let meta: serde_json::Value = serde_json::from_str(&out.json()).unwrap();
            assert_eq!(meta["ok"], true, "{id} anim failed: {}", out.json());
            assert_eq!(meta["frameCount"], 2, "{id} frameCount");
            assert_eq!(meta["loopCount"], 3, "{id} loopCount");
            assert_eq!(
                meta["frameDurationsMs"],
                serde_json::json!([120, 240]),
                "{id} durations"
            );
            assert_eq!(out.into_pixels(), f0, "{id} anim frame 0 exact");
        }
    }

    #[test]
    fn all_three_rust_decoders_report_error_cleanly_on_truncated_stream() {
        let broken = b"\x00\x00\x00\x0cJXL \x0d\x0a\x87\x0a\xff\x0a\x00";
        for id in ["zenjxl-decoder", "jxl-rs", "jxl-oxide"] {
            let out = decode_with_decoder(id, broken, "{}").expect(id);
            let meta: serde_json::Value = serde_json::from_str(&out.json()).unwrap();
            assert_eq!(meta["ok"], false, "{id} should fail on truncated stream");
            assert!(meta["error"].as_str().unwrap_or("").len() > 3);
        }
    }
}
