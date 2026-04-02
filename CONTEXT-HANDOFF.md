# Context Handoff — zenpipe WASM Editor Demo

## What was built this session

### Core pipeline caching (zenpipe)
- `src/cache.rs` — CachedPixels, CacheSource, prefix_hash, subtree_hash, geometry_split
- `src/session.rs` — Session with LRU eviction, generation counter (WASM-safe)
- `Session::stream_stoppable()` + `MaterializedSource::from_source_stoppable()` — enough::Stop cancellation
- RemoveAlpha short-circuit for opaque input (no-alpha)
- Benchmark: 32x speedup for cached suffix (92ms → 2.9ms on 4K)

### Demo WASM crate (`demo/crate/`)
- `Editor` — two Sessions (overview + detail), cancel via Arc<AtomicBool>
- `WasmEditor` — wasm-bindgen API (constructor, render_overview, render_region, get_filter_schema)
- `FiltersConverter` — bridges zenfilters nodes to pipeline NodeOps
- Generic node creation via `NodeRegistry::node_from_json()` for all 43 working filter nodes
- `pack_rgba()` uses zenpixels-convert RowConverter (SIMD-accelerated)
- 19 native tests including exhaustive filter node test

### Demo frontend (`demo/`)
- 11 ES modules under `demo/js/` (state, worker-client, toasts, css-preview, sidebar, render, region, file-load, export-modal, presets, main)
- Schema-driven slider generation from schema.json (44 filter nodes)
- Favorites section: Exposure, Contrast, Highlights/Shadows, Clarity, Brilliance, Saturation, Vibrance, Sharpen, Temperature, Dehaze, Vignette
- Collapsible groups, per-slider reset buttons, double-click reset
- CSS filter approximation for instant preview
- Mouse-drag panning with filtered upscaled preview from overview
- Scroll-to-zoom (proportional), pinch-to-zoom
- Error toasts centered in viewport with tap-to-reset + copy icon + auto-reset to last safe state
- Film preset strip with 32 presets and intensity slider
- Responsive layout (narrow mode for phones)
- Export modal with format tabs, dimension controls, aspect lock
- 21 Playwright e2e tests

### zenfilters fixes
- 15 missing node_to_filter bridge arms added (brilliance, highlights_shadows, vignette, etc.)
- film_look bridge arm added
- All 43/44 numeric filter nodes work (dt_sigmoid pending Filter impl)

## Open issues
- imazen/zenpipe#20 — Prefer RGBX8 over RGB8 as opaque working format
- imazen/zenpipe#21 — wasm-bindgen-rayon for multi-threaded WASM
- imazen/zenpipe#22 — Multi-worker encoding and srcset generation

## Remaining tasks

### Export panel (high priority)
- Export currently renders at overview size (tiny) — needs full-res rendering
- Codec-specific quality controls needed per format (see codec audit below)
- Encode preview: show small preview image with bpp/filesize/resolution overlay
- Estimate full-size from small preview
- Progress bar based on estimated time
- Cancellable encoding via enough::Stop

### Codec quality controls (from audit)
All encode nodes are in zencodecs/src/zennode_defs.rs:
- JPEG: quality [0-100, def 85], effort [0-2]
- PNG: effort [0-12, def 5], lossless always
- WebP: quality [0-100], effort [0-10], lossless toggle, near_lossless [0-100]
- JXL: quality [0-100] OR distance [0-25], effort [0-10], lossless
- AVIF: quality [1-100], effort [0-10], bit_depth, alpha_quality
- GIF: quality [1-100], dithering [0-1]

### Export system redesign (see demo/EXPORT-DESIGN.md)
- Two-panel export: quick export + advanced preservation
- Inline preview pane with bpp/filesize overlay
- Export history (collapsible, in-memory)
- Metadata/HDR/gain map preservation controls
- End-to-end pipeline for high-quality export (not just pack_rgba → encode)
- WASM codecs: JPEG (zenjpeg), WebP (zenwebp), PNG (zenpng), GIF (zengif), JXL (jxl-encoder) — all pure Rust
- AVIF: needs zenrav1e WASM compat verification (has build.rs)
- Browser fallback for unsupported formats with clear indicator

### UX polish remaining
- WASM-AUDIT.md is stale (zenfilters WASM support already done)
- dt_sigmoid needs Filter impl in zenfilters

## Build commands
```bash
# WASM
cd demo/crate && RUSTFLAGS="-C target-feature=+simd128" wasm-pack build --target web --out-dir ../pkg --release

# Rust tests
cd demo/crate && cargo test --no-default-features --lib

# Playwright
cd demo && npx playwright test tests/editor.spec.js

# Dev server
cd demo && python3 -m http.server 3847
```

## Local build note
Cargo.toml needs imageflow/zensally deps stripped for local builds (CI does this automatically via sed). The demo crate has its own Cargo.toml with [patch.crates-io] for zennode.
