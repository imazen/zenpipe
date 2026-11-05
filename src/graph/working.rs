//! Which format each node works in, decided once by the precision policy.
//!
//! `compile` and `estimate` read one answer. The nodes say what they can run in; only they know
//! which kernels exist. See zenpixels `docs/precision-policy-design.md`.

use alloc::boxed::Box;
use alloc::vec::Vec;

use hashbrown::HashMap;
use whereat::{At, at};
use zenpixels::precision::{
    ChainPlan, ConversionDecision, ConversionOffer, ConversionPolicy, ConversionQuestion,
    OutputChoice, SourcePrecision,
};

use super::{EdgeKind, NodeId, NodeOp, PipelineGraph, strip_mem};
use crate::error::PipeError;
use crate::format::{self, PixelFormat};

type Resolver =
    dyn Fn(ConversionQuestion<'_>) -> Result<ConversionDecision, At<PipeError>> + Send + Sync;

/// What a graph knows about precision before it runs.
#[derive(Default)]
pub(super) struct WorkingFormats {
    pub(super) policy: ConversionPolicy,
    pub(super) declared: HashMap<NodeId, SourcePrecision>,
    pub(super) resolver: Option<Box<Resolver>>,
    /// Filled by `compile` from the plan.
    pub(super) planned: HashMap<NodeId, PixelFormat>,
}

/// Formats a node runs in, and whether it composites alpha onto a matte.
fn node_candidates(op: &NodeOp, start: PixelFormat) -> Option<(&'static [PixelFormat], bool)> {
    const RGBA8: &[PixelFormat] = &[format::RGBA8_SRGB];
    const RGB8: &[PixelFormat] = &[format::RGB8_SRGB];
    const RGBA8_LINEAR: &[PixelFormat] = &[format::RGBA8_LINEAR];
    const EITHER: &[PixelFormat] = &[format::RGBA8_SRGB, format::RGBAF32_LINEAR];
    const BLEND: &[PixelFormat] = &[format::RGBAF32_LINEAR_PREMUL];
    #[cfg(feature = "std")]
    const FILTER: &[PixelFormat] = &[format::RGBAF32_LINEAR];
    Some(match op {
        NodeOp::Resize { .. } => (EITHER, false),
        // Eight-bit linear is gain-map data, resampled in its own domain.
        NodeOp::Layout { .. }
            if start.transfer == crate::TransferFunction::Linear
                && start.channel_type() == zenpixels::ChannelType::U8 =>
        {
            (RGBA8_LINEAR, false)
        }
        NodeOp::Layout { .. }
        | NodeOp::Constrain { .. }
        | NodeOp::ResizeAdvanced(_)
        | NodeOp::Orient(_)
        | NodeOp::AutoOrient(_)
        | NodeOp::AddAlpha
        | NodeOp::CropWhitespace { .. } => (RGBA8, false),
        NodeOp::RemoveAlpha { .. } => (RGB8, true),
        NodeOp::LayoutComposite { .. } | NodeOp::Composite { .. } | NodeOp::Overlay { .. } => {
            (BLEND, false)
        }
        #[cfg(feature = "std")]
        NodeOp::Filter(_) => (FILTER, false),
        _ => return None,
    })
}

impl PipelineGraph {
    /// Plan the chain from the output back to its source.
    pub(super) fn plan_working_formats(
        &self,
        sources: impl Iterator<Item = (NodeId, PixelFormat, u32)>,
    ) -> crate::PipeResult<(HashMap<NodeId, PixelFormat>, Option<ChainPlan>)> {
        let mut precision: Option<SourcePrecision> = None;
        let mut known: HashMap<NodeId, (PixelFormat, u32)> = HashMap::new();
        for (id, source_format, width) in sources {
            let this = self
                .working
                .declared
                .get(&id)
                .copied()
                .unwrap_or_else(|| SourcePrecision::from_storage(source_format));
            precision = Some(precision.map_or(this, |joined| joined.joined_with(this)));
            known.insert(id, (source_format, width));
        }
        let Some(precision) = precision else {
            return Ok((HashMap::new(), None));
        };

        // Follow the input edges back from the output.
        let mut chain = Vec::new();
        let mut at_node = self
            .nodes
            .iter()
            .position(|n| matches!(&n.op, Some(NodeOp::Output)));
        let mut start = None;
        while let Some(id) = at_node {
            if let Some(&found) = known.get(&id) {
                start = Some(found);
                break;
            }
            chain.push(id);
            at_node = self.find_input(id, EdgeKind::Input).ok();
        }
        let Some((start, width)) = start else {
            return Ok((HashMap::new(), None));
        };
        chain.reverse();

        let mut ids = Vec::new();
        let mut offers: Vec<Vec<ConversionOffer>> = Vec::new();
        for id in chain {
            let Some(op) = self.nodes[id].op.as_ref() else {
                continue;
            };
            let Some((formats, matte)) = node_candidates(op, start) else {
                continue;
            };
            ids.push(id);
            offers.push(
                formats
                    .iter()
                    .map(|&candidate| {
                        let offer = ConversionOffer::new(candidate)
                            .with_peak_bytes(strip_mem(width, candidate));
                        if matte { offer.with_matte() } else { offer }
                    })
                    .collect(),
            );
        }
        if ids.is_empty() {
            return Ok((HashMap::new(), None));
        }
        let nodes: Vec<&[ConversionOffer]> = offers.iter().map(Vec::as_slice).collect();

        let mut ask = |question: ConversionQuestion<'_>| match &self.working.resolver {
            Some(resolver) => resolver(question),
            None => Ok(ConversionDecision::Defer),
        };
        let plan = self
            .working
            .policy
            .plan_chain(&precision, start, &nodes, OutputChoice::Hidden, &mut ask)
            .map_err(|failure| {
                let node = ids.get(failure.error().node).copied();
                at!(PipeError::Op(alloc::format!(
                    "node {node:?}: {}",
                    failure.error().failure
                )))
            })?;
        let planned = ids
            .iter()
            .zip(&plan.steps)
            .map(|(&id, step)| (id, step.output))
            .collect();
        Ok((planned, Some(plan)))
    }
}

/// What the job API has always done, by name: where a node has no format that holds the source,
/// use the one that keeps the most. Nodes with one that holds it use it.
pub(crate) fn narrow_where_a_node_has_no_deeper_path(
    question: ConversionQuestion<'_>,
) -> Result<ConversionDecision, At<PipeError>> {
    if question.admitted().any(|c| c.effect().is_lossless()) {
        return Ok(ConversionDecision::Defer);
    }
    let best = question
        .admitted()
        .max_by_key(|c| c.effect().bits_kept)
        .map(|c| c.index());
    Ok(best.map_or(ConversionDecision::Defer, ConversionDecision::Choose))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::SourceInfo;
    use zenpixels::PixelDescriptor as D;

    #[test]
    fn the_job_api_resolver_narrows_only_where_no_node_holds_the_source() {
        let mut graph =
            PipelineGraph::new().with_conversion_resolver(narrow_where_a_node_has_no_deeper_path);
        let source = graph.add_node(NodeOp::Source);
        let resize = graph.add_node(NodeOp::Resize {
            w: 2,
            h: 2,
            filter: None,
            sharpen_percent: None,
        });
        let orient = graph.add_node(NodeOp::Orient(zenlayout::Orientation::Rotate180));
        let output = graph.add_node(NodeOp::Output);
        graph.add_edge(source, resize, EdgeKind::Input);
        graph.add_edge(resize, orient, EdgeKind::Input);
        graph.add_edge(orient, output, EdgeKind::Input);
        let infos = HashMap::from([(
            source,
            SourceInfo {
                width: 4,
                height: 4,
                format: D::RGBA16_SRGB,
            },
        )]);
        let plan = graph.working_format_plan(&infos).unwrap().unwrap();
        // Resize has a path that holds it, and Orient doesn't.
        assert_eq!(plan.steps[0].output, D::RGBAF32_LINEAR);
        assert_eq!(plan.steps[1].output, D::RGBA8_SRGB);
        assert_eq!(plan.steps[1].effect.bits_dropped, 8);
    }
}
