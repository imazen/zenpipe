# Context Handoff — zenpipe WASM Editor Demo

**Session: 55 commits, ~800K tokens**

## Live Demo
https://imazen.github.io/zenpipe/ (GitHub Pages, auto-deploys on push)

## What was built

### Core pipeline caching (zenpipe)
- `src/cache.rs` — CachedPixels, CacheSource, prefix_hash, subtree_hash, geometry_split
- `src/session.rs` — Session with LRU eviction, generation counter (WASM-safe)
- `Session::stream_stoppable()` + `MaterializedSource::from_source_stoppable()` — enough::Stop cancellation
- RemoveAlpha short-circuit for opaque input
- Benchmark: 32x speedup (92ms → 2.9ms on 4K)

### Demo WASM crate (`demo/crate/`, 5.6 MB module)
- **Editor** — two Sessions (overview + detail), cancel via Arc<AtomicBool>
- **Encode** — 6 codecs: JPEG (zenjpeg), WebP (zenwebp), PNG (zenpng), GIF (zengif), JXL (jxl-encoder), AVIF (zenavif) — all pure Rust WASM
- **Decode** — JXL + AVIF WASM decode for browsers that lack native support
- **FiltersConverter** — bridges zenfilters to pipeline NodeOps via NodeRegistry
- **43/44 filter nodes** working (dt_sigmoid pending Filter impl)
- `pack_rgba()` uses zenpixels-convert RowConverter (SIMD-accelerated)
- 30 native tests

### Demo frontend (`demo/`, 11 ES modules)
- **Favorites** section: Exposure, Contrast, Highlights/Shadows, Clarity, Brilliance, Saturation, Vibrance, Sharpen, Temperature, Dehaze, Vignette
- **Node-grouped sliders**: single-param nodes show node title, multi-param show header + param labels
- **Layout**: label + value/reset above, slider below (both desktop and mobile)
- **CSS filter approximation** for instant preview
- **Mouse-drag panning** with filtered upscaled preview from overview
- **Scroll-to-zoom** (proportional), **pinch-to-zoom**, **pixelated mode at >6x**
- **DPR-aware pixel ratio** display with device pixel accuracy
- **Error toasts** centered in viewport with tap-to-reset + copy icon
- **Film preset strip** (32 presets + intensity slider)
- **Toggleable minimap** and **crop region selector**
- **Export modal** with 6 format tabs (all showing "zen" badge), per-format quality controls
- **Responsive layout** (narrow mode for phones)
- **Picsum photo picker** (4000x3000 images)
- 21 Playwright tests

### CI/CD
- GitHub Pages auto-deploy on push (`.github/workflows/demo-deploy.yml`)
- Clones only 3 unpublished repos (zennode, zenfilters, zencodecs)
- All codec deps from crates.io

## Open issues
- imazen/zenpipe#20 — Prefer RGBX8 over RGB8
- imazen/zenpipe#21 — wasm-bindgen-rayon for multi-threaded WASM
- imazen/zenpipe#22 — Multi-worker encoding and srcset generation

## Next: End-to-End Pipeline (see `demo/E2E-PIPELINE-PLAN.md`)
6 phases: rich decode → Editor::from_bytes → WASM API → encode with metadata → two-phase worker decode → edit presets

## Build commands
```bash
cd demo/crate && RUSTFLAGS="-C target-feature=+simd128" wasm-pack build --target web --out-dir ../pkg --release
cd demo/crate && cargo test --no-default-features --lib
cd demo && npx playwright test tests/editor.spec.js
cd demo && python3 serve.py 3847
```
