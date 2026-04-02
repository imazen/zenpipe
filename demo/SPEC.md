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
**Goal**: all processing — decode, filter, encode — runs through zenpipe natively in WASM, preserving metadata/HDR/gain maps at every stage. Browser decode is only for instant preview.

- **Phase 1**: Rich decode (`try_decode_rich`) preserving `zencodec::Metadata` (ICC, EXIF, XMP, CICP), `ImageInfo`, gain map info
- **Phase 2**: `Editor::from_bytes()` — decode internally, content-based source_hash, metadata on Editor struct
- **Phase 3**: WASM API `WasmEditor::from_bytes()` with `has_metadata`, `source_format` getters
- **Phase 4**: Encode with metadata passthrough (ICC, EXIF, XMP per codec via `encode_with_metadata()`)
- **Phase 5**: Two-phase worker decode — browser decode → instant preview (send `ready`), WASM `from_bytes()` in background → replace editor (send `upgrade`), main thread re-renders on upgrade
- **Phase 6**: Edit presets (see §4.6)

### 1.4 RGBX8 Working Format ⬜ (issue #20)
- RGB8 has zero SIMD in zenresize horizontal filter (3-5x penalty)
- Pipeline should use RGBX8 (4 bpp) for opaque images internally
- Encoders accept RGBX8 natively (zenjpeg, etc.)
- Canvas display wants RGBA anyway

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
| HEIC  | ⬜ heic crate (pure Rust, on crates.io) | ✅ Safari only | ⬜ |

### 2.3 Metadata Preservation ⬜
- ICC profile passthrough (decode → pipeline → encode)
- EXIF preservation (orientation, camera info)
- XMP passthrough (editing metadata)
- CICP signaling for HDR content
- Gain map sidecar tracking (UltraHDR / ISO 21496-1 / JXL jhgm)
- **User controls**: per-metadata-type checkboxes (preserve/discard) in export panel
- **Default**: preserve everything — no data loss unless user explicitly discards

---

## 3. Export System

### 3.1 Full-Resolution Export ✅
- `render_at_size(max_dim)` renders at requested dimensions
- Width/height controls with aspect lock
- All 6 codecs encode through WASM (not browser `convertToBlob`)
- "zen" engine badge on each format tab

### 3.2 Inline Encode Preview ⬜
- Encode at overview size (~512px) with current settings — near-instant
- Display encoded preview image inline in the export modal (below format controls)
- Overlay stats: bits per pixel, resolution, file size
- "Est. full size: 2.4 MB" extrapolated from preview's compression ratio × full pixel count
- **Live updates** as quality/effort sliders change (debounced ~200ms)
- If browser supports the format, render the decoded preview so user sees actual artifacts
- For unsupported formats (JXL in Chrome), show the overview with stats overlay only

### 3.3 Export History ⬜
- Collapsible section below/beside export modal
- Each entry: thumbnail, format badge, file size, dimensions, bpp, timestamp
- **View in browser** — click to open full-size in a new tab (blob URL, no download)
- **Download** button per entry
- **Compare** — show two entries side by side with file size / bpp difference
- "Clear History" button
- In-memory by default (lost on refresh); optionally persist to IndexedDB for large images
- Shows compression ratio trend across attempts

### 3.4 Progress Bar ⬜
- Estimated time from `TIME_PER_KPIX × megapixels × effort_factor`
- Strip-count progress: `(strips_processed / total_strips)` as pipeline streams
- Cancellable: "Cancel" button triggers `enough::Stop`, returns to modal
- Shows elapsed time and estimated remaining
- Smooth animation (don't jump — interpolate between strip completions)

### 3.5 Advanced Preservation Panel ⬜
- **Metadata toggles**: EXIF, XMP, ICC, CICP — each a checkbox, all on by default
- **HDR / Gain Map** (when source has gain map):
  - "Preserve gain map" → embeds for UltraHDR/ISO 21496-1
  - "Discard gain map" → SDR-only output
  - "Reconstruct HDR" → apply gain map for HDR output
  - Preview: show gain-mapped version in browser (if supported)
- **Color space**: sRGB (default) / Display P3 / Rec.2020
- **Bit depth**: 8-bit (default) / 16-bit (PNG, JXL, AVIF) / Float (JXL)
- **Format-specific advanced**: JPEG subsampling, WebP near-lossless, JXL distance mode

### 3.6 Export UX Principles
1. **Try fast, decide slow**: preview encodes are instant (~50ms), users flip between formats/qualities rapidly
2. **Show, don't tell**: inline preview with artifacts visible at zoom
3. **No data loss by default**: metadata preserved unless explicitly discarded
4. **History enables comparison**: same image at JPEG 75 vs WebP 80, compare sizes and quality
5. **Clear engine indicator**: "zen" (blue) or "browser" (orange) badge per format
6. **Never block the editor**: exports run in background, user can keep editing

---

## 4. Filter UI

### 4.1 Favorites Section ✅
- Pinned expanded group: Exposure, Contrast, Highlights/Shadows, Clarity, Brilliance, Saturation, Vibrance, Sharpen, Temperature, Dehaze, Vignette
- Curated order (not alphabetical)

### 4.2 Node-Grouped Sliders ✅
- Single-param nodes: node title as label (e.g., "Clarity" not "Amount")
- Multi-param nodes: node title header + param labels beneath
- Label + value/reset on top line, slider below (both layouts)

### 4.3 Schema-Driven Generation ✅
- All 44 numeric filter nodes from schema.json
- Groups: Favorites, Tone, Tone Range, Tone Map, Color, Detail, Effects, Auto
- Collapsible groups (Favorites expanded by default)

### 4.4 Zennode UX Metadata Integration 🔧
Currently using: `x-zennode-group`, `x-zennode-identity`, `x-zennode-step`, `minimum`, `maximum`, `default`, `title`, `description`.

Not yet using:
- **`x-zennode-slider`** ⬜ — slider type affects UX:
  - `linear`: direct mapping (default)
  - `square_from_slider`: square the slider position (finer control near zero)
  - `factor_centered`: centered at 1.0 (e.g., Saturation factor)
- **`x-zennode-unit`** ⬜ — show unit after value (e.g., "1.5 EV", "2.0×")
- **`x-zennode-section`** ⬜ — sub-group params within a node (e.g., "Main" vs "Advanced")
- **`x-zennode-visible-when`** ⬜ — conditional visibility (e.g., near-lossless only when lossless is on)
- **`x-zennode-optional`** ⬜ — nullable params, show enable/disable toggle
- **`x-zennode-tags`** ⬜ — for search/filter in the slider panel
- **`x-zennode-labels`** ⬜ — for array params (e.g., tone curve points, mixer weights)

### 4.5 Slider UX ✅
- Per-slider reset button (visible only when user has touched it and value differs from identity)
- Double-click slider to reset to identity
- CSS filter approximation for instant preview (exposure → brightness, contrast, saturation)

### 4.6 Film Presets ✅
- 32 presets across 6 categories (creative, classic negative, slide, motion picture, digital, cinematic)
- Intensity slider (0-1)
- Collapsible section

### 4.7 User Presets / Edit Serialization ⬜
- **Save**: current adjustments + film preset + intensity → named preset
- **Load**: apply preset, update all sliders
- **Delete**: remove from storage
- **JSON format**: `{name, version, film_preset, film_preset_intensity, adjustments: {"zenfilters.exposure": {"stops": 1.5}, ...}}`
- **Querystring format**: compact URL-safe encoding for sharing / server API
  - Compatible with zenpipe/imageflow RIAPI querystring API
  - e.g., `?s.brightness=1.5&s.contrast=0.3&film=portra&fi=0.8`
- **localStorage persistence**: presets survive page refresh
- **Import/export**: download as JSON file, upload to restore
- **Per-image edit state**: optionally store last-used adjustments per image hash in IndexedDB
- **UI**: "Save Preset" button in sidebar, preset list with load/delete

---

## 5. Navigation & Interaction

### 5.1 Core Principle: Never Show Unfiltered ✅
During any interaction (drag, zoom, slider change), the user always sees the filtered image. The overview canvas (already rendered with current filters) is upscaled into the detail view as an instant preview. The worker-rendered sharp version replaces it when ready.

### 5.2 Detail View ✅
- Mouse-drag panning with filtered upscaled preview from overview
- Explicit CSS width/height to fill viewport (browser upscales small canvases)
- `image-rendering: pixelated` at >6x upscale

### 5.2.1 Original vs Edited Toggle ⬜
- Hold-to-compare: hold a key (e.g., `\` or spacebar) or long-press a button to show the original (unedited) image
- **Release returns to edited** — no flicker, no re-render delay
- Implementation: keep a cached unedited render (overview + detail at current region) alongside the edited one
- During drag/zoom: always show filtered (edited) — the original toggle is only for static comparison
- Visual indicator: "Original" badge overlaid when showing unedited
- The unedited render uses the same Session with empty adjustments (cache hit on geometry prefix, no filter suffix)
- Must not interfere with drag/zoom — if user is panning and hits the toggle key, ignore it until panning stops

### 5.3 Zoom ✅
- Scroll-to-zoom (proportional deltaY, smooth trackpad + discrete wheel)
- Pinch-to-zoom on touch
- Minimum region 1% of image (up to ~100:1 zoom)

### 5.4 Pixel Ratio Display ✅
- Large bold 22px text below image
- Color-coded: green 1:1, white downscale, orange upscale, red >4x upscale
- Shows DPR when > 1 (e.g., "@2.0x")
- **1:1 = 1 source pixel per device pixel** (DPR-aware)
- Clickable to reset to 1:1
- Shows source region dimensions and full image dimensions

### 5.5 Minimap (Overview) ✅
- Toggleable via "minimap" button (bottom-right of detail view)
- Smooth collapse/expand animation
- Shows blurred overview, sharpens after render
- Click to reposition detail view center

### 5.6 Crop Region Selector ✅
- Toggleable via "crop" button on overview
- Draggable rectangle to move detail view
- Smart initial sizing (viewport-aware, aspect clamped to 4:3/3:4, capped at max pixels)

### 5.7 Detail Render Resolution
- Requests `maxDim = viewport_css_size × DPR` from pipeline
- Capped at `DETAIL_MAX × DPR` to avoid excessive render time
- Pipeline renders at native crop size when zoomed in (no upscale compute — browser does it)
- Canvas CSS size explicitly set to fill viewport

---

## 6. Error Handling

### 6.1 Error Toasts ✅
- Centered in viewport (large, prominent, semi-transparent rounded box)
- Clickable "Tap to reset" with clipboard copy icon
- Auto-dismiss after 3 seconds with auto-reset to last safe state
- `lastSafeAdjustments` snapshot after each successful render

### 6.2 Slider Error Recovery ✅
- Track last changed slider key
- On render error: toast shows, auto-resets to last safe state
- Gray out problematic slider for 3 seconds

### 6.3 Image Decode Errors 🔧
- Prominent centered display (not status bar text)
- Fallback chain: WASM decode (JXL/AVIF) → browser `createImageBitmap` → WASM last resort → error
- Reports decoder used (`wasm`, `browser`, `wasm-fallback`)

### 6.4 Export Errors ⬜
- Show in export modal (not just status bar)
- "Retry" button with same settings
- Suggest alternative format if one fails

---

## 7. Layout & Responsiveness

### 7.1 Desktop Layout ✅
- Grid: main viewport (detail + overview) | sidebar (280px)
- Sidebar: Reset button, favorites, filter groups, film presets

### 7.2 Mobile/Narrow Layout ✅
- Single column below 840px
- Sidebar moves below viewport (max 40vh, scrollable)
- Consistent slider layout (label above, slider below)

### 7.3 Photo Picker ✅
- Picsum sample photos (4000×3000 for quality)
- Popover when editor is active, inline when dropzone is showing
- "Pick" button in header next to "Open"

### 7.4 Loading State ✅
- Large centered "Loading..." message in detail viewport
- Shown during image decode, hidden when render completes

---

## 8. Deployment

### 8.1 GitHub Pages ✅
- Auto-deploy on push to main (`.github/workflows/demo-deploy.yml`)
- Clones 3 unpublished repos (zennode, zenfilters, zencodecs)
- All codec deps from crates.io
- WASM build with simd128
- Live at https://imazen.github.io/zenpipe/

### 8.2 Vercel 🔧 (workflow ready, needs secrets)
- Native COOP/COEP headers for SharedArrayBuffer
- Same WASM build, deployed in parallel with Pages
- Needs VERCEL_TOKEN, VERCEL_ORG_ID, VERCEL_PROJECT_ID secrets

### 8.3 WASM Module
- 5.6 MB with 6 encode + 2 decode codecs + 43 filters + pipeline
- simd128 enabled
- Pure Rust (no C FFI, no asm on WASM)

---

## 9. Future / Tracked Issues

### 9.1 Multi-Worker Encoding (issue #22) ⬜
- Worker pool: 2-4 workers, each with own WASM Editor
- Primary worker handles interactive rendering (overview + detail)
- Export workers handle full-res encode jobs in background
- Job queue with priority (interactive > export)
- User can keep editing while exports run
- Cancel support via enough::Stop per worker

### 9.2 wasm-bindgen-rayon (issue #21) ⬜
- SharedArrayBuffer + Web Workers for parallel WASM execution
- Requires COOP/COEP headers (Vercel has them; Pages needs coi-serviceworker)
- Would enable multi-threaded resize (zenresize), filter (zenfilters), blend (zenblend)
- Significant speedup for large images

### 9.3 Srcset Generator (issue #22) ⬜
- User picks target widths (or presets: thumbnail, mobile, desktop, retina)
- Each width × format combination is an encode job
- Progress bar for batch exports
- Output as zip or individual downloads
- Generate `<img srcset>` HTML snippet

### 9.4 SVG Rendering (issue #1) ⬜
- resvg/usvg as image source via zenpipe

### 9.5 HEIC Decode ⬜
- Pure Rust `heic` crate (on crates.io)
- Add to decode.rs alongside JXL/AVIF

### 9.6 Display P3 Canvas ⬜
- Detect `canvas.getContext('2d', { colorSpace: 'display-p3' })` support
- Pass `PixelDescriptor` with `ColorPrimaries::DisplayP3` to `pack_rgba`
- RowConverter handles sRGB → P3 conversion
- Show wide-gamut indicator when active

### 9.7 End-to-End Streaming ⬜
- Decode → Session → encode without full materialization
- Pipeline streams strips directly to encoder
- Reduces peak memory for large images

---

## 10. Testing

### 10.1 Rust Tests ✅ (30 tests)
- Editor: init, render, cache hit/miss, cancellation, cancel independence
- Encode: JPEG, WebP, PNG, GIF, JXL, AVIF magic bytes + format metadata
- Decode: format detection (JPEG, JXL, AVIF)
- Filter recognition: exhaustive test of all 44 schema nodes (43 pass)

### 10.2 Playwright E2E ✅ (21 tests)
- Page load, schema load, filter count
- Image load, editor UI, sliders visible
- Overview + detail canvas have pixels
- Slider change triggers re-render
- Reset button resets all sliders
- Mouse drag panning on detail canvas
- Overview click repositions region
- Export modal: open, download JPEG, close on Escape, format switching, aspect lock
- Double-click slider reset
- Backend type shown in status
- Multiple simultaneous slider changes
- Pixel info below detail canvas
- Pick button visible, reset button in sidebar
- **Pixel verification**: non-zero pixels survive slider changes (overview + detail)

### 10.3 Missing Tests ⬜
- Export at full resolution (verify output dimensions match requested)
- JXL/AVIF decode (load a JXL file, verify pixels)
- Film preset rendering (select preset, verify output differs from no-preset)
- Zoom past 1:1 (verify ratio display shows upscale)
- Pinch-to-zoom (needs mobile emulation)
- Error toast display and auto-reset
- Responsive layout (narrow viewport)
- Export with each format (JPEG, WebP, PNG, JXL, AVIF, GIF)
