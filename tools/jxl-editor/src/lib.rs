#![forbid(unsafe_code)]

mod author;
mod cmyk;
mod splines;
pub use author::{encode_jxl_configured, encode_timeline, encode_timeline_configured};
pub use cmyk::{
    encode_cmyk_jxl, generate_minimal_cmyk_icc, preview_cmyk_to_rgba8, rgba8_to_cmyk8_jxl,
    rgba8_to_cmyk16_jxl,
};
pub use splines::{render_splines_rgba, spline_color_dct_from_srgb};

use core::num::NonZeroU32;
use jxl_encoder::{AnimationFrame, AnimationParams, Limits, LosslessConfig, PixelLayout};
use zencodec::ResourceLimits;
use zenjxl_decoder::api::{JxlCodestreamLevel, JxlDecoderLimits, JxlDecoderOptions, MoxCms};
use zenpixels::{PixelDescriptor, PixelSlice};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

const MAX_PIXELS: u64 = 24_000_000;
const MAX_SIDE: u32 = 8192;
const MAX_INPUT: u64 = 128 * 1024 * 1024;
const MAX_CODEC_MEMORY: u64 = 512 * 1024 * 1024;

fn limits() -> ResourceLimits {
    ResourceLimits::none()
        .with_max_pixels(MAX_PIXELS)
        .with_max_width(MAX_SIDE)
        .with_max_height(MAX_SIDE)
        .with_max_input_bytes(MAX_INPUT)
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub struct Raster {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
impl Raster {
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

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn encode_jxl(
    pixels: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<u8>, String> {
    limits()
        .check_dimensions(width, height)
        .map_err(|e| e.to_string())?;
    let view: PixelSlice<'_> =
        PixelSlice::new(pixels, width, height, stride, PixelDescriptor::RGBA8_SRGB)
            .map_err(|e| e.to_string())?;
    let config = LosslessConfig::new()
        .with_effort(3)
        .with_keep_invisible(true);
    let codec_limits = Limits::default().with_max_memory_bytes(MAX_CODEC_MEMORY);
    if view.stride() == width as usize * 4 {
        return config
            .encode_request(width, height, PixelLayout::Rgba8)
            .with_limits(&codec_limits)
            .encode(view.as_strided_bytes())
            .map_err(|e| e.to_string());
    }

    let mut encoder = config
        .encoder(width, height, PixelLayout::Rgba8)
        .map_err(|e| e.to_string())?
        .with_limits(&codec_limits);
    for y in 0..view.rows() {
        encoder
            .push_rows(view.row(y), 1)
            .map_err(|e| e.to_string())?;
    }
    encoder.finish().map_err(|e| e.to_string())
}

/// Try a native layered presentation. The fallback is a regular still image
/// whenever JPEG XL composition would change a single preview sample.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn encode_layers(
    pixels: &[u8],
    expected: &[u8],
    width: u32,
    height: u32,
    ids: &str,
) -> Result<Option<Vec<u8>>, String> {
    limits()
        .check_dimensions(width, height)
        .map_err(|e| e.to_string())?;
    let frame_bytes = width as usize * height as usize * 4;
    if frame_bytes == 0 || expected.len() != frame_bytes {
        return Err("Invalid native layer dimensions.".into());
    }
    let count = pixels.len() / frame_bytes;
    if !(2..=8).contains(&count)
        || !pixels.len().is_multiple_of(frame_bytes)
        || pixels.len() > 64_000_000
    {
        return Err("Invalid native layer pixel count.".into());
    }
    let names: Vec<_> = ids.split(',').collect();
    if names.len() != count || names.iter().any(|id| id.len() != 36) {
        return Err("Invalid native layer identifiers.".into());
    }
    let frames: Vec<_> = pixels
        .chunks_exact(frame_bytes)
        .zip(names)
        .enumerate()
        .map(|(i, (pixels, id))| {
            let last = i + 1 == count;
            AnimationFrame::new(pixels, u32::from(last))
                .with_name(id)
                .with_save_as_reference(u32::from(!last))
        })
        .collect();
    let params = AnimationParams {
        tps_numerator: 1,
        num_loops: 1,
        ..AnimationParams::default()
    };
    let Ok(encoded) = LosslessConfig::new()
        .with_effort(3)
        .with_keep_invisible(true)
        .encode_animation_with_limits(
            width,
            height,
            PixelLayout::Rgba8,
            &params,
            &frames,
            &Limits::default().with_max_memory_bytes(MAX_CODEC_MEMORY),
        )
    else {
        return Ok(None);
    };
    let Ok(decoded) = decode_jxl(&encoded) else {
        return Ok(None);
    };
    Ok(
        (decoded.width == width && decoded.height == height && decoded.pixels == expected)
            .then_some(encoded),
    )
}

fn decoder_limits() -> JxlDecoderLimits {
    let mut decode_limits = JxlDecoderLimits::restrictive()
        .with_max_pixels(MAX_PIXELS as usize)
        .with_max_reference_frames(4)
        .with_max_memory_bytes(MAX_CODEC_MEMORY);
    decode_limits.max_codestream_level = JxlCodestreamLevel::Level10;
    decode_limits
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn decode_jxl(bytes: &[u8]) -> Result<Raster, String> {
    limits()
        .check_input_size(bytes.len() as u64)
        .map_err(|e| e.to_string())?;
    let decode_limits = decoder_limits();
    let header = zenjxl_decoder::read_header_with(bytes, decode_limits.clone())
        .map_err(|e| e.to_string())?;
    let (width, height) = header.info.size;
    limits()
        .check_dimensions(width as u32, height as u32)
        .map_err(|e| e.to_string())?;
    let options = JxlDecoderOptions::default()
        .with_limits(decode_limits)
        .with_cms(Box::new(MoxCms))
        .with_dither_u8(false);
    let image = zenjxl_decoder::decode_with(bytes, options).map_err(|e| e.to_string())?;
    let mut pixels = if image.is_grayscale {
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(image.data.len() * 2)
            .map_err(|e| e.to_string())?;
        for sample in image.data.as_chunks::<2>().0 {
            rgba.extend_from_slice(&[sample[0], sample[0], sample[0], sample[1]]);
        }
        rgba
    } else {
        image.data
    };
    if image
        .info
        .extra_channels
        .iter()
        .any(|channel| channel.alpha_associated)
    {
        for sample in pixels.as_chunks_mut::<4>().0 {
            if let Some(alpha) = NonZeroU32::new(u32::from(sample[3])) {
                for value in &mut sample[..3] {
                    *value =
                        ((u32::from(*value) * 255 + alpha.get() / 2) / alpha.get()).min(255) as u8;
                }
            }
        }
    }
    Ok(Raster {
        width: image.width as u32,
        height: image.height as u32,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_frames_present_only_the_exact_composite() {
        let base = [20, 30, 40, 255, 65, 42, 23, 255];
        let expected = [220, 100, 7, 255, 65, 42, 23, 255];
        let pixels = [base, expected].concat();
        let ids = "00000000-0000-4000-8000-000000000001,00000000-0000-4000-8000-000000000002";
        let file = encode_layers(&pixels, &expected, 2, 1, ids)
            .unwrap()
            .unwrap();
        assert_eq!(decode_jxl(&file).unwrap().into_pixels(), expected);
        assert!(encode_layers(&pixels, &base, 2, 1, ids).unwrap().is_none());
    }

    #[test]
    fn oriented_precision_and_metadata_stay_on_the_source() {
        use jxl_encoder::{ImageMetadata, Orientation};

        // TIFF with an orientation value of 6, as in the native JPEG XL header.
        let exif = [
            73, 73, 42, 0, 8, 0, 0, 0, 1, 0, 18, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>";
        let metadata = ImageMetadata::new()
            .with_orientation(Orientation::Rotate90)
            .with_exif(&exif)
            .with_xmp(xmp);
        let rgba8: [[u8; 4]; 6] = [
            [20, 31, 42, 255],
            [30, 41, 52, 64],
            [40, 51, 62, 128],
            [50, 61, 72, 192],
            [60, 71, 82, 254],
            [70, 81, 92, 255],
        ];
        let rgba16: Vec<_> = rgba8
            .iter()
            .flatten()
            .flat_map(|sample| (u16::from(*sample) * 257).to_ne_bytes())
            .collect();
        let file = LosslessConfig::new()
            .encode_request(3, 2, PixelLayout::Rgba16)
            .with_metadata(&metadata)
            .encode(&rgba16)
            .unwrap();
        let mut limits = JxlDecoderLimits::restrictive();
        limits.max_codestream_level = JxlCodestreamLevel::Level10;
        let decoded =
            zenjxl_decoder::decode_with(&file, JxlDecoderOptions::default().with_limits(limits))
                .unwrap();
        assert_eq!(decoded.info.bit_depth.bits_per_sample(), 16);
        assert_eq!(decoded.info.coded_size, (3, 2));
        assert_eq!(decoded.info.intrinsic_orientation as u8, 6);
        assert_eq!(decoded.exif.as_deref(), Some(exif.as_slice()));
        assert_eq!(decoded.xmp.as_deref(), Some(xmp.as_slice()));

        let preview = decode_jxl(&file).unwrap();
        assert_eq!((preview.width(), preview.height()), (2, 3));
        let rotated = [rgba8[3], rgba8[0], rgba8[4], rgba8[1], rgba8[5], rgba8[2]].concat();
        assert_eq!(preview.into_pixels(), rotated);
    }

    #[test]
    fn encodes_odd_strided_rgba_at_full_alpha_and_color_precision() {
        let row1 = [5, 3, 1, 0, 17, 33, 129, 1, 41, 55, 23, 255];
        let row2 = [7, 3, 7, 128, 200, 23, 199, 93, 5, 55, 22, 254];
        let input = [row1.as_slice(), &[83; 4], &row2].concat();
        let file = encode_jxl(&input, 3, 2, 16).unwrap();
        let result = decode_jxl(&file).unwrap();
        assert_eq!((result.width(), result.height()), (3, 2));
        assert_eq!(result.into_pixels(), [row1, row2].concat());
        assert!(encode_jxl(&input, 3, 2, 11).is_err());
        assert!(encode_jxl(&input[..27], 3, 2, 16).is_err());
    }
}
