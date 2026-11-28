//! Unified media route planning and candidate ranking (`Demux`, `VideoDecode`, `VideoEncode`, `Mux`).

use alloc::sync::Arc;
use alloc::vec::Vec;

use whereat::at;
use zencodec::source::{AccessPattern, SourceAccess};
use zencodec::RegistrationId;
use zencodec_media::container::MuxHint;
use zencodec_media::registry::{
    ContainerId, DemuxFactory, DemuxNeeds, DemuxRole, MuxFactory, MuxNeeds, MuxRole, TimingPolicy,
    VideoDecodeFactory, VideoDecodeNeeds, VideoDecodeRole, VideoEncodeFactory, VideoEncodeNeeds,
    VideoEncodeRole,
};
use zencodec_media::session::CodecConfig;
use zencodec_media::track::{CodecId as MediaCodecId, TrackInfo, TrackKind};

use crate::engine::{
    EffectiveRun, Engine, EngineError, EngineResult, Failure, OutputIntent, PublicationMode,
    RejectReason, RouteKind, RouteReport, SelectionEvent, Stage, TargetFormat, TrackPolicy,
};
use crate::media::probe::{ContainerProbeOutcome, probe_container_prefix};
use crate::routing::RuleKey;

/// Ranked demux + video-decode route plan for extracting frames/posters from a media container.
#[derive(Clone)]
pub struct MediaDecodePlan {
    /// Confirmed source container.
    pub container: ContainerId,
    /// Selected demuxer ID.
    pub demuxer_id: RegistrationId,
    /// Selected demuxer factory.
    pub demuxer: Arc<dyn DemuxFactory>,
    /// Optional selected video decoder ID (bound once track codec is known).
    pub video_decoder_id: Option<RegistrationId>,
    /// Optional selected video decoder factory.
    pub video_decoder: Option<Arc<dyn VideoDecodeFactory>>,
}

/// Complete planned video/media transcode route (demux -> decode -> encode -> mux, or packet remux).
#[derive(Clone)]
pub struct MediaTranscodePlan {
    /// Source decode plan.
    pub decode: MediaDecodePlan,
    /// Destination container identity.
    pub target_container: ContainerId,
    /// Destination video codec identity.
    pub target_codec: MediaCodecId,
    /// Selected video encoder ID (`None` for direct packet remux).
    pub video_encoder_id: Option<RegistrationId>,
    /// Selected video encoder factory (`None` for direct packet remux).
    pub video_encoder: Option<Arc<dyn VideoEncodeFactory>>,
    /// Selected muxer ID.
    pub muxer_id: RegistrationId,
    /// Selected muxer factory.
    pub muxer: Arc<dyn MuxFactory>,
    /// Whether this route can stream-copy packets without re-encoding.
    pub packet_remux: bool,
}

/// Unified planned media route returned by [`plan_media_route`].
#[derive(Clone)]
pub enum MediaRoutePlan {
    /// Decode-only plan (e.g. poster extraction or video -> still/animation).
    Decode(MediaDecodePlan),
    /// Full container/video transcode or remux plan.
    Transcode(MediaTranscodePlan),
}

/// Validate container × video-codec compatibility against the qualified carrier matrix.
pub fn validate_container_codec_tuple(
    container: &ContainerId,
    codec: &MediaCodecId,
) -> EngineResult<()> {
    let ok = match container {
        ContainerId::Mp4 => matches!(
            codec,
            MediaCodecId::Av1 | MediaCodecId::Vp8 | MediaCodecId::Vp9
        ),
        ContainerId::Webm => matches!(
            codec,
            MediaCodecId::Av1 | MediaCodecId::Vp8 | MediaCodecId::Vp9
        ),
        ContainerId::Ivf => matches!(
            codec,
            MediaCodecId::Av1 | MediaCodecId::Vp8 | MediaCodecId::Vp9
        ),
        _ => true,
    };
    if !ok {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            "requested video codec is not representable in target container",
        )));
    }
    Ok(())
}

/// Validate non-video track disposition against discovered tracks and target container.
pub fn validate_track_policy(
    tracks: &[TrackInfo],
    policy: &TrackPolicy,
    target_is_single_video_only: bool,
) -> EngineResult<usize> {
    let video_indices: Vec<usize> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind() == TrackKind::Video)
        .map(|(i, _)| i)
        .collect();
    if video_indices.is_empty() {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::InvalidData,
            "media container has no video track",
        )));
    }

    let has_non_video = tracks.iter().any(|t| t.kind() != TrackKind::Video);

    match policy {
        TrackPolicy::Preserve => {
            if has_non_video && target_is_single_video_only {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "input contains audio/non-video tracks that target cannot carry; specify TrackPolicy::DropNonVideo explicitly",
                )));
            }
            if video_indices.len() > 1 && target_is_single_video_only {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "input contains multiple video tracks that single-video target cannot preserve; specify TrackPolicy::Select explicitly",
                )));
            }
            Ok(video_indices[0])
        }
        TrackPolicy::DropNonVideo => {
            if video_indices.len() > 1 && target_is_single_video_only {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "input contains multiple video tracks that single-video target cannot preserve; specify TrackPolicy::Select explicitly",
                )));
            }
            Ok(video_indices[0])
        }
        TrackPolicy::Select { video, copy } => {
            let v_idx = *video as usize;
            if tracks.get(v_idx).is_none_or(|t| t.kind() != TrackKind::Video) {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::InvalidRequest,
                    "TrackPolicy::Select video index does not refer to a video track",
                )));
            }
            if target_is_single_video_only && !copy.is_empty() {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::UnsupportedEffect,
                    "target container/format cannot carry copied side tracks",
                )));
            }
            for &c in copy {
                if (c as usize) >= tracks.len() {
                    return Err(at!(EngineError::simple(
                        Stage::Admit,
                        Failure::InvalidRequest,
                        "TrackPolicy::Select copy track index out of range",
                    )));
                }
            }
            Ok(v_idx)
        }
    }
}

/// Select the top-ranked eligible demuxer factory for a confirmed container.
pub fn select_demuxer(
    engine: &Engine,
    container: &ContainerId,
    eff: &mut EffectiveRun,
    report: &mut RouteReport,
) -> EngineResult<(RegistrationId, Arc<dyn DemuxFactory>)> {
    if !eff.routing.permits_input_container(container) {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::PolicyRefusal,
            "input media container is denied by Routing",
        )));
    }
    let key = RuleKey::Demux(container.clone());
    let ranked = eff.routing.rank_candidates(
        &key,
        engine
            .codecs()
            .factories::<DemuxRole>(container)
            .map(|e| (e.id.clone(), Arc::clone(&e.factory))),
    );
    let Some((id, factory)) = ranked.into_iter().next() else {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::NoMediaBackend,
            "no eligible registered DemuxFactory for input container",
        )));
    };
    report.demuxer = Some(id.clone());
    eff.trace_event(SelectionEvent::Selected {
        id: id.clone(),
        route: RouteKind::Media,
    });
    Ok((id, factory))
}

/// Select the top-ranked eligible video decoder factory for `codec`, checking workspace bounds.
pub fn select_video_decoder(
    engine: &Engine,
    config: &CodecConfig,
    need: &VideoDecodeNeeds,
    eff: &mut EffectiveRun,
    report: &mut RouteReport,
) -> EngineResult<(RegistrationId, Arc<dyn VideoDecodeFactory>, zencodec_media::registry::VideoDecodePlan)>
{
    let codec = config.codec.clone();
    let key = RuleKey::VideoDecode(codec.clone());
    let ranked = eff.routing.rank_candidates(
        &key,
        engine
            .codecs()
            .factories::<VideoDecodeRole>(&codec)
            .map(|e| (e.id.clone(), Arc::clone(&e.factory))),
    );
    if ranked.is_empty() {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::NoMediaBackend,
            "no eligible registered VideoDecodeFactory for stream codec",
        )));
    }

    let allow_fallback = eff.routing.allows_fallback(&key);
    let mut last_err = None;
    for (id, factory) in ranked {
        match factory.quote(config, need, &eff.resources) {
            Ok(plan) => {
                let mut probe_report = report.clone();
                match eff.admit_workspace(id.as_str(), plan.workspace, &mut probe_report) {
                    Ok(()) => {
                        *report = probe_report;
                        report.decoder = Some(id.clone());
                        eff.trace_event(SelectionEvent::Selected {
                            id: id.clone(),
                            route: RouteKind::Media,
                        });
                        return Ok((id, factory, plan));
                    }
                    Err(err) => {
                        eff.trace_event(SelectionEvent::Rejected {
                            id: Some(id),
                            reason: RejectReason::Limits,
                        });
                        last_err = Some(err);
                        if !allow_fallback {
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                eff.trace_event(SelectionEvent::Rejected {
                    id: Some(id),
                    reason: RejectReason::Unbuildable,
                });
                last_err = Some(at!(EngineError::simple(
                    Stage::Prepare,
                    Failure::UnsupportedEffect,
                    "video decoder quote failed",
                )
                .with_cause(err)));
                if !allow_fallback {
                    break;
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        at!(EngineError::simple(
            Stage::Admit,
            Failure::NoMediaBackend,
            "no video decoder candidate satisfied admission",
        ))
    }))
}

/// Select the top-ranked eligible video encoder factory for `need`, checking workspace bounds.
pub fn select_video_encoder(
    engine: &Engine,
    need: &VideoEncodeNeeds,
    eff: &mut EffectiveRun,
    report: &mut RouteReport,
) -> EngineResult<(RegistrationId, Arc<dyn VideoEncodeFactory>, zencodec_media::registry::VideoEncodePlan)>
{
    let key = RuleKey::VideoEncode(need.codec.clone());
    let ranked = eff.routing.rank_candidates(
        &key,
        engine
            .codecs()
            .factories::<VideoEncodeRole>(&need.codec)
            .map(|e| (e.id.clone(), Arc::clone(&e.factory))),
    );
    if ranked.is_empty() {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::NoMediaBackend,
            "no eligible registered VideoEncodeFactory for target video codec",
        )));
    }

    let allow_fallback = eff.routing.allows_fallback(&key);
    let mut last_err = None;
    for (id, factory) in ranked {
        match factory.quote(need, &eff.resources) {
            Ok(plan) => {
                let mut probe_report = report.clone();
                match eff.admit_workspace(id.as_str(), plan.workspace, &mut probe_report) {
                    Ok(()) => {
                        *report = probe_report;
                        report.encoder = Some(id.clone());
                        eff.trace_event(SelectionEvent::Selected {
                            id: id.clone(),
                            route: RouteKind::Media,
                        });
                        return Ok((id, factory, plan));
                    }
                    Err(err) => {
                        eff.trace_event(SelectionEvent::Rejected {
                            id: Some(id),
                            reason: RejectReason::Limits,
                        });
                        last_err = Some(err);
                        if !allow_fallback {
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                eff.trace_event(SelectionEvent::Rejected {
                    id: Some(id),
                    reason: RejectReason::Unbuildable,
                });
                last_err = Some(at!(EngineError::simple(
                    Stage::Prepare,
                    Failure::UnsupportedEffect,
                    "video encoder quote failed",
                )
                .with_cause(err)));
                if !allow_fallback {
                    break;
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        at!(EngineError::simple(
            Stage::Admit,
            Failure::NoMediaBackend,
            "no video encoder candidate satisfied admission",
        ))
    }))
}

/// Select the top-ranked eligible muxer factory for `container`, checking workspace & timing.
pub fn select_muxer(
    engine: &Engine,
    container: &ContainerId,
    tracks: Arc<[TrackInfo]>,
    publication: PublicationMode,
    timing: TimingPolicy,
    eff: &mut EffectiveRun,
    report: &mut RouteReport,
) -> EngineResult<(RegistrationId, Arc<dyn MuxFactory>, zencodec_media::registry::MuxPlan)> {
    if !eff.routing.permits_output_container(container) {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::PolicyRefusal,
            "target output container is denied by Routing",
        )));
    }
    let key = RuleKey::Mux(container.clone());
    let ranked = eff.routing.rank_candidates(
        &key,
        engine
            .codecs()
            .factories::<MuxRole>(container)
            .map(|e| (e.id.clone(), Arc::clone(&e.factory))),
    );
    if ranked.is_empty() {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::NoMediaBackend,
            "no eligible registered MuxFactory for target container",
        )));
    }

    let hint = match publication {
        PublicationMode::Incremental => MuxHint::forward_only(),
        PublicationMode::Verified => MuxHint::default(),
    };
    let need = MuxNeeds::new(tracks.to_vec(), hint).with_timing(timing);
    let allow_fallback = eff.routing.allows_fallback(&key);
    let mut last_err = None;

    for (id, factory) in ranked {
        match factory.quote(&need, &eff.resources) {
            Ok(plan) => {
                let mut probe_report = report.clone();
                match eff.admit_workspace(id.as_str(), plan.offer.workspace, &mut probe_report) {
                    Ok(()) => {
                        *report = probe_report;
                        report.muxer = Some(id.clone());
                        report.output_container = Some(container.clone());
                        eff.trace_event(SelectionEvent::Selected {
                            id: id.clone(),
                            route: RouteKind::Media,
                        });
                        return Ok((id, factory, plan));
                    }
                    Err(err) => {
                        eff.trace_event(SelectionEvent::Rejected {
                            id: Some(id),
                            reason: RejectReason::Limits,
                        });
                        last_err = Some(err);
                        if !allow_fallback {
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                eff.trace_event(SelectionEvent::Rejected {
                    id: Some(id),
                    reason: RejectReason::Unbuildable,
                });
                last_err = Some(at!(EngineError::simple(
                    Stage::Prepare,
                    Failure::UnsupportedEffect,
                    "muxer quote failed",
                )
                .with_cause(err)));
                if !allow_fallback {
                    break;
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        at!(EngineError::simple(
            Stage::Admit,
            Failure::NoMediaBackend,
            "no muxer candidate satisfied admission",
        ))
    }))
}

/// High-level planner: probe container prefix, select demuxer, and if targeting video, select muxer.
pub fn plan_media_route(
    engine: &Engine,
    prefix: &[u8],
    intent: &OutputIntent,
    source_access: &SourceAccess,
    eff: &mut EffectiveRun,
    report: &mut RouteReport,
) -> EngineResult<MediaRoutePlan> {
    intent.validate()?;
    let outcome = probe_container_prefix(
        engine.codecs(),
        prefix,
        &zencodec_media::registry::ProbeLimits::default(),
    )?;
    let container = match outcome {
        ContainerProbeOutcome::Confirmed { container, .. } => container,
        ContainerProbeOutcome::NeedMore { .. } => {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::InvalidData,
                "insufficient container prefix bytes to confirm container identity",
            )));
        }
        ContainerProbeOutcome::NoMatch => {
            return Err(at!(EngineError::simple(
                Stage::Inspect,
                Failure::UnrecognizedFormat,
                "prefix did not match any registered media container",
            )));
        }
    };

    let (demuxer_id, demuxer) = select_demuxer(engine, &container, eff, report)?;
    // Verify demuxer quote against caller's actual SourceAccess.
    let demux_need = DemuxNeeds::default();
    let demux_offer = demuxer
        .quote(&demux_need, source_access, &eff.resources)
        .map_err(|e| {
            at!(EngineError::simple(
                Stage::Prepare,
                Failure::UnsupportedEffect,
                "demuxer rejected source access or limits",
            )
            .with_cause(e))
        })?;
    if source_access.access_pattern == AccessPattern::Sequential
        && demux_offer.required_access == AccessPattern::RandomAccess
        && !source_access.immutable
    {
        return Err(at!(EngineError::simple(
            Stage::Admit,
            Failure::UnsupportedEffect,
            "demuxer requires random-access input but source is strictly sequential",
        )));
    }
    eff.admit_workspace(demuxer_id.as_str(), demux_offer.workspace, report)?;

    let decode_plan = MediaDecodePlan {
        container,
        demuxer_id,
        demuxer,
        video_decoder_id: None,
        video_decoder: None,
    };

    match &intent.target {
        TargetFormat::Video {
            container: dst_container,
            codec: dst_codec,
        } => {
            validate_container_codec_tuple(dst_container, dst_codec)?;
            if !eff.routing.permits_output_container(dst_container) {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::PolicyRefusal,
                    "target output container is denied by Routing",
                )));
            }
            let mux_key = RuleKey::Mux(dst_container.clone());
            let mux_ranked = eff.routing.rank_candidates(
                &mux_key,
                engine
                    .codecs()
                    .factories::<MuxRole>(dst_container)
                    .map(|e| (e.id.clone(), Arc::clone(&e.factory))),
            );
            let Some((muxer_id, muxer)) = mux_ranked.into_iter().next() else {
                return Err(at!(EngineError::simple(
                    Stage::Admit,
                    Failure::NoMediaBackend,
                    "no eligible MuxFactory for target container",
                )));
            };
            let enc_key = RuleKey::VideoEncode(dst_codec.clone());
            let enc_ranked = eff.routing.rank_candidates(
                &enc_key,
                engine
                    .codecs()
                    .factories::<VideoEncodeRole>(dst_codec)
                    .map(|e| (e.id.clone(), Arc::clone(&e.factory))),
            );
            let (video_encoder_id, video_encoder) = match enc_ranked.into_iter().next() {
                Some((id, f)) => (Some(id), Some(f)),
                None => (None, None),
            };
            report.output_container = Some(dst_container.clone());
            report.output_video_codec = Some(dst_codec.clone());
            report.muxer = Some(muxer_id.clone());
            if let Some(ref eid) = video_encoder_id {
                report.encoder = Some(eid.clone());
            }
            Ok(MediaRoutePlan::Transcode(MediaTranscodePlan {
                decode: decode_plan,
                target_container: dst_container.clone(),
                target_codec: dst_codec.clone(),
                video_encoder_id,
                video_encoder,
                muxer_id,
                muxer,
                packet_remux: false,
            }))
        }
        _ => Ok(MediaRoutePlan::Decode(decode_plan)),
    }
}
