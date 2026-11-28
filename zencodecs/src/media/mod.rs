//! First-class sans-I/O media container and video codec planning, pumping, and publication.
//!
//! Wires `zencodec::CodecSet` media roles (`DemuxRole`, `MuxRole`, `VideoDecodeRole`,
//! `VideoEncodeRole`), `zencodec_media::timeline::PresentationMap`, and
//! `zencodec_render::media::{VideoRenderPlan, VideoCapturePlan}` under the single
//! [`Engine`](crate::engine::Engine) + [`Routing`](crate::routing::Routing) +
//! [`JobResources`](zencodec::JobResources) contract.

pub mod probe;
pub mod publication;
pub mod pump;
pub mod route;
pub mod select;
pub mod timeline;

pub use probe::{ContainerProbeOutcome, probe_container_bytes};
pub use publication::{PublicationBuffer, PublishedMedia, VerifiedCheck};
pub use pump::{
    IoTicket, MediaPump, PreparedMediaTranscode, PumpEvent, WriteOffer, drive_pump_in_memory,
    extract_poster_from_bytes, transcode_media_bytes,
};
pub use route::{MediaDecodePlan, MediaRoutePlan, MediaTranscodePlan, plan_media_route};
pub use select::{
    DecodeSelection, PacketIndexEntry, PacketIndexV1, PresentationEntry, PresentationIndex,
    SeekMode, build_presentation_index, select_presentation_index,
};
pub use timeline::{
    QuantizedTiming, TimelineBridge, container_supports_loops, normalize_loop_for_target,
    quantize_endpoints,
};
