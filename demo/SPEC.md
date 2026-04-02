# zenpipe WASM Editor — Feature Spec

## Status Key
- ✅ Done
- 🔧 Partially done
- ⬜ Not started

---

## 1. Pipeline Architecture

### 1.1 Session Caching ✅
- Merkle-style subtree hashing on the DAG
- Geometry prefix cached (decode + crop + resize)
- Filter suffix re-runs from cache on parameter changes (~3ms vs ~90ms)
- LRU eviction with configurable memory budget
- Generation counter (WASM-safe, no `Instant`)

### 1.2 Cooperative Cancellation ✅
- `enough::Stop` threaded through `Session::stream_stoppable()` and `MaterializedSource::from_source_stoppable()`
- `Editor` uses `Arc<AtomicBool>` per render — new render cancels in-flight work
- 50ms JS debounce prevents worker message backlog

### 1.3 End-to-End Native Pipeline ⬜
- **Phase 1**: Rich decode (`try_decode_rich`) preserving metadata, ImageInfo, gain map
- **Phase 2**: `Editor::from_bytes()` — decode internally, content-based source_hash
- **Phase 3**: WASM API `WasmEditor::from_bytes()`
- **Phase 4**: Encode with metadata passthrough (ICC, EXIF, XMP per codec)
- **Phase 5**: Two-phase worker decode (browser preview → WASM upgrade)
- **Phase 6**: Edit presets (JSON serialization, localStorage, querystring)

### 1.4 RGBX8 Working Format ⬜ (issue #20)
- RGB8 has zero SIMD in zenresize horizontal filter (3-5x penalty)
- Pipeline should use RGBX8 (4 bpp) for opaque images
- Encoders accept RGBX8 natively (zenjpeg, etc.)

---

## 2. Codec Support

### 2.1 Encoding ✅
| Codec | Engine | Quality | Effort | Lossless | Status |
|-------|--------|---------|--------|----------|--------|
| JPEG  | zenjpeg | 0-100 | 0-2 | No | ✅ |
| WebP  | zenwebp | 0-100 | 0-10 | Yes | ✅ |
| PNG   | zenpng | — | 0-12 | Always | ✅ |
| GIF   | zengif | 0-100 | — | Palette | ✅ |
| JXL   | jxl-encoder | 0-100 | 0-10 | Yes | ✅ |
| AVIF  | zenavif | 0-100 | 0-10 | Yes | ✅ |

### 2.2 Decoding 🔧
| Codec | WASM Decode | Browser Decode | Status |
|-------|------------|---------------|--------|
| JPEG  | — | ✅ native | ✅ |
| PNG   | — | ✅ native | ✅ |
| WebP  | — | ✅ native | ✅ |
| GIF   | — | ✅ native | ✅ |
| JXL   | ✅ zenjxl-decoder | ❌ Chrome | ✅ |
| AVIF  | ✅ zenavif | ✅ Chrome/Safari | ✅ |
| HEIC  | ⬜ heic crate | ✅ Safari only | ⬜ |

### 2.3 Metadata Preservation ⬜
- ICC profile passthrough (decode → encode)
- EXIF preservation (orientation, camera info)
- XMP passthrough
- CICP signaling for HDR
- Gain map sidecar tracking (UltraHDR / ISO 21496-1)
- User controls: checkboxes to preserve/discard each metadata type

---

## 3. Export System

### 3.1 Full-Resolution Export ✅
- `render_at_size(max_dim)` renders at requested dimensions
- Width/height controls with aspect lock
- All 6 codecs encode through WASM (not browser `convertToBlob`)
- "zen" badge on each format tab

### 3.2 Inline Encode Preview ⬜
- Small preview encoded at overview size (~512px)
- Displayed in the export modal below format controls
- Overlay stats: bits per pixel, resolution, file size
- "Est. full size" extrapolated from preview compression ratio
- Updates live as quality/effort sliders change

### 3.3 Export History ⬜
- Collapsible section below export modal
- Each entry: thumbnail, format badge, file size, dimensions, timestamp
- Click to view full-size (blob URL, no download)
- Download button per entry
- Compare compression ratios across attempts
- In-memory (lost on refresh)

### 3.4 Progress Bar ⬜
- Based on estimated time to completion
- Strip-count-based progress (total strips known from dimensions)
- Cancellable via enough::Stop
- Shows elapsed time and estimated remaining

### 3.5 Advanced Preservation Panel ⬜
- Toggle: preserve/discard EXIF, XMP, ICC, CICP
- HDR mode: preserve gain map / discard / reconstruct HDR
- Color space: sRGB / Display P3 / Rec.2020
- Bit depth: 8 / 16 / float (format-dependent)
- Preview gain-mapped version in browser

---

## 4. Filter UI

### 4.1 Favorites Section ✅
- Pinned expanded group: Exposure, Contrast, Highlights/Shadows, Clarity, Brilliance, Saturation, Vibrance, Sharpen, Temperature, Dehaze, Vignette
- Curated order (not alphabetical)

### 4.2 Node-Grouped Sliders ✅
- Single-param nodes: node title as label (e.g., "Clarity" not "Amount")
- Multi-param nodes: node title header + param labels beneath
- Label + value/reset on top line, slider below

### 4.3 Schema-Driven Generation ✅
- All 44 numeric filter nodes from schema.json
- Groups: Tone, Tone Range, Tone Map, Color, Detail, Effects, Auto
- Collapsible groups (Favorites expanded by default)

### 4.4 Slider UX ✅
- Per-slider reset button (visible when changed)
- Double-click to reset to identity
- CSS filter approximation for instant preview (exposure, contrast, saturation)

### 4.5 Film Presets ✅
- 32 presets across 6 categories
- Intensity slider (0-1)
- Collapsible section

### 4.6 Filter Presets / User Presets ⬜
- Save current adjustments as named preset
- Load/delete presets
- JSON serialization: `{name, version, film_preset, adjustments}`
- Querystring serialization for URL sharing
- Compatible with zenpipe/imageflow server querystring API
- localStorage persistence
- Import/export preset files

---

## 5. Navigation & Interaction

### 5.1 Detail View ✅
- Mouse-drag panning with filtered upscaled preview
- Never shows unfiltered original during drag
- CSS-upscaled canvas for zoom past 1:1

### 5.2 Zoom ✅
- Scroll-to-zoom (proportional deltaY, not binary)
- Pinch-to-zoom on touch
- Minimum region 1% of image (up to ~100:1 zoom)
- Pixelated rendering at >6x upscale

### 5.3 Pixel Ratio Display ✅
- Large bold text below image
- Color-coded: green (1:1), white (downscale), orange (upscale), red (>4x)
- Shows DPR when > 1 (e.g., "@2.0x")
- Clickable to reset to 1:1 (1 source pixel = 1 device pixel)
- Device-pixel-aware calculations

### 5.4 Minimap (Overview) ✅
- Toggleable via "minimap" button
- Smooth collapse/expand animation
- Click to reposition detail view

### 5.5 Crop Region Selector ✅
- Toggleable via "crop" button on overview
- Draggable to move detail view
- Smart initial sizing (viewport-aware, capped at 1920×1080 source pixels)

---

## 6. Error Handling

### 6.1 Error Toasts ✅
- Centered in viewport (large, prominent)
- Clickable "Tap to reset" with clipboard copy icon
- Auto-dismiss after 3 seconds
- Resets to last safe adjustment state

### 6.2 Slider Error Recovery ✅
- Track last changed slider
- On render error: auto-reset to last safe state
- Gray out problematic slider for 3 seconds

### 6.3 Image Decode Errors 🔧
- Prominent centered display
- Fallback chain: browser → WASM → error message
- Partially working (WASM fallback for JXL/AVIF)

---

## 7. Layout & Responsiveness

### 7.1 Desktop Layout ✅
- Grid: main viewport (detail + overview) | sidebar (280px)
- Sidebar: Reset button, filter groups, film presets

### 7.2 Mobile/Narrow Layout ✅
- Single column below 840px
- Sidebar moves below viewport (max 40vh, scrollable)
- Consistent slider layout (label above, slider below)

### 7.3 Photo Picker ✅
- Picsum sample photos (4000×3000)
- Popover when editor is active, inline when dropzone is showing
- "Pick" button in header

---

## 8. Deployment

### 8.1 GitHub Pages ✅
- Auto-deploy on push to main
- Clones 3 unpublished repos (zennode, zenfilters, zencodecs)
- All codec deps from crates.io
- WASM build with simd128

### 8.2 Vercel ⬜ (workflow ready, needs secrets)
- Native COOP/COEP headers for SharedArrayBuffer
- Same build, deployed in parallel with Pages

### 8.3 WASM Module
- 5.6 MB with 6 encode + 2 decode codecs + 43 filters + pipeline
- simd128 enabled

---

## 9. Future / Tracked Issues

### 9.1 Multi-Worker Encoding (issue #22)
- Worker pool: 2-4 workers for parallel encode
- Primary worker handles interactive rendering
- Export workers handle full-res jobs
- Job queue with priority

### 9.2 wasm-bindgen-rayon (issue #21)
- SharedArrayBuffer + Web Workers for parallel WASM
- Requires COOP/COEP headers
- Would enable multi-threaded resize, filter, blend

### 9.3 Srcset Generator (issue #22)
- Multiple widths × formats
- Progress bar for batch exports
- Output as zip or individual downloads

### 9.4 SVG Rendering (issue #1)
- resvg/usvg as image source

### 9.5 HEIC Decode
- Pure Rust `heic` crate (exists locally, on crates.io)
- Needs integration into decode.rs

---

## 10. Testing

### 10.1 Rust Tests ✅
- 30 tests: editor, encode (7 codecs), decode, filter node recognition, cancellation

### 10.2 Playwright E2E ✅
- 21 tests: load, sliders, drag, zoom, export, reset, pixel verification

### 10.3 Exhaustive Filter Test ✅
- Iterates all 44 schema nodes on 32×32 image
- 43/44 pass (dt_sigmoid pending Filter impl)
