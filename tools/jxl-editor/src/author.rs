use jxl_encoder::{
    AnimationFrame, AnimationParams, BlendMode, ContainerMode, Limits, LosslessConfig, LossyConfig,
    PixelLayout, ProgressiveMode, RctType, api::EncoderStrategy,
};
use serde::Deserialize;

use crate::splines::{SplineSpec, render_splines_onto_rgba};
use crate::{MAX_CODEC_MEMORY, limits};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

const MAX_TIMELINE_FRAMES: usize = 64;
const MAX_TIMELINE_BYTES: usize = 64_000_000;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodingOptions {
    /// `"modular"` (lossless / near-lossless) or `"vardct"` (lossy).
    pub mode: Option<String>,
    /// Effort 1..=10 (default 3 for modular, 4 for vardct).
    pub effort: Option<u8>,
    /// VarDCT butteraugli distance 0.01..=15.0 (default 1.0).
    pub distance: Option<f32>,
    /// Container wrapping policy: `"auto"`, `"always"`, `"never"`.
    pub container_mode: Option<String>,
    /// Decoding speed tier 0..=4 (both modular and vardct).
    pub faster_decoding: Option<u8>,
    /// Preserve RGB values under transparent pixels (alpha = 0).
    pub keep_invisible: Option<bool>,
    /// Automatic delta/zero-add frame optimization (animation only).
    pub auto_delta_frames: Option<bool>,

    // ── Modular-only knobs ──
    /// Enable near-lossless delta palette with error diffusion.
    pub lossy_palette: Option<bool>,
    /// Palette cap (`None` = auto, `Some(0)` = off, `Some(1..=65536)`).
    pub palette_colors: Option<i64>,
    /// Responsive Haar wavelet squeeze transform.
    pub squeeze: Option<bool>,
    /// Force Reversible Color Transform: `"auto"`, `"none"`, `"subtract_green"`, `"ycocg"`, `"gbr_subgr"`.
    pub force_rct: Option<String>,
    /// Modular group size shift: `0` (128), `1` (256), `2` (512), `3` (1024).
    pub group_size_shift: Option<u8>,
    /// Modular predictor 0..=15 (`None` = auto).
    pub predictor: Option<u8>,
    /// Modular or VarDCT patch dictionary toggle (`None` = auto).
    pub patches: Option<bool>,

    // ── VarDCT-only knobs ──
    /// Progressive AC passes: `"single"`, `"quantized_ac"`, `"dc_vlf_lf_ac"`.
    pub progressive: Option<String>,
    /// Emit a separate LF/DC frame ahead of the main VarDCT frame (`0` or `1`).
    pub progressive_dc: Option<u8>,
    /// Enable center-first AC group ordering (effective on multi-group >256 px images).
    pub center_first: Option<bool>,
    /// Spatial resampling factor in opsin domain: `1`, `2`, `4`, or `8`.
    pub resampling: Option<u32>,
    /// Gaborish inverse pre-filter toggle (`None` = auto).
    pub gaborish: Option<bool>,
    /// Edge-preserving filter iterations: `-1` (auto), `0` (off), `1..=3`.
    pub epf_level: Option<i8>,
    /// Noise synthesis mode: `"off"`, `"content"`, `"photon_iso"`, `"manual_lut"`.
    pub noise_mode: Option<String>,
    /// ISO rating for `"photon_iso"` noise mode (e.g. 100..=50000).
    pub photon_noise_iso: Option<f32>,
    /// Manual 8-point noise LUT for `"manual_lut"` noise mode.
    pub manual_noise_lut: Option<[f32; 8]>,
    /// Wiener denoise pre-filter (implies noise synthesis).
    pub denoise: Option<bool>,
    /// Automatic thin-feature spline detection (active at effort >= 7 when manual splines unset).
    pub auto_splines: Option<bool>,
    /// Manual JXL splines (VarDCT still images only).
    pub splines: Option<Vec<SplineSpec>>,
    /// Bake authored splines into the source canvas before VarDCT subtraction (default true).
    pub bake_splines: Option<bool>,
    /// Lossy alpha distance (`None` or `0.0` = lossless alpha, `>0.0..=15.0`).
    pub alpha_distance: Option<f32>,
    /// Haar squeeze on lossy alpha (requires positive `alpha_distance`).
    pub alpha_squeeze: Option<bool>,
    /// VarDCT encoder strategy: `"zenjxl"`, `"libjxl"`, `"lean_faster"`.
    pub strategy: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TimelineFrameSpec {
    pub name: Option<String>,
    pub duration: u32,
    pub timecode: Option<u32>,
    pub blend_mode: Option<String>,
    pub blend_source: Option<u32>,
    pub save_as_reference: Option<u32>,
    pub reference_only: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TimelineOptions {
    pub tps_numerator: Option<u32>,
    pub tps_denominator: Option<u32>,
    pub loops: Option<u32>,
    pub frames: Vec<TimelineFrameSpec>,
    pub coding: CodingOptions,
}

fn parse_container_mode(value: Option<&str>) -> Result<ContainerMode, String> {
    match value.unwrap_or("auto") {
        "auto" => Ok(ContainerMode::Auto),
        "always" => Ok(ContainerMode::Always),
        "never" => Ok(ContainerMode::Never),
        other => Err(format!("Unsupported container mode '{other}'.")),
    }
}

fn parse_rct(value: Option<&str>) -> Result<Option<RctType>, String> {
    match value {
        None | Some("auto") | Some("") => Ok(None),
        Some("none") => Ok(Some(RctType::NONE)),
        Some("subtract_green") => Ok(Some(RctType::SUBTRACT_GREEN)),
        Some("ycocg") => Ok(Some(RctType::YCOCG)),
        Some("gbr_subgr") => Ok(Some(RctType::GBR_SUBGR)),
        Some(other) => Err(format!("Unsupported RCT transform '{other}'.")),
    }
}

fn parse_progressive(value: Option<&str>) -> Result<ProgressiveMode, String> {
    match value.unwrap_or("single") {
        "single" => Ok(ProgressiveMode::Single),
        "quantized_ac" => Ok(ProgressiveMode::QuantizedAcFullAc),
        "dc_vlf_lf_ac" => Ok(ProgressiveMode::DcVlfLfAc),
        other => Err(format!("Unsupported progressive mode '{other}'.")),
    }
}

fn parse_strategy(value: Option<&str>) -> Result<EncoderStrategy, String> {
    match value.unwrap_or("zenjxl") {
        "zenjxl" => Ok(EncoderStrategy::Zenjxl),
        "libjxl" => Ok(EncoderStrategy::Libjxl),
        "lean_faster" => Ok(EncoderStrategy::LeanFaster),
        other => Err(format!("Unsupported encoder strategy '{other}'.")),
    }
}

fn parse_blend_mode(value: Option<&str>) -> Result<Option<BlendMode>, String> {
    match value {
        None | Some("auto") | Some("") => Ok(None),
        Some("replace") => Ok(Some(BlendMode::Replace)),
        Some("add") => Ok(Some(BlendMode::Add)),
        Some("blend") => Ok(Some(BlendMode::Blend)),
        Some("alpha_weighted_add") => Ok(Some(BlendMode::AlphaWeightedAdd)),
        Some("mul") => Ok(Some(BlendMode::Mul)),
        Some(other) => Err(format!("Unsupported frame blend mode '{other}'.")),
    }
}

fn build_lossless_config(
    opts: &CodingOptions,
    is_animation: bool,
) -> Result<LosslessConfig, String> {
    if opts.splines.as_ref().is_some_and(|s| !s.is_empty()) {
        return Err("Manual JPEG XL splines require VarDCT lossy coding, not Modular.".into());
    }
    let effort = opts.effort.unwrap_or(3);
    if !(1..=10).contains(&effort) {
        return Err("Modular effort must be between 1 and 10.".into());
    }
    let faster = opts.faster_decoding.unwrap_or(0);
    if faster > 4 {
        return Err("Faster decoding tier must be between 0 and 4.".into());
    }
    let mut cfg = LosslessConfig::new()
        .with_effort(effort)
        .with_faster_decoding(faster)
        .with_keep_invisible(opts.keep_invisible.unwrap_or(true))
        .with_container_mode(parse_container_mode(opts.container_mode.as_deref())?);
    if let Some(lossy_palette) = opts.lossy_palette {
        cfg = cfg.with_lossy_palette(lossy_palette);
    }
    if let Some(cap) = opts.palette_colors {
        if !(0..=65_536).contains(&cap) {
            return Err("Modular palette color cap must be between 0 and 65536.".into());
        }
        cfg = cfg.with_modular_palette_colors(Some(cap));
    }
    if let Some(squeeze) = opts.squeeze {
        cfg = cfg.with_squeeze(squeeze);
    }
    if let Some(rct) = parse_rct(opts.force_rct.as_deref())? {
        cfg = cfg.with_force_rct(Some(rct));
    }
    if let Some(shift) = opts.group_size_shift {
        if shift > 3 {
            return Err("Modular group size shift must be 0..=3.".into());
        }
        cfg = cfg.with_modular_group_size(Some(shift));
    }
    if let Some(pred) = opts.predictor {
        if pred > 15 {
            return Err("Modular predictor must be 0..=15.".into());
        }
        cfg = cfg.with_modular_predictor(Some(pred));
    }
    if let Some(patches) = opts.patches {
        cfg = cfg.with_patches(patches);
    }
    if is_animation && let Some(auto_delta) = opts.auto_delta_frames {
        cfg = cfg.with_auto_delta_frames(auto_delta);
    }
    Ok(cfg)
}

fn build_lossy_config(opts: &CodingOptions, is_animation: bool) -> Result<LossyConfig, String> {
    let distance = opts.distance.unwrap_or(1.0);
    if !distance.is_finite() || !(0.01..=15.0).contains(&distance) {
        return Err("VarDCT distance must be between 0.01 and 15.0.".into());
    }
    let effort = opts.effort.unwrap_or(4);
    if !(1..=10).contains(&effort) {
        return Err("VarDCT effort must be between 1 and 10.".into());
    }
    let faster = opts.faster_decoding.unwrap_or(0);
    if faster > 4 {
        return Err("Faster decoding tier must be between 0 and 4.".into());
    }
    let resampling = opts.resampling.unwrap_or(1);
    if !matches!(resampling, 1 | 2 | 4 | 8) {
        return Err("Spatial resampling must be 1, 2, 4 or 8.".into());
    }
    let progressive_dc = opts.progressive_dc.unwrap_or(0);
    if progressive_dc > 1 {
        return Err("Progressive DC LF frame level must be 0 or 1.".into());
    }
    if is_animation {
        if opts.splines.as_ref().is_some_and(|s| !s.is_empty()) {
            return Err("Manual JPEG XL splines are not supported in animations.".into());
        }
        if progressive_dc > 0 {
            return Err("Progressive DC LF frames are not supported in animations.".into());
        }
        if resampling > 1 {
            return Err("Spatial resampling > 1 is not supported in animations.".into());
        }
    }
    if progressive_dc > 0 && resampling > 1 {
        return Err(
            "Progressive DC LF frame cannot be combined with spatial resampling > 1.".into(),
        );
    }

    let mut cfg = LossyConfig::new(distance)
        .with_effort(effort)
        .with_faster_decoding(faster)
        .with_progressive(parse_progressive(opts.progressive.as_deref())?)
        .with_progressive_dc(progressive_dc)
        .with_resampling(resampling)
        .with_center_first(opts.center_first.unwrap_or(false))
        .with_keep_invisible(opts.keep_invisible.unwrap_or(false))
        .with_strategy(parse_strategy(opts.strategy.as_deref())?)
        .with_container_mode(parse_container_mode(opts.container_mode.as_deref())?);

    if let Some(patches) = opts.patches {
        cfg = cfg.with_patches(patches);
    }
    if let Some(gaborish) = opts.gaborish {
        cfg = cfg.with_gaborish(gaborish);
    }
    if let Some(epf) = opts.epf_level {
        if !(-1..=3).contains(&epf) {
            return Err("EPF level must be between -1 and 3.".into());
        }
        cfg = cfg.with_epf_level(epf);
    }
    match opts.noise_mode.as_deref().unwrap_or("off") {
        "off" => {}
        "content" => {
            cfg = cfg.with_noise(true);
        }
        "photon_iso" => {
            let iso = opts.photon_noise_iso.unwrap_or(1600.0);
            if !iso.is_finite() || !(1.0..=50_000.0).contains(&iso) {
                return Err("Photon noise ISO must be between 1 and 50000.".into());
            }
            cfg = cfg.with_photon_noise_iso(Some(iso));
        }
        "manual_lut" => {
            let lut = opts
                .manual_noise_lut
                .ok_or_else(|| "Manual noise LUT requires 8 finite values.".to_string())?;
            if !lut.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)) {
                return Err("Manual noise LUT entries must be finite values in 0.0..=1.0.".into());
            }
            if lut.iter().all(|&v| v == 0.0) {
                return Err("Manual noise LUT must contain at least one positive value.".into());
            }
            cfg = cfg.with_manual_noise_lut(Some(lut));
        }
        other => return Err(format!("Unsupported noise mode '{other}'.")),
    }
    if opts.denoise.unwrap_or(false) {
        cfg = cfg.with_denoise(true);
    }
    if let Some(auto_splines) = opts.auto_splines {
        cfg = cfg.with_auto_splines(auto_splines);
    }
    if let Some(specs) = &opts.splines
        && !specs.is_empty()
    {
        let splines = specs
            .iter()
            .map(SplineSpec::to_encoder_spline)
            .collect::<Result<Vec<_>, _>>()?;
        cfg = cfg.with_splines(splines);
    }
    if let Some(ad) = opts.alpha_distance {
        if !ad.is_finite() || !(0.0..=15.0).contains(&ad) {
            return Err("Alpha distance must be between 0.0 and 15.0.".into());
        }
        if ad > 0.0 {
            cfg = cfg.with_alpha_distance(Some(ad));
            if opts.alpha_squeeze.unwrap_or(false) {
                cfg = cfg.with_alpha_squeeze(true);
            }
        }
    }
    if is_animation && let Some(auto_delta) = opts.auto_delta_frames {
        cfg = cfg.with_auto_delta_frames(auto_delta);
    }
    Ok(cfg)
}

/// Encode a still RGBA8 image using explicit Modular or VarDCT coding options,
/// including manual JPEG XL splines (with optional baking onto the target source).
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn encode_jxl_configured(
    pixels: &[u8],
    width: u32,
    height: u32,
    options_json: &str,
) -> Result<Vec<u8>, String> {
    limits()
        .check_dimensions(width, height)
        .map_err(|e| e.to_string())?;
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "Image byte size overflow.".to_string())?;
    if expected == 0 || pixels.len() != expected {
        return Err("Pixel buffer length does not match image dimensions.".into());
    }
    let opts: CodingOptions = if options_json.trim().is_empty() {
        CodingOptions::default()
    } else {
        serde_json::from_str(options_json)
            .map_err(|e| format!("Invalid coding options JSON: {e}"))?
    };
    let codec_limits = Limits::default().with_max_memory_bytes(MAX_CODEC_MEMORY);
    match opts.mode.as_deref().unwrap_or("modular") {
        "modular" => {
            let cfg = build_lossless_config(&opts, false)?;
            cfg.encode_request(width, height, PixelLayout::Rgba8)
                .with_limits(&codec_limits)
                .encode(pixels)
                .map_err(|e| e.to_string())
        }
        "vardct" => {
            let cfg = build_lossy_config(&opts, false)?;
            let baked: Vec<u8>;
            let source_pixels: &[u8] = if opts.bake_splines.unwrap_or(true)
                && let Some(specs) = &opts.splines
                && !specs.is_empty()
            {
                let splines = specs
                    .iter()
                    .map(SplineSpec::to_encoder_spline)
                    .collect::<Result<Vec<_>, _>>()?;
                baked = render_splines_onto_rgba(pixels, width, height, &splines)?;
                &baked
            } else {
                pixels
            };
            cfg.encode_request(width, height, PixelLayout::Rgba8)
                .with_limits(&codec_limits)
                .encode(source_pixels)
                .map_err(|e| e.to_string())
        }
        other => Err(format!("Unsupported coding mode '{other}'.")),
    }
}

/// Encode a native JPEG XL multi-frame animation timeline with full control over
/// timebase, loops, per-frame names, durations, timecodes, blend modes, reference
/// slots, reference-only frames, and Modular/VarDCT coding options.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn encode_timeline_configured(
    pixels: &[u8],
    width: u32,
    height: u32,
    options_json: &str,
) -> Result<Vec<u8>, String> {
    limits()
        .check_dimensions(width, height)
        .map_err(|e| e.to_string())?;
    let opts: TimelineOptions = serde_json::from_str(options_json)
        .map_err(|e| format!("Invalid timeline options JSON: {e}"))?;
    let frame_bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "Animation frame byte size overflow.".to_string())?;
    if frame_bytes == 0
        || opts.frames.is_empty()
        || opts.frames.len() > MAX_TIMELINE_FRAMES
        || pixels.len() > MAX_TIMELINE_BYTES
        || frame_bytes.checked_mul(opts.frames.len()) != Some(pixels.len())
    {
        return Err("Invalid animation frame count or pixel budget.".into());
    }
    let tps_num = opts.tps_numerator.unwrap_or(1000);
    let tps_den = opts.tps_denominator.unwrap_or(1);
    let loops = opts.loops.unwrap_or(0);
    if !(1..=(1u32 << 30)).contains(&tps_num) || !(1..=1024).contains(&tps_den) || loops > 65_535 {
        return Err("Invalid animation timebase or loop count.".into());
    }
    if opts.frames.last().is_some_and(|f| f.reference_only) {
        return Err("The last timeline frame cannot be reference-only.".into());
    }
    if opts.frames.iter().all(|f| f.reference_only) {
        return Err("A timeline needs at least one displayable frame.".into());
    }

    let mut frames = Vec::with_capacity(opts.frames.len());
    let mut last_timecode = 0u32;
    for (index, (slice, spec)) in pixels
        .chunks_exact(frame_bytes)
        .zip(opts.frames.iter())
        .enumerate()
    {
        if spec.duration > 0x7FFF_FFFF {
            return Err(format!(
                "Frame {} duration exceeds supported range.",
                index + 1
            ));
        }
        if let Some(ref name) = spec.name
            && name.len() > 1019
        {
            return Err(format!("Frame {} name exceeds 1019 bytes.", index + 1));
        }
        if let Some(slot) = spec.blend_source
            && slot > 3
        {
            return Err(format!(
                "Frame {} blend source slot must be 0..=3.",
                index + 1
            ));
        }
        if let Some(slot) = spec.save_as_reference
            && slot > 3
        {
            return Err(format!(
                "Frame {} save reference slot must be 0..=3.",
                index + 1
            ));
        }
        if !spec.reference_only
            && let Some(tc) = spec.timecode
        {
            if tc != 0 && tc <= last_timecode {
                return Err(format!(
                    "Frame {} timecode {tc} must exceed previous displayed timecode {last_timecode}.",
                    index + 1
                ));
            }
            if spec.duration != 0 {
                last_timecode = last_timecode.max(tc);
            }
        }
        let mut frame = AnimationFrame::new(
            slice,
            if spec.reference_only {
                0
            } else {
                spec.duration
            },
        );
        if let Some(ref name) = spec.name
            && !name.is_empty()
        {
            frame = frame.with_name(name.clone());
        }
        if let Some(tc) = spec.timecode
            && !spec.reference_only
        {
            frame = frame.with_timecode(tc);
        }
        if let Some(mode) = parse_blend_mode(spec.blend_mode.as_deref())? {
            frame = frame.with_blend_mode(mode);
        }
        if let Some(src) = spec.blend_source {
            frame = frame.with_blend_source(src);
        }
        if let Some(slot) = spec.save_as_reference {
            frame = frame.with_save_as_reference(slot);
        }
        if spec.reference_only {
            frame = frame.with_reference_only(true);
        }
        frames.push(frame);
    }

    let params = AnimationParams {
        tps_numerator: tps_num,
        tps_denominator: tps_den,
        num_loops: loops,
        ..AnimationParams::default()
    };
    let memory = Limits::default().with_max_memory_bytes(MAX_CODEC_MEMORY);
    match opts.coding.mode.as_deref().unwrap_or("modular") {
        "modular" => {
            let cfg = build_lossless_config(&opts.coding, true)?;
            cfg.encode_animation_with_limits(
                width,
                height,
                PixelLayout::Rgba8,
                &params,
                &frames,
                &memory,
            )
            .map_err(|e| e.to_string())
        }
        "vardct" => {
            let cfg = build_lossy_config(&opts.coding, true)?;
            cfg.encode_animation_with_limits(
                width,
                height,
                PixelLayout::Rgba8,
                &params,
                &frames,
                &memory,
            )
            .map_err(|e| e.to_string())
        }
        other => Err(format!("Unsupported animation coding mode '{other}'.")),
    }
}

/// Simple native timeline binding preserved from the initial timeline WIP.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn encode_timeline(
    pixels: &[u8],
    width: u32,
    height: u32,
    durations: &[u32],
    loops: u32,
    mode: &str,
    distance: f32,
) -> Result<Vec<u8>, String> {
    limits()
        .check_dimensions(width, height)
        .map_err(|error| error.to_string())?;
    let frame_bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "Animation frame byte size overflow.".to_string())?;
    if frame_bytes == 0
        || durations.is_empty()
        || durations.len() > 24
        || pixels.len() > MAX_TIMELINE_BYTES
        || frame_bytes.checked_mul(durations.len()) != Some(pixels.len())
        || durations
            .iter()
            .any(|duration| !(1..=60_000).contains(duration))
        || loops > 65_535
    {
        return Err("Invalid animation frames, duration, loop count or pixel budget.".into());
    }
    let frames: Vec<_> = pixels
        .chunks_exact(frame_bytes)
        .zip(durations)
        .enumerate()
        .map(|(index, (pixels, ticks))| {
            AnimationFrame::new(pixels, *ticks).with_name(format!("Frame {}", index + 1))
        })
        .collect();
    let params = AnimationParams {
        tps_numerator: 1000,
        num_loops: loops,
        ..AnimationParams::default()
    };
    let memory = Limits::default().with_max_memory_bytes(MAX_CODEC_MEMORY);
    let encoded = match mode {
        "modular" => LosslessConfig::new()
            .with_effort(3)
            .with_keep_invisible(true)
            .encode_animation_with_limits(
                width,
                height,
                PixelLayout::Rgba8,
                &params,
                &frames,
                &memory,
            ),
        "vardct" if (0.1..=15.0).contains(&distance) => LossyConfig::new(distance)
            .with_effort(4)
            .encode_animation_with_limits(
                width,
                height,
                PixelLayout::Rgba8,
                &params,
                &frames,
                &memory,
            ),
        _ => return Err("Choose a supported animation coding mode and distance.".into()),
    };
    encoded.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenjxl_decoder::api::{
        JxlDecoder, JxlDecoderOptions, JxlOutputBuffer, JxlPixelFormat, MoxCms, ProcessingResult,
    };

    fn decode_all_rgba_frames(bytes: &[u8]) -> Vec<(String, f64, Vec<u8>)> {
        let mut input = bytes;
        let opts = JxlDecoderOptions::default()
            .with_limits(crate::decoder_limits())
            .with_coalescing(true)
            .with_cms(Box::new(MoxCms))
            .with_dither_u8(false);
        let ProcessingResult::Complete { result: mut dec } =
            JxlDecoder::new(opts).process(&mut input).unwrap()
        else {
            panic!("truncated header");
        };
        let (w, h) = dec.basic_info().size;
        let num_ec = dec.basic_info().extra_channels.len();
        dec.set_pixel_format(JxlPixelFormat::rgba8(num_ec));
        let mut frames = Vec::new();
        while dec.has_more_frames() {
            let ProcessingResult::Complete { result: frame } = dec.process(&mut input).unwrap()
            else {
                panic!("truncated frame header");
            };
            let hdr = frame.frame_header();
            let mut rgba = vec![0u8; w * h * 4];
            let mut bufs = [JxlOutputBuffer::new(&mut rgba, h, w * 4)];
            let ProcessingResult::Complete { result: next } =
                frame.process(&mut input, &mut bufs).unwrap()
            else {
                panic!("truncated frame pixels");
            };
            frames.push((hdr.name, hdr.duration.unwrap_or(0.0), rgba));
            dec = next;
        }
        frames
    }

    #[test]
    fn authored_timeline_preserves_millisecond_clock_and_all_frames() {
        let first = [25, 88, 115, 255].repeat(17 * 11);
        let second = [85, 33, 60, 255].repeat(17 * 11);
        let joined = [first.clone(), second.clone()].concat();
        let file = encode_timeline(&joined, 17, 11, &[180, 420], 3, "modular", 1.0).unwrap();
        let header = zenjxl_decoder::read_header_with(&file, crate::decoder_limits()).unwrap();
        let animation = header.info.animation.unwrap();
        assert_eq!(
            (
                animation.tps_numerator,
                animation.tps_denominator,
                animation.num_loops
            ),
            (1000, 1, 3)
        );
        let decoded = decode_all_rgba_frames(&file);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].0, "Frame 1");
        assert!((decoded[0].1 - 180.0).abs() < 0.01);
        assert_eq!(decoded[0].2, first);
        assert_eq!(decoded[1].0, "Frame 2");
        assert!((decoded[1].1 - 420.0).abs() < 0.01);
        assert_eq!(decoded[1].2, second);
        assert!(encode_timeline(&joined, 17, 11, &[0, 420], 3, "modular", 1.0).is_err());
    }

    #[test]
    fn configured_timeline_supports_timebase_timecodes_reference_only_and_blending() {
        let w = 16u32;
        let h = 12u32;
        let bg = [20u8, 40, 80, 255].repeat((w * h) as usize);
        let mut delta = [0u8, 0, 0, 255].repeat((w * h) as usize);
        for y in 2..6 {
            for x in 3..9 {
                let o = ((y * w + x) * 4) as usize;
                delta[o..o + 4].copy_from_slice(&[60, 30, 15, 255]);
            }
        }
        let joined = [bg.clone(), delta.clone()].concat();
        let opts = r#"{
            "tpsNumerator": 24,
            "tpsDenominator": 1,
            "loops": 2,
            "frames": [
                {"name":"BG Ref","duration":0,"referenceOnly":true,"saveAsReference":1},
                {"name":"Composite","duration":12,"timecode":256,"blendMode":"add","blendSource":1}
            ],
            "coding":{"mode":"modular","effort":3,"autoDeltaFrames":true}
        }"#;
        let file = encode_timeline_configured(&joined, w, h, opts).unwrap();
        let header = zenjxl_decoder::read_header_with(&file, crate::decoder_limits()).unwrap();
        let anim = header.info.animation.unwrap();
        assert_eq!(
            (anim.tps_numerator, anim.tps_denominator, anim.num_loops),
            (24, 1, 2)
        );
        assert!(anim.have_timecodes);
        let decoded = decode_all_rgba_frames(&file);
        // ReferenceOnly frame is NOT displayed; only the composited regular frame is visible.
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].0, "Composite");
        assert!((decoded[0].1 - 500.0).abs() < 0.01);
        // Pixel (0,0) kept BG [20, 40, 80, 255] + [0, 0, 0]; (4,3) added [60, 30, 15] -> [80, 70, 95, 255].
        assert_eq!(&decoded[0].2[0..4], &[20, 40, 80, 255]);
        let hit = ((3 * w + 4) * 4) as usize;
        assert_eq!(&decoded[0].2[hit..hit + 4], &[80, 70, 95, 255]);

        // Invalid: last frame referenceOnly must fail.
        let bad = r#"{"frames":[{"duration":10,"referenceOnly":true}]}"#;
        assert!(encode_timeline_configured(&bg, w, h, bad).is_err());
        // Invalid: non-increasing nonzero timecodes must fail.
        let bad_tc =
            r#"{"frames":[{"duration":10,"timecode":500},{"duration":10,"timecode":500}]}"#;
        assert!(encode_timeline_configured(&joined, w, h, bad_tc).is_err());
        // Invalid: manual splines rejected in animation.
        let bad_splines = r#"{
            "frames":[{"duration":100},{"duration":100}],
            "coding":{"mode":"vardct","distance":1.0,"splines":[{"controlPoints":[[1,1],[8,8]],"colorDct":[[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]],"sigmaDct":[1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}]}
        }"#;
        assert!(encode_timeline_configured(&joined, w, h, bad_splines).is_err());
    }

    #[test]
    fn configured_coding_knobs_produce_distinct_valid_codestreams() {
        let w = 64u32;
        let h = 48u32;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&[
                    ((x * 4 + y) % 256) as u8,
                    ((y * 5 + 30) % 256) as u8,
                    ((x * 2 + y * 3) % 256) as u8,
                    if (x + y) % 7 == 0 { 200 } else { 255 },
                ]);
            }
        }

        // Modular lossless + squeeze + lossy palette + RCT + group_size_shift
        let mod_base =
            encode_jxl_configured(&pixels, w, h, r#"{"mode":"modular","effort":3}"#).unwrap();
        assert_eq!(crate::decode_jxl(&mod_base).unwrap().into_pixels(), pixels);
        let mod_sq = encode_jxl_configured(
            &pixels,
            w,
            h,
            r#"{"mode":"modular","effort":3,"squeeze":true,"groupSizeShift":0,"forceRct":"ycocg"}"#,
        )
        .unwrap();
        assert_ne!(mod_sq, mod_base);
        assert_eq!(crate::decode_jxl(&mod_sq).unwrap().into_pixels(), pixels);
        let mod_lp = encode_jxl_configured(
            &pixels,
            w,
            h,
            r#"{"mode":"modular","effort":3,"lossyPalette":true}"#,
        )
        .unwrap();
        assert_ne!(mod_lp, mod_base);
        assert_eq!(
            crate::decode_jxl(&mod_lp).unwrap().into_pixels().len(),
            pixels.len()
        );

        // VarDCT base vs progressive AC vs LF frame vs photon noise vs manual noise vs resampling vs splines
        let v_base = encode_jxl_configured(
            &pixels,
            w,
            h,
            r#"{"mode":"vardct","distance":1.5,"effort":4}"#,
        )
        .unwrap();
        let v_prog = encode_jxl_configured(&pixels, w, h, r#"{"mode":"vardct","distance":1.5,"effort":4,"progressive":"dc_vlf_lf_ac","progressiveDc":1}"#).unwrap();
        let v_noise = encode_jxl_configured(&pixels, w, h, r#"{"mode":"vardct","distance":1.5,"effort":4,"noiseMode":"photon_iso","photonNoiseIso":3200}"#).unwrap();
        let v_lut = encode_jxl_configured(&pixels, w, h, r#"{"mode":"vardct","distance":1.5,"effort":4,"noiseMode":"manual_lut","manualNoiseLut":[0.02,0.03,0.03,0.025,0.02,0.015,0.01,0.005]}"#).unwrap();
        let v_resamp = encode_jxl_configured(
            &pixels,
            w,
            h,
            r#"{"mode":"vardct","distance":1.5,"effort":4,"resampling":2}"#,
        )
        .unwrap();
        let v_epf0 = encode_jxl_configured(
            &pixels,
            w,
            h,
            r#"{"mode":"vardct","distance":1.5,"effort":4,"gaborish":false,"epfLevel":0}"#,
        )
        .unwrap();

        for (label, bytes) in [
            ("v_prog", &v_prog),
            ("v_noise", &v_noise),
            ("v_lut", &v_lut),
            ("v_resamp", &v_resamp),
            ("v_epf0", &v_epf0),
        ] {
            assert_ne!(
                bytes, &v_base,
                "{label} should differ from base VarDCT codestream"
            );
            let dec = crate::decode_jxl(bytes).unwrap();
            assert_eq!((dec.width(), dec.height()), (w, h), "{label}");
        }

        // Manual spline baked + subtracted + decoded visibly
        let dark = [20u8, 26, 38, 255].repeat((w * h) as usize);
        let dct =
            crate::splines::spline_color_dct_from_srgb(245, 225, 65, 20, 26, 38, 1.8).unwrap();
        let spline_opts = serde_json::json!({
            "mode": "vardct",
            "distance": 1.0,
            "effort": 4,
            "bakeSplines": true,
            "splines": [{
                "controlPoints": [[8.0, 24.0], [32.0, 14.0], [56.0, 24.0]],
                "colorDct": [
                    [dct[0],0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                    [dct[1],0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
                    [dct[2],0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
                ],
                "sigmaDct": [dct[3],0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
            }]
        })
        .to_string();
        let v_spline = encode_jxl_configured(&dark, w, h, &spline_opts).unwrap();
        let dec_spline = crate::decode_jxl(&v_spline).unwrap().into_pixels();
        let mid = ((14 * w + 32) * 4) as usize;
        assert!(
            dec_spline[mid] > 160 && dec_spline[mid + 1] > 140,
            "decoded spline pixel should be bright yellow, got {:?}",
            &dec_spline[mid..mid + 4]
        );
        // Corner remains dark background.
        let corner = ((2 * w + 2) * 4) as usize;
        assert!(dec_spline[corner] < 40 && dec_spline[corner + 1] < 45);
    }
}
