//! Format-agnostic depth map types and extraction.
//!
//! Depth maps encode per-pixel distance or disparity information, typically
//! from portrait mode photography or 3D-capable cameras. They can be stored
//! as secondary images in multi-image formats:
//!
//! - **JPEG**: iPhone MPF disparity maps, Android GDepth XMP, Android DDF
//! - **HEIC**: iPhone auxiliary depth images
//!
//! # Depth representations
//!
//! Depth data can be encoded several ways:
//! - **Range linear**: `depth = near + (far - near) * normalized_value`
//! - **Range inverse**: More precision near camera (like Z-buffer)
//! - **Disparity**: `1/depth` in meters
//! - **Absolute depth**: Direct distance in meters
//!
//! Use [`DecodedDepthMap::to_normalized_f32`] to convert any representation
//! to a uniform [0.0, 1.0] range (near=0, far=1), or [`DecodedDepthMap::to_meters`]
//! for metric depth values.

use alloc::vec::Vec;

use crate::{CodecError, ImageFormat};

/// Depth map extracted from a decoded image.
///
/// Contains the depth image pixels, metadata describing the encoding,
/// and an optional confidence map. The depth image has already been decoded
/// from the container's embedded format.
#[derive(Clone, Debug)]
pub struct DecodedDepthMap {
    /// The depth image pixels.
    pub depth: DepthImage,
    /// Depth map metadata (range, format, units).
    pub metadata: DepthMapMetadata,
    /// Optional confidence/quality map.
    ///
    /// When present, each pixel indicates how reliable the corresponding
    /// depth value is. Higher values = more confident. The pixel format
    /// matches the depth image.
    pub confidence: Option<DepthImage>,
    /// Source format this depth map was extracted from.
    pub source_format: ImageFormat,
    /// Source device type (if detectable from metadata).
    pub source_device: DepthSource,
}

/// Depth image pixel data.
///
/// Raw pixel bytes representing depth values. The interpretation depends
/// on the [`DepthMapMetadata`] associated with this image.
#[derive(Clone, Debug)]
pub struct DepthImage {
    /// Raw pixel bytes.
    pub data: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format of the depth data.
    pub pixel_format: DepthPixelFormat,
}

/// Pixel format of depth data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DepthPixelFormat {
    /// 8-bit unsigned grayscale (0-255 mapped to near-far range).
    Gray8,
    /// 16-bit unsigned grayscale, little-endian (0-65535 mapped to near-far range).
    Gray16,
    /// 32-bit float, little-endian (meters or disparity, depending on metadata).
    Float32,
    /// 16-bit float (IEEE 754 half precision), little-endian.
    Float16,
}

impl DepthPixelFormat {
    /// Bytes per pixel for this format.
    #[must_use]
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Gray8 => 1,
            Self::Gray16 | Self::Float16 => 2,
            Self::Float32 => 4,
        }
    }
}

/// How depth values are encoded.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum DepthFormat {
    /// Linear mapping: `depth = near + (far - near) * normalized_value`
    RangeLinear,
    /// Inverse mapping: more precision near camera (like Z-buffer).
    /// `depth = 1.0 / (inv_near + (inv_far - inv_near) * normalized_value)`
    RangeInverse,
    /// Disparity (1/depth in meters).
    Disparity,
    /// Direct depth in meters (or other unit per [`DepthUnits`]).
    AbsoluteDepth,
}

/// Units for depth values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DepthUnits {
    /// Depth in meters.
    Meters,
    /// Depth in millimeters.
    Millimeters,
    /// Disparity in diopters (1/meters).
    Diopters,
    /// Normalized to 0.0..1.0 range (no physical unit).
    Normalized,
}

/// How depth is measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DepthMeasureType {
    /// Perpendicular distance to the camera sensor plane.
    #[default]
    OpticalAxis,
    /// Distance along the ray from camera center through each pixel.
    OpticRay,
}

/// Metadata describing how to interpret depth values.
#[derive(Clone, Debug, PartialEq)]
pub struct DepthMapMetadata {
    /// How depth values are encoded.
    pub format: DepthFormat,
    /// Near plane distance (in the units specified by [`units`](Self::units)).
    pub near: f32,
    /// Far plane distance (in the units specified by [`units`](Self::units)).
    ///
    /// May be `f32::INFINITY` for unbounded depth.
    pub far: f32,
    /// Units for depth values.
    pub units: DepthUnits,
    /// How depth is measured relative to the camera.
    pub measure_type: DepthMeasureType,
}

impl Default for DepthMapMetadata {
    fn default() -> Self {
        Self {
            format: DepthFormat::RangeLinear,
            near: 0.0,
            far: 1.0,
            units: DepthUnits::Normalized,
            measure_type: DepthMeasureType::OpticalAxis,
        }
    }
}

/// Source device/format that produced the depth map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DepthSource {
    /// Google Camera GDepth XMP namespace in JPEG.
    AndroidGDepth,
    /// Android Dynamic Depth Format (DDF) appended to JPEG.
    AndroidDdf,
    /// iPhone MPF secondary image with Disparity type.
    AppleMpf,
    /// HEIC auxiliary depth image (Apple or other).
    AppleHeic,
    /// AVIF auxiliary depth image (auxl + auxC depth URN).
    Avif,
    /// Unknown or undetectable source.
    #[default]
    Unknown,
}

impl DepthImage {
    /// Total number of pixels in the depth image.
    #[must_use]
    pub fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// Expected byte length based on dimensions and pixel format.
    #[must_use]
    pub fn expected_len(&self) -> u64 {
        self.pixel_count() * self.pixel_format.bytes_per_pixel() as u64
    }

    /// Validate that the data length matches dimensions and pixel format.
    pub fn validate(&self) -> core::result::Result<(), CodecError> {
        if self.width == 0 || self.height == 0 {
            return Err(CodecError::InvalidInput(alloc::format!(
                "depth image has zero dimensions: {}x{}",
                self.width,
                self.height,
            )));
        }
        let expected = self.expected_len();
        if self.data.len() as u64 != expected {
            return Err(CodecError::InvalidInput(alloc::format!(
                "depth image data length {} does not match {}x{} {:?} = {}",
                self.data.len(),
                self.width,
                self.height,
                self.pixel_format,
                expected,
            )));
        }
        Ok(())
    }
}

impl DecodedDepthMap {
    /// Convert depth values to normalized f32 in [0.0, 1.0] range.
    ///
    /// 0.0 = near plane, 1.0 = far plane.
    ///
    /// # Integer vs Float pixel formats
    ///
    /// For `RangeLinear` and `RangeInverse`, integer pixel formats (Gray8, Gray16)
    /// store the normalized position directly (0=near, 1=far). Float formats
    /// (Float32, Float16) store actual depth/disparity values.
    ///
    /// For `Disparity` and `AbsoluteDepth`, all pixel formats store physical
    /// values (disparity or depth in units).
    ///
    /// NaN and infinite values are clamped to [0.0, 1.0].
    #[must_use]
    pub fn to_normalized_f32(&self) -> Vec<f32> {
        let pixel_count = self.depth.pixel_count() as usize;
        let raw = read_raw_f32(&self.depth);
        let near = self.metadata.near;
        let far = self.metadata.far;
        let is_integer_format = matches!(
            self.depth.pixel_format,
            DepthPixelFormat::Gray8 | DepthPixelFormat::Gray16
        );

        let mut output = alloc::vec![0.0f32; pixel_count];

        match self.metadata.format {
            DepthFormat::RangeLinear => {
                let range = far - near;
                if range.abs() < f32::EPSILON {
                    // Degenerate: near == far, all pixels at same depth.
                    output.fill(0.0);
                } else if is_integer_format {
                    // Integer formats: raw 0..1 IS the normalized position.
                    for (out, &val) in output.iter_mut().zip(raw.iter()) {
                        *out = val.clamp(0.0, 1.0);
                    }
                } else {
                    // Float formats: raw is depth value, normalize to 0..1.
                    let range = far - near;
                    if range.abs() < f32::EPSILON {
                        output.fill(0.0);
                    } else {
                        let inv_range = 1.0 / range;
                        for (out, &val) in output.iter_mut().zip(raw.iter()) {
                            *out = ((val - near) * inv_range).clamp(0.0, 1.0);
                        }
                    }
                }
            }
            DepthFormat::RangeInverse => {
                if is_integer_format {
                    // Integer formats: raw 0..1 is normalized inverse depth position.
                    // Convert through depth, then back to linear normalized.
                    let range = far - near;
                    if range.abs() < f32::EPSILON || near.abs() < f32::EPSILON {
                        output.fill(0.0);
                    } else {
                        let inv_near = 1.0 / near;
                        let inv_far = if far.is_infinite() { 0.0 } else { 1.0 / far };
                        let inv_range = inv_far - inv_near;
                        let inv_depth_range = 1.0 / range;
                        for (out, &val) in output.iter_mut().zip(raw.iter()) {
                            let inv_depth = inv_near + inv_range * val;
                            let depth = if inv_depth.abs() < f32::EPSILON {
                                far
                            } else {
                                1.0 / inv_depth
                            };
                            *out = ((depth - near) * inv_depth_range).clamp(0.0, 1.0);
                        }
                    }
                } else {
                    // Float formats: raw is actual inverse depth value.
                    let range = far - near;
                    if range.abs() < f32::EPSILON {
                        output.fill(0.0);
                    } else {
                        let inv_depth_range = 1.0 / range;
                        for (out, &val) in output.iter_mut().zip(raw.iter()) {
                            let depth = if val.abs() < f32::EPSILON {
                                far
                            } else {
                                1.0 / val
                            };
                            *out = ((depth - near) * inv_depth_range).clamp(0.0, 1.0);
                        }
                    }
                }
            }
            DepthFormat::Disparity => {
                // All formats: raw is disparity (1/depth).
                let range = far - near;
                if range.abs() < f32::EPSILON {
                    output.fill(0.0);
                } else {
                    let inv_range = 1.0 / range;
                    for (out, &val) in output.iter_mut().zip(raw.iter()) {
                        let depth = if val.abs() < f32::EPSILON {
                            far // infinite distance
                        } else {
                            1.0 / val
                        };
                        *out = ((depth - near) * inv_range).clamp(0.0, 1.0);
                    }
                }
            }
            DepthFormat::AbsoluteDepth => {
                // All formats: raw is depth in units.
                let range = far - near;
                if range.abs() < f32::EPSILON {
                    output.fill(0.0);
                } else {
                    let inv_range = 1.0 / range;
                    for (out, &val) in output.iter_mut().zip(raw.iter()) {
                        *out = ((val - near) * inv_range).clamp(0.0, 1.0);
                    }
                }
            }
        }

        output
    }

    /// Convert depth values to absolute meters.
    ///
    /// Returns `None` if units are not metric (i.e., [`DepthUnits::Normalized`]).
    ///
    /// For metric units, converts all depth representations to meters:
    /// - `Meters`: direct pass-through
    /// - `Millimeters`: divide by 1000
    /// - `Diopters`: `1/value`
    /// - `Disparity` format with metric units: `1/disparity`
    /// - `RangeLinear`/`RangeInverse`: denormalize using near/far, then convert to meters
    ///
    /// Integer pixel formats (Gray8, Gray16) are treated as normalized 0..1 for
    /// `RangeLinear` and `RangeInverse`. Float formats store actual values.
    pub fn to_meters(&self) -> Option<Vec<f32>> {
        let units_scale = match self.metadata.units {
            DepthUnits::Meters => 1.0,
            DepthUnits::Millimeters => 0.001,
            DepthUnits::Diopters => {
                // Diopters are 1/meters — handled specially below
                -1.0
            }
            DepthUnits::Normalized => return None,
        };

        let pixel_count = self.depth.pixel_count() as usize;
        let raw = read_raw_f32(&self.depth);
        let near = self.metadata.near;
        let far = self.metadata.far;
        let is_integer_format = matches!(
            self.depth.pixel_format,
            DepthPixelFormat::Gray8 | DepthPixelFormat::Gray16
        );

        let mut output = alloc::vec![0.0f32; pixel_count];

        /// Convert a depth value (in native units) to meters.
        fn to_meters_val(depth: f32, units_scale: f32) -> f32 {
            if units_scale < 0.0 {
                // Diopters: depth is in diopter units, meter = 1/diopter
                if depth.abs() < f32::EPSILON {
                    f32::INFINITY
                } else {
                    1.0 / depth
                }
            } else {
                depth * units_scale
            }
        }

        match self.metadata.format {
            DepthFormat::RangeLinear => {
                let range = far - near;
                if is_integer_format {
                    // Integer: raw is normalized 0..1, interpolate between near..far
                    for (out, &val) in output.iter_mut().zip(raw.iter()) {
                        let depth = near + range * val.clamp(0.0, 1.0);
                        *out = to_meters_val(depth, units_scale);
                    }
                } else {
                    // Float: raw IS depth in units
                    for (out, &val) in output.iter_mut().zip(raw.iter()) {
                        *out = to_meters_val(val, units_scale);
                    }
                }
            }
            DepthFormat::RangeInverse => {
                if is_integer_format {
                    // Integer: raw is normalized 0..1 in inverse depth space
                    let inv_near = if near.abs() < f32::EPSILON {
                        f32::INFINITY
                    } else {
                        1.0 / near
                    };
                    let inv_far = if far.is_infinite() || far.abs() < f32::EPSILON {
                        0.0
                    } else {
                        1.0 / far
                    };
                    let inv_range = inv_far - inv_near;
                    for (out, &val) in output.iter_mut().zip(raw.iter()) {
                        let inv_depth = inv_near + inv_range * val.clamp(0.0, 1.0);
                        let depth = if inv_depth.abs() < f32::EPSILON {
                            far
                        } else {
                            1.0 / inv_depth
                        };
                        *out = to_meters_val(depth, units_scale);
                    }
                } else {
                    // Float: raw is inverse depth value
                    for (out, &val) in output.iter_mut().zip(raw.iter()) {
                        let depth = if val.abs() < f32::EPSILON {
                            far
                        } else {
                            1.0 / val
                        };
                        *out = to_meters_val(depth, units_scale);
                    }
                }
            }
            DepthFormat::Disparity => {
                // All formats: raw is disparity (1/depth).
                for (out, &val) in output.iter_mut().zip(raw.iter()) {
                    let depth = if val.abs() < f32::EPSILON {
                        f32::INFINITY
                    } else {
                        1.0 / val
                    };
                    *out = if units_scale < 0.0 {
                        // Diopters: disparity is already 1/depth
                        val
                    } else {
                        depth * units_scale
                    };
                }
            }
            DepthFormat::AbsoluteDepth => {
                // All formats: raw is depth in units.
                for (out, &val) in output.iter_mut().zip(raw.iter()) {
                    *out = to_meters_val(val, units_scale);
                }
            }
        }

        Some(output)
    }

    /// Resize the depth image to match a target resolution using bilinear interpolation.
    ///
    /// This is useful when the depth map has different dimensions than the
    /// base image (common for JPEG depth maps which are often lower resolution).
    #[must_use]
    pub fn resize(&self, target_width: u32, target_height: u32) -> DepthImage {
        let bpp = self.depth.pixel_format.bytes_per_pixel();
        let src_w = self.depth.width;
        let src_h = self.depth.height;

        if src_w == target_width && src_h == target_height {
            return self.depth.clone();
        }

        if target_width == 0 || target_height == 0 || src_w == 0 || src_h == 0 {
            return DepthImage {
                data: Vec::new(),
                width: target_width,
                height: target_height,
                pixel_format: self.depth.pixel_format,
            };
        }

        // Read source as f32 for interpolation
        let src_f32 = read_raw_f32(&self.depth);
        let mut dst_f32 = alloc::vec![0.0f32; target_width as usize * target_height as usize];

        for y in 0..target_height {
            for x in 0..target_width {
                // Map target pixel center to source coordinates
                let src_x = (x as f32 + 0.5) * (src_w as f32 / target_width as f32) - 0.5;
                let src_y = (y as f32 + 0.5) * (src_h as f32 / target_height as f32) - 0.5;

                let x0 = src_x.floor().max(0.0) as u32;
                let y0 = src_y.floor().max(0.0) as u32;
                let x1 = (x0 + 1).min(src_w - 1);
                let y1 = (y0 + 1).min(src_h - 1);

                let fx = src_x - src_x.floor().max(0.0);
                let fy = src_y - src_y.floor().max(0.0);

                let v00 = src_f32[(y0 * src_w + x0) as usize];
                let v10 = src_f32[(y0 * src_w + x1) as usize];
                let v01 = src_f32[(y1 * src_w + x0) as usize];
                let v11 = src_f32[(y1 * src_w + x1) as usize];

                let val = bilinear(v00, v10, v01, v11, fx, fy);
                dst_f32[(y * target_width + x) as usize] = val;
            }
        }

        // Write back in the original pixel format
        let data = write_f32_to_format(&dst_f32, self.depth.pixel_format, bpp);

        DepthImage {
            data,
            width: target_width,
            height: target_height,
            pixel_format: self.depth.pixel_format,
        }
    }
}

/// Read depth image pixels as f32 values.
///
/// For integer formats, normalizes to 0.0..1.0.
/// For float formats, returns the raw float values.
fn read_raw_f32(img: &DepthImage) -> Vec<f32> {
    let pixel_count = img.pixel_count() as usize;
    let mut out = alloc::vec![0.0f32; pixel_count];

    match img.pixel_format {
        DepthPixelFormat::Gray8 => {
            for (o, &v) in out.iter_mut().zip(img.data.iter()) {
                *o = v as f32 / 255.0;
            }
        }
        DepthPixelFormat::Gray16 => {
            for (o, chunk) in out.iter_mut().zip(img.data.chunks_exact(2)) {
                let v = u16::from_le_bytes([chunk[0], chunk[1]]);
                *o = v as f32 / 65535.0;
            }
        }
        DepthPixelFormat::Float32 => {
            for (o, chunk) in out.iter_mut().zip(img.data.chunks_exact(4)) {
                *o = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            }
        }
        DepthPixelFormat::Float16 => {
            for (o, chunk) in out.iter_mut().zip(img.data.chunks_exact(2)) {
                let bits = u16::from_le_bytes([chunk[0], chunk[1]]);
                *o = f16_to_f32(bits);
            }
        }
    }

    out
}

/// Convert f32 values back to the target pixel format.
fn write_f32_to_format(values: &[f32], format: DepthPixelFormat, bpp: usize) -> Vec<u8> {
    let mut data = alloc::vec![0u8; values.len() * bpp];

    match format {
        DepthPixelFormat::Gray8 => {
            for (d, &v) in data.iter_mut().zip(values.iter()) {
                *d = (v * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        DepthPixelFormat::Gray16 => {
            for (chunk, &v) in data.chunks_exact_mut(2).zip(values.iter()) {
                let u = (v * 65535.0).round().clamp(0.0, 65535.0) as u16;
                chunk.copy_from_slice(&u.to_le_bytes());
            }
        }
        DepthPixelFormat::Float32 => {
            for (chunk, &v) in data.chunks_exact_mut(4).zip(values.iter()) {
                chunk.copy_from_slice(&v.to_le_bytes());
            }
        }
        DepthPixelFormat::Float16 => {
            for (chunk, &v) in data.chunks_exact_mut(2).zip(values.iter()) {
                chunk.copy_from_slice(&f32_to_f16(v).to_le_bytes());
            }
        }
    }

    data
}

/// Convert IEEE 754 half-precision (f16) to f32.
fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let mant = (h & 0x3FF) as u32;

    if exp == 0 {
        if mant == 0 {
            // +/- zero
            f32::from_bits(sign << 31)
        } else {
            // Denormalized: convert to normalized f32
            let mut m = mant;
            let mut e = 0u32;
            while (m & 0x400) == 0 {
                m <<= 1;
                e += 1;
            }
            m &= 0x3FF;
            let f32_exp = 127 - 15 - e;
            f32::from_bits((sign << 31) | (f32_exp << 23) | (m << 13))
        }
    } else if exp == 31 {
        if mant == 0 {
            // +/- infinity
            f32::from_bits((sign << 31) | (0xFF << 23))
        } else {
            // NaN — preserve some mantissa bits
            f32::from_bits((sign << 31) | (0xFF << 23) | (mant << 13))
        }
    } else {
        // Normalized
        let f32_exp = exp + 127 - 15;
        f32::from_bits((sign << 31) | (f32_exp << 23) | (mant << 13))
    }
}

/// Convert f32 to IEEE 754 half-precision (f16).
fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 31) & 1) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mant = bits & 0x7F_FFFF;

    if exp == 255 {
        // Inf or NaN
        if mant == 0 {
            (sign << 15) | (0x1F << 10)
        } else {
            (sign << 15) | (0x1F << 10) | ((mant >> 13) as u16).max(1)
        }
    } else if exp > 142 {
        // Overflow to infinity (exp - 127 + 15 > 30)
        (sign << 15) | (0x1F << 10)
    } else if exp < 103 {
        // Too small for f16, underflow to zero
        sign << 15
    } else if exp < 113 {
        // Denormalized f16
        let shift = 113 - exp;
        let m = (mant | 0x80_0000) >> (shift + 13);
        (sign << 15) | (m as u16)
    } else {
        // Normalized
        let f16_exp = (exp - 127 + 15) as u16;
        let f16_mant = (mant >> 13) as u16;
        (sign << 15) | (f16_exp << 10) | f16_mant
    }
}

/// Bilinear interpolation.
#[inline(always)]
fn bilinear(v00: f32, v10: f32, v01: f32, v11: f32, fx: f32, fy: f32) -> f32 {
    let top = v00 * (1.0 - fx) + v10 * fx;
    let bottom = v01 * (1.0 - fx) + v11 * fx;
    top * (1.0 - fy) + bottom * fy
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    // =====================================================================
    // DepthImage validation
    // =====================================================================

    #[test]
    fn depth_image_validate_gray8() {
        let img = DepthImage {
            data: vec![128; 4 * 4],
            width: 4,
            height: 4,
            pixel_format: DepthPixelFormat::Gray8,
        };
        assert!(img.validate().is_ok());
    }

    #[test]
    fn depth_image_validate_gray16() {
        let img = DepthImage {
            data: vec![0; 4 * 4 * 2],
            width: 4,
            height: 4,
            pixel_format: DepthPixelFormat::Gray16,
        };
        assert!(img.validate().is_ok());
    }

    #[test]
    fn depth_image_validate_float32() {
        let img = DepthImage {
            data: vec![0; 4 * 4 * 4],
            width: 4,
            height: 4,
            pixel_format: DepthPixelFormat::Float32,
        };
        assert!(img.validate().is_ok());
    }

    #[test]
    fn depth_image_validate_float16() {
        let img = DepthImage {
            data: vec![0; 4 * 4 * 2],
            width: 4,
            height: 4,
            pixel_format: DepthPixelFormat::Float16,
        };
        assert!(img.validate().is_ok());
    }

    #[test]
    fn depth_image_validate_wrong_len() {
        let img = DepthImage {
            data: vec![128; 10],
            width: 4,
            height: 4,
            pixel_format: DepthPixelFormat::Gray8,
        };
        let err = img.validate().unwrap_err();
        assert!(matches!(err, CodecError::InvalidInput(_)));
    }

    #[test]
    fn depth_image_validate_zero_dim() {
        let img = DepthImage {
            data: vec![],
            width: 0,
            height: 4,
            pixel_format: DepthPixelFormat::Gray8,
        };
        let err = img.validate().unwrap_err();
        assert!(matches!(err, CodecError::InvalidInput(_)));
    }

    #[test]
    fn depth_image_pixel_count() {
        let img = DepthImage {
            data: vec![0; 320 * 240],
            width: 320,
            height: 240,
            pixel_format: DepthPixelFormat::Gray8,
        };
        assert_eq!(img.pixel_count(), 76800);
        assert_eq!(img.expected_len(), 76800);
    }

    #[test]
    fn depth_image_expected_len_float32() {
        let img = DepthImage {
            data: vec![0; 10 * 10 * 4],
            width: 10,
            height: 10,
            pixel_format: DepthPixelFormat::Float32,
        };
        assert_eq!(img.expected_len(), 400);
    }

    // =====================================================================
    // DepthMapMetadata construction
    // =====================================================================

    #[test]
    fn metadata_default() {
        let meta = DepthMapMetadata::default();
        assert_eq!(meta.format, DepthFormat::RangeLinear);
        assert_eq!(meta.near, 0.0);
        assert_eq!(meta.far, 1.0);
        assert_eq!(meta.units, DepthUnits::Normalized);
        assert_eq!(meta.measure_type, DepthMeasureType::OpticalAxis);
    }

    #[test]
    fn metadata_all_format_unit_combinations() {
        // Verify all format/unit combos can be constructed
        let formats = [
            DepthFormat::RangeLinear,
            DepthFormat::RangeInverse,
            DepthFormat::Disparity,
            DepthFormat::AbsoluteDepth,
        ];
        let units = [
            DepthUnits::Meters,
            DepthUnits::Millimeters,
            DepthUnits::Diopters,
            DepthUnits::Normalized,
        ];
        for &fmt in &formats {
            for &unit in &units {
                let meta = DepthMapMetadata {
                    format: fmt,
                    near: 0.1,
                    far: 10.0,
                    units: unit,
                    measure_type: DepthMeasureType::OpticRay,
                };
                assert_eq!(meta.format, fmt);
                assert_eq!(meta.units, unit);
            }
        }
    }

    // =====================================================================
    // to_normalized_f32 — RangeLinear
    // =====================================================================

    #[test]
    fn normalized_range_linear_gray8() {
        // For integer formats with RangeLinear, raw 0..255 maps to normalized 0..1 directly.
        // The near/far values describe the physical depth range, but the normalized output
        // is just the raw position between them.
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![0, 128, 255],
                width: 3,
                height: 1,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeLinear,
                near: 0.5,
                far: 10.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let norm = dm.to_normalized_f32();
        assert_eq!(norm.len(), 3);
        // pixel 0: raw=0/255=0.0 => normalized=0.0 (near)
        assert!((norm[0] - 0.0).abs() < 1e-5);
        // pixel 1: raw=128/255=~0.502 => normalized=~0.502
        assert!((norm[1] - 128.0 / 255.0).abs() < 1e-3);
        // pixel 2: raw=255/255=1.0 => normalized=1.0 (far)
        assert!((norm[2] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn normalized_range_linear_gray16() {
        let val_half: u16 = 32768;
        let mut data = Vec::new();
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&val_half.to_le_bytes());
        data.extend_from_slice(&65535u16.to_le_bytes());

        let dm = DecodedDepthMap {
            depth: DepthImage {
                data,
                width: 3,
                height: 1,
                pixel_format: DepthPixelFormat::Gray16,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeLinear,
                near: 0.0,
                far: 1.0,
                units: DepthUnits::Normalized,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let norm = dm.to_normalized_f32();
        assert_eq!(norm.len(), 3);
        assert!((norm[0] - 0.0).abs() < 1e-5);
        assert!((norm[1] - 32768.0 / 65535.0).abs() < 1e-3);
        assert!((norm[2] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn normalized_range_linear_float32() {
        let mut data = Vec::new();
        data.extend_from_slice(&0.5f32.to_le_bytes());
        data.extend_from_slice(&5.0f32.to_le_bytes());
        data.extend_from_slice(&10.0f32.to_le_bytes());

        let dm = DecodedDepthMap {
            depth: DepthImage {
                data,
                width: 3,
                height: 1,
                pixel_format: DepthPixelFormat::Float32,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeLinear,
                near: 0.5,
                far: 10.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let norm = dm.to_normalized_f32();
        assert_eq!(norm.len(), 3);
        assert!((norm[0] - 0.0).abs() < 1e-5); // 0.5 => near
        assert!((norm[1] - (5.0 - 0.5) / 9.5).abs() < 1e-5); // ~0.4737
        assert!((norm[2] - 1.0).abs() < 1e-5); // 10.0 => far
    }

    // =====================================================================
    // to_normalized_f32 — RangeInverse
    // =====================================================================

    #[test]
    fn normalized_range_inverse_gray8() {
        // Integer format (Gray8): raw 0..1 is a normalized position in inverse depth space.
        // near=1.0, far=10.0
        // inv_near = 1/1.0 = 1.0, inv_far = 1/10.0 = 0.1
        // val=0/255: inv_depth = 1.0 + (0.1-1.0)*0 = 1.0, depth = 1/1.0 = 1.0 => norm 0.0
        // val=255/255: inv_depth = 1.0 + (-0.9)*1 = 0.1, depth = 1/0.1 = 10.0 => norm 1.0
        // val=128/255~0.502: inv_depth = 1.0 + (-0.9)*0.502 = 0.548, depth = 1/0.548 = 1.825
        // normalized = (1.825 - 1.0)/9.0 = 0.0917
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![0, 128, 255],
                width: 3,
                height: 1,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeInverse,
                near: 1.0,
                far: 10.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let norm = dm.to_normalized_f32();
        assert_eq!(norm.len(), 3);
        // val=0: depth=1.0 => normalized=0.0
        assert!((norm[0] - 0.0).abs() < 1e-4, "got {}", norm[0]);
        // val=255: depth=10.0 => normalized=1.0
        assert!((norm[2] - 1.0).abs() < 1e-4, "got {}", norm[2]);
        // val=128: midpoint in inverse space
        let mid_raw = 128.0 / 255.0;
        let inv_depth = 1.0 + (0.1 - 1.0) * mid_raw;
        let depth = 1.0 / inv_depth;
        let expected_mid = (depth - 1.0) / 9.0;
        assert!(
            (norm[1] - expected_mid).abs() < 1e-3,
            "got {} expected {}",
            norm[1],
            expected_mid
        );
    }

    #[test]
    fn normalized_range_inverse_float32() {
        // Float format (Float32): raw values are actual inverse depth (1/depth).
        // near=1.0, far=10.0
        // 1/depth = 1.0 => depth = 1.0 (near) => norm 0.0
        // 1/depth = 0.5 => depth = 2.0 => norm (2.0-1.0)/9.0 = 0.111
        // 1/depth = 0.1 => depth = 10.0 (far) => norm 1.0
        let mut data = Vec::new();
        data.extend_from_slice(&1.0f32.to_le_bytes()); // 1/depth for near
        data.extend_from_slice(&0.5f32.to_le_bytes()); // 1/depth for mid
        data.extend_from_slice(&0.1f32.to_le_bytes()); // 1/depth for far

        let dm = DecodedDepthMap {
            depth: DepthImage {
                data,
                width: 3,
                height: 1,
                pixel_format: DepthPixelFormat::Float32,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeInverse,
                near: 1.0,
                far: 10.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let norm = dm.to_normalized_f32();
        assert_eq!(norm.len(), 3);
        // 1/1.0 => depth=1.0 => norm=0.0
        assert!((norm[0] - 0.0).abs() < 1e-4, "got {}", norm[0]);
        // 1/0.1 => depth=10.0 => norm=1.0
        assert!((norm[2] - 1.0).abs() < 1e-4, "got {}", norm[2]);
        // 1/0.5 => depth=2.0 => norm=(2.0-1.0)/9.0 = 0.111
        let expected_mid = (2.0 - 1.0) / 9.0;
        assert!(
            (norm[1] - expected_mid).abs() < 1e-3,
            "got {} expected {}",
            norm[1],
            expected_mid
        );
    }

    // =====================================================================
    // to_normalized_f32 — Disparity
    // =====================================================================

    #[test]
    fn normalized_disparity() {
        // Disparity values: raw = 1/depth.
        // near=0.5m, far=10.0m
        // disparity=2.0 => depth=0.5 => norm=0.0
        // disparity=0.1 => depth=10.0 => norm=1.0
        let mut data = Vec::new();
        data.extend_from_slice(&2.0f32.to_le_bytes()); // near
        data.extend_from_slice(&0.5f32.to_le_bytes()); // mid (depth=2.0)
        data.extend_from_slice(&0.1f32.to_le_bytes()); // far

        let dm = DecodedDepthMap {
            depth: DepthImage {
                data,
                width: 3,
                height: 1,
                pixel_format: DepthPixelFormat::Float32,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::Disparity,
                near: 0.5,
                far: 10.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let norm = dm.to_normalized_f32();
        assert_eq!(norm.len(), 3);
        // disparity=2.0 => depth=0.5 => norm=(0.5-0.5)/9.5=0.0
        assert!((norm[0] - 0.0).abs() < 1e-4, "got {}", norm[0]);
        // disparity=0.5 => depth=2.0 => norm=(2.0-0.5)/9.5=0.1579
        assert!(
            (norm[1] - (2.0 - 0.5) / 9.5).abs() < 1e-4,
            "got {}",
            norm[1]
        );
        // disparity=0.1 => depth=10.0 => norm=(10.0-0.5)/9.5=1.0
        assert!((norm[2] - 1.0).abs() < 1e-4, "got {}", norm[2]);
    }

    // =====================================================================
    // to_meters
    // =====================================================================

    #[test]
    fn to_meters_range_linear() {
        // Gray8 with near=0.5, far=10.0, meters
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![0, 128, 255],
                width: 3,
                height: 1,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeLinear,
                near: 0.5,
                far: 10.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let meters = dm.to_meters().expect("should return Some for Meters");
        assert_eq!(meters.len(), 3);
        // pixel 0: val=0 => depth = 0.5 + 9.5 * 0.0 = 0.5m
        assert!((meters[0] - 0.5).abs() < 1e-3, "got {}", meters[0]);
        // pixel 2: val=1 => depth = 0.5 + 9.5 * 1.0 = 10.0m
        assert!((meters[2] - 10.0).abs() < 1e-3, "got {}", meters[2]);
    }

    #[test]
    fn to_meters_millimeters() {
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![255],
                width: 1,
                height: 1,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeLinear,
                near: 500.0,
                far: 10000.0,
                units: DepthUnits::Millimeters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let meters = dm.to_meters().unwrap();
        // val=1.0 => depth = 10000mm => 10.0m
        assert!((meters[0] - 10.0).abs() < 1e-2, "got {}", meters[0]);
    }

    #[test]
    fn to_meters_normalized_returns_none() {
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![128],
                width: 1,
                height: 1,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeLinear,
                near: 0.0,
                far: 1.0,
                units: DepthUnits::Normalized,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        assert!(dm.to_meters().is_none());
    }

    #[test]
    fn to_meters_disparity() {
        // Disparity value = 2.0 (1/depth) => depth = 0.5m
        let mut data = Vec::new();
        data.extend_from_slice(&2.0f32.to_le_bytes());

        let dm = DecodedDepthMap {
            depth: DepthImage {
                data,
                width: 1,
                height: 1,
                pixel_format: DepthPixelFormat::Float32,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::Disparity,
                near: 0.1,
                far: 100.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let meters = dm.to_meters().unwrap();
        // 1/2.0 = 0.5m
        assert!((meters[0] - 0.5).abs() < 1e-5, "got {}", meters[0]);
    }

    // =====================================================================
    // Edge cases
    // =====================================================================

    #[test]
    fn zero_disparity_yields_far() {
        let mut data = Vec::new();
        data.extend_from_slice(&0.0f32.to_le_bytes());

        let dm = DecodedDepthMap {
            depth: DepthImage {
                data,
                width: 1,
                height: 1,
                pixel_format: DepthPixelFormat::Float32,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::Disparity,
                near: 0.5,
                far: 100.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let norm = dm.to_normalized_f32();
        // disparity=0 => depth=far => normalized=1.0
        assert!((norm[0] - 1.0).abs() < 1e-4, "got {}", norm[0]);
    }

    #[test]
    fn degenerate_range_fills_zero() {
        // near == far => degenerate range
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![128, 200],
                width: 2,
                height: 1,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeLinear,
                near: 5.0,
                far: 5.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let norm = dm.to_normalized_f32();
        assert_eq!(norm, vec![0.0, 0.0]);
    }

    // =====================================================================
    // Float16 handling
    // =====================================================================

    #[test]
    fn f16_roundtrip() {
        let test_values: &[f32] = &[0.0, 1.0, -1.0, 0.5, 65504.0, 0.000061035156];
        for &v in test_values {
            let h = f32_to_f16(v);
            let back = f16_to_f32(h);
            // f16 has limited precision, check within tolerance
            if v.abs() > 0.0 {
                let rel_err = ((back - v) / v).abs();
                assert!(
                    rel_err < 0.01,
                    "f16 roundtrip failed for {v}: got {back}, rel_err={rel_err}"
                );
            } else {
                assert_eq!(back, v);
            }
        }
    }

    #[test]
    fn f16_special_values() {
        // Infinity
        let h = f32_to_f16(f32::INFINITY);
        assert!(f16_to_f32(h).is_infinite() && f16_to_f32(h).is_sign_positive());

        // Negative infinity
        let h = f32_to_f16(f32::NEG_INFINITY);
        assert!(f16_to_f32(h).is_infinite() && f16_to_f32(h).is_sign_negative());

        // NaN
        let h = f32_to_f16(f32::NAN);
        assert!(f16_to_f32(h).is_nan());

        // Zero
        let h = f32_to_f16(0.0);
        assert_eq!(f16_to_f32(h), 0.0);
    }

    #[test]
    fn normalized_float16_depth() {
        // Create a Float16 depth image with known values
        let h_near = f32_to_f16(0.0);
        let h_mid = f32_to_f16(0.5);
        let h_far = f32_to_f16(1.0);

        let mut data = Vec::new();
        data.extend_from_slice(&h_near.to_le_bytes());
        data.extend_from_slice(&h_mid.to_le_bytes());
        data.extend_from_slice(&h_far.to_le_bytes());

        let dm = DecodedDepthMap {
            depth: DepthImage {
                data,
                width: 3,
                height: 1,
                pixel_format: DepthPixelFormat::Float16,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::RangeLinear,
                near: 0.0,
                far: 1.0,
                units: DepthUnits::Normalized,
                measure_type: DepthMeasureType::OpticalAxis,
            },
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let norm = dm.to_normalized_f32();
        assert_eq!(norm.len(), 3);
        assert!((norm[0] - 0.0).abs() < 1e-3);
        assert!((norm[1] - 0.5).abs() < 1e-2);
        assert!((norm[2] - 1.0).abs() < 1e-3);
    }

    // =====================================================================
    // DepthSource detection
    // =====================================================================

    #[test]
    fn depth_source_variants() {
        assert_eq!(DepthSource::default(), DepthSource::Unknown);

        // All variants should be constructible
        let sources = [
            DepthSource::AndroidGDepth,
            DepthSource::AndroidDdf,
            DepthSource::AppleMpf,
            DepthSource::AppleHeic,
            DepthSource::Unknown,
        ];
        for s in &sources {
            let _ = alloc::format!("{s:?}");
        }
    }

    // =====================================================================
    // Resize
    // =====================================================================

    #[test]
    fn resize_noop() {
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![10, 20, 30, 40],
                width: 2,
                height: 2,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata::default(),
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let resized = dm.resize(2, 2);
        assert_eq!(resized.width, 2);
        assert_eq!(resized.height, 2);
        assert_eq!(resized.data, dm.depth.data);
    }

    #[test]
    fn resize_2x2_to_4x4_bilinear() {
        // 2x2 source:
        // [0, 255]
        // [255, 0]
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![0, 255, 255, 0],
                width: 2,
                height: 2,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata::default(),
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let resized = dm.resize(4, 4);
        assert_eq!(resized.width, 4);
        assert_eq!(resized.height, 4);
        assert_eq!(resized.data.len(), 16);
        assert_eq!(resized.pixel_format, DepthPixelFormat::Gray8);

        // Corners should be near the source values (allowing interpolation tolerance)
        // Top-left corner maps near source (0,0) which is 0
        assert!(
            resized.data[0] < 80,
            "top-left should be near 0, got {}",
            resized.data[0]
        );
        // Top-right corner maps near source (1,0) which is 255
        assert!(
            resized.data[3] > 175,
            "top-right should be near 255, got {}",
            resized.data[3]
        );
        // Bottom-left corner maps near source (0,1) which is 255
        assert!(
            resized.data[12] > 175,
            "bottom-left should be near 255, got {}",
            resized.data[12]
        );
        // Bottom-right corner maps near source (1,1) which is 0
        assert!(
            resized.data[15] < 80,
            "bottom-right should be near 0, got {}",
            resized.data[15]
        );
    }

    #[test]
    fn resize_zero_target() {
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![128; 4],
                width: 2,
                height: 2,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata::default(),
            confidence: None,
            source_format: ImageFormat::Jpeg,
            source_device: DepthSource::Unknown,
        };
        let resized = dm.resize(0, 0);
        assert_eq!(resized.width, 0);
        assert_eq!(resized.height, 0);
        assert!(resized.data.is_empty());
    }

    // =====================================================================
    // Constructed DecodedDepthMap
    // =====================================================================

    #[test]
    fn decoded_depth_map_with_confidence() {
        let dm = DecodedDepthMap {
            depth: DepthImage {
                data: vec![128; 4],
                width: 2,
                height: 2,
                pixel_format: DepthPixelFormat::Gray8,
            },
            metadata: DepthMapMetadata {
                format: DepthFormat::AbsoluteDepth,
                near: 0.1,
                far: 50.0,
                units: DepthUnits::Meters,
                measure_type: DepthMeasureType::OpticRay,
            },
            confidence: Some(DepthImage {
                data: vec![255; 4],
                width: 2,
                height: 2,
                pixel_format: DepthPixelFormat::Gray8,
            }),
            source_format: ImageFormat::Heic,
            source_device: DepthSource::AppleHeic,
        };
        assert!(dm.depth.validate().is_ok());
        assert!(dm.confidence.as_ref().unwrap().validate().is_ok());
        assert_eq!(dm.source_format, ImageFormat::Heic);
        assert_eq!(dm.source_device, DepthSource::AppleHeic);
    }
}
