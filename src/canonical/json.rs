//! Strict canonical JSON v1 wire contract (`P`, `PipelineEnvelopeV1`, `OutputV2`).
//!
//! Shared identically by:
//! - `zenpipe` JSON v1 (`{"zenpipe": 1, "inputs": {...}, "outputs": {...}, "pipeline": P}`),
//! - `imageflow` `v1/execute_zen` & `v2/execute` (`{"inputs": {"main": 0}, "outputs": {"web": 1}, "pipeline": P}`).
//!
//! Enforces strict key validation (unknown keys refused), bounded ingress bytes/nodes,
//! exact rational timestamps & durations, and zero backend crate types on the wire.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::num::NonZeroU32;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use whereat::at;
use zencodec::ImageFormat;
use zencodec::time::FrameDuration;
#[cfg(feature = "media")]
use zencodec::time::{TimeBase, Timestamp};
use zencodecs::engine::{
    AlphaPolicy, ChromaPolicy, ChromaSampling, CodecOptions, Coding, ColorTarget,
    DecodeOrientation, Effort, FormatPreferences, GainMapAction, GainMapEncodePolicy,
    GainMapFidelity, GainMapRendition, GamutMode, LoopCount, LoopPolicy, MetadataPolicy, Metric,
    MetricTarget, OptionMode, OptionValue, OrientationMode, OutputIntent, PublicationMode,
    QualityLevel, RouteReport, SequencePolicy, TargetFormat, TimingPolicy, ToneMap, ToneMapMethod,
    ToneMapPolicy, TrackPolicy,
};
use zenlayout::{Constraint, ConstraintMode, Orientation, Rect, Size};
use zenpixels::{ColorSpace, DiffuseWhite, Nits, PeakSource, PixelFormat, Primaries};

#[cfg(feature = "media")]
use zencodec_media::registry::ContainerId;
#[cfg(feature = "media")]
use zencodec_media::track::CodecId as MediaCodecId;
#[cfg(feature = "media")]
use zencodecs::media::select::{DecodeSelection, SeekMode};

use crate::canonical::graph::{
    BlendKind, CanonicalJob, ColorIngestMode, ColorIngestOp, ColorSpaceOp, CompositeOp,
    DecodeNodeOp, EncodeNodeOp, FilterNodeOp, GainMapNodeOp, GeometryOp, Graph, GraphBuilder,
    GraphError, ImageId, ImageOp, MakeGainMapOp, OutputOp, ResizeKernel, SlotName, ToneMapNodeOp,
};
use crate::error::{PipeError, PipeResult};

/// Ingress DoS ceilings enforced BEFORE parsing JSON structures or allocating buffers.
#[derive(Clone, Copy, Debug)]
pub struct WireLimitConfig {
    /// Maximum raw JSON byte length accepted (default 256 KiB).
    pub max_json_bytes: usize,
    /// Maximum steps/nodes accepted (default 256).
    pub max_nodes: usize,
}

impl Default for WireLimitConfig {
    fn default() -> Self {
        Self {
            max_json_bytes: 256 * 1024,
            max_nodes: 256,
        }
    }
}

/// Top-level `zenpipe` JSON v1 envelope (`{"zenpipe": 1, "inputs": ..., "outputs": ..., "pipeline": P}`).
#[derive(Clone, Debug)]
pub struct PipelineEnvelopeV1 {
    /// Declared input slots from the envelope.
    pub inputs: Vec<SlotName>,
    /// Declared output slots from the envelope.
    pub outputs: Vec<SlotName>,
    /// Lowered validated [`CanonicalJob`].
    pub job: CanonicalJob,
}

/// Single output artifact entry in the unified modern wire response [`OutputV2`].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OutputArtifactV2 {
    /// Output slot name.
    pub slot: String,
    /// Media kind (`"still"`, `"animation"`, `"video"`, `"pixels"`).
    pub kind: String,
    /// MIME type of encoded output (e.g. `"image/webp"`, `"video/mp4"`, `"video/webm"`).
    pub mime: String,
    /// Emitted byte length.
    pub bytes: u64,
    /// Output canvas width in pixels.
    pub width: u32,
    /// Output canvas height in pixels.
    pub height: u32,
    /// Frame count if established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<u64>,
    /// Selected video codec token (`"av1"`, `"vp8"`, `"vp9"`) for video outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    /// Requested metric token when `Coding::Verify` executed (e.g. `"zensim_a"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_metric: Option<String>,
    /// Effective scorer revision when `Coding::Verify` executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_revision: Option<String>,
    /// Requested target threshold when `Coding::Verify` executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_target: Option<f32>,
    /// Measured scalar score when `Coding::Verify` executed (score-only, never diffmaps).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified_score: Option<f32>,
    /// Native `f64` measured scalar score when `Coding::Verify` executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_score_f64: Option<f64>,
    /// Selected calibrated quality parameter when `Coding::Verify` converged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_quality: Option<f32>,
    /// Number of verification trials executed when `Coding::Verify` ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_attempts: Option<u32>,
    /// Requested metric token when one-shot `Coding::AimFor` executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim_metric: Option<String>,
    /// Requested target threshold when one-shot `Coding::AimFor` executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim_target: Option<f32>,
    /// Calibration heuristic revision when `Coding::AimFor` executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim_revision: Option<String>,
    /// Heuristic estimated quality selected when `Coding::AimFor` executed (unverified).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim_estimated_quality: Option<f32>,
    /// Components admitted with unknown native workspace bounds (empty under Strict policy).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_workspace_components: Vec<String>,
}

/// Shared modern wire response returned by `zenpipe` JSON v1 and `imageflow` `v1/execute_zen` & `v2/execute`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OutputV2 {
    /// Wire contract version (`2`).
    pub version: u32,
    /// Completed output artifacts keyed in declaration order.
    pub outputs: Vec<OutputArtifactV2>,
    /// Non-fatal diagnostics/advisory warnings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl OutputV2 {
    /// Construct a version-2 response.
    #[must_use]
    pub fn new(outputs: Vec<OutputArtifactV2>, warnings: Vec<String>) -> Self {
        Self {
            version: 2,
            outputs,
            warnings,
        }
    }

    /// Build an [`OutputArtifactV2`] summary from a [`RouteReport`].
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn artifact_from_report(
        slot: String,
        kind: &str,
        mime: String,
        bytes: u64,
        width: u32,
        height: u32,
        frames: Option<u64>,
        video_codec: Option<String>,
        report: &RouteReport,
    ) -> OutputArtifactV2 {
        OutputArtifactV2 {
            slot,
            kind: kind.to_string(),
            mime,
            bytes,
            width,
            height,
            frames,
            video_codec,
            verified_metric: report.verified_metric.map(|m| m.as_token().to_string()),
            verified_revision: report.verified_revision.clone(),
            verified_target: report.verified_target,
            verified_score: report.verified_score,
            verified_score_f64: report.verified_score_f64,
            verified_quality: report.verified_quality,
            verify_attempts: (report.verify_attempts > 0).then_some(report.verify_attempts),
            aim_metric: report.aim_metric.map(|m| m.as_token().to_string()),
            aim_target: report.aim_target,
            aim_revision: report.aim_revision.clone(),
            aim_estimated_quality: report.aim_estimated_quality,
            unknown_workspace_components: report.unknown_workspace_components.clone(),
        }
    }
}

fn bad_json(msg: impl Into<String>) -> whereat::At<PipeError> {
    at!(PipeError::Op(msg.into()))
}

/// Deserialize JSON bytes into `serde_json::Value` while strictly rejecting duplicate
/// object member keys at every nesting depth (preventing silent overwrite of duplicate
/// output slots, duplicate graph node IDs, or duplicate policy keys).
fn strict_json_from_slice(bytes: &[u8]) -> PipeResult<Value> {
    use core::fmt;
    use serde::de::{Deserialize, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

    struct StrictVal(Value);
    struct StrictVisitor;

    impl<'de> Visitor<'de> for StrictVisitor {
        type Value = StrictVal;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("any valid JSON value without duplicate object keys")
        }

        fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
            Ok(StrictVal(Value::Bool(v)))
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
            Ok(StrictVal(Value::Number(v.into())))
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
            Ok(StrictVal(Value::Number(v.into())))
        }

        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
            let num = serde_json::Number::from_f64(v)
                .ok_or_else(|| E::custom("non-finite JSON number"))?;
            Ok(StrictVal(Value::Number(num)))
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
            Ok(StrictVal(Value::String(v.to_string())))
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
            Ok(StrictVal(Value::String(v)))
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E> {
            Ok(StrictVal(Value::Null))
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(elem) = seq.next_element_seed(StrictSeed)? {
                out.push(elem.0);
            }
            Ok(StrictVal(Value::Array(out)))
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = serde_json::Map::new();
            while let Some(key) = map.next_key::<String>()? {
                if out.contains_key(&key) {
                    return Err(serde::de::Error::custom(alloc::format!(
                        "duplicate JSON object key '{key}'"
                    )));
                }
                let val = map.next_value_seed(StrictSeed)?;
                out.insert(key, val.0);
            }
            Ok(StrictVal(Value::Object(out)))
        }
    }

    struct StrictSeed;
    impl<'de> DeserializeSeed<'de> for StrictSeed {
        type Value = StrictVal;
        fn deserialize<D: Deserializer<'de>>(
            self,
            deserializer: D,
        ) -> Result<Self::Value, D::Error> {
            deserializer.deserialize_any(StrictVisitor)
        }
    }

    impl<'de> Deserialize<'de> for StrictVal {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_any(StrictVisitor)
        }
    }

    serde_json::from_slice::<StrictVal>(bytes)
        .map(|s| s.0)
        .map_err(|e| bad_json(alloc::format!("invalid JSON: {e}")))
}

fn check_allowed_keys(
    obj: &serde_json::Map<String, Value>,
    allowed: &[&str],
    ctx: &str,
) -> PipeResult<()> {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(bad_json(alloc::format!("unknown key '{key}' in {ctx}")));
        }
    }
    Ok(())
}

/// Parse a complete `zenpipe` JSON v1 envelope (`{"zenpipe": 1, "inputs": ..., "outputs": ..., "pipeline": P}`)
/// or shorthand with top-level `"steps"`/`"graph"`.
pub fn parse_json_v1(bytes: &[u8], limits: WireLimitConfig) -> PipeResult<PipelineEnvelopeV1> {
    if bytes.len() > limits.max_json_bytes {
        return Err(at!(PipeError::LimitExceeded(alloc::format!(
            "JSON payload {} B exceeds max_json_bytes {}",
            bytes.len(),
            limits.max_json_bytes
        ))));
    }
    let root = strict_json_from_slice(bytes)?;
    let obj = root
        .as_object()
        .ok_or_else(|| bad_json("zenpipe JSON v1 root must be an object"))?;

    check_allowed_keys(
        obj,
        &[
            "zenpipe", "inputs", "outputs", "pipeline", "steps", "graph", "limits", "color",
            "metadata",
        ],
        "zenpipe v1 envelope",
    )?;

    if let Some(ver) = obj.get("zenpipe")
        && ver.as_u64() != Some(1)
    {
        return Err(bad_json(
            "unsupported zenpipe envelope version (expected 1)",
        ));
    }

    let mut inputs = Vec::new();
    if let Some(in_val) = obj.get("inputs") {
        let in_obj = in_val
            .as_object()
            .ok_or_else(|| bad_json("envelope 'inputs' must be an object"))?;
        for k in in_obj.keys() {
            inputs.push(SlotName::new(k).map_err(|e| bad_json(e.to_string()))?);
        }
    }

    let mut outputs = Vec::new();
    if let Some(out_val) = obj.get("outputs") {
        let out_obj = out_val
            .as_object()
            .ok_or_else(|| bad_json("envelope 'outputs' must be an object"))?;
        for k in out_obj.keys() {
            outputs.push(SlotName::new(k).map_err(|e| bad_json(e.to_string()))?);
        }
    }

    let mut job = if let Some(p) = obj.get("pipeline") {
        if obj.contains_key("steps") || obj.contains_key("graph") {
            return Err(bad_json(
                "specify either 'pipeline' or top-level 'steps'/'graph', not both",
            ));
        }
        parse_pipeline_value(p, limits)?
    } else {
        let has_steps = obj.contains_key("steps");
        let has_graph = obj.contains_key("graph");
        if has_steps == has_graph {
            return Err(bad_json(
                "zenpipe v1 envelope must contain 'pipeline' or exactly one of 'steps' or 'graph'",
            ));
        }
        let graph = if has_steps {
            let steps = obj["steps"]
                .as_array()
                .ok_or_else(|| bad_json("'steps' must be an array"))?;
            parse_steps(steps, limits)?
        } else {
            parse_explicit_graph(&obj["graph"], limits)?
        };
        CanonicalJob::from_graph(graph)
    };
    if let Some(lim_val) = obj.get("limits") {
        let lim_obj = lim_val
            .as_object()
            .ok_or_else(|| bad_json("'limits' must be an object"))?;
        check_allowed_keys(lim_obj, &["max_output_bytes", "max_dimension"], "limits")?;
        if let Some(v) = lim_obj.get("max_output_bytes") {
            job.max_output_bytes =
                Some(v.as_u64().ok_or_else(|| {
                    bad_json("limits.max_output_bytes must be an unsigned integer")
                })?);
        }
        if let Some(v) = lim_obj.get("max_dimension") {
            let d = v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| bad_json("limits.max_dimension must be a u32"))?;
            job.max_dimension = Some(d);
        }
    }

    // Validate declared envelope slots cover graph slots when declared.
    if !inputs.is_empty() {
        for slot in job.graph().input_slots() {
            if !inputs.contains(slot) {
                return Err(bad_json(alloc::format!(
                    "graph references undeclared input slot '{}'",
                    slot.as_str()
                )));
            }
        }
    }
    if !outputs.is_empty() {
        for slot in job.graph().output_slots() {
            if !outputs.contains(slot) {
                return Err(bad_json(alloc::format!(
                    "graph references undeclared output slot '{}'",
                    slot.as_str()
                )));
            }
        }
    }

    Ok(PipelineEnvelopeV1 {
        inputs,
        outputs,
        job,
    })
}

/// Parse the shared canonical pipeline payload `P` (`{"steps": [...]}` or `{"graph": {"nodes": ..., "edges": ...}}`).
///
/// Used verbatim by both `zenpipe` JSON v1 AND `imageflow` `v1/execute_zen` & `v2/execute`.
pub fn parse_pipeline_json(bytes: &[u8], limits: WireLimitConfig) -> PipeResult<CanonicalJob> {
    if bytes.len() > limits.max_json_bytes {
        return Err(at!(PipeError::LimitExceeded(alloc::format!(
            "pipeline JSON {} B exceeds max_json_bytes {}",
            bytes.len(),
            limits.max_json_bytes
        ))));
    }
    let val = strict_json_from_slice(bytes)?;
    parse_pipeline_value(&val, limits)
}

/// Parse an already-deserialized JSON [`Value`] representing `P` (`steps` or `graph`).
pub fn parse_pipeline_value(val: &Value, limits: WireLimitConfig) -> PipeResult<CanonicalJob> {
    let obj = val
        .as_object()
        .ok_or_else(|| bad_json("pipeline value must be a JSON object"))?;

    let has_steps = obj.contains_key("steps");
    let has_graph = obj.contains_key("graph");
    if has_steps == has_graph {
        return Err(bad_json(
            "pipeline object must contain exactly one of 'steps' or 'graph'",
        ));
    }

    let graph = if has_steps {
        check_allowed_keys(obj, &["steps"], "pipeline")?;
        let steps = obj["steps"]
            .as_array()
            .ok_or_else(|| bad_json("'steps' must be an array"))?;
        parse_steps(steps, limits)?
    } else {
        check_allowed_keys(obj, &["graph"], "pipeline")?;
        parse_explicit_graph(&obj["graph"], limits)?
    };
    Ok(CanonicalJob::from_graph(graph))
}

fn parse_steps(steps: &[Value], limits: WireLimitConfig) -> PipeResult<Graph> {
    if steps.len() > limits.max_nodes {
        return Err(at!(PipeError::LimitExceeded(
            "steps length exceeds max_nodes".into()
        )));
    }
    let mut b = GraphBuilder::new();
    let mut cursor: Option<ImageId> = None;
    let mut checkpoints: BTreeMap<String, ImageId> = BTreeMap::new();

    for (idx, step) in steps.iter().enumerate() {
        let obj = step
            .as_object()
            .ok_or_else(|| bad_json(alloc::format!("step[{idx}] must be a single-key object")))?;
        if obj.len() != 1 {
            return Err(bad_json(alloc::format!(
                "step[{idx}] must have exactly 1 operation key, found {}",
                obj.len()
            )));
        }
        let (key, payload) = obj.iter().next().expect("len == 1");
        match key.as_str() {
            "checkpoint" => {
                let name = payload
                    .as_str()
                    .ok_or_else(|| bad_json("checkpoint payload must be a string name"))?;
                let cur = cursor
                    .ok_or_else(|| bad_json("checkpoint requires an active upstream image"))?;
                if checkpoints.insert(name.to_string(), cur).is_some() {
                    return Err(bad_json(alloc::format!("duplicate checkpoint '{name}'")));
                }
            }
            "resume" => {
                let name = payload
                    .as_str()
                    .ok_or_else(|| bad_json("resume payload must be a checkpoint name"))?;
                let id = *checkpoints
                    .get(name)
                    .ok_or_else(|| bad_json(alloc::format!("unknown checkpoint '{name}'")))?;
                cursor = Some(id);
            }
            "encode" => {
                let src = cursor.ok_or_else(|| bad_json("encode requires an upstream image"))?;
                let op = parse_encode_node(src, payload)?;
                b.output(OutputOp::Encode(Box::new(op)))
                    .map_err(|e| bad_json(e.to_string()))?;
            }
            "composite" => {
                let canvas =
                    cursor.ok_or_else(|| bad_json("composite requires an active canvas"))?;
                let map = payload
                    .as_object()
                    .ok_or_else(|| bad_json("composite payload must be an object"))?;
                let fg_ref = map
                    .get("foreground")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        bad_json("composite in steps requires 'foreground' checkpoint name")
                    })?;
                let fg = *checkpoints.get(fg_ref).ok_or_else(|| {
                    bad_json(alloc::format!("unknown foreground checkpoint '{fg_ref}'"))
                })?;
                let op = parse_composite_node(canvas, fg, map)?;
                let id = b
                    .image(ImageOp::Composite(op))
                    .map_err(|e| bad_json(e.to_string()))?;
                cursor = Some(id);
            }
            "make_gain_map" => {
                let base = cursor.ok_or_else(|| bad_json("make_gain_map requires active base"))?;
                let map = payload
                    .as_object()
                    .ok_or_else(|| bad_json("make_gain_map payload must be an object"))?;
                let alt_ref = map
                    .get("alternate")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        bad_json("make_gain_map in steps requires 'alternate' checkpoint")
                    })?;
                let alt = *checkpoints.get(alt_ref).ok_or_else(|| {
                    bad_json(alloc::format!("unknown alternate checkpoint '{alt_ref}'"))
                })?;
                let op = parse_make_gain_map_node(base, alt, map)?;
                let id = b
                    .image(ImageOp::MakeGainMap(op))
                    .map_err(|e| bad_json(e.to_string()))?;
                cursor = Some(id);
            }
            other => {
                let op = parse_unary_or_root_node(other, cursor, payload)?;
                let id = b.image(op).map_err(|e| bad_json(e.to_string()))?;
                cursor = Some(id);
            }
        }
    }

    b.finish().map_err(|e| bad_json(e.to_string()))
}

fn parse_explicit_graph(val: &Value, limits: WireLimitConfig) -> PipeResult<Graph> {
    let obj = val
        .as_object()
        .ok_or_else(|| bad_json("'graph' must be an object with 'nodes' and 'edges'"))?;
    check_allowed_keys(obj, &["nodes", "edges"], "graph")?;
    let nodes_map = obj
        .get("nodes")
        .and_then(Value::as_object)
        .ok_or_else(|| bad_json("graph.nodes must be an object map"))?;
    let edges_arr = obj
        .get("edges")
        .and_then(Value::as_array)
        .ok_or_else(|| bad_json("graph.edges must be an array"))?;

    if nodes_map.len() > limits.max_nodes {
        return Err(at!(PipeError::LimitExceeded(
            "graph.nodes exceeds max_nodes".into()
        )));
    }

    // Collect incoming edges keyed by target node name -> (port -> from node name).
    let mut incoming: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut out_edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
    for k in nodes_map.keys() {
        in_degree.insert(k.clone(), 0);
    }

    for (ei, edge) in edges_arr.iter().enumerate() {
        let eo = edge
            .as_object()
            .ok_or_else(|| bad_json(alloc::format!("graph.edges[{ei}] must be an object")))?;
        check_allowed_keys(eo, &["from", "to", "port"], "graph edge")?;
        let from = eo
            .get("from")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_json("edge missing string 'from'"))?;
        let to = eo
            .get("to")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_json("edge missing string 'to'"))?;
        let port = match eo.get("port") {
            None => "input",
            Some(v) => v
                .as_str()
                .ok_or_else(|| bad_json("edge 'port' must be a string"))?,
        };
        if !nodes_map.contains_key(from) || !nodes_map.contains_key(to) {
            return Err(bad_json(alloc::format!(
                "edge '{from}' -> '{to}' references unknown node"
            )));
        }
        let ports = incoming.entry(to.to_string()).or_default();
        if ports.insert(port.to_string(), from.to_string()).is_some() {
            return Err(bad_json(alloc::format!(
                "duplicate incoming edge on node '{to}' port '{port}'"
            )));
        }
        out_edges
            .entry(from.to_string())
            .or_default()
            .push(to.to_string());
        *in_degree.get_mut(to).expect("checked") += 1;
    }

    // Deterministic Kahn topological sort (BTreeMap keys sorted lexicographically).
    let mut ready: Vec<String> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(k, _)| k.clone())
        .collect();
    let mut order: Vec<String> = Vec::with_capacity(nodes_map.len());
    while let Some(node_name) = ready.first().cloned() {
        ready.remove(0);
        order.push(node_name.clone());
        if let Some(nexts) = out_edges.get(&node_name) {
            for nxt in nexts {
                let d = in_degree.get_mut(nxt).expect("exists");
                *d -= 1;
                if *d == 0 {
                    ready.push(nxt.clone());
                    ready.sort();
                }
            }
        }
    }
    if order.len() != nodes_map.len() {
        return Err(bad_json(GraphError::CycleDetected.to_string()));
    }

    let mut b = GraphBuilder::new();
    let mut handles: BTreeMap<String, ImageId> = BTreeMap::new();

    for name in order {
        let node_val = &nodes_map[&name];
        let node_obj = node_val
            .as_object()
            .ok_or_else(|| bad_json(alloc::format!("node '{name}' must be a single-key object")))?;
        if node_obj.len() != 1 {
            return Err(bad_json(alloc::format!(
                "node '{name}' must have exactly 1 operation key"
            )));
        }
        let (op_key, payload) = node_obj.iter().next().expect("len == 1");
        let ports = incoming.get(&name);
        let check_exact_ports = |expected: &[&str]| -> PipeResult<()> {
            if let Some(m) = ports {
                for k in m.keys() {
                    if !expected.contains(&k.as_str()) {
                        return Err(bad_json(alloc::format!(
                            "unexpected incoming port '{k}' on node '{name}'"
                        )));
                    }
                }
            }
            Ok(())
        };
        let get_port = |p: &str| -> PipeResult<ImageId> {
            let from = ports.and_then(|m| m.get(p)).ok_or_else(|| {
                bad_json(alloc::format!("node '{name}' missing incoming port '{p}'"))
            })?;
            handles.get(from).copied().ok_or_else(|| {
                bad_json(alloc::format!("upstream node '{from}' is not image-valued"))
            })
        };

        match op_key.as_str() {
            "encode" => {
                check_exact_ports(&["input"])?;
                let src = get_port("input")?;
                let op = parse_encode_node(src, payload)?;
                b.output(OutputOp::Encode(Box::new(op)))
                    .map_err(|e| bad_json(e.to_string()))?;
            }
            "composite" => {
                check_exact_ports(&["canvas", "foreground"])?;
                let canvas = get_port("canvas")?;
                let fg = get_port("foreground")?;
                let map = payload
                    .as_object()
                    .ok_or_else(|| bad_json("composite payload must be an object"))?;
                let op = parse_composite_node(canvas, fg, map)?;
                let id = b
                    .image(ImageOp::Composite(op))
                    .map_err(|e| bad_json(e.to_string()))?;
                handles.insert(name, id);
            }
            "make_gain_map" => {
                check_exact_ports(&["base", "alternate"])?;
                let base = get_port("base")?;
                let alt = get_port("alternate")?;
                let map = payload
                    .as_object()
                    .ok_or_else(|| bad_json("make_gain_map payload must be an object"))?;
                let op = parse_make_gain_map_node(base, alt, map)?;
                let id = b
                    .image(ImageOp::MakeGainMap(op))
                    .map_err(|e| bad_json(e.to_string()))?;
                handles.insert(name, id);
            }
            "decode" | "canvas" => {
                check_exact_ports(&[])?;
                let op = parse_unary_or_root_node(op_key, None, payload)?;
                let id = b.image(op).map_err(|e| bad_json(e.to_string()))?;
                handles.insert(name, id);
            }
            other => {
                check_exact_ports(&["input"])?;
                let src = get_port("input")?;
                let op = parse_unary_or_root_node(other, Some(src), payload)?;
                let id = b.image(op).map_err(|e| bad_json(e.to_string()))?;
                handles.insert(name, id);
            }
        }
    }

    b.finish().map_err(|e| bad_json(e.to_string()))
}

fn parse_unary_or_root_node(
    op_key: &str,
    upstream: Option<ImageId>,
    payload: &Value,
) -> PipeResult<ImageOp> {
    let map = payload
        .as_object()
        .ok_or_else(|| bad_json(alloc::format!("'{op_key}' payload must be an object")))?;
    match op_key {
        "decode" => {
            check_allowed_keys(map, &["input", "orientation", "select"], "decode")?;
            let slot_str = map
                .get("input")
                .and_then(Value::as_str)
                .ok_or_else(|| bad_json("decode requires string 'input' slot"))?;
            let slot = SlotName::new(slot_str).map_err(|e| bad_json(e.to_string()))?;
            let orientation = match opt_str(map, "orientation", "decode")? {
                None | Some("apply") => DecodeOrientation::Apply,
                Some("keep_stored") => DecodeOrientation::KeepStored,
                Some("ignore") => DecodeOrientation::Ignore,
                Some(other) => {
                    return Err(bad_json(alloc::format!(
                        "invalid decode.orientation '{other}'"
                    )));
                }
            };
            #[cfg(feature = "media")]
            let select = match map.get("select") {
                None => None,
                Some(v) => Some(parse_decode_selection(v)?),
            };
            #[cfg(not(feature = "media"))]
            if map.contains_key("select") {
                return Err(bad_json(
                    "decode.select requires the 'media' feature to be enabled",
                ));
            }
            Ok(ImageOp::Decode(DecodeNodeOp {
                slot,
                orientation,
                #[cfg(feature = "media")]
                select,
                #[cfg(not(feature = "media"))]
                select_frame: None,
            }))
        }
        "canvas" => {
            check_allowed_keys(map, &["width", "height", "fill"], "canvas")?;
            let w = req_u32(map, "width", "canvas")?;
            let h = req_u32(map, "height", "canvas")?;
            let fill = parse_rgba_opt(map.get("fill"))?.unwrap_or([0, 0, 0, 255]);
            Ok(ImageOp::Canvas {
                size: Size::new(w, h),
                fill_rgba: fill,
            })
        }
        "color_ingest" => {
            let src =
                upstream.ok_or_else(|| bad_json("color_ingest requires an upstream image"))?;
            check_allowed_keys(
                map,
                &["mode", "target", "strict_invalid_icc"],
                "color_ingest",
            )?;
            let mode = match opt_str(map, "mode", "color_ingest")? {
                None | Some("keep_tagged") => ColorIngestMode::KeepTagged,
                Some("assign") => ColorIngestMode::Assign,
                Some("convert") => ColorIngestMode::Convert,
                Some(o) => return Err(bad_json(alloc::format!("invalid color_ingest.mode '{o}'"))),
            };
            let target = map.get("target").map(parse_color_space_name).transpose()?;
            let strict_invalid_icc =
                opt_bool(map, "strict_invalid_icc", "color_ingest")?.unwrap_or(true);
            Ok(ImageOp::ColorIngest(ColorIngestOp {
                src,
                mode,
                target,
                strict_invalid_icc,
            }))
        }
        "color_space" => {
            let src = upstream.ok_or_else(|| bad_json("color_space requires an upstream image"))?;
            check_allowed_keys(map, &["to"], "color_space")?;
            let to = map
                .get("to")
                .ok_or_else(|| bad_json("color_space requires 'to'"))
                .and_then(parse_pixel_format_name)?;
            Ok(ImageOp::ColorSpace(ColorSpaceOp { src, to }))
        }
        "tone_map" => {
            let src = upstream.ok_or_else(|| bad_json("tone_map requires an upstream image"))?;
            let render = parse_tone_map_record(map)?;
            Ok(ImageOp::ToneMap(ToneMapNodeOp { src, render }))
        }
        "gain_map" => {
            let src = upstream.ok_or_else(|| bad_json("gain_map requires an upstream image"))?;
            let action = parse_gain_map_action(map)?;
            Ok(ImageOp::GainMap(GainMapNodeOp { src, action }))
        }
        "geometry" => {
            let src = upstream.ok_or_else(|| bad_json("geometry requires an upstream image"))?;
            Ok(ImageOp::Geometry(parse_geometry_node(src, map)?))
        }
        "filter" => {
            let src = upstream.ok_or_else(|| bad_json("filter requires an upstream image"))?;
            Ok(ImageOp::Filter(parse_filter_node(src, map)?))
        }
        other => Err(bad_json(alloc::format!("unknown canonical node '{other}'"))),
    }
}

#[cfg(feature = "media")]
fn parse_decode_selection(val: &Value) -> PipeResult<DecodeSelection> {
    if let Some(s) = val.as_str() {
        return match s {
            "all" => Ok(DecodeSelection::All),
            other => Err(bad_json(alloc::format!("invalid decode.select '{other}'"))),
        };
    }
    let obj = val
        .as_object()
        .ok_or_else(|| bad_json("decode.select must be 'all' or a single-key object"))?;
    if obj.len() != 1 {
        return Err(bad_json("decode.select object must have exactly 1 key"));
    }
    let (k, v) = obj.iter().next().expect("len == 1");
    match k.as_str() {
        "presentation_index" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("presentation_index must be an object"))?;
            check_allowed_keys(m, &["track", "index"], "presentation_index")?;
            let track = m
                .get("track")
                .map(|t| {
                    t.as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| bad_json("track must be u32"))
                })
                .transpose()?;
            let index = m
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| bad_json("presentation_index requires u64 'index'"))?;
            Ok(DecodeSelection::PresentationIndex { track, index })
        }
        "presentation_time" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("presentation_time must be an object"))?;
            check_allowed_keys(m, &["track", "time", "mode"], "presentation_time")?;
            let track = m
                .get("track")
                .map(|t| {
                    t.as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| bad_json("track must be u32"))
                })
                .transpose()?;
            let time = parse_rational_timestamp(
                m.get("time")
                    .ok_or_else(|| bad_json("presentation_time requires 'time'"))?,
            )?;
            let mode = match opt_str(m, "mode", "presentation_time")? {
                None | Some("at_or_before") => SeekMode::AtOrBefore,
                Some("at_or_after") => SeekMode::AtOrAfter,
                Some("nearest") => SeekMode::Nearest,
                Some(o) => return Err(bad_json(alloc::format!("invalid seek mode '{o}'"))),
            };
            Ok(DecodeSelection::PresentationTime { track, time, mode })
        }
        "page" => {
            let p = v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| bad_json("page must be u32"))?;
            Ok(DecodeSelection::Page(p))
        }
        other => Err(bad_json(alloc::format!(
            "unknown decode.select key '{other}'"
        ))),
    }
}

#[cfg(feature = "media")]
fn parse_rational_timestamp(val: &Value) -> PipeResult<Timestamp> {
    let m = val
        .as_object()
        .ok_or_else(|| bad_json("timestamp must be an object with 'ticks' and 'base'"))?;
    check_allowed_keys(m, &["ticks", "base"], "timestamp")?;
    let ticks = match m.get("ticks") {
        Some(Value::String(s)) => s
            .parse::<i64>()
            .map_err(|_| bad_json("timestamp.ticks must be a signed decimal integer"))?,
        Some(Value::Number(n)) => n
            .as_i64()
            .ok_or_else(|| bad_json("timestamp.ticks must be i64"))?,
        _ => return Err(bad_json("timestamp requires 'ticks'")),
    };
    let base_m = m
        .get("base")
        .and_then(Value::as_object)
        .ok_or_else(|| bad_json("timestamp.base must be an object"))?;
    check_allowed_keys(base_m, &["numerator", "denominator"], "timestamp.base")?;
    let num = parse_u32_or_str(base_m.get("numerator"), "base.numerator")?;
    let den = parse_u32_or_str(base_m.get("denominator"), "base.denominator")?;
    let tb = TimeBase::new(num, den).map_err(|_| bad_json("invalid rational TimeBase"))?;
    Ok(Timestamp::new(ticks, tb))
}

fn parse_rational_duration(val: &Value) -> PipeResult<FrameDuration> {
    let m = val
        .as_object()
        .ok_or_else(|| bad_json("duration must be {numerator, denominator}"))?;
    check_allowed_keys(m, &["numerator", "denominator"], "duration")?;
    let num = parse_u64_or_str(m.get("numerator"), "duration.numerator")?;
    let den = parse_u32_or_str(m.get("denominator"), "duration.denominator")?;
    FrameDuration::new(num, den).map_err(|_| bad_json("invalid rational FrameDuration"))
}

fn parse_geometry_node(
    src: ImageId,
    map: &serde_json::Map<String, Value>,
) -> PipeResult<GeometryOp> {
    if map.len() != 1 {
        return Err(bad_json(
            "geometry payload must be a single-key tagged object",
        ));
    }
    let (k, v) = map.iter().next().expect("len == 1");
    match k.as_str() {
        "orient" => {
            let name = v
                .as_str()
                .ok_or_else(|| bad_json("geometry.orient must be a D4 orientation string"))?;
            let orientation = match name {
                "identity" => Orientation::Identity,
                "flip_h" => Orientation::FlipH,
                "rotate180" => Orientation::Rotate180,
                "flip_v" => Orientation::FlipV,
                "transpose" => Orientation::Transpose,
                "rotate90" => Orientation::Rotate90,
                "transverse" => Orientation::Transverse,
                "rotate270" => Orientation::Rotate270,
                other => return Err(bad_json(alloc::format!("unknown orientation '{other}'"))),
            };
            Ok(GeometryOp::Orient { src, orientation })
        }
        "crop" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("geometry.crop must be an object"))?;
            check_allowed_keys(m, &["x", "y", "width", "height"], "geometry.crop")?;
            let rect = Rect::new(
                req_u32(m, "x", "crop")?,
                req_u32(m, "y", "crop")?,
                req_u32(m, "width", "crop")?,
                req_u32(m, "height", "crop")?,
            );
            Ok(GeometryOp::Crop { src, rect })
        }
        "resize" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("geometry.resize must be an object"))?;
            check_allowed_keys(
                m,
                &["width", "height", "kernel", "linear_light"],
                "geometry.resize",
            )?;
            let width = opt_u32(m, "width", "resize")?;
            let height = opt_u32(m, "height", "resize")?;
            let kernel = parse_kernel_opt(m.get("kernel"))?;
            let linear_light = opt_bool(m, "linear_light", "geometry.resize")?.unwrap_or(true);
            Ok(GeometryOp::Resize {
                src,
                width,
                height,
                kernel,
                linear_light,
            })
        }
        "constrain" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("geometry.constrain must be an object"))?;
            check_allowed_keys(
                m,
                &[
                    "mode",
                    "width",
                    "height",
                    "kernel",
                    "linear_light",
                    "canvas_color",
                ],
                "geometry.constrain",
            )?;
            let mode = match opt_str(m, "mode", "geometry.constrain")? {
                None | Some("within") => ConstraintMode::Within,
                Some("fit") => ConstraintMode::Fit,
                Some("fit_crop") => ConstraintMode::FitCrop,
                Some("within_crop") => ConstraintMode::WithinCrop,
                Some("aspect_crop") => ConstraintMode::AspectCrop,
                Some("fit_pad") => ConstraintMode::FitPad,
                Some("within_pad") => ConstraintMode::WithinPad,
                Some("pad_within") => ConstraintMode::PadWithin,
                Some("distort") => ConstraintMode::Distort,
                Some(o) => return Err(bad_json(alloc::format!("unknown constrain mode '{o}'"))),
            };
            let w = opt_u32(m, "width", "constrain")?;
            let h = opt_u32(m, "height", "constrain")?;
            let mut c = match (w, h) {
                (Some(w), Some(h)) => Constraint::new(mode, w, h),
                (Some(w), None) => Constraint::width_only(mode, w),
                (None, Some(h)) => Constraint::height_only(mode, h),
                (None, None) => {
                    return Err(bad_json("geometry.constrain requires width and/or height"));
                }
            };
            if let Some([r, g, b, a]) = parse_rgba_opt(m.get("canvas_color"))? {
                c = c.canvas_color(zenlayout::CanvasColor::Srgb { r, g, b, a });
            }
            let kernel = parse_kernel_opt(m.get("kernel"))?;
            let linear_light = opt_bool(m, "linear_light", "geometry.constrain")?.unwrap_or(true);
            Ok(GeometryOp::Constrain {
                src,
                constraint: c,
                kernel,
                linear_light,
            })
        }
        "canvas" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("geometry.canvas must be an object"))?;
            check_allowed_keys(
                m,
                &["width", "height", "offset_x", "offset_y", "fill"],
                "geometry.canvas",
            )?;
            let canvas = Size::new(
                req_u32(m, "width", "canvas")?,
                req_u32(m, "height", "canvas")?,
            );
            let offset_x = opt_i32(m, "offset_x", "geometry.canvas")?.unwrap_or(0);
            let offset_y = opt_i32(m, "offset_y", "geometry.canvas")?.unwrap_or(0);
            let fill_rgba = parse_rgba_opt(m.get("fill"))?.unwrap_or([0, 0, 0, 255]);
            Ok(GeometryOp::ExtendCanvas {
                src,
                canvas,
                offset_x,
                offset_y,
                fill_rgba,
            })
        }
        other => Err(bad_json(alloc::format!("unknown geometry op '{other}'"))),
    }
}

fn parse_filter_node(
    src: ImageId,
    map: &serde_json::Map<String, Value>,
) -> PipeResult<FilterNodeOp> {
    if map.len() != 1 {
        return Err(bad_json(
            "filter payload must be a single-key tagged object",
        ));
    }
    let (k, v) = map.iter().next().expect("len == 1");
    match k.as_str() {
        "exposure" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("filter.exposure must be an object"))?;
            check_allowed_keys(m, &["stops"], "filter.exposure")?;
            Ok(FilterNodeOp::Exposure {
                src,
                stops: req_f32(m, "stops", "exposure")?,
            })
        }
        "contrast" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("filter.contrast must be an object"))?;
            check_allowed_keys(m, &["amount"], "filter.contrast")?;
            Ok(FilterNodeOp::Contrast {
                src,
                amount: req_f32(m, "amount", "contrast")?,
            })
        }
        "saturation" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("filter.saturation must be an object"))?;
            check_allowed_keys(m, &["amount"], "filter.saturation")?;
            Ok(FilterNodeOp::Saturation {
                src,
                amount: req_f32(m, "amount", "saturation")?,
            })
        }
        "vibrance" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("filter.vibrance must be an object"))?;
            check_allowed_keys(m, &["amount"], "filter.vibrance")?;
            Ok(FilterNodeOp::Vibrance {
                src,
                amount: req_f32(m, "amount", "vibrance")?,
            })
        }
        "blur" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("filter.blur must be an object"))?;
            check_allowed_keys(m, &["sigma"], "filter.blur")?;
            Ok(FilterNodeOp::Blur {
                src,
                sigma: req_f32(m, "sigma", "blur")?,
            })
        }
        "sharpen" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("filter.sharpen must be an object"))?;
            check_allowed_keys(m, &["amount", "radius"], "filter.sharpen")?;
            Ok(FilterNodeOp::Sharpen {
                src,
                amount: req_f32(m, "amount", "sharpen")?,
                radius: opt_f32(m, "radius", "filter.sharpen")?.unwrap_or(1.0),
            })
        }
        "grayscale" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("filter.grayscale must be an empty object"))?;
            check_allowed_keys(m, &[], "filter.grayscale")?;
            Ok(FilterNodeOp::Grayscale { src })
        }
        "sepia" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("filter.sepia must be an object"))?;
            check_allowed_keys(m, &["amount"], "filter.sepia")?;
            let amount = opt_f32(m, "amount", "filter.sepia")?.unwrap_or(1.0);
            Ok(FilterNodeOp::Sepia { src, amount })
        }
        other => Err(bad_json(alloc::format!("unknown filter op '{other}'"))),
    }
}

fn parse_composite_node(
    canvas: ImageId,
    foreground: ImageId,
    map: &serde_json::Map<String, Value>,
) -> PipeResult<CompositeOp> {
    check_allowed_keys(
        map,
        &["foreground", "x", "y", "opacity", "blend"],
        "composite",
    )?;
    let x = opt_i32(map, "x", "composite")?.unwrap_or(0);
    let y = opt_i32(map, "y", "composite")?.unwrap_or(0);
    let opacity = opt_f32(map, "opacity", "composite")?.unwrap_or(1.0);
    let blend = match opt_str(map, "blend", "composite")? {
        None | Some("source_over") => BlendKind::SourceOver,
        Some("copy") => BlendKind::Copy,
        Some("multiply") => BlendKind::Multiply,
        Some("screen") => BlendKind::Screen,
        Some(o) => return Err(bad_json(alloc::format!("unknown composite.blend '{o}'"))),
    };
    Ok(CompositeOp {
        canvas,
        foreground,
        x,
        y,
        opacity,
        blend,
    })
}

fn parse_make_gain_map_node(
    base: ImageId,
    alternate: ImageId,
    map: &serde_json::Map<String, Value>,
) -> PipeResult<MakeGainMapOp> {
    check_allowed_keys(map, &["alternate", "primaries", "white"], "make_gain_map")?;
    let primaries = match opt_str(map, "primaries", "make_gain_map")? {
        None | Some("bt709") | Some("srgb") => Primaries::Bt709,
        Some("display_p3") => Primaries::DisplayP3,
        Some("bt2020") => Primaries::Bt2020,
        Some(o) => return Err(bad_json(alloc::format!("unknown primaries '{o}'"))),
    };
    let white = match opt_f32(map, "white", "make_gain_map")? {
        None => DiffuseWhite::BT2408,
        Some(n) => DiffuseWhite::new(n)
            .ok_or_else(|| bad_json("make_gain_map.white must be positive finite nits"))?,
    };
    Ok(MakeGainMapOp {
        base,
        alternate,
        primaries,
        white,
    })
}

fn parse_encode_node(src: ImageId, payload: &Value) -> PipeResult<EncodeNodeOp> {
    let map = payload
        .as_object()
        .ok_or_else(|| bad_json("encode payload must be an object"))?;
    check_allowed_keys(
        map,
        &[
            "output",
            "format",
            "allow",
            "video_codec",
            "coding",
            "effort",
            "chroma",
            "alpha",
            "orientation",
            "color",
            "tone_map",
            "gain_map",
            "metadata",
            "sequence",
            "tracks",
            "publication",
            "codec_options",
        ],
        "encode",
    )?;
    let slot_str = map
        .get("output")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_json("encode requires string 'output' slot"))?;
    let slot = SlotName::new(slot_str).map_err(|e| bad_json(e.to_string()))?;

    let fmt_str = map
        .get("format")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_json("encode requires string 'format'"))?;

    let target = match fmt_str {
        "keep" => {
            if map.contains_key("video_codec") {
                return Err(bad_json("'video_codec' cannot be used with format='keep'"));
            }
            TargetFormat::Keep
        }
        "auto" => {
            let allow = map
                .get("allow")
                .and_then(Value::as_array)
                .ok_or_else(|| bad_json("format='auto' requires non-empty 'allow' list"))?;
            let mut fmts = Vec::with_capacity(allow.len());
            for a in allow {
                let s = a
                    .as_str()
                    .ok_or_else(|| bad_json("encode.allow entries must be format strings"))?;
                fmts.push(parse_image_format_name(s)?);
            }
            TargetFormat::Auto(
                FormatPreferences::ordered(fmts).map_err(|e| bad_json(e.error().message()))?,
            )
        }
        #[cfg(feature = "media")]
        "mp4" | "webm" | "ivf" => {
            let container = match fmt_str {
                "mp4" => ContainerId::Mp4,
                "webm" => ContainerId::Webm,
                "ivf" => ContainerId::Ivf,
                _ => unreachable!(),
            };
            let vc = map
                .get("video_codec")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    bad_json(alloc::format!(
                        "format='{fmt_str}' requires semantic 'video_codec' ('av1' | 'vp8' | 'vp9')"
                    ))
                })?;
            let codec = match vc {
                "av1" => MediaCodecId::Av1,
                "vp8" => MediaCodecId::Vp8,
                "vp9" => MediaCodecId::Vp9,
                other => {
                    return Err(bad_json(alloc::format!(
                        "unsupported video_codec '{other}'"
                    )));
                }
            };
            TargetFormat::Video { container, codec }
        }
        other => {
            if map.contains_key("video_codec") {
                return Err(bad_json(
                    "'video_codec' is only valid for video container formats ('mp4', 'webm', 'ivf')",
                ));
            }
            TargetFormat::Exact(parse_image_format_name(other)?)
        }
    };

    let mut intent = OutputIntent::new(target);

    if let Some(coding_val) = map.get("coding") {
        intent = intent.with_coding(parse_coding(coding_val)?);
    }
    if let Some(eff_val) = opt_str(map, "effort", "encode")? {
        intent = intent.with_effort(match eff_val {
            "fast" => Effort::Fast,
            "balanced" => Effort::Balanced,
            "thorough" => Effort::Thorough,
            o => return Err(bad_json(alloc::format!("invalid effort '{o}'"))),
        });
    }
    if let Some(ch_val) = map.get("chroma") {
        intent = intent.with_chroma(parse_chroma_policy(ch_val)?);
    }
    if let Some(a_val) = map.get("alpha") {
        intent = intent.with_alpha(parse_alpha_policy(a_val)?);
    }
    if let Some(o_val) = opt_str(map, "orientation", "encode")? {
        intent = intent.with_orientation(match o_val {
            "bake" => OrientationMode::Bake,
            "preserve_stored" => OrientationMode::PreserveStored,
            o => return Err(bad_json(alloc::format!("invalid encode.orientation '{o}'"))),
        });
    }
    if let Some(c_val) = map.get("color") {
        if c_val.as_str() == Some("preserve") {
            intent = intent.with_color(ColorTarget::Preserve);
        } else {
            intent = intent.with_color(ColorTarget::ConvertTo(parse_pixel_format_name(c_val)?));
        }
    }
    if let Some(tm_val) = map.get("tone_map") {
        if tm_val.as_str() == Some("require_compatible") {
            intent = intent.with_tone_map(ToneMapPolicy::RequireCompatible);
        } else if let Some(tm_obj) = tm_val.as_object() {
            check_allowed_keys(tm_obj, &["map_to_sdr"], "encode.tone_map")?;
            if let Some(inner) = tm_obj.get("map_to_sdr").and_then(Value::as_object) {
                intent =
                    intent.with_tone_map(ToneMapPolicy::MapToSdr(parse_tone_map_record(inner)?));
            } else {
                return Err(bad_json(
                    "encode.tone_map must be 'require_compatible' or {'map_to_sdr': {...}}",
                ));
            }
        } else {
            return Err(bad_json("invalid encode.tone_map"));
        }
    }
    if let Some(gm_val) = opt_str(map, "gain_map", "encode")? {
        intent = intent.with_gain_map(match gm_val {
            "preserve" => GainMapEncodePolicy::Preserve,
            "require" => GainMapEncodePolicy::Require,
            "drop" => GainMapEncodePolicy::Drop,
            o => return Err(bad_json(alloc::format!("invalid encode.gain_map '{o}'"))),
        });
    }
    if let Some(m_val) = opt_str(map, "metadata", "encode")? {
        intent = intent.with_metadata(match m_val {
            "web" => MetadataPolicy::Web,
            "preserve" => MetadataPolicy::Preserve,
            "preserve_bytes" => MetadataPolicy::PreserveBytes,
            "color_and_rotation" => MetadataPolicy::ColorAndRotation,
            "strip_all" => MetadataPolicy::StripAll,
            o => return Err(bad_json(alloc::format!("invalid encode.metadata '{o}'"))),
        });
    }
    if let Some(seq_val) = map.get("sequence") {
        intent = intent.with_sequence(parse_sequence_policy(seq_val)?);
    }
    if let Some(tr_val) = map.get("tracks") {
        intent = intent.with_tracks(parse_track_policy(tr_val)?);
    }
    if let Some(pub_val) = opt_str(map, "publication", "encode")? {
        intent = intent.with_publication(match pub_val {
            "verified" => PublicationMode::Verified,
            "incremental" => PublicationMode::Incremental,
            o => return Err(bad_json(alloc::format!("invalid encode.publication '{o}'"))),
        });
    }
    if let Some(opts_val) = map.get("codec_options") {
        intent = intent.with_codec_options(parse_codec_options(opts_val)?);
    }

    intent
        .validate()
        .map_err(|e| bad_json(e.error().message()))?;
    Ok(EncodeNodeOp { src, slot, intent })
}

fn parse_coding(val: &Value) -> PipeResult<Coding> {
    if let Some(s) = val.as_str() {
        return match s {
            "auto" => Ok(Coding::Auto),
            "lossless" => Ok(Coding::Lossless),
            "lossless_or_keep" => Ok(Coding::LosslessOrKeep),
            "keep_bytes" => Ok(Coding::KeepBytes),
            "exact_jpeg_round_trip" => Ok(Coding::ExactJpegRoundTrip),
            other => Err(bad_json(alloc::format!("unknown coding '{other}'"))),
        };
    }
    let obj = val
        .as_object()
        .ok_or_else(|| bad_json("coding must be a string or single-key object"))?;
    if obj.len() != 1 {
        return Err(bad_json("coding object must have exactly 1 key"));
    }
    let (k, v) = obj.iter().next().expect("len == 1");
    match k.as_str() {
        "calibrated_quality" => {
            let q = v
                .as_f64()
                .ok_or_else(|| bad_json("calibrated_quality must be a number"))?
                as f32;
            Ok(Coding::CalibratedQuality(q))
        }
        "quality_profile" => {
            let m = v
                .as_object()
                .ok_or_else(|| bad_json("quality_profile must be an object"))?;
            check_allowed_keys(m, &["level", "dpr"], "quality_profile")?;
            let level = match opt_str(m, "level", "quality_profile")? {
                Some("low") => QualityLevel::Low,
                None | Some("good") => QualityLevel::Good,
                Some("high") => QualityLevel::High,
                Some("visually_lossless") => QualityLevel::VisuallyLossless,
                Some(o) => return Err(bad_json(alloc::format!("unknown quality level '{o}'"))),
            };
            let dpr = opt_f32(m, "dpr", "quality_profile")?.unwrap_or(1.0);
            Ok(Coding::QualityProfile { level, dpr })
        }
        "aim_for" => Ok(Coding::AimFor(parse_metric_target(v)?)),
        "verify" => Ok(Coding::Verify(parse_metric_target(v)?)),
        other => Err(bad_json(alloc::format!("unknown coding key '{other}'"))),
    }
}

fn parse_metric_target(val: &Value) -> PipeResult<MetricTarget> {
    let m = val
        .as_object()
        .ok_or_else(|| bad_json("metric target must be an object"))?;
    check_allowed_keys(m, &["metric", "revision", "value"], "metric target")?;
    let metric = match m.get("metric").and_then(Value::as_str) {
        Some("ssimulacra2") => Metric::Ssimulacra2,
        Some("butteraugli_max") => Metric::ButteraugliMax,
        Some("zensim_a") => Metric::ZensimA,
        Some("zensim_b") => Metric::ZensimB,
        _ => return Err(bad_json("invalid or missing metric name")),
    };
    let value = req_f32(m, "value", "metric target")?;
    let mut target = MetricTarget::new(metric, value).map_err(|e| bad_json(e.error().message()))?;
    if let Some(rev) = opt_str(m, "revision", "metric target")? {
        target = target.with_revision(rev);
    }
    Ok(target)
}

fn parse_sequence_policy(val: &Value) -> PipeResult<SequencePolicy> {
    if val.as_str() == Some("first_presentation") {
        return Ok(SequencePolicy::FirstPresentation);
    }
    let obj = val
        .as_object()
        .ok_or_else(|| bad_json("sequence must be 'first_presentation' or {'preserve': {...}}"))?;
    check_allowed_keys(obj, &["preserve"], "sequence")?;
    let p = obj
        .get("preserve")
        .and_then(Value::as_object)
        .ok_or_else(|| bad_json("sequence.preserve must be an object"))?;
    check_allowed_keys(p, &["timing", "loops"], "sequence.preserve")?;

    let timing = match p.get("timing") {
        None => TimingPolicy::Exact,
        Some(Value::String(s)) if s == "exact" => TimingPolicy::Exact,
        Some(Value::Object(tm)) => {
            check_allowed_keys(tm, &["quantize_endpoints"], "timing")?;
            let qe = tm
                .get("quantize_endpoints")
                .and_then(Value::as_object)
                .ok_or_else(|| bad_json("quantize_endpoints must be an object"))?;
            check_allowed_keys(qe, &["max_boundary_error"], "quantize_endpoints")?;
            let err = parse_rational_duration(
                qe.get("max_boundary_error")
                    .ok_or_else(|| bad_json("quantize_endpoints requires max_boundary_error"))?,
            )?;
            TimingPolicy::QuantizeEndpoints {
                max_boundary_error: err,
            }
        }
        _ => return Err(bad_json("invalid sequence.preserve.timing")),
    };

    let loops = match p.get("loops") {
        None => LoopPolicy::Preserve,
        Some(Value::String(s)) if s == "preserve" => LoopPolicy::Preserve,
        Some(Value::Object(lm)) => {
            check_allowed_keys(lm, &["set"], "loops")?;
            let set_val = lm
                .get("set")
                .ok_or_else(|| bad_json("loops object requires 'set'"))?;
            let lc = match set_val {
                Value::String(s) if s == "unspecified" => LoopCount::Unspecified,
                Value::String(s) if s == "infinite" => LoopCount::Infinite,
                Value::Object(pm) => {
                    check_allowed_keys(pm, &["plays"], "loops.set")?;
                    let plays = req_u32(pm, "plays", "loops.set")?;
                    let nz = NonZeroU32::new(plays)
                        .ok_or_else(|| bad_json("loops.set.plays must be >= 1"))?;
                    LoopCount::Plays(nz)
                }
                _ => return Err(bad_json("invalid loops.set value")),
            };
            LoopPolicy::Set(lc)
        }
        _ => return Err(bad_json("invalid sequence.preserve.loops")),
    };

    Ok(SequencePolicy::Preserve { timing, loops })
}

fn parse_track_policy(val: &Value) -> PipeResult<TrackPolicy> {
    if let Some(s) = val.as_str() {
        return match s {
            "preserve" => Ok(TrackPolicy::Preserve),
            "drop_non_video" => Ok(TrackPolicy::DropNonVideo),
            other => Err(bad_json(alloc::format!("invalid tracks policy '{other}'"))),
        };
    }
    let obj = val.as_object().ok_or_else(|| {
        bad_json("tracks must be 'preserve', 'drop_non_video', or {'select': {...}}")
    })?;
    check_allowed_keys(obj, &["select"], "tracks")?;
    let sel = obj
        .get("select")
        .and_then(Value::as_object)
        .ok_or_else(|| bad_json("tracks.select must be an object"))?;
    check_allowed_keys(sel, &["video", "copy"], "tracks.select")?;
    let video = req_u32(sel, "video", "tracks.select")?;
    let mut copy = Vec::new();
    if let Some(copy_val) = sel.get("copy") {
        let arr = copy_val
            .as_array()
            .ok_or_else(|| bad_json("tracks.select.copy must be an array of u32 indices"))?;
        for c in arr {
            let idx = c
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| bad_json("tracks.select.copy must contain u32 indices"))?;
            copy.push(idx);
        }
    }
    Ok(TrackPolicy::Select { video, copy })
}

fn parse_codec_options(val: &Value) -> PipeResult<CodecOptions> {
    let obj = val
        .as_object()
        .ok_or_else(|| bad_json("codec_options must be an object keyed by versioned namespace"))?;
    let mut opts = CodecOptions::new();
    for (ns, ns_val) in obj {
        if !ns.contains('/') {
            return Err(bad_json(alloc::format!(
                "codec_options namespace '{ns}' must be versioned (e.g. 'org.imazen.jpeg/v1')"
            )));
        }
        let ns_obj = ns_val
            .as_object()
            .ok_or_else(|| bad_json("codec_options namespace value must be an object"))?;
        check_allowed_keys(ns_obj, &["mode", "values"], "codec_options namespace")?;
        if let Some(mode_s) = opt_str(ns_obj, "mode", "codec_options")? {
            opts.mode = match mode_s {
                "advisory" => OptionMode::Advisory,
                "strict" => OptionMode::Strict,
                o => return Err(bad_json(alloc::format!("invalid codec_options mode '{o}'"))),
            };
        }
        if let Some(vals_v) = ns_obj.get("values") {
            let vals = vals_v
                .as_object()
                .ok_or_else(|| bad_json("codec_options.values must be an object"))?;
            for (k, v) in vals {
                let ov = match v {
                    Value::Bool(b) => OptionValue::Bool(*b),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            OptionValue::Int(i)
                        } else if let Some(f) = n.as_f64() {
                            if !f.is_finite() {
                                return Err(bad_json("codec_options float must be finite"));
                            }
                            OptionValue::Float(f)
                        } else {
                            return Err(bad_json("unsupported number in codec_options"));
                        }
                    }
                    Value::String(s) => OptionValue::Text(s.clone()),
                    _ => return Err(bad_json("codec_options values must be scalar")),
                };
                opts = opts.set(ns.clone(), k.clone(), ov);
            }
        }
    }
    Ok(opts)
}

fn parse_chroma_policy(val: &Value) -> PipeResult<ChromaPolicy> {
    if let Some(s) = val.as_str() {
        return match s {
            "auto" => Ok(ChromaPolicy::Auto),
            "preserve" => Ok(ChromaPolicy::Preserve),
            o => Err(bad_json(alloc::format!("invalid chroma '{o}'"))),
        };
    }
    let m = val
        .as_object()
        .ok_or_else(|| bad_json("chroma must be string or {'require': ...}"))?;
    check_allowed_keys(m, &["require"], "chroma")?;
    let s = m
        .get("require")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_json("chroma.require must be string"))?;
    let cs = match s {
        "444" => ChromaSampling::Cs444,
        "422" => ChromaSampling::Cs422,
        "420" => ChromaSampling::Cs420,
        o => return Err(bad_json(alloc::format!("invalid chroma sampling '{o}'"))),
    };
    Ok(ChromaPolicy::Require(cs))
}

fn parse_alpha_policy(val: &Value) -> PipeResult<AlphaPolicy> {
    if let Some(s) = val.as_str() {
        return match s {
            "preserve" => Ok(AlphaPolicy::Preserve),
            "require_opaque" => Ok(AlphaPolicy::RequireOpaque),
            o => Err(bad_json(alloc::format!("invalid alpha '{o}'"))),
        };
    }
    let m = val
        .as_object()
        .ok_or_else(|| bad_json("alpha must be string or {'flatten_srgb': [r,g,b]}"))?;
    check_allowed_keys(m, &["flatten_srgb"], "alpha")?;
    let arr = m
        .get("flatten_srgb")
        .and_then(Value::as_array)
        .ok_or_else(|| bad_json("flatten_srgb must be [r, g, b]"))?;
    if arr.len() != 3 {
        return Err(bad_json("flatten_srgb requires 3 u8 values"));
    }
    let mut rgb = [0u8; 3];
    for (i, v) in arr.iter().enumerate() {
        rgb[i] = v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| bad_json("flatten_srgb elements must be u8"))?;
    }
    Ok(AlphaPolicy::FlattenSrgb(rgb))
}

fn parse_tone_map_record(m: &serde_json::Map<String, Value>) -> PipeResult<ToneMap> {
    check_allowed_keys(
        m,
        &[
            "method",
            "target",
            "source_peak",
            "white",
            "target_peak",
            "gamut",
        ],
        "tone_map",
    )?;
    let method = match opt_str(m, "method", "tone_map")? {
        None | Some("bt2408") => ToneMapMethod::Bt2408,
        Some("bt2446_a") => ToneMapMethod::Bt2446A,
        Some("bt2446_b") => ToneMapMethod::Bt2446B,
        Some("bt2446_c") => ToneMapMethod::Bt2446C,
        Some(o) => return Err(bad_json(alloc::format!("unknown tone_map method '{o}'"))),
    };
    let source_peak = match m.get("source_peak") {
        None => PeakSource::Declared,
        Some(Value::String(s)) if s == "declared" => PeakSource::Declared,
        Some(Value::String(s)) if s == "measured" => PeakSource::Measured,
        Some(Value::Object(pm)) => {
            check_allowed_keys(pm, &["fixed"], "source_peak")?;
            let n = req_f32(pm, "fixed", "source_peak")?;
            PeakSource::Fixed(
                Nits::new_positive(n).ok_or_else(|| bad_json("source_peak.fixed must be > 0"))?,
            )
        }
        _ => return Err(bad_json("invalid tone_map.source_peak")),
    };
    let white = match opt_f32(m, "white", "tone_map")? {
        None => DiffuseWhite::BT2408,
        Some(w) => DiffuseWhite::new(w).ok_or_else(|| bad_json("invalid white nits"))?,
    };
    let target_peak = match opt_f32(m, "target_peak", "tone_map")? {
        None => Nits::new_positive(white.nits()).expect("positive"),
        Some(p) => Nits::new_positive(p).ok_or_else(|| bad_json("invalid target_peak nits"))?,
    };
    let mut tm = ToneMap::sdr(method, source_peak, white, target_peak)
        .map_err(|e| bad_json(e.error().message()))?;
    if let Some(t) = opt_str(m, "target", "tone_map")? {
        let p = match t {
            "bt709" | "srgb" => Primaries::Bt709,
            "display_p3" => Primaries::DisplayP3,
            "bt2020" => Primaries::Bt2020,
            o => return Err(bad_json(alloc::format!("unknown tone_map target '{o}'"))),
        };
        tm = tm.with_target_primaries(p);
    }
    if let Some(g) = opt_str(m, "gamut", "tone_map")? {
        tm = tm.with_gamut(match g {
            "preserve" => GamutMode::Preserve,
            "clip" => GamutMode::Clip,
            "compress" => GamutMode::Compress,
            o => return Err(bad_json(alloc::format!("unknown gamut '{o}'"))),
        });
    }
    Ok(tm)
}

fn parse_gain_map_action(m: &serde_json::Map<String, Value>) -> PipeResult<GainMapAction> {
    if m.len() != 1 {
        return Err(bad_json("gain_map must be a single-key object"));
    }
    let (k, v) = m.iter().next().expect("len == 1");
    match k.as_str() {
        "base_only" => {
            let bm = v
                .as_object()
                .ok_or_else(|| bad_json("gain_map.base_only must be an empty object"))?;
            check_allowed_keys(bm, &[], "gain_map.base_only")?;
            Ok(GainMapAction::BaseOnly)
        }
        "reconstruct" => {
            let rm = v
                .as_object()
                .ok_or_else(|| bad_json("reconstruct must be object"))?;
            check_allowed_keys(rm, &["rendition", "headroom_stops"], "gain_map.reconstruct")?;
            let stops_opt = opt_f32(rm, "headroom_stops", "gain_map.reconstruct")?;
            let rend_opt = opt_str(rm, "rendition", "gain_map.reconstruct")?;
            if stops_opt.is_some() && rend_opt.is_some() {
                return Err(bad_json(
                    "gain_map.reconstruct cannot specify both 'headroom_stops' and 'rendition'",
                ));
            }
            let rendition = if let Some(s) = stops_opt {
                if !s.is_finite() || s < 0.0 {
                    return Err(bad_json("headroom_stops must be finite >= 0"));
                }
                GainMapRendition::DisplayHeadroomStops(s)
            } else {
                match rend_opt {
                    Some("base") => GainMapRendition::Base,
                    None | Some("alternate") | Some("hdr") => GainMapRendition::Alternate,
                    Some(o) => return Err(bad_json(alloc::format!("unknown rendition '{o}'"))),
                }
            };
            Ok(GainMapAction::Reconstruct { rendition })
        }
        "carry" => {
            let cm = v
                .as_object()
                .ok_or_else(|| bad_json("gain_map.carry must be an object"))?;
            check_allowed_keys(cm, &["fidelity"], "gain_map.carry")?;
            let fidelity = match opt_str(cm, "fidelity", "gain_map.carry")? {
                None | Some("exact_registration") => GainMapFidelity::ExactRegistration,
                Some("resample") => GainMapFidelity::Resample,
                Some(o) => return Err(bad_json(alloc::format!("unknown gain_map fidelity '{o}'"))),
            };
            Ok(GainMapAction::Carry { fidelity })
        }
        other => Err(bad_json(alloc::format!(
            "unknown gain_map action '{other}'"
        ))),
    }
}

fn parse_image_format_name(s: &str) -> PipeResult<ImageFormat> {
    match s {
        "jpeg" | "jpg" => Ok(ImageFormat::Jpeg),
        "png" => Ok(ImageFormat::Png),
        "webp" => Ok(ImageFormat::WebP),
        "gif" => Ok(ImageFormat::Gif),
        "avif" => Ok(ImageFormat::Avif),
        "jxl" => Ok(ImageFormat::Jxl),
        "bmp" => Ok(ImageFormat::Bmp),
        "tiff" | "tif" => Ok(ImageFormat::Tiff),
        "heic" => Ok(ImageFormat::Heic),
        other => Err(bad_json(alloc::format!("unknown image format '{other}'"))),
    }
}

fn parse_color_space_name(val: &Value) -> PipeResult<ColorSpace> {
    match val.as_str() {
        Some("srgb") => Ok(ColorSpace::SRGB),
        Some("display_p3") => Ok(ColorSpace::DISPLAY_P3),
        Some("bt2020_pq") | Some("bt2100_pq") => Ok(ColorSpace::BT2100_PQ),
        Some("bt2020_hlg") | Some("bt2100_hlg") => Ok(ColorSpace::BT2100_HLG_1000),
        Some("linear_srgb") => Ok(ColorSpace::LINEAR_SRGB),
        _ => Err(bad_json("unknown color space token")),
    }
}

fn parse_pixel_format_name(val: &Value) -> PipeResult<PixelFormat> {
    match val.as_str() {
        Some("rgb8_srgb") | Some("srgb") => Ok(PixelFormat::RGB8_SRGB),
        Some("rgba8_srgb") => Ok(PixelFormat::RGBA8_SRGB),
        Some("rgb16_srgb") => Ok(PixelFormat::RGB16_SRGB),
        Some("rgba16_srgb") => Ok(PixelFormat::RGBA16_SRGB),
        Some("rgba_f32_linear") => Ok(PixelFormat::RGBAF32_LINEAR),
        _ => Err(bad_json("unknown pixel format token")),
    }
}

fn parse_kernel_opt(val: Option<&Value>) -> PipeResult<ResizeKernel> {
    let Some(v) = val else {
        return Ok(ResizeKernel::Robidoux);
    };
    let s = v
        .as_str()
        .ok_or_else(|| bad_json("resize kernel must be a string"))?;
    match s {
        "robidoux" => Ok(ResizeKernel::Robidoux),
        "lanczos3" => Ok(ResizeKernel::Lanczos3),
        "catmull_rom" => Ok(ResizeKernel::CatmullRom),
        "linear" => Ok(ResizeKernel::Linear),
        "nearest" => Ok(ResizeKernel::Nearest),
        o => Err(bad_json(alloc::format!("unknown resize kernel '{o}'"))),
    }
}

fn parse_rgba_opt(val: Option<&Value>) -> PipeResult<Option<[u8; 4]>> {
    let Some(v) = val else { return Ok(None) };
    let arr = v
        .as_array()
        .ok_or_else(|| bad_json("fill color must be [r, g, b, a]"))?;
    if arr.len() != 4 {
        return Err(bad_json("fill color requires 4 u8 components"));
    }
    let mut out = [0u8; 4];
    for (i, item) in arr.iter().enumerate() {
        out[i] = item
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| bad_json("fill color components must be u8"))?;
    }
    Ok(Some(out))
}

fn opt_str<'a>(
    m: &'a serde_json::Map<String, Value>,
    key: &str,
    ctx: &str,
) -> PipeResult<Option<&'a str>> {
    match m.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_str()
            .map(Some)
            .ok_or_else(|| bad_json(alloc::format!("{ctx}.{key} must be a string"))),
    }
}

fn opt_bool(m: &serde_json::Map<String, Value>, key: &str, ctx: &str) -> PipeResult<Option<bool>> {
    match m.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| bad_json(alloc::format!("{ctx}.{key} must be a boolean"))),
    }
}

fn opt_i32(m: &serde_json::Map<String, Value>, key: &str, ctx: &str) -> PipeResult<Option<i32>> {
    match m.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| bad_json(alloc::format!("{ctx}.{key} must be an i32"))),
    }
}

fn opt_f32(m: &serde_json::Map<String, Value>, key: &str, ctx: &str) -> PipeResult<Option<f32>> {
    match m.get(key) {
        None => Ok(None),
        Some(v) => {
            let f = v
                .as_f64()
                .ok_or_else(|| bad_json(alloc::format!("{ctx}.{key} must be a finite number")))?
                as f32;
            if !f.is_finite() {
                return Err(bad_json(alloc::format!("{ctx}.{key} must be finite")));
            }
            Ok(Some(f))
        }
    }
}

fn req_u32(m: &serde_json::Map<String, Value>, key: &str, ctx: &str) -> PipeResult<u32> {
    m.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| bad_json(alloc::format!("{ctx}.{key} must be a u32")))
}

fn opt_u32(m: &serde_json::Map<String, Value>, key: &str, ctx: &str) -> PipeResult<Option<u32>> {
    match m.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| bad_json(alloc::format!("{ctx}.{key} must be a u32"))),
    }
}

fn req_f32(m: &serde_json::Map<String, Value>, key: &str, ctx: &str) -> PipeResult<f32> {
    let f = m
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| bad_json(alloc::format!("{ctx}.{key} must be a finite number")))?
        as f32;
    if !f.is_finite() {
        return Err(bad_json(alloc::format!("{ctx}.{key} must be finite")));
    }
    Ok(f)
}

fn parse_u32_or_str(val: Option<&Value>, ctx: &str) -> PipeResult<u32> {
    match val {
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|x| u32::try_from(x).ok())
            .ok_or_else(|| bad_json(alloc::format!("{ctx} must be u32"))),
        Some(Value::String(s)) => s
            .parse::<u32>()
            .map_err(|_| bad_json(alloc::format!("{ctx} must be u32"))),
        _ => Err(bad_json(alloc::format!("{ctx} is required"))),
    }
}

fn parse_u64_or_str(val: Option<&Value>, ctx: &str) -> PipeResult<u64> {
    match val {
        Some(Value::Number(n)) => n
            .as_u64()
            .ok_or_else(|| bad_json(alloc::format!("{ctx} must be u64"))),
        Some(Value::String(s)) => s
            .parse::<u64>()
            .map_err(|_| bad_json(alloc::format!("{ctx} must be u64"))),
        _ => Err(bad_json(alloc::format!("{ctx} is required"))),
    }
}
