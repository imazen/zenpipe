//! End-to-end qualification of PR 5 (`zenpipe::canonical`: 10-node typed DAG IR,
//! strict JSON v1 wire contract `P` (`steps` & `graph`), `OutputV2` wire response,
//! Display P3 `zenfilters::recipe`, `preserve` vs `preserve_bytes`, video poster extraction,
//! animation <-> video conversion, video -> video transcode with geometry/filters,
//! and independent `ffprobe` oracle verification).

#![cfg(all(
    feature = "std",
    feature = "canonical-json",
    feature = "all-media",
    feature = "nodes-jpeg",
    feature = "nodes-png",
    feature = "nodes-webp",
    feature = "job"
))]

use std::num::NonZeroU32;
use std::process::Command;
use std::sync::Arc;

use zencodec::CodecSet;
use zencodec::time::FrameDuration;
use zencodecs::bundles;
use zencodecs::engine::{Engine, LoopCount, WorkspacePolicy};
use zenpipe::canonical::{
    Bindings, HostServices, OutputV2, PipelineRuntime, WireLimitConfig, parse_json_v1,
    parse_pipeline_json,
};
use zenpixels::{ColorSpace, PixelBuf, PixelFormat, SampleLayout};

fn make_gradient_rgb8(w: u32, h: u32, phase: u8) -> PixelBuf {
    let mut data = vec![0u8; (w as usize) * (h as usize) * 3];
    for y in 0..h {
        for x in 0..w {
            let idx = ((y as usize) * (w as usize) + (x as usize)) * 3;
            data[idx] = (x * 3 + u32::from(phase) / 2).min(255) as u8;
            data[idx + 1] = (y * 3 + u32::from(phase) / 3).min(255) as u8;
            data[idx + 2] = ((x + y) * 2 + u32::from(phase) / 4).min(255) as u8;
        }
    }
    PixelBuf::from_bytes(data, w, h, PixelFormat::RGB8_SRGB).unwrap()
}

fn ffprobe_video(bytes: &[u8], ext: &str) -> (String, u32, u32, u32) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!(
        "zenpipe_probe_{}_{}.{}",
        std::process::id(),
        bytes.len(),
        ext
    ));
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_packets",
            "-show_entries",
            "stream=codec_name,width,height,nb_read_packets",
            "-of",
            "csv=p=0",
        ])
        .arg(&path)
        .output()
        .expect("ffprobe must be available");
    let _ = std::fs::remove_file(&path);
    assert!(
        out.status.success(),
        "ffprobe rejected {ext}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut parts = line.split(',');
    let codec = parts.next().unwrap_or("").to_string();
    let w: u32 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    let h: u32 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    let n: u32 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    (codec, w, h, n)
}

fn build_runtime() -> PipelineRuntime {
    let set = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let engine = Engine::new(set)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services());
    let services = HostServices::new()
        .with_workspace_policy(WorkspacePolicy::BestEffort)
        .with_trace(NonZeroU32::new(64).unwrap());
    PipelineRuntime::new(Arc::new(engine), services)
}

#[test]
fn canonical_json_v1_rejects_unknown_keys_and_cycles_and_runs_still_dag_with_p3_filter() {
    // 1. Strict key rejection: unknown keys MUST fail!
    let bad_key = br#"{"steps":[{"decode":{"input":"main","bogus_key":1}},{"encode":{"output":"out","format":"png"}}]}"#;
    assert!(parse_pipeline_json(bad_key, WireLimitConfig::default()).is_err());

    // 2. Cycle detection in explicit graph mode MUST fail!
    let cyclic = br#"{
        "graph": {
            "nodes": {
                "a": {"filter": {"grayscale": {}}},
                "b": {"filter": {"grayscale": {}}},
                "out": {"encode": {"output": "web", "format": "png"}}
            },
            "edges": [
                {"from": "a", "to": "b"},
                {"from": "b", "to": "a"},
                {"from": "b", "to": "out"}
            ]
        }
    }"#;
    assert!(parse_pipeline_json(cyclic, WireLimitConfig::default()).is_err());

    // 3. Explicit multi-output DAG with Display P3 color ingest + filter + resize + composite + OutputV2.
    let rt = build_runtime();
    let src_p3 = PixelBuf::from_bytes(
        make_gradient_rgb8(64, 48, 20).into_vec().unwrap(),
        64,
        48,
        PixelFormat::new(SampleLayout::RGB8, ColorSpace::DISPLAY_P3),
    )
    .unwrap();
    let overlay = make_gradient_rgb8(16, 16, 120);

    let json = br#"{
        "zenpipe": 1,
        "inputs": {"main": {}, "badge": {}},
        "outputs": {"hero": {}, "thumb": {}},
        "pipeline": {
            "graph": {
                "nodes": {
                    "src": {"decode": {"input": "main"}},
                    "ov": {"decode": {"input": "badge"}},
                    "graded": {"filter": {"exposure": {"stops": 0.25}}},
                    "comp": {"composite": {"x": 4, "y": 4, "opacity": 0.75, "blend": "source_over"}},
                    "small": {"geometry": {"resize": {"width": 32, "height": 24, "kernel": "lanczos3"}}},
                    "out_hero": {"encode": {"output": "hero", "format": "jpeg", "coding": {"calibrated_quality": 85}, "metadata": "web"}},
                    "out_thumb": {"encode": {"output": "thumb", "format": "webp", "coding": {"calibrated_quality": 80}}}
                },
                "edges": [
                    {"from": "src", "to": "graded"},
                    {"from": "graded", "to": "comp", "port": "canvas"},
                    {"from": "ov", "to": "comp", "port": "foreground"},
                    {"from": "comp", "to": "out_hero"},
                    {"from": "comp", "to": "small"},
                    {"from": "small", "to": "out_thumb"}
                ]
            }
        }
    }"#;

    let env = parse_json_v1(json, WireLimitConfig::default()).unwrap();
    let bindings = Bindings::new()
        .with_pixels("main", src_p3.as_view())
        .unwrap()
        .with_pixels("badge", overlay.as_view())
        .unwrap();
    let prepared = rt.prepare(&env.job, bindings).unwrap();
    assert_eq!(prepared.admission().output_count, 2);

    let res = prepared.run().unwrap();
    let hero = res.artifact("hero").unwrap();
    let thumb = res.artifact("thumb").unwrap();
    assert_eq!(hero.mime, "image/jpeg");
    assert_eq!((hero.width, hero.height), (64, 48));
    assert_eq!(thumb.mime, "image/webp");
    assert_eq!((thumb.width, thumb.height), (32, 24));
    assert!(!hero.bytes.is_empty() && !thumb.bytes.is_empty());

    // OutputV2 wire serialization round-trip!
    let wire: OutputV2 = res.output_v2();
    assert_eq!(wire.version, 2);
    assert_eq!(wire.outputs.len(), 2);
    let wire_json = serde_json::to_string(&wire).unwrap();
    let wire_back: OutputV2 = serde_json::from_str(&wire_json).unwrap();
    assert_eq!(wire, wire_back);
}

#[test]
fn canonical_pipeline_av1_vp8_vp9_mp4_webm_poster_animation_and_transcode_with_ffprobe() {
    let rt = build_runtime();

    // Build a 3-frame animation sequence (64x64, 40ms/frame = 25 fps).
    let f0 = make_gradient_rgb8(64, 64, 0);
    let f1 = make_gradient_rgb8(64, 64, 60);
    let f2 = make_gradient_rgb8(64, 64, 120);
    let dur = FrameDuration::from_millis(40);

    // 1. Animation -> AV1 MP4 (with explicit one-pass LoopPolicy::Set(Plays(1))).
    let anim_to_av1_json = br#"{
        "steps": [
            {"decode": {"input": "anim"}},
            {"encode": {
                "output": "video",
                "format": "mp4",
                "video_codec": "av1",
                "sequence": {"preserve": {"timing": "exact", "loops": {"set": {"plays": 1}}}},
                "publication": "verified"
            }}
        ]
    }"#;
    let job1 = parse_pipeline_json(anim_to_av1_json, WireLimitConfig::default()).unwrap();
    let res1 = rt
        .prepare(
            &job1,
            Bindings::new()
                .with_frames(
                    "anim",
                    vec![
                        (f0.as_view(), dur),
                        (f1.as_view(), dur),
                        (f2.as_view(), dur),
                    ],
                    LoopCount::Infinite,
                )
                .unwrap(),
        )
        .unwrap()
        .run()
        .unwrap();
    let av1_art = res1.artifact("video").unwrap();
    assert_eq!(av1_art.kind, "video");
    assert_eq!(av1_art.mime, "video/mp4");
    assert_eq!(av1_art.video_codec.as_deref(), Some("av1"));
    let (c, w, h, n) = ffprobe_video(&av1_art.bytes, "mp4");
    assert_eq!((c.as_str(), w, h, n), ("av1", 64, 64, 3));

    // 2. AV1 MP4 -> resized JPEG poster at exact rational presentation time 80/1000 s (frame 2)!
    let poster_json = br#"{
        "steps": [
            {"decode": {
                "input": "clip",
                "select": {"presentation_time": {"time": {"ticks": 80, "base": {"numerator": 1, "denominator": 1000}}, "mode": "at_or_before"}}
            }},
            {"geometry": {"resize": {"width": 32, "height": 32, "kernel": "robidoux"}}},
            {"encode": {"output": "poster", "format": "jpeg", "coding": {"calibrated_quality": 85}}}
        ]
    }"#;
    let job2 = parse_pipeline_json(poster_json, WireLimitConfig::default()).unwrap();
    let res2 = rt
        .prepare(
            &job2,
            Bindings::new().with_bytes("clip", &av1_art.bytes).unwrap(),
        )
        .unwrap()
        .run()
        .unwrap();
    let poster = res2.artifact("poster").unwrap();
    assert_eq!(poster.kind, "still");
    assert_eq!(poster.mime, "image/jpeg");
    assert_eq!((poster.width, poster.height), (32, 32));
    assert!(!poster.bytes.is_empty());

    // 3. AV1 MP4 -> resized VP9 WebM video transcode (exercises video decode -> per-frame resize -> VP9 WebM)!
    let transcode_json = br#"{
        "steps": [
            {"decode": {"input": "clip"}},
            {"geometry": {"resize": {"width": 48, "height": 48, "kernel": "robidoux"}}},
            {"encode": {
                "output": "webm_out",
                "format": "webm",
                "video_codec": "vp9",
                "sequence": {"preserve": {
                    "timing": {"quantize_endpoints": {"max_boundary_error": {"numerator": 1, "denominator": 1000}}},
                    "loops": {"set": {"plays": 1}}
                }},
                "publication": "verified"
            }}
        ]
    }"#;
    let job3 = parse_pipeline_json(transcode_json, WireLimitConfig::default()).unwrap();
    let res3 = rt
        .prepare(
            &job3,
            Bindings::new().with_bytes("clip", &av1_art.bytes).unwrap(),
        )
        .unwrap()
        .run()
        .unwrap();
    let vp9_art = res3.artifact("webm_out").unwrap();
    assert_eq!(vp9_art.kind, "video");
    assert_eq!(vp9_art.mime, "video/webm");
    assert_eq!(vp9_art.video_codec.as_deref(), Some("vp9"));
    let (c, w, h, n) = ffprobe_video(&vp9_art.bytes, "webm");
    assert_eq!((c.as_str(), w, h, n), ("vp9", 48, 48, 3));

    // 4. Direct zero-pixel-edit streaming video transcode: VP9 WebM -> VP8 WebM via MediaPump!
    let direct_json = br#"{
        "steps": [
            {"decode": {"input": "clip"}},
            {"encode": {
                "output": "vp8_out",
                "format": "webm",
                "video_codec": "vp8",
                "sequence": {"preserve": {
                    "timing": {"quantize_endpoints": {"max_boundary_error": {"numerator": 1, "denominator": 1000}}},
                    "loops": {"set": {"plays": 1}}
                }},
                "publication": "verified"
            }}
        ]
    }"#;
    let job4 = parse_pipeline_json(direct_json, WireLimitConfig::default()).unwrap();
    let res4 = rt
        .prepare(
            &job4,
            Bindings::new().with_bytes("clip", &vp9_art.bytes).unwrap(),
        )
        .unwrap()
        .run()
        .unwrap();
    let vp8_art = res4.artifact("vp8_out").unwrap();
    let (c, w, h, n) = ffprobe_video(&vp8_art.bytes, "webm");
    assert_eq!((c.as_str(), w, h, n), ("vp8", 48, 48, 3));

    // 5. Video -> Animated WebP (honoring explicit LoopPolicy::Set(Plays(2))) AND
    //    Encoded Animated WebP -> AV1 MP4 (testing encoded animated image ingress)!
    let vid_to_webp_json = br#"{
        "steps": [
            {"decode": {"input": "clip"}},
            {"encode": {
                "output": "anim_webp",
                "format": "webp",
                "sequence": {"preserve": {"timing": "exact", "loops": {"set": {"plays": 2}}}}
            }}
        ]
    }"#;
    let job5 = parse_pipeline_json(vid_to_webp_json, WireLimitConfig::default()).unwrap();
    let res5 = rt
        .prepare(
            &job5,
            Bindings::new().with_bytes("clip", &vp8_art.bytes).unwrap(),
        )
        .unwrap()
        .run()
        .unwrap();
    let webp_art = res5.artifact("anim_webp").unwrap();
    assert_eq!(webp_art.kind, "animation");
    assert_eq!(webp_art.mime, "image/webp");
    assert_eq!(webp_art.frames, Some(3));
    assert!(&webp_art.bytes[0..4] == b"RIFF" && &webp_art.bytes[8..12] == b"WEBP");

    // 6. Encoded Animated WebP -> AV1 MP4 (must decode all 3 frames, NOT collapse to 1 frame!).
    let webp_to_av1_json = br#"{
        "steps": [
            {"decode": {"input": "anim_webp"}},
            {"encode": {
                "output": "roundtrip_mp4",
                "format": "mp4",
                "video_codec": "av1",
                "sequence": {"preserve": {"timing": "exact", "loops": {"set": {"plays": 1}}}},
                "publication": "verified"
            }}
        ]
    }"#;
    let job6 = parse_pipeline_json(webp_to_av1_json, WireLimitConfig::default()).unwrap();
    let res6 = rt
        .prepare(
            &job6,
            Bindings::new()
                .with_bytes("anim_webp", &webp_art.bytes)
                .unwrap(),
        )
        .unwrap()
        .run()
        .unwrap();
    let rt_mp4 = res6.artifact("roundtrip_mp4").unwrap();
    let (c, w, h, n) = ffprobe_video(&rt_mp4.bytes, "mp4");
    assert_eq!((c.as_str(), w, h, n), ("av1", 48, 48, 3));
}

#[test]
fn canonical_pr5_regressions_strict_json_limits_fit_pad_multiply_and_selection() {
    // 1. Strict JSON rejects duplicate keys, extra keys on P, unused graph ports, and out-of-range i32!
    let dup_key = br#"{"steps":[{"decode":{"input":"a","input":"b"}},{"encode":{"output":"o","format":"png"}}]}"#;
    assert!(parse_pipeline_json(dup_key, WireLimitConfig::default()).is_err());

    let extra_on_p = br#"{"steps":[{"canvas":{"width":4,"height":4}},{"encode":{"output":"o","format":"png"}}],"extra":1}"#;
    assert!(parse_pipeline_json(extra_on_p, WireLimitConfig::default()).is_err());

    let unused_port = br#"{
        "graph": {
            "nodes": {
                "a": {"canvas": {"width": 4, "height": 4}},
                "b": {"canvas": {"width": 4, "height": 4}},
                "g": {"filter": {"grayscale": {}}},
                "o": {"encode": {"output": "out", "format": "png"}}
            },
            "edges": [
                {"from": "a", "to": "g", "port": "input"},
                {"from": "b", "to": "g", "port": "unused"},
                {"from": "g", "to": "o"}
            ]
        }
    }"#;
    assert!(parse_pipeline_json(unused_port, WireLimitConfig::default()).is_err());

    let overflow_offset = br#"{"steps":[{"canvas":{"width":4,"height":4}},{"geometry":{"canvas":{"width":8,"height":8,"offset_x":2147483648}}},{"encode":{"output":"o","format":"png"}}]}"#;
    assert!(parse_pipeline_json(overflow_offset, WireLimitConfig::default()).is_err());

    // 2. Host/job ceilings enforce max_dimension & max_output_bytes on Canvas + OutputOp::Pixels!
    let rt = build_runtime();
    let mut b = zenpipe::canonical::GraphBuilder::new();
    let c_id = b
        .image(zenpipe::canonical::ImageOp::Canvas {
            size: zenlayout::Size::new(2, 2),
            fill_rgba: [255, 0, 0, 255],
        })
        .unwrap();
    b.output(zenpipe::canonical::OutputOp::Pixels {
        src: c_id,
        slot: zenpipe::canonical::SlotName::new("px").unwrap(),
        format: None,
    })
    .unwrap();
    let mut job_dim = zenpipe::canonical::CanonicalJob::from_graph(b.finish().unwrap());
    job_dim.max_dimension = Some(1);
    assert!(
        rt.prepare(&job_dim, Bindings::new())
            .unwrap()
            .run()
            .is_err()
    );

    let mut job_out = job_dim.clone();
    job_out.max_dimension = None;
    job_out.max_output_bytes = Some(1);
    assert!(
        rt.prepare(&job_out, Bindings::new())
            .unwrap()
            .run()
            .is_err()
    );

    // 3. Constrain FitPad executes complete layout (200x100 -> FitPad 100x100 => 100x100 canvas)!
    let fit_pad_json = br#"{
        "steps": [
            {"canvas": {"width": 200, "height": 100, "fill": [255, 0, 0, 255]}},
            {"geometry": {"constrain": {"mode": "fit_pad", "width": 100, "height": 100, "canvas_color": [0, 0, 255, 255]}}},
            {"encode": {"output": "padded", "format": "png"}}
        ]
    }"#;
    let fp_job = parse_pipeline_json(fit_pad_json, WireLimitConfig::default()).unwrap();
    let fp_res = rt.prepare(&fp_job, Bindings::new()).unwrap().run().unwrap();
    let padded = fp_res.artifact("padded").unwrap();
    assert_eq!((padded.width, padded.height), (100, 100));

    // 4. Multiply over transparent black MUST retain opaque red (W3C Compositing Level 1)!
    let mut b2 = zenpipe::canonical::GraphBuilder::new();
    let bg = b2
        .image(zenpipe::canonical::ImageOp::Canvas {
            size: zenlayout::Size::new(4, 4),
            fill_rgba: [0, 0, 0, 0],
        })
        .unwrap();
    let fg = b2
        .image(zenpipe::canonical::ImageOp::Canvas {
            size: zenlayout::Size::new(4, 4),
            fill_rgba: [255, 0, 0, 255],
        })
        .unwrap();
    let comp = b2
        .image(zenpipe::canonical::ImageOp::Composite(
            zenpipe::canonical::CompositeOp {
                canvas: bg,
                foreground: fg,
                x: 0,
                y: 0,
                opacity: 1.0,
                blend: zenpipe::canonical::BlendKind::Multiply,
            },
        ))
        .unwrap();
    b2.output(zenpipe::canonical::OutputOp::Pixels {
        src: comp,
        slot: zenpipe::canonical::SlotName::new("out").unwrap(),
        format: Some(PixelFormat::RGBA8_SRGB),
    })
    .unwrap();
    let mult_job = zenpipe::canonical::CanonicalJob::from_graph(b2.finish().unwrap());
    let mult_res = rt
        .prepare(&mult_job, Bindings::new())
        .unwrap()
        .run()
        .unwrap();
    let mult_px = mult_res.artifact("out").unwrap().pixels.as_ref().unwrap();
    let first_pixel = &mult_px.as_view().row(0)[..4];
    assert_eq!(
        first_pixel,
        &[255, 0, 0, 255],
        "opaque red Multiply over transparent black must remain opaque red"
    );

    // 5. Out-of-range PresentationIndex on bound Frames MUST refuse!
    let f0 = make_gradient_rgb8(16, 16, 0);
    let bad_sel_json = br#"{
        "steps": [
            {"decode": {"input": "seq", "select": {"presentation_index": {"index": 5}}}},
            {"encode": {"output": "out", "format": "png"}}
        ]
    }"#;
    let bad_sel_job = parse_pipeline_json(bad_sel_json, WireLimitConfig::default()).unwrap();
    let bad_sel_res = rt
        .prepare(
            &bad_sel_job,
            Bindings::new()
                .with_frames(
                    "seq",
                    vec![(f0.as_view(), FrameDuration::from_millis(40))],
                    LoopCount::once(),
                )
                .unwrap(),
        )
        .unwrap()
        .run();
    assert!(bad_sel_res.is_err());

    // 6. Strict presence-sensitive JSON rejects malformed field types instead of defaulting!
    let bad_pt_mode = br#"{
        "steps": [
            {"decode": {"input": "clip", "select": {"presentation_time": {"time": {"ticks": 0, "base": {"numerator": 1, "denominator": 1000}}, "mode": 17}}}},
            {"encode": {"output": "o", "format": "png"}}
        ]
    }"#;
    assert!(parse_pipeline_json(bad_pt_mode, WireLimitConfig::default()).is_err());

    let bad_tracks_copy = br#"{
        "steps": [
            {"canvas": {"width": 4, "height": 4}},
            {"encode": {"output": "o", "format": "mp4", "video_codec": "av1", "tracks": {"select": {"video": 0, "copy": "not-an-array"}}}}
        ]
    }"#;
    assert!(parse_pipeline_json(bad_tracks_copy, WireLimitConfig::default()).is_err());

    let bad_gain_map = br#"{
        "steps": [
            {"canvas": {"width": 4, "height": 4}},
            {"gain_map": {"reconstruct": {"headroom_stops": 1, "rendition": 17}}},
            {"encode": {"output": "o", "format": "png"}}
        ]
    }"#;
    assert!(parse_pipeline_json(bad_gain_map, WireLimitConfig::default()).is_err());

    let bad_tm_method = br#"{
        "steps": [
            {"canvas": {"width": 4, "height": 4}},
            {"tone_map": {"method": 42}},
            {"encode": {"output": "o", "format": "png"}}
        ]
    }"#;
    assert!(parse_pipeline_json(bad_tm_method, WireLimitConfig::default()).is_err());

    // 7. Cumulative live memory cap rejects two 2x2 RGBA canvases (32 B live) under max_memory_bytes = 16!
    let mut b_mem = zenpipe::canonical::GraphBuilder::new();
    let c1 = b_mem
        .image(zenpipe::canonical::ImageOp::Canvas {
            size: zenlayout::Size::new(2, 2),
            fill_rgba: [10, 20, 30, 255],
        })
        .unwrap();
    let c2 = b_mem
        .image(zenpipe::canonical::ImageOp::Canvas {
            size: zenlayout::Size::new(2, 2),
            fill_rgba: [40, 50, 60, 255],
        })
        .unwrap();
    b_mem
        .output(zenpipe::canonical::OutputOp::Pixels {
            src: c1,
            slot: zenpipe::canonical::SlotName::new("o1").unwrap(),
            format: None,
        })
        .unwrap();
    b_mem
        .output(zenpipe::canonical::OutputOp::Pixels {
            src: c2,
            slot: zenpipe::canonical::SlotName::new("o2").unwrap(),
            format: None,
        })
        .unwrap();
    let job_mem = zenpipe::canonical::CanonicalJob::from_graph(b_mem.finish().unwrap());
    let tight_limits = zencodec::ResourceLimits::default().with_max_memory(16);
    let set_mem = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let eng_mem = Engine::new(set_mem)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services());
    let rt_mem = PipelineRuntime::new(
        Arc::new(eng_mem),
        HostServices::new()
            .with_workspace_policy(WorkspacePolicy::BestEffort)
            .with_limits(tight_limits),
    );
    assert!(
        rt_mem
            .prepare(&job_mem, Bindings::new())
            .unwrap()
            .run()
            .is_err(),
        "two simultaneously live 16-byte canvases must exceed 16-byte max_memory_bytes"
    );

    // 7b. 2x2 RGBA8 neighborhood blur (16 B src + 64 B linear + 64 B dst + 64 B planar + >= 64 B Blur op scratch >= 272 B)
    //     MUST be refused even under max_memory_bytes = 208!
    let blur_json = br#"{
        "steps": [
            {"canvas": {"width": 2, "height": 2, "fill": [10, 20, 30, 255]}},
            {"filter": {"blur": {"sigma": 1.0}}},
            {"encode": {"output": "o", "format": "png"}}
        ]
    }"#;
    let blur_job = parse_pipeline_json(blur_json, WireLimitConfig::default()).unwrap();
    let set_blur = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let eng_blur = Engine::new(set_blur)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services());
    let rt_blur = PipelineRuntime::new(
        Arc::new(eng_blur),
        HostServices::new()
            .with_workspace_policy(WorkspacePolicy::BestEffort)
            .with_limits(zencodec::ResourceLimits::default().with_max_memory(208)),
    );
    assert!(
        rt_blur
            .prepare(&blur_job, Bindings::new())
            .unwrap()
            .run()
            .is_err(),
        "2x2 RGBA8 neighborhood blur (including Blur::apply scratch > 208 B) must exceed 208 B max_memory_bytes"
    );

    // 7c. 16x16 RGBA8 sigma-10 stackblur (scalar transpose + stack >= 17,408 B) MUST be refused under 15,664 B!
    let stackblur_json = br#"{
        "steps": [
            {"canvas": {"width": 16, "height": 16, "fill": [10, 20, 30, 255]}},
            {"filter": {"blur": {"sigma": 10.0}}},
            {"encode": {"output": "o", "format": "png"}}
        ]
    }"#;
    let sb_job = parse_pipeline_json(stackblur_json, WireLimitConfig::default()).unwrap();
    let set_sb = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let eng_sb = Engine::new(set_sb)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services());
    let rt_sb = PipelineRuntime::new(
        Arc::new(eng_sb),
        HostServices::new()
            .with_workspace_policy(WorkspacePolicy::BestEffort)
            .with_limits(zencodec::ResourceLimits::default().with_max_memory(15_664)),
    );
    assert!(
        rt_sb
            .prepare(&sb_job, Bindings::new())
            .unwrap()
            .run()
            .is_err(),
        "16x16 RGBA8 sigma-10 stackblur must exceed 15,664 B max_memory_bytes"
    );

    // 7d. 9x90 RGBA8 sigma-40 SIMD stackblur with scalar tail (col_buf + col_out + scalar stack >= 56,340 B)
    //     MUST be refused under 56,312 B!
    let tail_blur_json = br#"{
        "steps": [
            {"canvas": {"width": 9, "height": 90, "fill": [10, 20, 30, 255]}},
            {"filter": {"blur": {"sigma": 40.0}}},
            {"encode": {"output": "o", "format": "png"}}
        ]
    }"#;
    let tb_job = parse_pipeline_json(tail_blur_json, WireLimitConfig::default()).unwrap();
    let set_tb = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let eng_tb = Engine::new(set_tb)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services());
    let rt_tb = PipelineRuntime::new(
        Arc::new(eng_tb),
        HostServices::new()
            .with_workspace_policy(WorkspacePolicy::BestEffort)
            .with_limits(zencodec::ResourceLimits::default().with_max_memory(56_312)),
    );
    assert!(
        rt_tb
            .prepare(&tb_job, Bindings::new())
            .unwrap()
            .run()
            .is_err(),
        "9x90 RGBA8 sigma-40 stackblur (including SIMD scalar tail buffers) must exceed 56,312 B max_memory_bytes"
    );

    // 8. zenlayout exact u128 rounding accepts representable u32::MAX target (1717986918 x 4294967293 -> w=1717986919).
    let exact_layout =
        zenlayout::Constraint::width_only(zenlayout::ConstraintMode::Fit, 1_717_986_919)
            .compute(1_717_986_918, 4_294_967_293)
            .expect("representable u32::MAX derived height must not overflow");
    assert_eq!(exact_layout.resize_to.width, 1_717_986_919);
    assert_eq!(exact_layout.resize_to.height, u32::MAX);

    // 9. ExtendCanvas preserves Display P3 color space & verbatim source samples even with translucent fill!
    let p3_rgba_fmt = PixelFormat::new(
        SampleLayout::RGBA8,
        ColorSpace::DISPLAY_P3.with_alpha(zenpixels::AlphaMode::Straight),
    );
    let src_p3_rgba = PixelBuf::from_bytes(
        vec![
            250, 12, 77, 255, 11, 240, 33, 200, 99, 88, 77, 150, 5, 6, 7, 255,
        ],
        2,
        2,
        p3_rgba_fmt.clone(),
    )
    .unwrap();
    let mut b_ext = zenpipe::canonical::GraphBuilder::new();
    let dec_p3 = b_ext
        .image(zenpipe::canonical::ImageOp::Decode(
            zenpipe::canonical::DecodeNodeOp {
                slot: zenpipe::canonical::SlotName::new("p3").unwrap(),
                orientation: zencodecs::engine::DecodeOrientation::Apply,
                select: None,
            },
        ))
        .unwrap();
    let ext_p3 = b_ext
        .image(zenpipe::canonical::ImageOp::Geometry(
            zenpipe::canonical::GeometryOp::ExtendCanvas {
                src: dec_p3,
                canvas: zenlayout::Size::new(4, 4),
                offset_x: 1,
                offset_y: 1,
                fill_rgba: [20, 40, 60, 128],
            },
        ))
        .unwrap();
    b_ext
        .output(zenpipe::canonical::OutputOp::Pixels {
            src: ext_p3,
            slot: zenpipe::canonical::SlotName::new("p3_out").unwrap(),
            format: None,
        })
        .unwrap();
    let ext_job = zenpipe::canonical::CanonicalJob::from_graph(b_ext.finish().unwrap());
    let ext_res = rt
        .prepare(
            &ext_job,
            Bindings::new()
                .with_pixels("p3", src_p3_rgba.as_view())
                .unwrap(),
        )
        .unwrap()
        .run()
        .unwrap();
    let ext_px = ext_res.artifact("p3_out").unwrap().pixels.as_ref().unwrap();
    assert_eq!(
        ext_px.format(),
        &p3_rgba_fmt,
        "ExtendCanvas with translucent fill must preserve Display P3 RGBA8"
    );
    assert_eq!(
        &ext_px.as_view().row(1)[4..12],
        &[250, 12, 77, 255, 11, 240, 33, 200],
        "ExtendCanvas must preserve existing Display P3 source samples byte-for-byte"
    );

    // 9b. Opaque bit-packed U10_LSB source with translucent ExtendCanvas MUST refuse rather than
    //     silently rescaling 1/1023 codes onto a 1/65535 full-width grid!
    let u10_fmt = PixelFormat::new(
        SampleLayout::new(zenpixels::SampleType::U16, zenpixels::ChannelOrder::Rgb)
            .with_packing(zenpixels::BitPacking::U10_LSB),
        ColorSpace::SRGB,
    );
    let src_u10 = PixelBuf::from_bytes(vec![1u8, 0, 2, 0, 3, 0], 1, 1, u10_fmt).unwrap();
    let u10_err = rt
        .prepare(
            &ext_job,
            Bindings::new()
                .with_pixels("p3", src_u10.as_view())
                .unwrap(),
        )
        .unwrap()
        .run();
    assert!(
        u10_err.is_err(),
        "translucent ExtendCanvas on bit-packed U10_LSB source must refuse rather than rescale sample grid"
    );

    // 10. Composite Multiply preserves positive float alpha down to 2^-24 on RGBAF32_LINEAR backdrop!
    let tiny_alpha = 2.0f32.powi(-24);
    let bg_f32 =
        PixelBuf::from_bytes(vec![0u8; 2 * 2 * 16], 2, 2, PixelFormat::RGBAF32_LINEAR).unwrap();
    let fg_f32_px: Vec<u8> = bytemuck::cast_slice(&[1.0f32, 0.0, 0.0, 1.0].repeat(4)).to_vec();
    let fg_f32 = PixelBuf::from_bytes(fg_f32_px, 2, 2, PixelFormat::RGBAF32_LINEAR).unwrap();
    let mut b_tiny = zenpipe::canonical::GraphBuilder::new();
    let bg_id = b_tiny
        .image(zenpipe::canonical::ImageOp::Decode(
            zenpipe::canonical::DecodeNodeOp {
                slot: zenpipe::canonical::SlotName::new("bg").unwrap(),
                orientation: zencodecs::engine::DecodeOrientation::Apply,
                select: None,
            },
        ))
        .unwrap();
    let fg_id = b_tiny
        .image(zenpipe::canonical::ImageOp::Decode(
            zenpipe::canonical::DecodeNodeOp {
                slot: zenpipe::canonical::SlotName::new("fg").unwrap(),
                orientation: zencodecs::engine::DecodeOrientation::Apply,
                select: None,
            },
        ))
        .unwrap();
    let tiny_comp = b_tiny
        .image(zenpipe::canonical::ImageOp::Composite(
            zenpipe::canonical::CompositeOp {
                canvas: bg_id,
                foreground: fg_id,
                x: 0,
                y: 0,
                opacity: tiny_alpha,
                blend: zenpipe::canonical::BlendKind::Multiply,
            },
        ))
        .unwrap();
    b_tiny
        .output(zenpipe::canonical::OutputOp::Pixels {
            src: tiny_comp,
            slot: zenpipe::canonical::SlotName::new("out").unwrap(),
            format: Some(PixelFormat::RGBAF32_LINEAR),
        })
        .unwrap();
    let tiny_job = zenpipe::canonical::CanonicalJob::from_graph(b_tiny.finish().unwrap());
    let tiny_res = rt
        .prepare(
            &tiny_job,
            Bindings::new()
                .with_pixels("bg", bg_f32.as_view())
                .unwrap()
                .with_pixels("fg", fg_f32.as_view())
                .unwrap(),
        )
        .unwrap()
        .run()
        .unwrap();
    let tiny_px = tiny_res.artifact("out").unwrap().pixels.as_ref().unwrap();
    let px0: &[f32] = bytemuck::cast_slice(&tiny_px.as_view().row(0)[..16]);
    assert_eq!(
        px0[0], 1.0,
        "tiny-alpha Multiply must keep unpremultiplied red = 1.0"
    );
    assert_eq!(
        px0[3], tiny_alpha,
        "tiny-alpha Multiply must preserve float alpha 2^-24"
    );

    // 10b. Subnormal positive opacity 2^-129 MUST produce finite [1.0, 0.0, 0.0, 2^-129] (no inf/NaN)!
    let subnormal_alpha = f32::from_bits(0x0010_0000); // exact IEEE-754 subnormal 2^-129
    assert!(subnormal_alpha > 0.0 && subnormal_alpha.is_subnormal());
    let mut b_sub = zenpipe::canonical::GraphBuilder::new();
    let bg_s = b_sub
        .image(zenpipe::canonical::ImageOp::Decode(
            zenpipe::canonical::DecodeNodeOp {
                slot: zenpipe::canonical::SlotName::new("bg").unwrap(),
                orientation: zencodecs::engine::DecodeOrientation::Apply,
                select: None,
            },
        ))
        .unwrap();
    let fg_s = b_sub
        .image(zenpipe::canonical::ImageOp::Decode(
            zenpipe::canonical::DecodeNodeOp {
                slot: zenpipe::canonical::SlotName::new("fg").unwrap(),
                orientation: zencodecs::engine::DecodeOrientation::Apply,
                select: None,
            },
        ))
        .unwrap();
    let sub_comp = b_sub
        .image(zenpipe::canonical::ImageOp::Composite(
            zenpipe::canonical::CompositeOp {
                canvas: bg_s,
                foreground: fg_s,
                x: 0,
                y: 0,
                opacity: subnormal_alpha,
                blend: zenpipe::canonical::BlendKind::Multiply,
            },
        ))
        .unwrap();
    b_sub
        .output(zenpipe::canonical::OutputOp::Pixels {
            src: sub_comp,
            slot: zenpipe::canonical::SlotName::new("out").unwrap(),
            format: Some(PixelFormat::RGBAF32_LINEAR),
        })
        .unwrap();
    let sub_job = zenpipe::canonical::CanonicalJob::from_graph(b_sub.finish().unwrap());
    let sub_res = rt
        .prepare(
            &sub_job,
            Bindings::new()
                .with_pixels("bg", bg_f32.as_view())
                .unwrap()
                .with_pixels("fg", fg_f32.as_view())
                .unwrap(),
        )
        .unwrap()
        .run()
        .unwrap();
    let sub_px = sub_res.artifact("out").unwrap().pixels.as_ref().unwrap();
    let spx0: &[f32] = bytemuck::cast_slice(&sub_px.as_view().row(0)[..16]);
    assert!(
        spx0.iter().all(|v| v.is_finite()),
        "subnormal alpha composite must stay finite: {spx0:?}"
    );
    assert_eq!(spx0[0], 1.0);
    assert_eq!(spx0[1], 0.0);
    assert_eq!(spx0[2], 0.0);
    assert_eq!(spx0[3], subnormal_alpha);

    // 11. Exact tight-cap 2-frame 8x8 GIF with full-canvas Restore-Previous disposal (disposal=3):
    // encoded copy + 576 B GIF workspace (64 * 9) + 512 B retained outputs (2 * 256 B) = len + 1088 B.
    // Must SUCCEED at exact cap `len + 1088` (zengif gets its full 576 B reserved workspace rather
    // than being starved by `mem.remaining() == 512 B`) and FAIL at `len + 1087`!
    let gif_restore_prev = build_two_frame_8x8_restore_prev_gif();
    let exact_cap = gif_restore_prev.len() as u64 + 1088;
    let build_gif_decode_job = || {
        let mut gb = zenpipe::canonical::GraphBuilder::new();
        let src = gb
            .image(zenpipe::canonical::ImageOp::Decode(
                zenpipe::canonical::DecodeNodeOp {
                    slot: zenpipe::canonical::SlotName::new("gif").unwrap(),
                    orientation: zencodecs::engine::DecodeOrientation::Apply,
                    select: None,
                },
            ))
            .unwrap();
        gb.output(zenpipe::canonical::OutputOp::Pixels {
            src,
            slot: zenpipe::canonical::SlotName::new("out").unwrap(),
            format: None,
        })
        .unwrap();
        zenpipe::canonical::CanonicalJob::from_graph(gb.finish().unwrap())
    };
    let gif_decode_job = build_gif_decode_job();
    let gif_set = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let gif_engine = Arc::new(
        Engine::new(gif_set)
            .unwrap()
            .with_color_services(bundles::moxcms_color_services()),
    );
    let rt_exact = PipelineRuntime::new(
        gif_engine.clone(),
        HostServices::new()
            .with_workspace_policy(WorkspacePolicy::BestEffort)
            .with_limits(zencodec::ResourceLimits::default().with_max_memory(exact_cap)),
    );
    let exact_res = rt_exact
        .prepare(
            &gif_decode_job,
            Bindings::new()
                .with_bytes("gif", &gif_restore_prev)
                .unwrap(),
        )
        .unwrap()
        .run()
        .expect("2-frame 8x8 Restore-Previous GIF must succeed at exact cap len + 1088 B");
    let exact_px = exact_res.artifact("out").unwrap().pixels.as_ref().unwrap();
    assert_eq!(exact_px.width(), 8);
    assert_eq!(exact_px.height(), 8);
    assert_eq!(
        &exact_px.as_view().row(0)[..4],
        &[255, 0, 0, 255],
        "frame 0 of 2-frame Restore-Previous GIF must decode to exact red"
    );

    // 1 byte below exact cap (len + 1087 B) MUST fail with LimitExceeded!
    let rt_short = PipelineRuntime::new(
        gif_engine,
        HostServices::new()
            .with_workspace_policy(WorkspacePolicy::BestEffort)
            .with_limits(zencodec::ResourceLimits::default().with_max_memory(exact_cap - 1)),
    );
    let short_err = rt_short
        .prepare(
            &gif_decode_job,
            Bindings::new()
                .with_bytes("gif", &gif_restore_prev)
                .unwrap(),
        )
        .unwrap()
        .run()
        .expect_err("2-frame 8x8 Restore-Previous GIF must fail at cap len + 1087 B");
    assert!(
        matches!(short_err.error(), zenpipe::PipeError::LimitExceeded(_)),
        "expected LimitExceeded at cap len + 1087 B, got: {short_err}"
    );
}

/// Minimal valid 2-frame 8x8 GIF89a whose frame 0 uses disposal method 3 (Restore to Previous)
/// across the full 8x8 canvas, triggering `zengif`'s full 9 B/px (576 B) internal workspace.
fn build_two_frame_8x8_restore_prev_gif() -> Vec<u8> {
    // 2-color Global Color Table (red = 0, blue = 1), 8x8 canvas, min LZW code size = 2.
    // Clear code = 4, EOI = 5. For 64 identical symbols (0 or 1), emit Clear + 64 literal 3-bit
    // codes + EOI packed LSB-first with explicit Clear every 3 codes so code width stays 3 bits!
    fn lzw_solid_64(sym: u8) -> Vec<u8> {
        let mut bits: u64 = 0;
        let mut nbits: u32 = 0;
        let mut out = Vec::new();
        let mut emit = |code: u16| {
            bits |= (u64::from(code)) << nbits;
            nbits += 3;
            while nbits >= 8 {
                out.push((bits & 0xFF) as u8);
                bits >>= 8;
                nbits -= 8;
            }
        };
        // Max 3-bit code is 7; dictionary starts at 6, so 2 emissions stay in 3-bit width before Clear.
        for chunk in 0..32 {
            emit(4); // Clear -> resets code width to 3 bits
            emit(u16::from(sym));
            if chunk < 32 {
                emit(u16::from(sym));
            }
        }
        emit(4);
        emit(5); // EOI
        if nbits > 0 {
            out.push((bits & 0xFF) as u8);
        }
        out
    }

    let mut g = Vec::new();
    g.extend_from_slice(b"GIF89a");
    // Logical Screen Descriptor: 8x8, GCT flag=1 (size=0 -> 2 colors), bg=0, par=0
    g.extend_from_slice(&[8, 0, 8, 0, 0x80, 0, 0]);
    // GCT: color 0 = red, color 1 = blue
    g.extend_from_slice(&[255, 0, 0, 0, 0, 255]);
    // Frame 0: GCE with disposal = 3 (Restore to Previous: packed = 0x0C), delay = 5 (50ms)
    g.extend_from_slice(&[0x21, 0xF9, 0x04, 0x0C, 0x05, 0x00, 0x00, 0x00]);
    g.extend_from_slice(&[0x2C, 0, 0, 0, 0, 8, 0, 8, 0, 0x00]);
    g.push(2); // LZW min code size
    let f0 = lzw_solid_64(0);
    g.push(f0.len() as u8);
    g.extend_from_slice(&f0);
    g.push(0x00); // block terminator
    // Frame 1: GCE with disposal = 1 (Do Not Dispose: packed = 0x04), delay = 5 (50ms)
    g.extend_from_slice(&[0x21, 0xF9, 0x04, 0x04, 0x05, 0x00, 0x00, 0x00]);
    g.extend_from_slice(&[0x2C, 0, 0, 0, 0, 8, 0, 8, 0, 0x00]);
    g.push(2);
    let f1 = lzw_solid_64(1);
    g.push(f1.len() as u8);
    g.extend_from_slice(&f1);
    g.push(0x00);
    // Trailer
    g.push(0x3B);
    g
}

#[test]
fn prepared_job_run_with_pulse_cancels_and_preserves_typed_target_unmet() {
    use enough::{Stop, StopReason};
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use zencodec::{CodecSet, JobResources, SharedPulse};
    use zencodecs::bundles;
    use zencodecs::engine::{EngineError, EngineResult, Failure, Metric, MetricScorer};
    use zenpixels::{PixelFormat, PixelView};

    struct ConstantLowScorer;
    impl MetricScorer for ConstantLowScorer {
        fn metric(&self) -> Metric {
            Metric::ZensimA
        }
        fn revision(&self) -> &'static str {
            "const-low.r1"
        }
        fn comparison_format(&self) -> PixelFormat {
            PixelFormat::RGBA8_SRGB
        }
        fn score(
            &self,
            _reference: PixelView<'_>,
            _candidate: PixelView<'_>,
            _resources: &JobResources,
        ) -> EngineResult<f32> {
            Ok(42.0)
        }
    }

    let set = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let engine = Arc::new(
        Engine::new(set)
            .unwrap()
            .with_color_services(bundles::moxcms_color_services())
            .with_metric_scorer(Arc::new(ConstantLowScorer), NonZeroU32::new(4).unwrap())
            .unwrap(),
    );

    // 1. Typed Failure::TargetUnmet is preserved inside PipeError::Codec (not stringified into Op)
    //    and runs under default WorkspacePolicy::Strict!
    let job = parse_pipeline_json(
        br#"{
            "steps": [
                {"canvas": {"width": 32, "height": 32, "fill": [120, 160, 200, 255]}},
                {"encode": {"output": "out", "format": "jpeg", "coding": {"verify": {"metric": "zensim_a", "value": 90.0}}}}
            ]
        }"#,
        WireLimitConfig::default(),
    )
    .unwrap();

    let rt = PipelineRuntime::new(engine.clone(), HostServices::new());
    let err = rt
        .prepare(&job, Bindings::new())
        .unwrap()
        .run_with_pulse(SharedPulse::default())
        .unwrap_err();
    match err.error() {
        zenpipe::PipeError::Codec(boxed) => {
            let eng = boxed
                .downcast_ref::<EngineError>()
                .expect("PipeError::Codec must preserve typed EngineError");
            assert_eq!(eng.failure(), &Failure::TargetUnmet);
        }
        other => panic!("expected PipeError::Codec(EngineError(TargetUnmet)), got: {other:?}"),
    }

    // 2. Converging Coding::Verify emits rich IQA provenance fields in OutputV2 JSON!
    let ok_job = parse_pipeline_json(
        br#"{
            "steps": [
                {"canvas": {"width": 32, "height": 32, "fill": [120, 160, 200, 255]}},
                {"encode": {"output": "out", "format": "jpeg", "coding": {"verify": {"metric": "zensim_a", "value": 40.0}}}}
            ]
        }"#,
        WireLimitConfig::default(),
    )
    .unwrap();
    let ok_res = rt
        .prepare(&ok_job, Bindings::new())
        .unwrap()
        .run_with_pulse(SharedPulse::default())
        .unwrap();
    let out_v2 = ok_res.output_v2();
    assert_eq!(
        out_v2.outputs[0].verified_metric.as_deref(),
        Some("zensim_a")
    );
    assert_eq!(
        out_v2.outputs[0].verified_revision.as_deref(),
        Some("const-low.r1")
    );
    assert_eq!(out_v2.outputs[0].verified_target, Some(40.0));
    assert_eq!(out_v2.outputs[0].verified_score_f64, Some(42.0));
    assert!(out_v2.outputs[0].verified_quality.is_some());
    assert!(out_v2.outputs[0].verify_attempts.unwrap_or(0) >= 1);

    // 3. PreparedJob::run_with_pulse stops immediately with typed PipeError::Stopped(TimedOut).
    struct TimedOutStop;
    impl Stop for TimedOutStop {
        fn check(&self) -> Result<(), StopReason> {
            Err(StopReason::TimedOut)
        }
    }
    let pulse = SharedPulse::new(zencodec::how_far::StopOnly::new(TimedOutStop));
    let cancelled = rt
        .prepare(&job, Bindings::new())
        .unwrap()
        .run_with_pulse(pulse)
        .unwrap_err();
    assert!(
        matches!(
            cancelled.error(),
            zenpipe::PipeError::Stopped(StopReason::TimedOut)
        ),
        "expected PipeError::Stopped(TimedOut), got: {cancelled:?}"
    );

    // 4. Mid-Verify cancellation witness: stop flag trips DURING trial 1's scoring so trial 1
    //    executes and trial 2 aborts cooperatively with typed PipeError::Stopped(TimedOut).
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct TripAfterFirstScore {
        calls: Arc<AtomicUsize>,
        tripped: Arc<AtomicBool>,
    }
    impl MetricScorer for TripAfterFirstScore {
        fn metric(&self) -> Metric {
            Metric::ZensimA
        }
        fn revision(&self) -> &'static str {
            "trip-after-one.r1"
        }
        fn comparison_format(&self) -> PixelFormat {
            PixelFormat::RGBA8_SRGB
        }
        fn score(
            &self,
            _reference: PixelView<'_>,
            _candidate: PixelView<'_>,
            _resources: &JobResources,
        ) -> EngineResult<f32> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.tripped.store(true, Ordering::SeqCst);
            Ok(10.0)
        }
    }
    struct FlagStop(Arc<AtomicBool>);
    impl Stop for FlagStop {
        fn check(&self) -> Result<(), StopReason> {
            if self.0.load(Ordering::SeqCst) {
                Err(StopReason::TimedOut)
            } else {
                Ok(())
            }
        }
    }
    let trip_calls = Arc::new(AtomicUsize::new(0));
    let trip_flag = Arc::new(AtomicBool::new(false));
    let trip_engine = Arc::new(
        Engine::new(bundles::register_all_enabled(CodecSet::new()).unwrap())
            .unwrap()
            .with_color_services(bundles::moxcms_color_services())
            .with_metric_scorer(
                Arc::new(TripAfterFirstScore {
                    calls: Arc::clone(&trip_calls),
                    tripped: Arc::clone(&trip_flag),
                }),
                NonZeroU32::new(4).unwrap(),
            )
            .unwrap(),
    );
    let trip_rt = PipelineRuntime::new(trip_engine, HostServices::new());
    let trip_pulse = SharedPulse::new(zencodec::how_far::StopOnly::new(FlagStop(Arc::clone(
        &trip_flag,
    ))));
    let mid_verify_err = trip_rt
        .prepare(&job, Bindings::new())
        .unwrap()
        .run_with_pulse(trip_pulse)
        .unwrap_err();
    assert_eq!(
        trip_calls.load(Ordering::SeqCst),
        1,
        "trial 1 must have executed before mid-Verify cancellation tripped"
    );
    assert!(
        matches!(
            mid_verify_err.error(),
            zenpipe::PipeError::Stopped(StopReason::TimedOut)
        ),
        "expected PipeError::Stopped(TimedOut) mid-Verify, got: {mid_verify_err:?}"
    );

    // 5a. Phase-isolated mid-decode cancellation witness: wrap real `zenjpeg::JpegDecoderConfig`
    //     so all `probe()` calls pass untouched, then `DynDecoder::decode()` records
    //     `entered_decode = true` and flips the stop flag so cancellation fires strictly INSIDE
    //     real `zenjpeg::decode()` via the pulse forwarded by `PipelineRuntime`!
    let seed_job = parse_pipeline_json(
        br#"{
            "steps": [
                {"canvas": {"width": 256, "height": 256, "fill": [80, 140, 210, 255]}},
                {"encode": {"output": "src", "format": "jpeg", "coding": {"calibrated_quality": 85.0}}}
            ]
        }"#,
        WireLimitConfig::default(),
    )
    .unwrap();
    let jpeg_bytes = rt
        .prepare(&seed_job, Bindings::new())
        .unwrap()
        .run()
        .unwrap()
        .outputs
        .remove(0)
        .bytes;

    struct CountdownStop {
        allow_checks: usize,
        checks: Arc<AtomicUsize>,
    }
    impl Stop for CountdownStop {
        fn check(&self) -> Result<(), StopReason> {
            let n = self.checks.fetch_add(1, Ordering::SeqCst);
            if n >= self.allow_checks {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }

    struct DecodeCancelFlagStop(Arc<AtomicBool>);
    impl Stop for DecodeCancelFlagStop {
        fn check(&self) -> Result<(), StopReason> {
            if self.0.load(Ordering::SeqCst) {
                Err(StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }

    struct TripOnDecodeConfig {
        inner: Arc<dyn zencodec::decode::DynDecoderConfig>,
        probes: Arc<AtomicUsize>,
        entered_decode: Arc<AtomicBool>,
        backend_stopped: Arc<AtomicBool>,
        trip_flag: Arc<AtomicBool>,
    }
    struct TripOnDecodeJob<'a> {
        inner: Box<dyn zencodec::decode::DynDecodeJob<'a> + 'a>,
        probes: Arc<AtomicUsize>,
        entered_decode: Arc<AtomicBool>,
        backend_stopped: Arc<AtomicBool>,
        trip_flag: Arc<AtomicBool>,
    }
    struct TripOnDecodeExec<'a> {
        inner: Box<dyn zencodec::decode::DynDecoder + 'a>,
        entered_decode: Arc<AtomicBool>,
        backend_stopped: Arc<AtomicBool>,
        trip_flag: Arc<AtomicBool>,
    }
    impl<'a> zencodec::decode::DynDecoder for TripOnDecodeExec<'a> {
        fn decode(
            self: Box<Self>,
        ) -> Result<zencodec::decode::DecodeOutput, Box<dyn std::error::Error + Send + Sync>>
        {
            use zencodec::CodecErrorExt as _;
            self.entered_decode.store(true, Ordering::SeqCst);
            self.trip_flag.store(true, Ordering::SeqCst);
            let res = self.inner.decode();
            if let Err(ref err) = res
                && matches!(
                    err.error_category(),
                    Some(zencodec::ErrorCategory::Stopped(StopReason::Cancelled))
                )
            {
                self.backend_stopped.store(true, Ordering::SeqCst);
            }
            res
        }
    }
    impl<'a> zencodec::decode::DynDecodeJob<'a> for TripOnDecodeJob<'a> {
        fn set_pulse(&mut self, pulse: SharedPulse) {
            self.inner.set_pulse(pulse);
        }
        fn set_limits(&mut self, limits: zencodec::ResourceLimits) {
            self.inner.set_limits(limits);
        }
        fn probe(
            &self,
            data: &[u8],
        ) -> Result<zencodec::ImageInfo, Box<dyn std::error::Error + Send + Sync>> {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.inner.probe(data)
        }
        fn probe_full(
            &self,
            data: &[u8],
        ) -> Result<zencodec::ImageInfo, Box<dyn std::error::Error + Send + Sync>> {
            self.probes.fetch_add(1, Ordering::SeqCst);
            self.inner.probe_full(data)
        }
        fn set_crop_hint(&mut self, crop: zencodec::Rect) {
            self.inner.set_crop_hint(crop);
        }
        fn set_orientation(&mut self, hint: zencodec::OrientationHint) {
            self.inner.set_orientation(hint);
        }
        fn set_start_frame_index(&mut self, index: u32) {
            self.inner.set_start_frame_index(index);
        }
        fn extensions(&self) -> Option<&dyn std::any::Any> {
            self.inner.extensions()
        }
        fn extensions_mut(&mut self) -> Option<&mut dyn std::any::Any> {
            self.inner.extensions_mut()
        }
        fn output_info(
            &self,
            data: &[u8],
        ) -> Result<zencodec::decode::OutputInfo, Box<dyn std::error::Error + Send + Sync>>
        {
            self.inner.output_info(data)
        }
        fn into_decoder(
            self: Box<Self>,
            data: std::borrow::Cow<'a, [u8]>,
            preferred: &[PixelFormat],
        ) -> Result<
            Box<dyn zencodec::decode::DynDecoder + 'a>,
            Box<dyn std::error::Error + Send + Sync>,
        > {
            let dec = self.inner.into_decoder(data, preferred)?;
            Ok(Box::new(TripOnDecodeExec {
                inner: dec,
                entered_decode: self.entered_decode,
                backend_stopped: self.backend_stopped,
                trip_flag: self.trip_flag,
            }))
        }
        fn push_decode(
            self: Box<Self>,
            data: std::borrow::Cow<'a, [u8]>,
            sink: &mut dyn zencodec::decode::DecodeRowSink,
            preferred: &[PixelFormat],
        ) -> Result<zencodec::decode::OutputInfo, Box<dyn std::error::Error + Send + Sync>>
        {
            self.inner.push_decode(data, sink, preferred)
        }
        fn into_streaming_decoder(
            self: Box<Self>,
            data: std::borrow::Cow<'a, [u8]>,
            preferred: &[PixelFormat],
        ) -> Result<
            Box<dyn zencodec::decode::DynStreamingDecoder + 'a>,
            Box<dyn std::error::Error + Send + Sync>,
        > {
            self.inner.into_streaming_decoder(data, preferred)
        }
        fn into_animation_frame_decoder(
            self: Box<Self>,
            data: std::borrow::Cow<'a, [u8]>,
            preferred: &[PixelFormat],
        ) -> Result<
            Box<dyn zencodec::decode::DynAnimationFrameDecoder>,
            Box<dyn std::error::Error + Send + Sync>,
        > {
            self.inner.into_animation_frame_decoder(data, preferred)
        }
    }
    impl zencodec::decode::DynDecoderConfig for TripOnDecodeConfig {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn formats(&self) -> &'static [zencodec::ImageFormat] {
            self.inner.formats()
        }
        fn supported_pixel_formats(&self) -> &'static [PixelFormat] {
            self.inner.supported_pixel_formats()
        }
        fn capabilities(&self) -> &'static zencodec::decode::DecodeCapabilities {
            self.inner.capabilities()
        }
        fn dyn_job(&self) -> Box<dyn zencodec::decode::DynDecodeJob<'_> + '_> {
            Box::new(TripOnDecodeJob {
                inner: self.inner.dyn_job(),
                probes: Arc::clone(&self.probes),
                entered_decode: Arc::clone(&self.entered_decode),
                backend_stopped: Arc::clone(&self.backend_stopped),
                trip_flag: Arc::clone(&self.trip_flag),
            })
        }
    }

    let probes = Arc::new(AtomicUsize::new(0));
    let entered_decode = Arc::new(AtomicBool::new(false));
    let backend_stopped = Arc::new(AtomicBool::new(false));
    let dec_trip_flag = Arc::new(AtomicBool::new(false));
    let real_jpeg_dec = engine
        .codecs()
        .factories::<zencodec::ImageDecodeRole>(&zencodec::ImageFormat::Jpeg)
        .next()
        .expect("zenjpeg registered")
        .factory
        .clone();
    let trip_dec_id = zencodec::RegistrationId::new("org.test.jpeg.trip_decode").unwrap();
    let dec_set = bundles::register_all_enabled(CodecSet::new())
        .unwrap()
        .with_role::<zencodec::ImageDecodeRole>(
            vec![zencodec::ImageFormat::Jpeg],
            zencodec::NamedFactory::<zencodec::ImageDecodeRole>::new(
                trip_dec_id.clone(),
                "test-1",
                Arc::new(TripOnDecodeConfig {
                    inner: real_jpeg_dec,
                    probes: Arc::clone(&probes),
                    entered_decode: Arc::clone(&entered_decode),
                    backend_stopped: Arc::clone(&backend_stopped),
                    trip_flag: Arc::clone(&dec_trip_flag),
                }),
            ),
        )
        .unwrap();
    let dec_engine = Arc::new(
        Engine::new(dec_set)
            .unwrap()
            .with_color_services(bundles::moxcms_color_services())
            .with_routing(
                zencodecs::routing::Routing::unrestricted()
                    .requiring(
                        zencodecs::routing::RuleKey::ImageDecode(zencodec::ImageFormat::Jpeg),
                        trip_dec_id,
                    )
                    .unwrap(),
            ),
    );
    let dec_rt = PipelineRuntime::new(
        dec_engine,
        HostServices::new().with_workspace_policy(zencodecs::engine::WorkspacePolicy::BestEffort),
    );
    let dec_only_job = parse_pipeline_json(
        br#"{
            "steps": [
                {"decode": {"input": "in", "orientation": "apply"}},
                {"encode": {"output": "out", "format": "png"}}
            ]
        }"#,
        WireLimitConfig::default(),
    )
    .unwrap();
    let dec_pulse = SharedPulse::new(zencodec::how_far::StopOnly::new(DecodeCancelFlagStop(
        Arc::clone(&dec_trip_flag),
    )));
    let mid_dec_err = dec_rt
        .prepare(
            &dec_only_job,
            Bindings::new().with_bytes("in", &jpeg_bytes).unwrap(),
        )
        .unwrap()
        .run_with_pulse(dec_pulse)
        .unwrap_err();
    assert!(
        probes.load(Ordering::SeqCst) >= 1,
        "probe phase must complete cleanly before decode"
    );
    assert!(
        entered_decode.load(Ordering::SeqCst),
        "execution must enter DynDecoder::decode() before cancellation trips"
    );
    assert!(
        backend_stopped.load(Ordering::SeqCst),
        "real zenjpeg::decode() itself must observe the forwarded pulse and return Stopped(Cancelled)"
    );
    assert!(
        matches!(
            mid_dec_err.error(),
            zenpipe::PipeError::Stopped(StopReason::Cancelled)
        ),
        "expected PipeError::Stopped(Cancelled) from real zenjpeg::decode() via forwarded pulse, got: {mid_dec_err:?}"
    );

    // 5b. Phase-isolated mid-conversion cancellation witness: ZERO decode/probe — canvas ->
    //     color_space (256x256) -> encode cancels strictly inside `convert_pixel_buf` row loop!
    let conv_job = parse_pipeline_json(
        br#"{
            "steps": [
                {"canvas": {"width": 256, "height": 256, "fill": [80, 140, 210, 255]}},
                {"color_space": {"to": "rgb8_srgb"}},
                {"encode": {"output": "out", "format": "png"}}
            ]
        }"#,
        WireLimitConfig::default(),
    )
    .unwrap();
    let conv_checks = Arc::new(AtomicUsize::new(0));
    let conv_pulse = SharedPulse::new(zencodec::how_far::StopOnly::new(CountdownStop {
        // Canvas step passes, convert_pixel_buf entry + row 0 + row 64 pass, row 128 cancels!
        allow_checks: 5,
        checks: Arc::clone(&conv_checks),
    }));
    let mid_conv_err = rt
        .prepare(&conv_job, Bindings::new())
        .unwrap()
        .run_with_pulse(conv_pulse)
        .unwrap_err();
    assert!(
        conv_checks.load(Ordering::SeqCst) >= 6,
        "conversion stop must trip inside convert_pixel_buf row loop"
    );
    assert!(
        matches!(
            mid_conv_err.error(),
            zenpipe::PipeError::Stopped(StopReason::Cancelled)
        ),
        "expected PipeError::Stopped(Cancelled) mid-conversion, got: {mid_conv_err:?}"
    );

    // 5c. Phase-isolated mid-orientation cancellation witness in canonical runtime:
    let orient_job = parse_pipeline_json(
        br#"{
            "steps": [
                {"canvas": {"width": 256, "height": 256, "fill": [80, 140, 210, 255]}},
                {"geometry": {"orient": "rotate90"}},
                {"encode": {"output": "out", "format": "png"}}
            ]
        }"#,
        WireLimitConfig::default(),
    )
    .unwrap();
    let orient_checks = Arc::new(AtomicUsize::new(0));
    let orient_pulse = SharedPulse::new(zencodec::how_far::StopOnly::new(CountdownStop {
        allow_checks: 5,
        checks: Arc::clone(&orient_checks),
    }));
    let mid_orient_err = rt
        .prepare(&orient_job, Bindings::new())
        .unwrap()
        .run_with_pulse(orient_pulse)
        .unwrap_err();
    assert!(
        orient_checks.load(Ordering::SeqCst) >= 6,
        "orient stop must trip inside apply_orient row loop"
    );
    assert!(
        matches!(
            mid_orient_err.error(),
            zenpipe::PipeError::Stopped(StopReason::Cancelled)
        ),
        "expected PipeError::Stopped(Cancelled) mid-orientation, got: {mid_orient_err:?}"
    );

    // 6. Coding::AimFor exposes aim_metric, aim_target, aim_revision, aim_estimated_quality
    //    and leaves verified_score absent in OutputV2 JSON.
    let aim_job = parse_pipeline_json(
        br#"{
            "steps": [
                {"canvas": {"width": 32, "height": 32, "fill": [100, 120, 140, 255]}},
                {"encode": {"output": "out", "format": "jpeg", "coding": {"aim_for": {"metric": "zensim_a", "value": 85.0}}}}
            ]
        }"#,
        WireLimitConfig::default(),
    )
    .unwrap();
    let aim_res = rt
        .prepare(&aim_job, Bindings::new())
        .unwrap()
        .run_with_pulse(SharedPulse::default())
        .unwrap();
    let aim_v2 = aim_res.output_v2();
    assert_eq!(aim_v2.outputs[0].aim_metric.as_deref(), Some("zensim_a"));
    assert_eq!(aim_v2.outputs[0].aim_target, Some(85.0));
    assert!(aim_v2.outputs[0].aim_revision.is_some());
    assert!(aim_v2.outputs[0].aim_estimated_quality.is_some());
    assert_eq!(aim_v2.outputs[0].verified_score, None);
    assert_eq!(aim_v2.outputs[0].verified_score_f64, None);
}
