//! Gain map inspection example.
//!
//! Decodes an image and displays its gain map metadata if present.
//! Supports JPEG (UltraHDR), AVIF (tmap), JXL (jhgm), and DNG (Apple ProRAW).
//!
//! Usage:
//!     cargo run --example gainmap_inspect --features jpeg-ultrahdr -- path/to/image.jpg

fn main() {
    #[cfg(not(feature = "jpeg-ultrahdr"))]
    {
        eprintln!("This example requires the `jpeg-ultrahdr` feature.");
        eprintln!(
            "Run with: cargo run --example gainmap_inspect --features jpeg-ultrahdr -- <path>"
        );
        std::process::exit(1);
    }

    #[cfg(feature = "jpeg-ultrahdr")]
    run();
}

#[cfg(feature = "jpeg-ultrahdr")]
fn run() {
    use zencodecs::{DecodeRequest, GainMapDirection, GainMapParams};

    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: gainmap_inspect <image-path>");
        std::process::exit(1);
    });

    let data = std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        std::process::exit(1);
    });

    // Probe to check for gain map presence without full decode.
    let info = DecodeRequest::new(&data).probe().unwrap_or_else(|e| {
        eprintln!("Probe failed: {}", e.error());
        std::process::exit(1);
    });

    println!("Format:     {:?}", info.format);
    println!("Dimensions: {}x{}", info.width, info.height);
    println!("Has alpha:  {}", info.has_alpha);

    println!(
        "\nGain map probe: {}",
        match &info.gain_map {
            zencodecs::GainMapPresence::Unknown =>
                "unknown (probe window too small or not supported)".into(),
            zencodecs::GainMapPresence::Absent => "absent".into(),
            zencodecs::GainMapPresence::Available(gm_info) => {
                format!(
                    "present ({}x{}, {} ch)",
                    gm_info.width, gm_info.height, gm_info.channels
                )
            }
            _ => "unknown variant".into(),
        }
    );

    // Decode with gain map extraction.
    println!("\nDecoding with gain map extraction...");
    let (output, gainmap) = DecodeRequest::new(&data)
        .decode_gain_map()
        .unwrap_or_else(|e| {
            eprintln!("Decode failed: {}", e.error());
            std::process::exit(1);
        });

    println!("Base image: {}x{}", output.width(), output.height());

    let Some(gm) = gainmap else {
        println!("No gain map found in this image.");
        return;
    };

    // Gain map image dimensions.
    println!(
        "\nGain map image: {}x{}, {} channel(s), {} bytes",
        gm.gain_map.width,
        gm.gain_map.height,
        gm.gain_map.channels,
        gm.gain_map.data.len(),
    );

    // Direction.
    let direction = if gm.base_is_hdr {
        GainMapDirection::BaseIsHdr
    } else {
        GainMapDirection::BaseIsSdr
    };
    println!("Direction:  {:?}", direction);
    println!("Source:     {:?}", gm.source_format);

    // Metadata in log2/f64 domain (as stored in GainMapMetadata / ultrahdr-core).
    let meta = &gm.metadata;
    println!("\n--- Metadata (log2/f64 domain) ---");
    println!("  gain_map_max:          {:?}", meta.gain_map_max);
    println!("  gain_map_min:          {:?}", meta.gain_map_min);
    println!("  gamma:                 {:?}", meta.gamma);
    println!("  base_offset:           {:?}", meta.base_offset);
    println!("  alternate_offset:      {:?}", meta.alternate_offset);
    println!("  base_hdr_headroom:     {}", meta.base_hdr_headroom);
    println!("  alternate_hdr_headroom:{}", meta.alternate_hdr_headroom);
    println!("  use_base_cspace:       {}", meta.use_base_color_space);

    // Params in log2 domain (canonical ISO 21496-1 representation).
    let params: GainMapParams = gm.params();
    println!("\n--- Params (log2 domain) ---");
    let ch_label = if params.is_single_channel() {
        "  (single-channel, all identical)"
    } else {
        "  (per-channel R/G/B)"
    };
    println!("{ch_label}");
    for (i, ch) in params.channels.iter().enumerate() {
        let label = ["R", "G", "B"][i];
        println!(
            "  [{label}] min={:.4} max={:.4} gamma={:.4} base_off={:.6} alt_off={:.6}",
            ch.min, ch.max, ch.gamma, ch.base_offset, ch.alternate_offset,
        );
    }
    println!(
        "  base_hdr_headroom:      {:.4} (linear: {:.2}x)",
        params.base_hdr_headroom,
        params.linear_base_headroom(),
    );
    println!(
        "  alternate_hdr_headroom: {:.4} (linear: {:.2}x)",
        params.alternate_hdr_headroom,
        params.linear_alternate_headroom(),
    );
    println!("  derived direction:      {:?}", params.direction(),);
}
