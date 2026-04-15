//! Gain map inspection: decode an UltraHDR JPEG and inspect its JPEG extras.
//!
//! Demonstrates accessing JPEG-specific extras via `DecodeOutput::extras()`,
//! including MPF directory, secondary images, and gain maps. If a gain map is
//! found, it's decoded as a separate image and the full extras are roundtripped
//! via `DecodedExtras::to_encoder_segments()`.
//!
//! Run: `cargo run --example gainmap_roundtrip --features all,std`

use zencodecs::config::CodecConfig;
use zencodecs::config::jpeg::{ChromaSubsampling, DecodedExtras, EncoderConfig};
use zencodecs::{DecodeRequest, EncodeRequest, ImageFormat, PixelBufferConvertTypedExt as _};

fn main() {
    let jpeg_data = include_bytes!("../tests/images/ultrahdr_sample.jpg");

    // Decode the UltraHDR JPEG
    let decoded = DecodeRequest::new(jpeg_data)
        .decode_full_frame()
        .expect("failed to decode JPEG");

    println!(
        "Primary image: {}x{} {:?}",
        decoded.width(),
        decoded.height(),
        decoded.format(),
    );

    // Access JPEG-specific extras via typed extras
    let extras = decoded
        .extras::<DecodedExtras>()
        .expect("no jpeg extras — was this a JPEG?");

    println!("\nPreserved segments:");
    for seg in extras.segments() {
        println!(
            "  marker=0x{:02X}, type={:?}, {} bytes",
            seg.marker,
            seg.segment_type,
            seg.data.len()
        );
    }

    // Show MPF directory info
    if let Some(mpf) = extras.mpf() {
        println!("\nMPF directory: {} entries", mpf.images.len());
        for (i, entry) in mpf.images.iter().enumerate() {
            println!(
                "  [{}] type={:?}, size={} bytes, offset={}",
                i, entry.image_type, entry.size, entry.offset
            );
        }
    } else {
        println!("\nNo MPF directory found");
    }

    // Show secondary images
    let secondary = extras.secondary_images();
    println!("\nSecondary images: {}", secondary.len());
    for (i, img) in secondary.iter().enumerate() {
        println!(
            "  [{}] type={:?}, {} bytes",
            i,
            img.image_type,
            img.data.len()
        );
    }

    // Show metadata
    if let Some(icc) = extras.icc_profile() {
        println!("\nICC profile: {} bytes", icc.len());
    }
    if let Some(xmp) = extras.xmp() {
        let preview = if xmp.len() > 200 {
            format!("{}...", &xmp[..200])
        } else {
            xmp.to_string()
        };
        println!("\nXMP ({} bytes):\n  {preview}", xmp.len());
    }

    // Try to get gain map
    match extras.gainmap() {
        Some(gainmap_data) => {
            println!("\nGain map found: {} bytes", gainmap_data.len());

            // Decode the gain map as a separate JPEG
            let gm_decoded = DecodeRequest::new(gainmap_data)
                .decode_full_frame()
                .expect("failed to decode gain map JPEG");

            println!(
                "Gain map decoded: {}x{}",
                gm_decoded.width(),
                gm_decoded.height(),
            );
        }
        None => {
            println!(
                "\nNo gain map extracted (MPF secondary image extraction may not have matched)"
            );
        }
    }

    // Roundtrip: re-encode with all preserved segments using to_encoder_segments()
    let segments = extras.to_encoder_segments();
    let encoder_config =
        EncoderConfig::ycbcr(85, ChromaSubsampling::Quarter).with_segments(segments);

    let config = CodecConfig::default().with_jpeg_encoder(encoder_config);

    let rgb8 = decoded.into_buffer().to_rgb8();
    let img = rgb8.as_imgref();

    let re_encoded = EncodeRequest::new(ImageFormat::Jpeg)
        .with_codec_config(&config)
        .encode_full_frame_rgb8(img)
        .expect("re-encode failed");

    println!(
        "\nRe-encoded JPEG: {} bytes (original: {} bytes)",
        re_encoded.len(),
        jpeg_data.len()
    );

    // Verify segments survived
    let re_decoded = DecodeRequest::new(re_encoded.data())
        .decode_full_frame()
        .expect("failed to re-decode");

    if let Some(re_extras) = re_decoded.extras::<DecodedExtras>() {
        println!(
            "Preserved {} segments after roundtrip",
            re_extras.segments().len()
        );
        if let Some(xmp) = re_extras.xmp() {
            println!("XMP preserved: {} bytes", xmp.len());
        }
        if let Some(icc) = re_extras.icc_profile() {
            println!("ICC preserved: {} bytes", icc.len());
        }
        match re_extras.gainmap() {
            Some(gm) => println!("Gain map preserved: {} bytes", gm.len()),
            None => println!("No gain map in roundtripped output"),
        }
    }
}
