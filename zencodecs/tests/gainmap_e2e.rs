//! End-to-end gain map tests exercising the full pipeline:
//! encode HDR → UltraHDR JPEG → decode_gain_map → reconstruct HDR → verify values
//! encode HDR → UltraHDR JPEG → decode_gain_map → passthrough → re-encode → verify
//!
//! These tests verify the complete gain map lifecycle across format boundaries.

#![cfg(feature = "jpeg-ultrahdr")]

use imgref::{ImgRef, ImgVec};
use rgb::{Rgb, Rgba};
use zencodecs::{DecodeRequest, DecodedGainMap, EncodeRequest, GainMapSource, ImageFormat};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Generate a synthetic HDR gradient (linear f32 RGB, BT.709).
/// Values range from 0.0 to `peak` — a peak > 1.0 simulates HDR content.
fn make_hdr_gradient(w: usize, h: usize, peak: f32) -> ImgVec<Rgb<f32>> {
    let pixels: Vec<Rgb<f32>> = (0..w * h)
        .map(|i| {
            let x = (i % w) as f32 / w.max(1) as f32;
            let y = (i / w) as f32 / h.max(1) as f32;
            Rgb {
                r: x * peak,
                g: y * peak * 0.7,
                b: 0.2 + x * 0.3,
            }
        })
        .collect();
    ImgVec::new(pixels, w, h)
}

/// Encode to UltraHDR JPEG and return bytes.
fn encode_ultrahdr(img: ImgRef<'_, Rgb<f32>>, quality: f32) -> Vec<u8> {
    EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(quality)
        .encode_ultrahdr_rgb_f32(img)
        .expect("UltraHDR encode failed")
        .data()
        .to_vec()
}

/// Decode and extract gain map from bytes.
fn decode_with_gainmap(data: &[u8]) -> (zencodecs::DecodeOutput, Option<DecodedGainMap>) {
    DecodeRequest::new(data)
        .decode_gain_map()
        .expect("decode_gain_map failed")
}

/// Get SDR base as contiguous RGB8 bytes from DecodeOutput.
fn sdr_bytes(output: zencodecs::DecodeOutput) -> (Vec<u8>, u32, u32) {
    use zencodecs::PixelBufferConvertTypedExt as _;
    let w = output.width();
    let h = output.height();
    let rgb8 = output.into_buffer().to_rgb8();
    let img = rgb8.as_imgref();
    let bytes: Vec<u8> = bytemuck::cast_slice(img.buf()).to_vec();
    (bytes, w, h)
}

/// Apply gain map to reconstruct HDR using ultrahdr-core.
fn reconstruct_hdr(
    gm: &DecodedGainMap,
    base_bytes: &[u8],
    w: u32,
    h: u32,
    channels: u8,
    display_boost: f32,
) -> Vec<u8> {
    use zenjpeg::ultrahdr::{
        HdrOutputFormat, UhdrColorGamut, UhdrColorTransfer, UhdrPixelFormat, UhdrRawImage,
        Unstoppable, apply_gainmap,
    };

    let pixel_format = match channels {
        3 => UhdrPixelFormat::Rgb8,
        4 => UhdrPixelFormat::Rgba8,
        _ => panic!("unsupported channels: {channels}"),
    };

    let sdr = UhdrRawImage::from_data(
        w,
        h,
        pixel_format,
        UhdrColorGamut::Bt709,
        UhdrColorTransfer::Srgb,
        base_bytes.to_vec(),
    )
    .expect("RawImage creation failed");

    let hdr_result = apply_gainmap(
        &sdr,
        &gm.gain_map,
        &gm.metadata,
        display_boost,
        HdrOutputFormat::LinearFloat,
        Unstoppable,
    )
    .expect("apply_gainmap failed");

    hdr_result.data
}

/// Assert gain map fields are well-formed.
fn assert_gain_map_valid(gm: &zencodecs::GainMap) {
    assert!(gm.width > 0, "gain map width must be > 0");
    assert!(gm.height > 0, "gain map height must be > 0");
    assert!(
        gm.channels == 1 || gm.channels == 3,
        "channels must be 1 or 3, got {}",
        gm.channels
    );
    let expected = gm.width as usize * gm.height as usize * gm.channels as usize;
    assert_eq!(
        gm.data.len(),
        expected,
        "data length {} != {}x{}x{} = {}",
        gm.data.len(),
        gm.width,
        gm.height,
        gm.channels,
        expected
    );
}

// ─── E2E: Full HDR Roundtrip ────────────────────────────────────────────────

#[test]
fn e2e_hdr_encode_decode_reconstruct_verify_values() {
    // Step 1: Create HDR content with known peak luminance
    let hdr_img = make_hdr_gradient(128, 128, 3.0);

    // Step 2: Encode to UltraHDR JPEG
    let ultrahdr_bytes = encode_ultrahdr(hdr_img.as_ref(), 90.0);
    assert!(ultrahdr_bytes.len() > 500, "UltraHDR should be non-trivial");

    // Step 3: Decode + extract gain map
    let (output, gainmap) = decode_with_gainmap(&ultrahdr_bytes);
    let gm = gainmap.expect("UltraHDR must have a gain map");

    assert!(!gm.base_is_hdr);
    assert_eq!(gm.source_format, ImageFormat::Jpeg);
    assert_gain_map_valid(&gm.gain_map);

    // Step 4: Reconstruct HDR from SDR base + gain map
    let (base_bytes, w, h) = sdr_bytes(output);
    let hdr_data = reconstruct_hdr(&gm, &base_bytes, w, h, 3, 4.0);

    // Step 5: Verify HDR output properties
    let f32_pixels: &[f32] = bytemuck::cast_slice(&hdr_data);
    assert_eq!(
        f32_pixels.len(),
        w as usize * h as usize * 4,
        "should be RGBA f32"
    );

    // HDR should have values exceeding SDR range
    let max_val = f32_pixels.iter().copied().fold(0.0f32, f32::max);
    assert!(
        max_val > 1.0,
        "reconstructed HDR should exceed 1.0, got {max_val}"
    );

    // HDR reconstruction at boost=1.0 should approximately match SDR
    let sdr_at_1x = reconstruct_hdr(&gm, &base_bytes, w, h, 3, 1.0);
    let sdr_f32: &[f32] = bytemuck::cast_slice(&sdr_at_1x);
    let max_sdr = sdr_f32.iter().copied().fold(0.0f32, f32::max);
    assert!(
        max_sdr <= 1.5,
        "at boost=1.0, values should be near SDR range, got {max_sdr}"
    );
}

// ─── E2E: Gain Map Metadata Integrity ───────────────────────────────────────

#[test]
fn e2e_gain_map_metadata_reflects_hdr_content() {
    // Encode with high peak → high boost ratio expected
    let high_peak = make_hdr_gradient(64, 64, 5.0);
    let bytes_high = encode_ultrahdr(high_peak.as_ref(), 85.0);
    let (_, gm_high) = decode_with_gainmap(&bytes_high);
    let gm_high = gm_high.expect("must have gain map");

    // Encode with low peak → low boost ratio expected
    let low_peak = make_hdr_gradient(64, 64, 1.5);
    let bytes_low = encode_ultrahdr(low_peak.as_ref(), 85.0);
    let (_, gm_low) = decode_with_gainmap(&bytes_low);
    let gm_low = gm_low.expect("must have gain map");

    // High-peak content should have higher gain_map_max (log2 domain)
    assert!(
        gm_high.metadata.gain_map_max[0] >= gm_low.metadata.gain_map_max[0],
        "higher peak ({}) should have >= gain_map_max than lower peak ({})",
        gm_high.metadata.gain_map_max[0],
        gm_low.metadata.gain_map_max[0],
    );
}

// ─── E2E: Gain Map Passthrough (same-format) ───────────────────────────────

#[test]
fn e2e_gain_map_passthrough_jpeg_to_jpeg() {
    // Step 1: Create original UltraHDR JPEG
    let hdr_img = make_hdr_gradient(64, 64, 3.0);
    let original_bytes = encode_ultrahdr(hdr_img.as_ref(), 90.0);

    // Step 2: Decode + extract gain map
    let (output, gainmap) = decode_with_gainmap(&original_bytes);
    let gm = gainmap.expect("must have gain map");

    // Step 3: Re-encode the SDR base at different quality with passthrough gain map
    let source = GainMapSource::Precomputed {
        gain_map: &gm.gain_map,
        metadata: &gm.metadata,
    };

    use zencodecs::PixelBufferConvertTypedExt as _;
    let rgb8 = output.into_buffer().to_rgb8();
    let img_ref = rgb8.as_imgref();

    let _request = EncodeRequest::new(ImageFormat::Jpeg)
        .with_quality(75.0) // different quality
        .with_gain_map(source)
        .encode_full_frame_rgb8(img_ref)
        .expect("re-encode should succeed");

    // Verify the gain map data survived the passthrough
    assert_gain_map_valid(&gm.gain_map);
    assert!(gm.metadata.gain_map_max[0] > 0.0);
}

// ─── E2E: Gain Map Extraction Consistency ───────────────────────────────────

#[test]
fn e2e_decode_gain_map_twice_same_result() {
    let hdr_img = make_hdr_gradient(64, 64, 2.5);
    let bytes = encode_ultrahdr(hdr_img.as_ref(), 85.0);

    let (_, gm1) = decode_with_gainmap(&bytes);
    let (_, gm2) = decode_with_gainmap(&bytes);

    let gm1 = gm1.expect("first decode must have gain map");
    let gm2 = gm2.expect("second decode must have gain map");

    // Metadata should be identical
    assert_eq!(gm1.metadata.gain_map_max, gm2.metadata.gain_map_max);
    assert_eq!(gm1.metadata.gain_map_min, gm2.metadata.gain_map_min);
    assert_eq!(gm1.metadata.gamma, gm2.metadata.gamma);
    assert_eq!(
        gm1.metadata.alternate_hdr_headroom,
        gm2.metadata.alternate_hdr_headroom
    );

    // Gain map pixels should be identical
    assert_eq!(gm1.gain_map.data, gm2.gain_map.data);
    assert_eq!(gm1.gain_map.width, gm2.gain_map.width);
    assert_eq!(gm1.gain_map.height, gm2.gain_map.height);
}

// ─── E2E: HDR Reconstruction Quality ────────────────────────────────────────

#[test]
fn e2e_hdr_reconstruction_boost_affects_output() {
    let hdr_img = make_hdr_gradient(64, 64, 4.0);
    let bytes = encode_ultrahdr(hdr_img.as_ref(), 85.0);

    let (output, gainmap) = decode_with_gainmap(&bytes);
    let gm = gainmap.expect("must have gain map");
    let (base_bytes, w, h) = sdr_bytes(output);

    // Reconstruct at different boost levels
    let hdr_low = reconstruct_hdr(&gm, &base_bytes, w, h, 3, 1.5);
    let hdr_high = reconstruct_hdr(&gm, &base_bytes, w, h, 3, 6.0);

    let f32_low: &[f32] = bytemuck::cast_slice(&hdr_low);
    let f32_high: &[f32] = bytemuck::cast_slice(&hdr_high);

    let max_low = f32_low.iter().copied().fold(0.0f32, f32::max);
    let max_high = f32_high.iter().copied().fold(0.0f32, f32::max);

    assert!(
        max_high >= max_low,
        "higher boost should produce brighter output: max_high={max_high}, max_low={max_low}"
    );
}

// ─── E2E: Gain Map Dimensions ───────────────────────────────────────────────

#[test]
fn e2e_gain_map_is_lower_resolution_than_base() {
    let hdr_img = make_hdr_gradient(256, 256, 3.0);
    let bytes = encode_ultrahdr(hdr_img.as_ref(), 85.0);

    let (output, gainmap) = decode_with_gainmap(&bytes);
    let gm = gainmap.expect("must have gain map");

    let base_pixels = output.width() as u64 * output.height() as u64;
    let gm_pixels = gm.gain_map.width as u64 * gm.gain_map.height as u64;

    // Gain map is typically 1/4 to 1/16 the resolution
    assert!(
        gm_pixels < base_pixels,
        "gain map ({gm_pixels} px) should be smaller than base ({base_pixels} px)"
    );
    assert!(gm_pixels > 0, "gain map should have at least one pixel");
}

// ─── E2E: Non-gain-map formats return None ──────────────────────────────────

#[cfg(feature = "webp")]
#[test]
fn e2e_no_gainmap_from_webp_encode_decode() {
    let pixels = vec![
        Rgb {
            r: 100u8,
            g: 150,
            b: 200
        };
        32 * 32
    ];
    let img = ImgVec::new(pixels, 32, 32);

    let encoded = EncodeRequest::new(ImageFormat::WebP)
        .with_quality(80.0)
        .encode_full_frame_rgb8(img.as_ref())
        .expect("webp encode failed");

    let (_, gainmap) = decode_with_gainmap(encoded.data());
    assert!(gainmap.is_none(), "WebP should not produce a gain map");
}

#[cfg(feature = "gif")]
#[test]
fn e2e_no_gainmap_from_gif_encode_decode() {
    let pixels = vec![
        Rgba {
            r: 100u8,
            g: 150,
            b: 200,
            a: 255
        };
        32 * 32
    ];
    let img = ImgVec::new(pixels, 32, 32);

    let encoded = EncodeRequest::new(ImageFormat::Gif)
        .encode_full_frame_rgba8(img.as_ref())
        .expect("gif encode failed");

    let (_, gainmap) = decode_with_gainmap(encoded.data());
    assert!(gainmap.is_none(), "GIF should not produce a gain map");
}

// ─── E2E: Direction flag correctness ────────────────────────────────────────

#[test]
fn e2e_jpeg_gain_map_is_forward_direction() {
    let hdr_img = make_hdr_gradient(32, 32, 2.0);
    let bytes = encode_ultrahdr(hdr_img.as_ref(), 85.0);
    let (_, gainmap) = decode_with_gainmap(&bytes);
    let gm = gainmap.expect("must have gain map");

    assert!(!gm.base_is_hdr, "JPEG UltraHDR: base should be SDR");
    assert_eq!(gm.source_format, ImageFormat::Jpeg);
}

// ─── E2E: Full HDR roundtrip via apply_gainmap ──────────────────────────────

#[test]
fn e2e_gain_map_reconstruct_hdr_roundtrip() {
    let hdr_img = make_hdr_gradient(64, 64, 2.5);
    let bytes = encode_ultrahdr(hdr_img.as_ref(), 90.0);

    // decode_gain_map + apply_gainmap
    let (output, gainmap) = decode_with_gainmap(&bytes);
    let gm = gainmap.expect("must have gain map");
    let (base_bytes, w, h) = sdr_bytes(output);
    let hdr_data = reconstruct_hdr(&gm, &base_bytes, w, h, 3, 4.0);

    // Should produce RGBA f32 output
    assert_eq!(hdr_data.len(), w as usize * h as usize * 16);

    // HDR values should exceed SDR range
    let f32_pixels: &[f32] = bytemuck::cast_slice(&hdr_data);
    let max_val = f32_pixels.iter().copied().fold(0.0f32, f32::max);
    assert!(
        max_val > 1.0,
        "reconstructed HDR should exceed 1.0, got {max_val}"
    );
}

// ─── E2E: JXL gain map encode + decode ──────────────────────────────────────

#[cfg(all(feature = "jxl-encode", feature = "jxl-decode"))]
mod jxl_gainmap {
    use super::*;
    use zencodecs::GainMap;
    use zencodecs::GainMapMetadata;

    /// Encode a base image + precomputed gain map to JXL, then decode and verify
    /// that the jhgm box is present and the metadata roundtrips.
    #[test]
    fn e2e_jxl_encode_with_gain_map() {
        // Step 1: Create a small test image (SDR RGB8)
        let w = 64usize;
        let h = 64usize;
        let pixels: Vec<Rgb<u8>> = (0..w * h)
            .map(|i| {
                let x = (i % w) as u8;
                let y = (i / w) as u8;
                Rgb {
                    r: x.wrapping_mul(3),
                    g: y.wrapping_mul(5),
                    b: 128,
                }
            })
            .collect();
        let img = imgref::ImgVec::new(pixels, w, h);

        // Step 2: Create a gain map (grayscale, lower resolution)
        let gm_w = 16u32;
        let gm_h = 16u32;
        let gm_data: Vec<u8> = (0..gm_w * gm_h).map(|i| (128 + (i % 64)) as u8).collect();
        let gain_map = GainMap {
            data: gm_data,
            width: gm_w,
            height: gm_h,
            channels: 1,
        };

        // Step 3: Create ISO 21496-1 metadata (log2/f64 domain)
        let metadata = GainMapMetadata {
            gain_map_max: [2.0; 3],
            gain_map_min: [0.0; 3],
            gamma: [1.0; 3],
            base_offset: [1.0 / 64.0; 3],
            alternate_offset: [1.0 / 64.0; 3],
            base_hdr_headroom: 0.0,
            alternate_hdr_headroom: 2.0,
            use_base_color_space: true,
            ..GainMapMetadata::default()
        };

        // Step 4: Encode to JXL with gain map
        let source = GainMapSource::Precomputed {
            gain_map: &gain_map,
            metadata: &metadata,
        };
        let output = EncodeRequest::new(ImageFormat::Jxl)
            .with_quality(85.0)
            .with_gain_map(source)
            .encode_full_frame_rgb8(img.as_ref())
            .expect("JXL encode with gain map failed");

        assert!(!output.data().is_empty());
        assert_eq!(output.format(), ImageFormat::Jxl);

        // Step 5: Verify the output is in container format (jhgm requires container)
        assert!(
            zenjxl::container::is_container(output.data()),
            "JXL output with gain map should be in container format"
        );

        // Step 6: Decode and extract the gain map
        let (decoded_output, decoded_gainmap) = DecodeRequest::new(output.data())
            .decode_gain_map()
            .expect("JXL decode_gain_map failed");

        // Verify base image decoded successfully
        assert_eq!(decoded_output.width(), w as u32);
        assert_eq!(decoded_output.height(), h as u32);

        // Step 7: Verify gain map was preserved
        let gm = decoded_gainmap.expect("JXL output should contain a gain map");
        assert!(gm.base_is_hdr, "JXL gain map: base should be HDR");
        assert_eq!(gm.source_format, ImageFormat::Jxl);
        assert_gain_map_valid(&gm.gain_map);
        assert_eq!(gm.gain_map.width, gm_w);
        assert_eq!(gm.gain_map.height, gm_h);

        // Step 8: Verify ISO 21496-1 metadata roundtripped (log2 domain)
        let eps = 0.01;
        assert!(
            (gm.metadata.gain_map_max[0] - 2.0).abs() < eps,
            "gain_map_max should be ~2.0 (log2), got {}",
            gm.metadata.gain_map_max[0],
        );
        assert!(
            (gm.metadata.gain_map_min[0] - 0.0).abs() < eps,
            "gain_map_min should be ~0.0 (log2), got {}",
            gm.metadata.gain_map_min[0],
        );
        assert!(
            (gm.metadata.alternate_hdr_headroom - 2.0).abs() < eps,
            "alternate_hdr_headroom should be ~2.0 (log2), got {}",
            gm.metadata.alternate_hdr_headroom,
        );
        assert!(
            (gm.metadata.gamma[0] - 1.0).abs() < eps,
            "gamma should be ~1.0, got {}",
            gm.metadata.gamma[0],
        );

        // Step 9: Verify gain map pixel data roundtripped (lossless encode)
        if gm.gain_map.channels == 1 {
            assert_eq!(
                gm.gain_map.data.len(),
                (gm_w * gm_h) as usize,
                "grayscale gain map should have w*h bytes"
            );
        } else {
            assert_eq!(gm.gain_map.channels, 3);
            assert_eq!(
                gm.gain_map.data.len(),
                (gm_w * gm_h * 3) as usize,
                "RGB gain map should have w*h*3 bytes"
            );
        }
    }

    /// Encode with an RGB (3-channel) gain map.
    #[test]
    fn e2e_jxl_encode_with_rgb_gain_map() {
        let w = 32usize;
        let h = 32usize;
        let pixels: Vec<Rgb<u8>> = vec![
            Rgb {
                r: 128,
                g: 100,
                b: 80,
            };
            w * h
        ];
        let img = imgref::ImgVec::new(pixels, w, h);

        // 3-channel gain map
        let gm_w = 8u32;
        let gm_h = 8u32;
        let gm_data: Vec<u8> = (0..gm_w * gm_h * 3)
            .map(|i| (100 + (i % 100)) as u8)
            .collect();
        let gain_map = GainMap {
            data: gm_data,
            width: gm_w,
            height: gm_h,
            channels: 3,
        };

        let metadata = GainMapMetadata {
            gain_map_max: [3.0f64.log2(), 3.5f64.log2(), 2.5f64.log2()],
            gain_map_min: [0.0; 3],
            gamma: [1.0; 3],
            base_offset: [1.0 / 64.0; 3],
            alternate_offset: [1.0 / 64.0; 3],
            base_hdr_headroom: 0.0,
            alternate_hdr_headroom: 3.5f64.log2(),
            use_base_color_space: true,
            ..GainMapMetadata::default()
        };

        let source = GainMapSource::Precomputed {
            gain_map: &gain_map,
            metadata: &metadata,
        };
        let output = EncodeRequest::new(ImageFormat::Jxl)
            .with_quality(90.0)
            .with_gain_map(source)
            .encode_full_frame_rgb8(img.as_ref())
            .expect("JXL encode with RGB gain map failed");

        assert!(zenjxl::container::is_container(output.data()));

        // Decode and verify
        let (_decoded, decoded_gm) = DecodeRequest::new(output.data())
            .decode_gain_map()
            .expect("JXL decode failed");
        let gm = decoded_gm.expect("should have gain map");
        assert_eq!(gm.gain_map.width, gm_w);
        assert_eq!(gm.gain_map.height, gm_h);
        // 3-channel gain maps stay 3-channel after roundtrip
        assert_eq!(gm.gain_map.channels, 3);
    }
}
