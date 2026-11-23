#![forbid(unsafe_code)]

mod filters;
mod mask;
mod resize;

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
    filters::filter_rgba_compat(pixels, width, height, exposure, contrast, saturation)
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn filter_catalog_json() -> String {
    filters::filter_catalog_json_impl()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn apply_zenfilters_rgba(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    spec_json: &str,
) -> Result<Vec<u8>, String> {
    filters::apply_zenfilters_rgba_impl(pixels, width, height, spec_json)
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn resize_kernels_json() -> String {
    resize::resize_kernels_json_impl()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn resize_rgba(
    pixels: Vec<u8>,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    kernel: &str,
    linear: bool,
) -> Result<Vec<u8>, String> {
    resize::resize_rgba_impl(pixels, src_w, src_h, dst_w, dst_h, kernel, linear)
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen)]
pub fn apply_layer_mask_rgba(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    spec_json: &str,
    raster_mask: &[u8],
    overlay_ruby: bool,
) -> Result<Vec<u8>, String> {
    mask::apply_layer_mask_rgba_impl(pixels, width, height, spec_json, raster_mask, overlay_ruby)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rgba(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                out.push(((x * 13 + 30) % 256) as u8);
                out.push(((y * 17 + 40) % 256) as u8);
                out.push((((x + y) * 9 + 50) % 256) as u8);
                out.push(if (x + y) % 5 == 0 { 180 } else { 255 });
            }
        }
        out
    }

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

    #[test]
    fn zenfilters_full_catalog_presets_film_looks_auto_and_selective_mask() {
        let cat: serde_json::Value = serde_json::from_str(&filter_catalog_json()).unwrap();
        assert!(cat["presets"].as_array().unwrap().len() >= 19);
        assert!(cat["filmLooks"].as_array().unwrap().len() >= 34);
        assert!(cat["controls"].as_array().unwrap().len() >= 20);

        let img = sample_rgba(24, 18);
        assert_eq!(
            apply_zenfilters_rgba(img.clone(), 24, 18, "{}").unwrap(),
            img
        );

        let preset_out = apply_zenfilters_rgba(
            img.clone(),
            24,
            18,
            r#"{"preset":"Golden Hour","presetStrength":0.85,"clarity":0.35,"vignette":0.25}"#,
        )
        .unwrap();
        assert_ne!(preset_out, img);
        for (a, b) in img
            .as_chunks::<4>()
            .0
            .iter()
            .zip(preset_out.as_chunks::<4>().0)
        {
            assert_eq!(a[3], b[3], "alpha must be preserved");
        }

        let film_out = apply_zenfilters_rgba(
            img.clone(),
            24,
            18,
            r#"{"filmLook":"portra","filmStrength":0.9,"autoTone":0.5,"sharpen":0.4}"#,
        )
        .unwrap();
        assert_ne!(film_out, img);

        let masked = apply_zenfilters_rgba(
            img.clone(),
            24,
            18,
            r#"{"invert":true,"filterMask":{"kind":"linear","x0":0.0,"y0":0.5,"x1":1.0,"y1":0.5}}"#,
        )
        .unwrap();
        // Left edge untouched, right edge inverted
        assert_eq!(masked[0..3], img[0..3]);
        let right_idx = (23) * 4;
        assert_ne!(
            masked[right_idx..right_idx + 3],
            img[right_idx..right_idx + 3]
        );
    }

    #[test]
    fn zenresize_scales_with_multiple_kernels_and_preserves_dimensions() {
        let kernels: serde_json::Value = serde_json::from_str(&resize_kernels_json()).unwrap();
        assert!(kernels.as_array().unwrap().len() >= 10);

        let img = sample_rgba(32, 24);
        assert_eq!(
            resize_rgba(img.clone(), 32, 24, 32, 24, "robidoux", true).unwrap(),
            img
        );
        let down = resize_rgba(img.clone(), 32, 24, 16, 12, "lanczos", true).unwrap();
        assert_eq!(down.len(), 16 * 12 * 4);
        let up = resize_rgba(img.clone(), 32, 24, 64, 48, "ginseng", true).unwrap();
        assert_eq!(up.len(), 64 * 48 * 4);
        let box_up = resize_rgba(img.clone(), 32, 24, 64, 48, "box", false).unwrap();
        assert_ne!(
            up, box_up,
            "different kernels should produce distinct resampling"
        );
        assert!(resize_rgba(img, 32, 24, 64, 48, "nonexistent", true).is_err());
    }

    #[test]
    fn layer_mask_parametric_raster_feather_and_ruby_overlay() {
        let img = vec![200u8; 16 * 16 * 4];
        let radial = apply_layer_mask_rgba(
            img.clone(),
            16,
            16,
            r#"{"enabled":true,"mode":"parametric","shape":"radial","cx":0.5,"cy":0.5,"rx":0.3,"ry":0.3,"softness":0.2}"#,
            &[],
            false,
        )
        .unwrap();
        // Center opaque, corner masked out
        let center_a = radial[(8 * 16 + 8) * 4 + 3];
        let corner_a = radial[3];
        assert!(center_a > 180, "center alpha should stay high: {center_a}");
        assert!(
            corner_a < 15,
            "corner alpha should be masked out: {corner_a}"
        );

        // Raster mask with invert + feather
        let mut rmask = vec![0u8; 16 * 16];
        for y in 4..12 {
            for x in 4..12 {
                rmask[y * 16 + x] = 255;
            }
        }
        let painted = apply_layer_mask_rgba(
            img.clone(),
            16,
            16,
            r#"{"enabled":true,"mode":"raster","shape":"none","feather":1.5}"#,
            &rmask,
            false,
        )
        .unwrap();
        assert!(painted[(8 * 16 + 8) * 4 + 3] > 180);
        assert!(painted[3] < 20);

        // Ruby overlay tints masked-out corner red without zeroing alpha
        let ruby = apply_layer_mask_rgba(
            img,
            16,
            16,
            r#"{"enabled":true,"mode":"raster","shape":"none"}"#,
            &rmask,
            true,
        )
        .unwrap();
        assert_eq!(ruby[3], 200);
        assert!(
            ruby[0] > ruby[1] + 40,
            "ruby overlay should tint corner red: {:?}",
            &ruby[0..4]
        );
    }
}
