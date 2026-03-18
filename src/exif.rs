//! Lightweight EXIF/TIFF IFD parser.
//!
//! Parses raw EXIF bytes (TIFF-encoded IFD structure) into a structured
//! [`ExifData`] type containing camera metadata, exposure settings, GPS
//! coordinates, and orientation.
//!
//! Handles both JPEG-style EXIF (with `Exif\0\0` prefix) and raw TIFF
//! bytes (PNG, AVIF, HEIC).
//!
//! # Example
//!
//! ```
//! use zencodecs::exif::{parse_exif, ExifData};
//!
//! // Raw TIFF bytes from a decoded image's EXIF metadata
//! # let exif_bytes: &[u8] = &[];
//! if let Ok(exif) = parse_exif(exif_bytes) {
//!     if let Some(make) = &exif.make {
//!         println!("Camera: {make}");
//!     }
//!     if let Some(orientation) = exif.orientation {
//!         println!("Orientation: {orientation}");
//!     }
//! }
//! ```

use alloc::string::String;
use alloc::vec::Vec;

use crate::DecodeOutput;

// =========================================================================
// Public types
// =========================================================================

/// Parsed EXIF metadata from an image.
///
/// Fields are `None` when the corresponding EXIF tag is absent from the data.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ExifData {
    // Camera
    /// Camera manufacturer (tag 0x010F).
    pub make: Option<String>,
    /// Camera model (tag 0x0110).
    pub model: Option<String>,
    /// Software used to create the image (tag 0x0131).
    pub software: Option<String>,
    /// Date/time the file was last modified (tag 0x0132). Format: `"YYYY:MM:DD HH:MM:SS"`.
    pub date_time: Option<String>,
    /// Date/time when the photo was originally taken (tag 0x9003).
    pub date_time_original: Option<String>,

    // Exposure
    /// Exposure time in seconds (tag 0x829A). E.g., 1/500 = `Rational { numerator: 1, denominator: 500 }`.
    pub exposure_time: Option<Rational>,
    /// F-number / aperture (tag 0x829D). E.g., f/2.8 = `Rational { numerator: 28, denominator: 10 }`.
    pub f_number: Option<Rational>,
    /// ISO speed rating (tag 0x8827).
    pub iso: Option<u32>,
    /// Focal length in mm (tag 0x920A).
    pub focal_length: Option<Rational>,
    /// 35mm-equivalent focal length (tag 0xA405).
    pub focal_length_35mm: Option<u16>,
    /// Lens model (tag 0xA434).
    pub lens_model: Option<String>,

    // Orientation
    /// EXIF orientation value 1-8 (tag 0x0112).
    pub orientation: Option<u16>,

    // GPS
    /// GPS latitude.
    pub gps_latitude: Option<GpsCoordinate>,
    /// GPS longitude.
    pub gps_longitude: Option<GpsCoordinate>,
    /// GPS altitude in meters (tag 0x0006). Negative if below sea level.
    pub gps_altitude: Option<f64>,

    // Image
    /// Pixel width from EXIF IFD (tag 0xA002).
    pub width: Option<u32>,
    /// Pixel height from EXIF IFD (tag 0xA003).
    pub height: Option<u32>,
    /// Color space (tag 0xA001). 1=sRGB, 65535=uncalibrated.
    pub color_space: Option<u16>,

    // Flash
    /// Flash status bitfield (tag 0x9209).
    pub flash: Option<u16>,

    // White balance
    /// White balance mode (tag 0xA403). 0=auto, 1=manual.
    pub white_balance: Option<u16>,

    // Metering
    /// Metering mode (tag 0x9207).
    pub metering_mode: Option<u16>,
    /// Exposure program (tag 0x8822).
    pub exposure_program: Option<u16>,
    /// Exposure compensation in EV (tag 0x9204).
    pub exposure_compensation: Option<SRational>,
}

/// Unsigned rational number (two u32 values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rational {
    /// Numerator.
    pub numerator: u32,
    /// Denominator.
    pub denominator: u32,
}

impl Rational {
    /// Create a new rational number.
    pub const fn new(numerator: u32, denominator: u32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    /// Convert to f64. Returns 0.0 if denominator is zero.
    pub fn to_f64(self) -> f64 {
        if self.denominator == 0 {
            0.0
        } else {
            self.numerator as f64 / self.denominator as f64
        }
    }
}

impl core::fmt::Display for Rational {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

/// Signed rational number (two i32 values).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SRational {
    /// Numerator.
    pub numerator: i32,
    /// Denominator.
    pub denominator: i32,
}

impl SRational {
    /// Create a new signed rational number.
    pub const fn new(numerator: i32, denominator: i32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    /// Convert to f64. Returns 0.0 if denominator is zero.
    pub fn to_f64(self) -> f64 {
        if self.denominator == 0 {
            0.0
        } else {
            self.numerator as f64 / self.denominator as f64
        }
    }
}

impl core::fmt::Display for SRational {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

/// GPS coordinate in degrees, minutes, seconds with hemisphere reference.
#[derive(Debug, Clone)]
pub struct GpsCoordinate {
    /// Degrees component.
    pub degrees: f64,
    /// Minutes component.
    pub minutes: f64,
    /// Seconds component.
    pub seconds: f64,
    /// Hemisphere reference: `'N'`/`'S'` for latitude, `'E'`/`'W'` for longitude.
    pub reference: char,
}

impl GpsCoordinate {
    /// Convert to decimal degrees.
    ///
    /// South and West coordinates are returned as negative values.
    pub fn to_decimal(&self) -> f64 {
        let decimal = self.degrees + self.minutes / 60.0 + self.seconds / 3600.0;
        if self.reference == 'S' || self.reference == 'W' {
            -decimal
        } else {
            decimal
        }
    }
}

/// Errors that can occur during EXIF parsing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ExifError {
    /// Data is too short to contain a valid TIFF header.
    TooShort,
    /// Invalid byte order marker (expected `II` or `MM`).
    InvalidByteOrder,
    /// TIFF magic number is not 42 (0x002A).
    InvalidTiffMagic,
    /// An IFD offset points outside the data bounds.
    OffsetOutOfBounds,
    /// An IFD entry references data outside the buffer.
    ValueOutOfBounds,
    /// IFD entry count is unreasonably large (possible corruption).
    TooManyEntries,
}

impl core::fmt::Display for ExifError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort => write!(f, "data too short for TIFF header"),
            Self::InvalidByteOrder => write!(f, "invalid byte order marker"),
            Self::InvalidTiffMagic => write!(f, "invalid TIFF magic number (expected 42)"),
            Self::OffsetOutOfBounds => write!(f, "IFD offset out of bounds"),
            Self::ValueOutOfBounds => write!(f, "IFD value offset out of bounds"),
            Self::TooManyEntries => write!(f, "IFD entry count unreasonably large"),
        }
    }
}

impl core::error::Error for ExifError {}

// =========================================================================
// TIFF type IDs
// =========================================================================

const TYPE_BYTE: u16 = 1;
const TYPE_ASCII: u16 = 2;
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_RATIONAL: u16 = 5;
const TYPE_UNDEFINED: u16 = 7;
const TYPE_SLONG: u16 = 9;
const TYPE_SRATIONAL: u16 = 10;

/// Size of one element for each TIFF type.
fn type_size(type_id: u16) -> Option<u32> {
    match type_id {
        TYPE_BYTE | TYPE_ASCII | TYPE_UNDEFINED => Some(1),
        TYPE_SHORT => Some(2),
        TYPE_LONG | TYPE_SLONG => Some(4),
        TYPE_RATIONAL | TYPE_SRATIONAL => Some(8),
        _ => None,
    }
}

// =========================================================================
// EXIF tag constants
// =========================================================================

// IFD0 tags
const TAG_MAKE: u16 = 0x010F;
const TAG_MODEL: u16 = 0x0110;
const TAG_ORIENTATION: u16 = 0x0112;
const TAG_SOFTWARE: u16 = 0x0131;
const TAG_DATE_TIME: u16 = 0x0132;
const TAG_EXIF_IFD_POINTER: u16 = 0x8769;
const TAG_GPS_IFD_POINTER: u16 = 0x8825;

// EXIF IFD tags
const TAG_EXPOSURE_TIME: u16 = 0x829A;
const TAG_F_NUMBER: u16 = 0x829D;
const TAG_EXPOSURE_PROGRAM: u16 = 0x8822;
const TAG_ISO_SPEED: u16 = 0x8827;
const TAG_DATE_TIME_ORIGINAL: u16 = 0x9003;
const TAG_EXPOSURE_BIAS: u16 = 0x9204;
const TAG_METERING_MODE: u16 = 0x9207;
const TAG_FLASH: u16 = 0x9209;
const TAG_FOCAL_LENGTH: u16 = 0x920A;
const TAG_COLOR_SPACE: u16 = 0xA001;
const TAG_PIXEL_X_DIMENSION: u16 = 0xA002;
const TAG_PIXEL_Y_DIMENSION: u16 = 0xA003;
const TAG_WHITE_BALANCE: u16 = 0xA403;
const TAG_FOCAL_LENGTH_35MM: u16 = 0xA405;
const TAG_LENS_MODEL: u16 = 0xA434;

// GPS IFD tags
const TAG_GPS_LATITUDE_REF: u16 = 0x0001;
const TAG_GPS_LATITUDE: u16 = 0x0002;
const TAG_GPS_LONGITUDE_REF: u16 = 0x0003;
const TAG_GPS_LONGITUDE: u16 = 0x0004;
const TAG_GPS_ALTITUDE_REF: u16 = 0x0005;
const TAG_GPS_ALTITUDE: u16 = 0x0006;

/// Maximum number of IFD entries we will parse per directory.
/// Protects against corrupt data claiming millions of entries.
const MAX_IFD_ENTRIES: u16 = 1000;

// =========================================================================
// Byte-order reader
// =========================================================================

/// Reads values from a byte slice with configurable endianness.
#[derive(Clone, Copy)]
struct Reader<'a> {
    data: &'a [u8],
    little_endian: bool,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8], little_endian: bool) -> Self {
        Self {
            data,
            little_endian,
        }
    }

    fn u16_at(&self, offset: usize) -> Option<u16> {
        let bytes = self.data.get(offset..offset + 2)?;
        Some(if self.little_endian {
            u16::from_le_bytes([bytes[0], bytes[1]])
        } else {
            u16::from_be_bytes([bytes[0], bytes[1]])
        })
    }

    fn u32_at(&self, offset: usize) -> Option<u32> {
        let bytes = self.data.get(offset..offset + 4)?;
        Some(if self.little_endian {
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        } else {
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        })
    }

    fn i32_at(&self, offset: usize) -> Option<i32> {
        let bytes = self.data.get(offset..offset + 4)?;
        Some(if self.little_endian {
            i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        } else {
            i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        })
    }

    fn len(&self) -> usize {
        self.data.len()
    }
}

// =========================================================================
// IFD entry
// =========================================================================

/// A single IFD entry (12 bytes).
struct IfdEntry {
    tag: u16,
    type_id: u16,
    count: u32,
    /// Raw 4-byte value/offset field.
    value_offset: u32,
}

impl IfdEntry {
    /// Parse an IFD entry at the given offset.
    fn parse(reader: &Reader<'_>, offset: usize) -> Option<Self> {
        let tag = reader.u16_at(offset)?;
        let type_id = reader.u16_at(offset + 2)?;
        let count = reader.u32_at(offset + 4)?;
        let value_offset = reader.u32_at(offset + 8)?;
        Some(Self {
            tag,
            type_id,
            count,
            value_offset,
        })
    }

    /// Total byte size of the value data.
    fn value_size(&self) -> Option<u32> {
        type_size(self.type_id)?.checked_mul(self.count)
    }

    /// Returns the offset into the TIFF data where the value bytes live.
    /// If the value fits in 4 bytes, it's stored inline in the value_offset field.
    fn data_offset(&self, entry_offset: usize) -> Option<usize> {
        let size = self.value_size()?;
        if size <= 4 {
            // Value is stored inline at the value_offset field position
            // (bytes 8-11 of the 12-byte IFD entry).
            Some(entry_offset + 8)
        } else {
            Some(self.value_offset as usize)
        }
    }
}

// =========================================================================
// Value extraction helpers
// =========================================================================

/// Read a u16 value from an IFD entry (SHORT type, count=1).
fn read_short(reader: &Reader<'_>, entry: &IfdEntry, entry_offset: usize) -> Option<u16> {
    if entry.type_id != TYPE_SHORT || entry.count < 1 {
        return None;
    }
    let off = entry.data_offset(entry_offset)?;
    reader.u16_at(off)
}

/// Read a u32 value from an IFD entry (LONG or SHORT).
fn read_long_or_short(reader: &Reader<'_>, entry: &IfdEntry, entry_offset: usize) -> Option<u32> {
    match entry.type_id {
        TYPE_LONG if entry.count >= 1 => {
            let off = entry.data_offset(entry_offset)?;
            reader.u32_at(off)
        }
        TYPE_SHORT if entry.count >= 1 => {
            let off = entry.data_offset(entry_offset)?;
            reader.u16_at(off).map(u32::from)
        }
        _ => None,
    }
}

/// Read an ASCII string from an IFD entry.
fn read_ascii(reader: &Reader<'_>, entry: &IfdEntry, entry_offset: usize) -> Option<String> {
    if entry.type_id != TYPE_ASCII || entry.count == 0 {
        return None;
    }
    let off = entry.data_offset(entry_offset)?;
    let count = entry.count as usize;
    let end = off.checked_add(count)?;
    if end > reader.len() {
        return None;
    }
    let bytes = &reader.data[off..end];
    // Strip trailing NUL(s)
    let trimmed = match bytes.iter().position(|&b| b == 0) {
        Some(pos) => &bytes[..pos],
        None => bytes,
    };
    String::from_utf8(trimmed.to_vec()).ok()
}

/// Read a RATIONAL value from an IFD entry.
fn read_rational(reader: &Reader<'_>, entry: &IfdEntry, entry_offset: usize) -> Option<Rational> {
    if entry.type_id != TYPE_RATIONAL || entry.count < 1 {
        return None;
    }
    let off = entry.data_offset(entry_offset)?;
    let num = reader.u32_at(off)?;
    let den = reader.u32_at(off + 4)?;
    Some(Rational::new(num, den))
}

/// Read a SRATIONAL value from an IFD entry.
fn read_srational(reader: &Reader<'_>, entry: &IfdEntry, entry_offset: usize) -> Option<SRational> {
    if entry.type_id != TYPE_SRATIONAL || entry.count < 1 {
        return None;
    }
    let off = entry.data_offset(entry_offset)?;
    let num = reader.i32_at(off)?;
    let den = reader.i32_at(off + 4)?;
    Some(SRational::new(num, den))
}

/// Read 3 RATIONAL values for GPS coordinates (degrees, minutes, seconds).
fn read_gps_rationals(
    reader: &Reader<'_>,
    entry: &IfdEntry,
    entry_offset: usize,
) -> Option<(f64, f64, f64)> {
    if entry.type_id != TYPE_RATIONAL || entry.count < 3 {
        return None;
    }
    let off = entry.data_offset(entry_offset)?;
    let deg_n = reader.u32_at(off)? as f64;
    let deg_d = reader.u32_at(off + 4)? as f64;
    let min_n = reader.u32_at(off + 8)? as f64;
    let min_d = reader.u32_at(off + 12)? as f64;
    let sec_n = reader.u32_at(off + 16)? as f64;
    let sec_d = reader.u32_at(off + 20)? as f64;

    let degrees = if deg_d != 0.0 { deg_n / deg_d } else { 0.0 };
    let minutes = if min_d != 0.0 { min_n / min_d } else { 0.0 };
    let seconds = if sec_d != 0.0 { sec_n / sec_d } else { 0.0 };
    Some((degrees, minutes, seconds))
}

/// Read a single-character ASCII reference from an IFD entry.
fn read_ascii_char(reader: &Reader<'_>, entry: &IfdEntry, entry_offset: usize) -> Option<char> {
    if entry.type_id != TYPE_ASCII || entry.count < 1 {
        return None;
    }
    let off = entry.data_offset(entry_offset)?;
    let b = *reader.data.get(off)?;
    if b.is_ascii_alphabetic() {
        Some(b as char)
    } else {
        None
    }
}

/// Read a BYTE value from an IFD entry.
fn read_byte(reader: &Reader<'_>, entry: &IfdEntry, entry_offset: usize) -> Option<u8> {
    if entry.type_id != TYPE_BYTE || entry.count < 1 {
        return None;
    }
    let off = entry.data_offset(entry_offset)?;
    reader.data.get(off).copied()
}

// =========================================================================
// IFD walking
// =========================================================================

/// Parsed IFD entries with their offsets for value extraction.
struct ParsedIfd {
    entries: Vec<(IfdEntry, usize)>,
}

/// Parse an IFD at the given offset, returning entries and their offsets.
fn parse_ifd(reader: &Reader<'_>, ifd_offset: usize) -> Result<ParsedIfd, ExifError> {
    if ifd_offset + 2 > reader.len() {
        return Err(ExifError::OffsetOutOfBounds);
    }
    let count = reader
        .u16_at(ifd_offset)
        .ok_or(ExifError::OffsetOutOfBounds)?;
    if count > MAX_IFD_ENTRIES {
        return Err(ExifError::TooManyEntries);
    }

    let mut entries = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let entry_offset = ifd_offset + 2 + i * 12;
        if entry_offset + 12 > reader.len() {
            // Truncated — return what we have so far.
            break;
        }
        if let Some(entry) = IfdEntry::parse(reader, entry_offset) {
            // Validate that the value data is within bounds.
            if let Some(size) = entry.value_size()
                && size > 4
            {
                let end = (entry.value_offset as usize).saturating_add(size as usize);
                if end > reader.len() {
                    // Skip this entry but continue parsing others.
                    continue;
                }
            }
            entries.push((entry, entry_offset));
        }
    }
    Ok(ParsedIfd { entries })
}

/// Find an entry by tag in a parsed IFD.
fn find_entry(ifd: &ParsedIfd, tag: u16) -> Option<&(IfdEntry, usize)> {
    ifd.entries.iter().find(|(e, _)| e.tag == tag)
}

// =========================================================================
// GPS parsing
// =========================================================================

/// Parse GPS data from the GPS IFD.
fn parse_gps_ifd(reader: &Reader<'_>, ifd: &ParsedIfd, exif: &mut ExifData) {
    // Latitude
    let lat_ref =
        find_entry(ifd, TAG_GPS_LATITUDE_REF).and_then(|(e, o)| read_ascii_char(reader, e, *o));
    let lat_dms =
        find_entry(ifd, TAG_GPS_LATITUDE).and_then(|(e, o)| read_gps_rationals(reader, e, *o));
    if let (Some(reference), Some((degrees, minutes, seconds))) = (lat_ref, lat_dms) {
        exif.gps_latitude = Some(GpsCoordinate {
            degrees,
            minutes,
            seconds,
            reference,
        });
    }

    // Longitude
    let lon_ref =
        find_entry(ifd, TAG_GPS_LONGITUDE_REF).and_then(|(e, o)| read_ascii_char(reader, e, *o));
    let lon_dms =
        find_entry(ifd, TAG_GPS_LONGITUDE).and_then(|(e, o)| read_gps_rationals(reader, e, *o));
    if let (Some(reference), Some((degrees, minutes, seconds))) = (lon_ref, lon_dms) {
        exif.gps_longitude = Some(GpsCoordinate {
            degrees,
            minutes,
            seconds,
            reference,
        });
    }

    // Altitude
    let alt_ref = find_entry(ifd, TAG_GPS_ALTITUDE_REF).and_then(|(e, o)| read_byte(reader, e, *o));
    let alt_val = find_entry(ifd, TAG_GPS_ALTITUDE).and_then(|(e, o)| read_rational(reader, e, *o));
    if let Some(alt) = alt_val {
        let meters = alt.to_f64();
        exif.gps_altitude = Some(if alt_ref == Some(1) {
            -meters // Below sea level
        } else {
            meters
        });
    }
}

// =========================================================================
// Main parse function
// =========================================================================

/// Parse EXIF data from raw bytes.
///
/// Input is the EXIF payload. For JPEG, this is after the APP1 marker and
/// length — it may include the `Exif\0\0` prefix (which is stripped
/// automatically). For PNG/AVIF/HEIC, pass the raw TIFF bytes directly.
///
/// Returns structured EXIF data with all recognized fields populated.
/// Unrecognized or malformed tags are silently skipped.
pub fn parse_exif(data: &[u8]) -> Result<ExifData, ExifError> {
    // Strip JPEG-style "Exif\0\0" prefix if present.
    let data = if data.len() >= 6 && &data[..6] == b"Exif\0\0" {
        &data[6..]
    } else {
        data
    };

    // Need at least 8 bytes for the TIFF header.
    if data.len() < 8 {
        return Err(ExifError::TooShort);
    }

    // Parse byte order.
    let little_endian = match &data[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err(ExifError::InvalidByteOrder),
    };

    let reader = Reader::new(data, little_endian);

    // Check TIFF magic number (42).
    let magic = reader.u16_at(2).ok_or(ExifError::TooShort)?;
    if magic != 42 {
        return Err(ExifError::InvalidTiffMagic);
    }

    // Get IFD0 offset.
    let ifd0_offset = reader.u32_at(4).ok_or(ExifError::TooShort)? as usize;
    if ifd0_offset >= data.len() {
        return Err(ExifError::OffsetOutOfBounds);
    }

    let mut exif = ExifData::default();

    // Parse IFD0.
    let ifd0 = parse_ifd(&reader, ifd0_offset)?;
    parse_ifd0_tags(&reader, &ifd0, &mut exif);

    // Follow EXIF IFD pointer.
    if let Some((entry, offset)) = find_entry(&ifd0, TAG_EXIF_IFD_POINTER)
        && let Some(exif_ifd_offset) = read_long_or_short(&reader, entry, *offset)
    {
        let exif_ifd_offset = exif_ifd_offset as usize;
        if exif_ifd_offset < data.len()
            && let Ok(exif_ifd) = parse_ifd(&reader, exif_ifd_offset)
        {
            parse_exif_ifd_tags(&reader, &exif_ifd, &mut exif);
        }
    }

    // Follow GPS IFD pointer.
    if let Some((entry, offset)) = find_entry(&ifd0, TAG_GPS_IFD_POINTER)
        && let Some(gps_ifd_offset) = read_long_or_short(&reader, entry, *offset)
    {
        let gps_ifd_offset = gps_ifd_offset as usize;
        if gps_ifd_offset < data.len()
            && let Ok(gps_ifd) = parse_ifd(&reader, gps_ifd_offset)
        {
            parse_gps_ifd(&reader, &gps_ifd, &mut exif);
        }
    }

    Ok(exif)
}

/// Parse IFD0 tags into ExifData.
fn parse_ifd0_tags(reader: &Reader<'_>, ifd: &ParsedIfd, exif: &mut ExifData) {
    for (entry, offset) in &ifd.entries {
        match entry.tag {
            TAG_MAKE => exif.make = read_ascii(reader, entry, *offset),
            TAG_MODEL => exif.model = read_ascii(reader, entry, *offset),
            TAG_ORIENTATION => exif.orientation = read_short(reader, entry, *offset),
            TAG_SOFTWARE => exif.software = read_ascii(reader, entry, *offset),
            TAG_DATE_TIME => exif.date_time = read_ascii(reader, entry, *offset),
            _ => {}
        }
    }
}

/// Parse EXIF IFD tags into ExifData.
fn parse_exif_ifd_tags(reader: &Reader<'_>, ifd: &ParsedIfd, exif: &mut ExifData) {
    for (entry, offset) in &ifd.entries {
        match entry.tag {
            TAG_EXPOSURE_TIME => exif.exposure_time = read_rational(reader, entry, *offset),
            TAG_F_NUMBER => exif.f_number = read_rational(reader, entry, *offset),
            TAG_EXPOSURE_PROGRAM => {
                exif.exposure_program = read_short(reader, entry, *offset);
            }
            TAG_ISO_SPEED => {
                exif.iso = read_long_or_short(reader, entry, *offset);
            }
            TAG_DATE_TIME_ORIGINAL => {
                exif.date_time_original = read_ascii(reader, entry, *offset);
            }
            TAG_EXPOSURE_BIAS => {
                exif.exposure_compensation = read_srational(reader, entry, *offset);
            }
            TAG_METERING_MODE => exif.metering_mode = read_short(reader, entry, *offset),
            TAG_FLASH => exif.flash = read_short(reader, entry, *offset),
            TAG_FOCAL_LENGTH => exif.focal_length = read_rational(reader, entry, *offset),
            TAG_COLOR_SPACE => exif.color_space = read_short(reader, entry, *offset),
            TAG_PIXEL_X_DIMENSION => {
                exif.width = read_long_or_short(reader, entry, *offset);
            }
            TAG_PIXEL_Y_DIMENSION => {
                exif.height = read_long_or_short(reader, entry, *offset);
            }
            TAG_WHITE_BALANCE => exif.white_balance = read_short(reader, entry, *offset),
            TAG_FOCAL_LENGTH_35MM => {
                exif.focal_length_35mm = read_short(reader, entry, *offset);
            }
            TAG_LENS_MODEL => exif.lens_model = read_ascii(reader, entry, *offset),
            _ => {}
        }
    }
}

/// Parse EXIF metadata from a [`DecodeOutput`].
///
/// Extracts the raw EXIF bytes from the decode output's embedded metadata
/// and parses them into structured [`ExifData`].
///
/// Returns `None` if no EXIF data is present, or if parsing fails.
pub fn parse_exif_from_output(output: &DecodeOutput) -> Option<ExifData> {
    let exif_bytes = output.info().embedded_metadata.exif.as_deref()?;
    parse_exif(exif_bytes).ok()
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =====================================================================
    // Helper: build a minimal TIFF structure
    // =====================================================================

    /// Build a minimal TIFF file with IFD0 entries.
    /// Returns the raw TIFF bytes.
    fn build_tiff(little_endian: bool, entries: &[(u16, u16, u32, &[u8])]) -> Vec<u8> {
        // entries: (tag, type, count, raw_value_or_data)
        // If value fits in 4 bytes, it's inline. Otherwise we append data.

        let num_entries = entries.len() as u16;
        let ifd_offset: u32 = 8; // right after header
        let ifd_size = 2 + entries.len() * 12 + 4; // count + entries + next_ifd
        let mut data_offset = (ifd_offset as usize) + ifd_size;

        let mut buf = Vec::new();
        let mut extra_data: Vec<(usize, Vec<u8>)> = Vec::new();

        // TIFF header
        if little_endian {
            buf.extend_from_slice(b"II");
        } else {
            buf.extend_from_slice(b"MM");
        }
        push_u16(&mut buf, 42, little_endian);
        push_u32(&mut buf, ifd_offset, little_endian);

        // IFD entry count
        push_u16(&mut buf, num_entries, little_endian);

        // IFD entries
        for &(tag, type_id, count, value_data) in entries {
            push_u16(&mut buf, tag, little_endian);
            push_u16(&mut buf, type_id, little_endian);
            push_u32(&mut buf, count, little_endian);

            let elem_size = type_size(type_id).unwrap_or(1);
            let total_size = elem_size * count;

            if total_size <= 4 {
                // Inline: pad to 4 bytes
                let mut inline = [0u8; 4];
                let copy_len = value_data.len().min(4);
                inline[..copy_len].copy_from_slice(&value_data[..copy_len]);
                buf.extend_from_slice(&inline);
            } else {
                // Offset
                push_u32(&mut buf, data_offset as u32, little_endian);
                extra_data.push((data_offset, value_data.to_vec()));
                data_offset += value_data.len();
            }
        }

        // Next IFD offset (0 = no more IFDs)
        push_u32(&mut buf, 0, little_endian);

        // Append extra data
        for (offset, data) in &extra_data {
            // Pad to the expected offset
            while buf.len() < *offset {
                buf.push(0);
            }
            buf.extend_from_slice(data);
        }

        buf
    }

    /// Build a TIFF with IFD0 pointing to an EXIF sub-IFD.
    fn build_tiff_with_exif_ifd(
        little_endian: bool,
        ifd0_entries: &[(u16, u16, u32, &[u8])],
        exif_entries: &[(u16, u16, u32, &[u8])],
    ) -> Vec<u8> {
        // We'll build manually: header + IFD0 + EXIF IFD, with data appended.
        let ifd0_offset: usize = 8;
        let ifd0_count = (ifd0_entries.len() + 1) as u16; // +1 for EXIF IFD pointer
        let ifd0_size = 2 + (ifd0_count as usize) * 12 + 4;
        let exif_ifd_start = ifd0_offset + ifd0_size;
        let exif_count = exif_entries.len() as u16;
        let exif_ifd_size = 2 + (exif_count as usize) * 12 + 4;
        let mut data_area_offset = exif_ifd_start + exif_ifd_size;

        let mut buf = Vec::new();
        let mut pending_data: Vec<(usize, Vec<u8>)> = Vec::new();

        // Header
        if little_endian {
            buf.extend_from_slice(b"II");
        } else {
            buf.extend_from_slice(b"MM");
        }
        push_u16(&mut buf, 42, little_endian);
        push_u32(&mut buf, ifd0_offset as u32, little_endian);

        // IFD0
        push_u16(&mut buf, ifd0_count, little_endian);

        // IFD0 entries (user-provided)
        for &(tag, type_id, count, value_data) in ifd0_entries {
            push_u16(&mut buf, tag, little_endian);
            push_u16(&mut buf, type_id, little_endian);
            push_u32(&mut buf, count, little_endian);
            let elem_size = type_size(type_id).unwrap_or(1);
            let total_size = elem_size * count;
            if total_size <= 4 {
                let mut inline = [0u8; 4];
                let copy_len = value_data.len().min(4);
                inline[..copy_len].copy_from_slice(&value_data[..copy_len]);
                buf.extend_from_slice(&inline);
            } else {
                push_u32(&mut buf, data_area_offset as u32, little_endian);
                pending_data.push((data_area_offset, value_data.to_vec()));
                data_area_offset += value_data.len();
            }
        }

        // EXIF IFD pointer entry
        push_u16(&mut buf, TAG_EXIF_IFD_POINTER, little_endian);
        push_u16(&mut buf, TYPE_LONG, little_endian);
        push_u32(&mut buf, 1, little_endian);
        push_u32(&mut buf, exif_ifd_start as u32, little_endian);

        // Next IFD = 0
        push_u32(&mut buf, 0, little_endian);

        // EXIF IFD
        push_u16(&mut buf, exif_count, little_endian);
        for &(tag, type_id, count, value_data) in exif_entries {
            push_u16(&mut buf, tag, little_endian);
            push_u16(&mut buf, type_id, little_endian);
            push_u32(&mut buf, count, little_endian);
            let elem_size = type_size(type_id).unwrap_or(1);
            let total_size = elem_size * count;
            if total_size <= 4 {
                let mut inline = [0u8; 4];
                let copy_len = value_data.len().min(4);
                inline[..copy_len].copy_from_slice(&value_data[..copy_len]);
                buf.extend_from_slice(&inline);
            } else {
                push_u32(&mut buf, data_area_offset as u32, little_endian);
                pending_data.push((data_area_offset, value_data.to_vec()));
                data_area_offset += value_data.len();
            }
        }
        // Next IFD = 0
        push_u32(&mut buf, 0, little_endian);

        // Append pending data
        for (offset, data) in &pending_data {
            while buf.len() < *offset {
                buf.push(0);
            }
            buf.extend_from_slice(data);
        }

        buf
    }

    /// Build a TIFF with IFD0, EXIF IFD, and GPS IFD.
    fn build_tiff_with_gps(
        little_endian: bool,
        ifd0_entries: &[(u16, u16, u32, &[u8])],
        exif_entries: &[(u16, u16, u32, &[u8])],
        gps_entries: &[(u16, u16, u32, &[u8])],
    ) -> Vec<u8> {
        let ifd0_offset: usize = 8;
        // +2 for EXIF IFD pointer and GPS IFD pointer
        let ifd0_count = (ifd0_entries.len() + 2) as u16;
        let ifd0_size = 2 + (ifd0_count as usize) * 12 + 4;
        let exif_ifd_start = ifd0_offset + ifd0_size;
        let exif_count = exif_entries.len() as u16;
        let exif_ifd_size = 2 + (exif_count as usize) * 12 + 4;
        let gps_ifd_start = exif_ifd_start + exif_ifd_size;
        let gps_count = gps_entries.len() as u16;
        let gps_ifd_size = 2 + (gps_count as usize) * 12 + 4;
        let mut data_area_offset = gps_ifd_start + gps_ifd_size;

        let mut buf = Vec::new();
        let mut pending_data: Vec<(usize, Vec<u8>)> = Vec::new();

        // Header
        if little_endian {
            buf.extend_from_slice(b"II");
        } else {
            buf.extend_from_slice(b"MM");
        }
        push_u16(&mut buf, 42, little_endian);
        push_u32(&mut buf, ifd0_offset as u32, little_endian);

        // ---- IFD0 ----
        push_u16(&mut buf, ifd0_count, little_endian);
        for &(tag, type_id, count, value_data) in ifd0_entries {
            write_ifd_entry(
                &mut buf,
                tag,
                type_id,
                count,
                value_data,
                little_endian,
                &mut data_area_offset,
                &mut pending_data,
            );
        }
        // EXIF IFD pointer
        push_u16(&mut buf, TAG_EXIF_IFD_POINTER, little_endian);
        push_u16(&mut buf, TYPE_LONG, little_endian);
        push_u32(&mut buf, 1, little_endian);
        push_u32(&mut buf, exif_ifd_start as u32, little_endian);
        // GPS IFD pointer
        push_u16(&mut buf, TAG_GPS_IFD_POINTER, little_endian);
        push_u16(&mut buf, TYPE_LONG, little_endian);
        push_u32(&mut buf, 1, little_endian);
        push_u32(&mut buf, gps_ifd_start as u32, little_endian);
        // Next IFD = 0
        push_u32(&mut buf, 0, little_endian);

        // ---- EXIF IFD ----
        push_u16(&mut buf, exif_count, little_endian);
        for &(tag, type_id, count, value_data) in exif_entries {
            write_ifd_entry(
                &mut buf,
                tag,
                type_id,
                count,
                value_data,
                little_endian,
                &mut data_area_offset,
                &mut pending_data,
            );
        }
        push_u32(&mut buf, 0, little_endian);

        // ---- GPS IFD ----
        push_u16(&mut buf, gps_count, little_endian);
        for &(tag, type_id, count, value_data) in gps_entries {
            write_ifd_entry(
                &mut buf,
                tag,
                type_id,
                count,
                value_data,
                little_endian,
                &mut data_area_offset,
                &mut pending_data,
            );
        }
        push_u32(&mut buf, 0, little_endian);

        // ---- Extra data ----
        for (offset, data) in &pending_data {
            while buf.len() < *offset {
                buf.push(0);
            }
            buf.extend_from_slice(data);
        }

        buf
    }

    #[allow(clippy::too_many_arguments)]
    fn write_ifd_entry(
        buf: &mut Vec<u8>,
        tag: u16,
        type_id: u16,
        count: u32,
        value_data: &[u8],
        little_endian: bool,
        data_area_offset: &mut usize,
        pending_data: &mut Vec<(usize, Vec<u8>)>,
    ) {
        push_u16(buf, tag, little_endian);
        push_u16(buf, type_id, little_endian);
        push_u32(buf, count, little_endian);
        let elem_size = type_size(type_id).unwrap_or(1);
        let total_size = elem_size * count;
        if total_size <= 4 {
            let mut inline = [0u8; 4];
            let copy_len = value_data.len().min(4);
            inline[..copy_len].copy_from_slice(&value_data[..copy_len]);
            buf.extend_from_slice(&inline);
        } else {
            push_u32(buf, *data_area_offset as u32, little_endian);
            pending_data.push((*data_area_offset, value_data.to_vec()));
            *data_area_offset += value_data.len();
        }
    }

    fn push_u16(buf: &mut Vec<u8>, val: u16, little_endian: bool) {
        if little_endian {
            buf.extend_from_slice(&val.to_le_bytes());
        } else {
            buf.extend_from_slice(&val.to_be_bytes());
        }
    }

    fn push_u32(buf: &mut Vec<u8>, val: u32, little_endian: bool) {
        if little_endian {
            buf.extend_from_slice(&val.to_le_bytes());
        } else {
            buf.extend_from_slice(&val.to_be_bytes());
        }
    }

    fn push_i32(buf: &mut Vec<u8>, val: i32, little_endian: bool) {
        if little_endian {
            buf.extend_from_slice(&val.to_le_bytes());
        } else {
            buf.extend_from_slice(&val.to_be_bytes());
        }
    }

    fn make_rational_bytes(num: u32, den: u32, little_endian: bool) -> Vec<u8> {
        let mut v = Vec::new();
        push_u32(&mut v, num, little_endian);
        push_u32(&mut v, den, little_endian);
        v
    }

    fn make_srational_bytes(num: i32, den: i32, little_endian: bool) -> Vec<u8> {
        let mut v = Vec::new();
        push_i32(&mut v, num, little_endian);
        push_i32(&mut v, den, little_endian);
        v
    }

    fn make_gps_dms_bytes(
        deg_n: u32,
        deg_d: u32,
        min_n: u32,
        min_d: u32,
        sec_n: u32,
        sec_d: u32,
        little_endian: bool,
    ) -> Vec<u8> {
        let mut v = Vec::new();
        push_u32(&mut v, deg_n, little_endian);
        push_u32(&mut v, deg_d, little_endian);
        push_u32(&mut v, min_n, little_endian);
        push_u32(&mut v, min_d, little_endian);
        push_u32(&mut v, sec_n, little_endian);
        push_u32(&mut v, sec_d, little_endian);
        v
    }

    fn make_short_bytes(val: u16, little_endian: bool) -> Vec<u8> {
        if little_endian {
            val.to_le_bytes().to_vec()
        } else {
            val.to_be_bytes().to_vec()
        }
    }

    // =====================================================================
    // 1. Byte order: Little-endian (II) and big-endian (MM)
    // =====================================================================

    #[test]
    fn byte_order_little_endian() {
        let le = true;
        let tiff = build_tiff(
            le,
            &[(TAG_ORIENTATION, TYPE_SHORT, 1, &make_short_bytes(6, le))],
        );
        let exif = parse_exif(&tiff).unwrap();
        assert_eq!(exif.orientation, Some(6));
    }

    #[test]
    fn byte_order_big_endian() {
        let le = false;
        let tiff = build_tiff(
            le,
            &[(TAG_ORIENTATION, TYPE_SHORT, 1, &make_short_bytes(3, le))],
        );
        let exif = parse_exif(&tiff).unwrap();
        assert_eq!(exif.orientation, Some(3));
    }

    // =====================================================================
    // 2. IFD entry types: BYTE, ASCII, SHORT, LONG, RATIONAL, SRATIONAL
    // =====================================================================

    #[test]
    fn ifd_type_ascii() {
        let le = true;
        let tiff = build_tiff(le, &[(TAG_MAKE, TYPE_ASCII, 6, b"Nikon\0")]);
        let exif = parse_exif(&tiff).unwrap();
        assert_eq!(exif.make.as_deref(), Some("Nikon"));
    }

    #[test]
    fn ifd_type_short() {
        let le = true;
        let tiff = build_tiff(
            le,
            &[(TAG_ORIENTATION, TYPE_SHORT, 1, &make_short_bytes(8, le))],
        );
        let exif = parse_exif(&tiff).unwrap();
        assert_eq!(exif.orientation, Some(8));
    }

    #[test]
    fn ifd_type_rational() {
        let le = true;
        let rat_bytes = make_rational_bytes(1, 500, le);
        let tiff = build_tiff_with_exif_ifd(
            le,
            &[],
            &[(TAG_EXPOSURE_TIME, TYPE_RATIONAL, 1, &rat_bytes)],
        );
        let exif = parse_exif(&tiff).unwrap();
        let et = exif.exposure_time.unwrap();
        assert_eq!(et.numerator, 1);
        assert_eq!(et.denominator, 500);
    }

    #[test]
    fn ifd_type_srational() {
        let le = true;
        let srat_bytes = make_srational_bytes(-1, 3, le);
        let tiff = build_tiff_with_exif_ifd(
            le,
            &[],
            &[(TAG_EXPOSURE_BIAS, TYPE_SRATIONAL, 1, &srat_bytes)],
        );
        let exif = parse_exif(&tiff).unwrap();
        let ec = exif.exposure_compensation.unwrap();
        assert_eq!(ec.numerator, -1);
        assert_eq!(ec.denominator, 3);
    }

    // =====================================================================
    // 3. Camera metadata: full set from a "known camera"
    // =====================================================================

    #[test]
    fn camera_metadata() {
        let le = true;
        let tiff = build_tiff_with_exif_ifd(
            le,
            &[
                (TAG_MAKE, TYPE_ASCII, 6, b"Canon\0"),
                (TAG_MODEL, TYPE_ASCII, 14, b"EOS R5 Mark I\0"),
                (TAG_SOFTWARE, TYPE_ASCII, 11, b"Lightroom\0\0"),
                (TAG_DATE_TIME, TYPE_ASCII, 20, b"2025:06:15 14:30:00\0"),
            ],
            &[
                (
                    TAG_DATE_TIME_ORIGINAL,
                    TYPE_ASCII,
                    20,
                    b"2025:06:15 14:25:00\0",
                ),
                (TAG_LENS_MODEL, TYPE_ASCII, 15, b"RF 50mm f/1.2L\0"),
            ],
        );
        let exif = parse_exif(&tiff).unwrap();
        assert_eq!(exif.make.as_deref(), Some("Canon"));
        assert_eq!(exif.model.as_deref(), Some("EOS R5 Mark I"));
        assert_eq!(exif.software.as_deref(), Some("Lightroom"));
        assert_eq!(exif.date_time.as_deref(), Some("2025:06:15 14:30:00"));
        assert_eq!(
            exif.date_time_original.as_deref(),
            Some("2025:06:15 14:25:00")
        );
        assert_eq!(exif.lens_model.as_deref(), Some("RF 50mm f/1.2L"));
    }

    // =====================================================================
    // 4. GPS parsing: DMS -> decimal
    // =====================================================================

    #[test]
    fn gps_parsing() {
        let le = true;
        // 40 deg 26' 46.302" N, 79 deg 58' 56.1" W (Pittsburgh)
        let lat_bytes = make_gps_dms_bytes(40, 1, 26, 1, 46302, 1000, le);
        let lon_bytes = make_gps_dms_bytes(79, 1, 58, 1, 561, 10, le);
        let alt_bytes = make_rational_bytes(300, 1, le);

        let tiff = build_tiff_with_gps(
            le,
            &[],
            &[],
            &[
                (TAG_GPS_LATITUDE_REF, TYPE_ASCII, 2, b"N\0"),
                (TAG_GPS_LATITUDE, TYPE_RATIONAL, 3, &lat_bytes),
                (TAG_GPS_LONGITUDE_REF, TYPE_ASCII, 2, b"W\0"),
                (TAG_GPS_LONGITUDE, TYPE_RATIONAL, 3, &lon_bytes),
                (TAG_GPS_ALTITUDE_REF, TYPE_BYTE, 1, &[0]),
                (TAG_GPS_ALTITUDE, TYPE_RATIONAL, 1, &alt_bytes),
            ],
        );
        let exif = parse_exif(&tiff).unwrap();

        let lat = exif.gps_latitude.as_ref().unwrap();
        assert_eq!(lat.reference, 'N');
        assert!((lat.degrees - 40.0).abs() < 0.001);
        assert!((lat.minutes - 26.0).abs() < 0.001);
        assert!((lat.seconds - 46.302).abs() < 0.001);

        let lat_dec = lat.to_decimal();
        assert!((lat_dec - 40.446195).abs() < 0.0001);

        let lon = exif.gps_longitude.as_ref().unwrap();
        assert_eq!(lon.reference, 'W');
        let lon_dec = lon.to_decimal();
        assert!(lon_dec < 0.0); // West is negative

        let alt = exif.gps_altitude.unwrap();
        assert!((alt - 300.0).abs() < 0.001);
    }

    #[test]
    fn gps_below_sea_level() {
        let le = true;
        let alt_bytes = make_rational_bytes(15, 1, le);
        let tiff = build_tiff_with_gps(
            le,
            &[],
            &[],
            &[
                (TAG_GPS_ALTITUDE_REF, TYPE_BYTE, 1, &[1]), // 1 = below sea level
                (TAG_GPS_ALTITUDE, TYPE_RATIONAL, 1, &alt_bytes),
            ],
        );
        let exif = parse_exif(&tiff).unwrap();
        assert_eq!(exif.gps_altitude, Some(-15.0));
    }

    // =====================================================================
    // 5. Orientation: all 8 values
    // =====================================================================

    #[test]
    fn orientation_all_values() {
        for orient in 1u16..=8 {
            let le = true;
            let tiff = build_tiff(
                le,
                &[(
                    TAG_ORIENTATION,
                    TYPE_SHORT,
                    1,
                    &make_short_bytes(orient, le),
                )],
            );
            let exif = parse_exif(&tiff).unwrap();
            assert_eq!(exif.orientation, Some(orient), "orientation {orient}");
        }
    }

    // =====================================================================
    // 6. Exposure: ShutterSpeed, FNumber, ISO
    // =====================================================================

    #[test]
    fn exposure_roundtrip() {
        let le = true;
        let exp_bytes = make_rational_bytes(1, 250, le);
        let fnum_bytes = make_rational_bytes(28, 10, le);
        let focal_bytes = make_rational_bytes(50, 1, le);
        let iso_bytes = make_short_bytes(800, le);
        let focal35_bytes = make_short_bytes(75, le);

        let tiff = build_tiff_with_exif_ifd(
            le,
            &[],
            &[
                (TAG_EXPOSURE_TIME, TYPE_RATIONAL, 1, &exp_bytes),
                (TAG_F_NUMBER, TYPE_RATIONAL, 1, &fnum_bytes),
                (TAG_ISO_SPEED, TYPE_SHORT, 1, &iso_bytes),
                (TAG_FOCAL_LENGTH, TYPE_RATIONAL, 1, &focal_bytes),
                (TAG_FOCAL_LENGTH_35MM, TYPE_SHORT, 1, &focal35_bytes),
            ],
        );
        let exif = parse_exif(&tiff).unwrap();

        let et = exif.exposure_time.unwrap();
        assert_eq!(et.numerator, 1);
        assert_eq!(et.denominator, 250);
        assert!((et.to_f64() - 0.004).abs() < 0.0001);

        let fn_ = exif.f_number.unwrap();
        assert!((fn_.to_f64() - 2.8).abs() < 0.001);

        assert_eq!(exif.iso, Some(800));

        let fl = exif.focal_length.unwrap();
        assert!((fl.to_f64() - 50.0).abs() < 0.001);

        assert_eq!(exif.focal_length_35mm, Some(75));
    }

    // =====================================================================
    // 7. Sub-IFD navigation: IFD0 -> ExifIFD -> GPS IFD
    // =====================================================================

    #[test]
    fn sub_ifd_navigation() {
        let le = true;
        let exp_bytes = make_rational_bytes(1, 1000, le);
        let lat_bytes = make_gps_dms_bytes(48, 1, 51, 1, 24, 1, le);

        let tiff = build_tiff_with_gps(
            le,
            &[
                (TAG_MAKE, TYPE_ASCII, 5, b"Sony\0"),
                (TAG_ORIENTATION, TYPE_SHORT, 1, &make_short_bytes(1, le)),
            ],
            &[(TAG_EXPOSURE_TIME, TYPE_RATIONAL, 1, &exp_bytes)],
            &[
                (TAG_GPS_LATITUDE_REF, TYPE_ASCII, 2, b"N\0"),
                (TAG_GPS_LATITUDE, TYPE_RATIONAL, 3, &lat_bytes),
            ],
        );
        let exif = parse_exif(&tiff).unwrap();

        // IFD0 tags
        assert_eq!(exif.make.as_deref(), Some("Sony"));
        assert_eq!(exif.orientation, Some(1));

        // EXIF IFD tag
        let et = exif.exposure_time.unwrap();
        assert_eq!(et.numerator, 1);
        assert_eq!(et.denominator, 1000);

        // GPS IFD tag
        let lat = exif.gps_latitude.as_ref().unwrap();
        assert_eq!(lat.reference, 'N');
        assert!((lat.degrees - 48.0).abs() < 0.001);
    }

    // =====================================================================
    // 8. Rational math: to_f64(), zero denominator
    // =====================================================================

    #[test]
    fn rational_to_f64() {
        assert!((Rational::new(1, 4).to_f64() - 0.25).abs() < f64::EPSILON);
        assert!((Rational::new(22, 7).to_f64() - 3.142857).abs() < 0.001);
        assert_eq!(Rational::new(0, 0).to_f64(), 0.0);
        assert_eq!(Rational::new(100, 0).to_f64(), 0.0);
    }

    #[test]
    fn srational_to_f64() {
        assert!((SRational::new(-1, 3).to_f64() + 0.3333).abs() < 0.001);
        assert!((SRational::new(2, 3).to_f64() - 0.6667).abs() < 0.001);
        assert_eq!(SRational::new(0, 0).to_f64(), 0.0);
        assert_eq!(SRational::new(-5, 0).to_f64(), 0.0);
    }

    #[test]
    fn rational_display() {
        assert_eq!(alloc::format!("{}", Rational::new(1, 500)), "1/500");
        assert_eq!(alloc::format!("{}", SRational::new(-2, 3)), "-2/3");
    }

    // =====================================================================
    // 9. GpsCoordinate: N/S/E/W -> positive/negative decimal
    // =====================================================================

    #[test]
    fn gps_coordinate_north_positive() {
        let coord = GpsCoordinate {
            degrees: 40.0,
            minutes: 26.0,
            seconds: 46.0,
            reference: 'N',
        };
        assert!(coord.to_decimal() > 0.0);
        assert!((coord.to_decimal() - 40.4461).abs() < 0.001);
    }

    #[test]
    fn gps_coordinate_south_negative() {
        let coord = GpsCoordinate {
            degrees: 33.0,
            minutes: 51.0,
            seconds: 54.0,
            reference: 'S',
        };
        let dec = coord.to_decimal();
        assert!(dec < 0.0);
        assert!((dec + 33.865).abs() < 0.001);
    }

    #[test]
    fn gps_coordinate_east_positive() {
        let coord = GpsCoordinate {
            degrees: 2.0,
            minutes: 17.0,
            seconds: 40.0,
            reference: 'E',
        };
        assert!(coord.to_decimal() > 0.0);
    }

    #[test]
    fn gps_coordinate_west_negative() {
        let coord = GpsCoordinate {
            degrees: 79.0,
            minutes: 58.0,
            seconds: 56.0,
            reference: 'W',
        };
        assert!(coord.to_decimal() < 0.0);
    }

    // =====================================================================
    // 10. Error cases
    // =====================================================================

    #[test]
    fn error_empty_data() {
        assert!(matches!(parse_exif(&[]), Err(ExifError::TooShort)));
    }

    #[test]
    fn error_too_short() {
        assert!(matches!(
            parse_exif(&[0x49, 0x49, 0x2A]),
            Err(ExifError::TooShort)
        ));
    }

    #[test]
    fn error_invalid_byte_order() {
        let data = [b'X', b'X', 0x00, 0x2A, 0x00, 0x00, 0x00, 0x08];
        assert!(matches!(
            parse_exif(&data),
            Err(ExifError::InvalidByteOrder)
        ));
    }

    #[test]
    fn error_invalid_tiff_magic() {
        // II header but magic=99 instead of 42
        let data = [b'I', b'I', 0x63, 0x00, 0x08, 0x00, 0x00, 0x00];
        assert!(matches!(
            parse_exif(&data),
            Err(ExifError::InvalidTiffMagic)
        ));
    }

    #[test]
    fn error_ifd_offset_out_of_bounds() {
        // Valid header but IFD offset points past end
        let data = [b'I', b'I', 0x2A, 0x00, 0xFF, 0xFF, 0x00, 0x00];
        assert!(matches!(
            parse_exif(&data),
            Err(ExifError::OffsetOutOfBounds)
        ));
    }

    #[test]
    fn truncated_ifd_entries_handled_gracefully() {
        // Valid header, IFD at offset 8, claims 5 entries but data ends early.
        // Should parse what it can without panicking.
        let le = true;
        let mut buf = Vec::new();
        buf.extend_from_slice(b"II");
        push_u16(&mut buf, 42, le);
        push_u32(&mut buf, 8, le);
        push_u16(&mut buf, 5, le); // claims 5 entries
        // Only provide 1 entry (12 bytes)
        push_u16(&mut buf, TAG_ORIENTATION, le);
        push_u16(&mut buf, TYPE_SHORT, le);
        push_u32(&mut buf, 1, le);
        let mut inline = [0u8; 4];
        inline[0] = 3;
        buf.extend_from_slice(&inline);

        let exif = parse_exif(&buf).unwrap();
        assert_eq!(exif.orientation, Some(3));
    }

    #[test]
    fn value_offset_out_of_bounds_skipped() {
        // An entry with value offset pointing past the buffer should be skipped.
        let le = true;
        let mut buf = Vec::new();
        buf.extend_from_slice(b"II");
        push_u16(&mut buf, 42, le);
        push_u32(&mut buf, 8, le);
        push_u16(&mut buf, 2, le); // 2 entries

        // Entry 1: Make with offset pointing past end (8 bytes > 4 so offset used)
        push_u16(&mut buf, TAG_MAKE, le);
        push_u16(&mut buf, TYPE_ASCII, le);
        push_u32(&mut buf, 8, le); // 8 chars
        push_u32(&mut buf, 0xFFFF, le); // bad offset

        // Entry 2: valid orientation
        push_u16(&mut buf, TAG_ORIENTATION, le);
        push_u16(&mut buf, TYPE_SHORT, le);
        push_u32(&mut buf, 1, le);
        let mut inline = [0u8; 4];
        inline[0] = 6;
        buf.extend_from_slice(&inline);

        push_u32(&mut buf, 0, le); // next IFD

        let exif = parse_exif(&buf).unwrap();
        assert!(exif.make.is_none()); // skipped
        assert_eq!(exif.orientation, Some(6)); // still parsed
    }

    // =====================================================================
    // 11. Additional fields: color space, flash, white balance, metering
    // =====================================================================

    #[test]
    fn additional_exif_fields() {
        let le = true;
        let tiff = build_tiff_with_exif_ifd(
            le,
            &[],
            &[
                (TAG_COLOR_SPACE, TYPE_SHORT, 1, &make_short_bytes(1, le)), // sRGB
                (TAG_FLASH, TYPE_SHORT, 1, &make_short_bytes(0x0F, le)), // fired + return detected
                (TAG_WHITE_BALANCE, TYPE_SHORT, 1, &make_short_bytes(0, le)), // auto
                (TAG_METERING_MODE, TYPE_SHORT, 1, &make_short_bytes(5, le)), // pattern
                (
                    TAG_EXPOSURE_PROGRAM,
                    TYPE_SHORT,
                    1,
                    &make_short_bytes(2, le),
                ), // normal
                (TAG_PIXEL_X_DIMENSION, TYPE_LONG, 1, &4000u32.to_le_bytes()),
                (TAG_PIXEL_Y_DIMENSION, TYPE_LONG, 1, &3000u32.to_le_bytes()),
            ],
        );
        let exif = parse_exif(&tiff).unwrap();
        assert_eq!(exif.color_space, Some(1));
        assert_eq!(exif.flash, Some(0x0F));
        assert_eq!(exif.white_balance, Some(0));
        assert_eq!(exif.metering_mode, Some(5));
        assert_eq!(exif.exposure_program, Some(2));
        assert_eq!(exif.width, Some(4000));
        assert_eq!(exif.height, Some(3000));
    }

    // =====================================================================
    // 12. EXIF with "Exif\0\0" header (JPEG-style)
    // =====================================================================

    #[test]
    fn jpeg_style_exif_with_prefix() {
        let le = true;
        let tiff = build_tiff(
            le,
            &[(TAG_ORIENTATION, TYPE_SHORT, 1, &make_short_bytes(3, le))],
        );
        let mut with_prefix = b"Exif\0\0".to_vec();
        with_prefix.extend_from_slice(&tiff);

        let exif = parse_exif(&with_prefix).unwrap();
        assert_eq!(exif.orientation, Some(3));
    }

    // =====================================================================
    // 13. EXIF without header (PNG/AVIF-style raw TIFF)
    // =====================================================================

    #[test]
    fn raw_tiff_without_prefix() {
        let le = false; // big-endian
        let tiff = build_tiff(le, &[(TAG_MAKE, TYPE_ASCII, 5, b"Sony\0")]);
        // No "Exif\0\0" prefix — raw TIFF
        let exif = parse_exif(&tiff).unwrap();
        assert_eq!(exif.make.as_deref(), Some("Sony"));
    }

    // =====================================================================
    // Extra: big-endian full roundtrip
    // =====================================================================

    #[test]
    fn big_endian_full_roundtrip() {
        let le = false;
        let exp_bytes = make_rational_bytes(1, 125, le);
        let fnum_bytes = make_rational_bytes(56, 10, le);

        let tiff = build_tiff_with_exif_ifd(
            le,
            &[
                (TAG_MAKE, TYPE_ASCII, 8, b"Olympus\0"),
                (TAG_MODEL, TYPE_ASCII, 8, b"E-M1 II\0"),
                (TAG_ORIENTATION, TYPE_SHORT, 1, &make_short_bytes(1, le)),
            ],
            &[
                (TAG_EXPOSURE_TIME, TYPE_RATIONAL, 1, &exp_bytes),
                (TAG_F_NUMBER, TYPE_RATIONAL, 1, &fnum_bytes),
                (TAG_ISO_SPEED, TYPE_SHORT, 1, &make_short_bytes(400, le)),
                (TAG_COLOR_SPACE, TYPE_SHORT, 1, &make_short_bytes(1, le)),
            ],
        );
        let exif = parse_exif(&tiff).unwrap();
        assert_eq!(exif.make.as_deref(), Some("Olympus"));
        assert_eq!(exif.model.as_deref(), Some("E-M1 II"));
        assert_eq!(exif.orientation, Some(1));

        let et = exif.exposure_time.unwrap();
        assert_eq!(et.numerator, 1);
        assert_eq!(et.denominator, 125);

        let fn_ = exif.f_number.unwrap();
        assert!((fn_.to_f64() - 5.6).abs() < 0.001);

        assert_eq!(exif.iso, Some(400));
        assert_eq!(exif.color_space, Some(1));
    }

    // =====================================================================
    // Empty EXIF data returns default
    // =====================================================================

    #[test]
    fn minimal_valid_tiff() {
        let le = true;
        let tiff = build_tiff(le, &[]);
        let exif = parse_exif(&tiff).unwrap();
        assert!(exif.make.is_none());
        assert!(exif.orientation.is_none());
        assert!(exif.gps_latitude.is_none());
    }

    // =====================================================================
    // ExifData Default
    // =====================================================================

    #[test]
    fn exif_data_default_is_all_none() {
        let exif = ExifData::default();
        assert!(exif.make.is_none());
        assert!(exif.model.is_none());
        assert!(exif.orientation.is_none());
        assert!(exif.iso.is_none());
        assert!(exif.gps_latitude.is_none());
        assert!(exif.gps_longitude.is_none());
        assert!(exif.gps_altitude.is_none());
    }
}
