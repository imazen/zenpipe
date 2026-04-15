//! Metadata roundtrip: decode JPEG with ICC/EXIF/XMP, re-encode to other formats.
//!
//! Demonstrates that `ImageInfo::metadata()` extracts borrowed metadata from decode
//! output, which can be passed directly to `EncodeRequest::with_metadata()`.
//!
//! Run: `cargo run --example metadata_roundtrip --features all,std`

use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, PixelBufferConvertTypedExt as _};

fn main() {
    let jpeg_data = include_bytes!("../tests/images/ultrahdr_sample.jpg");

    // Decode the JPEG
    let decoded = DecodeRequest::new(jpeg_data)
        .decode_full_frame()
        .expect("failed to decode JPEG");

    println!(
        "Decoded: {}x{} {:?}",
        decoded.width(),
        decoded.height(),
        decoded.format()
    );

    // Extract metadata via the convenience method
    let meta = decoded.metadata();
    println!("\nMetadata from decode:");
    println!(
        "  ICC profile: {}",
        meta.icc_profile
            .as_deref()
            .map(|p| format!("{} bytes", p.len()))
            .unwrap_or_else(|| "none".into())
    );
    println!(
        "  EXIF: {}",
        meta.exif
            .as_deref()
            .map(|p| format!("{} bytes", p.len()))
            .unwrap_or_else(|| "none".into())
    );
    println!(
        "  XMP: {}",
        meta.xmp
            .as_deref()
            .map(|p| format!("{} bytes", p.len()))
            .unwrap_or_else(|| "none".into())
    );

    // Convert to RGB8 for encoding
    let rgb8 = decoded.into_buffer().to_rgb8();
    let img = rgb8.as_imgref();

    // Re-encode to each format that supports metadata
    let formats = [
        ("JPEG", ImageFormat::Jpeg, "ICC+EXIF+XMP"),
        ("WebP", ImageFormat::WebP, "ICC+EXIF+XMP"),
        ("PNG", ImageFormat::Png, "ICC+EXIF+XMP"),
    ];

    println!("\nRoundtrip results:");
    for (name, format, expected) in &formats {
        let encoded = EncodeRequest::new(*format)
            .with_quality(85.0)
            .with_metadata(meta.clone())
            .encode_full_frame_rgb8(img)
            .expect("encode failed");

        // Decode the re-encoded image to verify metadata survived
        let re_decoded = DecodeRequest::new(encoded.data())
            .decode_full_frame()
            .expect("re-decode failed");

        let re_meta = re_decoded.metadata();
        let icc_match = match (meta.icc_profile.as_deref(), re_meta.icc_profile.as_deref()) {
            (Some(a), Some(b)) => {
                if a == b {
                    "exact match"
                } else {
                    "changed"
                }
            }
            (None, None) => "both none",
            (Some(_), None) => "LOST",
            (None, Some(_)) => "appeared",
        };
        let exif_ok = re_meta.exif.is_some() == meta.exif.is_some();
        let xmp_ok = re_meta.xmp.is_some() == meta.xmp.is_some();

        println!(
            "  {name}: {size} bytes, ICC={icc_match}, EXIF={exif}, XMP={xmp} (supports: {expected})",
            size = encoded.len(),
            exif = if exif_ok { "preserved" } else { "LOST" },
            xmp = if xmp_ok { "preserved" } else { "LOST" },
        );
    }
}
