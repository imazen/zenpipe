//! Diagnostics of the current b25 contract, independent of the two retained
//! tag-carriage assertions in the parent suite. These witnesses do not resolve
//! which encode representation the shared metadata contract should select.
use super::*;
use zencodec::encode::{EncodeJob as _, Encoder as _, EncoderConfig as _};
use zencodec::{MetadataPolicy, Orientation, OrientationHint};
use zencodecs::PixelBufferConvertTypedExt as _;

fn asymmetric() -> ImgVec<Rgb<u8>> {
    ImgVec::new(
        (0..24)
            .flat_map(|y| {
                (0..32).map(move |x| {
                    let value = ((x * 7 + y * 11) % 256) as u8;
                    Rgb::new(value, value, value)
                })
            })
            .collect(),
        32,
        24,
    )
}

fn encode(image: &ImgVec<Rgb<u8>>, meta: Metadata, tagged: bool) -> Vec<u8> {
    let mut config = zenavif::AvifEncoderConfig::new()
        .with_lossless_mode(true)
        .with_effort_u32(10);
    if tagged {
        // The actual serializer uses quarter turns (3 = 270 degrees CCW).
        // This native escape hatch is not a codec-agnostic metadata option.
        let native = config.inner().clone().rotation(3);
        *config.inner_mut() = native;
    }
    let typed: PixelSlice<'_, Rgb<u8>> = PixelSlice::from(image.as_ref());
    config
        .job()
        .with_metadata_policy(meta, MetadataPolicy::PreserveExact)
        .encoder()
        .unwrap()
        .encode(typed.erase())
        .unwrap()
        .into_vec()
}

fn rgba(bytes: &[u8], hint: OrientationHint) -> (u32, u32, Orientation, Vec<u8>) {
    let decoded = DecodeRequest::new(bytes)
        .with_orientation(hint)
        .decode_full_frame()
        .unwrap();
    let result = (
        decoded.width(),
        decoded.height(),
        decoded.info().orientation,
    );
    let pixels = decoded.into_buffer().to_rgba8().copy_to_contiguous_bytes();
    (result.0, result.1, result.2, pixels)
}

fn rotate_clockwise(samples: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut rotated = Vec::with_capacity(samples.len());
    for y in 0..width {
        for x in 0..height {
            let index = ((height - 1 - x) * width + y) * 4;
            rotated.extend_from_slice(&samples[index..index + 4]);
        }
    }
    rotated
}

fn baked_witness(meta: Metadata, has_exif: bool) {
    let image = asymmetric();
    let baseline = encode(&image, Metadata::none(), false);
    let raw = rgba(&baseline, OrientationHint::Preserve);
    assert_eq!((raw.0, raw.1, raw.2), (32, 24, Orientation::Identity));
    assert_eq!(meta.orientation, Orientation::Rotate90);
    let bytes = encode(&image, meta, false);
    let preserved = rgba(&bytes, OrientationHint::Preserve);
    assert_eq!((preserved.0, preserved.1), (24, 32));
    assert_eq!(preserved.2, Orientation::Identity);
    assert_eq!(preserved.3, rotate_clockwise(&raw.3, 32, 24));
    // Correct must not apply the input orientation a second time.
    assert_eq!(rgba(&bytes, OrientationHint::Correct), preserved);
    // Inspect coded dimensions and container properties separately from the
    // decoder's reported orientation. This is not a third-party decoder oracle.
    let parsed = zenavif_parse::AvifParser::from_bytes(&bytes).unwrap();
    let coded = parsed.primary_metadata().unwrap();
    assert_eq!(
        (coded.max_frame_width.get(), coded.max_frame_height.get()),
        (24, 32)
    );
    assert!(parsed.rotation().is_none());
    assert!(parsed.mirror().is_none());
    if has_exif {
        // Keep this strict: the known serializer EXIF-prefix issue must remain
        // visible if it prevents independent tag inspection.
        let exif = parsed.exif().expect("EXIF item retained").unwrap();
        assert_eq!(
            zencodec::helpers::parse_exif_orientation(&exif),
            Some(Orientation::Identity)
        );
    }
}

#[test]
fn explicit_orientation_pixel_witness() {
    baked_witness(
        Metadata::none().with_orientation(Orientation::Rotate90),
        false,
    );
}

#[test]
fn exif_only_orientation_pixel_witness() {
    baked_witness(
        Metadata::none().with_exif(build_minimal_exif_with_orientation(6)),
        true,
    );
}

#[test]
fn native_pass_through_orientation_pixel_witness() {
    let image = asymmetric();
    let baseline = encode(&image, Metadata::none(), false);
    let raw = rgba(&baseline, OrientationHint::Preserve);
    let tagged = encode(&image, Metadata::none(), true);
    let parsed = zenavif_parse::AvifParser::from_bytes(&tagged).unwrap();
    assert_eq!(parsed.rotation().expect("irot carrier").angle, 270);
    assert!(parsed.mirror().is_none());
    let coded = parsed.primary_metadata().unwrap();
    assert_eq!(
        (coded.max_frame_width.get(), coded.max_frame_height.get()),
        (32, 24)
    );
    let preserved = rgba(&tagged, OrientationHint::Preserve);
    assert_eq!(
        (preserved.0, preserved.1, preserved.2),
        (32, 24, Orientation::Rotate90)
    );
    assert_eq!(preserved.3, raw.3);
    let corrected = rgba(&tagged, OrientationHint::Correct);
    assert_eq!(
        (corrected.0, corrected.1, corrected.2),
        (24, 32, Orientation::Identity)
    );
    assert_eq!(corrected.3, rotate_clockwise(&raw.3, 32, 24));
}
