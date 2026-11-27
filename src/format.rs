//! Pixel format descriptor — re-exported from [`zenpixels`].
//!
//! [`PixelFormat`] carries the sample layout (sample type, channel order,
//! bit packing) and color space (primaries, transfer, range, diffuse white,
//! alpha mode, or embedded ICC profile).

pub use zenpixels::{
    AlphaMode, ChannelOrder, ColorOrigin, ColorSpace, HlgMode, PixelFormat, Primaries, RowAlign,
    SampleLayout, SampleType, SignalRange, Transfer,
};

// =========================================================================
// Well-known format constants
// =========================================================================

/// 4 bytes/pixel, sRGB transfer, straight alpha. Standard decode/encode format.
pub const RGBA8_SRGB: PixelFormat = PixelFormat::RGBA8_SRGB;

/// 16 bytes/pixel (4×f32), linear light, premultiplied alpha.
/// Working format for resize and compositing.
pub const RGBAF32_LINEAR_PREMUL: PixelFormat = PixelFormat::new(
    SampleLayout::RGBAF32,
    ColorSpace::Rgb {
        primaries: Primaries::Bt709,
        transfer: Transfer::Linear,
        range: SignalRange::Full,
        white: None,
        alpha: AlphaMode::Premultiplied,
    },
);

/// 16 bytes/pixel (4×f32), linear light, straight alpha.
pub const RGBAF32_LINEAR: PixelFormat = PixelFormat::RGBAF32_LINEAR;

/// 3 bytes/pixel, sRGB transfer, no alpha. Standard JPEG format.
pub const RGB8_SRGB: PixelFormat = PixelFormat::RGB8_SRGB;

/// 4 bytes/pixel, linear transfer, straight alpha. Working format for
/// sources whose bytes must be resampled in their own encoded domain —
/// gain-map sidecars (zenpipe#41) and genuinely linear-light u8 data.
/// zenresize maps the Linear transfer to raw u8↔f32 (no gamma round-trip).
pub const RGBA8_LINEAR: PixelFormat = PixelFormat::new(
    SampleLayout::RGBA8,
    ColorSpace::Rgb {
        primaries: Primaries::Bt709,
        transfer: Transfer::Linear,
        range: SignalRange::Full,
        white: None,
        alpha: AlphaMode::Straight,
    },
);

/// 16 bytes/pixel (4×f32), sRGB transfer, straight alpha.
/// Identity mode — no gamma conversion, just normalized to 0..1.
pub const RGBAF32_SRGB: PixelFormat = PixelFormat::new(
    SampleLayout::RGBAF32,
    ColorSpace::Rgb {
        primaries: Primaries::Bt709,
        transfer: Transfer::Srgb,
        range: SignalRange::Full,
        white: None,
        alpha: AlphaMode::Straight,
    },
);

/// Tight byte stride for `width` pixels of `format`, saturating on overflow.
#[inline]
pub(crate) const fn aligned_stride(format: &PixelFormat, width: u32) -> usize {
    (width as usize).saturating_mul(format.layout.bytes_per_pixel())
}

/// Whether `format` carries an active alpha channel (`Straight`, `Premultiplied`,
/// or `Opaque` — false for `None` and padding `Undefined`).
#[inline]
pub(crate) const fn format_has_alpha(format: &PixelFormat) -> bool {
    matches!(
        format.color.alpha(),
        AlphaMode::Straight | AlphaMode::Premultiplied | AlphaMode::Opaque
    )
}

/// Transfer function carried by `format`'s color space, if parametric.
#[inline]
pub(crate) const fn format_transfer(format: &PixelFormat) -> Transfer {
    match format.color {
        ColorSpace::Rgb { transfer, .. } | ColorSpace::Gray { transfer, .. } => transfer,
        ColorSpace::Xyz { .. } | ColorSpace::Cmyk { .. } | ColorSpace::Untagged { .. } => {
            Transfer::Linear
        }
        _ => Transfer::Unknown,
    }
}

/// Whether `format` uses linear light.
#[allow(dead_code)]
#[inline]
pub(crate) const fn format_is_linear(format: &PixelFormat) -> bool {
    matches!(format_transfer(format), Transfer::Linear)
}
