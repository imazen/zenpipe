//! Shared test helpers for zencodecs integration tests.

use imgref::ImgVec;
use rgb::{Rgb, Rgba};
use zencodecs::{EncodeRequest, ImageFormat};

/// Create a synthetic RGB8 gradient image.
pub fn rgb8_image(w: usize, h: usize) -> ImgVec<Rgb<u8>> {
    let pixels: Vec<Rgb<u8>> = (0..w * h)
        .map(|i| {
            let x = (i % w) as u8;
            let y = (i / w) as u8;
            Rgb { r: x, g: y, b: 128 }
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

/// Create a synthetic RGBA8 gradient image.
pub fn rgba8_image(w: usize, h: usize) -> ImgVec<Rgba<u8>> {
    let pixels: Vec<Rgba<u8>> = (0..w * h)
        .map(|i| {
            let x = (i % w) as u8;
            let y = (i / w) as u8;
            Rgba {
                r: x,
                g: y,
                b: 128,
                a: 200,
            }
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

/// Encode test data for a given format at a given size.
pub fn encode_test_data(format: ImageFormat, w: usize, h: usize) -> Vec<u8> {
    let img = rgb8_image(w, h);
    EncodeRequest::new(format)
        .with_quality(50.0)
        .encode_full_frame_rgb8(img.as_ref())
        .unwrap_or_else(|e| panic!("failed to encode {format:?} test data: {e}"))
        .into_vec()
}

/// Encode RGBA test data for formats that need alpha (GIF).
pub fn encode_rgba_test_data(format: ImageFormat, w: usize, h: usize) -> Vec<u8> {
    let img = rgba8_image(w, h);
    EncodeRequest::new(format)
        .with_quality(50.0)
        .encode_full_frame_rgba8(img.as_ref())
        .unwrap_or_else(|e| panic!("failed to encode {format:?} RGBA test data: {e}"))
        .into_vec()
}
