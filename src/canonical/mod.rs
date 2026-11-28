//! Canonical 10-node `zenpipe` IR, strict JSON v1 wire contract (`P`), and unified
//! `PipelineRuntime` over `Arc<zencodecs::Engine>` for stills, animations, video
//! (AV1/VP8/VP9 × MP4/WebM/IVF), and gain maps.

pub mod graph;
#[cfg(feature = "canonical-json")]
pub mod json;
#[cfg(feature = "zennode")]
pub mod runtime;

pub use graph::{
    BlendKind, CanonicalJob, ColorIngestMode, ColorIngestOp, ColorSpaceOp, CompositeOp,
    DecodeNodeOp, EncodeNodeOp, FilterNodeOp, GainMapNodeOp, GeometryOp, Graph, GraphBuilder,
    GraphError, ImageId, ImageOp, MakeGainMapOp, OutputOp, ResizeKernel, SlotName, ToneMapNodeOp,
};
#[cfg(feature = "canonical-json")]
pub use json::{
    OutputArtifactV2, OutputV2, PipelineEnvelopeV1, WireLimitConfig, parse_json_v1,
    parse_pipeline_json,
};
#[cfg(feature = "zennode")]
pub use runtime::{
    AdmissionReport, Bindings, HostServices, JobResult, OutputArtifact, PipelineRuntime,
    PreparedJob,
};
