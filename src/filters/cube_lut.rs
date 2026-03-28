use crate::access::ChannelAccess;
use crate::context::FilterContext;
use crate::filter::Filter;
use crate::planes::OklabPlanes;
use crate::prelude::*;
use zenpixels_convert::oklab;

/// 3D color lookup table loaded from Adobe .cube format.
///
/// .cube is the universal LUT exchange format used by colorists. Every
/// major grading application (DaVinci Resolve, Premiere, FCPX, Lightroom)
/// can export and import .cube files.
///
/// The LUT maps linear RGB → linear RGB via trilinear interpolation on a
/// uniform 3D grid. Typical sizes are 17³, 33³, or 65³.
///
/// The filter converts Oklab → linear RGB, applies the LUT, then converts
/// back to Oklab.
///
/// A `strength` parameter (0.0–1.0) blends between the original and
/// LUT-transformed color for partial application.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct CubeLut {
    /// Flattened 3D LUT data: `[r][g][b]` order, each entry is `[R, G, B]`.
    /// Total entries: `size * size * size`.
    data: Vec<[f32; 3]>,
    /// Grid size per axis (e.g., 17, 33, 65).
    size: usize,
    /// Input domain minimum (default: [0, 0, 0]).
    domain_min: [f32; 3],
    /// Input domain maximum (default: [1, 1, 1]).
    domain_max: [f32; 3],
    /// Blend strength. 1.0 = full LUT, 0.0 = bypass.
    pub strength: f32,
    /// Optional title from the .cube file.
    pub title: String,
}

/// Errors from parsing .cube files.
#[derive(Clone, Debug)]
pub enum CubeParseError {
    /// No LUT_3D_SIZE found.
    MissingSize,
    /// LUT size is outside valid range (2–256).
    InvalidSize(usize),
    /// Not enough data rows for the declared size.
    InsufficientData { expected: usize, found: usize },
    /// A data line couldn't be parsed as three floats.
    BadDataLine(usize),
}

impl core::fmt::Display for CubeParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingSize => write!(f, "no LUT_3D_SIZE found in .cube file"),
            Self::InvalidSize(s) => write!(f, "invalid LUT_3D_SIZE {s} (must be 2–256)"),
            Self::InsufficientData { expected, found } => {
                write!(f, "expected {expected} data rows, found {found}")
            }
            Self::BadDataLine(n) => write!(f, "bad data at line {n}"),
        }
    }
}

impl Default for CubeLut {
    fn default() -> Self {
        // 2×2×2 identity LUT
        let size = 2;
        let mut data = Vec::with_capacity(size * size * size);
        for r in 0..size {
            for g in 0..size {
                for b in 0..size {
                    data.push([
                        r as f32 / (size - 1) as f32,
                        g as f32 / (size - 1) as f32,
                        b as f32 / (size - 1) as f32,
                    ]);
                }
            }
        }
        Self {
            data,
            size,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            strength: 1.0,
            title: String::new(),
        }
    }
}

impl CubeLut {
    /// Parse a .cube file from its text content.
    ///
    /// Supports the standard Adobe .cube format:
    /// - `TITLE "name"` (optional)
    /// - `LUT_3D_SIZE N` (required)
    /// - `DOMAIN_MIN r g b` (optional, default 0 0 0)
    /// - `DOMAIN_MAX r g b` (optional, default 1 1 1)
    /// - Data rows: `r g b` (one per line, N³ total)
    ///
    /// Lines starting with `#` are comments. Blank lines are skipped.
    /// 1D LUTs (`LUT_1D_SIZE`) are not supported by this filter.
    pub fn parse(text: &str) -> Result<Self, CubeParseError> {
        let mut size: Option<usize> = None;
        let mut domain_min = [0.0f32; 3];
        let mut domain_max = [1.0f32; 3];
        let mut title = String::new();
        let mut data = Vec::new();
        let mut line_num = 0;

        for line in text.lines() {
            line_num += 1;
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Metadata keywords
            if let Some(rest) = line.strip_prefix("TITLE") {
                let rest = rest.trim().trim_matches('"');
                title = String::from(rest);
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                if let Ok(s) = rest.trim().parse::<usize>() {
                    size = Some(s);
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_1D_SIZE") {
                // Skip 1D LUT size declarations — we only handle 3D
                let _ = rest;
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MIN") {
                if let Some(vals) = parse_three_floats(rest) {
                    domain_min = vals;
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MAX") {
                if let Some(vals) = parse_three_floats(rest) {
                    domain_max = vals;
                }
                continue;
            }

            // Data lines: three floats separated by whitespace
            if let Some(vals) = parse_three_floats(line) {
                data.push(vals);
            } else {
                // Check if it looks like a keyword we should skip
                if line.chars().next().is_some_and(|c| c.is_alphabetic()) {
                    continue;
                }
                return Err(CubeParseError::BadDataLine(line_num));
            }
        }

        let size = size.ok_or(CubeParseError::MissingSize)?;
        if !(2..=256).contains(&size) {
            return Err(CubeParseError::InvalidSize(size));
        }

        let expected = size * size * size;
        if data.len() < expected {
            return Err(CubeParseError::InsufficientData {
                expected,
                found: data.len(),
            });
        }
        data.truncate(expected);

        Ok(Self {
            data,
            size,
            domain_min,
            domain_max,
            strength: 1.0,
            title,
        })
    }

    /// Create an identity LUT of the given size.
    pub fn identity(size: usize) -> Self {
        let n = size * size * size;
        let mut data = Vec::with_capacity(n);
        let scale = 1.0 / (size - 1) as f32;
        for r in 0..size {
            for g in 0..size {
                for b in 0..size {
                    data.push([r as f32 * scale, g as f32 * scale, b as f32 * scale]);
                }
            }
        }
        Self {
            data,
            size,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            strength: 1.0,
            title: String::new(),
        }
    }

    /// Grid size per axis.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Total memory footprint in bytes.
    pub fn size_bytes(&self) -> usize {
        self.data.len() * core::mem::size_of::<[f32; 3]>()
    }

    /// Trilinear lookup into the 3D LUT.
    #[inline]
    fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let s = self.size;
        let max_idx = (s - 1) as f32;

        // Normalize input to [0, 1] range using domain
        let nr = ((rgb[0] - self.domain_min[0]) / (self.domain_max[0] - self.domain_min[0]))
            .clamp(0.0, 1.0)
            * max_idx;
        let ng = ((rgb[1] - self.domain_min[1]) / (self.domain_max[1] - self.domain_min[1]))
            .clamp(0.0, 1.0)
            * max_idx;
        let nb = ((rgb[2] - self.domain_min[2]) / (self.domain_max[2] - self.domain_min[2]))
            .clamp(0.0, 1.0)
            * max_idx;

        let r0 = nr as usize;
        let g0 = ng as usize;
        let b0 = nb as usize;
        let r1 = (r0 + 1).min(s - 1);
        let g1 = (g0 + 1).min(s - 1);
        let b1 = (b0 + 1).min(s - 1);

        let fr = nr - r0 as f32;
        let fg = ng - g0 as f32;
        let fb = nb - b0 as f32;

        // 8 corner lookups
        let c000 = self.data[r0 * s * s + g0 * s + b0];
        let c001 = self.data[r0 * s * s + g0 * s + b1];
        let c010 = self.data[r0 * s * s + g1 * s + b0];
        let c011 = self.data[r0 * s * s + g1 * s + b1];
        let c100 = self.data[r1 * s * s + g0 * s + b0];
        let c101 = self.data[r1 * s * s + g0 * s + b1];
        let c110 = self.data[r1 * s * s + g1 * s + b0];
        let c111 = self.data[r1 * s * s + g1 * s + b1];

        // Trilinear interpolation
        let mut out = [0.0f32; 3];
        for ch in 0..3 {
            let c00 = c000[ch] * (1.0 - fb) + c001[ch] * fb;
            let c01 = c010[ch] * (1.0 - fb) + c011[ch] * fb;
            let c10 = c100[ch] * (1.0 - fb) + c101[ch] * fb;
            let c11 = c110[ch] * (1.0 - fb) + c111[ch] * fb;

            let c0 = c00 * (1.0 - fg) + c01 * fg;
            let c1 = c10 * (1.0 - fg) + c11 * fg;

            out[ch] = c0 * (1.0 - fr) + c1 * fr;
        }
        out
    }
}

fn parse_three_floats(s: &str) -> Option<[f32; 3]> {
    let mut iter = s.split_whitespace();
    let r = iter.next()?.parse::<f32>().ok()?;
    let g = iter.next()?.parse::<f32>().ok()?;
    let b = iter.next()?.parse::<f32>().ok()?;
    Some([r, g, b])
}

impl Filter for CubeLut {
    fn channel_access(&self) -> ChannelAccess {
        ChannelAccess::L_AND_CHROMA
    }

    fn apply(&self, planes: &mut OklabPlanes, _ctx: &mut FilterContext) {
        if self.strength.abs() < 1e-6 {
            return;
        }

        let m1_inv = oklab::lms_to_rgb_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");
        let m1 = oklab::rgb_to_lms_matrix(zenpixels::ColorPrimaries::Bt709)
            .expect("BT.709 always supported");

        let n = planes.pixel_count();
        let blend = self.strength.clamp(0.0, 1.0);
        let inv_blend = 1.0 - blend;

        for i in 0..n {
            // Oklab → linear RGB
            let [r, g, b] = oklab::oklab_to_rgb(planes.l[i], planes.a[i], planes.b[i], &m1_inv);

            // Apply 3D LUT
            let lut_rgb = self.lookup([r.max(0.0), g.max(0.0), b.max(0.0)]);

            // Blend original and LUT result
            let r2 = inv_blend * r + blend * lut_rgb[0];
            let g2 = inv_blend * g + blend * lut_rgb[1];
            let b2 = inv_blend * b + blend * lut_rgb[2];

            // Linear RGB → Oklab
            let [l, oa, ob] = oklab::rgb_to_oklab(r2.max(0.0), g2.max(0.0), b2.max(0.0), &m1);
            planes.l[i] = l;
            planes.a[i] = oa;
            planes.b[i] = ob;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_lut_is_noop() {
        let lut = CubeLut::identity(17);
        assert_eq!(lut.size(), 17);

        let mut planes = OklabPlanes::new(4, 4);
        for (i, v) in planes.l.iter_mut().enumerate() {
            *v = 0.3 + (i as f32) * 0.01;
        }
        for v in &mut planes.a {
            *v = 0.05;
        }
        for v in &mut planes.b {
            *v = -0.03;
        }
        let l_orig = planes.l.clone();
        let a_orig = planes.a.clone();
        let b_orig = planes.b.clone();

        lut.apply(&mut planes, &mut FilterContext::new());

        for i in 0..planes.pixel_count() {
            assert!(
                (planes.l[i] - l_orig[i]).abs() < 0.02,
                "L[{i}]: {} vs {}",
                planes.l[i],
                l_orig[i]
            );
            assert!(
                (planes.a[i] - a_orig[i]).abs() < 0.02,
                "a[{i}]: {} vs {}",
                planes.a[i],
                a_orig[i]
            );
            assert!(
                (planes.b[i] - b_orig[i]).abs() < 0.02,
                "b[{i}]: {} vs {}",
                planes.b[i],
                b_orig[i]
            );
        }
    }

    #[test]
    fn parse_minimal_cube() {
        let cube_text = "\
# Comment line
TITLE \"Test LUT\"
LUT_3D_SIZE 2

0.0 0.0 0.0
0.0 0.0 1.0
0.0 1.0 0.0
0.0 1.0 1.0
1.0 0.0 0.0
1.0 0.0 1.0
1.0 1.0 0.0
1.0 1.0 1.0
";
        let lut = CubeLut::parse(cube_text).unwrap();
        assert_eq!(lut.size(), 2);
        assert_eq!(lut.title, "Test LUT");
        assert_eq!(lut.data.len(), 8);
    }

    #[test]
    fn parse_with_domain() {
        let cube_text = "\
LUT_3D_SIZE 2
DOMAIN_MIN 0.0 0.0 0.0
DOMAIN_MAX 1.0 1.0 1.0

0.0 0.0 0.0
0.0 0.0 1.0
0.0 1.0 0.0
0.0 1.0 1.0
1.0 0.0 0.0
1.0 0.0 1.0
1.0 1.0 0.0
1.0 1.0 1.0
";
        let lut = CubeLut::parse(cube_text).unwrap();
        assert_eq!(lut.domain_min, [0.0; 3]);
        assert_eq!(lut.domain_max, [1.0; 3]);
    }

    #[test]
    fn parse_error_missing_size() {
        let cube_text = "0.0 0.0 0.0\n0.0 0.0 1.0\n";
        assert!(matches!(
            CubeLut::parse(cube_text),
            Err(CubeParseError::MissingSize)
        ));
    }

    #[test]
    fn parse_error_insufficient_data() {
        let cube_text = "LUT_3D_SIZE 2\n0.0 0.0 0.0\n";
        assert!(matches!(
            CubeLut::parse(cube_text),
            Err(CubeParseError::InsufficientData { .. })
        ));
    }

    #[test]
    fn strength_zero_is_bypass() {
        let mut lut = CubeLut::identity(2);
        // Make LUT non-identity: invert all values
        for entry in &mut lut.data {
            *entry = [1.0 - entry[0], 1.0 - entry[1], 1.0 - entry[2]];
        }
        lut.strength = 0.0;

        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.5;
        planes.a[0] = 0.05;
        planes.b[0] = -0.03;
        let l_orig = planes.l[0];

        lut.apply(&mut planes, &mut FilterContext::new());
        assert!(
            (planes.l[0] - l_orig).abs() < 1e-6,
            "strength=0 should bypass: {} vs {}",
            planes.l[0],
            l_orig
        );
    }

    #[test]
    fn strength_partial_blends() {
        // Create a LUT that maps everything to white
        let mut lut = CubeLut::identity(2);
        for entry in &mut lut.data {
            *entry = [1.0, 1.0, 1.0];
        }
        lut.strength = 0.5;

        let mut planes = OklabPlanes::new(1, 1);
        planes.l[0] = 0.3;
        planes.a[0] = 0.0;
        planes.b[0] = 0.0;

        lut.apply(&mut planes, &mut FilterContext::new());

        // Should be between original L and 1.0
        assert!(
            planes.l[0] > 0.3 && planes.l[0] < 1.0,
            "50% blend should be between original and white: {}",
            planes.l[0]
        );
    }

    #[test]
    fn trilinear_interpolation_midpoint() {
        // Identity LUT: lookup at midpoint should return midpoint
        let lut = CubeLut::identity(17);
        let result = lut.lookup([0.5, 0.5, 0.5]);
        for ch in 0..3 {
            assert!(
                (result[ch] - 0.5).abs() < 0.01,
                "midpoint ch{ch}: {}",
                result[ch]
            );
        }
    }

    #[test]
    fn size_bytes_correct() {
        let lut = CubeLut::identity(17);
        assert_eq!(lut.size_bytes(), 17 * 17 * 17 * 12); // 12 bytes per [f32; 3]
    }
}
