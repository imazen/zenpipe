//! Generate side-by-side montages for every tunable filter at multiple parameter levels.
//!
//! Used with scripts/filter_audit_eval.py to detect broken, no-effect, or banding filters
//! via OpenAI Vision API.
//!
//! Usage:
//!   cargo run --release --features experimental --example filter_audit [output_dir]
//!
//! Output defaults to /mnt/v/output/zenfilters/audit/

use std::fs;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageReader, RgbImage};
use zenfilters::filters::*;
use zenfilters::{
    Filter, FilterContext, OklabPlanes, gather_oklab_to_srgb_u8, scatter_srgb_u8_to_oklab,
};
use zenpixels::ColorPrimaries;
use zenpixels_convert::oklab;

const MAX_DIM: u32 = 512;
const JPEG_QUALITY: u8 = 85;
const MAX_IMAGES: usize = 6;

struct AuditConfig {
    filter_name: &'static str,
    level: &'static str,
    params: String,
    filter: Box<dyn Filter>,
}

/// Helper: create a filter from Default + field mutation.
macro_rules! mk {
    ($ty:ty, $($field:ident = $val:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut f = <$ty>::default();
        $(f.$field = $val;)*
        f
    }};
}

fn audit_configs() -> Vec<AuditConfig> {
    let mut c: Vec<AuditConfig> = Vec::new();

    macro_rules! add {
        ($name:expr, $level:expr, $params:expr, $filter:expr) => {
            c.push(AuditConfig {
                filter_name: $name,
                level: $level,
                params: $params.to_string(),
                filter: Box::new($filter),
            });
        };
    }

    // ── Tone ──────────────────────────────────────────────────────────
    add!(
        "Exposure",
        "subtle",
        "stops=0.3",
        mk!(Exposure, stops = 0.3)
    );
    add!(
        "Exposure",
        "moderate",
        "stops=1.0",
        mk!(Exposure, stops = 1.0)
    );
    add!(
        "Exposure",
        "extreme",
        "stops=3.0",
        mk!(Exposure, stops = 3.0)
    );

    add!(
        "Contrast",
        "subtle",
        "amount=0.1",
        mk!(Contrast, amount = 0.1)
    );
    add!(
        "Contrast",
        "moderate",
        "amount=0.3",
        mk!(Contrast, amount = 0.3)
    );
    add!(
        "Contrast",
        "extreme",
        "amount=0.8",
        mk!(Contrast, amount = 0.8)
    );

    add!("Levels", "subtle", "gamma=1.3", mk!(Levels, gamma = 1.3));
    add!(
        "Levels",
        "moderate",
        "gamma=2.0, in_black=0.1",
        mk!(Levels, gamma = 2.0, in_black = 0.1)
    );
    add!(
        "Levels",
        "extreme",
        "gamma=3.0, in_black=0.2, out_white=0.8",
        mk!(Levels, gamma = 3.0, in_black = 0.2, out_white = 0.8)
    );

    add!(
        "Sigmoid",
        "subtle",
        "contrast=1.0",
        mk!(Sigmoid, contrast = 1.0)
    );
    add!(
        "Sigmoid",
        "moderate",
        "contrast=2.0",
        mk!(Sigmoid, contrast = 2.0)
    );
    add!(
        "Sigmoid",
        "extreme",
        "contrast=4.0",
        mk!(Sigmoid, contrast = 4.0)
    );

    add!(
        "HighlightsShadows",
        "subtle",
        "h=-0.2, s=0.2",
        mk!(HighlightsShadows, highlights = -0.2, shadows = 0.2)
    );
    add!(
        "HighlightsShadows",
        "moderate",
        "h=-0.5, s=0.5",
        mk!(HighlightsShadows, highlights = -0.5, shadows = 0.5)
    );
    add!(
        "HighlightsShadows",
        "extreme",
        "h=-1.0, s=1.0",
        mk!(HighlightsShadows, highlights = -1.0, shadows = 1.0)
    );

    add!(
        "WhitesBlacks",
        "subtle",
        "w=0.1, b=-0.1",
        mk!(WhitesBlacks, whites = 0.1, blacks = -0.1)
    );
    add!(
        "WhitesBlacks",
        "moderate",
        "w=0.3, b=-0.3",
        mk!(WhitesBlacks, whites = 0.3, blacks = -0.3)
    );
    add!(
        "WhitesBlacks",
        "extreme",
        "w=0.8, b=-0.8",
        mk!(WhitesBlacks, whites = 0.8, blacks = -0.8)
    );

    add!(
        "BlackPoint",
        "subtle",
        "level=0.02",
        mk!(BlackPoint, level = 0.02)
    );
    add!(
        "BlackPoint",
        "moderate",
        "level=0.1",
        mk!(BlackPoint, level = 0.1)
    );
    add!(
        "BlackPoint",
        "extreme",
        "level=0.3",
        mk!(BlackPoint, level = 0.3)
    );

    add!(
        "WhitePoint",
        "subtle",
        "level=0.95",
        mk!(WhitePoint, level = 0.95)
    );
    add!(
        "WhitePoint",
        "moderate",
        "level=0.85",
        mk!(WhitePoint, level = 0.85)
    );
    add!(
        "WhitePoint",
        "extreme",
        "level=0.6",
        mk!(WhitePoint, level = 0.6)
    );

    add!(
        "ShadowLift",
        "subtle",
        "strength=0.1",
        mk!(ShadowLift, strength = 0.1)
    );
    add!(
        "ShadowLift",
        "moderate",
        "strength=0.3",
        mk!(ShadowLift, strength = 0.3)
    );
    add!(
        "ShadowLift",
        "extreme",
        "strength=0.7",
        mk!(ShadowLift, strength = 0.7)
    );

    add!(
        "HighlightRecovery",
        "subtle",
        "strength=0.3",
        mk!(HighlightRecovery, strength = 0.3)
    );
    add!(
        "HighlightRecovery",
        "moderate",
        "strength=0.7",
        mk!(HighlightRecovery, strength = 0.7)
    );
    add!(
        "HighlightRecovery",
        "extreme",
        "strength=1.0",
        mk!(HighlightRecovery, strength = 1.0)
    );

    // ── Color ─────────────────────────────────────────────────────────
    add!(
        "Saturation",
        "subtle",
        "factor=1.15",
        mk!(Saturation, factor = 1.15)
    );
    add!(
        "Saturation",
        "moderate",
        "factor=1.5",
        mk!(Saturation, factor = 1.5)
    );
    add!(
        "Saturation",
        "extreme",
        "factor=2.5",
        mk!(Saturation, factor = 2.5)
    );

    add!(
        "Vibrance",
        "subtle",
        "amount=0.2",
        mk!(Vibrance, amount = 0.2)
    );
    add!(
        "Vibrance",
        "moderate",
        "amount=0.5",
        mk!(Vibrance, amount = 0.5)
    );
    add!(
        "Vibrance",
        "extreme",
        "amount=1.0",
        mk!(Vibrance, amount = 1.0)
    );

    add!(
        "Temperature",
        "subtle",
        "shift=0.1",
        mk!(Temperature, shift = 0.1)
    );
    add!(
        "Temperature",
        "moderate",
        "shift=0.3",
        mk!(Temperature, shift = 0.3)
    );
    add!(
        "Temperature",
        "extreme",
        "shift=0.8",
        mk!(Temperature, shift = 0.8)
    );

    add!("Tint", "subtle", "shift=0.1", mk!(Tint, shift = 0.1));
    add!("Tint", "moderate", "shift=0.3", mk!(Tint, shift = 0.3));
    add!("Tint", "extreme", "shift=0.8", mk!(Tint, shift = 0.8));

    add!(
        "HueRotate",
        "subtle",
        "degrees=15",
        mk!(HueRotate, degrees = 15.0)
    );
    add!(
        "HueRotate",
        "moderate",
        "degrees=90",
        mk!(HueRotate, degrees = 90.0)
    );
    add!(
        "HueRotate",
        "extreme",
        "degrees=180",
        mk!(HueRotate, degrees = 180.0)
    );

    add!(
        "ColorGrading",
        "subtle",
        "shadow_a=0.02",
        mk!(ColorGrading, shadow_a = 0.02)
    );
    add!(
        "ColorGrading",
        "moderate",
        "shadow_a=0.05, highlight_b=-0.05",
        mk!(ColorGrading, shadow_a = 0.05, highlight_b = -0.05)
    );
    add!(
        "ColorGrading",
        "extreme",
        "all_offsets=0.1",
        mk!(
            ColorGrading,
            shadow_a = 0.1,
            shadow_b = 0.05,
            midtone_a = -0.05,
            midtone_b = 0.1,
            highlight_a = -0.1,
            highlight_b = -0.05
        )
    );

    {
        let mut h = HslAdjust::default();
        h.saturation[0] = 0.2;
        add!("HslAdjust", "subtle", "sat[0]=0.2", h);
    }
    {
        let mut h = HslAdjust::default();
        h.saturation[0] = 0.5;
        h.luminance[2] = -0.3;
        add!("HslAdjust", "moderate", "sat[0]=0.5, lum[2]=-0.3", h);
    }
    {
        let mut h = HslAdjust::default();
        h.hue[0] = 30.0;
        h.saturation = [0.8, 0.8, 0.8, 0.8, 0.0, 0.0, 0.0, 0.0];
        h.luminance = [0.0, 0.0, 0.0, 0.0, -0.5, -0.5, -0.5, -0.5];
        add!(
            "HslAdjust",
            "extreme",
            "hue[0]=30, sat spread, lum spread",
            h
        );
    }

    add!("BwMixer", "subtle", "default_weights", BwMixer::default());
    {
        let mut m = BwMixer::default();
        m.weights = [0.5, 0.2, 0.0, 0.0, 0.0, 0.0, 0.15, 0.15];
        add!("BwMixer", "moderate", "push_red", m);
    }
    {
        let mut m = BwMixer::default();
        m.weights = [1.0, 0.0, -0.5, -0.3, 0.0, 0.0, 0.0, 0.8];
        add!("BwMixer", "extreme", "extreme_red", m);
    }

    add!("Sepia", "moderate", "amount=1.0", mk!(Sepia, amount = 1.0));
    add!("Grayscale", "moderate", "default", Grayscale::default());

    {
        let mut f = AscCdl::default();
        f.slope = [1.1, 1.0, 1.0];
        add!("AscCdl", "subtle", "slope=[1.1,1.0,1.0]", f);
    }
    {
        let mut f = AscCdl::default();
        f.slope = [1.3, 1.0, 0.9];
        f.offset = [0.02, 0.0, 0.0];
        add!(
            "AscCdl",
            "moderate",
            "slope=[1.3,1.0,0.9], offset=[0.02,0,0]",
            f
        );
    }
    {
        let mut f = AscCdl::default();
        f.slope = [2.0, 0.8, 0.5];
        f.offset = [0.1, 0.0, 0.0];
        f.power = [1.5, 1.0, 1.2];
        add!(
            "AscCdl",
            "extreme",
            "slope=[2,0.8,0.5], offset=[0.1,0,0], power=[1.5,1,1.2]",
            f
        );
    }

    add!(
        "CameraCalibration",
        "subtle",
        "red_hue=5",
        mk!(CameraCalibration, red_hue = 5.0)
    );
    add!(
        "CameraCalibration",
        "moderate",
        "red_hue=15, blue_sat=-0.3",
        mk!(CameraCalibration, red_hue = 15.0, blue_saturation = -0.3)
    );
    add!(
        "CameraCalibration",
        "extreme",
        "all shifted",
        mk!(
            CameraCalibration,
            red_hue = 30.0,
            red_saturation = 0.5,
            green_hue = -20.0,
            green_saturation = 0.3,
            blue_hue = 10.0,
            blue_saturation = -0.5,
            shadow_tint = 0.3
        )
    );

    add!(
        "GamutExpand",
        "subtle",
        "strength=0.3",
        mk!(GamutExpand, strength = 0.3)
    );
    add!(
        "GamutExpand",
        "moderate",
        "strength=0.6",
        mk!(GamutExpand, strength = 0.6)
    );
    add!(
        "GamutExpand",
        "extreme",
        "strength=1.0",
        mk!(GamutExpand, strength = 1.0)
    );

    add!("Invert", "moderate", "default", Invert::default());

    // ── Curves ────────────────────────────────────────────────────────
    add!(
        "ToneCurve",
        "subtle",
        "gentle S",
        ToneCurve::from_points(&[
            (0.0, 0.0),
            (0.25, 0.20),
            (0.5, 0.5),
            (0.75, 0.80),
            (1.0, 1.0)
        ])
    );
    add!(
        "ToneCurve",
        "moderate",
        "strong S",
        ToneCurve::from_points(&[
            (0.0, 0.0),
            (0.25, 0.15),
            (0.5, 0.5),
            (0.75, 0.85),
            (1.0, 1.0)
        ])
    );
    add!(
        "ToneCurve",
        "extreme",
        "extreme S",
        ToneCurve::from_points(&[
            (0.0, 0.0),
            (0.25, 0.05),
            (0.5, 0.5),
            (0.75, 0.95),
            (1.0, 1.0)
        ])
    );

    add!(
        "ParametricCurve",
        "subtle",
        "shadows=0.2",
        ParametricCurve::new(0.2, 0.0, 0.0, 0.0, 0.25, 0.5, 0.75)
    );
    add!(
        "ParametricCurve",
        "moderate",
        "shadows=0.5, highlights=-0.3",
        ParametricCurve::new(0.5, 0.0, 0.0, -0.3, 0.25, 0.5, 0.75)
    );
    add!(
        "ParametricCurve",
        "extreme",
        "all zones extreme",
        ParametricCurve::new(0.8, 0.5, -0.5, -0.8, 0.25, 0.5, 0.75)
    );

    add!(
        "ChannelCurves",
        "subtle",
        "R gentle boost",
        ChannelCurves::from_points_uniform(&[
            (0.0, 0.0),
            (0.25, 0.28),
            (0.5, 0.5),
            (0.75, 0.75),
            (1.0, 1.0)
        ])
    );
    add!(
        "ChannelCurves",
        "moderate",
        "per-channel S",
        ChannelCurves::from_points(
            &[(0.0, 0.0), (0.25, 0.15), (0.75, 0.85), (1.0, 1.0)],
            &[(0.0, 0.0), (0.25, 0.20), (0.75, 0.80), (1.0, 1.0)],
            &[(0.0, 0.0), (0.25, 0.30), (0.75, 0.70), (1.0, 1.0)],
        )
    );
    add!(
        "ChannelCurves",
        "extreme",
        "heavy cross-push",
        ChannelCurves::from_points(
            &[(0.0, 0.0), (0.25, 0.05), (0.75, 0.95), (1.0, 1.0)],
            &[(0.0, 0.1), (0.5, 0.5), (1.0, 0.9)],
            &[(0.0, 0.0), (0.25, 0.35), (0.75, 0.65), (1.0, 1.0)],
        )
    );

    {
        let mut hc = HueCurves::default();
        hc.set_hue_sat(&[(0.0, 1.0), (60.0, 1.3), (120.0, 1.0), (360.0, 1.0)]);
        add!("HueCurves", "subtle", "sat boost at 60deg", hc);
    }
    {
        let mut hc = HueCurves::default();
        hc.set_hue_sat(&[
            (0.0, 1.0),
            (30.0, 1.5),
            (120.0, 0.7),
            (240.0, 1.3),
            (360.0, 1.0),
        ]);
        hc.set_hue_lum(&[(0.0, 0.0), (60.0, 0.1), (180.0, -0.1), (360.0, 0.0)]);
        add!("HueCurves", "moderate", "multi-hue sat+lum", hc);
    }
    {
        let mut hc = HueCurves::default();
        hc.set_hue_sat(&[
            (0.0, 2.0),
            (90.0, 0.3),
            (180.0, 2.0),
            (270.0, 0.3),
            (360.0, 2.0),
        ]);
        hc.set_hue_hue(&[(0.0, 0.0), (60.0, 30.0), (180.0, -20.0), (360.0, 0.0)]);
        add!("HueCurves", "extreme", "extreme all-hue", hc);
    }

    // ── Detail / Neighborhood ─────────────────────────────────────────
    add!(
        "Clarity",
        "subtle",
        "sigma=3, amount=0.15",
        mk!(Clarity, sigma = 3.0, amount = 0.15)
    );
    add!(
        "Clarity",
        "moderate",
        "sigma=3, amount=0.5",
        mk!(Clarity, sigma = 3.0, amount = 0.5)
    );
    add!(
        "Clarity",
        "extreme",
        "sigma=3, amount=1.0",
        mk!(Clarity, sigma = 3.0, amount = 1.0)
    );

    add!(
        "Sharpen",
        "subtle",
        "sigma=1, amount=0.3",
        mk!(Sharpen, sigma = 1.0, amount = 0.3)
    );
    add!(
        "Sharpen",
        "moderate",
        "sigma=1, amount=1.0",
        mk!(Sharpen, sigma = 1.0, amount = 1.0)
    );
    add!(
        "Sharpen",
        "extreme",
        "sigma=1, amount=2.0",
        mk!(Sharpen, sigma = 1.0, amount = 2.0)
    );

    add!(
        "AdaptiveSharpen",
        "subtle",
        "amount=0.3",
        mk!(AdaptiveSharpen, amount = 0.3, sigma = 1.0)
    );
    add!(
        "AdaptiveSharpen",
        "moderate",
        "amount=1.0",
        mk!(AdaptiveSharpen, amount = 1.0, sigma = 1.0)
    );
    add!(
        "AdaptiveSharpen",
        "extreme",
        "amount=2.0",
        mk!(AdaptiveSharpen, amount = 2.0, sigma = 1.0)
    );

    add!(
        "Texture",
        "subtle",
        "sigma=1, amount=0.3",
        mk!(Texture, sigma = 1.0, amount = 0.3)
    );
    add!(
        "Texture",
        "moderate",
        "sigma=1, amount=0.7",
        mk!(Texture, sigma = 1.0, amount = 0.7)
    );
    add!(
        "Texture",
        "extreme",
        "sigma=1, amount=1.0",
        mk!(Texture, sigma = 1.0, amount = 1.0)
    );

    add!(
        "Brilliance",
        "subtle",
        "sigma=10, amount=0.2",
        mk!(Brilliance, sigma = 10.0, amount = 0.2)
    );
    add!(
        "Brilliance",
        "moderate",
        "sigma=10, amount=0.5",
        mk!(Brilliance, sigma = 10.0, amount = 0.5)
    );
    add!(
        "Brilliance",
        "extreme",
        "sigma=10, amount=1.0",
        mk!(Brilliance, sigma = 10.0, amount = 1.0)
    );

    add!(
        "NoiseReduction",
        "subtle",
        "lum=0.3, chroma=0.3",
        mk!(NoiseReduction, luminance = 0.3, chroma = 0.3)
    );
    add!(
        "NoiseReduction",
        "moderate",
        "lum=0.6, chroma=0.6",
        mk!(NoiseReduction, luminance = 0.6, chroma = 0.6)
    );
    add!(
        "NoiseReduction",
        "extreme",
        "lum=1.0, chroma=1.0",
        mk!(NoiseReduction, luminance = 1.0, chroma = 1.0)
    );

    add!(
        "Bilateral",
        "subtle",
        "strength=0.3",
        mk!(Bilateral, strength = 0.3, spatial_sigma = 5.0)
    );
    add!(
        "Bilateral",
        "moderate",
        "strength=0.6",
        mk!(Bilateral, strength = 0.6, spatial_sigma = 5.0)
    );
    add!(
        "Bilateral",
        "extreme",
        "strength=1.0",
        mk!(Bilateral, strength = 1.0, spatial_sigma = 5.0)
    );

    add!("Blur", "subtle", "sigma=1", mk!(Blur, sigma = 1.0));
    add!("Blur", "moderate", "sigma=5", mk!(Blur, sigma = 5.0));
    add!("Blur", "extreme", "sigma=20", mk!(Blur, sigma = 20.0));

    add!(
        "MedianBlur",
        "subtle",
        "radius=1",
        mk!(MedianBlur, radius = 1)
    );
    add!(
        "MedianBlur",
        "moderate",
        "radius=3",
        mk!(MedianBlur, radius = 3)
    );
    add!(
        "MedianBlur",
        "extreme",
        "radius=5",
        mk!(MedianBlur, radius = 5)
    );

    add!(
        "EdgeDetect",
        "subtle",
        "strength=0.3",
        mk!(EdgeDetect, strength = 0.3)
    );
    add!(
        "EdgeDetect",
        "moderate",
        "strength=0.7",
        mk!(EdgeDetect, strength = 0.7)
    );
    add!(
        "EdgeDetect",
        "extreme",
        "strength=1.0",
        mk!(EdgeDetect, strength = 1.0)
    );

    // ── Effects ───────────────────────────────────────────────────────
    add!(
        "Bloom",
        "subtle",
        "amount=0.1, threshold=0.7",
        mk!(Bloom, amount = 0.1, threshold = 0.7)
    );
    add!(
        "Bloom",
        "moderate",
        "amount=0.3, threshold=0.5",
        mk!(Bloom, amount = 0.3, threshold = 0.5)
    );
    add!(
        "Bloom",
        "extreme",
        "amount=0.8, threshold=0.3",
        mk!(Bloom, amount = 0.8, threshold = 0.3)
    );

    add!(
        "Grain",
        "subtle",
        "amount=0.1, size=1",
        mk!(Grain, amount = 0.1, size = 1.0, seed = 42)
    );
    add!(
        "Grain",
        "moderate",
        "amount=0.3, size=2",
        mk!(Grain, amount = 0.3, size = 2.0, seed = 42)
    );
    add!(
        "Grain",
        "extreme",
        "amount=0.8, size=3",
        mk!(Grain, amount = 0.8, size = 3.0, seed = 42)
    );

    add!(
        "Vignette",
        "subtle",
        "strength=0.15",
        mk!(Vignette, strength = 0.15)
    );
    add!(
        "Vignette",
        "moderate",
        "strength=0.4",
        mk!(Vignette, strength = 0.4)
    );
    add!(
        "Vignette",
        "extreme",
        "strength=0.8",
        mk!(Vignette, strength = 0.8)
    );

    add!(
        "Devignette",
        "subtle",
        "strength=0.2",
        mk!(Devignette, strength = 0.2)
    );
    add!(
        "Devignette",
        "moderate",
        "strength=0.5",
        mk!(Devignette, strength = 0.5)
    );
    add!(
        "Devignette",
        "extreme",
        "strength=1.0",
        mk!(Devignette, strength = 1.0)
    );

    add!(
        "Dehaze",
        "subtle",
        "strength=0.2",
        mk!(Dehaze, strength = 0.2)
    );
    add!(
        "Dehaze",
        "moderate",
        "strength=0.5",
        mk!(Dehaze, strength = 0.5)
    );
    add!(
        "Dehaze",
        "extreme",
        "strength=1.0",
        mk!(Dehaze, strength = 1.0)
    );

    add!(
        "ChromaticAberration",
        "subtle",
        "shift_a=0.5",
        mk!(ChromaticAberration, shift_a = 0.5, shift_b = -0.5)
    );
    add!(
        "ChromaticAberration",
        "moderate",
        "shift_a=2.0",
        mk!(ChromaticAberration, shift_a = 2.0, shift_b = -2.0)
    );
    add!(
        "ChromaticAberration",
        "extreme",
        "shift_a=5.0",
        mk!(ChromaticAberration, shift_a = 5.0, shift_b = -5.0)
    );

    // ── Spatial ───────────────────────────────────────────────────────
    add!(
        "LocalToneMap",
        "subtle",
        "compression=0.2",
        mk!(LocalToneMap, compression = 0.2)
    );
    add!(
        "LocalToneMap",
        "moderate",
        "compression=0.5",
        mk!(LocalToneMap, compression = 0.5)
    );
    add!(
        "LocalToneMap",
        "extreme",
        "compression=1.0",
        mk!(LocalToneMap, compression = 1.0)
    );

    {
        let mut t = ToneEqualizer::default();
        t.zones[4] = 0.5;
        add!("ToneEqualizer", "subtle", "zones[4]=0.5", t);
    }
    {
        let mut t = ToneEqualizer::default();
        t.zones = [-1.0, -0.5, -0.2, 0.0, 0.0, 0.0, 0.2, 0.5, 1.0];
        add!("ToneEqualizer", "moderate", "spread ±1EV", t);
    }
    {
        let mut t = ToneEqualizer::default();
        t.zones = [-3.0, -2.0, -1.0, -0.5, 0.0, 0.5, 1.0, 2.0, 3.0];
        add!("ToneEqualizer", "extreme", "spread ±3EV", t);
    }

    // ── Auto ──────────────────────────────────────────────────────────
    add!(
        "AutoExposure",
        "subtle",
        "strength=0.3",
        mk!(AutoExposure, strength = 0.3)
    );
    add!(
        "AutoExposure",
        "moderate",
        "strength=0.7",
        mk!(AutoExposure, strength = 0.7)
    );
    add!(
        "AutoExposure",
        "extreme",
        "strength=1.0",
        mk!(AutoExposure, strength = 1.0)
    );

    add!(
        "AutoLevels",
        "subtle",
        "strength=0.3",
        mk!(AutoLevels, strength = 0.3)
    );
    add!(
        "AutoLevels",
        "moderate",
        "strength=0.7",
        mk!(AutoLevels, strength = 0.7)
    );
    add!(
        "AutoLevels",
        "extreme",
        "strength=1.0",
        mk!(AutoLevels, strength = 1.0)
    );

    // ── New Auto Filters ─────────────────────────────────────────────
    add!(
        "AutoTone",
        "subtle",
        "strength=0.25, pi=0.3",
        mk!(AutoTone, strength = 0.25, preserve_intent = 0.3)
    );
    add!(
        "AutoTone",
        "moderate",
        "strength=0.5, pi=0.3",
        mk!(AutoTone, strength = 0.5, preserve_intent = 0.3)
    );
    add!(
        "AutoTone",
        "extreme",
        "strength=1.0, pi=0.0",
        mk!(AutoTone, strength = 1.0, preserve_intent = 0.0)
    );

    add!(
        "AutoWhiteBalance",
        "subtle",
        "strength=0.3",
        mk!(AutoWhiteBalance, strength = 0.3)
    );
    add!(
        "AutoWhiteBalance",
        "moderate",
        "strength=0.7",
        mk!(AutoWhiteBalance, strength = 0.7)
    );
    add!(
        "AutoWhiteBalance",
        "extreme",
        "strength=1.0",
        mk!(AutoWhiteBalance, strength = 1.0)
    );

    add!(
        "AutoContrast",
        "subtle",
        "strength=0.3",
        mk!(AutoContrast, strength = 0.3)
    );
    add!(
        "AutoContrast",
        "moderate",
        "strength=0.7",
        mk!(AutoContrast, strength = 0.7)
    );
    add!(
        "AutoContrast",
        "extreme",
        "strength=1.0",
        mk!(AutoContrast, strength = 1.0)
    );

    add!(
        "AutoVibrance",
        "subtle",
        "strength=0.3",
        mk!(AutoVibrance, strength = 0.3)
    );
    add!(
        "AutoVibrance",
        "moderate",
        "strength=0.7",
        mk!(AutoVibrance, strength = 0.7)
    );
    add!(
        "AutoVibrance",
        "extreme",
        "strength=1.0",
        mk!(AutoVibrance, strength = 1.0)
    );

    c
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let output_dir = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/v/output/zenfilters/audit"));
    let dataset = args
        .get(2)
        .map(|s| s.as_str())
        .unwrap_or("clic2025/final-test");
    let skip_count: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);

    // Get test images via codec-corpus
    let corpus = codec_corpus::Corpus::new().expect("failed to init codec-corpus");
    let input_dir = corpus.get(dataset).unwrap_or_else(|e| {
        eprintln!("Failed to get dataset '{dataset}': {e}");
        std::process::exit(1);
    });

    let mut images: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&input_dir).expect("cannot read input dir") {
        let entry = entry.unwrap();
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_lowercase().as_str() {
                "jpg" | "jpeg" | "png" => images.push(path),
                _ => {}
            }
        }
    }
    images.sort();
    // Skip known non-photographic images
    let skip_prefixes = ["2a760bf1", "86127fbd"];
    images.retain(|p| {
        let stem = p.file_stem().unwrap().to_str().unwrap_or("");
        !skip_prefixes.iter().any(|pfx| stem.starts_with(pfx))
    });
    // Skip first N images (for testing on a different subset)
    if skip_count > 0 && skip_count < images.len() {
        images = images.split_off(skip_count);
    }
    images.truncate(MAX_IMAGES);

    if images.is_empty() {
        eprintln!("No images found in {input_dir:?}");
        std::process::exit(1);
    }
    eprintln!("Found {} test images", images.len());

    let configs = audit_configs();
    eprintln!("{} filter configs to test", configs.len());

    let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
    let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();
    let mut ctx = FilterContext::new();
    let mut manifest: Vec<String> = Vec::new();

    for (img_idx, img_path) in images.iter().enumerate() {
        let stem = img_path.file_stem().unwrap().to_str().unwrap();
        eprintln!("[{}/{}] Loading {stem}...", img_idx + 1, images.len());

        let img = match ImageReader::open(img_path)
            .and_then(|r| r.with_guessed_format())
            .map_err(|e| e.to_string())
            .and_then(|r| r.decode().map_err(|e| e.to_string()))
        {
            Ok(i) => i,
            Err(e) => {
                eprintln!("  Skipping: {e}");
                continue;
            }
        };

        let img = if img.width() > MAX_DIM || img.height() > MAX_DIM {
            img.resize(MAX_DIM, MAX_DIM, FilterType::Lanczos3)
        } else {
            img
        };

        let rgb = img.to_rgb8();
        let (rw, rh) = (rgb.width(), rgb.height());
        let srgb_u8 = rgb.as_raw();

        // Scatter to Oklab once per image
        let mut base_planes = OklabPlanes::new(rw, rh);
        scatter_srgb_u8_to_oklab(srgb_u8, &mut base_planes, 3, &m1);

        for config in &configs {
            let dir = output_dir.join(config.filter_name);
            fs::create_dir_all(&dir).unwrap();

            let filename = format!("{}_{}.jpg", config.level, stem);
            let out_path = dir.join(&filename);

            // Apply filter to a fresh copy
            let mut planes = base_planes.clone();
            config.filter.apply(&mut planes, &mut ctx);

            // Gather filtered result to sRGB u8
            let mut filtered_u8 = vec![0u8; (rw as usize) * (rh as usize) * 3];
            gather_oklab_to_srgb_u8(&planes, &mut filtered_u8, 3, &m1_inv);
            let filtered_img = RgbImage::from_raw(rw, rh, filtered_u8).unwrap();

            // Create side-by-side montage
            let montage = create_montage(&rgb, &filtered_img);
            save_jpeg(&out_path, &montage);

            let rel_path = format!("{}/{}", config.filter_name, filename);
            manifest.push(format!(
                "  {{\"filter\": \"{}\", \"level\": \"{}\", \"params\": \"{}\", \"image\": \"{}\", \"source\": \"{}\"}}",
                config.filter_name, config.level, config.params, rel_path, stem
            ));
        }
    }

    // Write manifest
    let manifest_json = format!("[\n{}\n]\n", manifest.join(",\n"));
    fs::write(output_dir.join("manifest.json"), &manifest_json).unwrap();

    eprintln!(
        "Done. {} montages written to {:?}",
        manifest.len(),
        output_dir
    );
}

fn create_montage(original: &RgbImage, filtered: &RgbImage) -> RgbImage {
    let w = original.width();
    let h = original.height();
    let mut montage = RgbImage::new(w * 2, h);

    // Left: original
    for y in 0..h {
        for x in 0..w {
            montage.put_pixel(x, y, *original.get_pixel(x, y));
        }
    }
    // Right: filtered
    for y in 0..h {
        for x in 0..w {
            montage.put_pixel(w + x, y, *filtered.get_pixel(x, y));
        }
    }

    montage
}

fn save_jpeg(path: &Path, img: &RgbImage) {
    let mut buf = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    img.write_with_encoder(encoder).expect("JPEG encode failed");
    fs::write(path, &buf).unwrap();
}
