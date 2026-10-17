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
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};
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
    let mut se = EncodeRequest::new(format)
        .with_metadata(meta)
        .build_streaming_encoder(w, H)
        .unwrap();
    se.encoder.begin(w, H, d).unwrap();
    let stride = w as usize * 12;
    for y in 0..H as usize {
        let row = &px[y * stride..(y + 1) * stride];
        se.encoder
            .push_rows(PixelSlice::new(row, w, 1, stride, d).unwrap())
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
    // No colour metadata: the descriptor's transfer says PQ. The one-shot
    // encode measures the peak (1000 nits here) and agrees with a streaming
    // encode, whose default peak is also 1000 nits.
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
