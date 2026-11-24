#![forbid(unsafe_code)]

mod inspect;

use core::fmt::Display;
use core::num::NonZeroU32;

use serde_json::{Value, json};
use zenjxl_decoder::api::{
    Endianness, ExtraChannel, JxlCms, JxlCodestreamLevel, JxlColorEncoding, JxlColorProfile,
    JxlColorType, JxlDataFormat, JxlDecoder, JxlDecoderLimits, JxlDecoderOptions, JxlFrameHeader,
    JxlLayerBlend, JxlLayerInfo, JxlOutputBuffer, JxlPixelFormat, MoxCms, Orientation,
    ProcessingResult, states,
};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

const MAX_NATIVE_FRAMES: usize = 128;
const MAX_NATIVE_SAMPLES: usize = 32_000_000;
const MAX_TIMELINE_DECODE_FRAMES: usize = 64;
const MAX_TIMELINE_DECODE_BYTES: usize = 128_000_000;

fn complete<T, U, E: Display>(step: Result<ProcessingResult<T, U>, E>) -> Result<T, String> {
    match step.map_err(|error| error.to_string())? {
        ProcessingResult::Complete { result } => Ok(result),
        ProcessingResult::NeedsMoreInput { .. } => Err("The JPEG XL source is truncated.".into()),
    }
}

fn source_limits() -> JxlDecoderLimits {
    let mut limits = JxlDecoderLimits::restrictive()
        .with_max_pixels(64_000_000)
        .with_max_extra_channels(256)
        .with_max_tree_size(1 << 22)
        .with_max_reference_frames(4)
        .with_max_memory_bytes(1024 * 1024 * 1024)
        .with_max_frame_headers(MAX_NATIVE_FRAMES as u64);
    limits.max_patches = None;
    limits.max_spline_points = Some(1 << 20);
    limits.max_codestream_level = JxlCodestreamLevel::Level10;
    limits
}

fn initialize(
    bytes: &mut &[u8],
    headers_only: bool,
) -> Result<JxlDecoder<states::WithImageInfo>, String> {
    if bytes.len() > 128 * 1024 * 1024 {
        return Err("The JPEG XL source exceeds 128 MiB.".into());
    }
    let options = JxlDecoderOptions::default()
        .with_limits(source_limits())
        .with_coalescing(false)
        .with_adjust_orientation(false)
        .with_skip_preview(true)
        .with_high_precision(true)
        .with_render_spot_colors(false)
        .with_scan_frames_only(headers_only);
    let decoder = complete(JxlDecoder::new(options).process(bytes))?;
    let (width, height) = decoder.basic_info().coded_size;
    check_dimensions(width, height)?;
    Ok(decoder)
}

fn check_dimensions(width: usize, height: usize) -> Result<(), String> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 || width * height > 64_000_000 {
        return Err(
            "The JPEG XL frame exceeds the browser canvas limit (16,384 px / 64 MP).".into(),
        );
    }
    Ok(())
}

fn describe_blend(blend: &JxlLayerBlend) -> Value {
    json!({
        "mode": format!("{:?}", blend.mode),
        "sourceReference": blend.source,
        "alphaChannel": blend.alpha_channel,
        "clamp": blend.clamp,
    })
}

fn describe_frame(
    layer: &JxlLayerInfo,
    frame: &JxlFrameHeader,
    timecode: Option<u32>,
    coding: Option<Value>,
) -> Value {
    json!({
        "name": frame.name,
        "origin": layer.origin,
        "size": layer.size,
        "colorBlend": describe_blend(&layer.color_blend),
        "extraBlends": layer.extra_channel_blends.iter().map(describe_blend).collect::<Vec<_>>(),
        "savedReference": layer.saved_reference,
        "savedBeforeColorTransform": layer.saved_before_color_transform,
        "durationTicks": layer.duration_ticks,
        "durationMilliseconds": frame.duration,
        "timecode": timecode,
        "isLast": layer.is_last,
        "coding": coding,
    })
}

fn describe_profile(profile: &JxlColorProfile) -> Value {
    match profile {
        JxlColorProfile::Icc(bytes) => {
            json!({ "iccBytes": bytes.len(), "cmyk": profile.is_cmyk() })
        }
        JxlColorProfile::Simple(encoding) => json!({ "encoding": format!("{encoding:?}") }),
    }
}

fn probe_vardct_quantizer(bytes: &[u8]) -> Option<Value> {
    let mut input = bytes;
    let options = JxlDecoderOptions::default()
        .with_limits(source_limits())
        .with_coalescing(true)
        .with_skip_preview(true);
    let mut decoder = match JxlDecoder::new(options).process(&mut input) {
        Ok(ProcessingResult::Complete { result }) => result,
        _ => return None,
    };
    let (w, h) = decoder.basic_info().size;
    if w * h > 2_000_000 || !decoder.has_more_frames() {
        return None;
    }
    let num_ec = decoder.basic_info().extra_channels.len();
    decoder.set_pixel_format(JxlPixelFormat::rgba8(num_ec));
    let frame = match decoder.process(&mut input) {
        Ok(ProcessingResult::Complete { result }) => result,
        _ => return None,
    };
    let mut rgba = vec![0u8; w * h * 4];
    let mut bufs = [JxlOutputBuffer::new(&mut rgba, h, w * 4)];
    let next = match frame.process(&mut input, &mut bufs) {
        Ok(ProcessingResult::Complete { result }) => result,
        _ => return None,
    };
    if let Some(q) = next.vardct_quantizer() {
        return Some(json!({
            "globalScale": q.global_scale,
            "quantLf": q.quant_lf,
            "invGlobalScale": q.inv_global_scale(),
        }));
    }
    None
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn native_jxl_info(bytes: &[u8]) -> Result<String, String> {
    let container = inspect::extract_codestream(bytes);
    let mut input = bytes;
    let mut decoder = initialize(&mut input, true)?;
    let info = decoder.basic_info().clone();
    let extras = info
        .extra_channels
        .iter()
        .enumerate()
        .map(|(i, channel)| {
            let details = decoder.extra_channel_details(i);
            json!({
                "type": format!("{:?}", channel.ec_type),
                "name": channel.name,
                "bits": channel.bits_per_sample,
                "alphaAssociated": channel.alpha_associated,
                "dimensionShift": channel.dim_shift,
                "sampleFormat": details.as_ref().map(|v| format!("{:?}", v.bit_depth)),
                "spotColor": details.as_ref().and_then(|v| v.spot_color),
                "cfaChannel": details.as_ref().and_then(|v| v.cfa_channel),
            })
        })
        .collect::<Vec<_>>();
    let mut summary = json!({
        "canvas": info.coded_size,
        "orientation": info.intrinsic_orientation as u8,
        "bits": info.bit_depth.bits_per_sample(),
        "sampleFormat": format!("{:?}", info.bit_depth),
        "usesOriginalProfile": info.uses_original_profile,
        "xybEncoded": !info.uses_original_profile,
        "codestreamLevel": container.as_ref().map_or(5, |c| c.codestream_level),
        "isContainer": container.as_ref().is_some_and(|c| c.is_container),
        "intrinsicSize": info.intrinsic_size,
        "previewSize": info.preview_size,
        "profile": describe_profile(decoder.embedded_color_profile()),
        "toneMapping": {
            "intensityTarget": info.tone_mapping.intensity_target,
            "minimumNits": info.tone_mapping.min_nits,
            "relativeToMaximumDisplay": info.tone_mapping.relative_to_max_display,
            "linearBelow": info.tone_mapping.linear_below,
        },
        "animation": info.animation.as_ref().map(|clock| json!({
            "ticksNumerator": clock.tps_numerator,
            "ticksDenominator": clock.tps_denominator,
            "loopCount": clock.num_loops,
            "hasTimecodes": clock.have_timecodes,
        })),
        "extraChannels": extras,
    });
    let frame_ctx = inspect::FrameHeaderContext {
        xyb_encoded: !info.uses_original_profile,
        num_extra_channels: info.extra_channels.len() as u32,
        img_width: info.coded_size.0 as u32,
        img_height: info.coded_size.1 as u32,
        have_animation: info.animation.is_some(),
        have_timecodes: info.animation.as_ref().is_some_and(|a| a.have_timecodes),
    };
    let mut frames = Vec::new();
    let mut frame_index = 0usize;
    while decoder.has_more_frames() {
        let frame = complete(decoder.process(&mut input))?;
        let layer = frame.frame_layer_info().map_err(|e| e.to_string())?;
        let hdr = frame.frame_header();
        let tc = frame.frame_timecode();
        // Read exact bitstream coding flags using the scanned frame's file_offset.
        let coding = frame
            .scanned_frames()
            .get(frame_index)
            .and_then(|sf| sf.file_offset)
            .and_then(|file_off| {
                let cs = container.as_ref()?;
                let cs_off = cs.file_to_codestream_offset(file_off)?;
                inspect::parse_frame_header_json(cs.codestream.get(cs_off..)?, &frame_ctx)
            });
        frames.push(describe_frame(&layer, &hdr, tc, coding));
        frame_index += 1;
        decoder = complete(frame.skip_frame(&mut input))?;
    }
    summary["frames"] = json!(frames);
    summary["exifBytes"] = json!(decoder.exif().map(|b| b.len()));
    summary["xmpBytes"] = json!(decoder.xmp().map(|b| b.len()));
    summary["hasGainMap"] = json!(decoder.gain_map().is_some());
    summary["hasFrameIndex"] = json!(decoder.frame_index().is_some());
    summary["keyframes"] = json!(
        decoder
            .scanned_frames()
            .iter()
            .map(|sf| json!({
                "index": sf.index,
                "isKeyframe": sf.is_keyframe,
                "fileOffset": sf.file_offset,
            }))
            .collect::<Vec<_>>()
    );
    if !info.uses_original_profile {
        summary["vardctQuantizer"] = json!(probe_vardct_quantizer(bytes));
    }
    Ok(summary.to_string())
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn native_embedded_icc(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut input = bytes;
    let decoder = initialize(&mut input, true)?;
    match decoder.embedded_color_profile() {
        JxlColorProfile::Icc(bytes) => Ok(bytes.clone()),
        JxlColorProfile::Simple(_) => Err("The source uses a parametric color encoding.".into()),
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn native_reconstruct_jpeg(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut input = bytes;
    initialize(&mut input, true)?;
    let options = JxlDecoderOptions::default().with_limits(source_limits());
    zenjxl_decoder::reconstruct_jpeg_with(bytes, options)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "The source has no reconstructible original JPEG.".into())
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct NativeTimeline {
    info: String,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl NativeTimeline {
    pub fn info(&self) -> String {
        self.info.clone()
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

fn decode_timeline_inner(bytes: &[u8], use_cms: bool) -> Result<NativeTimeline, String> {
    let mut options = JxlDecoderOptions::default()
        .with_limits(source_limits())
        .with_coalescing(true)
        .with_adjust_orientation(true)
        .with_skip_preview(true)
        .with_dither_u8(false);
    if use_cms {
        options = options.with_cms(Box::new(MoxCms));
    }
    let mut input = bytes;
    let mut decoder = complete(JxlDecoder::new(options).process(&mut input))?;
    let info = decoder.basic_info().clone();
    let (width, height) = info.size;
    check_dimensions(width, height)?;
    let frame_bytes = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "Animation frame buffer size overflow.".to_string())?;
    let num_ec = info.extra_channels.len();
    let alpha_associated = info
        .extra_channels
        .iter()
        .any(|ch| ch.ec_type == ExtraChannel::Alpha && ch.alpha_associated);
    decoder.set_pixel_format(JxlPixelFormat::rgba8(num_ec));

    let mut all_pixels = Vec::new();
    let mut frames_meta = Vec::new();
    while decoder.has_more_frames() {
        if frames_meta.len() >= MAX_TIMELINE_DECODE_FRAMES {
            return Err("Animation exceeds the 64-frame timeline limit.".into());
        }
        if all_pixels.len().saturating_add(frame_bytes) > MAX_TIMELINE_DECODE_BYTES {
            return Err("Animation exceeds the 128 MB decoded timeline budget.".into());
        }
        let frame = complete(decoder.process(&mut input))?;
        let hdr = frame.frame_header();
        let layer = frame.frame_layer_info().ok();
        let tc = frame.frame_timecode();
        let mut rgba = vec![0u8; frame_bytes];
        let mut bufs = [JxlOutputBuffer::new(&mut rgba, height, width * 4)];
        decoder = complete(frame.process(&mut input, &mut bufs))?;
        if alpha_associated {
            for px in rgba.as_chunks_mut::<4>().0 {
                if let Some(a) = NonZeroU32::new(u32::from(px[3])) {
                    for v in &mut px[..3] {
                        *v = ((u32::from(*v) * 255 + a.get() / 2) / a.get()).min(255) as u8;
                    }
                }
            }
        }
        all_pixels.extend_from_slice(&rgba);
        frames_meta.push(json!({
            "name": hdr.name,
            "durationTicks": layer.as_ref().map_or(0, |l| l.duration_ticks),
            "durationMilliseconds": hdr.duration,
            "timecode": tc,
        }));
    }
    let meta = json!({
        "width": width,
        "height": height,
        "animation": info.animation.as_ref().map(|a| json!({
            "tpsNumerator": a.tps_numerator,
            "tpsDenominator": a.tps_denominator,
            "loops": a.num_loops,
            "hasTimecodes": a.have_timecodes,
        })),
        "frames": frames_meta,
    });
    Ok(NativeTimeline {
        info: meta.to_string(),
        width: width as u32,
        height: height as u32,
        pixels: all_pixels,
    })
}

/// Decode all visible coalesced animation frames into RGBA8 in display orientation,
/// preserving exact animation clock, loop count, frame names, durations and timecodes.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn native_jxl_timeline(bytes: &[u8]) -> Result<NativeTimeline, String> {
    if bytes.len() > 128 * 1024 * 1024 {
        return Err("The JPEG XL source exceeds 128 MiB.".into());
    }
    decode_timeline_inner(bytes, true).or_else(|_| decode_timeline_inner(bytes, false))
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct NativeCmyk {
    width: u32,
    height: u32,
    bit_depth: u8,
    uses_original_profile: bool,
    samples: Vec<u8>,
    icc: Vec<u8>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl NativeCmyk {
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn bit_depth(&self) -> u8 {
        self.bit_depth
    }
    pub fn uses_original_profile(&self) -> bool {
        self.uses_original_profile
    }
    pub fn take_samples(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.samples)
    }
    pub fn take_icc(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.icc)
    }
}

/// Extract exact coalesced CMYK samples (8-bit or 16-bit LE in JXL inverted ink convention:
/// `0 = full ink`, `max = no ink`) and embedded CMYK ICC profile bytes from a CMYK JPEG XL.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn native_extract_cmyk(bytes: &[u8]) -> Result<NativeCmyk, String> {
    if bytes.len() > 128 * 1024 * 1024 {
        return Err("The JPEG XL source exceeds 128 MiB.".into());
    }
    let options = JxlDecoderOptions::default()
        .with_limits(source_limits())
        .with_coalescing(true)
        .with_adjust_orientation(true)
        .with_skip_preview(true)
        .with_dither_u8(false);
    let mut input = bytes;
    let mut decoder = complete(JxlDecoder::new(options).process(&mut input))?;
    let icc = match decoder.embedded_color_profile() {
        JxlColorProfile::Icc(b) if decoder.embedded_color_profile().is_cmyk() => b.clone(),
        _ => return Err("The JPEG XL source does not have an embedded CMYK ICC profile.".into()),
    };
    let info = decoder.basic_info().clone();
    if !info
        .extra_channels
        .iter()
        .any(|c| c.ec_type == ExtraChannel::Black)
    {
        return Err("The JPEG XL source has no Black extra channel.".into());
    }
    let (width, height) = info.size;
    check_dimensions(width, height)?;
    let wide = info.bit_depth.bits_per_sample() > 8;
    let bit_depth = if wide { 16u8 } else { 8u8 };
    let bpp = if wide { 8usize } else { 4usize };
    let total_bytes = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(bpp))
        .ok_or_else(|| "CMYK output size overflow.".to_string())?;
    decoder.set_pixel_format(JxlPixelFormat {
        color_type: JxlColorType::Cmyk,
        color_data_format: Some(if wide {
            JxlDataFormat::U16 {
                endianness: Endianness::LittleEndian,
                bit_depth: 16,
            }
        } else {
            JxlDataFormat::U8 { bit_depth: 8 }
        }),
        extra_channel_format: vec![None; info.extra_channels.len()],
    });
    let frame = complete(decoder.process(&mut input))?;
    let mut samples = vec![0u8; total_bytes];
    let mut bufs = [JxlOutputBuffer::new(&mut samples, height, width * bpp)];
    complete(frame.process(&mut input, &mut bufs))?;
    Ok(NativeCmyk {
        width: width as u32,
        height: height as u32,
        bit_depth,
        uses_original_profile: info.uses_original_profile,
        samples,
        icc,
    })
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct NativeFrame {
    info: String,
    preview: Vec<u8>,
    preview_error: String,
    samples: Vec<Vec<f32>>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl NativeFrame {
    pub fn info(&self) -> String {
        self.info.clone()
    }
    pub fn preview_error(&self) -> String {
        self.preview_error.clone()
    }
    pub fn take_preview(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.preview)
    }
    pub fn plane_count(&self) -> usize {
        self.samples.len()
    }
    pub fn take_plane(&mut self, index: usize) -> Result<Vec<f32>, String> {
        self.samples
            .get_mut(index)
            .map(core::mem::take)
            .ok_or_else(|| "Native frame plane index is out of range.".into())
    }
}

fn oriented_origin(
    orientation: Orientation,
    origin: (i32, i32),
    size: (usize, usize),
    canvas: (usize, usize),
) -> [i64; 2] {
    let (u, v) = orientation.display_pixel((0, 0), (2, 2));
    let (ux, vx) = orientation.display_pixel((1, 0), (2, 2));
    let (uy, vy) = orientation.display_pixel((0, 1), (2, 2));
    let (a, b) = orientation.display_pixel((0, 0), canvas);
    let slopes_x = (ux as i64 - u as i64, uy as i64 - u as i64);
    let slopes_y = (vx as i64 - v as i64, vy as i64 - v as i64);
    let evaluate = |slope: (i64, i64), offset: usize| {
        let x = i64::from(origin.0) + if slope.0 < 0 { size.0 as i64 - 1 } else { 0 };
        let y = i64::from(origin.1) + if slope.1 < 0 { size.1 as i64 - 1 } else { 0 };
        offset as i64 + slope.0 * x + slope.1 * y
    };
    [evaluate(slopes_x, a), evaluate(slopes_y, b)]
}

fn orient_preview(
    preview: Vec<u8>,
    width: usize,
    height: usize,
    orientation: Orientation,
) -> Result<Vec<u8>, String> {
    if orientation == Orientation::Identity {
        return Ok(preview);
    }
    let (display_width, _) = orientation.map_size((width, height));
    let mut output = Vec::new();
    output
        .try_reserve_exact(preview.len())
        .map_err(|e| e.to_string())?;
    output.resize(preview.len(), 0);
    for (i, samples) in preview.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = orientation.display_pixel((i % width, i / width), (width, height));
        let offset = (y * display_width + x) * 4;
        output[offset..offset + 4].copy_from_slice(samples);
    }
    Ok(output)
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn native_jxl_frame(bytes: &[u8], selected: u32) -> Result<NativeFrame, String> {
    if selected as usize >= MAX_NATIVE_FRAMES {
        return Err("Native frame index is out of range.".into());
    }
    let mut input = bytes;
    let mut decoder = initialize(&mut input, false)?;
    let source = decoder.basic_info().clone();
    let color_type = if decoder.embedded_color_profile().is_cmyk() {
        JxlColorType::Cmyk
    } else if decoder.current_pixel_format().color_type.is_grayscale() {
        JxlColorType::Grayscale
    } else {
        JxlColorType::Rgb
    };
    decoder.set_pixel_format(JxlPixelFormat {
        color_type,
        color_data_format: Some(JxlDataFormat::f32()),
        extra_channel_format: vec![Some(JxlDataFormat::f32()); source.extra_channels.len()],
    });
    let profile = decoder.output_color_profile().clone();
    for index in 0..=selected {
        if !decoder.has_more_frames() {
            break;
        }
        let frame = complete(decoder.process(&mut input))?;
        if index != selected {
            decoder = complete(frame.skip_frame(&mut input))?;
            continue;
        }
        let layer = frame.frame_layer_info().map_err(|e| e.to_string())?;
        let (width, height) = layer.size;
        let channels = color_type.samples_per_pixel();
        check_dimensions(width, height)?;
        if width * height * (channels + source.extra_channels.len()) > MAX_NATIVE_SAMPLES {
            return Err("The local layer exceeds the browser’s native sample limit.".into());
        }
        let mut samples = Vec::with_capacity(1 + source.extra_channels.len());
        for n in 0..=source.extra_channels.len() {
            let count = width * height * if n == 0 { channels } else { 1 };
            let mut plane = Vec::new();
            plane.try_reserve_exact(count).map_err(|e| e.to_string())?;
            plane.resize(count, 0.0_f32);
            samples.push(plane);
        }
        let mut info = describe_frame(&layer, &frame.frame_header(), frame.frame_timecode(), None);
        info["planeColorChannels"] = json!(channels);
        info["planeColorModel"] = json!(format!("{color_type:?}"));
        info["planeProfile"] = describe_profile(&profile);
        let orientation = source.intrinsic_orientation;
        info["orientation"] = json!(orientation as u8);
        info["displayOrigin"] = json!(oriented_origin(
            orientation,
            layer.origin,
            layer.size,
            source.coded_size
        ));
        info["displaySize"] = json!(orientation.map_size((width, height)));
        let mut outputs: Vec<_> = samples
            .iter_mut()
            .enumerate()
            .map(|(n, plane)| {
                let row_samples = if n == 0 { width * channels } else { width };
                JxlOutputBuffer::new(bytemuck::cast_slice_mut(plane), height, row_samples * 4)
            })
            .collect();
        complete(frame.process(&mut input, &mut outputs))?;
        let alpha_index = source
            .extra_channels
            .iter()
            .position(|ch| ch.ec_type == ExtraChannel::Alpha);
        let alpha = alpha_index.map(|i| {
            (
                samples[i + 1].as_slice(),
                source.extra_channels[i].alpha_associated,
            )
        });
        let preview = render_native_preview(
            &samples[0],
            alpha,
            width,
            height,
            channels,
            profile,
            source.tone_mapping.intensity_target,
        )
        .and_then(|pixels| orient_preview(pixels, width, height, orientation));
        let (preview, preview_error) = match preview {
            Ok(bytes) => (bytes, String::new()),
            Err(error) => (Vec::new(), error),
        };
        return Ok(NativeFrame {
            info: info.to_string(),
            samples,
            preview,
            preview_error,
        });
    }
    Err("Native frame index is out of range.".into())
}

fn render_native_preview(
    color: &[f32],
    alpha: Option<(&[f32], bool)>,
    width: usize,
    height: usize,
    channels: usize,
    profile: JxlColorProfile,
    intensity: f32,
) -> Result<Vec<u8>, String> {
    let same_encoding = profile.same_color_encoding(&JxlColorProfile::Simple(
        JxlColorEncoding::srgb(channels == 1),
    ));
    let mut transform = if same_encoding {
        None
    } else {
        let srgb = JxlColorProfile::Simple(JxlColorEncoding::srgb(false));
        let (_, mut converters) = MoxCms
            .initialize_transforms(1, width, profile, srgb, intensity)
            .map_err(|e| e.to_string())?;
        converters.pop()
    };
    let mut output = Vec::new();
    output
        .try_reserve_exact(width * height * 4)
        .map_err(|e| e.to_string())?;
    let mut row = vec![0.0; width * channels];
    let mut converted = vec![0.0; width * 3];
    let rounded = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    for y in 0..height {
        row.copy_from_slice(&color[y * width * channels..(y + 1) * width * channels]);
        if let Some((alpha, true)) = alpha {
            for (pixel, opacity) in row
                .chunks_exact_mut(channels)
                .zip(&alpha[y * width..(y + 1) * width])
            {
                if *opacity > 0.0 {
                    for value in pixel {
                        *value /= *opacity;
                    }
                }
            }
        }
        let color_row = if let Some(cms) = &mut transform {
            cms.do_transform(&row, &mut converted)
                .map_err(|e| e.to_string())?;
            converted.as_slice()
        } else {
            row.as_slice()
        };
        let out_channels = if transform.is_some() { 3 } else { channels };
        for (x, pixel) in color_row.chunks_exact(out_channels).enumerate() {
            let red = rounded(pixel[0]);
            let (green, blue) = if out_channels == 1 {
                (red, red)
            } else {
                (rounded(pixel[1]), rounded(pixel[2]))
            };
            let opacity = alpha.map_or(255, |(plane, _)| rounded(plane[y * width + x]));
            output.extend([red, green, blue, opacity]);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_crop_rotates_with_the_stored_canvas() {
        assert_eq!(
            oriented_origin(Orientation::Identity, (-2, 7), (11, 5), (40, 20)),
            [-2, 7]
        );
        assert_eq!(
            oriented_origin(Orientation::Rotate90Cw, (-2, 7), (11, 5), (40, 20)),
            [8, -2]
        );
        assert_eq!(
            oriented_origin(Orientation::FlipHorizontal, (-2, 7), (11, 5), (40, 20)),
            [31, 7]
        );
    }
}
