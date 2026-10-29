//! A validated row view need not own padding after its final row.
#![cfg(feature = "png")]

use zencodecs::{EncodeRequest, ImageFormat};
use zenpixels::{PixelDescriptor, PixelSlice};

#[test]
fn encode_accepts_strided_views_without_final_padding() {
    let packed: Vec<u8> = (1..=18).collect();
    let encode = |data: &[u8], stride| {
        let pixels = PixelSlice::new(data, 2, 3, stride, PixelDescriptor::RGB8_SRGB).unwrap();
        EncodeRequest::new(ImageFormat::Png)
            .encode(pixels, false)
            .unwrap()
            .into_vec()
    };
    let expected = encode(&packed, 6);
    for stride in [9, 12] {
        let mut padded = vec![0xCD; stride * 3];
        for y in 0..3 {
            padded[y * stride..y * stride + 6].copy_from_slice(&packed[y * 6..y * 6 + 6]);
        }
        assert_eq!(encode(&padded, stride), expected);
        padded.truncate(stride * 2 + 6);
        assert_eq!(encode(&padded, stride), expected);
    }
}
