//! Native density carriage, independently read by png, with exact pixel checks.
use super::*;
use zencodec::{Resolution, ResolutionUnit};

fn density(x: f64, y: f64, unit: ResolutionUnit) -> Resolution {
    Resolution { x, y, unit }
}
fn png_source(resolution: Resolution, metadata: Metadata) -> Vec<u8> {
    let bytes = [19, 83, 201, 23, 151, 43, 255, 255, 255].repeat(9);
    let pixels = PixelSlice::new(&bytes, 2, 9, 9, PixelDescriptor::RGB8_SRGB).unwrap();
    zenpng::PngEncoderConfig::new()
        .job()
        .with_metadata_policy(metadata, MetadataPolicy::PreserveExact)
        .try_with_resolution(resolution)
        .unwrap()
        .encoder()
        .unwrap()
        .encode(pixels)
        .unwrap()
        .data()
        .to_vec()
}
fn inspect(bytes: &[u8], expected: Resolution, pixels: &[u8]) {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().unwrap();
    let phys = reader
        .info()
        .pixel_dims
        .expect("independent reader lost pHYs");
    let unit = match phys.unit {
        png::Unit::Meter => ResolutionUnit::Meter,
        png::Unit::Unspecified => ResolutionUnit::Unknown,
    };
    assert!(expected.is_equivalent_to(&density(phys.xppu.into(), phys.yppu.into(), unit)));
    let mut actual = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut actual).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgb);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    assert_eq!(&actual[..info.buffer_size()], pixels);
}

#[test]
fn native_png_density_keeps_pixels_and_prepared_offers_across_strips() {
    let codecs = codecs();
    for required in [
        density(254.0, 127.0, ResolutionUnit::Inch),
        density(100.0, 50.0, ResolutionUnit::Centimeter),
        density(3780.0, 7559.0, ResolutionUnit::Meter),
        density(0.5, 0.25, ResolutionUnit::Unknown),
        density(f64::from(i32::MAX), 1.0, ResolutionUnit::Meter),
    ] {
        let input = png_source(required, Metadata::none());
        for rows in [1, 3, 64] {
            let budget = StorageBudget::new(512 * 1024, 16);
            let mut calls = 0;
            let output = TranscodeRequest::new(ImageFormat::Png)
                .with_storage(budget.clone())
                .with_strip_rows(rows)
                .transcode_with(&codecs, &input, &mut |q: Question<'_>| -> Result<
                    Decision,
                    At<Infallible>,
                > {
                    if let Question::Transcode { choices, .. } = q {
                        calls += 1;
                        for choice in choices.iter() {
                            assert!(
                                required
                                    .is_equivalent_to(&choice.value().encoder.resolution.unwrap())
                            );
                        }
                    }
                    Ok(Decision::UsePreset)
                })
                .unwrap_or_else(|e| panic!("{}", explain(&e)));
            assert_eq!(calls, 1);
            inspect(
                output.data(),
                required,
                &[19, 83, 201, 23, 151, 43].repeat(9),
            );
            assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
        }
    }
}

#[test]
fn native_jfif_density_is_converted_exactly_to_png_units() {
    use zenjpeg::decoder::{DensityUnits, JfifInfo};
    use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout};
    let codecs = codecs();
    for (units, x, y, expected) in [
        (
            DensityUnits::PixelsPerInch,
            254,
            127,
            density(10000.0, 5000.0, ResolutionUnit::Meter),
        ),
        (
            DensityUnits::PixelsPerCm,
            100,
            50,
            density(10000.0, 5000.0, ResolutionUnit::Meter),
        ),
        (
            DensityUnits::None,
            120,
            80,
            density(3.0, 2.0, ResolutionUnit::Unknown),
        ),
    ] {
        let native = EncoderConfig::ycbcr(90.0, ChromaSubsampling::Quarter);
        let segments = native
            .get_segments()
            .cloned()
            .unwrap_or_default()
            .set_jfif(JfifInfo {
                version_major: 1,
                version_minor: 2,
                density_units: units,
                x_density: x,
                y_density: y,
            });
        let input = native
            .with_segments(segments)
            .encode_bytes(&[19, 83, 201].repeat(17 * 9), 17, 9, PixelLayout::Rgb8Srgb)
            .unwrap();
        let decoded = zencodec::operation::DecodeRequest::new()
            .decode(&codecs, &input)
            .unwrap();
        assert!(
            expected.is_equivalent_to(
                &decoded
                    .info()
                    .resolution
                    .expect("native source omitted density/aspect")
            )
        );
        let decoded_pixels = decoded.pixels();
        let pixels: Vec<_> = (0..9)
            .flat_map(|y| decoded_pixels.row(y).iter().copied())
            .collect();
        let output = TranscodeRequest::new(ImageFormat::Png)
            .transcode(&codecs, &input)
            .unwrap_or_else(|e| panic!("{}", explain(&e)));
        inspect(output.data(), expected, &pixels);
    }
}

#[test]
fn unqualified_duplicate_density_refuses_before_joint_policy_and_explicit_discard_still_works() {
    let codecs = codecs();
    let required = density(10000.0, 5000.0, ResolutionUnit::Meter);
    #[allow(deprecated)]
    let exif = {
        let mut exif = zencodec::exif::Exif::new(zencodec::exif::TextEncoding::Ascii);
        exif.set_artist(SECRET);
        exif.to_bytes()
    };
    for metadata in [
        Metadata::none().with_exif(exif),
        Metadata::none().with_xmp(b"<unparseable-private>".to_vec()),
        Metadata::none().with_xmp(br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:t="http://ns.adobe.com/tiff/1.0/" t:XResolution="300/1" t:YResolution="150/1" t:ResolutionUnit="2"/></rdf:RDF></x:xmpmeta>"#.to_vec()),
    ] {
        let input = png_source(required, metadata);
        let budget = StorageBudget::new(512 * 1024, 16);
        let request = TranscodeRequest::new(ImageFormat::Png).with_storage(budget.clone());
        let mut calls = 0;
        let error = request.transcode_with(&codecs, &input, &mut |question: Question<'_>| -> Result<Decision, At<Infallible>> {
            if matches!(question, Question::Transcode { .. }) {
                calls += 1;
            }
            Ok(Decision::UsePreset)
        }).unwrap_err();
        assert_eq!(calls, 0);
        assert_eq!(error.error().stage(), Stage::Plan);
        assert_eq!(zencodec::find_cause::<zencodecs::checked_transcode::Refusal>(&error), Some(&zencodecs::checked_transcode::Refusal::ResolutionMetadata));
        assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
        let output = request.with_metadata(MetadataTransfer::DiscardDescriptive).transcode(&codecs, &input).unwrap();
        let reader = png::Decoder::new(Cursor::new(output.data())).read_info().unwrap();
        assert!(reader.info().pixel_dims.is_none());
        assert!(reader.info().exif_metadata.is_none());
        assert!(reader.info().utf8_text.is_empty());
    }
}

#[cfg(feature = "checked-transcode-async")]
#[test]
fn density_is_retained_across_pending_network_reads() {
    let required = density(3780.0, 7559.0, ResolutionUnit::Meter);
    let input = png_source(required, Metadata::none());
    let codecs = codecs();
    let request = TranscodeRequest::new(ImageFormat::Png);
    let expected = request.transcode(&codecs, &input).unwrap();
    for chunk in [1, 7, 97] {
        for known in [false, true] {
            let mut network = super::asynchronous::Network::new(&input, chunk, known);
            let (actual, pending) =
                super::asynchronous::drive(request.transcode_async(&codecs, &mut network));
            assert!(pending > 1);
            assert_eq!(actual.unwrap().data(), expected.data());
        }
    }
}
