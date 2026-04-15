//! Typed pixel buffer definitions — re-exports from zencodec.
//!
//! Uses `imgref::ImgVec` for 2D pixel data with typed pixels from the `rgb` crate.

pub use imgref::{Img, ImgRef, ImgRefMut, ImgVec};
pub use rgb::{Bgra, Gray, Rgb, Rgba};
