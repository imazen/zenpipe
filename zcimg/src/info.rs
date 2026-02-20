//! Image inspection — probe and display metadata without decoding.

use std::path::Path;

use serde::Serialize;
use crate::batch;
use crate::InfoArgs;

/// Run the `info` subcommand.
pub fn run(args: InfoArgs) -> anyhow::Result<()> {
    let files = batch::expand_inputs(&args.files)?;

    if files.is_empty() {
        anyhow::bail!("no image files found");
    }

    let multi = files.len() > 1;

    for (i, path) in files.iter().enumerate() {
        if multi && !args.json {
            if i > 0 {
                println!();
            }
            println!("{}:", path.display());
        }

        match inspect_file(path) {
            Ok(info) => {
                if args.json {
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
fn inspect_file(path: &Path) -> anyhow::Result<ImageInfoDisplay> {
    let data = std::fs::read(path)?;
    let file_size = data.len() as u64;

    // Try full probe (requires codec feature)
    let info = zencodecs::from_bytes(&data)?;

    let warnings = info.warnings.clone();

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
        file_size,
        warnings,
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
    file_size: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
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

    // Metadata
    let has_meta = info.icc_profile_size.is_some()
        || info.exif_size.is_some()
        || info.xmp_size.is_some()
        || info.cicp.is_some();

    if has_meta {
        println!("  Metadata:");
        if let Some(size) = info.icc_profile_size {
            println!("    ICC profile: {} bytes", size);
        }
        if let Some(size) = info.exif_size {
            println!("    EXIF:        {} bytes", size);
        }
        if let Some(size) = info.xmp_size {
            println!("    XMP:         {} bytes", size);
        }
        if let Some(ref cicp) = info.cicp {
            println!(
                "    CICP:        {}/{}/{} ({})",
                cicp.color_primaries,
                cicp.transfer_characteristics,
                cicp.matrix_coefficients,
                if cicp.full_range { "full" } else { "limited" }
            );
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
