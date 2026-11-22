#![forbid(unsafe_code)]

use core::fmt::Display;

use serde_json::{Value, json};
use zenjxl_decoder::api::{
    ExtraChannel, JxlCms, JxlColorEncoding, JxlColorProfile, JxlColorType, JxlDataFormat,
    JxlDecoder, JxlDecoderOptions, JxlFrameHeader, JxlLayerBlend, JxlLayerInfo, JxlOutputBuffer,
    JxlPixelFormat, MoxCms, ProcessingResult, states,
};
use zenjxl_decoder::api::{JxlCodestreamLevel, JxlDecoderLimits, Orientation};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

const MAX_NATIVE_FRAMES: usize = 128;
const MAX_NATIVE_SAMPLES: usize = 16_000_000;

fn complete<T, U, E: Display>(step: Result<ProcessingResult<T, U>, E>) -> Result<T, String> {
    match step.map_err(|error| error.to_string())? {
        ProcessingResult::Complete { result } => Ok(result),
        ProcessingResult::NeedsMoreInput { .. } => Err("The JPEG XL source is truncated.".into()),
    }
}

fn source_limits() -> JxlDecoderLimits {
    let mut limits = JxlDecoderLimits::restrictive()
        .with_max_pixels(24_000_000)
        .with_max_reference_frames(4)
        .with_max_memory_bytes(512 * 1024 * 1024)
        .with_max_frame_headers(MAX_NATIVE_FRAMES as u64);
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
    if width == 0 || height == 0 || width > 8192 || height > 8192 || width * height > 24_000_000 {
        return Err("The JPEG XL frame exceeds the browser canvas limit.".into());
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

fn describe_frame(layer: &JxlLayerInfo, frame: &JxlFrameHeader, timecode: Option<u32>) -> Value {
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
        "isLast": layer.is_last
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

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn native_jxl_info(bytes: &[u8]) -> Result<String, String> {
    let mut input = bytes;
    let mut decoder = initialize(&mut input, true)?;
    let info = decoder.basic_info();
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
    let mut frames = Vec::new();
    while decoder.has_more_frames() {
        let frame = complete(decoder.process(&mut input))?;
        let layer = frame.frame_layer_info().map_err(|e| e.to_string())?;
        frames.push(describe_frame(
            &layer,
            &frame.frame_header(),
            frame.frame_timecode(),
        ));
        decoder = complete(frame.skip_frame(&mut input))?;
    }
    summary["frames"] = json!(frames);
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
    // Derive the signed affine map from the shared pixel orientation. The
    // translated crop may start outside the coded canvas.
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
        let mut info = describe_frame(&layer, &frame.frame_header(), frame.frame_timecode());
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
