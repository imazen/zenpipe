//! Image inspection — probe and display metadata without decoding.

use std::path::Path;

use serde::Serialize;

use crate::InfoArgs;
use crate::batch;
use crate::metadata::{self, ParsedCicp, ParsedExif, ParsedIcc, ParsedXmp};

/// Run the `info` subcommand.
pub fn run(args: InfoArgs) -> anyhow::Result<()> {
    let files = batch::expand_inputs(&args.files)?;

    if files.is_empty() {
        anyhow::bail!("no image files found");
    }

    let multi = files.len() > 1;
    // --jsonl implies metadata parsing for enrichment use
    let parse_metadata = args.metadata || args.jsonl;

    for (i, path) in files.iter().enumerate() {
        if multi && !args.json && !args.jsonl {
            if i > 0 {
                println!();
            }
            println!("{}:", path.display());
        }

        match inspect_file(path, parse_metadata) {
            Ok(info) => {
                if args.jsonl {
                    println!("{}", serde_json::to_string(&info)?);
                } else if args.json {
                    println!("{}", serde_json::to_string_pretty(&info)?);
                } else {
                    print_info(&info);
                }
            }
            Err(e) => {
                eprintln!("  error: {e}");
            }
        }
    }

    Ok(())
}

/// Probe a single file and return structured info.
fn inspect_file(path: &Path, parse_metadata: bool) -> anyhow::Result<ImageInfoDisplay> {
    let data = std::fs::read(path)?;
    let file_size = data.len() as u64;

    // Try full probe (requires codec feature)
    let info = zencodecs::from_bytes(&data)?;

    let warnings = info.warnings.clone();

    // Optionally parse metadata for rich display
    let (parsed_exif, parsed_icc, parsed_cicp, parsed_xmp) = if parse_metadata {
        let exif = info.exif.as_deref().and_then(metadata::parse_exif);
        let icc = info.icc_profile.as_deref().and_then(metadata::parse_icc);
        let cicp = info.cicp.as_ref().map(metadata::parse_cicp);
        let xmp = info.xmp.as_deref().and_then(metadata::parse_xmp);
        (exif, icc, cicp, xmp)
    } else {
        (None, None, None, None)
    };

    Ok(ImageInfoDisplay {
        path: path.display().to_string(),
        format: format!("{:?}", info.format),
        mime_type: info.format.mime_type().to_string(),
        width: info.width,
        height: info.height,
        display_width: info.display_width(),
        display_height: info.display_height(),
        has_alpha: info.has_alpha,
        has_animation: info.has_animation,
        frame_count: info.frame_count,
        bit_depth: info.bit_depth,
        channel_count: info.channel_count,
        orientation: info.orientation.exif_value() as u8,
        icc_profile_size: info.icc_profile.as_ref().map(|p| p.len()),
        exif_size: info.exif.as_ref().map(|e| e.len()),
        xmp_size: info.xmp.as_ref().map(|x| x.len()),
        cicp: info.cicp.map(|c| CicpDisplay {
            color_primaries: c.color_primaries,
            transfer_characteristics: c.transfer_characteristics,
            matrix_coefficients: c.matrix_coefficients,
            full_range: c.full_range,
        }),
        has_gain_map: info.has_gain_map,
        file_size,
        warnings,
        parsed_exif,
        parsed_icc,
        parsed_cicp,
        parsed_xmp,
    })
}

#[derive(Debug, Serialize)]
struct ImageInfoDisplay {
    path: String,
    format: String,
    mime_type: String,
    width: u32,
    height: u32,
    display_width: u32,
    display_height: u32,
    has_alpha: bool,
    has_animation: bool,
    frame_count: Option<u32>,
    bit_depth: Option<u8>,
    channel_count: Option<u8>,
    orientation: u8,
    icc_profile_size: Option<usize>,
    exif_size: Option<usize>,
    xmp_size: Option<usize>,
    cicp: Option<CicpDisplay>,
    has_gain_map: bool,
    file_size: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parsed_exif: Option<ParsedExif>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parsed_icc: Option<ParsedIcc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parsed_cicp: Option<ParsedCicp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parsed_xmp: Option<ParsedXmp>,
}

#[derive(Debug, Serialize)]
struct CicpDisplay {
    color_primaries: u8,
    transfer_characteristics: u8,
    matrix_coefficients: u8,
    full_range: bool,
}

fn print_info(info: &ImageInfoDisplay) {
    println!("  Format:       {} ({})", info.format, info.mime_type);
    println!("  Dimensions:   {}x{}", info.width, info.height);
    if info.display_width != info.width || info.display_height != info.height {
        println!(
            "  Display:      {}x{} (orientation: {})",
            info.display_width, info.display_height, info.orientation
        );
    } else if info.orientation != 1 {
        println!("  Orientation:  {}", info.orientation);
    }
    if let Some(depth) = info.bit_depth {
        print!("  Bit depth:    {}", depth);
        if let Some(ch) = info.channel_count {
            print!(" x {} channels", ch);
        }
        println!();
    }
    println!(
        "  Alpha:        {}",
        if info.has_alpha { "yes" } else { "no" }
    );
    if info.has_animation {
        print!("  Animation:    yes");
        if let Some(count) = info.frame_count {
            print!(" ({} frames)", count);
        }
        println!();
    }

    if info.has_gain_map {
        println!("  Gain map:     yes (HDR)");
    }

    // Metadata section
    let has_meta = info.icc_profile_size.is_some()
        || info.exif_size.is_some()
        || info.xmp_size.is_some()
        || info.cicp.is_some();

    if has_meta {
        println!("  Metadata:");

        // ICC
        if let Some(size) = info.icc_profile_size {
            if let Some(ref icc) = info.parsed_icc {
                // Rich ICC display
                if let Some(ref desc) = icc.description {
                    println!("    ICC profile: {} ({} bytes)", desc, size);
                } else {
                    println!("    ICC profile: {} bytes", size);
                }
                println!("      {}", icc.note);
                println!(
                    "      Color space: {}, PCS: {}, Class: {}",
                    icc.color_space, icc.pcs, icc.profile_class
                );
                println!("      Version: {}", icc.version);
                if let Some(ref trc_desc) = icc.trc_description {
                    println!("      TRC: {}", trc_desc);
                }
                if let Some(ref formula) = icc.trc_formula {
                    for line in formula.lines() {
                        println!("        {}", line);
                    }
                }
            } else {
                println!("    ICC profile: {} bytes", size);
            }
        }

        // EXIF
        if let Some(size) = info.exif_size {
            if let Some(ref exif) = info.parsed_exif {
                println!("    EXIF ({} tags, {} bytes):", exif.total_tags, size);
                for (key, value) in &exif.fields {
                    println!("      {}: {}", key, value);
                }
            } else {
                println!("    EXIF:        {} bytes", size);
            }
        }

        // XMP
        if let Some(size) = info.xmp_size {
            if let Some(ref xmp) = info.parsed_xmp {
                println!(
                    "    XMP ({} properties, {} bytes):",
                    xmp.properties.len(),
                    size
                );
                let tree = metadata::format_xmp_tree(xmp);
                for line in tree.lines() {
                    println!("      {}", line);
                }
            } else {
                println!("    XMP:         {} bytes", size);
            }
        }

        // CICP
        if let Some(ref cicp) = info.cicp {
            if let Some(ref parsed) = info.parsed_cicp {
                println!("    CICP:        {}", parsed.summary);
                println!("      {}", parsed.note);
                if let Some(ref formula) = parsed.transfer_formula {
                    println!("      Transfer function:");
                    for line in formula.lines() {
                        println!("        {}", line);
                    }
                }
            } else {
                println!(
                    "    CICP:        {}/{}/{} ({})",
                    cicp.color_primaries,
                    cicp.transfer_characteristics,
                    cicp.matrix_coefficients,
                    if cicp.full_range { "full" } else { "limited" }
                );
            }
        }
    }

    println!("  File size:    {}", batch::format_size(info.file_size));

    if !info.warnings.is_empty() {
        println!("  Warnings:");
        for w in &info.warnings {
            println!("    - {w}");
        }
    }
}
