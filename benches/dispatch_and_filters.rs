use zenbench::prelude::*;
use zencodecs::{EncodeRequest, ImageFormat};
use zenfilters::filters::FusedAdjust;
use zenfilters::{__bench_kernels as k, Filter, FilterContext, OklabPlanes};
use zenpixels::{PixelBuffer, PixelDescriptor};

fn make_textured_rgb8(w: u32, h: u32) -> PixelBuffer {
    let mut data = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 * 0.17;
            let fy = y as f32 * 0.23;
            let r = (128.0 + 90.0 * fx.sin() * (fy * 0.8).cos() + ((x ^ y) & 31) as f32)
                .clamp(0.0, 255.0) as u8;
            let g = (120.0 + 80.0 * (fx * 1.3 + 0.5).cos() + (((x * 5) ^ y) & 31) as f32)
                .clamp(0.0, 255.0) as u8;
            let b =
                (110.0 + 95.0 * (fx + fy).sin() + ((y * 7) & 31) as f32).clamp(0.0, 255.0) as u8;
            data.extend_from_slice(&[r, g, b]);
        }
    }
    PixelBuffer::from_vec(data, w, h, PixelDescriptor::RGB8_SRGB).unwrap()
}

fn make_odd_planes(w: u32, h: u32) -> OklabPlanes {
    let n = w as usize * h as usize;
    let mut planes = OklabPlanes::new(w, h);
    for i in 0..n {
        let t = i as f32 / n as f32;
        let x = i as u32 % w;
        let y = i as u32 / w;
        planes.l[i] = 0.2 + 0.6 * ((t * 17.0).sin().abs()) + ((x ^ y) & 7) as f32 * 0.01;
        planes.a[i] = (t * 23.0).sin() * 0.14;
        planes.b[i] = (t * 31.0).cos() * 0.12;
    }
    planes
}

fn bench_dispatch_and_filters(suite: &mut Suite) {
    // 517 * 387 = 200,079 elements (200,079 % 8 == 7), exercising the 7-element AVX2 tail every call.
    let (w, h) = (517u32, 387u32);
    let base_planes: &'static OklabPlanes = Box::leak(Box::new(make_odd_planes(w, h)));

    suite.group("zenfilters_odd_517x387", |g| {
        g.throughput(Throughput::Elements((w * h) as u64));
        g.throughput_unit("px");

        g.bench("hue_rotate_tail7", move |b| {
            let mut a = base_planes.a.clone();
            let mut c = base_planes.b.clone();
            b.iter(|| {
                a.copy_from_slice(&base_planes.a);
                c.copy_from_slice(&base_planes.b);
                k::hue_rotate(&mut a, &mut c, 0.866, 0.5);
                zenbench::black_box((a[a.len() - 1], c[c.len() - 1]))
            })
        });

        g.bench("vibrance_tail7", move |b| {
            let mut a = base_planes.a.clone();
            let mut c = base_planes.b.clone();
            b.iter(|| {
                a.copy_from_slice(&base_planes.a);
                c.copy_from_slice(&base_planes.b);
                k::vibrance(&mut a, &mut c, 0.45, 2.0);
                zenbench::black_box((a[a.len() - 1], c[c.len() - 1]))
            })
        });

        g.bench("fused_adjust_tail7", move |b| {
            let mut planes = base_planes.clone();
            let mut ctx = FilterContext::new();
            let mut adj = FusedAdjust::new();
            adj.exposure = 0.25;
            adj.contrast = 0.15;
            adj.saturation = 1.15;
            adj.vibrance = 0.35;
            adj.temperature = 0.12;
            adj.tint = -0.08;
            adj.dehaze = 0.10;
            b.iter(|| {
                planes.l.copy_from_slice(&base_planes.l);
                planes.a.copy_from_slice(&base_planes.a);
                planes.b.copy_from_slice(&base_planes.b);
                adj.apply(&mut planes, &mut ctx);
                zenbench::black_box((
                    planes.l[planes.l.len() - 1],
                    planes.a[planes.a.len() - 1],
                    planes.b[planes.b.len() - 1],
                ))
            })
        });
    });

    let (ew, eh) = (128u32, 96u32);
    let img: &'static PixelBuffer = Box::leak(Box::new(make_textured_rgb8(ew, eh)));

    suite.group("zencodecs_dispatch_128x96", |g| {
        g.throughput(Throughput::Elements((ew * eh) as u64));
        g.throughput_unit("px");

        for (label, fmt, q) in [
            ("oneshot_jpeg", ImageFormat::Jpeg, Some(80.0)),
            ("oneshot_png", ImageFormat::Png, None),
            ("oneshot_webp", ImageFormat::WebP, Some(80.0)),
            ("oneshot_bmp", ImageFormat::Bmp, None),
        ] {
            g.bench(label, move |b| {
                b.iter(|| {
                    let mut req = EncodeRequest::new(fmt);
                    if let Some(q) = q {
                        req = req.with_quality(q);
                    }
                    zenbench::black_box(req.encode(img.as_slice(), false).unwrap().into_vec())
                })
            });
        }

        for (label, fmt, q) in [
            ("stream_jpeg", ImageFormat::Jpeg, Some(80.0)),
            ("stream_png", ImageFormat::Png, None),
            ("stream_webp", ImageFormat::WebP, Some(80.0)),
            ("stream_bmp", ImageFormat::Bmp, None),
        ] {
            g.bench(label, move |b| {
                b.iter(|| {
                    let mut req = EncodeRequest::new(fmt);
                    if let Some(q) = q {
                        req = req.with_quality(q);
                    }
                    let mut se = req.build_streaming_encoder(ew, eh).unwrap();
                    se.encoder.push_rows(img.as_slice()).unwrap();
                    zenbench::black_box(se.encoder.finish().unwrap().into_vec())
                })
            });
        }
    });
}

zenbench::main!(bench_dispatch_and_filters);
