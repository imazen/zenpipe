//! Incremental pipeline session with automatic DAG-level caching.
//!
//! [`Session`] wraps the pipeline execution engine and transparently caches
//! intermediate results using Merkle-style subtree hashing on the node DAG.
//! When only downstream nodes change (e.g., tweaking a filter parameter),
//! the engine detects unchanged upstream subtrees and resumes from cached
//! materializations — no decode, no geometry recomputation.
//!
//! # Design
//!
//! Every node gets a deterministic identity during compilation:
//!
//! ```text
//! source_hash = caller-provided (e.g., hash(path, mtime, size))
//! node_hash(i) = fnv(schema.id, schema.version, params, [node_hash(input) for input in inputs])
//! ```
//!
//! The node list is in order, so this is one forward pass. When the user
//! changes a parameter, only that node's hash changes, cascading downstream
//! but leaving the upstream subtree unchanged.
//!
//! # WASM compatibility
//!
//! Uses a monotonic generation counter instead of `Instant` for LRU ordering.
//! No `std::time` dependency. Works on `wasm32-unknown-unknown`.
//!
//! # Example
//!
//! ```ignore
//! use zenpipe::session::Session;
//! use zenpipe::orchestrate::{ProcessConfig, SourceImageInfo};
//!
//! let mut session = Session::new(64 * 1024 * 1024); // 64 MB cache budget
//!
//! // First render — full execution, caches at geometry split point.
//! let output = session.stream(decoded_source, &config, None, source_hash)?;
//!
//! // Filter tweak — cache hit, skips decode + geometry.
//! let output = session.stream(decoded_source2, &config2, None, source_hash)?;
//! ```

#[cfg(feature = "zennode")]
mod inner {
    use alloc::boxed::Box;

    use crate::Source;
    use crate::cache::{CachedPixels, geometry_split, prefix_hash};
    use crate::format::PixelFormat;
    use crate::orchestrate::{ProcessConfig, SourceImageInfo, StreamingOutput};
    use crate::sidecar::{ProcessedSidecar, SidecarStream};
    use crate::sources::MaterializedSource;

    /// A cache entry: materialized pixels at a subtree boundary.
    struct CacheEntry {
        /// Materialized pixels (Arc-backed, cheap to produce sources from).
        pixels: CachedPixels,
        /// Decoder metadata for encoder passthrough.
        metadata: Option<zencodec::Metadata>,
        /// Processed sidecar (gain map), if present.
        sidecar: Option<ProcessedSidecar>,
        /// Post-cache-point dimensions.
        width: u32,
        height: u32,
        /// Pixel format at the cache point.
        format: PixelFormat,
        /// Generation counter for LRU eviction (monotonically increasing).
        last_used: u64,
    }

    impl CacheEntry {
        fn byte_size(&self) -> usize {
            self.pixels.byte_size()
        }
    }

    /// Incremental pipeline session with automatic caching.
    ///
    /// Caches intermediate pipeline results using content-addressed hashing.
    /// When only downstream nodes change, the engine detects unchanged
    /// upstream subtrees and resumes from cached materializations.
    pub struct Session {
        /// Content-addressed cache: subtree_hash → materialized pixels + metadata.
        cache: hashbrown::HashMap<u64, CacheEntry>,
        /// Memory budget for cached pixels (bytes). LRU eviction when exceeded.
        memory_budget: usize,
        /// Current total cached bytes.
        current_bytes: usize,
        /// Monotonic generation counter — incremented on each stream() call.
        /// Used for LRU ordering instead of `Instant` (WASM-safe).
        generation: u64,
    }

    impl Session {
        /// Create a new session with the given memory budget (in bytes).
        ///
        /// The budget controls how much pixel data is retained in the cache.
        /// When the budget is exceeded, the least recently used entries are
        /// evicted. A budget of 0 disables caching entirely.
        pub fn new(memory_budget: usize) -> Self {
            Self {
                cache: hashbrown::HashMap::new(),
                memory_budget,
                current_bytes: 0,
                generation: 0,
            }
        }

        /// Number of entries currently in the cache.
        pub fn cache_len(&self) -> usize {
            self.cache.len()
        }

        /// Current total cached bytes.
        pub fn current_bytes(&self) -> usize {
            self.current_bytes
        }

        /// Memory budget.
        pub fn memory_budget(&self) -> usize {
            self.memory_budget
        }

        /// Clear all cached entries.
        pub fn clear(&mut self) {
            self.cache.clear();
            self.current_bytes = 0;
        }

        /// Build a streaming pipeline, using cached prefix data when available.
        ///
        /// # Cache logic
        ///
        /// 1. Split the node list at the geometry/filter boundary.
        /// 2. Compute a prefix hash from source identity + geometry node configs.
        /// 3. If the prefix hash matches a cache entry, inject a `CacheSource`
        ///    at the split point — skip decode + geometry entirely.
        /// 4. If cache miss, execute the full pipeline and cache the prefix
        ///    materialization for next time.
        ///
        /// # Arguments
        ///
        /// * `source` — Decoded pixel source. Required on cache miss.
        ///   On cache hit the source is still consumed (it may hold resources).
        /// * `config` — Processing configuration (nodes, converters, source info).
        /// * `sidecar` — Optional gain map sidecar stream.
        /// * `source_hash` — Caller-provided hash identifying the source image
        ///   (e.g., hash of file path + mtime + size). Used as part of the
        ///   prefix cache key.
        pub fn stream(
            &mut self,
            source: Box<dyn Source>,
            config: &ProcessConfig<'_>,
            sidecar: Option<SidecarStream>,
            source_hash: u64,
        ) -> crate::PipeResult<StreamingOutput> {
            self.generation += 1;

            let nodes = config.nodes;
            let split = geometry_split(nodes);

            // Compute prefix hash: source identity + geometry nodes.
            let prefix_key = prefix_hash(
                &nodes[..split],
                config.source_info.width,
                config.source_info.height,
                config.source_info.format,
                config.source_info.exif_orientation,
            ) ^ source_hash; // XOR source hash into the prefix key.

            // Check cache.
            if let Some(entry) = self.cache.get_mut(&prefix_key) {
                entry.last_used = self.generation;

                // Build suffix-only config.
                let suffix_source = Box::new(entry.pixels.source());
                let suffix_info = SourceImageInfo {
                    width: entry.width,
                    height: entry.height,
                    format: entry.format,
                    has_alpha: config.source_info.has_alpha,
                    has_animation: false,
                    has_gain_map: config.source_info.has_gain_map,
                    is_hdr: config.source_info.is_hdr,
                    exif_orientation: 1, // Already applied in prefix.
                    metadata: entry.metadata.clone(),
                };

                let suffix_nodes = &nodes[split..];
                let suffix_config = ProcessConfig {
                    nodes: suffix_nodes,
                    converters: config.converters,
                    hdr_mode: config.hdr_mode,
                    source_info: &suffix_info,
                    trace_config: config.trace_config,
                };

                // Drop the provided source — not needed for cache hit path.
                drop(source);

                let mut output = crate::orchestrate::stream(
                    suffix_source,
                    &suffix_config,
                    None, // Sidecar already cached.
                )?;

                // Attach cached sidecar if present.
                if output.sidecar.is_none() {
                    output.sidecar = entry.sidecar.clone();
                }

                return Ok(output);
            }

            // Cache miss — execute full pipeline.
            if split == 0 || split == nodes.len() || self.memory_budget == 0 {
                // No geometry prefix to cache, or caching disabled.
                return crate::orchestrate::stream(source, config, sidecar);
            }

            // Execute prefix (geometry nodes only) and materialize.
            let prefix_info = config.source_info;
            let prefix_config = ProcessConfig {
                nodes: &nodes[..split],
                converters: config.converters,
                hdr_mode: config.hdr_mode,
                source_info: prefix_info,
                trace_config: config.trace_config,
            };

            let prefix_output = crate::orchestrate::stream(source, &prefix_config, sidecar)?;

            // Materialize the prefix output so we can cache it.
            let mat = MaterializedSource::from_source(prefix_output.source)?;
            let cached = CachedPixels::from_materialized(mat.clone());
            let entry_bytes = cached.byte_size();

            // Evict entries if over budget.
            self.evict_for(entry_bytes);

            let width = cached.width();
            let height = cached.height();
            let format = cached.format();

            let entry = CacheEntry {
                pixels: cached,
                metadata: prefix_output.metadata.clone(),
                sidecar: prefix_output.sidecar.clone(),
                width,
                height,
                format,
                last_used: self.generation,
            };

            self.current_bytes += entry_bytes;
            self.cache.insert(prefix_key, entry);

            // Now execute suffix from the materialized data.
            let suffix_source = Box::new(MaterializedSource::from_data(
                mat.into_data(),
                width,
                height,
                format,
            ));

            let suffix_info = SourceImageInfo {
                width,
                height,
                format,
                has_alpha: config.source_info.has_alpha,
                has_animation: false,
                has_gain_map: config.source_info.has_gain_map,
                is_hdr: config.source_info.is_hdr,
                exif_orientation: 1,
                metadata: prefix_output.metadata.clone(),
            };

            let suffix_config = ProcessConfig {
                nodes: &nodes[split..],
                converters: config.converters,
                hdr_mode: config.hdr_mode,
                source_info: &suffix_info,
                trace_config: config.trace_config,
            };

            let mut output = crate::orchestrate::stream(suffix_source, &suffix_config, None)?;

            // Attach sidecar from prefix.
            if output.sidecar.is_none() {
                output.sidecar = prefix_output.sidecar;
            }

            // Preserve metadata.
            if output.metadata.is_none() {
                output.metadata = prefix_output.metadata;
            }

            Ok(output)
        }

        /// Evict least-recently-used entries until there's room for `needed` bytes.
        fn evict_for(&mut self, needed: usize) {
            while self.current_bytes + needed > self.memory_budget && !self.cache.is_empty() {
                // Find the entry with the smallest generation (LRU).
                let lru_key = self
                    .cache
                    .iter()
                    .min_by_key(|(_, e)| e.last_used)
                    .map(|(&k, _)| k);

                if let Some(key) = lru_key {
                    if let Some(evicted) = self.cache.remove(&key) {
                        self.current_bytes = self.current_bytes.saturating_sub(evicted.byte_size());
                    }
                } else {
                    break;
                }
            }
        }
    }
}

#[cfg(feature = "zennode")]
pub use inner::Session;

#[cfg(all(test, feature = "zennode", feature = "std"))]
mod tests {
    use super::*;
    use crate::format::RGBA8_SRGB;
    use crate::orchestrate::{ProcessConfig, SourceImageInfo};
    use crate::strip::Strip;
    use alloc::boxed::Box;
    use alloc::vec;

    /// Solid-color test source.
    struct SolidSource {
        w: u32,
        h: u32,
        y: u32,
    }
    impl SolidSource {
        fn new(w: u32, h: u32) -> Self {
            Self { w, h, y: 0 }
        }
    }
    impl crate::Source for SolidSource {
        fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
            use crate::strip::BufferResultExt as _;
            if self.y >= self.h {
                return Ok(None);
            }
            let rows = 16.min(self.h - self.y);
            let stride = RGBA8_SRGB.aligned_stride(self.w);
            let data = vec![128u8; stride * rows as usize];
            self.y += rows;
            let leaked: &'static [u8] = alloc::vec::Vec::leak(data);
            Ok(Some(
                Strip::new(leaked, self.w, rows, stride, RGBA8_SRGB).pipe_err()?,
            ))
        }
        fn width(&self) -> u32 {
            self.w
        }
        fn height(&self) -> u32 {
            self.h
        }
        fn format(&self) -> crate::PixelFormat {
            RGBA8_SRGB
        }
    }

    fn source_info(w: u32, h: u32) -> SourceImageInfo {
        SourceImageInfo {
            width: w,
            height: h,
            format: RGBA8_SRGB,
            has_alpha: true,
            has_animation: false,
            has_gain_map: false,
            is_hdr: false,
            exif_orientation: 1,
            metadata: None,
        }
    }

    fn make_constrain(w: u32, h: u32) -> Box<dyn zennode::NodeInstance> {
        Box::new(crate::zennode_defs::Constrain {
            w: Some(w),
            h: Some(h),
            mode: "within".into(),
            ..Default::default()
        })
    }

    fn make_remove_alpha(r: u32, g: u32, b: u32) -> Box<dyn zennode::NodeInstance> {
        Box::new(crate::zennode_defs::RemoveAlpha {
            matte_r: r,
            matte_g: g,
            matte_b: b,
        })
    }

    // ─── geometry_split tests ───

    #[test]
    fn geometry_split_all_geometry() {
        let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![make_constrain(800, 600)];
        assert_eq!(crate::cache::geometry_split(&nodes), 1);
    }

    #[test]
    fn geometry_split_all_filter() {
        let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![make_remove_alpha(255, 255, 255)];
        assert_eq!(crate::cache::geometry_split(&nodes), 0);
    }

    #[test]
    fn geometry_split_mixed() {
        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(800, 600), make_remove_alpha(255, 255, 255)];
        assert_eq!(crate::cache::geometry_split(&nodes), 1);
    }

    // ─── prefix_hash tests ───

    #[test]
    fn prefix_hash_deterministic() {
        let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![make_constrain(800, 600)];
        let h1 = crate::cache::prefix_hash(&nodes, 4000, 3000, RGBA8_SRGB, 1);
        let h2 = crate::cache::prefix_hash(&nodes, 4000, 3000, RGBA8_SRGB, 1);
        assert_eq!(h1, h2);
    }

    #[test]
    fn prefix_hash_changes_with_params() {
        let nodes_a: Vec<Box<dyn zennode::NodeInstance>> = vec![make_constrain(800, 600)];
        let nodes_b: Vec<Box<dyn zennode::NodeInstance>> = vec![make_constrain(400, 300)];
        let h1 = crate::cache::prefix_hash(&nodes_a, 4000, 3000, RGBA8_SRGB, 1);
        let h2 = crate::cache::prefix_hash(&nodes_b, 4000, 3000, RGBA8_SRGB, 1);
        assert_ne!(h1, h2);
    }

    #[test]
    fn prefix_hash_changes_with_source_dims() {
        let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![make_constrain(800, 600)];
        let h1 = crate::cache::prefix_hash(&nodes, 4000, 3000, RGBA8_SRGB, 1);
        let h2 = crate::cache::prefix_hash(&nodes, 2000, 1500, RGBA8_SRGB, 1);
        assert_ne!(h1, h2);
    }

    // ─── subtree_hash tests ───

    #[test]
    fn subtree_hash_deterministic() {
        let node = crate::zennode_defs::Constrain {
            w: Some(800),
            h: Some(600),
            ..Default::default()
        };
        let h1 = crate::cache::subtree_hash(&node, &[42]);
        let h2 = crate::cache::subtree_hash(&node, &[42]);
        assert_eq!(h1, h2);
    }

    #[test]
    fn subtree_hash_changes_with_inputs() {
        let node = crate::zennode_defs::Constrain {
            w: Some(800),
            h: Some(600),
            ..Default::default()
        };
        let h1 = crate::cache::subtree_hash(&node, &[42]);
        let h2 = crate::cache::subtree_hash(&node, &[99]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn subtree_hash_changes_with_params() {
        let node_a = crate::zennode_defs::Constrain {
            w: Some(800),
            h: Some(600),
            ..Default::default()
        };
        let node_b = crate::zennode_defs::Constrain {
            w: Some(400),
            h: Some(300),
            ..Default::default()
        };
        let h1 = crate::cache::subtree_hash(&node_a, &[42]);
        let h2 = crate::cache::subtree_hash(&node_b, &[42]);
        assert_ne!(h1, h2);
    }

    // ─── Session tests ───

    #[test]
    fn session_cache_miss_then_hit() {
        let mut session = Session::new(64 * 1024 * 1024); // 64 MB

        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let info = source_info(200, 200);

        // First call: cache miss, full execution.
        let config = ProcessConfig {
            nodes: &nodes,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let source = Box::new(SolidSource::new(200, 200));
        let _output = session.stream(source, &config, None, 0xDEAD).unwrap();
        assert_eq!(session.cache_len(), 1);

        // Second call with different filter params: cache hit on geometry prefix.
        let nodes2: Vec<Box<dyn zennode::NodeInstance>> = vec![
            make_constrain(100, 100),   // Same geometry.
            make_remove_alpha(0, 0, 0), // Different filter params.
        ];
        let config2 = ProcessConfig {
            nodes: &nodes2,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let source2 = Box::new(SolidSource::new(200, 200));
        let bytes_before = session.current_bytes();
        let _output2 = session.stream(source2, &config2, None, 0xDEAD).unwrap();
        // Cache should not have grown — hit on existing entry.
        assert_eq!(session.cache_len(), 1);
        assert_eq!(session.current_bytes(), bytes_before);
    }

    #[test]
    fn session_cache_miss_on_geometry_change() {
        let mut session = Session::new(64 * 1024 * 1024);

        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let info = source_info(200, 200);
        let config = ProcessConfig {
            nodes: &nodes,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let _output = session
            .stream(Box::new(SolidSource::new(200, 200)), &config, None, 0xBEEF)
            .unwrap();
        assert_eq!(session.cache_len(), 1);

        // Change geometry → different prefix hash → cache miss → new entry.
        let nodes2: Vec<Box<dyn zennode::NodeInstance>> = vec![
            make_constrain(50, 50), // Different geometry.
            make_remove_alpha(255, 255, 255),
        ];
        let config2 = ProcessConfig {
            nodes: &nodes2,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let _output2 = session
            .stream(Box::new(SolidSource::new(200, 200)), &config2, None, 0xBEEF)
            .unwrap();
        assert_eq!(session.cache_len(), 2);
    }

    #[test]
    fn session_cache_miss_on_source_change() {
        let mut session = Session::new(64 * 1024 * 1024);

        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let info = source_info(200, 200);
        let config = ProcessConfig {
            nodes: &nodes,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let _output = session
            .stream(Box::new(SolidSource::new(200, 200)), &config, None, 0xAAAA)
            .unwrap();
        assert_eq!(session.cache_len(), 1);

        // Same nodes, different source hash → miss.
        let _output2 = session
            .stream(Box::new(SolidSource::new(200, 200)), &config, None, 0xBBBB)
            .unwrap();
        assert_eq!(session.cache_len(), 2);
    }

    #[test]
    fn session_lru_eviction() {
        // Tiny budget: only room for one cache entry.
        // A 100x100 RGBA8 image at 4bpp = 40,000 bytes (stride may be slightly more).
        let mut session = Session::new(50_000);

        let info = source_info(200, 200);
        let nodes_a: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let config_a = ProcessConfig {
            nodes: &nodes_a,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };

        let _output = session
            .stream(Box::new(SolidSource::new(200, 200)), &config_a, None, 0xAA)
            .unwrap();
        assert_eq!(session.cache_len(), 1);

        // Insert a second entry — should evict the first.
        let nodes_b: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let config_b = ProcessConfig {
            nodes: &nodes_b,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let _output2 = session
            .stream(Box::new(SolidSource::new(200, 200)), &config_b, None, 0xBB)
            .unwrap();
        // Should have evicted old entry to fit under budget.
        assert_eq!(session.cache_len(), 1);
    }

    #[test]
    fn session_no_cache_when_no_geometry() {
        let mut session = Session::new(64 * 1024 * 1024);

        // Only filter nodes, no geometry → geometry_split returns 0 → no caching.
        let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![make_remove_alpha(255, 255, 255)];
        let info = source_info(100, 100);
        let config = ProcessConfig {
            nodes: &nodes,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let _output = session
            .stream(Box::new(SolidSource::new(100, 100)), &config, None, 0xCC)
            .unwrap();
        assert_eq!(session.cache_len(), 0);
    }

    #[test]
    fn session_no_cache_when_all_geometry() {
        let mut session = Session::new(64 * 1024 * 1024);

        // Only geometry nodes → split == nodes.len() → no suffix → no caching.
        let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![make_constrain(100, 100)];
        let info = source_info(200, 200);
        let config = ProcessConfig {
            nodes: &nodes,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let _output = session
            .stream(Box::new(SolidSource::new(200, 200)), &config, None, 0xDD)
            .unwrap();
        assert_eq!(session.cache_len(), 0);
    }

    #[test]
    fn session_clear() {
        let mut session = Session::new(64 * 1024 * 1024);

        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let info = source_info(200, 200);
        let config = ProcessConfig {
            nodes: &nodes,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let _output = session
            .stream(Box::new(SolidSource::new(200, 200)), &config, None, 0xEE)
            .unwrap();
        assert_eq!(session.cache_len(), 1);

        session.clear();
        assert_eq!(session.cache_len(), 0);
        assert_eq!(session.current_bytes(), 0);
    }

    #[test]
    fn session_zero_budget_disables_caching() {
        let mut session = Session::new(0);

        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let info = source_info(200, 200);
        let config = ProcessConfig {
            nodes: &nodes,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };
        let _output = session
            .stream(Box::new(SolidSource::new(200, 200)), &config, None, 0xFF)
            .unwrap();
        assert_eq!(session.cache_len(), 0);
    }
}
