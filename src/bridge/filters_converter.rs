//! NodeConverter for zenfilters nodes.
//!
//! Converts `zenfilters.*` NodeInstance types into `NodeOp::Filter` pipelines.
//! Available when the `nodes-filters` feature is enabled.

use super::NodeConverter;
use crate::graph::NodeOp;
use zennode::NodeInstance;

use crate::PipeError;

/// Converter for `zenfilters.*` nodes → `NodeOp::Filter(pipeline)`.
///
/// Handles single nodes, coalesced groups, and fused groups.
pub struct ZenFiltersConverter;

impl NodeConverter for ZenFiltersConverter {
    fn can_convert(&self, schema_id: &str) -> bool {
        zenfilters::zennode_defs::is_zenfilters_node(schema_id)
    }

    fn convert(&self, node: &dyn NodeInstance) -> crate::PipeResult<NodeOp> {
        let filter = zenfilters::zennode_defs::node_to_filter(node).ok_or_else(|| {
            PipeError::Op(alloc::format!(
                "zenfilters converter: unrecognized node '{}'",
                node.schema().id
            ))
        })?;

        let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default())
            .map_err(|e| {
                PipeError::Op(alloc::format!("zenfilters pipeline creation failed: {e:?}"))
            })?;
        pipeline.push(filter);
        Ok(NodeOp::Filter(pipeline))
    }

    fn convert_group(&self, nodes: &[&dyn NodeInstance]) -> crate::PipeResult<NodeOp> {
        let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default())
            .map_err(|e| {
                PipeError::Op(alloc::format!("zenfilters pipeline creation failed: {e:?}"))
            })?;

        for node in nodes {
            let filter = zenfilters::zennode_defs::node_to_filter(*node).ok_or_else(|| {
                PipeError::Op(alloc::format!(
                    "zenfilters converter: unrecognized node '{}'",
                    node.schema().id
                ))
            })?;
            pipeline.push(filter);
        }

        Ok(NodeOp::Filter(pipeline))
    }

    fn fuse_group(&self, nodes: &[&dyn NodeInstance]) -> crate::PipeResult<Option<NodeOp>> {
        if nodes.len() < 2 {
            return Ok(None);
        }

        let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default())
            .map_err(|e| {
                PipeError::Op(alloc::format!("zenfilters pipeline creation failed: {e:?}"))
            })?;

        for node in nodes {
            if let Some(filter) = zenfilters::zennode_defs::node_to_filter(*node) {
                pipeline.push(filter);
            } else {
                return Ok(None);
            }
        }

        Ok(Some(NodeOp::Filter(pipeline)))
    }
}
