//! Generate before/after comparison images for all film look presets.
//!
//! Takes a directory of input images (JPEG/PNG), applies each of the 34
//! built-in film look presets, and writes output images to a gallery directory.
//! Also generates a JSON manifest for the comparison viewer.
//!
//! Usage:
//!   cargo run --release --features experimental --example film_look_gallery -- <input_dir> <output_dir>
//!
//! The output directory will contain:
//!   originals/    — resized input images (max 1024px long edge)
//!   presets/      — filtered images: <preset_id>/<image_name>.jpg
//!   manifest.json — metadata for the web viewer

use std::fs;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageReader, RgbImage};
use zenfilters::filters::{FilmLook, FilmPreset};
use zenfilters::{
    Filter, FilterContext, OklabPlanes, gather_oklab_to_srgb_u8, scatter_srgb_u8_to_oklab,
};
use zenpixels::ColorPrimaries;
use zenpixels_convert::oklab;

const MAX_DIM: u32 = 1024;
const JPEG_QUALITY: u8 = 88;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <input_dir> <output_dir>", args[0]);
        std::process::exit(1);
    }

    let input_dir = PathBuf::from(&args[1]);
    let output_dir = PathBuf::from(&args[2]);

    // Collect input images
    let mut images: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&input_dir).expect("cannot read input dir") {
        let entry = entry.unwrap();
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_lowercase().as_str() {
                "jpg" | "jpeg" | "png" => images.push(path),
                _ => {}
            }
        }
    }
    images.sort();

    if images.is_empty() {
        eprintln!("No JPEG/PNG images found in {:?}", input_dir);
        std::process::exit(1);
    }

    eprintln!(
        "Found {} images, {} presets",
        images.len(),
        FilmPreset::ALL.len()
    );

    // Create output directories
    let originals_dir = output_dir.join("originals");
    fs::create_dir_all(&originals_dir).unwrap();
    for preset in FilmPreset::ALL {
        fs::create_dir_all(output_dir.join("presets").join(preset.id())).unwrap();
    }

    // Pre-build all film looks (one-time cost)
    let looks: Vec<(FilmPreset, FilmLook)> = FilmPreset::ALL
        .iter()
        .map(|&p| {
            eprintln!("  Building {}...", p.name());
            (p, FilmLook::new(p))
        })
        .collect();

    let mut ctx = FilterContext::new();
    let mut manifest_images = Vec::new();

    for (img_idx, img_path) in images.iter().enumerate() {
        let stem = img_path.file_stem().unwrap().to_str().unwrap();
        eprintln!("[{}/{}] Processing {}...", img_idx + 1, images.len(), stem);

        // Decode and resize
        let img = match ImageReader::open(img_path)
            .and_then(|r| r.with_guessed_format())
            .map_err(|e| e.to_string())
            .and_then(|r| r.decode().map_err(|e| e.to_string()))
        {
            Ok(i) => i,
            Err(e) => {
                eprintln!("  Skipping: {e}");
                continue;
            }
        };

        let img = if img.width() > MAX_DIM || img.height() > MAX_DIM {
            img.resize(MAX_DIM, MAX_DIM, FilterType::Lanczos3)
        } else {
            img
        };

        let rgb = img.to_rgb8();
        let (rw, rh) = (rgb.width(), rgb.height());
        let srgb_u8 = rgb.as_raw();

        // Save original
        save_jpeg(&originals_dir.join(format!("{stem}.jpg")), &rgb);

        // Scatter to Oklab once
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let mut base_planes = OklabPlanes::new(rw, rh);
        scatter_srgb_u8_to_oklab(srgb_u8, &mut base_planes, 3, &m1);

        // Apply each preset
        for (preset, look) in &looks {
            let mut planes = base_planes.clone();
            look.apply(&mut planes, &mut ctx);

            // Gather to sRGB u8
            let mut out = vec![0u8; (rw as usize) * (rh as usize) * 3];
            gather_oklab_to_srgb_u8(&planes, &mut out, 3, &m1);

            let out_img = RgbImage::from_raw(rw, rh, out).unwrap();
            save_jpeg(
                &output_dir
                    .join("presets")
                    .join(preset.id())
                    .join(format!("{stem}.jpg")),
                &out_img,
            );
        }

        manifest_images.push(stem.to_string());
    }

    // Write manifest
    let presets_json: Vec<String> = FilmPreset::ALL
        .iter()
        .map(|p| format!("    {{\"id\": \"{}\", \"name\": \"{}\"}}", p.id(), p.name()))
        .collect();

    let images_json: Vec<String> = manifest_images
        .iter()
        .map(|s| format!("    \"{}\"", s))
        .collect();

    let manifest = format!(
        "{{\n  \"presets\": [\n{}\n  ],\n  \"images\": [\n{}\n  ]\n}}\n",
        presets_json.join(",\n"),
        images_json.join(",\n"),
    );

    fs::write(output_dir.join("manifest.json"), &manifest).unwrap();
    eprintln!(
        "Done. {} images x {} presets = {} outputs",
        manifest_images.len(),
        FilmPreset::ALL.len(),
        manifest_images.len() * FilmPreset::ALL.len(),
    );
}

fn save_jpeg(path: &Path, img: &RgbImage) {
    let mut buf = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    img.write_with_encoder(encoder).expect("JPEG encode failed");
    fs::write(path, &buf).unwrap();
}
