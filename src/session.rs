//! Incremental pipeline session with automatic DAG-level caching.
//!
//! [`Session`] wraps the pipeline execution engine and transparently caches
//! intermediate results using Merkle-style subtree hashing on the node list.
//! When only downstream nodes change (e.g., tweaking a filter parameter),
//! the engine detects the unchanged upstream prefix and resumes from its
//! cached materialization — no decode, no geometry recomputation.
//!
//! # Design
//!
//! Every node gets a deterministic identity, chained from the source:
//!
//! ```text
//! chain[0] = fnv(source_hash, width, height, format, orientation, flags, hdr_mode)
//! chain[i] = fnv(schema.id, schema.version, params(nodes[i-1]), chain[i-1])
//! ```
//!
//! `chain[i]` identifies the pixels that exist after the first `i` nodes;
//! one forward pass computes the whole chain. Editing a node changes only
//! its own link and everything downstream, so the upstream prefix keeps its
//! identity:
//!
//! ```text
//! run 1:  source(abc) → constrain(800) → exposure(+0.5) → encode
//! run 2:  source(abc) → constrain(800) → exposure(+1.0) → encode
//!                                  ↑ chain[1] identical → cache hit
//! ```
//!
//! The cache point is the geometry/filter boundary (decode + orient + crop +
//! resize is the expensive prefix). Lookup takes the *longest* cached prefix
//! up to that boundary, so appending a geometry node re-runs only the new
//! node from the cached pixels rather than the whole prefix.
//!
//! Each entry holds everything the suffix needs — pixels, decoder metadata,
//! the processed gain-map sidecar, and dimensions/format — so no decode is
//! needed on a hit. Entries are evicted least-recently-used against a byte
//! budget; an entry that alone exceeds the budget is never stored.
//!
//! # WASM compatibility
//!
//! Uses a monotonic generation counter instead of `Instant` for LRU ordering.
//! No `std::time` dependency. Works on `wasm32-unknown-unknown`.
//!
//! # Editor example
//!
//! The caller owns the source identity: hash whatever makes the decoded
//! pixels unique (path + mtime + size, a content digest, an upload id).
//!
//! ```ignore
//! use zenpipe::session::Session;
//! use zenpipe::orchestrate::{ProcessConfig, SourceImageInfo};
//!
//! let mut session = Session::new(64 * 1024 * 1024); // 64 MB cache budget
//! let source_hash = hash_of(path, mtime, size);
//!
//! // First render — full execution, caches the post-geometry pixels.
//! let nodes = vec![constrain(800, 600), exposure(0.5), encode_jpeg(85)];
//! let config = ProcessConfig { nodes: &nodes, limits: Some(&limits), ..base };
//! let output = session.stream(decode(path)?, &config, None, source_hash)?;
//!
//! // Slider moves — same source, same geometry: the decoder source is
//! // dropped unread and only exposure + encode run.
//! let nodes = vec![constrain(800, 600), exposure(1.0), encode_jpeg(85)];
//! let config = ProcessConfig { nodes: &nodes, limits: Some(&limits), ..base };
//! let output = session.stream(decode(path)?, &config, None, source_hash)?;
//!
//! // Different image → different source_hash → its own entries. Both
//! // images stay cached until the budget forces LRU eviction.
//! ```
//!
//! [`Session::stream_stoppable`] is the same with cooperative cancellation
//! (checked between strips while the prefix materializes).

#[cfg(feature = "zennode")]
mod inner {
    use alloc::boxed::Box;

    use crate::Source;
    use crate::cache::{CachedPixels, SourceIdentity, geometry_split, prefix_chain};
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
        /// 1. Compute the Merkle chain over the node list: `chain[i]` identifies
        ///    the pixels after the first `i` nodes (source identity + `hdr_mode`
        ///    + every upstream node's schema/params).
        /// 2. Split the node list at the geometry/filter boundary (`split`).
        /// 3. Find the longest cached prefix `i ≤ split`. On a full hit
        ///    (`i == split`) inject a `CacheSource` at the split point — no
        ///    decode, no geometry. On a partial hit (`0 < i < split`, e.g. the
        ///    caller appended a geometry node) run only `nodes[i..split]` from
        ///    the cached pixels.
        /// 4. Materialize at `split` and cache it for next time (unless the
        ///    entry alone would exceed the memory budget).
        ///
        /// `config.limits` is enforced on every executed segment exactly as
        /// [`orchestrate::stream`](crate::orchestrate::stream) enforces it.
        ///
        /// # Arguments
        ///
        /// * `source` — Decoded pixel source. Pulled only on a cache miss (or
        ///   on a partial hit when a gain-map sidecar must be re-derived from
        ///   the original source dimensions). Always consumed — on a hit it is
        ///   dropped without being read.
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
            self.stream_inner(source, config, sidecar, source_hash, &enough::Unstoppable)
        }

        /// Like [`stream()`](Self::stream) but with cooperative cancellation.
        ///
        /// Checks `stop` between strips during prefix materialization (cache miss path).
        /// Returns `PipeError::Cancelled` if cancelled mid-render.
        pub fn stream_stoppable(
            &mut self,
            source: Box<dyn Source>,
            config: &ProcessConfig<'_>,
            sidecar: Option<SidecarStream>,
            source_hash: u64,
            stop: &dyn enough::Stop,
        ) -> crate::PipeResult<StreamingOutput> {
            self.stream_inner(source, config, sidecar, source_hash, stop)
        }

        /// Shared implementation for `stream` and `stream_stoppable`.
        fn stream_inner(
            &mut self,
            source: Box<dyn Source>,
            config: &ProcessConfig<'_>,
            sidecar: Option<SidecarStream>,
            source_hash: u64,
            stop: &dyn enough::Stop,
        ) -> crate::PipeResult<StreamingOutput> {
            stop.check()
                .map_err(|_| whereat::at!(crate::error::PipeError::Cancelled))?;

            self.generation += 1;

            let nodes = config.nodes;
            let split = geometry_split(nodes);

            // Merkle chain: chain[i] identifies the pixels after nodes[..i].
            let identity = SourceIdentity {
                source_hash,
                width: config.source_info.width,
                height: config.source_info.height,
                format: config.source_info.format,
                exif_orientation: config.source_info.exif_orientation,
                has_alpha: config.source_info.has_alpha,
                has_gain_map: config.source_info.has_gain_map,
                is_hdr: config.source_info.is_hdr,
                hdr_mode: config.hdr_mode,
            };
            let chain = prefix_chain(nodes, &identity);
            let split_key = chain[split];

            // Longest cached prefix within the geometry segment.
            let mut hit = (1..=split)
                .rev()
                .find(|&i| self.cache.contains_key(&chain[i]));

            // A partial hit re-runs geometry nodes from the cached pixels, but a
            // gain-map sidecar is derived relative to the ORIGINAL source
            // dimensions — which the cached segment no longer knows. Take the
            // full path in that case so the sidecar geometry stays exact.
            if matches!(hit, Some(i) if i < split)
                && sidecar.is_some()
                && config.hdr_mode != "sdr_only"
            {
                hit = None;
            }

            // Full hit at the split point: suffix only, no geometry work.
            if hit == Some(split) {
                let entry = self
                    .cache
                    .get_mut(&split_key)
                    .expect("hit key was just found in the cache");
                entry.last_used = self.generation;

                let suffix_source = Box::new(entry.pixels.source());
                let suffix_info = Self::segment_info(
                    config.source_info,
                    entry.width,
                    entry.height,
                    entry.format,
                    entry.metadata.clone(),
                );
                let suffix_config = Self::segment_config(config, &nodes[split..], &suffix_info);
                let cached_sidecar = entry.sidecar.clone();

                // The provided source is not needed on a hit.
                drop(source);

                let mut output = crate::orchestrate::stream(
                    suffix_source,
                    &suffix_config,
                    None, // Sidecar already cached.
                )?;
                if output.sidecar.is_none() {
                    output.sidecar = cached_sidecar;
                }
                return Ok(output);
            }

            // Nothing cacheable: no geometry prefix, no suffix, or caching off.
            if split == 0 || split == nodes.len() || self.memory_budget == 0 {
                return crate::orchestrate::stream(source, config, sidecar);
            }

            // Miss or partial hit: run nodes[start..split] as the prefix
            // segment, from the caller's source (start == 0) or from the
            // longest cached prefix (start > 0), then materialize at `split`.
            let (start, segment_source, segment_info, segment_sidecar, sidecar_stream) = match hit {
                Some(i) => {
                    let entry = self
                        .cache
                        .get_mut(&chain[i])
                        .expect("hit key was just found in the cache");
                    entry.last_used = self.generation;
                    let seg_source: Box<dyn Source> = Box::new(entry.pixels.source());
                    let seg_info = Self::segment_info(
                        config.source_info,
                        entry.width,
                        entry.height,
                        entry.format,
                        entry.metadata.clone(),
                    );
                    let seg_sidecar = entry.sidecar.clone();
                    // Neither the caller's source nor its sidecar is read.
                    drop(source);
                    drop(sidecar);
                    (i, seg_source, seg_info, seg_sidecar, None)
                }
                None => (0, source, config.source_info.clone(), None, sidecar),
            };

            let prefix_config = Self::segment_config(config, &nodes[start..split], &segment_info);
            let prefix_output =
                crate::orchestrate::stream(segment_source, &prefix_config, sidecar_stream)?;
            let prefix_sidecar = prefix_output.sidecar.or(segment_sidecar);
            let prefix_metadata = prefix_output.metadata;

            // Materialize the prefix output so we can cache it.
            let mat = MaterializedSource::from_source_stoppable(prefix_output.source, stop)?;
            let width = mat.width();
            let height = mat.height();
            let format = mat.format();

            let suffix_source: Box<dyn Source> = if mat.data().len() <= self.memory_budget {
                let cached = CachedPixels::from_materialized(mat);
                let entry_bytes = cached.byte_size();
                self.evict_for(entry_bytes);
                let suffix_source = Box::new(cached.source());
                self.current_bytes += entry_bytes;
                self.cache.insert(
                    split_key,
                    CacheEntry {
                        pixels: cached,
                        metadata: prefix_metadata.clone(),
                        sidecar: prefix_sidecar.clone(),
                        width,
                        height,
                        format,
                        last_used: self.generation,
                    },
                );
                suffix_source
            } else {
                // Entry alone exceeds the budget — evicting everything else
                // would not make it fit, so run the suffix uncached.
                Box::new(mat)
            };

            let suffix_info = Self::segment_info(
                config.source_info,
                width,
                height,
                format,
                prefix_metadata.clone(),
            );
            let suffix_config = Self::segment_config(config, &nodes[split..], &suffix_info);

            let mut output = crate::orchestrate::stream(suffix_source, &suffix_config, None)?;

            // Attach sidecar and metadata from the prefix.
            if output.sidecar.is_none() {
                output.sidecar = prefix_sidecar;
            }
            if output.metadata.is_none() {
                output.metadata = prefix_metadata;
            }

            Ok(output)
        }

        /// Source info for a pipeline segment that starts from materialized
        /// pixels: dims/format of those pixels, orientation already applied,
        /// content flags carried over from the original source.
        fn segment_info(
            original: &SourceImageInfo,
            width: u32,
            height: u32,
            format: PixelFormat,
            metadata: Option<zencodec::Metadata>,
        ) -> SourceImageInfo {
            SourceImageInfo {
                width,
                height,
                format,
                has_alpha: original.has_alpha,
                has_animation: false,
                has_gain_map: original.has_gain_map,
                is_hdr: original.is_hdr,
                exif_orientation: 1, // Applied in the cached prefix.
                metadata,
            }
        }

        /// `config` narrowed to `nodes` and `info`, keeping converters,
        /// hdr_mode, tracing and — importantly — the caller's `limits`.
        fn segment_config<'a>(
            config: &ProcessConfig<'a>,
            nodes: &'a [Box<dyn zennode::NodeInstance>],
            info: &'a SourceImageInfo,
        ) -> ProcessConfig<'a> {
            ProcessConfig {
                nodes,
                converters: config.converters,
                hdr_mode: config.hdr_mode,
                source_info: info,
                trace_config: config.trace_config,
                limits: config.limits,
            }
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
            limits: None,
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
            limits: None,
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
            limits: None,
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
            limits: None,
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
            limits: None,
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
            limits: None,
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
            limits: None,
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
            limits: None,
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
            limits: None,
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
            limits: None,
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
            limits: None,
        };
        let _output = session
            .stream(Box::new(SolidSource::new(200, 200)), &config, None, 0xFF)
            .unwrap();
        assert_eq!(session.cache_len(), 0);
    }

    /// A source that fails the moment anything pulls a strip from it.
    /// Proves a cache hit never touches the caller's source.
    struct PoisonSource {
        w: u32,
        h: u32,
    }
    impl crate::Source for PoisonSource {
        fn next(&mut self) -> crate::PipeResult<Option<Strip<'_>>> {
            Err(whereat::at!(crate::error::PipeError::Op(
                "poison source was pulled".into()
            )))
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

    fn config<'a>(
        nodes: &'a [Box<dyn zennode::NodeInstance>],
        info: &'a SourceImageInfo,
        hdr_mode: &'a str,
        limits: Option<&'a crate::Limits>,
    ) -> ProcessConfig<'a> {
        ProcessConfig {
            nodes,
            converters: &[],
            hdr_mode,
            source_info: info,
            trace_config: None,
            limits,
        }
    }

    fn unwrap_err(
        r: crate::PipeResult<crate::orchestrate::StreamingOutput>,
        what: &str,
    ) -> whereat::At<crate::error::PipeError> {
        match r {
            Err(e) => e,
            Ok(_) => panic!("{what}"),
        }
    }

    fn drain(output: crate::orchestrate::StreamingOutput) -> (u32, u32) {
        let mut src = output.source;
        let (w, h) = (src.width(), src.height());
        let mut rows = 0;
        while let Some(strip) = src.next().unwrap() {
            rows += strip.rows();
        }
        assert_eq!(rows, h);
        (w, h)
    }

    #[test]
    fn session_hit_never_pulls_the_source() {
        let mut session = Session::new(64 * 1024 * 1024);
        let info = source_info(200, 200);
        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let cfg = config(&nodes, &info, "sdr_only", None);
        session
            .stream(Box::new(SolidSource::new(200, 200)), &cfg, None, 0x51)
            .unwrap();

        // Same geometry, different filter: a hit. The poison source must not
        // be read — if the cache key were wrong, this pulls and fails.
        let nodes2: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(0, 0, 0)];
        let cfg2 = config(&nodes2, &info, "sdr_only", None);
        let out = session
            .stream(Box::new(PoisonSource { w: 200, h: 200 }), &cfg2, None, 0x51)
            .unwrap();
        assert_eq!(drain(out), (100, 100));
        assert_eq!(session.cache_len(), 1);
    }

    #[test]
    fn session_partial_prefix_hit_resumes_from_cache() {
        let mut session = Session::new(64 * 1024 * 1024);
        let info = source_info(200, 200);
        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let cfg = config(&nodes, &info, "sdr_only", None);
        session
            .stream(Box::new(SolidSource::new(200, 200)), &cfg, None, 0x52)
            .unwrap();
        assert_eq!(session.cache_len(), 1);

        // Editor appends a geometry node after the cached one: the new split
        // point misses, but chain[1] (constrain 100) hits — the second
        // constrain runs from the cached pixels, never from the source.
        let nodes2: Vec<Box<dyn zennode::NodeInstance>> = vec![
            make_constrain(100, 100),
            make_constrain(50, 50),
            make_remove_alpha(255, 255, 255),
        ];
        let cfg2 = config(&nodes2, &info, "sdr_only", None);
        let out = session
            .stream(Box::new(PoisonSource { w: 200, h: 200 }), &cfg2, None, 0x52)
            .unwrap();
        assert_eq!(drain(out), (50, 50));
        // The new split point is cached too.
        assert_eq!(session.cache_len(), 2);

        // And the extended prefix is now a full hit.
        let out = session
            .stream(Box::new(PoisonSource { w: 200, h: 200 }), &cfg2, None, 0x52)
            .unwrap();
        assert_eq!(drain(out), (50, 50));
        assert_eq!(session.cache_len(), 2);
    }

    #[test]
    fn session_enforces_limits_on_miss_and_hit() {
        let mut session = Session::new(64 * 1024 * 1024);
        let info = source_info(200, 200);
        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];

        // 200×200 source exceeds a 10-pixel cap: the prefix estimate must
        // reject before any pixel work (miss path).
        let tiny = crate::Limits::NONE.with_max_pixels(10);
        let cfg = config(&nodes, &info, "sdr_only", Some(&tiny));
        let err = unwrap_err(
            session.stream(Box::new(SolidSource::new(200, 200)), &cfg, None, 0x53),
            "miss path must enforce limits",
        );
        assert!(
            matches!(err.error(), crate::error::PipeError::LimitExceeded(_)),
            "{err:?}"
        );
        assert_eq!(session.cache_len(), 0);

        // Populate the cache without limits, then hit it with a cap the
        // cached 100×100 prefix still exceeds: the suffix estimate rejects.
        let cfg_free = config(&nodes, &info, "sdr_only", None);
        session
            .stream(Box::new(SolidSource::new(200, 200)), &cfg_free, None, 0x53)
            .unwrap();
        assert_eq!(session.cache_len(), 1);
        let cfg_hit = config(&nodes, &info, "sdr_only", Some(&tiny));
        let err = unwrap_err(
            session.stream(
                Box::new(PoisonSource { w: 200, h: 200 }),
                &cfg_hit,
                None,
                0x53,
            ),
            "hit path must enforce limits",
        );
        assert!(
            matches!(err.error(), crate::error::PipeError::LimitExceeded(_)),
            "{err:?}"
        );

        // A cap the cached prefix fits under passes on the hit path.
        let roomy = crate::Limits::NONE.with_max_pixels(100 * 100);
        let cfg_ok = config(&nodes, &info, "sdr_only", Some(&roomy));
        let out = session
            .stream(
                Box::new(PoisonSource { w: 200, h: 200 }),
                &cfg_ok,
                None,
                0x53,
            )
            .unwrap();
        assert_eq!(drain(out), (100, 100));
    }

    #[test]
    fn session_hdr_mode_is_part_of_the_key() {
        let mut session = Session::new(64 * 1024 * 1024);
        let info = source_info(200, 200);
        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let sdr = config(&nodes, &info, "sdr_only", None);
        session
            .stream(Box::new(SolidSource::new(200, 200)), &sdr, None, 0x54)
            .unwrap();
        assert_eq!(session.cache_len(), 1);

        // hdr_mode decides whether a sidecar is processed into the entry, so
        // a different mode must not reuse the sdr_only entry.
        let preserve = config(&nodes, &info, "preserve", None);
        session
            .stream(Box::new(SolidSource::new(200, 200)), &preserve, None, 0x54)
            .unwrap();
        assert_eq!(session.cache_len(), 2);
    }

    #[test]
    fn session_skips_entry_larger_than_budget() {
        // 100×100 RGBA8 is ~40 KB; a 1 KB budget can never hold it. The
        // pipeline still runs, but nothing is cached and the byte
        // accounting stays at zero.
        let mut session = Session::new(1024);
        let info = source_info(200, 200);
        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let cfg = config(&nodes, &info, "sdr_only", None);
        let out = session
            .stream(Box::new(SolidSource::new(200, 200)), &cfg, None, 0x55)
            .unwrap();
        assert_eq!(drain(out), (100, 100));
        assert_eq!(session.cache_len(), 0);
        assert_eq!(session.current_bytes(), 0);
    }

    #[test]
    fn session_lru_evicts_least_recently_used_entry() {
        // Room for two ~40 KB entries, not three.
        let mut session = Session::new(100_000);
        let info = source_info(200, 200);
        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let cfg = config(&nodes, &info, "sdr_only", None);
        for key in [0xA1u64, 0xA2] {
            session
                .stream(Box::new(SolidSource::new(200, 200)), &cfg, None, key)
                .unwrap();
        }
        assert_eq!(session.cache_len(), 2);
        // Touch 0xA1 so 0xA2 becomes the LRU entry.
        session
            .stream(Box::new(PoisonSource { w: 200, h: 200 }), &cfg, None, 0xA1)
            .unwrap();
        // A third image evicts exactly one entry — the LRU one (0xA2).
        session
            .stream(Box::new(SolidSource::new(200, 200)), &cfg, None, 0xA3)
            .unwrap();
        assert_eq!(session.cache_len(), 2);
        session
            .stream(Box::new(PoisonSource { w: 200, h: 200 }), &cfg, None, 0xA1)
            .expect("0xA1 was touched and must survive");
        assert!(
            session
                .stream(Box::new(PoisonSource { w: 200, h: 200 }), &cfg, None, 0xA2)
                .is_err(),
            "0xA2 was the LRU entry and must have been evicted"
        );
    }

    // ─── prefix_chain tests ───

    #[test]
    fn prefix_chain_shares_common_prefix_and_diverges_after() {
        let identity = |hash: u64| crate::cache::SourceIdentity {
            source_hash: hash,
            width: 200,
            height: 200,
            format: RGBA8_SRGB,
            exif_orientation: 1,
            has_alpha: true,
            has_gain_map: false,
            is_hdr: false,
            hdr_mode: "sdr_only",
        };
        let a: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let b: Vec<Box<dyn zennode::NodeInstance>> = vec![
            make_constrain(100, 100),
            make_constrain(50, 50),
            make_remove_alpha(255, 255, 255),
        ];
        let ca = crate::cache::prefix_chain(&a, &identity(7));
        let cb = crate::cache::prefix_chain(&b, &identity(7));
        assert_eq!(ca.len(), 3);
        assert_eq!(cb.len(), 4);
        assert_eq!(ca[0], cb[0]);
        assert_eq!(ca[1], cb[1]);
        assert_ne!(ca[2], cb[2]);

        // Source identity changes the root and therefore every link.
        let cc = crate::cache::prefix_chain(&a, &identity(8));
        assert!(ca.iter().zip(&cc).all(|(x, y)| x != y));
    }

    // ─── stream_stoppable tests ───

    #[test]
    fn session_stream_stoppable_cancellation() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct AtomicStop(Arc<AtomicBool>);
        impl enough::Stop for AtomicStop {
            fn check(&self) -> Result<(), enough::StopReason> {
                if self.0.load(Ordering::Relaxed) {
                    Err(enough::StopReason::Cancelled)
                } else {
                    Ok(())
                }
            }
        }

        let mut session = Session::new(64 * 1024 * 1024);
        let cancel = Arc::new(AtomicBool::new(true)); // Pre-cancelled
        let stop = AtomicStop(cancel);

        let nodes: Vec<Box<dyn zennode::NodeInstance>> =
            vec![make_constrain(100, 100), make_remove_alpha(255, 255, 255)];
        let info = source_info(200, 200);
        let config = ProcessConfig {
            nodes: &nodes,
            converters: &[],
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
            limits: None,
        };
        let source = Box::new(SolidSource::new(200, 200));
        let result = session.stream_stoppable(source, &config, None, 0xCC, &stop);
        assert!(result.is_err()); // Should be cancelled
    }
}
