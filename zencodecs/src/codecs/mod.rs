//! Codec adapters for format-specific implementations.
//!
//! Each module provides a thin adapter between zencodecs' unified API and
//! the format-specific codec crate.

#[cfg(feature = "jpeg")]
pub(crate) mod jpeg;

#[cfg(feature = "webp")]
pub(crate) mod webp;

#[cfg(feature = "gif")]
pub(crate) mod gif;

#[cfg(feature = "png")]
pub(crate) mod png;

#[cfg(feature = "avif-decode")]
pub(crate) mod avif_dec;

#[cfg(feature = "avif-encode")]
pub(crate) mod avif_enc;

#[cfg(feature = "jxl-decode")]
pub(crate) mod jxl_dec;

#[cfg(feature = "jxl-encode")]
pub(crate) mod jxl_enc;

#[cfg(feature = "heic-decode")]
pub(crate) mod heic;

#[cfg(feature = "bitmaps")]
pub(crate) mod pnm;

#[cfg(feature = "bitmaps-bmp")]
pub(crate) mod bmp;

#[cfg(feature = "bitmaps")]
pub(crate) mod farbfeld;

#[cfg(feature = "bitmaps-qoi")]
pub(crate) mod qoi;

#[cfg(feature = "bitmaps-tga")]
pub(crate) mod tga;

#[cfg(feature = "bitmaps-hdr")]
pub(crate) mod hdr;

#[cfg(feature = "tiff")]
pub(crate) mod tiff;

#[cfg(feature = "raw-decode")]
pub(crate) mod raw;

#[cfg(feature = "jp2-decode")]
pub(crate) mod jp2;

#[cfg(feature = "svg")]
pub(crate) mod svg;
