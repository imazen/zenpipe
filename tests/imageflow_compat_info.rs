//! `zen_get_image_info` contract: dimensions are display-oriented
//! (EXIF orientation applied), matching imageflow's `v1/get_image_info`
//! (zenpipe#16).

#![cfg(all(feature = "imageflow-compat", feature = "nodes-jpeg"))]

use zenpipe::imageflow_compat::execute::zen_get_image_info;

/// Minimal little-endian TIFF/EXIF blob containing only an Orientation tag.
fn exif_with_orientation(value: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"II"); // little-endian
    v.extend_from_slice(&42u16.to_le_bytes());
    v.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
    v.extend_from_slice(&1u16.to_le_bytes()); // 1 entry
    v.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation tag
    v.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    v.extend_from_slice(&1u32.to_le_bytes()); // count
    v.extend_from_slice(&value.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes()); // pad value to 4 bytes
    v.extend_from_slice(&0u32.to_le_bytes()); // next IFD = none
    v
}

/// Encode a `w`×`h` RGB JPEG carrying the given EXIF orientation value.
fn jpeg_with_orientation(w: u32, h: u32, orientation: u16) -> Vec<u8> {
    let stride = w as usize * 3;
    let pixels = vec![100u8; stride * h as usize];
    let ps =
        zenpixels::PixelSlice::new(&pixels, w, h, stride, zenpixels::PixelDescriptor::RGB8_SRGB)
            .expect("pixel slice");
    zencodecs::EncodeRequest::new(zencodec::ImageFormat::Jpeg)
        .with_quality(80.0)
        .with_metadata(zencodec::Metadata::none().with_exif(exif_with_orientation(orientation)))
        .encode(ps, false)
        .expect("JPEG encode")
        .into_vec()
}

#[test]
fn image_info_swaps_dimensions_for_transposed_exif_orientation() {
    // Stored 64×32 with orientation 6 (Rotate90): displays as 32×64.
    let jpeg = jpeg_with_orientation(64, 32, 6);

    // Sanity: the raw probe still sees the stored dims + the orientation.
    let raw = zencodecs::from_bytes(&jpeg).expect("probe");
    assert_eq!((raw.width, raw.height), (64, 32));
    assert_eq!(raw.orientation, zencodec::Orientation::Rotate90);

    let info = zen_get_image_info(&jpeg).expect("zen_get_image_info");
    assert_eq!(
        (info.width, info.height),
        (32, 64),
        "zen_get_image_info must report display-oriented dimensions"
    );
    assert_eq!(
        info.orientation,
        zencodec::Orientation::Identity,
        "orientation is folded into the reported dimensions"
    );
    assert_eq!((info.display_width(), info.display_height()), (32, 64));
}

#[test]
fn image_info_keeps_dimensions_for_non_transposed_orientation() {
    // Orientation 3 (Rotate180) does not swap axes.
    let jpeg = jpeg_with_orientation(64, 32, 3);
    let info = zen_get_image_info(&jpeg).expect("zen_get_image_info");
    assert_eq!((info.width, info.height), (64, 32));
    assert_eq!(info.orientation, zencodec::Orientation::Identity);

    // No EXIF at all: unchanged.
    let jpeg = jpeg_with_orientation(64, 32, 1);
    let info = zen_get_image_info(&jpeg).expect("zen_get_image_info");
    assert_eq!((info.width, info.height), (64, 32));
}
