//! Pipeline cachepoint for incremental re-execution.
//!
//! Stores materialized pixels at a pipeline split point (typically post-geometry)
//! so that when only downstream nodes change (filters, encode settings), the
//! expensive upstream prefix (decode + orient + crop + resize) can be skipped.
//!
//! Designed for editor use cases where the user repeatedly tweaks filters
//! while the source image and geometry stay fixed.
//!
//! # Low-level API
//!
//! [`CachedPixels`] and [`CacheSource`] are the building blocks — they
//! store Arc-backed pixels and stream strips from them. These work in
//! `no_std + alloc` and are WASM-safe.
//!
//! # High-level API (requires `zennode` feature)
//!
//! [`PipelineCache`], [`prefix_hash()`], and [`geometry_split()`] provide
//! the full cache-point workflow: hash node configs, find the geometry/filter
//! boundary, and store everything the suffix pipeline needs.

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::Source;
use crate::format::PixelFormat;
use crate::strip::Strip;
#[allow(unused_imports)]
use whereat::at;

/// Shared pixel buffer backing a [`CacheSource`].
///
/// Reference-counted so multiple suffix pipelines (e.g., preview + export)
/// can read from the same cached pixels without copying.
#[derive(Clone)]
pub struct CachedPixels {
    data: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    format: PixelFormat,
    stride: usize,
}

impl CachedPixels {
    /// Create from a [`MaterializedSource`](crate::sources::MaterializedSource),
    /// moving the pixel data into a shared `Arc`.
    pub fn from_materialized(mat: crate::sources::MaterializedSource) -> Self {
        let width = mat.width();
        let height = mat.height();
        let format = mat.format();
        let stride = mat.stride();
        Self {
            data: Arc::new(mat.into_data()),
            width,
            height,
            format,
            stride,
        }
    }

    /// Width of the cached image.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of the cached image.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Pixel format of the cached image.
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Bytes per row (may include alignment padding).
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Raw pixel data.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Total size of the cached pixel buffer in bytes.
    pub fn byte_size(&self) -> usize {
        self.data.len()
    }

    /// Create a [`CacheSource`] that streams strips from these cached pixels.
    ///
    /// Each source is independent — call this multiple times for fan-out
    /// (e.g., preview pipeline + export pipeline reading the same cache).
    pub fn source(&self) -> CacheSource {
        CacheSource {
            data: Arc::clone(&self.data),
            width: self.width,
            height: self.height,
            format: self.format,
            stride: self.stride,
            strip_height: 16.min(self.height),
            y: 0,
        }
    }
}

/// A [`Source`] that streams strips from a cached pixel buffer.
///
/// Reads from an `Arc<Vec<u8>>` with an independent row cursor.
/// Multiple `CacheSource` instances can read from the same cache
/// concurrently without copying.
pub struct CacheSource {
    data: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    format: PixelFormat,
    stride: usize,
    strip_height: u32,
    y: u32,
}

impl Source for CacheSource {
    fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
        use crate::strip::BufferResultExt as _;
        if self.y >= self.height {
            return Ok(None);
        }

        let rows = self.strip_height.min(self.height - self.y);
        let start = self.y as usize * self.stride;
        let end = start + rows as usize * self.stride;

        self.y += rows;

        Ok(Some(
            Strip::new(
                &self.data[start..end],
                self.width,
                rows,
                self.stride,
                self.format,
            )
            .pipe_err()?,
        ))
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn format(&self) -> PixelFormat {
        self.format
    }
}

// ─── Prefix hashing & node splitting (zennode feature) ───

#[cfg(feature = "zennode")]
mod zennode_impls {
    use super::*;
    use crate::bridge::EncodeConfig;
    use crate::sidecar::ProcessedSidecar;
    use alloc::boxed::Box;

    /// Cached pipeline state at a split point.
    ///
    /// Contains everything the suffix pipeline and encoder need:
    /// - Materialized pixels (post-geometry, in working colorspace)
    /// - Decoder metadata for encoder passthrough (EXIF, XMP, ICC, CICP, HDR)
    /// - Processed sidecar (gain map), if present
    /// - Encode configuration from prefix-phase encode nodes
    ///
    /// The `prefix_key` is caller-provided for cache identity. The caller
    /// manages invalidation — when the source image or geometry parameters
    /// change, create a new `PipelineCache` with a different key.
    pub struct PipelineCache {
        /// Caller-provided key identifying the prefix configuration.
        ///
        /// Typically a hash of: source image identity + geometry node params.
        pub prefix_key: u64,

        /// Materialized pixels at the cache split point.
        pub pixels: CachedPixels,

        /// Metadata from the decoder, for passthrough to the encoder.
        pub metadata: Option<zencodec::Metadata>,

        /// Processed sidecar (gain map), if present.
        pub sidecar: Option<ProcessedSidecar>,

        /// Encode configuration extracted from the prefix compilation.
        pub encode_config: EncodeConfig,

        /// Post-prefix dimensions and format.
        pub cached_width: u32,
        pub cached_height: u32,
        pub cached_format: PixelFormat,
    }

    impl PipelineCache {
        /// Create a streaming [`CacheSource`] from this cache.
        pub fn source(&self) -> CacheSource {
            self.pixels.source()
        }

        /// Check if this cache matches a given prefix key.
        pub fn matches(&self, key: u64) -> bool {
            self.prefix_key == key
        }
    }

    /// Compute a deterministic hash of a prefix node configuration.
    ///
    /// Hashes each node's schema ID, version, and all parameter values,
    /// plus the source image identity (dimensions, format, orientation).
    /// Two prefixes with identical hashes produce identical pixel output.
    ///
    /// Uses FNV-1a with a fixed seed for determinism (no random seed).
    pub fn prefix_hash(
        nodes: &[Box<dyn zennode::NodeInstance>],
        source_width: u32,
        source_height: u32,
        source_format: PixelFormat,
        exif_orientation: u8,
    ) -> u64 {
        use core::hash::{Hash, Hasher};

        let mut h = FnvHasher::new();

        // Source identity.
        source_width.hash(&mut h);
        source_height.hash(&mut h);
        hash_pixel_format(&mut h, source_format);
        exif_orientation.hash(&mut h);

        // Node configs.
        for node in nodes {
            hash_node(&mut h, node.as_ref());
        }

        h.finish()
    }

    /// Compute a deterministic subtree hash for a single node given its
    /// input hashes (Merkle-style).
    ///
    /// ```text
    /// node_hash = fnv(schema.id, schema.version, params, [input_hashes...])
    /// ```
    pub fn subtree_hash(node: &dyn zennode::NodeInstance, input_hashes: &[u64]) -> u64 {
        use core::hash::{Hash, Hasher};

        let mut h = FnvHasher::new();
        hash_node(&mut h, node);

        // Chain input subtree hashes.
        for &ih in input_hashes {
            ih.hash(&mut h);
        }

        h.finish()
    }

    /// Hash a single node's identity (schema + params) into a hasher.
    fn hash_node(h: &mut impl core::hash::Hasher, node: &dyn zennode::NodeInstance) {
        use core::hash::Hash;

        let schema = node.schema();
        schema.id.hash(h);
        schema.version.hash(h);

        // ParamMap is BTreeMap — deterministic iteration order.
        let params = node.to_params();
        for (name, value) in &params {
            name.hash(h);
            hash_param_value(h, value);
        }
    }

    /// Find the split index: the first node that is NOT a geometry operation.
    ///
    /// All nodes before this index are geometry (crop, orient, resize, constrain).
    /// All nodes at and after this index are the suffix (filters, composite, etc.).
    ///
    /// Returns `nodes.len()` if all nodes are geometry.
    /// Returns `0` if the first node is already non-geometry.
    pub fn geometry_split(nodes: &[Box<dyn zennode::NodeInstance>]) -> usize {
        for (i, node) in nodes.iter().enumerate() {
            if !node.schema().role.is_geometry() {
                return i;
            }
        }
        nodes.len()
    }

    fn hash_pixel_format(h: &mut impl core::hash::Hasher, fmt: PixelFormat) {
        use core::hash::Hash;
        fmt.bytes_per_pixel().hash(h);
        let s = alloc::format!("{fmt}");
        s.hash(h);
    }

    fn hash_param_value(h: &mut impl core::hash::Hasher, value: &zennode::ParamValue) {
        use core::hash::Hash;
        core::mem::discriminant(value).hash(h);
        match value {
            zennode::ParamValue::None => {}
            zennode::ParamValue::F32(v) => v.to_bits().hash(h),
            zennode::ParamValue::I32(v) => v.hash(h),
            zennode::ParamValue::U32(v) => v.hash(h),
            zennode::ParamValue::Bool(v) => v.hash(h),
            zennode::ParamValue::Str(v) => v.hash(h),
            zennode::ParamValue::Enum(v) => v.hash(h),
            zennode::ParamValue::F32Array(v) => {
                v.len().hash(h);
                for f in v {
                    f.to_bits().hash(h);
                }
            }
            zennode::ParamValue::Color(v) => {
                for f in v {
                    f.to_bits().hash(h);
                }
            }
            zennode::ParamValue::Json(v) => v.hash(h),
            _ => {}
        }
    }
}

#[cfg(feature = "zennode")]
pub use zennode_impls::{PipelineCache, geometry_split, prefix_hash, subtree_hash};

// ─── FNV-1a hasher ───

/// FNV-1a hasher — deterministic, no random seed.
///
/// Public so `Session` and other modules can use the same hasher.
#[cfg(any(feature = "zennode", test))]
pub(crate) struct FnvHasher(u64);

#[cfg(any(feature = "zennode", test))]
impl FnvHasher {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x00000100000001B3;

    pub(crate) fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }
}

#[cfg(any(feature = "zennode", test))]
impl core::hash::Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= byte as u64;
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn cache_source_streams_all_rows() {
        let width = 64u32;
        let height = 100u32;
        let format = crate::format::RGBA8_SRGB;
        let stride = format.aligned_stride(width);
        let data = vec![128u8; stride * height as usize];

        let cached = CachedPixels {
            data: Arc::new(data),
            width,
            height,
            format,
            stride,
        };

        let mut source = cached.source();
        assert_eq!(source.width(), width);
        assert_eq!(source.height(), height);
        assert_eq!(source.format(), format);

        let mut total_rows = 0u32;
        while let Some(strip) = source.next().unwrap() {
            total_rows += strip.rows();
            assert_eq!(strip.width(), width);
        }
        assert_eq!(total_rows, height);
    }

    #[test]
    fn cache_source_multiple_readers() {
        let width = 32u32;
        let height = 48u32;
        let format = crate::format::RGBA8_SRGB;
        let stride = format.aligned_stride(width);
        let data = vec![42u8; stride * height as usize];

        let cached = CachedPixels {
            data: Arc::new(data),
            width,
            height,
            format,
            stride,
        };

        let mut src_a = cached.source();
        let mut src_b = cached.source();

        let mut rows_a = 0u32;
        while let Some(strip) = src_a.next().unwrap() {
            rows_a += strip.rows();
        }

        let mut rows_b = 0u32;
        while let Some(strip) = src_b.next().unwrap() {
            rows_b += strip.rows();
        }

        assert_eq!(rows_a, height);
        assert_eq!(rows_b, height);
    }

    #[test]
    fn cache_source_pixel_fidelity() {
        let width = 4u32;
        let height = 2u32;
        let format = crate::format::RGBA8_SRGB;
        let stride = format.aligned_stride(width);
        let mut data = vec![0u8; stride * height as usize];
        data[..stride].fill(0xAA);
        data[stride..2 * stride].fill(0xBB);

        let cached = CachedPixels {
            data: Arc::new(data),
            width,
            height,
            format,
            stride,
        };

        let mut source = cached.source();
        let strip = source.next().unwrap().unwrap();
        assert_eq!(strip.rows(), 2);
        assert!(strip.row(0).iter().all(|&b| b == 0xAA));
        assert!(strip.row(1).iter().all(|&b| b == 0xBB));
    }

    #[test]
    fn cached_pixels_clone_shares_data() {
        let format = crate::format::RGBA8_SRGB;
        let stride = format.aligned_stride(8);
        let data = vec![1u8; stride * 4];
        let original = CachedPixels {
            data: Arc::new(data),
            width: 8,
            height: 4,
            format,
            stride,
        };

        let cloned = original.clone();
        assert!(Arc::ptr_eq(&original.data, &cloned.data));
    }

    #[test]
    fn from_materialized_roundtrip() {
        let width = 16u32;
        let height = 8u32;
        let format = crate::format::RGBA8_SRGB;
        let stride = format.aligned_stride(width);
        let data: Vec<u8> = (0..stride * height as usize).map(|i| i as u8).collect();

        let mat =
            crate::sources::MaterializedSource::from_data(data.clone(), width, height, format);
        let cached = CachedPixels::from_materialized(mat);

        assert_eq!(cached.width(), width);
        assert_eq!(cached.height(), height);
        assert_eq!(cached.format(), format);
        assert_eq!(cached.stride(), stride);
        assert_eq!(cached.data(), &data[..]);
    }

    #[test]
    fn byte_size_matches_buffer() {
        let format = crate::format::RGBA8_SRGB;
        let stride = format.aligned_stride(32);
        let data = vec![0u8; stride * 16];
        let expected = data.len();
        let cached = CachedPixels {
            data: Arc::new(data),
            width: 32,
            height: 16,
            format,
            stride,
        };
        assert_eq!(cached.byte_size(), expected);
    }

    #[test]
    fn fnv_hasher_deterministic() {
        use core::hash::{Hash, Hasher};

        let mut h1 = FnvHasher::new();
        42u64.hash(&mut h1);
        "hello".hash(&mut h1);

        let mut h2 = FnvHasher::new();
        42u64.hash(&mut h2);
        "hello".hash(&mut h2);

        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn fnv_hasher_different_inputs_differ() {
        use core::hash::{Hash, Hasher};

        let mut h1 = FnvHasher::new();
        1u32.hash(&mut h1);

        let mut h2 = FnvHasher::new();
        2u32.hash(&mut h2);

        assert_ne!(h1.finish(), h2.finish());
    }
}
