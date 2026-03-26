//! Full node registry collecting all zennode definitions across the ecosystem.
//!
//! Each codec and processing crate registers its nodes behind a feature flag.
//! Call [`full_registry()`] to get a registry with every available node.
//!
//! # Features
//!
//! Each crate's nodes are gated behind an optional feature:
//! - `nodes-jpeg`, `nodes-png`, `nodes-webp`, `nodes-gif`
//! - `nodes-avif`, `nodes-jxl`, `nodes-tiff`, `nodes-bmp`
//! - `nodes-resize`, `nodes-layout`, `nodes-quant`
//! - `nodes-filters` (requires std)
//!
//! `nodes-all` enables everything.

#[cfg(feature = "zennode")]
use zennode::NodeRegistry;

/// Build a [`NodeRegistry`] with all node definitions enabled by features.
///
/// Each crate's `register()` function is called if its feature is active.
/// The resulting registry contains every node schema — encode, decode,
/// geometry, filter, quantize — with full param metadata for
/// documentation, validation, and RIAPI parsing.
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

    // Codec encode/decode nodes
    #[cfg(feature = "nodes-jpeg")]
    zenjpeg::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-png")]
    zenpng::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-webp")]
    zenwebp::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-gif")]
    zengif::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-avif")]
    zenavif::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-jxl")]
    zenjxl::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-tiff")]
    zentiff::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-bmp")]
    zenbitmaps::zennode_defs::register(&mut r);

    // Processing nodes
    #[cfg(feature = "nodes-resize")]
    zenresize::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-layout")]
    zenlayout::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-quant")]
    zenquant::zennode_defs::register(&mut r);

    #[cfg(feature = "nodes-filters")]
    zenfilters::zennode_defs::register(&mut r);

    r
}
