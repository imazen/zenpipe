//! An encode keeps the EXIF ColorSpace tag in step with the colour it writes:
//! `1` only for sRGB, `0xFFFF` otherwise (imazen/zencodec#132).

#![cfg(feature = "jpeg")]

use zencodec::Metadata;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};
use zenpixels::{PixelDescriptor, PixelSlice};

/// II TIFF: IFD0 {ExifIFD ->}; Exif IFD {ColorSpace = `cs`}.
fn exif_with_color_space(cs: u16) -> Vec<u8> {
    let mut t = b"II*\0".to_vec();
    t.extend_from_slice(&8u32.to_le_bytes());
    t.extend_from_slice(&1u16.to_le_bytes());
    t.extend_from_slice(&[0x69, 0x87, 4, 0, 1, 0, 0, 0]);
    t.extend_from_slice(&26u32.to_le_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(&1u16.to_le_bytes());
    t.extend_from_slice(&[0x01, 0xA0, 3, 0, 1, 0, 0, 0]);
    t.extend_from_slice(&cs.to_le_bytes());
    t.extend_from_slice(&[0, 0]);
    t.extend_from_slice(&0u32.to_le_bytes());
    t
}

fn color_space_after_encode(meta: Metadata) -> u16 {
    let px = vec![128u8; 8 * 8 * 3];
    let slice = PixelSlice::new(&px, 8, 8, 24, PixelDescriptor::RGB8_SRGB).unwrap();
    let bytes = EncodeRequest::new(ImageFormat::Jpeg)
        .with_metadata(meta)
        .encode(slice, false)
        .unwrap()
        .into_vec();
    let exif = DecodeRequest::new(&bytes)
        .decode_full_frame()
        .unwrap()
        .metadata()
        .exif
        .expect("EXIF kept");
    let exif = exif.strip_prefix(b"Exif\0\0").unwrap_or(&exif);
    let at = exif
        .windows(4)
        .position(|w| w == [0x01, 0xA0, 3, 0])
        .expect("ColorSpace entry");
    u16::from_le_bytes([exif[at + 8], exif[at + 9]])
}

#[test]
fn a_wide_gamut_encode_says_uncalibrated() {
    let meta = Metadata::none()
        .with_exif(exif_with_color_space(1))
        .with_icc(zenpixels_convert::icc_profiles::DISPLAY_P3_V4.to_vec());
    assert_eq!(color_space_after_encode(meta), 0xFFFF);
}

#[test]
fn an_srgb_encode_says_srgb() {
    let meta = Metadata::none().with_exif(exif_with_color_space(0xFFFF));
    assert_eq!(color_space_after_encode(meta), 1);
}
