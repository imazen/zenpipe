#![forbid(unsafe_code)]

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn filter_rgba(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    exposure: f32,
    contrast: f32,
    saturation: f32,
) -> Result<Vec<u8>, String> {
    use zenfilters::filters::{Contrast, Exposure, Saturation};
    use zenfilters::{FilterContext, Pipeline, PipelineConfig, apply_to_buffer};

    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > 24_000_000
        || !(-2.0..=2.0).contains(&exposure)
        || !(0.0..=2.0).contains(&contrast)
        || !(0.0..=2.0).contains(&saturation)
        || pixels.len() != width as usize * height as usize * 4
    {
        return Err("Invalid photo adjustments or RGBA dimensions.".into());
    }
    if exposure == 0.0 && contrast == 1.0 && saturation == 1.0 {
        return Ok(pixels);
    }

    let input = filter_pixels::PixelBuffer::from_vec(
        pixels,
        width,
        height,
        filter_pixels::PixelDescriptor::RGBA8_SRGB,
    )
    .map_err(|e| e.to_string())?;
    let mut pipeline = Pipeline::new(PipelineConfig::default()).map_err(|e| e.to_string())?;
    if exposure != 0.0 {
        let mut filter = Exposure::default();
        filter.stops = exposure;
        pipeline.push(Box::new(filter));
    }
    if contrast != 1.0 {
        let mut filter = Contrast::default();
        filter.amount = contrast - 1.0;
        pipeline.push(Box::new(filter));
    }
    if saturation != 1.0 {
        let mut filter = Saturation::default();
        filter.factor = saturation;
        pipeline.push(Box::new(filter));
    }
    let output = apply_to_buffer(&pipeline, &input, true, &mut FilterContext::new())
        .map_err(|e| e.to_string())?;
    let view = output.as_slice();
    let mut packed = Vec::new();
    packed
        .try_reserve_exact(width as usize * height as usize * 4)
        .map_err(|e| e.to_string())?;
    for y in 0..height {
        packed.extend_from_slice(view.row(y));
    }
    Ok(packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photo_filters_preserve_alpha_and_leave_neutral_layers_exact() {
        let input = vec![60, 60, 60, 0, 90, 90, 90, 127, 125, 125, 125, 255];
        assert_eq!(
            filter_rgba(input.clone(), 3, 1, 0.0, 1.0, 1.0).unwrap(),
            input
        );
        let brighter = filter_rgba(input.clone(), 3, 1, 1.0, 1.0, 1.0).unwrap();
        for (before, after) in input
            .as_chunks::<4>()
            .0
            .iter()
            .zip(brighter.as_chunks::<4>().0)
        {
            assert!(
                after[0] > before[0],
                "exposure left {before:?} at {after:?}"
            );
            assert_eq!(after[3], before[3]);
        }
        assert!(filter_rgba(input, 3, 1, 0.0, f32::NAN, 1.0).is_err());
    }
}
