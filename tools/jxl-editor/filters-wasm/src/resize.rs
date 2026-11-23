use serde::Serialize;
use zenresize::{Filter, PixelDescriptor, ResizeConfig, Resizer};

#[derive(Serialize)]
struct KernelInfo {
    id: &'static str,
    label: &'static str,
    note: &'static str,
}

const CURATED_KERNELS: &[(&str, &str, &str, Filter)] = &[
    (
        "robidoux",
        "Robidoux (Balanced)",
        "Default crisp cubic without haloing",
        Filter::Robidoux,
    ),
    (
        "lanczos",
        "Lanczos 3",
        "Sharp sinc — photographic detail",
        Filter::Lanczos,
    ),
    (
        "lanczos_sharp",
        "Lanczos 3 Sharp",
        "Extra crisp edges",
        Filter::LanczosSharp,
    ),
    (
        "lanczos2",
        "Lanczos 2",
        "Clean sinc with minimal ringing",
        Filter::Lanczos2,
    ),
    (
        "ginseng",
        "Ginseng (Upscale)",
        "Jinc-windowed sinc — great for upscaling",
        Filter::Ginseng,
    ),
    (
        "ginseng_sharp",
        "Ginseng Sharp",
        "Crisp Jinc-windowed sinc",
        Filter::GinsengSharp,
    ),
    (
        "robidoux_sharp",
        "Robidoux Sharp",
        "Sharper cubic resampling",
        Filter::RobidouxSharp,
    ),
    (
        "mitchell",
        "Mitchell-Netravali",
        "Smooth balanced B=1/3, C=1/3",
        Filter::Mitchell,
    ),
    (
        "catmull_rom",
        "Catmull-Rom",
        "Interpolating bicubic spline",
        Filter::CatmullRom,
    ),
    (
        "cubic_b_spline",
        "Cubic B-Spline",
        "Ultra-smooth, zero ringing",
        Filter::CubicBSpline,
    ),
    (
        "hermite",
        "Hermite",
        "Smooth zero-ringing cubic",
        Filter::Hermite,
    ),
    (
        "linear",
        "Bilinear (Triangle)",
        "Simple fast linear blend",
        Filter::Linear,
    ),
    (
        "box",
        "Box (Nearest/Area)",
        "Pixel-art / crisp nearest scaling",
        Filter::Box,
    ),
];

pub fn parse_kernel(name: &str) -> Result<Filter, String> {
    if let Some((_, _, _, f)) = CURATED_KERNELS.iter().find(|(id, _, _, _)| *id == name) {
        return Ok(*f);
    }
    Filter::all()
        .iter()
        .copied()
        .find(|f| f.name() == name)
        .ok_or_else(|| format!("Unknown zenresize kernel '{name}'."))
}

pub fn resize_kernels_json_impl() -> String {
    let list: Vec<KernelInfo> = CURATED_KERNELS
        .iter()
        .map(|(id, label, note, _)| KernelInfo { id, label, note })
        .collect();
    serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
}

pub fn resize_rgba_impl(
    pixels: Vec<u8>,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    kernel: &str,
    linear: bool,
) -> Result<Vec<u8>, String> {
    if src_w == 0
        || src_h == 0
        || dst_w == 0
        || dst_h == 0
        || src_w > 8192
        || src_h > 8192
        || dst_w > 8192
        || dst_h > 8192
        || u64::from(src_w) * u64::from(src_h) > 24_000_000
        || u64::from(dst_w) * u64::from(dst_h) > 24_000_000
        || pixels.len() != src_w as usize * src_h as usize * 4
    {
        return Err("Invalid dimensions or RGBA buffer for zenresize.".into());
    }
    if src_w == dst_w && src_h == dst_h {
        return Ok(pixels);
    }
    let filter = parse_kernel(kernel)?;
    let mut builder = ResizeConfig::builder(src_w, src_h, dst_w, dst_h)
        .filter(filter)
        .format(PixelDescriptor::RGBA8_SRGB);
    builder = if linear {
        builder.linear()
    } else {
        builder.srgb()
    };
    let config = builder.build();
    let mut resizer = Resizer::try_new(&config).map_err(|e| e.to_string())?;
    Ok(resizer.resize(&pixels))
}
