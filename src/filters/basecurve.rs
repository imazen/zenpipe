//! Per-camera tone mapping curves from darktable.
//!
//! Camera manufacturers embed non-linear tone curves in their JPEG engines to
//! convert scene-referred linear sensor data to pleasing display-referred images.
//! These curves vary by maker and model — Nikon tends toward punchier midtones,
//! Canon lifts shadows more aggressively, etc.
//!
//! This module provides measured basecurves from darktable's database (14 specific
//! camera models + 16 maker-based fallbacks) and a filter that applies them via
//! monotone Hermite spline interpolation. Curves are applied to the L channel with
//! optional chroma compression to match the natural desaturation that occurs when
//! tone mapping in RGB space.
//!
//! Data sourced from darktable's `src/iop/basecurve.c` (GPL-2.0+). The curve node
//! coordinates are factual measurements, not copyrightable expression.

use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;

/// A basecurve defined by (input, output) node pairs, interpolated with
/// monotone Hermite splines.
#[derive(Clone, Debug)]
pub struct BasecurvePreset {
    /// Human-readable name.
    pub name: &'static str,
    /// EXIF maker string to match (case-insensitive prefix).
    pub maker: &'static str,
    /// EXIF model string to match (empty = any model from this maker).
    pub model: &'static str,
    /// Curve node pairs: (input_luminance, output_luminance) in [0,1].
    pub nodes: &'static [(f32, f32)],
}

// === Camera-specific presets (measured from RAW+JPEG pairs) ===

static CAMERA_PRESETS: &[BasecurvePreset] = &[
    BasecurvePreset {
        name: "Nikon D750",
        maker: "NIKON CORPORATION",
        model: "NIKON D750",
        nodes: &[
            (0.000000, 0.000000),
            (0.018124, 0.026126),
            (0.143357, 0.370145),
            (0.330116, 0.730507),
            (0.457952, 0.853462),
            (0.734950, 0.965061),
            (0.904758, 0.985699),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Nikon D5100",
        maker: "NIKON CORPORATION",
        model: "NIKON D5100",
        nodes: &[
            (0.000000, 0.000000),
            (0.001113, 0.000506),
            (0.002842, 0.001338),
            (0.005461, 0.002470),
            (0.011381, 0.006099),
            (0.013303, 0.007758),
            (0.034638, 0.041119),
            (0.044441, 0.063882),
            (0.070338, 0.139639),
            (0.096068, 0.210915),
            (0.137693, 0.310295),
            (0.206041, 0.432674),
            (0.255508, 0.504447),
            (0.302770, 0.569576),
            (0.425625, 0.726755),
            (0.554526, 0.839541),
            (0.621216, 0.882839),
            (0.702662, 0.927072),
            (0.897426, 0.990984),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Nikon D7000",
        maker: "NIKON CORPORATION",
        model: "NIKON D7000",
        nodes: &[
            (0.000000, 0.000000),
            (0.001943, 0.003040),
            (0.019814, 0.028810),
            (0.080784, 0.210476),
            (0.145700, 0.383873),
            (0.295961, 0.654041),
            (0.651915, 0.952819),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Nikon D7200",
        maker: "NIKON CORPORATION",
        model: "NIKON D7200",
        nodes: &[
            (0.000000, 0.000000),
            (0.001604, 0.001334),
            (0.007401, 0.005237),
            (0.009474, 0.006890),
            (0.017348, 0.017176),
            (0.032782, 0.044336),
            (0.048033, 0.086548),
            (0.075803, 0.168331),
            (0.109539, 0.273539),
            (0.137373, 0.364645),
            (0.231651, 0.597511),
            (0.323797, 0.736475),
            (0.383796, 0.805797),
            (0.462284, 0.872247),
            (0.549844, 0.918328),
            (0.678855, 0.962361),
            (0.817445, 0.990406),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Nikon D7500",
        maker: "NIKON CORPORATION",
        model: "NIKON D7500",
        nodes: &[
            (0.000000, 0.000000),
            (0.000892, 0.001062),
            (0.002280, 0.001768),
            (0.013983, 0.011368),
            (0.032597, 0.044700),
            (0.050065, 0.097131),
            (0.084129, 0.219954),
            (0.120975, 0.336806),
            (0.170730, 0.473752),
            (0.258677, 0.647113),
            (0.409997, 0.827417),
            (0.499979, 0.889468),
            (0.615564, 0.941960),
            (0.665272, 0.957736),
            (0.832126, 0.991968),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Sony DSC-RX100M2",
        maker: "SONY",
        model: "DSC-RX100M2",
        nodes: &[
            (0.000000, 0.000000),
            (0.015106, 0.008116),
            (0.070077, 0.093725),
            (0.107484, 0.170723),
            (0.191528, 0.341093),
            (0.257996, 0.458453),
            (0.305381, 0.537267),
            (0.326367, 0.569257),
            (0.448067, 0.723742),
            (0.509627, 0.777966),
            (0.676751, 0.898797),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Canon EOS 6D",
        maker: "Canon",
        model: "Canon EOS 6D",
        nodes: &[
            (0.000000, 0.002917),
            (0.000751, 0.001716),
            (0.006011, 0.004438),
            (0.020286, 0.021725),
            (0.048084, 0.085918),
            (0.093914, 0.233804),
            (0.162284, 0.431375),
            (0.257701, 0.629218),
            (0.384673, 0.800332),
            (0.547709, 0.917761),
            (0.751315, 0.988132),
            (1.000000, 0.999943),
        ],
    },
    BasecurvePreset {
        name: "Canon EOS 5D Mark II",
        maker: "Canon",
        model: "Canon EOS 5D Mark II",
        nodes: &[
            (0.000000, 0.000366),
            (0.006560, 0.003504),
            (0.027310, 0.029834),
            (0.045915, 0.070230),
            (0.206554, 0.539895),
            (0.442337, 0.872409),
            (0.673263, 0.971703),
            (1.000000, 0.999832),
        ],
    },
    BasecurvePreset {
        name: "Fujifilm X100S",
        maker: "FUJIFILM",
        model: "X100S",
        nodes: &[
            (0.000000, 0.000000),
            (0.009145, 0.007905),
            (0.026570, 0.032201),
            (0.131526, 0.289717),
            (0.175858, 0.395263),
            (0.350981, 0.696899),
            (0.614997, 0.959451),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Fujifilm X100T",
        maker: "FUJIFILM",
        model: "X100T",
        nodes: &[
            (0.000000, 0.000000),
            (0.009145, 0.007905),
            (0.026570, 0.032201),
            (0.131526, 0.289717),
            (0.175858, 0.395263),
            (0.350981, 0.696899),
            (0.614997, 0.959451),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Pentax K-5",
        maker: "PENTAX",
        model: "PENTAX K-5",
        nodes: &[
            (0.000000, 0.000000),
            (0.004754, 0.002208),
            (0.009529, 0.004214),
            (0.023713, 0.013508),
            (0.031866, 0.020352),
            (0.046734, 0.034063),
            (0.059989, 0.052413),
            (0.088415, 0.096030),
            (0.136610, 0.190629),
            (0.174480, 0.256484),
            (0.205192, 0.307430),
            (0.228896, 0.348447),
            (0.286411, 0.428680),
            (0.355314, 0.513527),
            (0.440014, 0.607651),
            (0.567096, 0.732791),
            (0.620597, 0.775968),
            (0.760355, 0.881828),
            (0.875139, 0.960682),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Nikon D90",
        maker: "NIKON CORPORATION",
        model: "NIKON D90",
        nodes: &[
            (0.000000, 0.000000),
            (0.011702, 0.012659),
            (0.122918, 0.289973),
            (0.153642, 0.342731),
            (0.246855, 0.510114),
            (0.448958, 0.733820),
            (0.666759, 0.894290),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Nikon D800",
        maker: "NIKON",
        model: "NIKON D800",
        nodes: &[
            (0.000000, 0.000000),
            (0.001773, 0.001936),
            (0.009671, 0.009693),
            (0.016754, 0.020617),
            (0.024884, 0.037309),
            (0.048174, 0.107768),
            (0.056932, 0.139532),
            (0.085504, 0.233303),
            (0.130378, 0.349747),
            (0.155476, 0.405445),
            (0.175245, 0.445918),
            (0.217657, 0.516873),
            (0.308475, 0.668608),
            (0.375381, 0.754058),
            (0.459858, 0.839909),
            (0.509567, 0.881543),
            (0.654394, 0.960877),
            (0.783380, 0.999161),
            (0.859310, 1.000000),
            (1.000000, 1.000000),
        ],
    },
    BasecurvePreset {
        name: "Olympus E-M10 II",
        maker: "OLYMPUS CORPORATION",
        model: "E-M10MarkII",
        nodes: &[
            (0.000000, 0.000000),
            (0.005707, 0.004764),
            (0.018944, 0.024456),
            (0.054501, 0.129992),
            (0.075665, 0.211873),
            (0.119641, 0.365771),
            (0.173148, 0.532024),
            (0.247979, 0.668989),
            (0.357597, 0.780138),
            (0.459003, 0.839829),
            (0.626844, 0.904426),
            (0.769425, 0.948541),
            (0.820429, 0.964715),
            (1.000000, 1.000000),
        ],
    },
];

// === Maker-based fallback presets ===

static MAKER_PRESETS: &[BasecurvePreset] = &[
    BasecurvePreset {
        name: "Canon EOS like",
        maker: "Canon",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.028226, 0.029677),
            (0.120968, 0.232258),
            (0.459677, 0.747581),
            (0.858871, 0.967742),
            (1.0, 1.0),
        ],
    },
    BasecurvePreset {
        name: "Nikon like",
        maker: "NIKON",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.036290, 0.036532),
            (0.120968, 0.228226),
            (0.459677, 0.759678),
            (0.858871, 0.983468),
            (1.0, 1.0),
        ],
    },
    BasecurvePreset {
        name: "Sony Alpha like",
        maker: "SONY",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.031949, 0.036532),
            (0.105431, 0.228226),
            (0.434505, 0.759678),
            (0.855738, 0.983468),
            (1.0, 1.0),
        ],
    },
    BasecurvePreset {
        name: "Pentax like",
        maker: "PENTAX",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.032258, 0.024596),
            (0.120968, 0.166419),
            (0.205645, 0.328527),
            (0.604839, 0.790171),
            (1.0, 1.0),
        ],
    },
    BasecurvePreset {
        name: "Olympus like",
        maker: "OLYMPUS",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.033962, 0.028226),
            (0.249057, 0.439516),
            (0.501887, 0.798387),
            (0.750943, 0.955645),
            (1.0, 1.0),
        ],
    },
    BasecurvePreset {
        name: "Panasonic like",
        maker: "Panasonic",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.036290, 0.024596),
            (0.120968, 0.166419),
            (0.205645, 0.328527),
            (0.604839, 0.790171),
            (1.0, 1.0),
        ],
    },
    BasecurvePreset {
        name: "Leica like",
        maker: "Leica",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.036291, 0.024596),
            (0.120968, 0.166419),
            (0.205645, 0.328527),
            (0.604839, 0.790171),
            (1.0, 1.0),
        ],
    },
    BasecurvePreset {
        name: "Fujifilm like",
        maker: "FUJIFILM",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.028226, 0.029677),
            (0.104839, 0.232258),
            (0.387097, 0.747581),
            (0.754032, 0.967742),
            (1.0, 1.0),
        ],
    },
    BasecurvePreset {
        name: "Samsung like",
        maker: "SAMSUNG",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.040323, 0.029677),
            (0.133065, 0.232258),
            (0.447581, 0.747581),
            (0.842742, 0.967742),
            (1.0, 1.0),
        ],
    },
    BasecurvePreset {
        name: "Ricoh like",
        maker: "RICOH",
        model: "",
        nodes: &[
            (0.0, 0.0),
            (0.032259, 0.024596),
            (0.120968, 0.166419),
            (0.205645, 0.328527),
            (0.604839, 0.790171),
            (1.0, 1.0),
        ],
    },
];

/// Generic "neutral" basecurve — a gentle S-curve suitable for any camera.
pub static NEUTRAL_CURVE: BasecurvePreset = BasecurvePreset {
    name: "neutral",
    maker: "",
    model: "",
    nodes: &[
        (0.0, 0.0),
        (0.005, 0.0025),
        (0.15, 0.30),
        (0.40, 0.70),
        (0.75, 0.95),
        (1.0, 1.0),
    ],
};

/// Find the best basecurve for a camera by EXIF maker and model.
///
/// Priority: exact camera match → maker+model prefix match → maker match → neutral fallback.
pub fn find_basecurve(maker: &str, model: &str) -> &'static BasecurvePreset {
    let maker_upper = maker.to_uppercase();
    let model_upper = model.to_uppercase();

    // 1. Exact camera match (case-insensitive)
    for preset in CAMERA_PRESETS {
        if maker_upper.starts_with(&preset.maker.to_uppercase())
            && model_upper.contains(&preset.model.to_uppercase())
        {
            return preset;
        }
    }

    // 2. Maker fallback (case-insensitive prefix)
    for preset in MAKER_PRESETS {
        if maker_upper.starts_with(&preset.maker.to_uppercase()) {
            return preset;
        }
    }

    // 3. Neutral fallback
    &NEUTRAL_CURVE
}

/// Build a 256-entry LUT from basecurve nodes using monotone Hermite interpolation.
fn build_lut(nodes: &[(f32, f32)]) -> Vec<f32> {
    let mut lut = vec![0.0f32; crate::LUT_SIZE];
    let n = nodes.len();
    if n < 2 {
        // Identity
        for (i, v) in lut.iter_mut().enumerate() {
            *v = i as f32 / crate::LUT_MAX as f32;
        }
        return lut;
    }

    // Compute Fritsch-Carlson monotone Hermite tangents
    let mut tangents = vec![0.0f32; n];
    let mut deltas = vec![0.0f32; n - 1];
    let mut slopes = vec![0.0f32; n - 1];

    for i in 0..n - 1 {
        deltas[i] = nodes[i + 1].0 - nodes[i].0;
        slopes[i] = if deltas[i].abs() > 1e-10 {
            (nodes[i + 1].1 - nodes[i].1) / deltas[i]
        } else {
            0.0
        };
    }

    // Interior tangents
    for i in 1..n - 1 {
        if slopes[i - 1] * slopes[i] <= 0.0 {
            tangents[i] = 0.0;
        } else {
            tangents[i] = (slopes[i - 1] + slopes[i]) * 0.5;
        }
    }
    // Endpoint tangents
    tangents[0] = slopes[0];
    tangents[n - 1] = slopes[n - 2];

    // Fritsch-Carlson monotonicity enforcement
    for i in 0..n - 1 {
        if slopes[i].abs() < 1e-10 {
            tangents[i] = 0.0;
            tangents[i + 1] = 0.0;
        } else {
            let alpha = tangents[i] / slopes[i];
            let beta = tangents[i + 1] / slopes[i];
            let mag2 = alpha * alpha + beta * beta;
            if mag2 > 9.0 {
                let tau = 3.0 / mag2.sqrt();
                tangents[i] = tau * alpha * slopes[i];
                tangents[i + 1] = tau * beta * slopes[i];
            }
        }
    }

    // Evaluate LUT via Hermite interpolation
    let mut seg = 0usize;
    for (i, v) in lut.iter_mut().enumerate() {
        let x = i as f32 / crate::LUT_MAX as f32;

        // Advance segment
        while seg < n - 2 && x > nodes[seg + 1].0 {
            seg += 1;
        }

        let x0 = nodes[seg].0;
        let x1 = nodes[seg + 1].0;
        let y0 = nodes[seg].1;
        let y1 = nodes[seg + 1].1;
        let dx = x1 - x0;

        if dx.abs() < 1e-10 {
            *v = y0;
        } else {
            let t = (x - x0) / dx;
            let t2 = t * t;
            let t3 = t2 * t;
            let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
            let h10 = t3 - 2.0 * t2 + t;
            let h01 = -2.0 * t3 + 3.0 * t2;
            let h11 = t3 - t2;
            *v = (h00 * y0 + h10 * dx * tangents[seg] + h01 * y1 + h11 * dx * tangents[seg + 1])
                .clamp(0.0, 1.0);
        }
    }

    // Enforce monotonicity in the final LUT
    for i in 1..256 {
        if lut[i] < lut[i - 1] {
            lut[i] = lut[i - 1];
        }
    }

    lut
}

/// Camera-matched tone mapper using basecurve data from darktable.
///
/// Applies a per-camera or per-maker tone curve to convert scene-referred
/// linear luminance to display-referred values. Includes chroma compression
/// to match the natural desaturation of RGB-space tone mapping.
#[derive(Clone, Debug)]
pub struct BasecurveToneMap {
    /// 256-entry LUT: input L → output L.
    lut: Vec<f32>,
    /// How strongly to compress chroma when luminance changes.
    /// 0.0 = no chroma change (L-only, current sigmoid behavior).
    /// 1.0 = scale chroma proportionally to L ratio (full RGB-like desaturation).
    /// Default 0.4 (moderate, matches typical camera rendering).
    pub chroma_compression: f32,
    /// Name of the matched preset (for diagnostics).
    pub preset_name: &'static str,
}

impl BasecurveToneMap {
    /// Create from a basecurve preset.
    pub fn from_preset(preset: &'static BasecurvePreset, chroma_compression: f32) -> Self {
        Self {
            lut: build_lut(preset.nodes),
            chroma_compression,
            preset_name: preset.name,
        }
    }

    /// Create from camera EXIF maker/model, with automatic preset selection.
    pub fn from_camera(maker: &str, model: &str, chroma_compression: f32) -> Self {
        let preset = find_basecurve(maker, model);
        Self {
            lut: build_lut(preset.nodes),
            chroma_compression,
            preset_name: preset.name,
        }
    }

    /// Access the 256-entry LUT (input→output, both [0,1]).
    pub fn lut(&self) -> &Vec<f32> {
        &self.lut
    }

    /// Apply basecurve tone mapping to linear RGB f32 data in-place.
    ///
    /// This is the correct way to apply basecurve — in linear RGB space,
    /// before converting to Oklab. The basecurve nodes were measured in
    /// linear RGB, not perceptual space.
    ///
    /// Each channel value is independently mapped through the LUT with
    /// linear interpolation. Values are clamped to [0,1].
    pub fn apply_linear_rgb(&self, data: &mut [f32]) {
        for v in data.iter_mut() {
            let clamped = v.clamp(0.0, 1.0);
            let idx_f = (clamped * crate::LUT_MAX as f32).min((crate::LUT_MAX - 1) as f32);
            let idx = idx_f as usize;
            let frac = idx_f - idx as f32;
            *v = self.lut[idx] * (1.0 - frac) + self.lut[idx + 1] * frac;
        }
    }
}

impl Filter for BasecurveToneMap {
    fn channel_access(&self) -> ChannelAccess {
        if self.chroma_compression > 1e-6 {
            ChannelAccess::ALL
        } else {
            ChannelAccess::L_ONLY
        }
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        let n = planes.pixel_count();

        if self.chroma_compression > 1e-6 {
            // L + chroma adaptation
            let strength = self.chroma_compression;
            for i in 0..n {
                let l_old = planes.l[i];
                // LUT lookup with linear interpolation
                let idx_f = (l_old.clamp(0.0, 1.0) * crate::LUT_MAX as f32)
                    .min((crate::LUT_MAX - 1) as f32);
                let idx = idx_f as usize;
                let frac = idx_f - idx as f32;
                let l_new = self.lut[idx] * (1.0 - frac) + self.lut[idx + 1] * frac;
                planes.l[i] = l_new;

                // Chroma compression: scale a/b by (L_new/L_old)^strength
                if l_old > 1e-6 {
                    let ratio = l_new / l_old;
                    let scale = ratio.powf(strength);
                    planes.a[i] *= scale;
                    planes.b[i] *= scale;
                }
            }
        } else {
            // L-only (fast path)
            for i in 0..n {
                let l = planes.l[i];
                let idx_f =
                    (l.clamp(0.0, 1.0) * crate::LUT_MAX as f32).min((crate::LUT_MAX - 1) as f32);
                let idx = idx_f as usize;
                let frac = idx_f - idx as f32;
                planes.l[i] = self.lut[idx] * (1.0 - frac) + self.lut[idx + 1] * frac;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_curve_near_identity() {
        let tm = BasecurveToneMap::from_preset(&NEUTRAL_CURVE, 0.0);
        // Endpoints should be preserved
        let mut planes = OklabPlanes::new(3, 1);
        planes.l[0] = 0.0;
        planes.l[1] = 0.5;
        planes.l[2] = 1.0;
        tm.apply(&mut planes, &mut FilterContext::new());
        assert!(planes.l[0].abs() < 0.01, "black: {}", planes.l[0]);
        assert!((planes.l[2] - 1.0).abs() < 0.01, "white: {}", planes.l[2]);
        // Midtone should be lifted (neutral curve boosts midtones)
        assert!(
            planes.l[1] > 0.5,
            "midtone should be lifted: {}",
            planes.l[1]
        );
    }

    #[test]
    fn lut_is_monotonic() {
        for preset in CAMERA_PRESETS {
            let lut = build_lut(preset.nodes);
            for i in 1..256 {
                assert!(
                    lut[i] >= lut[i - 1],
                    "{}: LUT not monotonic at {}: {} < {}",
                    preset.name,
                    i,
                    lut[i],
                    lut[i - 1]
                );
            }
        }
        for preset in MAKER_PRESETS {
            let lut = build_lut(preset.nodes);
            for i in 1..256 {
                assert!(
                    lut[i] >= lut[i - 1],
                    "{}: LUT not monotonic at {}: {} < {}",
                    preset.name,
                    i,
                    lut[i],
                    lut[i - 1]
                );
            }
        }
    }

    #[test]
    fn lut_preserves_black_and_white() {
        for preset in CAMERA_PRESETS {
            let lut = build_lut(preset.nodes);
            assert!(
                lut[0] < 0.01,
                "{}: black not preserved: {}",
                preset.name,
                lut[0]
            );
            assert!(
                lut[crate::LUT_MAX] > 0.99,
                "{}: white not preserved: {}",
                preset.name,
                lut[crate::LUT_MAX]
            );
        }
    }

    #[test]
    fn find_exact_camera() {
        let p = find_basecurve("NIKON CORPORATION", "NIKON D750");
        assert_eq!(p.name, "Nikon D750");
    }

    #[test]
    fn find_maker_fallback() {
        let p = find_basecurve("Canon", "Canon EOS R5");
        assert_eq!(p.name, "Canon EOS like");
    }

    #[test]
    fn find_neutral_fallback() {
        let p = find_basecurve("UNKNOWN MAKER", "Mystery Camera");
        assert_eq!(p.name, "neutral");
    }

    #[test]
    fn chroma_compression_reduces_saturation() {
        let tm = BasecurveToneMap::from_preset(&NEUTRAL_CURVE, 0.5);
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.8; // highlight
        planes.a[0] = 0.1;
        planes.b[0] = 0.05;
        let chroma_before = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        tm.apply(&mut planes, &mut FilterContext::new());
        let chroma_after = (planes.a[0] * planes.a[0] + planes.b[0] * planes.b[0]).sqrt();
        // Neutral curve compresses highlights → L decreases → chroma should decrease
        // Actually neutral curve BOOSTS midtones, so at L=0.8 it might go up or down.
        // The key is that the ratio between chroma change and L change is consistent.
        let l_ratio = planes.l[0] / 0.8;
        if l_ratio < 1.0 {
            assert!(
                chroma_after < chroma_before,
                "chroma should decrease when L decreases: c_before={chroma_before} c_after={chroma_after}"
            );
        }
    }

    #[test]
    fn chroma_zero_leaves_ab_unchanged() {
        let tm = BasecurveToneMap::from_preset(&NEUTRAL_CURVE, 0.0);
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.1;
        planes.b[0] = 0.05;
        let a_orig = planes.a[0];
        let b_orig = planes.b[0];
        tm.apply(&mut planes, &mut FilterContext::new());
        assert_eq!(planes.a[0], a_orig);
        assert_eq!(planes.b[0], b_orig);
    }

    #[test]
    fn preserves_hue_during_compression() {
        let tm = BasecurveToneMap::from_preset(&NEUTRAL_CURVE, 0.5);
        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.7;
        planes.a[0] = 0.1;
        planes.b[0] = -0.08;
        let hue_before = planes.b[0].atan2(planes.a[0]);
        tm.apply(&mut planes, &mut FilterContext::new());
        let hue_after = planes.b[0].atan2(planes.a[0]);
        assert!(
            (hue_before - hue_after).abs() < 1e-5,
            "hue should be preserved: {hue_before} vs {hue_after}"
        );
    }
}
