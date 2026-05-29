//! clipart_flatten_demo — run `ClipartFlatten` over a clipart corpus and emit
//! before/after/diff images + a zensim-scored CSV report, into subfolders
//! (sources are only read, never written).
//!
//!   cargo run --release --features experimental --example clipart_flatten_demo
//!   cargo run --release --features experimental --example clipart_flatten_demo -- \
//!       --input /mnt/v/zen/ai/clipart --out /mnt/v/zen/ai/_clipartflatten --strength 0.8

#![allow(clippy::needless_range_loop)]

use image::{ImageBuffer, Rgb, RgbImage};
use std::path::{Path, PathBuf};

use zenfilters::filters::ClipartFlatten;
use zenfilters::{FilterContext, Pipeline, PipelineConfig, apply_to_buffer};
use zensim::{RgbSlice, Zensim, ZensimProfile};

fn flatten(img: &RgbImage, strength: f32, cartoon: f32) -> RgbImage {
    let (w, h) = img.dimensions();
    let desc = zenpixels::PixelDescriptor::RGB8_SRGB;
    let input = zenpixels::buffer::PixelBuffer::from_vec(img.as_raw().clone(), w, h, desc).unwrap();
    let mut pipeline = Pipeline::new(PipelineConfig::default()).unwrap();
    let mut f = ClipartFlatten::default();
    f.strength = strength;
    f.cartoon = cartoon;
    pipeline.push(Box::new(f));
    let mut ctx = FilterContext::new();
    let out = apply_to_buffer(&pipeline, &input, true, &mut ctx).unwrap();
    ImageBuffer::from_raw(w, h, out.copy_to_contiguous_bytes()).unwrap()
}

fn zensim_score(a: &RgbImage, b: &RgbImage) -> f64 {
    let a_px: &[[u8; 3]] = bytemuck::cast_slice(a.as_raw());
    let b_px: &[[u8; 3]] = bytemuck::cast_slice(b.as_raw());
    let (w, h) = a.dimensions();
    let z = Zensim::new(ZensimProfile::latest()).with_parallel(true);
    z.compute(
        &RgbSlice::new(a_px, w as usize, h as usize),
        &RgbSlice::new(b_px, w as usize, h as usize),
    )
    .unwrap()
    .score()
}

/// Red diff heatmap: per-pixel max-channel delta amplified onto a black→red→
/// yellow ramp so where (and how strongly) the filter changed pixels is obvious.
fn diff_heatmap(a: &RgbImage, b: &RgbImage, amp: u32) -> RgbImage {
    let (w, h) = a.dimensions();
    let mut out = RgbImage::new(w, h);
    for (px, (pa, pb)) in out.pixels_mut().zip(a.pixels().zip(b.pixels())) {
        let d = (0..3)
            .map(|c| (pa[c] as i32 - pb[c] as i32).unsigned_abs())
            .max()
            .unwrap_or(0);
        let v = (d * amp).min(255) as i32;
        // 0..128 → black→red, 128..255 → red→yellow (green ramps in)
        let r = v.min(255) as u8;
        let g = ((v - 128).max(0) * 2).min(255) as u8;
        *px = Rgb([r, g, 0]);
    }
    out
}

fn load_dir(root: &Path) -> Vec<(String, RgbImage)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            if !matches!(ext.as_deref(), Some("png" | "jpg" | "jpeg" | "webp")) {
                continue;
            }
            if let Ok(img) = image::open(&path) {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                let name = rel.with_extension("").to_string_lossy().replace('\\', "/");
                out.push((name, img.to_rgb8()));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn main() {
    let mut input = PathBuf::from("/mnt/v/zen/ai/clipart");
    let mut out_dir = PathBuf::from("/mnt/v/zen/ai/_clipartflatten");
    let mut strength = 0.85f32;
    let mut cartoon = 0.0f32;
    let mut limit = usize::MAX;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--input" if i + 1 < args.len() => {
                input = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--out" if i + 1 < args.len() => {
                out_dir = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            "--strength" if i + 1 < args.len() => {
                strength = args[i + 1].parse().unwrap_or(0.85);
                i += 1;
            }
            "--cartoon" if i + 1 < args.len() => {
                cartoon = args[i + 1].parse().unwrap_or(0.0);
                i += 1;
            }
            "--limit" if i + 1 < args.len() => {
                limit = args[i + 1].parse().unwrap_or(usize::MAX);
                i += 1;
            }
            other => eprintln!("ignoring unknown arg: {other}"),
        }
        i += 1;
    }
    for sub in ["before", "after", "diff"] {
        std::fs::create_dir_all(out_dir.join(sub)).expect("create output subdir");
    }

    let images = load_dir(&input);
    let mut csv = String::from("name,width,height,strength,zensim,mean_diff,max_diff\n");
    println!(
        "ClipartFlatten demo — {} images (limit {}), strength={strength}, cartoon={cartoon}, out={}",
        images.len(),
        limit,
        out_dir.display()
    );
    println!(
        "{:<40} {:>8} {:>9} {:>7}",
        "image", "zensim", "meanΔ", "maxΔ"
    );

    for (name, img) in images.iter().take(limit) {
        let (w, h) = img.dimensions();
        let res = flatten(img, strength, cartoon);
        let z = zensim_score(img, &res);
        let mut sum = 0u64;
        let mut maxd = 0u8;
        for (pa, pb) in img.pixels().zip(res.pixels()) {
            for c in 0..3 {
                let d = (pa[c] as i32 - pb[c] as i32).unsigned_abs() as u8;
                sum += d as u64;
                maxd = maxd.max(d);
            }
        }
        let meand = sum as f64 / img.as_raw().len() as f64;
        let diff = diff_heatmap(img, &res, 12);
        for (sub, im) in [("before", img), ("after", &res), ("diff", &diff)] {
            let dst = out_dir.join(sub).join(format!("{name}.png"));
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            im.save(&dst).ok();
        }
        println!("{name:<40} {z:>8.2} {meand:>9.3} {maxd:>7}");
        csv.push_str(&format!(
            "{name},{w},{h},{strength:.2},{z:.3},{meand:.4},{maxd}\n"
        ));
    }
    std::fs::write(out_dir.join("report.csv"), csv).ok();
    println!(
        "\nWrote before/after/diff + report.csv to {}",
        out_dir.display()
    );
}
