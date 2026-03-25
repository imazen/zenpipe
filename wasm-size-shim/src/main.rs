//! WASM runtime demo — runs under wasmtime via `wasm32-wasip1`.
//!
//! Build:  RUSTFLAGS="-C target-feature=+simd128" cargo build --target wasm32-wasip1 --release --bin wasm-demo
//! Run:    wasmtime --wasm simd target/wasm32-wasip1/release/wasm-demo.wasm
//!
//! Generates a 256x256 gradient, runs it through the full pipeline, prints timings.

use std::time::Instant;
use zencodec::encode::{DynEncoderConfig, DynEncoder};
use zencodec::decode::DynDecoderConfig;

fn generate_gradient(w: u32, h: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            buf.extend_from_slice(&[
                (x * 255 / w) as u8,
                (y * 255 / h) as u8,
                ((x + y) * 255 / (w + h)) as u8,
                255,
            ]);
        }
    }
    buf
}

fn timed<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let result = f();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    // print timing inline — caller adds details
    print!("{name}: {ms:.2}ms");
    result
}

fn encode_via_zencodec(name: &str, cfg: &dyn DynEncoderConfig, pixels: &[u8], w: u32, h: u32) {
    let t = Instant::now();
    let mut job = cfg.dyn_job();
    let desc = zenpixels::PixelDescriptor::RGBA8_SRGB;
    match job.into_encoder() {
        Ok(mut enc) => {
            if let Ok(slice) = zenpixels::PixelSlice::new(pixels, w, h, w as usize * 4, desc) {
                let _ = enc.push_rows(slice);
                match enc.finish() {
                    Ok(out) => {
                        let ms = t.elapsed().as_secs_f64() * 1000.0;
                        println!("{name} encode: {} bytes, {ms:.2}ms", out.data().len());
                    }
                    Err(e) => println!("{name} finish failed: {e}"),
                }
            }
        }
        Err(e) => println!("{name} encoder creation failed: {e}"),
    }
}

fn decode_via_zencodec(name: &str, cfg: &dyn DynDecoderConfig, data: &[u8]) {
    let t = Instant::now();
    let job = cfg.dyn_job();
    let pref = [zenpixels::PixelDescriptor::RGBA8_SRGB];
    match job.into_streaming_decoder(std::borrow::Cow::Borrowed(data), &pref) {
        Ok(mut dec) => {
            let info = dec.info();
            let (iw, ih) = (info.width, info.height);
            let mut total = 0usize;
            loop {
                match dec.next_batch() {
                    Ok(Some((_y, px))) => total += px.as_strided_bytes().len(),
                    _ => break,
                }
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            println!("{name} decode: {iw}x{ih}, {total} bytes, {ms:.2}ms");
        }
        Err(e) => println!("{name} decode failed: {e}"),
    }
}

fn main() {
    // If SCALAR_ONLY env var is set, disable all SIMD tokens to force scalar/wide fallback
    // This gives a fair comparison: native scalar vs wasmtime scalar
    if std::env::var("SCALAR_ONLY").is_ok() {
        let _ = archmage::dangerously_disable_tokens_except_wasm(true);
        println!("*** SIMD tokens disabled — scalar/wide fallback only ***");
    }

    let w: u32 = 3840;
    let h: u32 = 2160;
    println!("zenpipe WASM demo ({w}x{h} gradient, 4K)");
    println!("==========================================");

    let pixels = generate_gradient(w, h);
    println!("Source: {}x{} RGBA ({} KB)\n", w, h, pixels.len() / 1024);

    // --- Resize via pipeline graph ---
    {
        let t = Instant::now();
        let fmt = zenpipe::format::RGBA8_SRGB;
        let mut g = zenpipe::graph::PipelineGraph::new();
        let sn = g.add_node(zenpipe::graph::NodeOp::Source);
        let rn = g.add_node(zenpipe::graph::NodeOp::Resize {
            w: 1280, h: 720,
            filter: Some(zenresize::Filter::Lanczos),
            sharpen_percent: None,
        });
        let on = g.add_node(zenpipe::graph::NodeOp::Output);
        g.add_edge(sn, rn, zenpipe::graph::EdgeKind::Input);
        g.add_edge(rn, on, zenpipe::graph::EdgeKind::Input);

        let mut sources = hashbrown::HashMap::new();
        sources.insert(sn, Box::new(zenpipe::sources::CallbackSource::from_data(&pixels, w, h, fmt, 16)) as Box<dyn zenpipe::Source>);

        match g.compile(sources) {
            Ok(mut pipeline) => {
                let mut out_size = 0;
                while let Ok(Some(strip)) = pipeline.next() { out_size += strip.as_strided_bytes().len(); }
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                println!("Resize {w}x{h} -> 1280x720: {} KB, {ms:.2}ms", out_size / 1024);
            }
            Err(e) => println!("Resize failed: {e}"),
        }
    }

    // --- Format conversion ---
    {
        let t = Instant::now();
        let from = zenpipe::format::RGBA8_SRGB;
        let to = zenpipe::format::RGBAF32_LINEAR;
        let mut g = zenpipe::graph::PipelineGraph::new();
        let sn = g.add_node(zenpipe::graph::NodeOp::Source);
        let cn = g.add_node(zenpipe::graph::NodeOp::PixelTransform(
            Box::new(zenpipe::ops::RowConverterOp::must(from, to)),
        ));
        let on = g.add_node(zenpipe::graph::NodeOp::Output);
        g.add_edge(sn, cn, zenpipe::graph::EdgeKind::Input);
        g.add_edge(cn, on, zenpipe::graph::EdgeKind::Input);

        let mut sources = hashbrown::HashMap::new();
        sources.insert(sn, Box::new(zenpipe::sources::CallbackSource::from_data(&pixels, w, h, from, 16)) as Box<dyn zenpipe::Source>);

        match g.compile(sources) {
            Ok(mut pipeline) => {
                let mut out_size = 0;
                while let Ok(Some(strip)) = pipeline.next() { out_size += strip.as_strided_bytes().len(); }
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                println!("Convert RGBA8 -> F32 linear: {} KB, {ms:.2}ms", out_size / 1024);
            }
            Err(e) => println!("Convert failed: {e}"),
        }
    }

    // --- Zenfilters (exposure + saturation + contrast + blur + sharpen) ---
    {
        let t = Instant::now();
        let fmt = zenpipe::format::RGBA8_SRGB;
        let config = zenfilters::PipelineConfig::default();
        if let Ok(mut pipe) = zenfilters::Pipeline::new(config) {
            let mut e = zenfilters::filters::Exposure::default(); e.stops = 0.5; pipe.push(Box::new(e));
            let mut sa = zenfilters::filters::Saturation::default(); sa.factor = 1.3; pipe.push(Box::new(sa));
            let mut c = zenfilters::filters::Contrast::default(); c.amount = 0.3; pipe.push(Box::new(c));

            let mut g = zenpipe::graph::PipelineGraph::new();
            let sn = g.add_node(zenpipe::graph::NodeOp::Source);
            let fn_ = g.add_node(zenpipe::graph::NodeOp::Filter(pipe));
            let on = g.add_node(zenpipe::graph::NodeOp::Output);
            g.add_edge(sn, fn_, zenpipe::graph::EdgeKind::Input);
            g.add_edge(fn_, on, zenpipe::graph::EdgeKind::Input);

            let mut sources = hashbrown::HashMap::new();
            sources.insert(sn, Box::new(zenpipe::sources::CallbackSource::from_data(&pixels, w, h, fmt, 16)) as Box<dyn zenpipe::Source>);

            match g.compile(sources) {
                Ok(mut pipeline) => {
                    let mut out_size = 0;
                    while let Ok(Some(strip)) = pipeline.next() { out_size += strip.as_strided_bytes().len(); }
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    println!("Filters (exp+sat+con): {} KB, {ms:.2}ms", out_size / 1024);
                }
                Err(e) => println!("Filter failed: {e}"),
            }
        }
    }

    // --- Zenfilters with neighborhood (blur + sharpen) ---
    {
        let t = Instant::now();
        let fmt = zenpipe::format::RGBA8_SRGB;
        let config = zenfilters::PipelineConfig::default();
        if let Ok(mut pipe) = zenfilters::Pipeline::new(config) {
            let mut bl = zenfilters::filters::Blur::default(); bl.sigma = 2.0; pipe.push(Box::new(bl));
            let mut sh = zenfilters::filters::Sharpen::default(); sh.amount = 0.5; pipe.push(Box::new(sh));

            let mut g = zenpipe::graph::PipelineGraph::new();
            let sn = g.add_node(zenpipe::graph::NodeOp::Source);
            let fn_ = g.add_node(zenpipe::graph::NodeOp::Filter(pipe));
            let on = g.add_node(zenpipe::graph::NodeOp::Output);
            g.add_edge(sn, fn_, zenpipe::graph::EdgeKind::Input);
            g.add_edge(fn_, on, zenpipe::graph::EdgeKind::Input);

            let mut sources = hashbrown::HashMap::new();
            sources.insert(sn, Box::new(zenpipe::sources::CallbackSource::from_data(&pixels, w, h, fmt, 16)) as Box<dyn zenpipe::Source>);

            match g.compile(sources) {
                Ok(mut pipeline) => {
                    let mut out_size = 0;
                    while let Ok(Some(strip)) = pipeline.next() { out_size += strip.as_strided_bytes().len(); }
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    println!("Filters (blur+sharp): {} KB, {ms:.2}ms", out_size / 1024);
                }
                Err(e) => println!("Filter failed: {e}"),
            }
        }
    }

    println!();

    // --- Codec round-trips ---

    // JPEG
    {
        let rgba: &[rgb::RGBA<u8>] = bytemuck::cast_slice(&pixels);
        let cfg = zenjpeg::encoder::EncoderConfig::ycbcr(85, zenjpeg::encoder::ChromaSubsampling::Quarter);
        let t = Instant::now();
        match cfg.request().encode(rgba, w, h) {
            Ok(jpeg) => {
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                println!("JPEG encode Q85: {} bytes, {ms:.2}ms", jpeg.len());
                decode_via_zencodec("JPEG", &zenjpeg::JpegDecoderConfig::default(), &jpeg);
            }
            Err(e) => println!("JPEG encode failed: {e}"),
        }
    }

    // PNG
    encode_via_zencodec("PNG", &zenpng::PngEncoderConfig::default(), &pixels, w, h);

    // WebP
    encode_via_zencodec("WebP", &zenwebp::WebpEncoderConfig::lossy().with_quality(80.0), &pixels, w, h);

    // JXL
    encode_via_zencodec("JXL", &zenjxl::JxlEncoderConfig::new(), &pixels, w, h);

    // GIF (use smaller size since quantization is expensive at 4K)
    {
        let gif_w: u32 = 256;
        let gif_h: u32 = 256;
        let gif_pixels = generate_gradient(gif_w, gif_h);
        let t = Instant::now();
        let rgba_pixels: Vec<zengif::Rgba> = bytemuck::cast_slice(&gif_pixels).to_vec();
        let frame = zengif::FrameInput::new(gif_w as u16, gif_h as u16, 0, rgba_pixels);
        let cfg = zengif::EncoderConfig::new();
        let limits = zengif::Limits::default();
        match zengif::encode_gif(vec![frame], gif_w as u16, gif_h as u16, cfg, limits, &zengif::Unstoppable) {
            Ok(gif) => {
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                println!("GIF encode 256x256: {} bytes, {ms:.2}ms", gif.len());
                decode_via_zencodec("GIF", &zengif::GifDecoderConfig::default(), &gif);
            }
            Err(e) => println!("GIF encode failed: {e}"),
        }
    }

    println!("\n==========================================");
    println!("Done.");
}
