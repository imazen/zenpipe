//! Node ordering and optimization for the bridge pipeline.
//!
//! Three ordering strategies:
//!
//! 1. **Preserve** — JSON steps, Rust API. User controls order; bridge coalesces
//!    adjacent compatible nodes but never reorders.
//! 2. **Canonical sort** — RIAPI. Querystring keys have no meaningful order, so
//!    nodes are sorted into the conventional phase sequence.
//! 3. **Optimize** — reorder for speed while preserving visual equivalence,
//!    controlled by [`OptimizationLevel`].
//!
//! # Usage
//!
//! ```ignore
//! use zenpipe::bridge::ordering::{canonical_sort, optimize_node_order, OptimizationLevel};
//!
//! // RIAPI: sort into canonical order, then optimize for speed.
//! canonical_sort(&mut nodes);
//! optimize_node_order(OptimizationLevel::Speed, &mut nodes);
//!
//! // JSON steps: preserve user order, no optimization (default).
//! // Or opt in:
//! optimize_node_order(OptimizationLevel::Lossless, &mut nodes);
//! ```

use alloc::boxed::Box;

use zennode::{NodeInstance, NodeRole};

/// How aggressively to reorder nodes for performance.
///
/// Higher levels produce faster pipelines but may introduce tiny
/// pixel-level differences (≤1px at crop/resize boundaries).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum OptimizationLevel {
    /// No reordering. User order preserved exactly.
    ///
    /// Default for JSON steps and DAG inputs.
    #[default]
    None,
    /// Lossless reorderings only.
    ///
    /// - Swap adjacent commutative per-pixel filters (same coalesce group).
    /// - Move orient before crop (coordinate rewrite — identical output).
    Lossless,
    /// Nearly-lossless reorderings for speed.
    ///
    /// Everything in [`Lossless`](Self::Lossless), plus:
    /// - Move crop before resize (fewer pixels to resample; ≤1px border difference).
    /// - Move crop before per-pixel filters (fewer pixels to process).
    ///
    /// Default for RIAPI querystring inputs.
    Speed,
}

/// Sort nodes into canonical RIAPI phase order.
///
/// RIAPI querystring keys have no meaningful position — `?w=800&crop=10,10,500,400`
/// is identical to `?crop=10,10,500,400&w=800`. This function sorts nodes into
/// the conventional pipeline phase sequence:
///
/// ```text
/// Decode → Orient → Geometry → Resize → Filter → Composite → Analysis → Quantize → Encode
/// ```
///
/// Nodes with the same role preserve their relative order (stable sort).
///
/// This should be called **before** [`optimize_node_order()`] when processing
/// RIAPI inputs, since RIAPI has no inherent ordering to preserve.
pub fn canonical_sort(nodes: &mut [Box<dyn NodeInstance>]) {
    nodes.sort_by_key(|node| role_phase_order(node.schema().role));
}

/// Reorder nodes for speed while preserving visual equivalence.
///
/// Uses only schema metadata ([`NodeRole`], [`FormatHint`](zennode::FormatHint))
/// to decide which reorderings are safe. No pixel data is examined.
///
/// Does nothing if `level` is [`OptimizationLevel::None`].
///
/// # Reorderings by level
///
/// **[`Lossless`](OptimizationLevel::Lossless)**:
/// - Orient before crop (coordinate rewrite — output is identical).
/// - Swap adjacent commutative per-pixel filters in the same coalesce group.
///
/// **[`Speed`](OptimizationLevel::Speed)** (includes Lossless):
/// - Crop/dimension-reducing geometry before resize (fewer pixels to resample).
/// - Crop/dimension-reducing geometry before per-pixel filters (fewer pixels to process).
///
/// # Unsafe reorderings (never performed)
///
/// - Resize before sharpen (destroys detail that sharpen enhances).
/// - Filter before neighborhood op (sharpen sees different input).
/// - Crop after composite (changes what's visible).
/// - Reorder non-commutative filters across coalesce group boundaries.
pub fn optimize_node_order(level: OptimizationLevel, nodes: &mut [Box<dyn NodeInstance>]) {
    if level == OptimizationLevel::None || nodes.len() < 2 {
        return;
    }

    // Bubble-pass optimization: repeatedly scan for safe swaps until stable.
    // Node lists are typically short (3–10 nodes) so O(n²) is fine.
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..nodes.len() - 1 {
            if should_swap(nodes[i].as_ref(), nodes[i + 1].as_ref(), level) {
                nodes.swap(i, i + 1);
                changed = true;
            }
        }
    }
}

/// Whether node `a` should be moved after node `b` for optimization.
///
/// Returns `true` if swapping `a` and `b` is safe at the given level
/// AND produces a faster pipeline (fewer pixels processed).
fn should_swap(a: &dyn NodeInstance, b: &dyn NodeInstance, level: OptimizationLevel) -> bool {
    let a_schema = a.schema();
    let b_schema = b.schema();
    let a_role = a_schema.role;
    let b_role = b_schema.role;
    let a_fmt = &a_schema.format;
    let b_fmt = &b_schema.format;

    match level {
        OptimizationLevel::None => false,

        OptimizationLevel::Lossless => {
            // Orient can move before geometry (coordinate rewrite).
            // "Move orient before crop" means: if `a` is crop and `b` is orient,
            // swap so orient comes first.
            if b_role == NodeRole::Orient && a_role.is_geometry() && a_role != NodeRole::Orient {
                return true;
            }

            // Swap adjacent commutative per-pixel filters in the same coalesce group.
            // This enables better coalescing (grouping same-group nodes together).
            if a_role == NodeRole::Filter
                && b_role == NodeRole::Filter
                && !a_fmt.is_neighborhood
                && !b_fmt.is_neighborhood
                && let (Some(a_coal), Some(b_coal)) = (&a_schema.coalesce, &b_schema.coalesce)
                // Only swap if same group — purpose is to cluster for fusion.
                // Swap to sort by group name, so scattered same-group nodes cluster.
                && a_coal.group > b_coal.group
            {
                return true;
            }

            false
        }

        OptimizationLevel::Speed => {
            // Everything from Lossless level.
            if should_swap(a, b, OptimizationLevel::Lossless) {
                return true;
            }

            // Dimension-reducing geometry (crop) before resize.
            // If `a` is a resize and `b` is a dimension-reducing crop, swap them
            // so fewer pixels are resampled.
            if a_role == NodeRole::Resize
                && b_role.is_geometry()
                && b_role != NodeRole::Resize
                && b_fmt.changes_dimensions
            {
                return true;
            }

            // Dimension-reducing geometry (crop) before per-pixel filter.
            // If `a` is a per-pixel filter and `b` is a crop, swap them
            // so fewer pixels are filtered.
            if a_role == NodeRole::Filter
                && !a_fmt.is_neighborhood
                && b_role.is_geometry()
                && b_fmt.changes_dimensions
            {
                return true;
            }

            false
        }
    }
}

/// Map [`NodeRole`] to a phase-order integer for canonical sorting.
///
/// Lower values come first in the pipeline. Roles that are interchangeable
/// (Geometry, Orient, Resize) share adjacent values to minimize unnecessary
/// reordering within the geometry group.
fn role_phase_order(role: NodeRole) -> u32 {
    match role {
        NodeRole::Decode => 0,
        NodeRole::Orient => 1,
        NodeRole::Geometry => 2,
        NodeRole::Resize => 3,
        NodeRole::Filter => 4,
        NodeRole::Composite => 5,
        NodeRole::Analysis => 6,
        NodeRole::Quantize => 7,
        NodeRole::Encode => 8,
        // NodeRole is #[non_exhaustive] — future roles sort after Encode.
        _ => 9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use core::any::Any;
    use zennode::{CoalesceInfo, FormatHint, NodeGroup, NodeSchema, ParamMap, ParamValue};

    // ─── Test infrastructure ───

    struct TestNode {
        schema: &'static NodeSchema,
    }

    impl NodeInstance for TestNode {
        fn schema(&self) -> &'static NodeSchema {
            self.schema
        }
        fn to_params(&self) -> ParamMap {
            ParamMap::new()
        }
        fn get_param(&self, _name: &str) -> Option<ParamValue> {
            None
        }
        fn set_param(&mut self, _name: &str, _value: ParamValue) -> bool {
            false
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
        fn clone_boxed(&self) -> Box<dyn NodeInstance> {
            Box::new(TestNode {
                schema: self.schema,
            })
        }
    }

    fn make_node(schema: &'static NodeSchema) -> Box<dyn NodeInstance> {
        Box::new(TestNode { schema })
    }

    // ─── Static schemas for testing ───

    static CROP_SCHEMA: NodeSchema = NodeSchema {
        id: "test.crop",
        label: "Crop",
        description: "",
        group: NodeGroup::Geometry,
        role: NodeRole::Geometry,
        params: &[],
        tags: &[],
        coalesce: None,
        format: FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: true,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    static ORIENT_SCHEMA: NodeSchema = NodeSchema {
        id: "test.orient",
        label: "Orient",
        description: "",
        group: NodeGroup::Geometry,
        role: NodeRole::Orient,
        params: &[],
        tags: &[],
        coalesce: None,
        format: FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: false,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    static RESIZE_SCHEMA: NodeSchema = NodeSchema {
        id: "test.resize",
        label: "Resize",
        description: "",
        group: NodeGroup::Geometry,
        role: NodeRole::Resize,
        params: &[],
        tags: &[],
        coalesce: None,
        format: FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: true,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    static EXPOSURE_SCHEMA: NodeSchema = NodeSchema {
        id: "test.exposure",
        label: "Exposure",
        description: "",
        group: NodeGroup::Tone,
        role: NodeRole::Filter,
        params: &[],
        tags: &[],
        coalesce: Some(CoalesceInfo {
            group: "fused_adjust",
            fusable: true,
            is_target: false,
        }),
        format: FormatHint {
            preferred: zennode::PixelFormatPreference::OklabF32,
            alpha: zennode::AlphaHandling::Skip,
            changes_dimensions: false,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    static CONTRAST_SCHEMA: NodeSchema = NodeSchema {
        id: "test.contrast",
        label: "Contrast",
        description: "",
        group: NodeGroup::Tone,
        role: NodeRole::Filter,
        params: &[],
        tags: &[],
        coalesce: Some(CoalesceInfo {
            group: "fused_adjust",
            fusable: true,
            is_target: false,
        }),
        format: FormatHint {
            preferred: zennode::PixelFormatPreference::OklabF32,
            alpha: zennode::AlphaHandling::Skip,
            changes_dimensions: false,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    static SHARPEN_SCHEMA: NodeSchema = NodeSchema {
        id: "test.sharpen",
        label: "Sharpen",
        description: "",
        group: NodeGroup::Detail,
        role: NodeRole::Filter,
        params: &[],
        tags: &[],
        coalesce: None,
        format: FormatHint {
            preferred: zennode::PixelFormatPreference::OklabF32,
            alpha: zennode::AlphaHandling::Skip,
            changes_dimensions: false,
            is_neighborhood: true,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    static ENCODE_SCHEMA: NodeSchema = NodeSchema {
        id: "test.encode",
        label: "Encode",
        description: "",
        group: NodeGroup::Encode,
        role: NodeRole::Encode,
        params: &[],
        tags: &[],
        coalesce: None,
        format: FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: false,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    static DECODE_SCHEMA: NodeSchema = NodeSchema {
        id: "test.decode",
        label: "Decode",
        description: "",
        group: NodeGroup::Decode,
        role: NodeRole::Decode,
        params: &[],
        tags: &[],
        coalesce: None,
        format: FormatHint {
            preferred: zennode::PixelFormatPreference::Any,
            alpha: zennode::AlphaHandling::Process,
            changes_dimensions: false,
            is_neighborhood: false,
        },
        version: 1,
        compat_version: 1,
        json_key: "",
        deny_unknown_fields: false,
        inputs: &[],
    };

    // ─── canonical_sort tests ───

    #[test]
    fn canonical_sort_orders_by_phase() {
        let mut nodes: Vec<Box<dyn NodeInstance>> = vec![
            make_node(&ENCODE_SCHEMA),
            make_node(&RESIZE_SCHEMA),
            make_node(&EXPOSURE_SCHEMA),
            make_node(&CROP_SCHEMA),
            make_node(&ORIENT_SCHEMA),
            make_node(&DECODE_SCHEMA),
        ];

        canonical_sort(&mut nodes);

        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(
            ids,
            &[
                "test.decode",
                "test.orient",
                "test.crop",
                "test.resize",
                "test.exposure",
                "test.encode"
            ]
        );
    }

    #[test]
    fn canonical_sort_preserves_same_role_order() {
        let mut nodes: Vec<Box<dyn NodeInstance>> = vec![
            make_node(&CONTRAST_SCHEMA),
            make_node(&EXPOSURE_SCHEMA),
            make_node(&SHARPEN_SCHEMA),
        ];

        canonical_sort(&mut nodes);

        // All are Filter — original relative order preserved (stable sort).
        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(ids, &["test.contrast", "test.exposure", "test.sharpen"]);
    }

    // ─── optimize_node_order tests ───

    #[test]
    fn optimize_none_preserves_order() {
        let mut nodes: Vec<Box<dyn NodeInstance>> = vec![
            make_node(&EXPOSURE_SCHEMA),
            make_node(&CROP_SCHEMA),
            make_node(&RESIZE_SCHEMA),
        ];

        optimize_node_order(OptimizationLevel::None, &mut nodes);

        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(ids, &["test.exposure", "test.crop", "test.resize"]);
    }

    #[test]
    fn optimize_lossless_moves_orient_before_crop() {
        let mut nodes: Vec<Box<dyn NodeInstance>> =
            vec![make_node(&CROP_SCHEMA), make_node(&ORIENT_SCHEMA)];

        optimize_node_order(OptimizationLevel::Lossless, &mut nodes);

        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(ids, &["test.orient", "test.crop"]);
    }

    #[test]
    fn optimize_lossless_does_not_move_crop_before_resize() {
        let mut nodes: Vec<Box<dyn NodeInstance>> =
            vec![make_node(&RESIZE_SCHEMA), make_node(&CROP_SCHEMA)];

        optimize_node_order(OptimizationLevel::Lossless, &mut nodes);

        // Crop before resize is a Speed-level optimization, not Lossless.
        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(ids, &["test.resize", "test.crop"]);
    }

    #[test]
    fn optimize_speed_moves_crop_before_resize() {
        let mut nodes: Vec<Box<dyn NodeInstance>> =
            vec![make_node(&RESIZE_SCHEMA), make_node(&CROP_SCHEMA)];

        optimize_node_order(OptimizationLevel::Speed, &mut nodes);

        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(ids, &["test.crop", "test.resize"]);
    }

    #[test]
    fn optimize_speed_moves_crop_before_per_pixel_filter() {
        let mut nodes: Vec<Box<dyn NodeInstance>> =
            vec![make_node(&EXPOSURE_SCHEMA), make_node(&CROP_SCHEMA)];

        optimize_node_order(OptimizationLevel::Speed, &mut nodes);

        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(ids, &["test.crop", "test.exposure"]);
    }

    #[test]
    fn optimize_speed_does_not_move_crop_before_neighborhood_filter() {
        let mut nodes: Vec<Box<dyn NodeInstance>> =
            vec![make_node(&SHARPEN_SCHEMA), make_node(&CROP_SCHEMA)];

        optimize_node_order(OptimizationLevel::Speed, &mut nodes);

        // Sharpen is a neighborhood filter — crop after sharpen changes output.
        // The rule only applies to non-neighborhood filters.
        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(ids, &["test.sharpen", "test.crop"]);
    }

    #[test]
    fn optimize_speed_does_not_move_resize_before_sharpen() {
        let mut nodes: Vec<Box<dyn NodeInstance>> =
            vec![make_node(&SHARPEN_SCHEMA), make_node(&RESIZE_SCHEMA)];

        optimize_node_order(OptimizationLevel::Speed, &mut nodes);

        // Moving resize before sharpen destroys detail — unsafe reordering.
        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(ids, &["test.sharpen", "test.resize"]);
    }

    #[test]
    fn optimize_speed_full_pipeline() {
        // Typical RIAPI pipeline after canonical sort:
        // orient → crop → resize → exposure → contrast → sharpen → encode
        //
        // Speed optimization should keep this order (it's already optimal).
        let mut nodes: Vec<Box<dyn NodeInstance>> = vec![
            make_node(&ORIENT_SCHEMA),
            make_node(&CROP_SCHEMA),
            make_node(&RESIZE_SCHEMA),
            make_node(&EXPOSURE_SCHEMA),
            make_node(&CONTRAST_SCHEMA),
            make_node(&SHARPEN_SCHEMA),
            make_node(&ENCODE_SCHEMA),
        ];

        optimize_node_order(OptimizationLevel::Speed, &mut nodes);

        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(
            ids,
            &[
                "test.orient",
                "test.crop",
                "test.resize",
                "test.exposure",
                "test.contrast",
                "test.sharpen",
                "test.encode"
            ]
        );
    }

    #[test]
    fn optimize_speed_suboptimal_pipeline() {
        // Suboptimal order: resize → exposure → crop → sharpen
        // Speed should move crop before resize and before exposure.
        let mut nodes: Vec<Box<dyn NodeInstance>> = vec![
            make_node(&RESIZE_SCHEMA),
            make_node(&EXPOSURE_SCHEMA),
            make_node(&CROP_SCHEMA),
            make_node(&SHARPEN_SCHEMA),
        ];

        optimize_node_order(OptimizationLevel::Speed, &mut nodes);

        let ids: Vec<&str> = nodes.iter().map(|n| n.schema().id).collect();
        assert_eq!(
            ids,
            &["test.crop", "test.resize", "test.exposure", "test.sharpen"]
        );
    }
}
