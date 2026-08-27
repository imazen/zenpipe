//! SVG decode through the zencodecs public surface (`svg` feature → zensvg).
//! Before zenpipe#1 landed the feature was a `compile_error!` stub; this pins
//! that detection, probe, decode, and the registry all route SVG for real.

#![cfg(feature = "svg")]

use zencodecs::{AllowedFormats, ImageFormat};

// `##` delimiters: the `"#` in `fill="#ff0000"` would end a single-`#` raw string.
const SVG: &[u8] =
    br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="32" viewBox="0 0 64 32">
  <rect x="0" y="0" width="64" height="32" fill="#ff0000"/>
</svg>"##;

#[test]
fn svg_is_detected_probed_and_rasterized() {
    let all = AllowedFormats::all();

    // `from_bytes` is detection + header probe; its `format` is what detection chose.
    let fmt = zencodecs::from_bytes(SVG).expect("SVG detected").format;
    assert!(
        matches!(fmt, ImageFormat::Custom(def) if def.name == "svg"),
        "{fmt:?}"
    );
    assert!(
        all.can_decode(fmt),
        "AllowedFormats::all() must enable the svg Custom format"
    );

    let info = zencodecs::probe(SVG, &all).expect("probe");
    assert_eq!((info.width, info.height), (64, 32));

    let out = zencodecs::decode_full_frame(SVG, &all).expect("decode");
    assert_eq!((out.width(), out.height()), (64, 32));
    let pixels = out.pixels();
    let bpp = pixels.descriptor().bytes_per_pixel();
    let row0 = pixels.row(0);
    let px = &row0[..bpp];
    assert_eq!(
        &px[..3],
        &[255, 0, 0],
        "top-left pixel should be the red fill, got {px:?}"
    );
}

#[test]
fn malformed_svg_errors_instead_of_panicking() {
    let all = AllowedFormats::all();
    let bad = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"0\" height=\"0\"><rect";
    let r = std::panic::catch_unwind(|| zencodecs::decode_full_frame(bad, &all).map(|_| ()));
    assert!(matches!(r, Ok(Err(_))), "expected a typed error, got {r:?}");
}
