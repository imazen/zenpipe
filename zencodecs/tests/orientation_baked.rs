//! An encode whose metadata asks for a rotation or flip writes the rotated
//! pixels with orientation 1 (imazen/zencodec#132: common readers ignore
//! orientation metadata on every target format). Checked pixel-exactly with
//! lossless targets, for every orientation, through the one-shot, streaming
//! and animation encoders.

#![cfg(feature = "png")]

use zencodec::{Metadata, Orientation};
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat};
use zenpixels::{PixelDescriptor, PixelSlice};

const W: u32 = 7;
const H: u32 = 4;

const ALL: [Orientation; 8] = [
    Orientation::Identity,
    Orientation::FlipH,
    Orientation::Rotate180,
    Orientation::FlipV,
    Orientation::Transpose,
    Orientation::Rotate90,
    Orientation::Transverse,
    Orientation::Rotate270,
];

/// Every pixel distinct, so any misplaced pixel shows.
fn source(frame: u8) -> Vec<u8> {
    (0..W * H)
        .flat_map(|i| [(i * 9) as u8, frame.wrapping_mul(50), 255 - i as u8])
        .collect()
}

fn slice(px: &[u8]) -> PixelSlice<'_> {
    PixelSlice::new(px, W, H, W as usize * 3, PixelDescriptor::RGB8_SRGB).unwrap()
}

/// `px` as it displays under `o`.
fn displayed(px: &[u8], o: Orientation) -> (u32, u32, Vec<[u8; 3]>) {
    let (dw, dh) = o.output_dimensions(W, H);
    let mut out = vec![[0u8; 3]; (dw * dh) as usize];
    for y in 0..H {
        for x in 0..W {
            let (dx, dy) = o.forward_map(x, y, W, H);
            let i = ((y * W + x) * 3) as usize;
            out[(dy * dw + dx) as usize] = [px[i], px[i + 1], px[i + 2]];
        }
    }
    (dw, dh, out)
}

/// RGB samples of a decoded RGB8 or RGBA8 image.
fn rgb(px: &PixelSlice<'_>) -> (u32, u32, Vec<[u8; 3]>) {
    let n = px.descriptor().layout().channels();
    assert!(n == 3 || n == 4, "{:?}", px.descriptor());
    let mut out = Vec::new();
    for y in 0..px.rows() {
        let row = px.row(y);
        for x in 0..px.width() as usize {
            out.push([row[x * n], row[x * n + 1], row[x * n + 2]]);
        }
    }
    (px.width(), px.rows(), out)
}

// `Exif::to_bytes` is deprecated on zencodec main; the registry zencodec
// this crate builds against has no other writer.
#[allow(deprecated)]
fn exif_with(o: Orientation) -> Vec<u8> {
    let mut e = zencodec::exif::Exif::new(zencodec::exif::TextEncoding::Ascii);
    e.set_orientation(o);
    e.to_bytes()
}

fn check_upright(bytes: &[u8], want: &(u32, u32, Vec<[u8; 3]>), label: &str) {
    let d = DecodeRequest::new(bytes).decode_full_frame().unwrap();
    assert_eq!(d.info().orientation, Orientation::Identity, "{label}");
    assert_eq!(&rgb(&d.pixels()), want, "{label}: displayed pixels");
    if let Some(exif) = d.metadata().exif {
        assert_eq!(
            zencodec::helpers::parse_exif_orientation(&exif),
            Some(Orientation::Identity),
            "{label}: the EXIF tag is rewritten to 1"
        );
    }
}

#[test]
fn one_shot_and_streaming_encodes_write_every_orientation_upright() {
    let px = source(0);
    for o in ALL {
        let want = displayed(&px, o);
        for (how, meta) in [
            ("field", Metadata::none().with_orientation(o)),
            ("exif", Metadata::none().with_exif(exif_with(o))),
        ] {
            let one_shot = EncodeRequest::new(ImageFormat::Png)
                .with_metadata(meta.clone())
                .encode(slice(&px), false)
                .unwrap()
                .into_vec();
            check_upright(&one_shot, &want, &format!("{o:?} {how} one-shot"));

            let mut se = EncodeRequest::new(ImageFormat::Png)
                .with_metadata(meta)
                .build_streaming_encoder(W, H)
                .unwrap();
            se.encoder.begin(W, H, PixelDescriptor::RGB8_SRGB).unwrap();
            for y in 0..H {
                let row = &px[(y * W * 3) as usize..((y + 1) * W * 3) as usize];
                se.encoder
                    .push_rows(
                        PixelSlice::new(row, W, 1, row.len(), PixelDescriptor::RGB8_SRGB).unwrap(),
                    )
                    .unwrap();
            }
            let mut streamed = Vec::new();
            se.encoder.take_output(&mut streamed);
            streamed.extend_from_slice(se.encoder.finish().unwrap().data());
            check_upright(&streamed, &want, &format!("{o:?} {how} streaming"));
        }
    }
}

#[test]
fn animation_encodes_write_every_frame_upright() {
    #[allow(unused_mut)]
    let mut formats = vec![ImageFormat::Png];
    #[cfg(feature = "gif")]
    formats.push(ImageFormat::Gif);
    let frames: Vec<Vec<u8>> = (0..3).map(source).collect();
    for format in formats {
        for o in ALL {
            let label = format!("{format:?} {o:?}");
            let mut enc = EncodeRequest::new(format)
                .with_lossless(true)
                .with_metadata(Metadata::none().with_orientation(o))
                .animation_frame_encoder(W, H)
                .unwrap();
            for f in &frames {
                enc.push_frame(slice(f), 100, None).unwrap();
            }
            let bytes = enc.finish(None).unwrap().into_vec();

            let mut dec = DecodeRequest::new(&bytes)
                .animation_frame_decoder()
                .unwrap();
            let mut n = 0;
            while let Some(frame) = dec.render_next_frame_owned(None).unwrap() {
                assert_eq!(
                    rgb(&frame.pixels()),
                    displayed(&frames[n], o),
                    "{label}: frame {n}"
                );
                n += 1;
            }
            assert_eq!(n, frames.len(), "{label}: frame count");
            let info = DecodeRequest::new(&bytes).probe().unwrap();
            assert_eq!(info.orientation, Orientation::Identity, "{label}");
        }
    }
}

#[test]
fn xmp_orientation_is_rewritten_with_the_exif_tag() {
    let px = source(0);
    let xmp = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:tiff="http://ns.adobe.com/tiff/1.0/" tiff:Orientation="6"/></rdf:RDF></x:xmpmeta>"#;
    let meta = Metadata::none()
        .with_orientation(Orientation::Rotate90)
        .with_xmp(xmp.to_vec());
    let bytes = EncodeRequest::new(ImageFormat::Png)
        .with_metadata(meta)
        .encode(slice(&px), false)
        .unwrap()
        .into_vec();
    let d = DecodeRequest::new(&bytes).decode_full_frame().unwrap();
    let got = d.metadata().xmp.expect("XMP kept");
    let text = core::str::from_utf8(&got).unwrap();
    assert!(text.contains(r#"tiff:Orientation="1""#), "{text}");
    assert_eq!(got.len(), xmp.len(), "same-length rewrite");
    check_upright(&bytes, &displayed(&px, Orientation::Rotate90), "xmp");
}
