//! Container-level privacy and cost contracts. Tiny framing fixtures intentionally
//! isolate rewriting from image decoding; real-codec roundtrips are separate.
#![cfg(feature = "metadata")]

use zencodecs::metadata::{Action, Error, ScrubRequest};

#[test]
fn rejects_unknown_formats_and_enforces_input_budget_before_parsing() {
    assert!(matches!(
        ScrubRequest::new(b"secret").plan(),
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        ScrubRequest::new(b"secret").with_byte_limits(2, 100).plan(),
        Err(Error::Limit(_))
    ));
}

#[cfg(feature = "jpeg")]
mod jpeg {
    use super::*;
    fn app(marker: u8, bytes: &[u8]) -> Vec<u8> {
        let mut out = vec![255, marker];
        out.extend_from_slice(&((bytes.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(bytes);
        out
    }
    fn image(metadata: &[u8]) -> Vec<u8> {
        [
            &[255, 216][..],
            metadata,
            &[255, 218, 0, 2, 17, 255, 0, 29, 255, 217],
        ]
        .concat()
    }
    #[test]
    fn removes_private_packets_between_scans_and_trailer_without_copying_scan() {
        let mut input = image(&app(0xfe, b"SECRET-COMMENT"));
        input.extend_from_slice(b"SECRET-TRAILER");
        let request = ScrubRequest::new(&input);
        let plan = request.plan().unwrap();
        assert_eq!(plan.to_vec().unwrap(), image(&[]));
        assert_eq!(
            plan.output_len(),
            plan.chunks().map(<[u8]>::len).sum::<usize>()
        );
        assert!(plan.chunks().any(|c| c.as_ptr() == input[20..].as_ptr()));
        assert!(plan.report().iter().any(|r| r.action() == Action::Remove));
        let mut stream = Vec::new();
        for bytes in plan.chunks() {
            stream.extend_from_slice(bytes);
        }
        assert_eq!(stream, plan.to_vec().unwrap());
    }
    #[test]
    fn retains_orientation_removes_identity_and_refuses_duplicate_exif() {
        let mut exif = zencodec::exif::Exif::new(zencodec::exif::TextEncoding::Ascii);
        exif.set_orientation(zencodec::Orientation::Rotate90);
        exif.set_artist("SECRET-PERSON");
        let tiff = exif.to_bytes();
        let exif = if tiff.starts_with(b"Exif\0\0") {
            tiff
        } else {
            [b"Exif\0\0".as_slice(), &tiff].concat()
        };
        let segment = app(0xe1, &exif);
        let input = image(&segment);
        let output = ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap();
        assert!(!output.windows(6).any(|w| w == b"SECRET"));
        let span = zenjpeg::container::iter(&output)
            .find(|s| s.payload.starts_with(b"Exif\0\0"))
            .unwrap();
        assert_eq!(
            zencodec::exif::Exif::parse(span.payload)
                .unwrap()
                .orientation(),
            Some(zencodec::Orientation::Rotate90)
        );
        assert!(
            ScrubRequest::new(&image(&[&segment[..], &segment].concat()))
                .plan()
                .is_err()
        );
    }
    #[test]
    fn damaged_orientation_and_cyclic_exif_are_not_silently_salvaged() {
        let valid = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        for index in [12, 18, 22] {
            let mut damaged = valid.to_vec();
            damaged[index] = if index == 22 { 8 } else { 255 };
            let input = image(&app(0xe1, &[b"Exif\0\0".as_slice(), &damaged].concat()));
            assert!(ScrubRequest::new(&input).plan().is_err(), "{index}");
        }
    }
    #[test]
    fn profiles_are_explicitly_opaque_and_incomplete_chunks_refuse() {
        let input = image(&app(0xe2, b"ICC_PROFILE\0\x01\x01opaque-identity"));
        let request = ScrubRequest::new(&input);
        let plan = request.plan().unwrap();
        assert_eq!(plan.to_vec().unwrap(), input);
        assert!(
            plan.report()
                .iter()
                .any(|r| r.action() == Action::OpaqueIcc)
        );
        let input = image(&app(0xe2, b"ICC_PROFILE\0\x01\x02incomplete"));
        assert!(ScrubRequest::new(&input).plan().is_err());
    }
    #[test]
    fn truncations_and_resource_budgets_never_produce_partial_plans() {
        let input = image(&app(0xfe, b"private"));
        for end in 0..input.len() {
            assert!(ScrubRequest::new(&input[..end]).plan().is_err(), "{end}");
        }
        assert!(matches!(
            ScrubRequest::new(&input).with_byte_limits(100, 2).plan(),
            Err(Error::Limit(_))
        ));
        assert!(matches!(
            ScrubRequest::new(&input).with_carrier_limit(1).plan(),
            Err(Error::Limit(_))
        ));
    }
    #[test]
    fn does_not_silently_drop_gain_maps_or_indexed_images() {
        let input = image(&app(0xe2, b"MPF\0not-a-valid-index"));
        assert!(ScrubRequest::new(&input).plan().is_err());
        assert!(
            ScrubRequest::new(&[image(&[]), image(&[])].concat())
                .plan()
                .is_err()
        );
    }
}

mod png_scrub {
    use super::*;
    pub fn chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut c = !0u32;
        for &b in &out[4..] {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xedb8_8320
                } else {
                    c >> 1
                };
            }
        }
        out.extend_from_slice(&(!c).to_be_bytes());
        out
    }
    pub fn fixture(extra: &[u8], sixteen: bool) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(if sixteen {
                png::BitDepth::Sixteen
            } else {
                png::BitDepth::Eight
            });
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(if sixteen {
                    &[0, 1, 0x7f, 0xff, 0xff, 0xff, 0x55, 0x55, 0xaa, 0xaa, 0, 2]
                } else {
                    &[0, 1, 2, 3, 4, 255]
                })
                .unwrap();
        }
        out.splice(33..33, extra.iter().copied());
        out
    }
    fn pixels(bytes: &[u8]) -> Vec<u8> {
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .unwrap();
        let mut out = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut out).unwrap();
        out.truncate(info.buffer_size());
        out
    }
    #[test]
    fn removes_all_private_carriers_and_trailer_preserving_sdr_samples() {
        let extras = [
            chunk(b"tEXt", b"Author\0SECRET"),
            chunk(b"zTXt", b"SECRET-compressed-not-inflated"),
            chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0SECRET"),
            chunk(b"vpAg", b"SECRET-unknown"),
        ]
        .concat();
        let mut input = fixture(&extras, false);
        input.extend_from_slice(b"SECRET-trailer");
        let output = ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap();
        assert!(!output.windows(6).any(|w| w == b"SECRET"));
        assert_eq!(pixels(&input), pixels(&output));
        assert_eq!(output, fixture(&[], false));
    }
    #[test]
    fn hdr_signals_and_sixteen_bit_samples_survive_exactly() {
        for transfer in [16, 18] {
            let extras = [
                chunk(b"cICP", &[9, transfer, 0, 1]),
                chunk(b"mDCV", &[0; 24]),
                chunk(b"cLLI", &[0; 8]),
            ]
            .concat();
            let input = fixture(&extras, true);
            let output = ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap();
            assert_eq!(input, output);
            assert_eq!(pixels(&input), pixels(&output));
            assert_eq!(
                pixels(&output),
                [0, 1, 0x7f, 0xff, 0xff, 0xff, 0x55, 0x55, 0xaa, 0xaa, 0, 2]
            );
        }
    }
    #[test]
    fn unknown_color_is_never_relabelled() {
        let input = fixture(&chunk(b"cICP", &[255, 255, 0, 1]), true);
        assert_eq!(
            ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap(),
            input
        );
    }
    #[test]
    fn invalid_crc_duplicates_critical_extensions_and_hdr_contracts_refuse() {
        for extra in [
            chunk(b"ABCD", b"unknown-critical"),
            [chunk(b"sRGB", &[0]), chunk(b"sRGB", &[0])].concat(),
            chunk(b"cICP", &[9, 16, 9, 0]),
            chunk(b"mDCV", &[0; 24]),
        ] {
            assert!(ScrubRequest::new(&fixture(&extra, false)).plan().is_err());
        }
        let mut input = fixture(&[], false);
        input[29] ^= 1;
        assert!(matches!(
            ScrubRequest::new(&input).plan(),
            Err(Error::Malformed("PNG CRC"))
        ));
    }
    #[test]
    fn every_truncated_prefix_refuses() {
        let input = fixture(&[], false);
        for end in 0..input.len() {
            assert!(ScrubRequest::new(&input[..end]).plan().is_err(), "{end}");
        }
    }
    #[test]
    fn compressed_icc_is_opaque_and_profile_name_is_scrubbed_without_inflation() {
        let input = fixture(
            &chunk(b"iCCP", b"SECRET profile name\0\0opaque-zlib"),
            false,
        );
        let req = ScrubRequest::new(&input);
        let plan = req.plan().unwrap();
        assert!(
            plan.report()
                .iter()
                .any(|e| e.action() == Action::OpaqueIcc)
        );
        let output = plan.to_vec().unwrap();
        assert!(!output.windows(6).any(|w| w == b"SECRET"));
        assert!(output.windows(11).any(|w| w == b"opaque-zlib"));
        assert!(
            ScrubRequest::new(&input)
                .with_icc_normalization(true)
                .plan()
                .is_err()
        );
    }
    #[test]
    fn animation_all_frames_and_timing_survive() {
        let mut input = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut input, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            encoder.set_animated(2, 3).unwrap();
            encoder.set_frame_delay(17, 1000).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&[0, 1, 0, 2, 0, 3, 255, 255])
                .unwrap();
            writer.set_frame_delay(19, 1000).unwrap();
            writer
                .write_image_data(&[0, 4, 0, 5, 0, 6, 255, 255])
                .unwrap();
        }
        let expected = input.clone();
        input.splice(33..33, chunk(b"tEXt", b"Author\0SECRET"));
        let output = ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap();
        assert_eq!(output, expected);
        let mut reader = png::Decoder::new(std::io::Cursor::new(&output))
            .read_info()
            .unwrap();
        for (delay, want) in [
            (17, [0, 1, 0, 2, 0, 3, 255, 255]),
            (19, [0, 4, 0, 5, 0, 6, 255, 255]),
        ] {
            let mut buf = [0; 8];
            reader.next_frame(&mut buf).unwrap();
            assert_eq!(buf, want);
            assert_eq!(
                reader.info().frame_control.as_ref().unwrap().delay_num,
                delay
            );
        }
    }
}

mod jxl_scrub {
    use super::*;
    pub fn box_(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        [(data.len() as u32 + 8).to_be_bytes().as_slice(), kind, data].concat()
    }
    pub fn fixture(extras: &[u8]) -> Vec<u8> {
        [
            b"\0\0\0\x0cJXL \r\n\x87\n".as_slice(),
            &box_(b"ftyp", b"jxl \0\0\0\0jxl "),
            extras,
            &box_(b"jxlc", b"\xff\x0aopaque-codestream"),
        ]
        .concat()
    }
    #[test]
    fn removes_plain_and_compressed_private_packets_without_decompressing() {
        let extra = [box_(b"xml ", b"SECRET"), box_(b"zzzz", b"SECRET")].concat();
        let input = fixture(&extra);
        let out = ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap();
        assert_eq!(out, fixture(&[]));
        let input = fixture(&box_(b"brob", b"xml SECRET-not-valid-brotli"));
        assert_eq!(
            ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap(),
            fixture(&[])
        );
    }
    #[test]
    fn reconstruction_loss_requires_explicit_opt_in_even_when_compressed() {
        for b in [
            box_(b"jbrd", b"SECRET-reconstruction"),
            box_(b"brob", b"jbrdSECRET-reconstruction"),
        ] {
            let input = fixture(&b);
            assert!(ScrubRequest::new(&input).plan().is_err());
            let req = ScrubRequest::new(&input).with_jpeg_reconstruction_removal(true);
            let plan = req.plan().unwrap();
            assert!(
                plan.report()
                    .iter()
                    .any(|r| r.action() == Action::RemoveJpegReconstruction)
            );
            assert_eq!(plan.to_vec().unwrap(), fixture(&[]));
        }
    }
    #[test]
    fn malformed_box_sizes_and_partial_codestream_sequences_refuse() {
        for extra in [
            b"\0\0\0\x04zzzz".to_vec(),
            box_(b"jxlp", b"\x80\0\0\x02X"),
            box_(b"brob", b"jxlcX"),
            box_(b"jxlc", b"duplicate"),
        ] {
            assert!(ScrubRequest::new(&fixture(&extra)).plan().is_err());
        }
        let input = fixture(&[]);
        for end in 0..input.len() {
            assert!(ScrubRequest::new(&input[..end]).plan().is_err(), "{end}");
        }
    }
    #[test]
    fn compressed_exif_requires_bounded_decompression() {
        let input = fixture(&box_(b"brob", b"Exifcompressed"));
        assert!(matches!(
            ScrubRequest::new(&input).plan(),
            Err(Error::Unsupported(_))
        ));
    }
}

#[test]
fn cancellation_is_observed_before_parsing() {
    struct Stop;
    impl enough::Stop for Stop {
        fn check(&self) -> Result<(), enough::StopReason> {
            Err(enough::StopReason::Cancelled)
        }
    }
    assert_eq!(
        ScrubRequest::new(b"data")
            .with_stop(&Stop)
            .plan()
            .unwrap_err(),
        Error::Cancelled
    );
}

#[cfg(feature = "jpeg-ultrahdr")]
mod gain_map_scrub {
    use super::*;
    use zenjpeg::container::{MarkerKind, MpImageType, marker, mpf};
    fn source() -> Vec<u8> {
        source_format(zencodecs::ImageFormat::Jpeg)
    }
    fn source_format(format: zencodecs::ImageFormat) -> Vec<u8> {
        let pixels: Vec<u8> = (0..16 * 16 * 3)
            .map(|i| ((i * 37 + i / 7 * 53) % 256) as u8)
            .collect();
        let gain = zencodecs::gainmap::GainMap {
            width: 4,
            height: 4,
            channels: 1,
            data: vec![128; 16],
        };
        let mut params = zencodecs::gainmap::GainMapMetadata::default();
        params.alternate_hdr_headroom = 2.0;
        for ch in &mut params.channels {
            ch.max = 2.0;
        }
        let view =
            zenpixels::PixelSlice::new(&pixels, 16, 16, 48, zenpixels::PixelDescriptor::RGB8_SRGB)
                .unwrap();
        zencodecs::EncodeRequest::new(format)
            .with_metadata_policy(zencodec::MetadataPolicy::ColorAndRotation)
            .with_gain_map(zencodecs::gainmap::GainMapSource::Precomputed {
                gain_map: &gain,
                metadata: &params,
            })
            .encode(view, false)
            .unwrap()
            .into_vec()
    }
    fn add_private_comments(jpeg: &[u8]) -> Vec<u8> {
        let entries = mpf::parse_mpf(jpeg).unwrap();
        let private = b"\xff\xfe\0\x08SECRET";
        let gain = &jpeg[entries[1].offset..entries[1].offset + entries[1].size];
        let gain = [&gain[..2], private, &gain[2..]].concat();
        let mut base = vec![255, 216];
        base.extend_from_slice(private);
        let mut mpf_offset = 0;
        for span in marker::iter(&jpeg[..entries[0].size]).skip(1) {
            if span.payload.starts_with(b"MPF\0") {
                mpf_offset = base.len();
                base.extend_from_slice(&jpeg[span.offset..span.offset + span.length]);
            } else if span.payload.starts_with(b"http://ns.adobe.com/xap/1.0/\0") {
                let prefix = b"http://ns.adobe.com/xap/1.0/\0";
                let xml = std::str::from_utf8(&span.payload[prefix.len()..]).unwrap();
                let xml = xml.replace(
                    &format!("Item:Length=\"{}\"", entries[1].size),
                    &format!("Item:Length=\"{}\"", gain.len()),
                );
                base.extend_from_slice(&zenjpeg::container::xmp::create_xmp_app1_marker(&xml));
            } else {
                base.extend_from_slice(&jpeg[span.offset..span.offset + span.length]);
            }
        }
        let new_mpf = mpf::create_mpf_header_typed(
            base.len(),
            &[(MpImageType::Undefined, gain.len())],
            Some(mpf_offset),
        );
        let old_len = marker::iter(&base)
            .find(|s| s.payload.starts_with(b"MPF\0"))
            .unwrap()
            .length;
        assert_eq!(old_len, new_mpf.len());
        base[mpf_offset..mpf_offset + old_len].copy_from_slice(&new_mpf);
        [base, gain, b"SECRET-trailer".to_vec()].concat()
    }
    fn scans(jpeg: &[u8]) -> Vec<Vec<u8>> {
        mpf::parse_mpf(jpeg)
            .unwrap()
            .into_iter()
            .flat_map(|e| {
                let data = &jpeg[e.offset..e.offset + e.size];
                marker::iter(data)
                    .filter(|s| s.kind == MarkerKind::Sos)
                    .map(|s| data[s.offset..s.offset + s.length].to_vec())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    #[test]
    fn scrubs_both_components_without_changing_scans_parameters_or_reconstructed_hdr() {
        let clean = source();
        let input = add_private_comments(&clean);
        let req = ScrubRequest::new(&input);
        let plan = req.plan().unwrap();
        let output = plan.to_vec().unwrap();
        assert!(!output.windows(6).any(|b| b == b"SECRET"));
        assert_eq!(scans(&input), scans(&output));
        assert_eq!(
            plan.report()
                .iter()
                .filter(|e| e.carrier() == "private JPEG marker")
                .count(),
            2
        );
        let a = zenjpeg::ultrahdr::decode_ultrahdr_hdr(
            &input,
            4.0,
            zenjpeg::ultrahdr::HdrOutputFormat::LinearFloat,
        )
        .unwrap();
        let b = zenjpeg::ultrahdr::decode_ultrahdr_hdr(
            &output,
            4.0,
            zenjpeg::ultrahdr::HdrOutputFormat::LinearFloat,
        )
        .unwrap();
        assert_eq!(
            a.as_slice().contiguous_bytes(),
            b.as_slice().contiguous_bytes()
        );
        let (_, a) = zencodecs::DecodeRequest::new(&input)
            .decode_gain_map()
            .unwrap();
        let (_, b) = zencodecs::DecodeRequest::new(&output)
            .decode_gain_map()
            .unwrap();
        assert_eq!(a.unwrap().metadata, b.unwrap().metadata);
    }
    #[test]
    fn gain_map_editing_is_explicit_and_cannot_replace_required_signaling() {
        use core::cell::Cell;
        use zencodecs::metadata::Services;
        struct Editor {
            calls: Cell<usize>,
            corrupt: bool,
        }
        impl Services for Editor {
            fn rewrite_xmp(
                &self,
                _: &[u8],
                _: usize,
                _: &dyn enough::Stop,
            ) -> Result<Option<Vec<u8>>, Error> {
                self.calls.set(self.calls.get() + 1);
                Ok(Some(b"description".to_vec()))
            }
            fn merge_xmp(
                &self,
                _: &[u8],
                required: &[u8],
                _: usize,
                _: &dyn enough::Stop,
            ) -> Result<Vec<u8>, Error> {
                Ok(if self.corrupt {
                    b"<invalid/>".to_vec()
                } else {
                    required.to_vec()
                })
            }
        }
        let input = source();
        let service = Editor {
            calls: Cell::new(0),
            corrupt: false,
        };
        ScrubRequest::new(&input)
            .with_services(&service)
            .plan()
            .unwrap();
        assert_eq!(service.calls.get(), 0);
        let output = ScrubRequest::new(&input)
            .with_services(&service)
            .with_xmp_editing(true)
            .plan()
            .unwrap()
            .to_vec()
            .unwrap();
        assert_eq!(service.calls.get(), 2);
        assert_eq!(scans(&input), scans(&output));
        let corrupt = Editor {
            calls: Cell::new(0),
            corrupt: true,
        };
        assert!(
            ScrubRequest::new(&input)
                .with_services(&corrupt)
                .with_xmp_editing(true)
                .plan()
                .is_err()
        );
    }
    #[cfg(all(feature = "jxl-encode", feature = "jxl-decode"))]
    #[test]
    fn native_jxl_gain_map_and_parameters_survive() {
        let input = source_format(zencodecs::ImageFormat::Jxl);
        let output = ScrubRequest::new(&input).plan().unwrap().to_vec().unwrap();
        let (a, ga) = zencodecs::DecodeRequest::new(&input)
            .decode_gain_map()
            .unwrap();
        let (b, gb) = zencodecs::DecodeRequest::new(&output)
            .decode_gain_map()
            .unwrap();
        assert_eq!(a.pixels().contiguous_bytes(), b.pixels().contiguous_bytes());
        let (ga, gb) = (ga.unwrap(), gb.unwrap());
        assert_eq!(ga.metadata, gb.metadata);
        assert_eq!(ga.gain_map.data, gb.gain_map.data);
        assert_eq!(
            (ga.gain_map.width, ga.gain_map.height, ga.gain_map.channels),
            (gb.gain_map.width, gb.gain_map.height, gb.gain_map.channels)
        );
    }
    #[test]
    fn invalid_authoritative_iso_never_falls_back_to_valid_xmp() {
        let mut input = source();
        let e = mpf::parse_mpf(&input).unwrap()[1];
        let iso = marker::iter(&input[e.offset..e.offset + e.size])
            .find(|s| s.payload.starts_with(zencodec::ISO_21496_1_URN))
            .unwrap();
        let version = e.offset + iso.offset + 4 + zencodec::ISO_21496_1_URN.len();
        input[version] = 255;
        assert!(ScrubRequest::new(&input).plan().is_err());
    }
}

#[cfg(all(feature = "jxl-encode", feature = "jxl-decode"))]
#[test]
fn native_jxl_pixels_survive_container_scrubbing() {
    let pixels = zenpixels::PixelBuffer::from_vec(
        vec![73u8; 16 * 16 * 3],
        16,
        16,
        zenpixels::PixelDescriptor::RGB8_SRGB,
    )
    .unwrap();
    let encoded = zencodecs::EncodeRequest::new(zencodecs::ImageFormat::Jxl)
        .with_metadata(zencodec::Metadata::none().with_xmp(b"SECRET".to_vec()))
        .encode(pixels.as_slice(), false)
        .unwrap()
        .into_vec();
    let output = ScrubRequest::new(&encoded)
        .plan()
        .unwrap()
        .to_vec()
        .unwrap();
    assert!(!output.windows(6).any(|b| b == b"SECRET"));
    let a = zencodecs::DecodeRequest::new(&encoded)
        .decode_full_frame()
        .unwrap();
    let b = zencodecs::DecodeRequest::new(&output)
        .decode_full_frame()
        .unwrap();
    assert_eq!(a.pixels().contiguous_bytes(), b.pixels().contiguous_bytes());
    assert_eq!(a.info().source_color, b.info().source_color);
}

#[test]
fn bounded_mutation_smoke_preserves_plan_accounting() {
    let png = png_scrub::chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]);
    let seeds = [
        b"\xff\x0a".to_vec(),
        [b"\x89PNG\r\n\x1a\n".as_slice(), &png].concat(),
        b"\xff\xd8\xff\xd9".to_vec(),
    ];
    for seed in seeds {
        for i in 0..seed.len() {
            for b in [0, 1, 127, 255] {
                let mut input = seed.clone();
                input[i] = b;
                if let Ok(plan) = ScrubRequest::new(&input)
                    .with_byte_limits(4096, 4096)
                    .with_metadata_limit(1024)
                    .with_carrier_limit(64)
                    .plan()
                {
                    assert_eq!(
                        plan.output_len(),
                        plan.chunks().map(<[u8]>::len).sum::<usize>()
                    );
                    for entry in plan.report() {
                        assert!(entry.source_range().end <= input.len());
                    }
                }
            }
        }
    }
}
