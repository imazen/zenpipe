//! Generate before/after comparison images for film looks and pipeline presets.
//!
//! Uses codec-corpus to download test images automatically. Applies all 34
//! film look presets and all 19 pipeline presets, writes output JPEGs and
//! a JSON manifest for the comparison viewer.
//!
//! Usage:
//!   cargo run --release --features experimental --example film_look_gallery -- <output_dir> [dataset]
//!
//! dataset defaults to "clic2025/final-test" (30 high-res photographic images).
//! Other options: "clic2025/training", "CID22/CID22-512/validation", "gb82".

use std::fs;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageReader, RgbImage};
use zenfilters::filters::{FilmLook, FilmPreset};
use zenfilters::presets::{self, Preset};
use zenfilters::{
    Filter, FilterContext, OklabPlanes, Pipeline, PipelineConfig, gather_oklab_to_srgb_u8,
    scatter_srgb_u8_to_oklab,
};
use zenpixels::ColorPrimaries;
use zenpixels_convert::oklab;

const MAX_DIM: u32 = 1024;
const JPEG_QUALITY: u8 = 88;
const MAX_IMAGES: usize = 20;

/// A gallery entry: either a film look or a pipeline preset.
enum GalleryEntry {
    Film(FilmPreset, FilmLook),
    Pipeline(String, Pipeline),
}

impl GalleryEntry {
    fn id(&self) -> String {
        match self {
            Self::Film(p, _) => p.id().to_string(),
            Self::Pipeline(name, _) => {
                format!("preset_{}", name.to_lowercase().replace(' ', "_"))
            }
        }
    }

    fn name(&self) -> String {
        match self {
            Self::Film(p, _) => p.name().to_string(),
            Self::Pipeline(name, _) => name.clone(),
        }
    }

    fn group(&self) -> &'static str {
        match self {
            Self::Film(..) => "Film Looks",
            Self::Pipeline(..) => "Presets",
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <output_dir> [dataset]", args[0]);
        eprintln!("  dataset: codec-corpus path (default: clic2025/final-test)");
        std::process::exit(1);
    }

    let output_dir = PathBuf::from(&args[1]);
    let dataset = args
        .get(2)
        .map(|s| s.as_str())
        .unwrap_or("clic2025/final-test");

    // Get images via codec-corpus
    let corpus = codec_corpus::Corpus::new().expect("failed to init codec-corpus");
    let input_dir = corpus.get(dataset).unwrap_or_else(|e| {
        eprintln!("Failed to get dataset '{}': {}", dataset, e);
        std::process::exit(1);
    });

    // Collect input images
    let mut images: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&input_dir).expect("cannot read input dir") {
        let entry = entry.unwrap();
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_lowercase().as_str() {
                "jpg" | "jpeg" | "png" | "bmp" | "tiff" | "tif" => images.push(path),
                _ => {}
            }
        }
    }
    images.sort();

    // Skip non-photographic images by hash prefix
    let skip_prefixes = ["2a760bf1", "86127fbd"];
    images.retain(|p| {
        let stem = p.file_stem().unwrap().to_str().unwrap();
        !skip_prefixes.iter().any(|pfx| stem.starts_with(pfx))
    });
    images.truncate(MAX_IMAGES);

    if images.is_empty() {
        eprintln!("No images found in {:?}", input_dir);
        std::process::exit(1);
    }

    // Build all gallery entries: film looks + pipeline presets
    let mut entries: Vec<GalleryEntry> = Vec::new();

    // Film looks
    for &p in FilmPreset::ALL {
        eprintln!("  Building film look: {}...", p.name());
        entries.push(GalleryEntry::Film(p, FilmLook::new(p)));
    }

    // Pipeline presets
    for preset in presets::builtin_presets() {
        eprintln!("  Building preset: {}...", preset.name);
        let pipe = preset.build_pipeline_at(1.0);
        entries.push(GalleryEntry::Pipeline(preset.name.clone(), pipe));
    }

    eprintln!(
        "Found {} images, {} entries ({} film looks + {} presets)",
        images.len(),
        entries.len(),
        FilmPreset::ALL.len(),
        presets::builtin_presets().len(),
    );

    // Create output directories
    let originals_dir = output_dir.join("originals");
    fs::create_dir_all(&originals_dir).unwrap();
    for entry in &entries {
        fs::create_dir_all(output_dir.join("presets").join(entry.id())).unwrap();
    }

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

        // Prepare both formats for the two entry types
        let m1 = oklab::rgb_to_lms_matrix(ColorPrimaries::Bt709).unwrap();
        let m1_inv = oklab::lms_to_rgb_matrix(ColorPrimaries::Bt709).unwrap();

        // Oklab planes for film looks
        let mut base_planes = OklabPlanes::new(rw, rh);
        scatter_srgb_u8_to_oklab(srgb_u8, &mut base_planes, 3, &m1);

        // Linear f32 for pipeline presets (sRGB u8 → linear f32)
        let linear_src: Vec<f32> = srgb_u8
            .iter()
            .map(|&v| {
                let x = v as f32 / 255.0;
                if x <= 0.04045 {
                    x / 12.92
                } else {
                    ((x + 0.055) / 1.055).powf(2.4)
                }
            })
            .collect();

        // Apply each entry
        for entry in &entries {
            let out_path = output_dir
                .join("presets")
                .join(entry.id())
                .join(format!("{stem}.jpg"));

            match entry {
                GalleryEntry::Film(_, look) => {
                    let mut planes = base_planes.clone();
                    look.apply(&mut planes, &mut ctx);
                    let mut out = vec![0u8; (rw as usize) * (rh as usize) * 3];
                    gather_oklab_to_srgb_u8(&planes, &mut out, 3, &m1_inv);
                    let out_img = RgbImage::from_raw(rw, rh, out).unwrap();
                    save_jpeg(&out_path, &out_img);
                }
                GalleryEntry::Pipeline(_, pipe) => {
                    let mut dst = vec![0.0f32; linear_src.len()];
                    pipe.apply(&linear_src, &mut dst, rw, rh, 3, &mut ctx)
                        .unwrap();
                    // Linear f32 → sRGB u8
                    let out: Vec<u8> = dst
                        .iter()
                        .map(|&v| {
                            let s = if v <= 0.0031308 {
                                v * 12.92
                            } else {
                                1.055 * v.max(0.0).powf(1.0 / 2.4) - 0.055
                            };
                            (s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                        })
                        .collect();
                    let out_img = RgbImage::from_raw(rw, rh, out).unwrap();
                    save_jpeg(&out_path, &out_img);
                }
            }
        }

        manifest_images.push(stem.to_string());
    }

    // Write manifest
    let presets_json: Vec<String> = entries
        .iter()
        .map(|e| {
            format!(
                "    {{\"id\": \"{}\", \"name\": \"{}\", \"group\": \"{}\"}}",
                e.id(),
                e.name(),
                e.group()
            )
        })
        .collect();

    let images_json: Vec<String> = manifest_images
        .iter()
        .map(|s| format!("    \"{}\"", s))
        .collect();

    // Default to image 18 (0-indexed 17)
    let default_idx = 17.min(manifest_images.len().saturating_sub(1));

    let manifest = format!(
        "{{\n  \"presets\": [\n{}\n  ],\n  \"images\": [\n{}\n  ],\n  \"default_image\": {}\n}}\n",
        presets_json.join(",\n"),
        images_json.join(",\n"),
        default_idx,
    );

    fs::write(output_dir.join("manifest.json"), &manifest).unwrap();
    eprintln!(
        "Done. {} images x {} entries = {} outputs",
        manifest_images.len(),
        entries.len(),
        manifest_images.len() * entries.len(),
    );
}

fn save_jpeg(path: &Path, img: &RgbImage) {
    let mut buf = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    img.write_with_encoder(encoder).expect("JPEG encode failed");
    fs::write(path, &buf).unwrap();
}
