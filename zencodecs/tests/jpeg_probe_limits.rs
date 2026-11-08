//! Header-only controls: caller budgets reach the JPEG parser and admission.
#![cfg(feature = "jpeg")]

mod common;

use zencodecs::{AllowedFormats, DecodeRequest, ImageFormat, Limits};

fn large_header() -> Vec<u8> {
    let mut bytes = common::encode_test_data(ImageFormat::Jpeg, 8, 8);
    let offset = bytes
        .windows(2)
        .position(|marker| marker == [0xff, 0xc0] || marker == [0xff, 0xc2])
        .expect("generated JPEG has SOF0 or SOF2");
    bytes[offset + 5..offset + 7].copy_from_slice(&14173_u16.to_be_bytes());
    bytes[offset + 7..offset + 9].copy_from_slice(&11339_u16.to_be_bytes());
    bytes
}

#[test]
fn jpeg_probe_explicit_budget_preserves_default_and_tight_rejection() {
    let bytes = large_header();
    assert!(zencodecs::from_bytes(&bytes).is_err());
    assert!(DecodeRequest::new(&bytes).probe().is_err());
    let roomy = Limits::none().with_max_pixels(200_000_000);
    let info = DecodeRequest::new(&bytes)
        .with_limits(&roomy)
        .probe()
        .unwrap();
    assert_eq!((info.width, info.height), (11339, 14173));
    let tight = Limits::none().with_max_pixels(120_000_000);
    assert!(
        DecodeRequest::new(&bytes)
            .with_limits(&tight)
            .probe()
            .is_err()
    );
    let narrow = roomy.with_max_width(11000);
    assert!(
        DecodeRequest::new(&bytes)
            .with_limits(&narrow)
            .probe()
            .is_err()
    );
    let registry = AllowedFormats::none();
    assert!(
        DecodeRequest::new(&bytes)
            .with_limits(&roomy)
            .with_registry(&registry)
            .probe()
            .is_err()
    );
}

#[test]
fn jpeg_probe_estimate_uses_the_same_explicit_header_budget() {
    let bytes = large_header();
    let roomy = Limits::none().with_max_pixels(200_000_000);
    assert!(
        DecodeRequest::new(&bytes)
            .with_limits(&roomy)
            .estimate()
            .is_ok()
    );
    let tight = Limits::none().with_max_pixels(120_000_000);
    assert!(
        DecodeRequest::new(&bytes)
            .with_limits(&tight)
            .estimate()
            .is_err()
    );
}
