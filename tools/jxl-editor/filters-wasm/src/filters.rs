use std::cell::RefCell;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use zenfilters::filters::{
    AdaptiveSharpen, AutoContrast, AutoExposure, AutoLevels, AutoTone, AutoVibrance,
    AutoWhiteBalance, Bloom, Blur, Brilliance, Clarity, Contrast, Exposure, FilmLook, FilmPreset,
    FusedAdjust, Grain, Grayscale, HueRotate, Invert, LocalToneMap, NoiseReduction, Posterize,
    Saturation, Sepia, Solarize, TensorLut, Texture, Vignette, WhitesBlacks,
};
use zenfilters::presets::{PresetCategory, builtin_presets};
use zenfilters::{FilterContext, Pipeline, PipelineConfig, apply_to_buffer};

use crate::mask::{oklab_l_from_srgb, smoothstep};

thread_local! {
    static FILM_LUT_CACHE: RefCell<HashMap<&'static str, TensorLut>> = RefCell::new(HashMap::new());
}

fn cached_film_look(preset: FilmPreset, strength: f32) -> FilmLook {
    let key = preset.id();
    let tensor = FILM_LUT_CACHE.with(|cache| {
        let mut map = cache.borrow_mut();
        map.entry(key)
            .or_insert_with(|| FilmLook::new(preset).tensor().clone())
            .clone()
    });
    let mut look = FilmLook::from_tensor(preset, tensor);
    look.strength = strength.clamp(0.0, 1.0);
    look
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FilterMaskSpec {
    pub kind: String,
    pub invert: bool,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub cx: f32,
    pub cy: f32,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub low: f32,
    pub high: f32,
    pub feather: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ZenFilterSpec {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub brilliance: f32,
    pub black_point: f32,
    pub white_point: f32,
    pub local_tone_map: f32,
    pub vibrance: f32,
    pub temperature: f32,
    pub tint: f32,
    pub hue: f32,
    pub dehaze: f32,
    pub clarity: f32,
    pub texture: f32,
    pub sharpen: f32,
    pub blur: f32,
    pub denoise_luma: f32,
    pub denoise_chroma: f32,
    pub vignette: f32,
    pub bloom: f32,
    pub grain: f32,
    pub sepia: f32,
    pub grayscale: bool,
    pub invert: bool,
    pub posterize: u32,
    pub solarize: f32,
    pub auto_tone: f32,
    pub auto_white_balance: f32,
    pub auto_exposure: f32,
    pub auto_contrast: f32,
    pub auto_levels: f32,
    pub auto_vibrance: f32,
    pub preset: Option<String>,
    pub preset_strength: f32,
    pub film_look: Option<String>,
    pub film_strength: f32,
    pub filter_mask: Option<FilterMaskSpec>,
}

impl Default for ZenFilterSpec {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            brilliance: 0.0,
            black_point: 0.0,
            white_point: 1.0,
            local_tone_map: 0.0,
            vibrance: 0.0,
            temperature: 0.0,
            tint: 0.0,
            hue: 0.0,
            dehaze: 0.0,
            clarity: 0.0,
            texture: 0.0,
            sharpen: 0.0,
            blur: 0.0,
            denoise_luma: 0.0,
            denoise_chroma: 0.0,
            vignette: 0.0,
            bloom: 0.0,
            grain: 0.0,
            sepia: 0.0,
            grayscale: false,
            invert: false,
            posterize: 0,
            solarize: 0.0,
            auto_tone: 0.0,
            auto_white_balance: 0.0,
            auto_exposure: 0.0,
            auto_contrast: 0.0,
            auto_levels: 0.0,
            auto_vibrance: 0.0,
            preset: None,
            preset_strength: 1.0,
            film_look: None,
            film_strength: 1.0,
            filter_mask: None,
        }
    }
}

impl ZenFilterSpec {
    fn validate(&self) -> Result<(), String> {
        let check = |val: f32, min: f32, max: f32, name: &str| -> Result<(), String> {
            if !val.is_finite() || val < min || val > max {
                return Err(format!(
                    "ZenFilters parameter '{name}' out of range [{min}, {max}]."
                ));
            }
            Ok(())
        };
        check(self.exposure, -2.0, 2.0, "exposure")?;
        check(self.contrast, 0.0, 2.0, "contrast")?;
        check(self.saturation, 0.0, 2.0, "saturation")?;
        check(self.highlights, -1.0, 1.0, "highlights")?;
        check(self.shadows, -1.0, 1.0, "shadows")?;
        check(self.whites, -1.0, 1.0, "whites")?;
        check(self.blacks, -1.0, 1.0, "blacks")?;
        check(self.brilliance, -1.0, 1.0, "brilliance")?;
        check(self.black_point, 0.0, 0.25, "blackPoint")?;
        check(self.white_point, 0.75, 1.0, "whitePoint")?;
        check(self.local_tone_map, 0.0, 1.0, "localToneMap")?;
        check(self.vibrance, -1.0, 1.0, "vibrance")?;
        check(self.temperature, -1.0, 1.0, "temperature")?;
        check(self.tint, -1.0, 1.0, "tint")?;
        check(self.hue, -180.0, 180.0, "hue")?;
        check(self.dehaze, -1.0, 1.0, "dehaze")?;
        check(self.clarity, -1.0, 1.0, "clarity")?;
        check(self.texture, -1.0, 1.0, "texture")?;
        check(self.sharpen, 0.0, 2.0, "sharpen")?;
        check(self.blur, 0.0, 20.0, "blur")?;
        check(self.denoise_luma, 0.0, 1.0, "denoiseLuma")?;
        check(self.denoise_chroma, 0.0, 1.0, "denoiseChroma")?;
        check(self.vignette, -1.0, 1.0, "vignette")?;
        check(self.bloom, 0.0, 1.0, "bloom")?;
        check(self.grain, 0.0, 1.0, "grain")?;
        check(self.sepia, 0.0, 1.0, "sepia")?;
        check(self.solarize, 0.0, 0.95, "solarize")?;
        check(self.auto_tone, 0.0, 1.0, "autoTone")?;
        check(self.auto_white_balance, 0.0, 1.0, "autoWhiteBalance")?;
        check(self.auto_exposure, 0.0, 1.0, "autoExposure")?;
        check(self.auto_contrast, 0.0, 1.0, "autoContrast")?;
        check(self.auto_levels, 0.0, 1.0, "autoLevels")?;
        check(self.auto_vibrance, 0.0, 1.0, "autoVibrance")?;
        check(self.preset_strength, 0.0, 1.0, "presetStrength")?;
        check(self.film_strength, 0.0, 1.0, "filmStrength")?;
        if self.posterize != 0 && !(2..=16).contains(&self.posterize) {
            return Err("ZenFilters parameter 'posterize' must be 0 or 2..=16.".into());
        }
        Ok(())
    }

    pub fn is_identity(&self) -> bool {
        let preset_active = self
            .preset
            .as_deref()
            .is_some_and(|p| !p.is_empty() && p != "none")
            && self.preset_strength > 1e-4;
        let film_active = self
            .film_look
            .as_deref()
            .is_some_and(|p| !p.is_empty() && p != "none")
            && self.film_strength > 1e-4;
        !preset_active
            && !film_active
            && self.exposure.abs() < 1e-5
            && (self.contrast - 1.0).abs() < 1e-5
            && (self.saturation - 1.0).abs() < 1e-5
            && self.highlights.abs() < 1e-5
            && self.shadows.abs() < 1e-5
            && self.whites.abs() < 1e-5
            && self.blacks.abs() < 1e-5
            && self.brilliance.abs() < 1e-5
            && self.black_point.abs() < 1e-5
            && (self.white_point - 1.0).abs() < 1e-5
            && self.local_tone_map.abs() < 1e-5
            && self.vibrance.abs() < 1e-5
            && self.temperature.abs() < 1e-5
            && self.tint.abs() < 1e-5
            && self.hue.abs() < 1e-4
            && self.dehaze.abs() < 1e-5
            && self.clarity.abs() < 1e-5
            && self.texture.abs() < 1e-5
            && self.sharpen.abs() < 1e-5
            && self.blur < 0.01
            && self.denoise_luma < 1e-4
            && self.denoise_chroma < 1e-4
            && self.vignette.abs() < 1e-5
            && self.bloom < 1e-4
            && self.grain < 1e-4
            && self.sepia < 1e-4
            && !self.grayscale
            && !self.invert
            && self.posterize == 0
            && self.solarize < 0.01
            && self.auto_tone < 1e-4
            && self.auto_white_balance < 1e-4
            && self.auto_exposure < 1e-4
            && self.auto_contrast < 1e-4
            && self.auto_levels < 1e-4
            && self.auto_vibrance < 1e-4
    }
}

fn push_auto_filters(spec: &ZenFilterSpec, pipe: &mut Pipeline) -> bool {
    let mut added = false;
    if spec.auto_tone > 1e-4 {
        let mut f = AutoTone::default();
        f.strength = spec.auto_tone;
        pipe.push(Box::new(f));
        added = true;
    }
    if spec.auto_white_balance > 1e-4 {
        let mut f = AutoWhiteBalance::default();
        f.strength = spec.auto_white_balance;
        pipe.push(Box::new(f));
        added = true;
    }
    if spec.auto_exposure > 1e-4 {
        let mut f = AutoExposure::default();
        f.strength = spec.auto_exposure;
        pipe.push(Box::new(f));
        added = true;
    }
    if spec.auto_contrast > 1e-4 {
        let mut f = AutoContrast::default();
        f.strength = spec.auto_contrast;
        pipe.push(Box::new(f));
        added = true;
    }
    if spec.auto_levels > 1e-4 {
        let mut f = AutoLevels::default();
        f.strength = spec.auto_levels;
        pipe.push(Box::new(f));
        added = true;
    }
    if spec.auto_vibrance > 1e-4 {
        let mut f = AutoVibrance::default();
        f.strength = spec.auto_vibrance;
        pipe.push(Box::new(f));
        added = true;
    }
    added
}

fn push_main_filters(spec: &ZenFilterSpec, pipe: &mut Pipeline) -> Result<bool, String> {
    let mut added = false;

    // Tensor-LUT Film Look
    if let Some(id) = spec.film_look.as_deref()
        && !id.is_empty()
        && id != "none"
        && spec.film_strength > 1e-4
    {
        let fp = FilmPreset::from_id(id)
            .ok_or_else(|| format!("Unknown ZenFilters film look '{id}'."))?;
        pipe.push(Box::new(cached_film_look(fp, spec.film_strength)));
        added = true;
    }

    // Core FusedAdjust (single-pass SIMD Oklab adjustments)
    let mut fused = FusedAdjust::new();
    fused.exposure = spec.exposure;
    fused.contrast = spec.contrast - 1.0;
    fused.highlights = spec.highlights;
    fused.shadows = spec.shadows;
    fused.vibrance = spec.vibrance;
    fused.saturation = spec.saturation;
    fused.temperature = spec.temperature;
    fused.tint = spec.tint;
    fused.dehaze = spec.dehaze;
    fused.black_point = spec.black_point;
    fused.white_point = spec.white_point;
    if !fused.is_identity() {
        pipe.push(Box::new(fused));
        added = true;
    }

    // Additional tone & color nodes
    if spec.whites.abs() > 1e-5 || spec.blacks.abs() > 1e-5 {
        let mut wb = WhitesBlacks::default();
        wb.whites = spec.whites;
        wb.blacks = spec.blacks;
        pipe.push(Box::new(wb));
        added = true;
    }
    if spec.brilliance.abs() > 1e-5 {
        let mut br = Brilliance::default();
        br.amount = spec.brilliance;
        pipe.push(Box::new(br));
        added = true;
    }
    if spec.local_tone_map > 1e-4 {
        pipe.push(Box::new(LocalToneMap::from_slider(
            spec.local_tone_map,
            1.0 + spec.local_tone_map * 0.4,
            32.0,
        )));
        added = true;
    }
    if spec.hue.abs() > 1e-4 {
        let mut hr = HueRotate::default();
        hr.degrees = spec.hue;
        pipe.push(Box::new(hr));
        added = true;
    }

    // Detail & optics
    if spec.denoise_luma > 1e-4 || spec.denoise_chroma > 1e-4 {
        let mut nr = NoiseReduction::default();
        nr.luminance = spec.denoise_luma;
        nr.chroma = spec.denoise_chroma;
        pipe.push(Box::new(nr));
        added = true;
    }
    if spec.clarity.abs() > 1e-5 {
        let mut cl = Clarity::default();
        cl.amount = spec.clarity;
        pipe.push(Box::new(cl));
        added = true;
    }
    if spec.texture.abs() > 1e-5 {
        let mut tx = Texture::default();
        tx.amount = spec.texture;
        pipe.push(Box::new(tx));
        added = true;
    }
    if spec.sharpen > 1e-4 {
        let mut sh = AdaptiveSharpen::default();
        sh.amount = spec.sharpen;
        pipe.push(Box::new(sh));
        added = true;
    }
    if spec.blur >= 0.01 {
        let mut b = Blur::default();
        b.sigma = spec.blur;
        pipe.push(Box::new(b));
        added = true;
    }

    // Creative effects
    if spec.bloom > 1e-4 {
        let mut bl = Bloom::default();
        bl.amount = spec.bloom;
        pipe.push(Box::new(bl));
        added = true;
    }
    if spec.vignette.abs() > 1e-5 {
        let mut vg = Vignette::default();
        vg.strength = spec.vignette;
        pipe.push(Box::new(vg));
        added = true;
    }
    if spec.grain > 1e-4 {
        let mut gr = Grain::default();
        gr.amount = spec.grain;
        pipe.push(Box::new(gr));
        added = true;
    }
    if spec.grayscale {
        pipe.push(Box::new(Grayscale::default()));
        added = true;
    }
    if spec.sepia > 1e-4 {
        let mut sep = Sepia::default();
        sep.amount = spec.sepia;
        pipe.push(Box::new(sep));
        added = true;
    }
    if spec.posterize >= 2 {
        let mut p = Posterize::default();
        p.levels = spec.posterize;
        pipe.push(Box::new(p));
        added = true;
    }
    if spec.solarize >= 0.05 {
        let mut s = Solarize::default();
        s.threshold = spec.solarize;
        pipe.push(Box::new(s));
        added = true;
    }
    if spec.invert {
        pipe.push(Box::new(Invert::default()));
        added = true;
    }

    Ok(added)
}

fn blend_with_filter_mask(
    original: &[u8],
    filtered: &mut [u8],
    width: usize,
    height: usize,
    spec: &FilterMaskSpec,
) {
    let inv_w = if width > 1 {
        1.0 / (width - 1) as f32
    } else {
        0.0
    };
    let inv_h = if height > 1 {
        1.0 / (height - 1) as f32
    } else {
        0.0
    };
    let kind = spec.kind.as_str();
    if kind.is_empty() || kind == "none" {
        return;
    }
    let dx_lin = spec.x1 - spec.x0;
    let dy_lin = spec.y1 - spec.y0;
    let len2 = (dx_lin * dx_lin + dy_lin * dy_lin).max(1e-6);

    for y in 0..height {
        let ny = y as f32 * inv_h;
        for x in 0..width {
            let nx = x as f32 * inv_w;
            let idx = (y * width + x) * 4;
            let mut m = match kind {
                "linear" => {
                    let t = ((nx - spec.x0) * dx_lin + (ny - spec.y0) * dy_lin) / len2;
                    t.clamp(0.0, 1.0)
                }
                "radial" => {
                    let d = ((nx - spec.cx).powi(2) + (ny - spec.cy).powi(2)).sqrt();
                    let inner = spec.inner_radius.clamp(0.0, 1.0);
                    let outer = spec.outer_radius.clamp(inner + 0.01, 1.5);
                    1.0 - smoothstep(inner, outer, d)
                }
                "luminance" => {
                    let l = oklab_l_from_srgb(original[idx], original[idx + 1], original[idx + 2]);
                    let low = spec.low.clamp(0.0, 1.0);
                    let high = spec.high.clamp(low, 1.0);
                    let f = spec.feather.clamp(0.0, 0.5);
                    let lo = if low <= 0.0 {
                        1.0
                    } else {
                        smoothstep(low - f, low + f * 0.5, l)
                    };
                    let hi = if high >= 1.0 {
                        1.0
                    } else {
                        1.0 - smoothstep(high - f * 0.5, high + f, l)
                    };
                    (lo * hi).clamp(0.0, 1.0)
                }
                _ => 1.0,
            };
            if spec.invert {
                m = 1.0 - m;
            }
            for c in 0..3 {
                let o = f32::from(original[idx + c]);
                let f = f32::from(filtered[idx + c]);
                filtered[idx + c] = (o + (f - o) * m).round().clamp(0.0, 255.0) as u8;
            }
            filtered[idx + 3] = original[idx + 3];
        }
    }
}

fn pack_buffer(
    buf: &filter_pixels::PixelBuffer,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let view = buf.as_slice();
    let mut packed = Vec::new();
    packed
        .try_reserve_exact(width as usize * height as usize * 4)
        .map_err(|e| e.to_string())?;
    for y in 0..height {
        packed.extend_from_slice(view.row(y));
    }
    Ok(packed)
}

pub fn apply_zenfilters_rgba_impl(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    spec_json: &str,
) -> Result<Vec<u8>, String> {
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 64_000_000
        || pixels.len() != width as usize * height as usize * 4
    {
        return Err("Invalid RGBA dimensions for ZenFilters.".into());
    }
    let spec: ZenFilterSpec =
        serde_json::from_str(spec_json).map_err(|e| format!("Invalid ZenFilters JSON: {e}"))?;
    spec.validate()?;
    if spec.is_identity() {
        return Ok(pixels);
    }

    let has_mask = spec
        .filter_mask
        .as_ref()
        .is_some_and(|m| !m.kind.is_empty() && m.kind != "none");
    let original_copy = if has_mask { Some(pixels.clone()) } else { None };

    let mut buf = filter_pixels::PixelBuffer::from_vec(
        pixels,
        width,
        height,
        filter_pixels::PixelDescriptor::RGBA8_SRGB,
    )
    .map_err(|e| e.to_string())?;
    let mut ctx = FilterContext::new();

    // Stage 1: Auto corrections
    let mut auto_pipe = Pipeline::new(PipelineConfig::default()).map_err(|e| e.to_string())?;
    if push_auto_filters(&spec, &mut auto_pipe) {
        buf = apply_to_buffer(&auto_pipe, &buf, true, &mut ctx).map_err(|e| e.to_string())?;
    }

    // Stage 2: Built-in Preset (intensity blended)
    if let Some(name) = spec.preset.as_deref()
        && !name.is_empty()
        && name != "none"
        && spec.preset_strength > 1e-4
    {
        let preset = builtin_presets()
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("Unknown ZenFilters preset '{name}'."))?;
        let preset_pipe = preset.build_pipeline_at(spec.preset_strength);
        buf = apply_to_buffer(&preset_pipe, &buf, true, &mut ctx).map_err(|e| e.to_string())?;
    }

    // Stage 3: Film Look + FusedAdjust + Tone/Color/Detail/Effects
    let mut main_pipe = Pipeline::new(PipelineConfig::default()).map_err(|e| e.to_string())?;
    if push_main_filters(&spec, &mut main_pipe)? {
        buf = apply_to_buffer(&main_pipe, &buf, true, &mut ctx).map_err(|e| e.to_string())?;
    }

    let mut packed = pack_buffer(&buf, width, height)?;
    if let (Some(orig), Some(fmask)) = (original_copy.as_deref(), spec.filter_mask.as_ref()) {
        blend_with_filter_mask(orig, &mut packed, width as usize, height as usize, fmask);
    }
    Ok(packed)
}

pub fn filter_rgba_compat(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    exposure: f32,
    contrast: f32,
    saturation: f32,
) -> Result<Vec<u8>, String> {
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 64_000_000
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
    pack_buffer(&output, width, height)
}

#[derive(Serialize)]
struct PresetEntry {
    name: String,
    category: &'static str,
    description: String,
}

#[derive(Serialize)]
struct FilmLookEntry {
    id: &'static str,
    name: &'static str,
    category: &'static str,
}

#[derive(Serialize)]
struct ControlDesc {
    id: &'static str,
    label: &'static str,
    group: &'static str,
    kind: &'static str,
    min: f32,
    max: f32,
    step: f32,
    default: f32,
    unit: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FilterCatalog {
    presets: Vec<PresetEntry>,
    film_looks: Vec<FilmLookEntry>,
    controls: Vec<ControlDesc>,
}

fn preset_category_name(c: PresetCategory) -> &'static str {
    match c {
        PresetCategory::Enhance => "Enhance",
        PresetCategory::Portrait => "Portrait",
        PresetCategory::Landscape => "Landscape",
        PresetCategory::Warm => "Warm",
        PresetCategory::Cool => "Cool",
        PresetCategory::Film => "Film",
        PresetCategory::BlackWhite => "Black & White",
        PresetCategory::Cinematic => "Cinematic",
        PresetCategory::Custom => "Custom",
    }
}

fn film_category(p: FilmPreset) -> &'static str {
    match p {
        FilmPreset::BleachBypass
        | FilmPreset::CrossProcess
        | FilmPreset::TealOrange
        | FilmPreset::FadedFilm
        | FilmPreset::GoldenHour
        | FilmPreset::Noir
        | FilmPreset::Technicolor
        | FilmPreset::Matte => "Creative Grades",
        FilmPreset::Portra
        | FilmPreset::KodakGold
        | FilmPreset::Ektar
        | FilmPreset::Superia
        | FilmPreset::Pro400H => "Classic Negative",
        FilmPreset::Velvia
        | FilmPreset::Provia
        | FilmPreset::Kodachrome
        | FilmPreset::Ektachrome => "Slide Film",
        FilmPreset::Print2383 | FilmPreset::Tungsten500T => "Motion Picture",
        FilmPreset::ClassicChrome | FilmPreset::ClassicNeg | FilmPreset::CoolChrome => {
            "Digital Film"
        }
        _ => "Cinematic Moods",
    }
}

pub fn filter_catalog_json_impl() -> String {
    let presets = builtin_presets()
        .into_iter()
        .map(|p| PresetEntry {
            name: p.name,
            category: preset_category_name(p.category),
            description: p.description,
        })
        .collect();
    let film_looks = FilmPreset::ALL
        .iter()
        .copied()
        .map(|p| FilmLookEntry {
            id: p.id(),
            name: p.name(),
            category: film_category(p),
        })
        .collect();
    let c = |id, label, group, min, max, step, default, unit| ControlDesc {
        id,
        label,
        group,
        kind: "range",
        min,
        max,
        step,
        default,
        unit,
    };
    let b = |id, label, group| ControlDesc {
        id,
        label,
        group,
        kind: "bool",
        min: 0.0,
        max: 1.0,
        step: 1.0,
        default: 0.0,
        unit: "",
    };
    let controls = vec![
        // Tone
        c(
            "highlights",
            "Highlights",
            "Tone",
            -1.0,
            1.0,
            0.02,
            0.0,
            "%",
        ),
        c("shadows", "Shadows", "Tone", -1.0, 1.0, 0.02, 0.0, "%"),
        c("whites", "Whites", "Tone", -1.0, 1.0, 0.02, 0.0, "%"),
        c("blacks", "Blacks", "Tone", -1.0, 1.0, 0.02, 0.0, "%"),
        c(
            "brilliance",
            "Brilliance",
            "Tone",
            -1.0,
            1.0,
            0.02,
            0.0,
            "%",
        ),
        c(
            "blackPoint",
            "Black Point",
            "Tone",
            0.0,
            0.25,
            0.005,
            0.0,
            "%",
        ),
        c(
            "whitePoint",
            "White Point",
            "Tone",
            0.75,
            1.0,
            0.005,
            1.0,
            "%",
        ),
        c(
            "localToneMap",
            "Local Tone Map",
            "Tone",
            0.0,
            1.0,
            0.02,
            0.0,
            "%",
        ),
        // Color
        c("vibrance", "Vibrance", "Color", -1.0, 1.0, 0.02, 0.0, "%"),
        c(
            "temperature",
            "Temperature",
            "Color",
            -1.0,
            1.0,
            0.02,
            0.0,
            "%",
        ),
        c("tint", "Tint", "Color", -1.0, 1.0, 0.02, 0.0, "%"),
        c("hue", "Hue Rotate", "Color", -180.0, 180.0, 1.0, 0.0, "°"),
        c("dehaze", "Dehaze", "Color", -1.0, 1.0, 0.02, 0.0, "%"),
        // Detail
        c("clarity", "Clarity", "Detail", -1.0, 1.0, 0.02, 0.0, "%"),
        c("texture", "Texture", "Detail", -1.0, 1.0, 0.02, 0.0, "%"),
        c(
            "sharpen",
            "Adaptive Sharpen",
            "Detail",
            0.0,
            2.0,
            0.02,
            0.0,
            "%",
        ),
        c("blur", "Gaussian Blur", "Detail", 0.0, 20.0, 0.2, 0.0, "px"),
        c(
            "denoiseLuma",
            "Denoise Luma",
            "Detail",
            0.0,
            1.0,
            0.02,
            0.0,
            "%",
        ),
        c(
            "denoiseChroma",
            "Denoise Chroma",
            "Detail",
            0.0,
            1.0,
            0.02,
            0.0,
            "%",
        ),
        // Effects
        c("vignette", "Vignette", "Effects", -1.0, 1.0, 0.02, 0.0, "%"),
        c("bloom", "Bloom", "Effects", 0.0, 1.0, 0.02, 0.0, "%"),
        c("grain", "Film Grain", "Effects", 0.0, 1.0, 0.02, 0.0, "%"),
        c("sepia", "Sepia", "Effects", 0.0, 1.0, 0.02, 0.0, "%"),
        c(
            "posterize",
            "Posterize Levels",
            "Effects",
            0.0,
            16.0,
            1.0,
            0.0,
            "lv",
        ),
        c(
            "solarize",
            "Solarize Threshold",
            "Effects",
            0.0,
            0.95,
            0.05,
            0.0,
            "%",
        ),
        b("grayscale", "Grayscale (B&W)", "Effects"),
        b("invert", "Invert Colors", "Effects"),
    ];
    serde_json::to_string(&FilterCatalog {
        presets,
        film_looks,
        controls,
    })
    .unwrap_or_else(|_| "{}".into())
}
