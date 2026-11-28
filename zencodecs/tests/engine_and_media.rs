//! End-to-end qualification of PR 4 (`zencodecs::Engine`, `Routing`, `ColorServices`,
//! `Verify(MetricTarget)`, `Preserve` vs `PreserveBytes`, `Strict` vs `BestEffort`,
//! and first-class AV1/VP8/VP9 × MP4/WebM/IVF sans-I/O `MediaPump` with independent
//! `ffmpeg`/`ffprobe` oracles).

#![cfg(all(
    feature = "std",
    feature = "jpeg",
    feature = "png",
    feature = "webp",
    feature = "cms",
    feature = "all-media"
))]

use std::num::NonZeroU32;
use std::process::Command;
use std::sync::Arc;

use zencodec::time::{FrameDuration, TimeBase, Timestamp};
use zencodec::{CodecSet, ImageFormat, JobResources, Metadata, RegistrationId, ResourceLimits};
use zencodec_media::registry::ContainerId;
use zencodec_media::track::CodecId as MediaCodecId;
use zencodecs::bundles::{self, ids};
use zencodecs::engine::{
    Coding, Engine, Failure, LoopCount, LoopPolicy, MetadataPolicy, Metric, MetricScorer,
    MetricTarget, OrientationMode, OutputIntent, PublicationMode, RouteKind, Run, SequencePolicy,
    WorkspacePolicy,
};
use zencodecs::media::pump::{
    MediaPump, drive_pump_in_memory, encode_frames_to_video, extract_poster_from_bytes,
    transcode_media_bytes,
};
use zencodecs::media::select::{DecodeSelection, SeekMode};
use zencodecs::routing::{Routing, RuleKey};
use zenpixels::{Orientation, PixelBuf, PixelFormat, PixelView};
use zenpixels_convert::ColorServices;

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

/// Deterministic score-only scalar PSNR-mapped metric scorer for `Coding::Verify` testing.
struct SimplePsnrScorer;

impl MetricScorer for SimplePsnrScorer {
    fn metric(&self) -> Metric {
        Metric::Ssimulacra2
    }

    fn revision(&self) -> &'static str {
        "test-psnr-v1"
    }

    fn comparison_format(&self) -> PixelFormat {
        PixelFormat::RGB8_SRGB
    }

    fn score(
        &self,
        reference: PixelView<'_>,
        candidate: PixelView<'_>,
        _resources: &JobResources,
    ) -> zencodecs::EngineResult<f32> {
        assert_eq!(reference.width(), candidate.width());
        assert_eq!(reference.height(), candidate.height());
        let mut sq_err = 0.0f64;
        let mut count = 0u64;
        for y in 0..reference.height() {
            let r = reference.row(y);
            let c = candidate.row(y);
            for (&a, &b) in r.iter().zip(c.iter()) {
                let d = f64::from(a) - f64::from(b);
                sq_err += d * d;
                count += 1;
            }
        }
        let mse = sq_err / (count.max(1) as f64);
        if mse <= 1e-9 {
            return Ok(100.0);
        }
        let psnr = 10.0 * (255.0f64 * 255.0 / mse).log10();
        // Map PSNR [20..50] -> [30..99] monotonically.
        Ok(((psnr - 20.0) * 2.3 + 30.0).clamp(0.0, 99.5) as f32)
    }
}

#[test]
fn engine_rejects_prestamped_defaults_enforces_strict_bounds_preserve_bytes_and_verify() {
    // 1. Engine::new must refuse a CodecSet with pre-stamped operation defaults.
    let bad_set = CodecSet::new().with_limits(ResourceLimits::for_untrusted_input());
    assert!(Engine::new(bad_set).is_err());

    // 2. Register two distinct JPEG encoders (zenjpeg + second named alias) to prove Routing swap.
    let mut set = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let alt_jpeg_id = RegistrationId::new("org.imazen.jpeg.alt-test").unwrap();
    set = set
        .with_named_send_encoder(
            alt_jpeg_id.clone(),
            "alt-test-1",
            zenjpeg::JpegEncoderConfig::new(),
        )
        .unwrap();

    let engine = Engine::new(set)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services())
        .with_metric_scorer(Arc::new(SimplePsnrScorer), NonZeroU32::new(6).unwrap())
        .unwrap();

    let img = make_gradient_rgb8(64, 64, 17);

    // 3. Decision 4: qualified pure-Rust zen codecs report real bounds and succeed under
    //    default `WorkspacePolicy::Strict`, while any third-party/unknown-bound codec is
    //    refused under `Strict` and admitted+recorded only under explicit `BestEffort`.
    let strict_run = Run::new().with_trace(NonZeroU32::new(64).unwrap());
    let encoded_default = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::CalibratedQuality(85.0)),
            img.as_view(),
            &Metadata::none(),
            None,
            &strict_run,
        )
        .unwrap();
    assert_eq!(
        encoded_default.report.encoder.as_ref().map(|r| r.as_str()),
        Some(ids::ZENJPEG)
    );
    assert!(
        encoded_default
            .report
            .unknown_workspace_components
            .is_empty(),
        "qualified zenjpeg must have zero unknown workspace components under Strict"
    );

    // Explicit BestEffort run remains supported for hosts opting into unknown-bound codecs.
    let best_effort_run = Run::new()
        .with_workspace_policy(WorkspacePolicy::BestEffort)
        .with_trace(NonZeroU32::new(64).unwrap());

    // 4. Routing swap to alt_jpeg_id using the EXACT SAME OutputIntent (zero node/request change).
    let swap_routing = Routing::unrestricted()
        .requiring(RuleKey::ImageEncode(ImageFormat::Jpeg), alt_jpeg_id.clone())
        .unwrap();
    let swap_run = Run::new()
        .with_routing(&swap_routing)
        .with_workspace_policy(WorkspacePolicy::BestEffort);
    let encoded_swapped = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::CalibratedQuality(85.0)),
            img.as_view(),
            &Metadata::none(),
            None,
            &swap_run,
        )
        .unwrap();
    assert_eq!(
        encoded_swapped.report.encoder.as_ref(),
        Some(&alt_jpeg_id),
        "Routing::requiring must swap JPEG provider without changing OutputIntent"
    );

    // 5. Decision 2: MetadataPolicy::PreserveBytes refuses EXIF orientation rewrite when baking
    //    non-identity orientation, whereas MetadataPolicy::Preserve actually bakes D4 pixels
    //    (64x48 -> 48x64) and reconciles metadata orientation to Identity!
    let non_square = make_gradient_rgb8(64, 48, 33);
    let mut rotated_meta = Metadata::none();
    rotated_meta.orientation = Orientation::Rotate90;
    let err_preserve_bytes = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg)
                .with_orientation(OrientationMode::Bake)
                .with_metadata(MetadataPolicy::PreserveBytes),
            non_square.as_view(),
            &rotated_meta,
            None,
            &best_effort_run,
        )
        .unwrap_err();
    assert_eq!(
        err_preserve_bytes.error().failure(),
        &Failure::UnsupportedEffect
    );
    let ok_preserve = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg)
                .with_orientation(OrientationMode::Bake)
                .with_metadata(MetadataPolicy::Preserve),
            non_square.as_view(),
            &rotated_meta,
            None,
            &best_effort_run,
        )
        .unwrap();
    let decoded_baked = engine
        .decode(
            &zencodecs::DecodeNeeds::still(),
            ok_preserve.output.as_bytes(),
            &best_effort_run,
        )
        .unwrap();
    assert_eq!(
        (decoded_baked.output.width(), decoded_baked.output.height()),
        (48, 64),
        "OrientationMode::Bake + Rotate90 must physically rotate 64x48 -> 48x64"
    );
    assert_eq!(
        decoded_baked.output.metadata().orientation,
        Orientation::Identity
    );

    // 5b. Cross-engine DecodeOffer & Strict decode refusal regressions.
    let other_set = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let other_engine = Engine::new(other_set).unwrap();
    let probed_other = other_engine
        .probe(ok_preserve.output.as_bytes(), &best_effort_run)
        .unwrap();
    let offers_other = other_engine
        .decode_offers(
            &probed_other,
            &zencodecs::DecodeNeeds::still(),
            &best_effort_run,
        )
        .unwrap();
    let probed_here = engine
        .probe(ok_preserve.output.as_bytes(), &best_effort_run)
        .unwrap();
    let cross_err = engine
        .prepare_decode(
            offers_other[0].clone(),
            probed_here,
            zencodecs::DecodeNeeds::still(),
            &best_effort_run,
        )
        .err()
        .expect("offer from another Engine must be refused");
    assert_eq!(
        cross_err.into_error().error().failure(),
        &Failure::InvalidRequest
    );

    // Qualified pure-Rust still decode succeeds under default Strict workspace policy with
    // zero unknown workspace components.
    let strict_decode_ok = engine
        .decode(
            &zencodecs::DecodeNeeds::still(),
            ok_preserve.output.as_bytes(),
            &Run::new(),
        )
        .unwrap();
    assert!(
        strict_decode_ok
            .report
            .unknown_workspace_components
            .is_empty()
    );

    // An unknown-workspace third-party/unqualified decoder MUST still be refused under
    // Strict policy and admitted+recorded only under explicit BestEffort.
    #[derive(Clone, Debug)]
    struct UnknownBoundJpegDecoder(zenjpeg::JpegDecoderConfig);
    impl zencodec::decode::DecoderConfig for UnknownBoundJpegDecoder {
        type Error = <zenjpeg::JpegDecoderConfig as zencodec::decode::DecoderConfig>::Error;
        type Job<'a> = <zenjpeg::JpegDecoderConfig as zencodec::decode::DecoderConfig>::Job<'a>;
        fn formats() -> &'static [ImageFormat] {
            <zenjpeg::JpegDecoderConfig as zencodec::decode::DecoderConfig>::formats()
        }
        fn supported_pixel_formats() -> &'static [zenpixels::PixelFormat] {
            <zenjpeg::JpegDecoderConfig as zencodec::decode::DecoderConfig>::supported_pixel_formats(
            )
        }
        fn capabilities() -> &'static zencodec::decode::DecodeCapabilities {
            <zenjpeg::JpegDecoderConfig as zencodec::decode::DecoderConfig>::capabilities()
        }
        fn job<'a>(self) -> Self::Job<'a> {
            self.0.job()
        }
    }
    let unknown_id = RegistrationId::new("org.example.jpeg.unknown-bound").unwrap();
    let unknown_set = CodecSet::new()
        .with_named_decoder(
            unknown_id.clone(),
            "unknown-1",
            UnknownBoundJpegDecoder(zenjpeg::JpegDecoderConfig::new()),
        )
        .unwrap();
    let unknown_engine = Engine::new(unknown_set).unwrap();
    let strict_unknown_err = unknown_engine
        .decode(
            &zencodecs::DecodeNeeds::still(),
            ok_preserve.output.as_bytes(),
            &Run::new(),
        )
        .unwrap_err();
    assert_eq!(
        strict_unknown_err.error().failure(),
        &Failure::PolicyRefusal
    );
    let best_effort_unknown = unknown_engine
        .decode(
            &zencodecs::DecodeNeeds::still(),
            ok_preserve.output.as_bytes(),
            &best_effort_run,
        )
        .unwrap();
    assert_eq!(
        best_effort_unknown.report.unknown_workspace_components,
        vec![unknown_id.as_str().to_string()]
    );

    // 6. Decision 5: Coding::Verify(MetricTarget) score-only closed-loop still SDR convergence.
    let target = MetricTarget::new(Metric::Ssimulacra2, 75.0)
        .unwrap()
        .with_revision("test-psnr-v1");
    let verified = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Jpeg).with_coding(Coding::Verify(target)),
            img.as_view(),
            &Metadata::none(),
            None,
            &best_effort_run,
        )
        .unwrap();
    assert!(verified.report.verified_score.unwrap() >= 75.0);
    assert!(verified.report.verify_attempts >= 1 && verified.report.verify_attempts <= 6);

    // 7. StrictNoIcc ColorServices still permits parametric RGB8 -> RGBA8 conversion.
    let strict_no_icc_run = Run::new()
        .with_color_services(ColorServices::strict_no_icc())
        .with_workspace_policy(WorkspacePolicy::BestEffort);
    let png_out = engine
        .encode_pixels(
            &OutputIntent::format(ImageFormat::Png)
                .with_color(zencodecs::ColorTarget::ConvertTo(PixelFormat::RGBA8_SRGB)),
            img.as_view(),
            &Metadata::none(),
            None,
            &strict_no_icc_run,
        )
        .unwrap();
    assert_eq!(png_out.report.output_format, Some(ImageFormat::Png));
    assert_eq!(png_out.report.route, Some(RouteKind::Raster));
}

fn ffprobe_video_codec_and_frames(bytes: &[u8], ext: &str) -> (String, u32) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!(
        "zencodecs_probe_{}_{}.{}",
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
            "stream=codec_name,nb_read_packets",
            "-of",
            "csv=p=0",
        ])
        .arg(&path)
        .output()
        .expect("ffprobe must be available for independent oracle check");
    let _ = std::fs::remove_file(&path);
    assert!(
        out.status.success(),
        "ffprobe rejected container ({ext}): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut parts = line.split(',');
    let codec = parts.next().unwrap_or("").to_string();
    let count: u32 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    (codec, count)
}

#[test]
fn first_class_av1_vp8_vp9_mp4_webm_ivf_streaming_poster_and_ffprobe_oracle() {
    let set = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let engine = Engine::new(set)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services());

    // Build 3 distinct 64x64 frames with exact 40ms durations (25 fps).
    let f0 = make_gradient_rgb8(64, 64, 0);
    let f1 = make_gradient_rgb8(64, 64, 64);
    let f2 = make_gradient_rgb8(64, 64, 128);
    let dur = FrameDuration::from_millis(40);
    let frames = vec![
        (f0.as_view(), dur),
        (f1.as_view(), dur),
        (f2.as_view(), dur),
    ];

    let run = Run::new()
        .with_workspace_policy(WorkspacePolicy::BestEffort)
        .with_trace(NonZeroU32::new(64).unwrap());

    // 1. Repeating animation -> MP4 must REFUSE under LoopPolicy::Preserve!
    let loop_refuse_intent = OutputIntent::video(ContainerId::Mp4, MediaCodecId::Av1);
    let loop_err = encode_frames_to_video(
        &engine,
        &frames,
        LoopCount::Infinite,
        &loop_refuse_intent,
        &run,
    )
    .unwrap_err();
    assert_eq!(loop_err.error().failure(), &Failure::UnsupportedEffect);

    let exact_one_pass = SequencePolicy::Preserve {
        timing: zencodecs::TimingPolicy::Exact,
        loops: LoopPolicy::Set(LoopCount::once()),
    };
    let quantized_one_pass = SequencePolicy::Preserve {
        timing: zencodecs::TimingPolicy::QuantizeEndpoints {
            max_boundary_error: FrameDuration::from_millis(1),
        },
        loops: LoopPolicy::Set(LoopCount::once()),
    };

    // 2. Encode AV1 in MP4 (exact timing), verify WebM honestly refuses ExactTiming,
    //    then encode VP8/VP9 in WebM (quantized timing) and VP9 in MP4 (exact timing),
    //    verifying every single output container with the independent ffprobe oracle!
    let av1_mp4 = encode_frames_to_video(
        &engine,
        &frames,
        LoopCount::Infinite,
        &OutputIntent::video(ContainerId::Mp4, MediaCodecId::Av1)
            .with_sequence(exact_one_pass)
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap();
    let (c, n) = ffprobe_video_codec_and_frames(av1_mp4.output.as_bytes(), "mp4");
    assert_eq!(c, "av1");
    assert_eq!(n, 3);

    // WebM does not preserve arbitrary exact DTS/duration timing without endpoint quantization;
    // TimingPolicy::Exact must refuse rather than silently quantize!
    let webm_exact_err = encode_frames_to_video(
        &engine,
        &frames,
        LoopCount::once(),
        &OutputIntent::video(ContainerId::Webm, MediaCodecId::Vp8)
            .with_sequence(exact_one_pass)
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap_err();
    assert_eq!(
        webm_exact_err.error().failure(),
        &Failure::UnsupportedEffect
    );

    let vp8_webm = encode_frames_to_video(
        &engine,
        &frames,
        LoopCount::once(),
        &OutputIntent::video(ContainerId::Webm, MediaCodecId::Vp8)
            .with_sequence(quantized_one_pass)
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap();
    let (c, n) = ffprobe_video_codec_and_frames(vp8_webm.output.as_bytes(), "webm");
    assert_eq!(c, "vp8");
    assert_eq!(n, 3);

    let vp9_webm = encode_frames_to_video(
        &engine,
        &frames,
        LoopCount::once(),
        &OutputIntent::video(ContainerId::Webm, MediaCodecId::Vp9)
            .with_sequence(quantized_one_pass)
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap();
    let (c, n) = ffprobe_video_codec_and_frames(vp9_webm.output.as_bytes(), "webm");
    assert_eq!(c, "vp9");
    assert_eq!(n, 3);

    let vp9_mp4 = encode_frames_to_video(
        &engine,
        &frames,
        LoopCount::once(),
        &OutputIntent::video(ContainerId::Mp4, MediaCodecId::Vp9)
            .with_sequence(exact_one_pass)
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap();
    let (c, n) = ffprobe_video_codec_and_frames(vp9_mp4.output.as_bytes(), "mp4");
    assert_eq!(c, "vp9");
    assert_eq!(n, 3);

    // 3. Poster extraction by PresentationIndex AND PresentationTime from AV1 MP4, VP8 WebM, and VP9 WebM.
    let poster_av1 = extract_poster_from_bytes(
        &engine,
        av1_mp4.output.as_bytes(),
        DecodeSelection::PresentationIndex {
            track: None,
            index: 1,
        },
        PixelFormat::RGB8_SRGB,
        &run,
    )
    .unwrap();
    assert_eq!(poster_av1.output.0.width(), 64);
    assert_eq!(poster_av1.output.0.height(), 64);
    assert_eq!(poster_av1.output.2, 1);

    let poster_vp8_time = extract_poster_from_bytes(
        &engine,
        vp8_webm.output.as_bytes(),
        DecodeSelection::PresentationTime {
            track: None,
            time: Timestamp::new(80, TimeBase::new(1, 1000).unwrap()),
            mode: SeekMode::AtOrBefore,
        },
        PixelFormat::RGB8_SRGB,
        &run,
    )
    .unwrap();
    assert_eq!(poster_vp8_time.output.2, 2);

    // 4. Cross-container + cross-codec streaming transcode via MediaPump with tiny 17-byte chunks:
    //    VP8 WebM -> VP9 IVF (Incremental live streaming mode!) and AV1 MP4 -> VP8 WebM (Verified)!
    let access = zencodec::source::SourceAccess::borrowed(vp8_webm.output.as_bytes().len() as u64);
    let mut pump = MediaPump::prepare_transcode(
        engine.clone(),
        ContainerId::Webm,
        &access,
        OutputIntent::video(ContainerId::Ivf, MediaCodecId::Vp9)
            .with_sequence(quantized_one_pass)
            .with_publication(PublicationMode::Incremental),
        &run,
    )
    .unwrap();
    let published_ivf = drive_pump_in_memory(
        &mut pump,
        vp8_webm.output.as_bytes(),
        17, // Tiny 17-byte chunk streaming stress test!
    )
    .unwrap();
    let (c, n) = ffprobe_video_codec_and_frames(&published_ivf.bytes, "ivf");
    assert_eq!(c, "vp9");
    assert_eq!(n, 3);

    // 4b. Exact fractional 1/30 s frames encode cleanly to MP4 under TimingPolicy::Exact!
    let dur30 = FrameDuration::new(1, 30).unwrap();
    let frames_30fps = vec![
        (f0.as_view(), dur30),
        (f1.as_view(), dur30),
        (f2.as_view(), dur30),
    ];
    let exact_30fps_mp4 = encode_frames_to_video(
        &engine,
        &frames_30fps,
        LoopCount::once(),
        &OutputIntent::video(ContainerId::Mp4, MediaCodecId::Vp9)
            .with_sequence(exact_one_pass)
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap();
    let (c, n) = ffprobe_video_codec_and_frames(exact_30fps_mp4.output.as_bytes(), "mp4");
    assert_eq!(c, "vp9");
    assert_eq!(n, 3);

    // 4c. Verified MediaPump MUST emit ZERO NeedWriter bytes until verified_receipt() is populated!
    let av1_bytes = av1_mp4.output.as_bytes();
    let av1_access = zencodec::source::SourceAccess::borrowed(av1_bytes.len() as u64);
    let mut verified_pump = MediaPump::prepare_transcode(
        engine.clone(),
        ContainerId::Mp4,
        &av1_access,
        OutputIntent::video(ContainerId::Webm, MediaCodecId::Vp8)
            .with_sequence(quantized_one_pass)
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap();
    let mut staged_out = Vec::new();
    loop {
        match verified_pump.advance().unwrap() {
            zencodecs::media::pump::PumpEvent::NeedInput { ticket, requests } => {
                let req = requests[0];
                let off = req.offset().unwrap_or(0) as usize;
                let slice = if off < av1_bytes.len() {
                    &av1_bytes[off..]
                } else {
                    &[]
                };
                verified_pump
                    .supply_input(
                        ticket,
                        zencodec::decode::Input::new(off as u64, slice)
                            .with_total_len(av1_bytes.len() as u64),
                    )
                    .unwrap();
            }
            zencodecs::media::pump::PumpEvent::NeedWriter { ticket, write } => {
                let len = match write {
                    zencodecs::media::pump::WriteOffer::Append(chunk) => {
                        staged_out.extend_from_slice(chunk);
                        chunk.len()
                    }
                    zencodecs::media::pump::WriteOffer::Patch { .. } => {
                        panic!("Verified MediaPump must apply patches privately before emitting")
                    }
                };
                assert!(
                    verified_pump.verified_receipt().is_some(),
                    "Verified MediaPump must NEVER emit NeedWriter bytes before readback verification completes"
                );
                verified_pump.confirm_written(ticket, len).unwrap();
            }
            zencodecs::media::pump::PumpEvent::Presented => {}
            zencodecs::media::pump::PumpEvent::Finished => break,
        }
    }
    let (c, n) = ffprobe_video_codec_and_frames(&staged_out, "webm");
    assert_eq!(c, "vp8");
    assert_eq!(n, 3);

    let av1_to_vp8 = transcode_media_bytes(
        &engine,
        av1_mp4.output.as_bytes(),
        &OutputIntent::video(ContainerId::Webm, MediaCodecId::Vp8)
            .with_sequence(quantized_one_pass)
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap();
    let (c, n) = ffprobe_video_codec_and_frames(av1_to_vp8.output.as_bytes(), "webm");
    assert_eq!(c, "vp8");
    assert_eq!(n, 3);
}

#[test]
fn media_pump_rejects_stale_wrong_direction_tickets_and_fuses_on_fault() {
    use zencodec::decode::Input;
    use zencodecs::media::pump::{IoTicket, PumpEvent};

    let set = bundles::register_all_enabled(CodecSet::new()).unwrap();
    let engine = Engine::new(set)
        .unwrap()
        .with_color_services(bundles::moxcms_color_services());
    let run = Run::new().with_workspace_policy(WorkspacePolicy::BestEffort);
    let one_pass_seq = SequencePolicy::Preserve {
        timing: zencodecs::TimingPolicy::Exact,
        loops: LoopPolicy::Set(LoopCount::once()),
    };
    let intent = OutputIntent::video(ContainerId::Ivf, MediaCodecId::Vp9)
        .with_sequence(one_pass_seq)
        .with_publication(PublicationMode::Incremental);

    // 1. Stale ticket on supply_input refuses and fuses the pump.
    let access = zencodec::source::SourceAccess::forward_stream();
    let mut pump = MediaPump::prepare_transcode(
        engine.clone(),
        ContainerId::Webm,
        &access,
        intent.clone(),
        &run,
    )
    .unwrap();
    let ticket = match pump.advance().unwrap() {
        PumpEvent::NeedInput { ticket, .. } => ticket,
        other => panic!("expected NeedInput, got {other:?}"),
    };
    let wrong_ticket = IoTicket::from_raw(ticket.raw() + 99);
    let err = pump
        .supply_input(wrong_ticket, Input::new(0, b"bad"))
        .unwrap_err();
    assert_eq!(err.error().failure(), &Failure::InvalidRequest);
    // Pump is now fused; even the original ticket or advance must refuse immediately.
    assert!(pump.supply_input(ticket, Input::new(0, b"bad")).is_err());
    assert!(pump.advance().is_err());

    // 2. Wrong-direction ticket (passing NeedInput ticket to confirm_written) refuses and fuses.
    let mut pump2 = MediaPump::prepare_transcode(
        engine.clone(),
        ContainerId::Webm,
        &access,
        intent.clone(),
        &run,
    )
    .unwrap();
    let ticket2 = match pump2.advance().unwrap() {
        PumpEvent::NeedInput { ticket, .. } => ticket,
        other => panic!("expected NeedInput, got {other:?}"),
    };
    let err2 = pump2.confirm_written(ticket2, 0).unwrap_err();
    assert_eq!(err2.error().failure(), &Failure::InvalidRequest);
    assert!(pump2.advance().is_err());

    // 3. Advancing while a ticket is unresolved refuses and fuses.
    let mut pump3 = MediaPump::prepare_transcode(
        engine.clone(),
        ContainerId::Webm,
        &access,
        intent.clone(),
        &run,
    )
    .unwrap();
    assert!(matches!(
        pump3.advance().unwrap(),
        PumpEvent::NeedInput { .. }
    ));
    let err3 = pump3.advance().unwrap_err();
    assert_eq!(err3.error().failure(), &Failure::InvalidRequest);
    assert!(pump3.advance().is_err());

    // 4. Wrong offset and conflicting total_len on supply_input must refuse and fuse!
    let mut pump3b = MediaPump::prepare_transcode(
        engine.clone(),
        ContainerId::Webm,
        &zencodec::source::SourceAccess::borrowed(128),
        intent.clone(),
        &run,
    )
    .unwrap();
    let ticket3b = match pump3b.advance().unwrap() {
        PumpEvent::NeedInput { ticket, .. } => ticket,
        other => panic!("expected NeedInput, got {other:?}"),
    };
    // Wrong offset (99 instead of 0)
    let off_err = pump3b
        .supply_input(ticket3b, Input::new(99, b"bad").with_total_len(128))
        .unwrap_err();
    assert_eq!(off_err.error().failure(), &Failure::InvalidRequest);
    assert!(pump3b.advance().is_err());

    let mut pump3c = MediaPump::prepare_transcode(
        engine.clone(),
        ContainerId::Webm,
        &zencodec::source::SourceAccess::borrowed(128),
        intent.clone(),
        &run,
    )
    .unwrap();
    let ticket3c = match pump3c.advance().unwrap() {
        PumpEvent::NeedInput { ticket, .. } => ticket,
        other => panic!("expected NeedInput, got {other:?}"),
    };
    // Conflicting total_len (256 != 128)
    let len_err = pump3c
        .supply_input(ticket3c, Input::new(0, b"bad").with_total_len(256))
        .unwrap_err();
    assert_eq!(len_err.error().failure(), &Failure::InvalidRequest);
    assert!(pump3c.advance().is_err());

    // 5. fail_transport fuses permanently.
    let mut pump4 = MediaPump::prepare_transcode(
        engine.clone(),
        ContainerId::Webm,
        &access,
        intent.clone(),
        &run,
    )
    .unwrap();
    let t_err = pump4.fail_transport(Box::new(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "peer closed",
    )));
    assert_eq!(t_err.failure(), &Failure::Io);
    assert!(pump4.advance().is_err());

    // 6. MediaPump QuantizeEndpoints MUST check the FINAL endpoint (pts + duration) on WebM!
    //    Single frame at PTS 0 lasting 1/30 s has end error 1/3000 s on 1 ms WebM clock,
    //    which MUST be refused when max_boundary_error = 1/5000 s!
    let f_single = make_gradient_rgb8(64, 64, 10);
    let one_30th = FrameDuration::new(1, 30).unwrap();
    let single_mp4 = encode_frames_to_video(
        &engine,
        &[(f_single.as_view(), one_30th)],
        LoopCount::once(),
        &OutputIntent::video(ContainerId::Mp4, MediaCodecId::Vp9)
            .with_sequence(SequencePolicy::Preserve {
                timing: zencodecs::TimingPolicy::Exact,
                loops: LoopPolicy::Set(LoopCount::once()),
            })
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap();
    let tight_err = transcode_media_bytes(
        &engine,
        single_mp4.output.as_bytes(),
        &OutputIntent::video(ContainerId::Webm, MediaCodecId::Vp8)
            .with_sequence(SequencePolicy::Preserve {
                timing: zencodecs::TimingPolicy::QuantizeEndpoints {
                    max_boundary_error: FrameDuration::new(1, 5000).unwrap(),
                },
                loops: LoopPolicy::Set(LoopCount::once()),
            })
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap_err();
    assert_eq!(
        tight_err.error().failure(),
        &Failure::UnsupportedEffect,
        "1/30s frame end-endpoint error 1/3000s must exceed max_boundary_error 1/5000s on WebM"
    );

    // 6a. Conversely, a 1/10_000 s (100 us) clock yields exact Matroska TimestampScale = 100_000 ns
    //     in zenwebm, so a 100 us frame MUST succeed on WebM even under 50 us max_boundary_error!
    let hundred_us = FrameDuration::new(1, 10_000).unwrap();
    let us_mp4 = encode_frames_to_video(
        &engine,
        &[(f_single.as_view(), hundred_us)],
        LoopCount::once(),
        &OutputIntent::video(ContainerId::Mp4, MediaCodecId::Vp9)
            .with_sequence(SequencePolicy::Preserve {
                timing: zencodecs::TimingPolicy::Exact,
                loops: LoopPolicy::Set(LoopCount::once()),
            })
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .unwrap();
    let us_webm = transcode_media_bytes(
        &engine,
        us_mp4.output.as_bytes(),
        &OutputIntent::video(ContainerId::Webm, MediaCodecId::Vp8)
            .with_sequence(SequencePolicy::Preserve {
                timing: zencodecs::TimingPolicy::QuantizeEndpoints {
                    max_boundary_error: FrameDuration::new(1, 20_000).unwrap(),
                },
                loops: LoopPolicy::Set(LoopCount::once()),
            })
            .with_publication(PublicationMode::Verified),
        &run,
    )
    .expect("1/10_000 s clock has exact 100_000 ns WebM TimestampScale and 0 quantization error");
    assert!(!us_webm.output.as_bytes().is_empty());

    // 6b. Short nonempty reply to an exact Range request MUST actually hit Range, refuse, and fuse!
    let mp4_bytes = single_mp4.output.as_bytes();
    let mut pump5 = MediaPump::prepare_transcode(
        engine,
        ContainerId::Mp4,
        &zencodec::source::SourceAccess::borrowed(mp4_bytes.len() as u64),
        intent,
        &run,
    )
    .unwrap();
    let mut saw_range = false;
    for _ in 0..64 {
        match pump5.advance().unwrap() {
            PumpEvent::NeedInput { ticket, requests } => {
                if let zencodec::decode::InputRequest::Range(r) = requests[0]
                    && r.length > 1
                {
                    let short_err = pump5
                        .supply_input(
                            ticket,
                            Input::new(r.offset, &[mp4_bytes[r.offset as usize]])
                                .with_total_len(mp4_bytes.len() as u64),
                        )
                        .unwrap_err();
                    assert_eq!(short_err.error().failure(), &Failure::InvalidData);
                    assert!(pump5.advance().is_err());
                    saw_range = true;
                    break;
                }
                let off = requests[0].offset().unwrap_or(0) as usize;
                let end = (off + 16).min(mp4_bytes.len());
                pump5
                    .supply_input(
                        ticket,
                        Input::new(off as u64, &mp4_bytes[off..end])
                            .with_total_len(mp4_bytes.len() as u64),
                    )
                    .unwrap();
            }
            PumpEvent::NeedWriter { ticket, write } => {
                let len = match write {
                    zencodecs::media::pump::WriteOffer::Append(b)
                    | zencodecs::media::pump::WriteOffer::Patch { bytes: b, .. } => b.len(),
                };
                pump5.confirm_written(ticket, len).unwrap();
            }
            PumpEvent::Presented => {}
            PumpEvent::Finished => break,
        }
    }
    assert!(
        saw_range,
        "MP4 short-range regression test MUST observe an exact InputRequest::Range"
    );

    // 7. TrackPolicy::Preserve and DropNonVideo MUST refuse >1 video track on single-video targets!
    use zencodec_media::track::{TrackInfo, VideoTrackInfo};
    use zencodecs::engine::TrackPolicy;
    use zencodecs::media::route::validate_track_policy;
    let tb = TimeBase::new(1, 1000).unwrap();
    let t0 = TrackInfo::video(0, MediaCodecId::Av1, tb, VideoTrackInfo::new(64, 64))
        .build()
        .unwrap();
    let t1 = TrackInfo::video(1, MediaCodecId::Av1, tb, VideoTrackInfo::new(64, 64))
        .build()
        .unwrap();
    let two_video = vec![t0.clone(), t1];
    assert_eq!(
        validate_track_policy(&two_video, &TrackPolicy::Preserve, true)
            .unwrap_err()
            .error()
            .failure(),
        &Failure::UnsupportedEffect
    );
    assert_eq!(
        validate_track_policy(&two_video, &TrackPolicy::DropNonVideo, true)
            .unwrap_err()
            .error()
            .failure(),
        &Failure::UnsupportedEffect
    );
    assert_eq!(
        validate_track_policy(
            &two_video,
            &TrackPolicy::Select {
                video: 1,
                copy: Vec::new()
            },
            true,
        )
        .unwrap(),
        1
    );

    // 8. TimelineBridge refuses ambiguous same-PTS packet collisions (both orders + rational eq)!
    use zencodec_media::packet::Discard;
    use zencodec_media::timeline::{TimingEvidence, TrimTreatment};
    use zencodecs::media::timeline::TimelineBridge;
    let res = JobResources::new(
        ResourceLimits::default(),
        zencodec::storage::StorageBudget::new(64 * 1024 * 1024, 1024),
        zencodec::SharedPulse::default(),
    );
    let mut tl_a = TimelineBridge::prepare(
        &t0,
        TimingEvidence::new(
            TrimTreatment::ApplyInPresentation,
            TrimTreatment::ApplyInPresentation,
        ),
        &res,
    )
    .unwrap();
    let ts_ms = Timestamp::new(40, TimeBase::new(1, 1000).unwrap());
    let ts_us = Timestamp::new(40_000, TimeBase::new(1, 1_000_000).unwrap());
    let disc = Discard::start(FrameDuration::from_millis(5));
    // Order A: hidden packet with discard, then visible packet at same instant (different base!).
    tl_a.record_accepted_packet(Some(ts_ms), Some(disc))
        .unwrap();
    assert_eq!(
        tl_a.record_accepted_packet(Some(ts_us), None)
            .unwrap_err()
            .error()
            .failure(),
        &Failure::UnsupportedEffect
    );
    // Order B: visible packet without discard, then hidden packet with discard at same instant.
    let mut tl_b = TimelineBridge::prepare(
        &t0,
        TimingEvidence::new(
            TrimTreatment::ApplyInPresentation,
            TrimTreatment::ApplyInPresentation,
        ),
        &res,
    )
    .unwrap();
    tl_b.record_accepted_packet(Some(ts_ms), None).unwrap();
    assert_eq!(
        tl_b.record_accepted_packet(Some(ts_us), Some(disc))
            .unwrap_err()
            .error()
            .failure(),
        &Failure::UnsupportedEffect
    );
    // Discard without PTS MUST refuse (cannot silently lose discard evidence)!
    assert_eq!(
        tl_b.record_accepted_packet(None, Some(disc))
            .unwrap_err()
            .error()
            .failure(),
        &Failure::UnsupportedEffect
    );

    // 9. PublicationBuffer honors peak simultaneous memory cap (60 + 30 <= 100 B without 160 B spike)!
    use zencodecs::media::publication::PublicationBuffer;
    let tight_storage = zencodec::storage::StorageBudget::new(100, 16);
    let mut pb =
        PublicationBuffer::new(PublicationMode::Incremental, 100, tight_storage.clone()).unwrap();
    pb.append(&[1u8; 60]).unwrap();
    pb.append(&[2u8; 30]).unwrap();
    assert!(tight_storage.reserved_bytes() <= 100);
    // Appending 11 more bytes (101 > 100) MUST fail with ResourceLimit.
    assert_eq!(
        pb.append(&[3u8; 11]).unwrap_err().error().failure(),
        &Failure::ResourceLimit
    );
}
