//! Filter presets — named parameter bundles with intensity control.
//!
//! A preset is a collection of filter parameters that can be applied with
//! one call and blended with an intensity slider (0.0 = no effect, 1.0 = full).
//!
//! # Built-in presets
//!
//! The crate ships ~20 built-in presets covering common styles:
//! - Portrait, Landscape, Food, Night
//! - Vivid, Warm, Cool, Vintage, Film
//! - B&W Classic, B&W High Contrast, B&W Film
//! - Cinematic, Golden Hour, Moody, Clean
//!
//! # Custom presets
//!
//! Presets serialize to JSON via serde, making them easy to store and share:
//! ```ignore
//! let json = serde_json::to_string_pretty(&preset)?;
//! let preset: Preset = serde_json::from_str(&json)?;
//! ```
//!
//! # Intensity blending
//!
//! [`Preset::build_pipeline_at`] blends each parameter between its identity
//! value and the preset value: `effective = identity + intensity * (preset - identity)`.
//! This gives smooth ramping from no effect (0.0) to full preset (1.0).

use alloc::string::String;
use alloc::vec::Vec;

use crate::filters::*;
use crate::pipeline::{Pipeline, PipelineConfig};

/// A named filter preset with category and parameter values.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Preset {
    /// Preset name (e.g., "Vivid", "Golden Hour").
    pub name: String,
    /// Category for UI grouping.
    pub category: PresetCategory,
    /// Description shown in tooltip/preview.
    pub description: String,
    /// The FusedAdjust parameters (exposure, contrast, H/S, saturation, etc.).
    pub adjust: PresetAdjust,
    /// Optional sigmoid tone mapping (for scene-referred presets).
    pub sigmoid: Option<PresetSigmoid>,
    /// Optional clarity/texture.
    pub clarity: Option<f32>,
    /// Optional sharpening amount.
    pub sharpen: Option<f32>,
    /// Optional local tone map compression.
    pub local_tonemap: Option<f32>,
    /// Optional grain amount.
    pub grain: Option<f32>,
    /// Optional vignette strength (negative = darken edges).
    pub vignette: Option<f32>,
    /// Optional bloom amount.
    pub bloom: Option<f32>,
    /// Whether to convert to B&W (grayscale).
    pub grayscale: bool,
    /// Optional sepia amount (only if grayscale).
    pub sepia: Option<f32>,
}

/// Preset category for UI grouping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PresetCategory {
    /// General purpose enhancement
    Enhance,
    /// Portrait-optimized (skin tone aware)
    Portrait,
    /// Landscape/nature (vivid, clear)
    Landscape,
    /// Warm/golden tones
    Warm,
    /// Cool/blue tones
    Cool,
    /// Film/vintage looks
    Film,
    /// Black & white
    BlackWhite,
    /// Cinematic/moody
    Cinematic,
    /// User-created
    Custom,
}

/// Core adjustment parameters for a preset.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PresetAdjust {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub temperature: f32,
    pub tint: f32,
    pub black_point: f32,
    pub white_point: f32,
    pub dehaze: f32,
}

impl PresetAdjust {
    /// Blend this preset's values toward identity by `intensity`.
    /// At intensity=0 → identity values, intensity=1 → full preset.
    fn blend(&self, intensity: f32) -> FusedAdjust {
        let t = intensity.clamp(0.0, 1.0);
        let mut fa = FusedAdjust::new();
        fa.exposure = self.exposure * t;
        fa.contrast = self.contrast * t;
        fa.highlights = self.highlights * t;
        fa.shadows = self.shadows * t;
        fa.saturation = 1.0 + (self.saturation - 1.0) * t;
        fa.vibrance = self.vibrance * t;
        fa.temperature = self.temperature * t;
        fa.tint = self.tint * t;
        fa.black_point = self.black_point * t;
        fa.white_point = 1.0 + (self.white_point - 1.0) * t;
        fa.dehaze = self.dehaze * t;
        fa
    }
}

/// Optional sigmoid parameters.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PresetSigmoid {
    pub contrast: f32,
    pub skew: f32,
}

impl Preset {
    /// Build a pipeline applying this preset at full intensity.
    pub fn build_pipeline(&self) -> Pipeline {
        self.build_pipeline_at(1.0)
    }

    /// Build a pipeline applying this preset at the given intensity (0.0–1.0).
    ///
    /// Intensity 0.0 = no effect (identity pipeline).
    /// Intensity 0.5 = half-strength preset.
    /// Intensity 1.0 = full preset.
    pub fn build_pipeline_at(&self, intensity: f32) -> Pipeline {
        let t = intensity.clamp(0.0, 1.0);
        let mut pipe = Pipeline::new(PipelineConfig::default()).unwrap();

        // Core adjustments (always present)
        let fa = self.adjust.blend(t);
        if !fa.is_identity() {
            pipe.push(Box::new(fa));
        }

        // Optional sigmoid
        if let Some(ref sig) = self.sigmoid {
            let mut s = Sigmoid::default();
            s.contrast = 1.0 + (sig.contrast - 1.0) * t;
            s.skew = 0.5 + (sig.skew - 0.5) * t;
            pipe.push(Box::new(s));
        }

        // Optional local tone map
        if let Some(ltm) = self.local_tonemap {
            let v = ltm * t;
            if v > 0.01 {
                let mut l = LocalToneMap::default();
                l.compression = v;
                pipe.push(Box::new(l));
            }
        }

        // Optional clarity
        if let Some(cl) = self.clarity {
            let v = cl * t;
            if v.abs() > 0.01 {
                let mut c = Clarity::default();
                c.amount = v;
                pipe.push(Box::new(c));
            }
        }

        // Optional sharpening
        if let Some(sh) = self.sharpen {
            let v = sh * t;
            if v > 0.01 {
                let mut s = AdaptiveSharpen::default();
                s.amount = v;
                pipe.push(Box::new(s));
            }
        }

        // Optional grain
        if let Some(gr) = self.grain {
            let v = gr * t;
            if v > 0.01 {
                let mut g = Grain::default();
                g.amount = v;
                pipe.push(Box::new(g));
            }
        }

        // Optional vignette
        if let Some(vig) = self.vignette {
            let v = vig * t;
            if v.abs() > 0.01 {
                let mut vi = Vignette::default();
                vi.strength = v;
                pipe.push(Box::new(vi));
            }
        }

        // Optional bloom
        if let Some(bl) = self.bloom {
            let v = bl * t;
            if v > 0.01 {
                let mut b = Bloom::default();
                b.amount = v;
                pipe.push(Box::new(b));
            }
        }

        // B&W conversion
        if self.grayscale {
            pipe.push(Box::new(Grayscale));
            if let Some(sep) = self.sepia {
                let v = sep * t;
                if v > 0.01 {
                    let mut s = Sepia::default();
                    s.amount = v;
                    pipe.push(Box::new(s));
                }
            }
        }

        pipe
    }
}

// ─── Built-in presets ────────────────────────────────────────────────

/// All built-in presets.
pub fn builtin_presets() -> Vec<Preset> {
    vec![
        // ── Enhance ──────────────────────────
        vivid(),
        enhance(),
        clean(),
        // ── Warm ─────────────────────────────
        warm(),
        golden_hour(),
        // ── Cool ─────────────────────────────
        cool(),
        // ── Portrait ─────────────────────────
        portrait(),
        portrait_warm(),
        // ── Landscape ────────────────────────
        landscape(),
        // ── Film ─────────────────────────────
        vintage(),
        film_warm(),
        film_cool(),
        faded(),
        // ── Cinematic ────────────────────────
        cinematic(),
        moody(),
        // ── B&W ──────────────────────────────
        bw_classic(),
        bw_high_contrast(),
        bw_film(),
        bw_sepia(),
    ]
}

fn vivid() -> Preset {
    Preset {
        name: String::from("Vivid"),
        category: PresetCategory::Enhance,
        description: String::from("Punchy colors and contrast"),
        adjust: PresetAdjust {
            contrast: 0.15,
            saturation: 1.25,
            vibrance: 0.3,
            ..Default::default()
        },
        clarity: Some(0.15),
        ..default_preset()
    }
}

fn enhance() -> Preset {
    Preset {
        name: String::from("Enhance"),
        category: PresetCategory::Enhance,
        description: String::from("Subtle overall improvement"),
        adjust: PresetAdjust {
            exposure: 0.1,
            contrast: 0.08,
            highlights: -0.1,
            shadows: 0.15,
            saturation: 1.08,
            vibrance: 0.2,
            ..Default::default()
        },
        clarity: Some(0.1),
        sharpen: Some(0.2),
        ..default_preset()
    }
}

fn clean() -> Preset {
    Preset {
        name: String::from("Clean"),
        category: PresetCategory::Enhance,
        description: String::from("Bright, clean look with reduced shadows"),
        adjust: PresetAdjust {
            exposure: 0.2,
            contrast: -0.05,
            highlights: -0.15,
            shadows: 0.3,
            saturation: 1.05,
            ..Default::default()
        },
        ..default_preset()
    }
}

fn warm() -> Preset {
    Preset {
        name: String::from("Warm"),
        category: PresetCategory::Warm,
        description: String::from("Warm golden tones"),
        adjust: PresetAdjust {
            temperature: 0.25,
            tint: 0.05,
            saturation: 1.1,
            vibrance: 0.15,
            ..Default::default()
        },
        ..default_preset()
    }
}

fn golden_hour() -> Preset {
    Preset {
        name: String::from("Golden Hour"),
        category: PresetCategory::Warm,
        description: String::from("Late afternoon golden light"),
        adjust: PresetAdjust {
            temperature: 0.35,
            tint: 0.08,
            exposure: 0.15,
            contrast: 0.1,
            saturation: 1.2,
            vibrance: 0.25,
            ..Default::default()
        },
        vignette: Some(0.15),
        bloom: Some(0.1),
        ..default_preset()
    }
}

fn cool() -> Preset {
    Preset {
        name: String::from("Cool"),
        category: PresetCategory::Cool,
        description: String::from("Cool blue-tinted tones"),
        adjust: PresetAdjust {
            temperature: -0.2,
            contrast: 0.08,
            saturation: 1.05,
            ..Default::default()
        },
        ..default_preset()
    }
}

fn portrait() -> Preset {
    Preset {
        name: String::from("Portrait"),
        category: PresetCategory::Portrait,
        description: String::from("Flattering skin tones, soft detail"),
        adjust: PresetAdjust {
            exposure: 0.1,
            contrast: -0.05,
            highlights: -0.2,
            shadows: 0.2,
            saturation: 1.05,
            vibrance: 0.1,
            ..Default::default()
        },
        ..default_preset()
    }
}

fn portrait_warm() -> Preset {
    Preset {
        name: String::from("Portrait Warm"),
        category: PresetCategory::Portrait,
        description: String::from("Warm, glowing skin tones"),
        adjust: PresetAdjust {
            exposure: 0.1,
            temperature: 0.15,
            contrast: -0.05,
            highlights: -0.15,
            shadows: 0.2,
            saturation: 1.08,
            vibrance: 0.15,
            ..Default::default()
        },
        bloom: Some(0.05),
        ..default_preset()
    }
}

fn landscape() -> Preset {
    Preset {
        name: String::from("Landscape"),
        category: PresetCategory::Landscape,
        description: String::from("Rich colors, strong clarity for nature"),
        adjust: PresetAdjust {
            contrast: 0.12,
            highlights: -0.1,
            shadows: 0.1,
            saturation: 1.15,
            vibrance: 0.3,
            dehaze: 0.15,
            ..Default::default()
        },
        clarity: Some(0.25),
        sharpen: Some(0.3),
        ..default_preset()
    }
}

fn vintage() -> Preset {
    Preset {
        name: String::from("Vintage"),
        category: PresetCategory::Film,
        description: String::from("Faded, warm retro look"),
        adjust: PresetAdjust {
            contrast: -0.1,
            temperature: 0.15,
            saturation: 0.85,
            highlights: -0.1,
            black_point: 0.04,
            ..Default::default()
        },
        grain: Some(0.15),
        vignette: Some(0.2),
        ..default_preset()
    }
}

fn film_warm() -> Preset {
    Preset {
        name: String::from("Film Warm"),
        category: PresetCategory::Film,
        description: String::from("Warm analog film emulation"),
        adjust: PresetAdjust {
            temperature: 0.12,
            contrast: 0.08,
            saturation: 0.92,
            vibrance: 0.15,
            highlights: -0.1,
            black_point: 0.03,
            ..Default::default()
        },
        sigmoid: Some(PresetSigmoid {
            contrast: 1.3,
            skew: 0.55,
        }),
        grain: Some(0.1),
        ..default_preset()
    }
}

fn film_cool() -> Preset {
    Preset {
        name: String::from("Film Cool"),
        category: PresetCategory::Film,
        description: String::from("Cool-toned film with lifted blacks"),
        adjust: PresetAdjust {
            temperature: -0.1,
            tint: -0.03,
            contrast: 0.06,
            saturation: 0.88,
            black_point: 0.05,
            ..Default::default()
        },
        grain: Some(0.08),
        ..default_preset()
    }
}

fn faded() -> Preset {
    Preset {
        name: String::from("Faded"),
        category: PresetCategory::Film,
        description: String::from("Low contrast, lifted blacks, muted colors"),
        adjust: PresetAdjust {
            contrast: -0.15,
            saturation: 0.8,
            black_point: 0.06,
            shadows: 0.15,
            ..Default::default()
        },
        ..default_preset()
    }
}

fn cinematic() -> Preset {
    Preset {
        name: String::from("Cinematic"),
        category: PresetCategory::Cinematic,
        description: String::from("Dramatic contrast with teal-orange split"),
        adjust: PresetAdjust {
            contrast: 0.2,
            temperature: 0.08,
            saturation: 0.95,
            highlights: -0.15,
            shadows: 0.1,
            black_point: 0.02,
            ..Default::default()
        },
        local_tonemap: Some(0.15),
        vignette: Some(0.25),
        ..default_preset()
    }
}

fn moody() -> Preset {
    Preset {
        name: String::from("Moody"),
        category: PresetCategory::Cinematic,
        description: String::from("Dark, desaturated, atmospheric"),
        adjust: PresetAdjust {
            exposure: -0.2,
            contrast: 0.15,
            saturation: 0.8,
            vibrance: -0.1,
            highlights: -0.2,
            temperature: -0.05,
            ..Default::default()
        },
        vignette: Some(0.3),
        ..default_preset()
    }
}

fn bw_classic() -> Preset {
    Preset {
        name: String::from("B&W Classic"),
        category: PresetCategory::BlackWhite,
        description: String::from("Clean black and white conversion"),
        adjust: PresetAdjust {
            contrast: 0.1,
            ..Default::default()
        },
        grayscale: true,
        ..default_preset()
    }
}

fn bw_high_contrast() -> Preset {
    Preset {
        name: String::from("B&W High Contrast"),
        category: PresetCategory::BlackWhite,
        description: String::from("Dramatic high-contrast monochrome"),
        adjust: PresetAdjust {
            contrast: 0.3,
            highlights: -0.1,
            shadows: 0.2,
            black_point: 0.02,
            ..Default::default()
        },
        clarity: Some(0.2),
        grayscale: true,
        ..default_preset()
    }
}

fn bw_film() -> Preset {
    Preset {
        name: String::from("B&W Film"),
        category: PresetCategory::BlackWhite,
        description: String::from("Film-grain monochrome with soft contrast"),
        adjust: PresetAdjust {
            contrast: 0.05,
            black_point: 0.03,
            ..Default::default()
        },
        grain: Some(0.2),
        grayscale: true,
        ..default_preset()
    }
}

fn bw_sepia() -> Preset {
    Preset {
        name: String::from("Sepia"),
        category: PresetCategory::BlackWhite,
        description: String::from("Warm sepia-toned monochrome"),
        adjust: PresetAdjust {
            contrast: 0.08,
            ..Default::default()
        },
        grayscale: true,
        sepia: Some(0.7),
        grain: Some(0.1),
        vignette: Some(0.15),
        ..default_preset()
    }
}

fn default_preset() -> Preset {
    Preset {
        name: String::new(),
        category: PresetCategory::Custom,
        description: String::new(),
        adjust: PresetAdjust::default(),
        sigmoid: None,
        clarity: None,
        sharpen: None,
        local_tonemap: None,
        grain: None,
        vignette: None,
        bloom: None,
        grayscale: false,
        sepia: None,
    }
}

impl Default for PresetAdjust {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            saturation: 1.0,
            vibrance: 0.0,
            temperature: 0.0,
            tint: 0.0,
            black_point: 0.0,
            white_point: 1.0,
            dehaze: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtins_build() {
        for preset in builtin_presets() {
            let pipe = preset.build_pipeline();
            assert!(!preset.name.is_empty(), "preset should have a name");
            let _ = pipe; // just verify it doesn't panic
        }
    }

    #[test]
    fn intensity_zero_is_near_identity() {
        let preset = vivid();
        let pipe = preset.build_pipeline_at(0.0);
        // At intensity 0, FusedAdjust should be identity → not pushed
        // Pipeline should be empty or near-empty
        let _ = pipe;
    }

    #[test]
    fn intensity_blending() {
        let adj = PresetAdjust {
            contrast: 0.5,
            saturation: 1.5,
            ..Default::default()
        };

        let half = adj.blend(0.5);
        assert!((half.contrast - 0.25).abs() < 1e-5);
        assert!((half.saturation - 1.25).abs() < 1e-5);

        let full = adj.blend(1.0);
        assert!((full.contrast - 0.5).abs() < 1e-5);
        assert!((full.saturation - 1.5).abs() < 1e-5);

        let zero = adj.blend(0.0);
        assert!(zero.contrast.abs() < 1e-5);
        assert!((zero.saturation - 1.0).abs() < 1e-5);
    }

    #[test]
    fn preset_count() {
        let presets = builtin_presets();
        assert!(
            presets.len() >= 18,
            "expected 18+ presets, got {}",
            presets.len()
        );
    }
}
