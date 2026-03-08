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
