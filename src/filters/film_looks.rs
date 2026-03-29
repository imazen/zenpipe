use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::param_schema::*;
use crate::planes::OklabPlanes;
use crate::prelude::*;

use super::cube_lut::{CubeLut, TensorLut};

/// Film look presets using compressed tensor LUTs.
///
/// Each preset is a mathematical RGB→RGB transform decomposed into a
/// rank-8 tensor approximation (~9.5 KB per look). No copyrighted LUT
/// data — all transforms are derived from first-principles color science.
///
/// Use `FilmLook::new(preset)` to create, then use as a `Filter`.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct FilmLook {
    tensor: TensorLut,
    /// Blend strength. 1.0 = full effect, 0.0 = bypass.
    pub strength: f32,
    preset: FilmPreset,
}

/// Available film look presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum FilmPreset {
    // ── Creative grades ──────────────────────────────────────────
    /// Bleach bypass: high contrast, desaturated, gritty.
    BleachBypass,
    /// Cross-processed: shifted color channels, saturated, punchy.
    CrossProcess,
    /// Teal and orange: cinematic complementary color grade.
    TealOrange,
    /// Faded film: lifted blacks, low contrast, muted colors.
    FadedFilm,
    /// Golden hour: warm light, soft contrast, glowing highlights.
    GoldenHour,
    /// Noir: high contrast, heavy desaturation, deep blacks.
    Noir,
    /// Technicolor: vivid, two-strip Technicolor-inspired rendering.
    Technicolor,
    /// Matte: lifted blacks, reduced highlights, editorial look.
    Matte,

    // ── Classic negative film ────────────────────────────────────
    /// Portra-inspired: warm skin tones, soft contrast, muted greens.
    /// The portrait film aesthetic.
    Portra,
    /// Gold-inspired: warm, saturated consumer film. Nostalgic.
    KodakGold,
    /// Ektar-inspired: ultra-saturated, punchy, fine-grain colors.
    Ektar,
    /// Superia-inspired: cool-toned consumer film, slight green cast.
    Superia,
    /// Pro 400H-inspired: clean, slightly cool, beautiful skin tones.
    Pro400H,

    // ── Slide film (reversal) ────────────────────────────────────
    /// Velvia-inspired: hyper-saturated, vivid greens and blues.
    /// The landscape photographer's film.
    Velvia,
    /// Provia-inspired: neutral, accurate slide film.
    Provia,
    /// Kodachrome-inspired: deep reds, vivid yellows, rich blacks.
    /// Legendary color rendering.
    Kodachrome,
    /// Ektachrome-inspired: clean, slightly warm modern slide film.
    Ektachrome,

    // ── Motion picture ───────────────────────────────────────────
    /// 2383 print-inspired: warm shadows, soft shoulder rolloff.
    /// The Hollywood print stock character.
    Print2383,
    /// 500T-inspired: tungsten-balanced cinema negative. Warm, cinematic.
    Tungsten500T,

    // ── Digital era / Fuji sim-inspired ──────────────────────────
    /// Classic Chrome-inspired: muted, desaturated, documentary feel.
    ClassicChrome,
    /// Classic Negative-inspired: high contrast, warm highlights, cool shadows.
    ClassicNeg,
    /// Cool chrome: slight blue-green cast, punchy contrast.
    CoolChrome,
}

impl FilmPreset {
    /// All available presets.
    pub const ALL: &[FilmPreset] = &[
        // Creative
        Self::BleachBypass,
        Self::CrossProcess,
        Self::TealOrange,
        Self::FadedFilm,
        Self::GoldenHour,
        Self::Noir,
        Self::Technicolor,
        Self::Matte,
        // Classic negative
        Self::Portra,
        Self::KodakGold,
        Self::Ektar,
        Self::Superia,
        Self::Pro400H,
        // Slide
        Self::Velvia,
        Self::Provia,
        Self::Kodachrome,
        Self::Ektachrome,
        // Motion picture
        Self::Print2383,
        Self::Tungsten500T,
        // Digital era
        Self::ClassicChrome,
        Self::ClassicNeg,
        Self::CoolChrome,
    ];

    /// Human-readable name.
    pub fn name(self) -> &'static str {
        match self {
            Self::BleachBypass => "Bleach Bypass",
            Self::CrossProcess => "Cross Process",
            Self::TealOrange => "Teal & Orange",
            Self::FadedFilm => "Faded Film",
            Self::GoldenHour => "Golden Hour",
            Self::Noir => "Noir",
            Self::Technicolor => "Technicolor",
            Self::Matte => "Matte",
            Self::Portra => "Portra",
            Self::KodakGold => "Kodak Gold",
            Self::Ektar => "Ektar",
            Self::Superia => "Superia",
            Self::Pro400H => "Pro 400H",
            Self::Velvia => "Velvia",
            Self::Provia => "Provia",
            Self::Kodachrome => "Kodachrome",
            Self::Ektachrome => "Ektachrome",
            Self::Print2383 => "Print 2383",
            Self::Tungsten500T => "500T Tungsten",
            Self::ClassicChrome => "Classic Chrome",
            Self::ClassicNeg => "Classic Negative",
            Self::CoolChrome => "Cool Chrome",
        }
    }

    /// Machine identifier.
    pub fn id(self) -> &'static str {
        match self {
            Self::BleachBypass => "bleach_bypass",
            Self::CrossProcess => "cross_process",
            Self::TealOrange => "teal_orange",
            Self::FadedFilm => "faded_film",
            Self::GoldenHour => "golden_hour",
            Self::Noir => "noir",
            Self::Technicolor => "technicolor",
            Self::Matte => "matte",
            Self::Portra => "portra",
            Self::KodakGold => "kodak_gold",
            Self::Ektar => "ektar",
            Self::Superia => "superia",
            Self::Pro400H => "pro_400h",
            Self::Velvia => "velvia",
            Self::Provia => "provia",
            Self::Kodachrome => "kodachrome",
            Self::Ektachrome => "ektachrome",
            Self::Print2383 => "print_2383",
            Self::Tungsten500T => "tungsten_500t",
            Self::ClassicChrome => "classic_chrome",
            Self::ClassicNeg => "classic_neg",
            Self::CoolChrome => "cool_chrome",
        }
    }

    /// Look up a preset by its machine identifier.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|p| p.id() == id)
    }
}

/// LUT generation size and tensor rank for presets.
const PRESET_LUT_SIZE: usize = 17;
const PRESET_RANK: usize = 8;
const PRESET_ALS_ITERATIONS: usize = 25;

impl FilmLook {
    /// Create a film look from a preset.
    ///
    /// Generates the LUT and decomposes it on first call.
    /// The result is ~5–10 KB in memory.
    pub fn new(preset: FilmPreset) -> Self {
        let lut = generate_preset_lut(preset);
        let tensor = TensorLut::decompose(&lut, PRESET_RANK, PRESET_ALS_ITERATIONS);
        Self {
            tensor,
            strength: 1.0,
            preset,
        }
    }

    /// Create from a pre-computed TensorLut (for embedded presets).
    pub fn from_tensor(preset: FilmPreset, tensor: TensorLut) -> Self {
        Self {
            tensor,
            strength: 1.0,
            preset,
        }
    }

    /// Which preset this look uses.
    pub fn preset(&self) -> FilmPreset {
        self.preset
    }

    /// Access the underlying tensor LUT.
    pub fn tensor(&self) -> &TensorLut {
        &self.tensor
    }

    /// Storage size in bytes.
    pub fn size_bytes(&self) -> usize {
        self.tensor.size_bytes()
    }
}

static FILM_LOOK_SCHEMA: FilterSchema = FilterSchema {
    name: "film_look",
    label: "Film Look",
    description: "Film emulation presets using compressed tensor LUTs",
    group: FilterGroup::Color,
    params: &[ParamDesc {
        name: "strength",
        label: "Strength",
        description: "Blend strength (0 = bypass, 1 = full effect)",
        kind: ParamKind::Float {
            min: 0.0,
            max: 1.0,
            default: 1.0,
            identity: 0.0,
            step: 0.05,
        },
        unit: "",
        section: "Main",
        slider: SliderMapping::Linear,
    }],
};

impl Describe for FilmLook {
    fn schema() -> &'static FilterSchema {
        &FILM_LOOK_SCHEMA
    }

    fn get_param(&self, name: &str) -> Option<ParamValue> {
        match name {
            "strength" => Some(ParamValue::Float(self.strength)),
            _ => None,
        }
    }

    fn set_param(&mut self, name: &str, value: ParamValue) -> bool {
        let v = match value.as_f32() {
            Some(v) => v,
            None => return false,
        };
        match name {
            "strength" => self.strength = v,
            _ => return false,
        }
        true
    }
}

impl Filter for FilmLook {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength.abs() < 1e-6 {
            return;
        }

        use zenpixels_convert::oklab;
        let m1_inv = oklab::lms_to_rgb_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");
        let m1 = oklab::rgb_to_lms_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");

        let n = planes.pixel_count();
        let blend = self.strength.clamp(0.0, 1.0);
        let inv_blend = 1.0 - blend;

        for i in 0..n {
            let [r, g, b] = oklab::oklab_to_rgb(planes.l[i], planes.a[i], planes.b[i], &m1_inv);
            let rgb = [r.max(0.0), g.max(0.0), b.max(0.0)];
            let lut_rgb = self.tensor.lookup(rgb);

            let r2 = inv_blend * r + blend * lut_rgb[0];
            let g2 = inv_blend * g + blend * lut_rgb[1];
            let b2 = inv_blend * b + blend * lut_rgb[2];

            let [l, oa, ob] = oklab::rgb_to_oklab(r2.max(0.0), g2.max(0.0), b2.max(0.0), &m1);
            planes.l[i] = l;
            planes.a[i] = oa;
            planes.b[i] = ob;
        }
    }
}

// ── Preset LUT generators ────────────────────────────────────────────
//
// Each generates a mathematical RGB→RGB transform. All operate on
// linear [0,1] RGB. No copyrighted data.

fn generate_preset_lut(preset: FilmPreset) -> CubeLut {
    let size = PRESET_LUT_SIZE;
    let mut lut = CubeLut::identity(size);
    let scale = 1.0 / (size - 1) as f32;

    for ri in 0..size {
        for gi in 0..size {
            for bi in 0..size {
                let r = ri as f32 * scale;
                let g = gi as f32 * scale;
                let b = bi as f32 * scale;
                let idx = ri * size * size + gi * size + bi;
                lut.data_mut()[idx] = match preset {
                    FilmPreset::BleachBypass => bleach_bypass(r, g, b),
                    FilmPreset::CrossProcess => cross_process(r, g, b),
                    FilmPreset::TealOrange => teal_orange(r, g, b),
                    FilmPreset::FadedFilm => faded_film(r, g, b),
                    FilmPreset::GoldenHour => golden_hour(r, g, b),
                    FilmPreset::Noir => noir(r, g, b),
                    FilmPreset::Technicolor => technicolor(r, g, b),
                    FilmPreset::Matte => matte(r, g, b),
                    FilmPreset::Portra => portra(r, g, b),
                    FilmPreset::KodakGold => kodak_gold(r, g, b),
                    FilmPreset::Ektar => ektar(r, g, b),
                    FilmPreset::Superia => superia(r, g, b),
                    FilmPreset::Pro400H => pro_400h(r, g, b),
                    FilmPreset::Velvia => velvia(r, g, b),
                    FilmPreset::Provia => provia(r, g, b),
                    FilmPreset::Kodachrome => kodachrome(r, g, b),
                    FilmPreset::Ektachrome => ektachrome(r, g, b),
                    FilmPreset::Print2383 => print_2383(r, g, b),
                    FilmPreset::Tungsten500T => tungsten_500t(r, g, b),
                    FilmPreset::ClassicChrome => classic_chrome(r, g, b),
                    FilmPreset::ClassicNeg => classic_neg(r, g, b),
                    FilmPreset::CoolChrome => cool_chrome(r, g, b),
                };
            }
        }
    }
    lut
}

/// BT.709 luminance.
#[inline]
fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// S-curve: smooth contrast boost.
#[inline]
fn s_curve(x: f32, strength: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    // Attempt smooth blend so 0 strength = identity
    let curved = if x < 0.5 {
        2.0 * x * x
    } else {
        1.0 - 2.0 * (1.0 - x) * (1.0 - x)
    };
    x + strength * (curved - x)
}

/// Inverse S-curve: reduce contrast.
#[inline]
fn inv_s_curve(x: f32, strength: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let flat = if x < 0.5 {
        (x * 0.5).sqrt()
    } else {
        1.0 - (0.5 * (1.0 - x)).sqrt()
    };
    x + strength * (flat - x)
}

/// Desaturate toward luma by a factor.
#[inline]
fn desat(r: f32, g: f32, b: f32, amount: f32) -> [f32; 3] {
    let l = luma(r, g, b);
    [
        (r + amount * (l - r)).clamp(0.0, 1.0),
        (g + amount * (l - g)).clamp(0.0, 1.0),
        (b + amount * (l - b)).clamp(0.0, 1.0),
    ]
}

/// Film shoulder rolloff.
#[inline]
fn shoulder(x: f32, knee: f32) -> f32 {
    if x < knee {
        x
    } else {
        let over = x - knee;
        let range = 1.0 - knee;
        knee + range * (1.0 - (-over / range).exp())
    }
}

fn bleach_bypass(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Strong S-curve + heavy desaturation
    let [r, g, b] = desat(r, g, b, 0.6);
    [
        s_curve(r, 0.8).clamp(0.0, 1.0),
        s_curve(g, 0.8).clamp(0.0, 1.0),
        s_curve(b, 0.8).clamp(0.0, 1.0),
    ]
}

fn cross_process(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Per-channel curve shifts: boost R highlights, suppress B shadows
    let r_out = s_curve(r, 0.4) + 0.02;
    let g_out = inv_s_curve(g, 0.2);
    let b_out = s_curve(b * 0.9, 0.3) - 0.03;
    [
        r_out.clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        b_out.clamp(0.0, 1.0),
    ]
}

fn teal_orange(r: f32, g: f32, b: f32) -> [f32; 3] {
    let l = luma(r, g, b);
    // Shadow → teal (reduce R, boost B slightly), highlight → warm
    let shadow = (1.0 - l * 2.0).max(0.0); // 1 at black, 0 at mid
    let highlight = ((l - 0.5) * 2.0).max(0.0); // 0 at mid, 1 at white

    let r_out = r - shadow * 0.06 + highlight * 0.04;
    let g_out = g - shadow * 0.01 - highlight * 0.01;
    let b_out = b + shadow * 0.05 - highlight * 0.05;

    // Mild S-curve for punch
    [
        s_curve(r_out, 0.3).clamp(0.0, 1.0),
        s_curve(g_out, 0.3).clamp(0.0, 1.0),
        s_curve(b_out, 0.3).clamp(0.0, 1.0),
    ]
}

fn faded_film(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Lift blacks, lower whites, desaturate
    let lift = 0.06;
    let ceil = 0.94;
    let range = ceil - lift;
    let r_out = lift + r * range;
    let g_out = lift + g * range;
    let b_out = lift + b * range;
    let [r_out, g_out, b_out] = desat(r_out, g_out, b_out, 0.3);
    // Slight warm shift in shadows
    let l = luma(r, g, b);
    let shadow = (1.0 - l * 3.0).max(0.0);
    [
        (r_out + shadow * 0.02).clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        (b_out - shadow * 0.01).clamp(0.0, 1.0),
    ]
}

fn golden_hour(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Warm shift, lifted shadows, soft shoulder
    let r_out = shoulder(r * 1.05 + 0.03, 0.85);
    let g_out = shoulder(g * 1.0 + 0.01, 0.88);
    let b_out = shoulder(b * 0.88, 0.9);
    [
        r_out.clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        b_out.clamp(0.0, 1.0),
    ]
}

fn cool_chrome(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Slight blue-green cast, punchy contrast
    let r_out = s_curve(r * 0.98, 0.4);
    let g_out = s_curve(g * 1.0 + 0.01, 0.3);
    let b_out = s_curve(b * 1.04 + 0.02, 0.3);
    [
        r_out.clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        b_out.clamp(0.0, 1.0),
    ]
}

fn noir(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Heavy desat, strong S-curve, crush blacks
    let [r, g, b] = desat(r, g, b, 0.85);
    let r_out = s_curve((r - 0.02).max(0.0), 0.7);
    let g_out = s_curve((g - 0.02).max(0.0), 0.7);
    let b_out = s_curve((b - 0.02).max(0.0), 0.7);
    [
        r_out.clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        b_out.clamp(0.0, 1.0),
    ]
}

fn technicolor(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Vivid, saturated, slightly warm. Inspired by two-strip process:
    // boost reds and cyans, compress greens
    let r_out = r * 1.08 + 0.01;
    let g_out = g * 0.95;
    let b_out = b * 1.04;
    // Slight S-curve for punch
    [
        s_curve(r_out, 0.3).clamp(0.0, 1.0),
        s_curve(g_out, 0.2).clamp(0.0, 1.0),
        s_curve(b_out, 0.25).clamp(0.0, 1.0),
    ]
}

fn matte(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Lifted blacks, lowered highlights, low saturation
    let lift = 0.08;
    let ceil = 0.90;
    let range = ceil - lift;
    let r_out = lift + inv_s_curve(r, 0.2) * range;
    let g_out = lift + inv_s_curve(g, 0.2) * range;
    let b_out = lift + inv_s_curve(b, 0.2) * range;
    let [r_out, g_out, b_out] = desat(r_out, g_out, b_out, 0.25);
    [
        r_out.clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        b_out.clamp(0.0, 1.0),
    ]
}

// ── Classic negative film stocks ─────────────────────────────────────

fn portra(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Warm skin tones, soft contrast, muted greens, wide latitude
    let l = luma(r, g, b);
    // Soft S-curve (low contrast negative film)
    let r_out = inv_s_curve(r, -0.15) + 0.01; // slight warm push
    let g_out = inv_s_curve(g, -0.15) * 0.97; // mute greens slightly
    let b_out = inv_s_curve(b, -0.15) - 0.005;
    // Warm shadows
    let shadow = (1.0 - l * 2.5).max(0.0);
    let r_out = r_out + shadow * 0.025;
    let b_out = b_out - shadow * 0.015;
    // Soft shoulder
    [
        shoulder(r_out, 0.88).clamp(0.0, 1.0),
        shoulder(g_out, 0.90).clamp(0.0, 1.0),
        shoulder(b_out, 0.92).clamp(0.0, 1.0),
    ]
}

fn kodak_gold(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Warm, saturated consumer film. Golden tones, punchy.
    let l = luma(r, g, b);
    // Warm overall push
    let r_out = r * 1.06 + 0.015;
    let g_out = g * 1.01;
    let b_out = b * 0.92 - 0.01;
    // Moderate S-curve for consumer punch
    let r_out = s_curve(r_out, 0.25);
    let g_out = s_curve(g_out, 0.2);
    let b_out = s_curve(b_out, 0.2);
    // Warm shadow lift
    let shadow = (1.0 - l * 3.0).max(0.0);
    [
        (r_out + shadow * 0.02).clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        (b_out - shadow * 0.01).clamp(0.0, 1.0),
    ]
}

fn ektar(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Ultra-saturated, punchy, fine grain. Boost everything.
    // Strong S-curve, vivid colors
    let r_out = s_curve(r * 1.05, 0.35);
    let g_out = s_curve(g * 1.04, 0.3);
    let b_out = s_curve(b * 1.06, 0.3);
    // Slight warm bias
    [
        (r_out + 0.01).clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        b_out.clamp(0.0, 1.0),
    ]
}

fn superia(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Cool-toned consumer film, slight green cast, moderate contrast
    let r_out = s_curve(r * 0.97, 0.2);
    let g_out = s_curve(g * 1.02 + 0.008, 0.2); // slight green push
    let b_out = s_curve(b * 1.01, 0.2);
    // Cool shadow tint
    let l = luma(r, g, b);
    let shadow = (1.0 - l * 2.5).max(0.0);
    [
        (r_out - shadow * 0.01).clamp(0.0, 1.0),
        (g_out + shadow * 0.005).clamp(0.0, 1.0),
        (b_out + shadow * 0.01).clamp(0.0, 1.0),
    ]
}

fn pro_400h(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Clean, slightly cool, beautiful skin tones. Low contrast.
    // Distinctive pastel quality
    let r_out = inv_s_curve(r, -0.1);
    let g_out = inv_s_curve(g * 1.01, -0.1);
    let b_out = inv_s_curve(b * 1.03 + 0.005, -0.1); // slight cool push
    // Slight desaturation for pastel quality
    let [r_out, g_out, b_out] = desat(r_out, g_out, b_out, 0.12);
    [
        shoulder(r_out, 0.90).clamp(0.0, 1.0),
        shoulder(g_out, 0.91).clamp(0.0, 1.0),
        shoulder(b_out, 0.92).clamp(0.0, 1.0),
    ]
}

// ── Slide film stocks ────────────────────────────────────────────────

fn velvia(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Hyper-saturated, vivid greens and blues, crushed blacks
    // The landscape photographer's film
    let l = luma(r, g, b);
    // Strong saturation boost (opposite of desat)
    let boost = 0.3;
    let r_out = r + boost * (r - l);
    let g_out = g + boost * (g - l) + 0.01; // extra green boost
    let b_out = b + boost * (b - l) + 0.005;
    // Strong S-curve, crushed blacks
    let r_out = s_curve((r_out - 0.01).max(0.0), 0.5);
    let g_out = s_curve((g_out - 0.01).max(0.0), 0.45);
    let b_out = s_curve((b_out - 0.01).max(0.0), 0.45);
    [
        r_out.clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        b_out.clamp(0.0, 1.0),
    ]
}

fn provia(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Neutral, accurate, slight saturation boost. The versatile slide film.
    let l = luma(r, g, b);
    // Gentle saturation boost
    let boost = 0.08;
    let r_out = r + boost * (r - l);
    let g_out = g + boost * (g - l);
    let b_out = b + boost * (b - l);
    // Mild S-curve
    [
        s_curve(r_out, 0.15).clamp(0.0, 1.0),
        s_curve(g_out, 0.15).clamp(0.0, 1.0),
        s_curve(b_out, 0.15).clamp(0.0, 1.0),
    ]
}

fn kodachrome(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Deep reds, vivid yellows, rich blacks, cyan-shifted shadows
    let l = luma(r, g, b);
    // Strong contrast
    let r_out = s_curve(r * 1.04, 0.45);
    let g_out = s_curve(g * 0.98, 0.4);
    let b_out = s_curve(b * 0.94, 0.4);
    // Warm highlights, cyan-ish shadows
    let shadow = (1.0 - l * 2.0).max(0.0);
    let highlight = ((l - 0.5) * 2.0).max(0.0);
    let r_out = r_out + highlight * 0.02 - shadow * 0.02;
    let g_out = g_out;
    let b_out = b_out + shadow * 0.02 - highlight * 0.01;
    // Crush blacks slightly
    [
        ((r_out - 0.01).max(0.0)).clamp(0.0, 1.0),
        ((g_out - 0.01).max(0.0)).clamp(0.0, 1.0),
        ((b_out - 0.01).max(0.0)).clamp(0.0, 1.0),
    ]
}

fn ektachrome(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Clean, slightly warm modern slide film. Less saturated than Velvia.
    let l = luma(r, g, b);
    let boost = 0.12;
    let r_out = r + boost * (r - l) + 0.005;
    let g_out = g + boost * (g - l);
    let b_out = b + boost * (b - l) - 0.003;
    // Moderate S-curve
    [
        s_curve(r_out, 0.2).clamp(0.0, 1.0),
        s_curve(g_out, 0.2).clamp(0.0, 1.0),
        s_curve(b_out, 0.2).clamp(0.0, 1.0),
    ]
}

// ── Motion picture stocks ────────────────────────────────────────────

fn print_2383(r: f32, g: f32, b: f32) -> [f32; 3] {
    // THE Hollywood print stock. Warm shadows, soft highlight shoulder,
    // flattering skin tones, ~13 stops of DR feeling.
    let l = luma(r, g, b);
    let shadow = (1.0 - l * 2.0).max(0.0);
    let highlight = ((l - 0.5) * 2.0).max(0.0);
    // Warm shadow push, slightly cool highlights
    let r_out = r + shadow * 0.04 + highlight * 0.01;
    let g_out = g + shadow * 0.01;
    let b_out = b - shadow * 0.025 + highlight * 0.005;
    // Soft shoulder rolloff (the signature 2383 characteristic)
    let r_out = shoulder(r_out, 0.78);
    let g_out = shoulder(g_out, 0.82);
    let b_out = shoulder(b_out, 0.85);
    // Mild highlight desaturation
    let [r_out, g_out, b_out] = desat(r_out, g_out, b_out, highlight * 0.15);
    // Slight S-curve for body
    [
        s_curve(r_out, 0.15).clamp(0.0, 1.0),
        s_curve(g_out, 0.12).clamp(0.0, 1.0),
        s_curve(b_out, 0.12).clamp(0.0, 1.0),
    ]
}

fn tungsten_500t(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Cinema negative balanced for tungsten (3200K). Under daylight
    // gives a cool blue cast. Warm, cinematic, wide DR.
    let l = luma(r, g, b);
    // Warm overall (tungsten WB on daylight-lit scenes)
    let r_out = r * 1.03 + 0.01;
    let g_out = g * 0.99;
    let b_out = b * 0.93;
    // Low contrast negative character
    let r_out = inv_s_curve(r_out, -0.1);
    let g_out = inv_s_curve(g_out, -0.1);
    let b_out = inv_s_curve(b_out, -0.1);
    // Soft shoulder
    let r_out = shoulder(r_out, 0.84);
    let g_out = shoulder(g_out, 0.86);
    let b_out = shoulder(b_out, 0.88);
    // Warm shadow lift
    let shadow = (1.0 - l * 2.5).max(0.0);
    [
        (r_out + shadow * 0.02).clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        (b_out - shadow * 0.01).clamp(0.0, 1.0),
    ]
}

// ── Digital era looks ────────────────────────────────────────────────

fn classic_chrome(r: f32, g: f32, b: f32) -> [f32; 3] {
    // Muted, desaturated, documentary feel. Understated.
    let l = luma(r, g, b);
    // Desaturate
    let [r, g, b] = desat(r, g, b, 0.25);
    // Slightly warm midtones, cool shadows
    let shadow = (1.0 - l * 2.5).max(0.0);
    let r_out = r + 0.005 - shadow * 0.01;
    let g_out = g;
    let b_out = b - 0.005 + shadow * 0.01;
    // Mild contrast
    [
        s_curve(r_out, 0.2).clamp(0.0, 1.0),
        s_curve(g_out, 0.2).clamp(0.0, 1.0),
        s_curve(b_out, 0.2).clamp(0.0, 1.0),
    ]
}

fn classic_neg(r: f32, g: f32, b: f32) -> [f32; 3] {
    // High contrast, warm highlights, cool shadows. Distinctive split.
    let l = luma(r, g, b);
    let shadow = (1.0 - l * 2.0).max(0.0);
    let highlight = ((l - 0.5) * 2.0).max(0.0);
    // Cool shadows, warm highlights
    let r_out = r - shadow * 0.04 + highlight * 0.03;
    let g_out = g - shadow * 0.01;
    let b_out = b + shadow * 0.03 - highlight * 0.04;
    // Strong contrast
    let r_out = s_curve(r_out, 0.4);
    let g_out = s_curve(g_out, 0.35);
    let b_out = s_curve(b_out, 0.35);
    // Slight desaturation
    let [r_out, g_out, b_out] = desat(r_out, g_out, b_out, 0.15);
    [
        r_out.clamp(0.0, 1.0),
        g_out.clamp(0.0, 1.0),
        b_out.clamp(0.0, 1.0),
    ]
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn all_presets_build() {
        for &preset in FilmPreset::ALL {
            let look = FilmLook::new(preset);
            assert!(look.size_bytes() > 0, "{}: zero size", preset.name());
            std::eprintln!(
                "{:15} {:>6} bytes  (rank {}, grid {})",
                preset.name(),
                look.size_bytes(),
                PRESET_RANK,
                look.tensor().grid_size(),
            );
        }
    }

    #[test]
    fn all_presets_accuracy() {
        for &preset in FilmPreset::ALL {
            let lut = generate_preset_lut(preset);
            let look = FilmLook::new(preset);
            let acc = lut.measure_accuracy(&|rgb| look.tensor().lookup(rgb), 33);
            let max_8bit = (acc.max_diff * 255.0).ceil() as u32;
            std::eprintln!(
                "{:15} max={:.4} ({:>2}@8bit) avg={:.6}",
                preset.name(),
                acc.max_diff,
                max_8bit,
                acc.avg_diff,
            );
            assert!(
                acc.max_diff < 0.1,
                "{}: max_diff too high: {}",
                preset.name(),
                acc.max_diff
            );
        }
    }

    #[test]
    fn strength_zero_is_bypass() {
        let mut look = FilmLook::new(FilmPreset::BleachBypass);
        look.strength = 0.0;

        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.3 + (i as f32) * 0.01;
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        let l_orig = planes.l.clone();
        look.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.l, l_orig);
    }

    #[test]
    fn preset_from_id_roundtrip() {
        for &preset in FilmPreset::ALL {
            let id = preset.id();
            let back = FilmPreset::from_id(id).unwrap();
            assert_eq!(back, preset);
        }
    }

    #[test]
    fn tensor_serialization_roundtrip() {
        let look = FilmLook::new(FilmPreset::TealOrange);
        let bytes = look.tensor().to_bytes();
        let restored = TensorLut::from_bytes(&bytes).unwrap();
        // Spot-check a few values
        let test_pts = [[0.5, 0.3, 0.7], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]];
        for pt in &test_pts {
            let a = look.tensor().lookup(*pt);
            let b = restored.lookup(*pt);
            for ch in 0..3 {
                assert!(
                    (a[ch] - b[ch]).abs() < 1e-6,
                    "serialization mismatch at {pt:?} ch{ch}: {} vs {}",
                    a[ch],
                    b[ch]
                );
            }
        }
    }

    #[test]
    fn total_embedded_size() {
        let mut total = 0;
        for &preset in FilmPreset::ALL {
            let look = FilmLook::new(preset);
            total += look.size_bytes();
        }
        std::eprintln!(
            "Total embedded size for {} presets: {} bytes ({:.1} KB)",
            FilmPreset::ALL.len(),
            total,
            total as f64 / 1024.0,
        );
        // Should be well under 200 KB total for 22 presets (~5 KB each)
        assert!(total < 200_000, "presets too large: {} bytes", total);
    }
}
