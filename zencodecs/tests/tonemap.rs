//! HDR (PQ/HLG) pixels encoded to a target that carries neither HDR nor
//! CICP are tone-mapped to sRGB (feature `tonemap`); a target that carries
//! CICP (PNG) keeps them.
//!
//! BMP is the lossless no-carrier target, so tone-mapped pixels compare
//! exactly between the one-shot and streaming paths.

#![cfg(all(
    feature = "tonemap",
    feature = "bitmaps",
    feature = "png",
    feature = "gif"
))]

use zencodec::Metadata;
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, ToneMapPolicy};
use zenpixels::hdr::ContentLightLevel;
use zenpixels::{Cicp, PixelDescriptor, PixelSlice, TransferFunction};

/// One gray step per column, `H` rows.
const H: u32 = 4;

const PQ_BT2020: Cicp = Cicp::new(9, 16, 0, true);
const HLG_BT2020: Cicp = Cicp::new(9, 18, 0, true);

fn pq_code(nits: f32) -> f32 {
    linear_srgb::tf::linear_to_pq(nits / 10_000.0)
}

/// A gray ramp of f32 RGB code values, one column per value.
fn gray_rows(codes: &[f32]) -> Vec<u8> {
    let row: Vec<u8> = codes
        .iter()
        .flat_map(|&c| [c, c, c])
        .flat_map(f32::to_ne_bytes)
        .collect();
    row.repeat(H as usize)
}

fn desc(transfer: TransferFunction) -> PixelDescriptor {
    PixelDescriptor::RGBF32_LINEAR
        .with_transfer(transfer)
        .with_primaries(zenpixels::ColorPrimaries::Bt2020)
}

fn encode(format: ImageFormat, px: &[u8], w: u32, d: PixelDescriptor, meta: Metadata) -> Vec<u8> {
    let slice = PixelSlice::new(px, w, H, w as usize * 12, d).unwrap();
    EncodeRequest::new(format)
        .with_tone_map_policy(ToneMapPolicy::Bt2408)
        .with_metadata(meta)
        .encode(slice, false)
        .unwrap()
        .into_vec()
}

fn encode_streaming(
    format: ImageFormat,
    px: &[u8],
    w: u32,
    d: PixelDescriptor,
    meta: Metadata,
) -> Vec<u8> {
    encode_chunks(format, px, w, d, meta, 1)
}

fn encode_chunks(
    format: ImageFormat,
    px: &[u8],
    w: u32,
    d: PixelDescriptor,
    meta: Metadata,
    chunk: usize,
) -> Vec<u8> {
    let mut se = EncodeRequest::new(format)
        .with_tone_map_policy(ToneMapPolicy::Bt2408)
        .with_metadata(meta)
        .build_streaming_encoder(w, H)
        .unwrap();
    se.encoder.begin(w, H, d).unwrap();
    let stride = w as usize * 12;
    for y in (0..H as usize).step_by(chunk) {
        let rows = chunk.min(H as usize - y);
        let row = &px[y * stride..(y + rows) * stride];
        se.encoder
            .push_rows(PixelSlice::new(row, w, rows as u32, stride, d).unwrap())
            .unwrap();
    }
    let mut out = Vec::new();
    se.encoder.take_output(&mut out);
    out.extend_from_slice(se.encoder.finish().unwrap().data());
    out
}

/// The first row's red channel, after decoding to 8-bit.
fn decoded_first_row(bytes: &[u8], w: u32) -> Vec<u8> {
    let d = DecodeRequest::new(bytes).decode_full_frame().unwrap();
    assert_eq!(d.descriptor().channel_type(), zenpixels::ChannelType::U8);
    let bpp = d.descriptor().bytes_per_pixel();
    let row = d.pixels().row(0).to_vec();
    (0..w as usize).map(|x| row[x * bpp]).collect()
}

const NITS: [f32; 8] = [0.0, 1.0, 10.0, 50.0, 100.0, 203.0, 400.0, 1000.0];

fn pq_meta() -> Metadata {
    let mut m = Metadata::none().with_cicp(PQ_BT2020);
    m.content_light_level = Some(ContentLightLevel::new(1000, 400));
    m
}

#[test]
fn pq_into_bmp_is_tone_mapped_and_streaming_matches() {
    let codes: Vec<f32> = NITS.iter().map(|&n| pq_code(n)).collect();
    let px = gray_rows(&codes);
    let w = codes.len() as u32;
    let d = desc(TransferFunction::Pq);
    let bytes = encode(ImageFormat::Bmp, &px, w, d, pq_meta());
    let grays = decoded_first_row(&bytes, w);
    for pair in grays.windows(2) {
        assert!(pair[0] < pair[1], "gray ramp must increase: {grays:?}");
    }
    assert_eq!(grays[0], 0);
    assert_eq!(
        grays[7], 255,
        "MaxCLL (1000 nits) maps to SDR white: {grays:?}"
    );
    // Raw PQ code values shown as sRGB would put 203 nits at 148.
    assert!(
        grays[5] >= 200,
        "203-nit reference white is near 255: {grays:?}"
    );
    // The colour description is gone: the pixels are sRGB now.
    let info = DecodeRequest::new(&bytes).decode_full_frame().unwrap();
    assert!(info.info().source_color.cicp.is_none());

    // Same peak (from MaxCLL) on both paths: the same file.
    assert_eq!(
        encode_streaming(ImageFormat::Bmp, &px, w, d, pq_meta()),
        bytes
    );
}

#[test]
fn pq_into_gif_matches_the_bmp_tone_map() {
    let codes: Vec<f32> = NITS.iter().map(|&n| pq_code(n)).collect();
    let px = gray_rows(&codes);
    let w = codes.len() as u32;
    let d = desc(TransferFunction::Pq);
    let bmp = decoded_first_row(&encode(ImageFormat::Bmp, &px, w, d, pq_meta()), w);
    // Eight grays fit a palette exactly.
    let gif = decoded_first_row(&encode(ImageFormat::Gif, &px, w, d, pq_meta()), w);
    assert_eq!(gif, bmp);
}

#[cfg(feature = "jpeg")]
#[test]
fn pq_into_jpeg_is_tone_mapped() {
    // Wide columns so JPEG's 8x8 blocks are flat.
    let codes: Vec<f32> = NITS
        .iter()
        .flat_map(|&n| std::iter::repeat_n(pq_code(n), 16))
        .collect();
    let w = codes.len() as u32;
    let px = gray_rows(&codes);
    let d = desc(TransferFunction::Pq);
    let bmp = decoded_first_row(&encode(ImageFormat::Bmp, &px, w, d, pq_meta()), w);
    let jpeg = decoded_first_row(&encode(ImageFormat::Jpeg, &px, w, d, pq_meta()), w);
    for (i, (j, p)) in jpeg.iter().zip(&bmp).enumerate() {
        assert!(
            j.abs_diff(*p) <= 3,
            "column {i}: jpeg {j} vs tone-mapped {p}"
        );
    }
}

#[test]
fn png_keeps_pq() {
    let codes: Vec<f32> = NITS.iter().map(|&n| pq_code(n)).collect();
    let px = gray_rows(&codes);
    let w = codes.len() as u32;
    let bytes = encode(
        ImageFormat::Png,
        &px,
        w,
        desc(TransferFunction::Pq),
        pq_meta(),
    );
    let info = DecodeRequest::new(&bytes).decode_full_frame().unwrap();
    assert_eq!(info.info().source_color.cicp, Some(PQ_BT2020));
}

#[test]
fn hlg_reference_white_matches_pq_reference_white() {
    // HLG 0.75 is BT.2408's reference white (203 nits on the 1000-nit
    // reference display), as PQ 203 nits is.
    let hlg = gray_rows(&[0.0, 0.25, 0.5, 0.75, 1.0]);
    let bytes = encode(
        ImageFormat::Bmp,
        &hlg,
        5,
        desc(TransferFunction::Hlg),
        Metadata::none().with_cicp(HLG_BT2020),
    );
    let grays = decoded_first_row(&bytes, 5);
    for pair in grays.windows(2) {
        assert!(pair[0] < pair[1], "HLG ramp must increase: {grays:?}");
    }
    assert_eq!(grays[4], 255, "HLG peak maps to SDR white");

    let pq = gray_rows(&[pq_code(203.0)]);
    let mut meta = Metadata::none().with_cicp(PQ_BT2020);
    meta.content_light_level = Some(ContentLightLevel::new(1000, 400));
    let pq_white = decoded_first_row(
        &encode(ImageFormat::Bmp, &pq, 1, desc(TransferFunction::Pq), meta),
        1,
    )[0];
    assert!(
        grays[3].abs_diff(pq_white) <= 2,
        "HLG 0.75 gives {}, PQ 203 nits gives {pq_white}",
        grays[3]
    );

    // Streaming gives the same file (HLG's peak is fixed).
    assert_eq!(
        encode_streaming(
            ImageFormat::Bmp,
            &hlg,
            5,
            desc(TransferFunction::Hlg),
            Metadata::none().with_cicp(HLG_BT2020)
        ),
        bytes
    );
}

#[test]
fn a_pq_descriptor_without_metadata_is_tone_mapped() {
    // No colour metadata: both paths use the documented 1000-nit fallback.
    let codes: Vec<f32> = NITS.iter().map(|&n| pq_code(n)).collect();
    let px = gray_rows(&codes);
    let w = codes.len() as u32;
    let d = desc(TransferFunction::Pq);
    let bytes = encode(ImageFormat::Bmp, &px, w, d, Metadata::none());
    let grays = decoded_first_row(&bytes, w);
    assert_eq!(grays[7], 255);
    assert!(grays[5] >= 200, "{grays:?}");
    assert_eq!(
        encode_streaming(ImageFormat::Bmp, &px, w, d, Metadata::none()),
        bytes
    );
}

#[test]
fn feature_alone_does_not_authorize_tone_mapping() {
    let px = gray_rows(&[pq_code(203.0), pq_code(400.0)]);
    let d = desc(TransferFunction::Pq);
    for meta in [Metadata::none(), pq_meta()] {
        let request = || EncodeRequest::new(ImageFormat::Bmp).with_metadata(meta.clone());
        assert!(
            request()
                .encode(PixelSlice::new(&px, 2, H, 24, d).unwrap(), false)
                .is_err()
        );
        assert!(
            request()
                .with_tone_map_policy(ToneMapPolicy::Preserve)
                .encode(PixelSlice::new(&px, 2, H, 24, d).unwrap(), false)
                .is_err()
        );
        let built = request().build_streaming_encoder(2, H);
        if let Ok(mut se) = built {
            // Buffered codecs may reject only at finalization. A successful
            // file is forbidden unless rendering was explicitly authorized.
            let result = se.encoder.begin(2, H, d).and_then(|_| {
                se.encoder
                    .push_rows(PixelSlice::new(&px, 2, H, 24, d).unwrap())
            });
            let result = result.and_then(|_| se.encoder.finish().map(|_| ()));
            assert!(result.is_err());
        }
    }
}

#[test]
fn peak_and_chunking_do_not_change_rendering() {
    // Content varies by row; measured peaks differ from the fallback in both
    // directions, exposing image-wide or per-strip peak measurements.
    let px: Vec<u8> = [50.0, 203.0, 400.0, 4000.0]
        .into_iter()
        .flat_map(|n| [pq_code(n); 6])
        .flat_map(f32::to_ne_bytes)
        .collect();
    let d = desc(TransferFunction::Pq);
    let mut mastering = Metadata::none();
    let mut display = zenpixels::hdr::MasteringDisplay::HDR10_REFERENCE;
    display.max_luminance = 4000.0;
    mastering.mastering_display = Some(display);
    for meta in [
        Metadata::none(),
        Metadata::none().with_cicp(PQ_BT2020),
        pq_meta(),
        mastering,
    ] {
        let whole = encode(ImageFormat::Bmp, &px, 2, d, meta.clone());
        for chunk in [1, 2, 3, 4] {
            assert_eq!(
                whole,
                encode_chunks(ImageFormat::Bmp, &px, 2, d, meta.clone(), chunk)
            );
        }
    }
}

#[test]
fn strict_policy_refuses_tone_mapping_on_both_paths() {
    let px = gray_rows(&[pq_code(203.0)]);
    let d = desc(TransferFunction::Pq);
    for (format, meta) in [ImageFormat::Bmp, ImageFormat::Jpeg]
        .into_iter()
        .flat_map(|f| {
            [Metadata::none(), pq_meta()]
                .into_iter()
                .map(move |m| (f, m))
        })
    {
        let request = || {
            EncodeRequest::new(format)
                .with_metadata(meta.clone())
                .with_tone_map_policy(ToneMapPolicy::Bt2408)
                .with_encode_policy(zencodec::encode::EncodePolicy::none().with_strict(true))
        };
        assert!(
            request()
                .encode(PixelSlice::new(&px, 1, H, 12, d).unwrap(), false)
                .is_err()
        );
        if let Ok(mut se) = request().build_streaming_encoder(1, H) {
            assert!(se.encoder.begin(1, H, d).is_err());
        }
    }
}

#[test]
fn unsupported_hdr_color_is_an_error_on_both_paths() {
    let px = gray_rows(&[pq_code(203.0)]);
    let d = desc(TransferFunction::Pq);
    for cicp in [
        Cicp::new(11, 16, 0, true),
        Cicp::new(9, 16, 9, true),
        Cicp::new(9, 16, 0, false),
    ] {
        let request = || {
            EncodeRequest::new(ImageFormat::Bmp)
                .with_metadata(Metadata::none().with_cicp(cicp))
                .with_tone_map_policy(ToneMapPolicy::Bt2408)
        };
        assert!(
            request()
                .encode(PixelSlice::new(&px, 1, H, 12, d).unwrap(), false)
                .is_err()
        );
        let mut se = request().build_streaming_encoder(1, H).unwrap();
        assert!(se.encoder.begin(1, H, d).is_err());
    }
}

#[cfg(feature = "cms")]
#[test]
fn srgb_icc_takes_precedence_over_hdr_cicp() {
    let mut meta = Metadata::none().with_icc(zencodecs::cms::srgb_icc_profile());
    meta.cicp = Some(PQ_BT2020);
    let px = [30u8, 100, 200].repeat(3 * H as usize);
    let d = PixelDescriptor::RGB8_SRGB;
    let request = || EncodeRequest::new(ImageFormat::Bmp).with_metadata(meta.clone());
    let expected = request()
        .encode(PixelSlice::new(&px, 3, H, 9, d).unwrap(), false)
        .unwrap()
        .into_vec();
    let actual = request()
        .with_tone_map_policy(ToneMapPolicy::Bt2408)
        .encode(PixelSlice::new(&px, 3, H, 9, d).unwrap(), false)
        .unwrap()
        .into_vec();
    assert_eq!(actual, expected);
    let mut se = request()
        .with_tone_map_policy(ToneMapPolicy::Bt2408)
        .build_streaming_encoder(3, H)
        .unwrap();
    se.encoder.begin(3, H, d).unwrap();
    se.encoder
        .push_rows(PixelSlice::new(&px, 3, H, 9, d).unwrap())
        .unwrap();
    let mut streamed = Vec::new();
    se.encoder.take_output(&mut streamed);
    streamed.extend_from_slice(se.encoder.finish().unwrap().data());
    assert_eq!(streamed, expected);
}

#[test]
fn transcode_sink_reaches_tone_mapping_before_native_format_negotiation() {
    use zencodec::decode::DecodeRowSink;
    let px = gray_rows(&[pq_code(50.0), pq_code(203.0), pq_code(400.0)]);
    let d = desc(TransferFunction::Pq);
    for meta in [Metadata::none(), pq_meta()] {
        let expected = encode(ImageFormat::Bmp, &px, 3, d, meta.clone());
        let se = EncodeRequest::new(ImageFormat::Bmp)
            .with_metadata(meta)
            .with_tone_map_policy(ToneMapPolicy::Bt2408)
            .build_streaming_encoder(3, H)
            .unwrap();
        let mut sink = zencodecs::transcode::TranscodeSink::new(se.encoder, se.supported);
        sink.begin(3, H, d).unwrap();
        for y in 0..H {
            let mut strip = sink.provide_next_buffer(y, 1, 3, d).unwrap();
            strip
                .row_mut(0)
                .copy_from_slice(&px[y as usize * 36..(y as usize + 1) * 36]);
        }
        sink.finish().unwrap();
        let mut actual = Vec::new();
        sink.take_pending_output(&mut actual);
        actual.extend_from_slice(sink.finish_encode().unwrap().data());
        assert_eq!(actual, expected);
    }
}

#[cfg(feature = "webp")]
#[test]
fn srgba8_explicit_opacity_survives_the_tone_map_adapter() {
    let original = [80u8, 120, 160, 32].repeat(2 * H as usize);
    for opaque in [false, true] {
        let mut pixels = original.clone();
        let se = EncodeRequest::new(ImageFormat::WebP)
            .with_lossless(true)
            .with_tone_map_policy(ToneMapPolicy::Bt2408)
            .build_streaming_encoder(2, H)
            .unwrap();
        let encoded = se
            .encoder
            .encode_srgba8(&mut pixels, opaque, 2, H, 2)
            .unwrap();
        let decoded = DecodeRequest::new(encoded.data())
            .decode_full_frame()
            .unwrap();
        let px = decoded.pixels();
        let bpp = decoded.descriptor().bytes_per_pixel();
        for y in 0..H {
            for p in px.row(y).chunks_exact(bpp) {
                assert_eq!(
                    if bpp == 4 { p[3] } else { 255 },
                    if opaque { 255 } else { 32 }
                );
            }
        }
    }
    // The convenience API accepts U8 only. HDR metadata must cause the same
    // unsupported-sample refusal, regardless of the opaque request.
    for opaque in [false, true] {
        let mut pixels = original.clone();
        let se = EncodeRequest::new(ImageFormat::WebP)
            .with_lossless(true)
            .with_metadata(pq_meta())
            .with_tone_map_policy(ToneMapPolicy::Bt2408)
            .build_streaming_encoder(2, H)
            .unwrap();
        assert!(
            se.encoder
                .encode_srgba8(&mut pixels, opaque, 2, H, 2)
                .is_err()
        );
    }
}

#[cfg(feature = "webp")]
#[test]
fn hdr_padding_alpha_is_opaque_after_rendering() {
    let data: Vec<u8> = [pq_code(203.0), pq_code(203.0), pq_code(203.0), 0.25]
        .into_iter()
        .flat_map(f32::to_ne_bytes)
        .collect();
    let desc = PixelDescriptor::RGBAF32_LINEAR.with_transfer(TransferFunction::Pq);
    for alpha in [
        zenpixels::AlphaMode::Straight,
        zenpixels::AlphaMode::Undefined,
        zenpixels::AlphaMode::Opaque,
    ] {
        let desc = desc.with_alpha(Some(alpha));
        let encoded = EncodeRequest::new(ImageFormat::WebP)
            .with_lossless(true)
            .with_tone_map_policy(ToneMapPolicy::Bt2408)
            .encode(PixelSlice::new(&data, 1, 1, 16, desc).unwrap(), true)
            .unwrap();
        let decoded = DecodeRequest::new(encoded.data())
            .decode_full_frame()
            .unwrap();
        let expected = if alpha == zenpixels::AlphaMode::Straight {
            64
        } else {
            255
        };
        assert_eq!(
            if decoded.descriptor().bytes_per_pixel() == 4 {
                decoded.pixels().row(0)[3]
            } else {
                255
            },
            expected
        );
    }
}
