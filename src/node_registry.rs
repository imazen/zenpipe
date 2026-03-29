//! Full node registry collecting all zennode definitions.
//!
//! Geometry, resize, and pipeline-level nodes are defined in
//! [`crate::zennode_defs`]. Codec, quantize, and quality-intent nodes
//! are defined in [`zencodecs::zennode_defs`]. Filter nodes come from
//! zenfilters. All are registered into a single [`NodeRegistry`].
//!
//! # Features
//!
//! - `zennode` — enables all node registries (zencodecs nodes included automatically)
//! - `nodes-filters` (requires std) — zenfilters' own zennode definitions
//! - `nodes-all` enables everything.

#[cfg(feature = "zennode")]
use zennode::NodeRegistry;

/// Build a [`NodeRegistry`] with all node definitions enabled by features.
///
/// - [`zencodecs::zennode_defs::register`] registers codec, quantize,
///   and quality-intent nodes.
/// - [`zenfilters::zennode_defs::register`] adds filter nodes when
///   `nodes-filters` is active.
/// - [`crate::zennode_defs::register`] registers geometry, resize, and
///   pipeline-level nodes.
///
/// # Example
///
/// ```ignore
/// let registry = zenpipe::full_registry();
/// // Parse RIAPI querystring against all known nodes
/// let nodes = registry.from_querystring("w=800&h=600&jpeg.quality=85");
/// // Generate markdown documentation
/// for def in registry.iter() {
///     println!("{}", def.schema().to_markdown());
/// }
/// ```
#[cfg(feature = "zennode")]
pub fn full_registry() -> NodeRegistry {
    let mut r = NodeRegistry::new();

    // Codec, quantize, and quality-intent nodes (from zencodecs)
    zencodecs::zennode_defs::register(&mut r);

    // Filters (still external — zenfilters keeps its own zennode_defs)
    #[cfg(feature = "nodes-filters")]
    zenfilters::zennode_defs::register(&mut r);

    // Geometry, resize, and pipeline-level nodes (this crate)
    crate::zennode_defs::register(&mut r);

    r
}
