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

### 5.2.1 Original vs Edited Compare ⬜
- **Mobile-first**: tap-and-hold the image to see original, release to see edited
  - Touch: `touchstart` on detail canvas → show original after 200ms hold (distinguishes from drag start)
  - If finger moves > 10px before 200ms, it's a drag — cancel the compare
  - `touchend` → snap back to edited instantly
- **Desktop**: same tap-hold works with mouse; also `\` key as shortcut (hold = original, release = edited)
- **Visual**: large "ORIGINAL" badge centered on the image while showing unedited; fades instantly on release
- **Implementation**: keep a pre-rendered unedited canvas (or ImageData) alongside the edited one
  - Render original using same Session with empty adjustments → geometry prefix cache hit, no filter suffix → fast
  - Swap canvas content (not CSS filter removal — that would show wrong result for non-CSS-approximated filters)
  - Original render updates when region changes (drag/zoom), but only when compare is not active
- **Never interfere with interaction**: if user is dragging/zooming, ignore compare gesture entirely
- **Split-screen option** (future): swipe a divider left/right to reveal original on one side

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

---

## 11. Geometry & Document Tools

### 11.1 Step Groups / Pipeline Stages ⬜
The editor should organize operations into distinct pipeline stages, each with its own UI section:

1. **Geometry** — crop, rotate, flip, resize, deskew, perspective
2. **Document** — whitespace crop, deskew, perspective correction, margins
3. **Raw Development** — white balance, exposure recovery, noise reduction, lens correction
4. **Filters** — the current filter panel (tone, color, detail, effects)
5. **Export** — format, quality, metadata

Each stage applies in order. The Session Merkle cache means changing a filter doesn't re-run geometry. Changing geometry invalidates everything downstream.

### 11.2 Crop Interface ⬜
- **Freeform crop**: drag handles on corners/edges
- **Aspect ratio presets**: 1:1, 4:3, 3:2, 16:9, 5:4, custom
- **CMS crop sets**: define multiple named aspect ratio crops per image
  - e.g., "hero" (16:9), "thumbnail" (1:1), "social" (4:5)
  - Saved as JSON — not editing the image, just defining crop regions
  - Each crop set can be exported independently
  - Compatible with imageflow/zenpipe server crop API
- **Rule of thirds overlay** (toggle)
- **Crop to content**: auto whitespace detection via `zenpipe.crop_whitespace` node

### 11.3 Rotate & Flip ⬜
- **90° rotate** left/right buttons
- **Flip** horizontal/vertical buttons
- **Arbitrary rotation**: slider or text input (0-360°)
  - Needs new `zenlayout` node or zenfilters warp
  - Preview shows rotated image with transparent/fill corners
- **Auto-straighten**: detect horizon line and auto-rotate to level
  - Uses edge detection heuristics

### 11.4 Affine Transform / Deskew ⬜
- **Perspective correction**: 4-corner drag to correct keystoning
  - Useful for documents, architecture, whiteboards
- **Auto-deskew**: detect dominant lines and correct rotation
  - Common for scanned documents
- **Warp/distortion correction**: lens distortion, barrel/pincushion
  - Needs new zenfilters or zenlayout node

### 11.5 Document Mode ⬜
Specialized tools for document/whiteboard photos:
- **Auto whitespace crop** — detect content bounds, trim margins
- **Auto deskew** — straighten rotated scans
- **Auto contrast/levels** — normalize text contrast
- **Binarize** — threshold for B&W documents
- **Shadow removal** — flatten lighting for whiteboard captures
- **Perspective to rectangle** — 4-point transform for tilted documents

---

## 12. Procedural Edit System

### 12.1 Core Concept ⬜
Edits are a **procedural recipe** — a JSON document describing a sequence of operations that can be applied to any image. The recipe is independent of the source image.

```json
{
  "name": "golden sunset edit",
  "version": 1,
  "created": "2026-04-03T01:23:45Z",
  "stages": {
    "geometry": {
      "crop": { "x": 0.1, "y": 0.05, "w": 0.8, "h": 0.9 },
      "rotate": 2.5,
      "flip_h": false
    },
    "filters": {
      "zenfilters.exposure": { "stops": 0.5 },
      "zenfilters.contrast": { "amount": 0.3 },
      "zenfilters.temperature": { "shift": 0.15 }
    },
    "film_preset": "golden_hour",
    "film_preset_intensity": 0.7
  },
  "crop_sets": {
    "hero": { "aspect": "16:9", "anchor": "center" },
    "thumb": { "aspect": "1:1", "anchor": "face" },
    "social": { "aspect": "4:5", "anchor": "center" }
  }
}
```

### 12.2 Reusable Across Files ⬜
- User can **switch source images** while keeping all edits intact
- Edits auto-reapply to the new source (Session cache invalidates on source change)
- Crop regions use normalized coordinates (0..1) so they work at any resolution
- "Apply to batch" — select multiple files, apply the same recipe

### 12.3 Edit History & Naming ⬜
- **Auto-named edit sets**: two-word combo + datetime
  - e.g., "amber-fox 2026-04-03 01:23", "coral-wave 2026-04-03 14:05"
  - Word pairs from curated lists (adjective + noun, ~200 combos)
- **Auto-save**: every significant edit (slider release, preset change) snapshots the recipe
- **History list**: collapsible panel showing recent auto-named edits
  - Click to restore that edit state
  - Shows thumbnail + name + timestamp
  - Can rename, pin, or delete entries

### 12.4 Per-Image Edit Persistence ⬜
- **Per-image edits**: when a file is loaded, check if we have saved edits for it
  - Key by content hash (first 4KB + file size + last-modified)
  - Restore last edit state automatically
  - "Start fresh" button to discard saved edits
- **Cross-session**: edits survive page refresh and browser restart

### 12.5 Storage Architecture ⬜
```
localStorage:
  zenpipe-recent-edits: [{name, recipe_json, timestamp}, ...]  (last 50)
  zenpipe-user-presets: [{name, recipe_json}, ...]             (explicit saves)

IndexedDB (zenpipe-edits):
  image-edits: { content_hash → recipe_json }                 (per-image)
  edit-history: { id → {name, recipe_json, timestamp, thumb} } (full history)

Remote API (optional, future):
  GET  /api/edits/:hash    → recipe_json
  POST /api/edits/:hash    → save recipe
  GET  /api/presets         → user preset list
  POST /api/presets         → save preset
```

### 12.6 UX Flow ⬜
```
User opens image
  → Check IndexedDB for content_hash
  → Found: "Restore 'amber-fox' edit from 2h ago?" [Restore] [Start Fresh]
  → Not found: start with identity adjustments

User edits image
  → Every slider release / preset change: auto-snapshot to history
  → History sidebar shows: "coral-wave 01:23" "amber-fox 01:20" ...
  → User can click any history entry to restore

User wants to apply same edit to another image
  → Click "Switch Image" (or drop new file)
  → Edits stay intact, source changes
  → Pipeline re-renders with new source + same recipe
  → Session cache: geometry prefix invalidates (new source), filter suffix may hit cache

User wants to save a reusable preset
  → Click "Save as Preset" → name it → stored in localStorage + IndexedDB
  → Available in "My Presets" section alongside film presets
  → Can export as JSON file for sharing
```

---

## 13. Existing Geometry Capabilities (Inventory)

Almost everything needed already exists — it just needs to be exposed in the demo UI.

### 13.1 zenfilters — Warp & Transform (FULLY IMPLEMENTED)
`src/filters/warp.rs` (1566 lines) + `warp_simd.rs` (2019 lines):

| Feature | API | SIMD | WASM | Notes |
|---------|-----|------|------|-------|
| Arbitrary rotation | `Rotate` struct | AVX2, NEON, scalar | ✓ scalar | 4 border modes: Crop, Deskew, FillClamp, Fill |
| Affine transform | `Warp::affine()` | AVX2, NEON, scalar | ✓ scalar | 2×3 matrix (rotate+scale+shear+translate) |
| Perspective/homography | `Warp::projective()` | scalar only | ✓ scalar | 3×3 projective matrix |
| Cardinal rotation | `Warp::rotate_90/180/270()` | pixel-perfect copy | ✓ | No interpolation |
| Deskew | `Warp::deskew()` | via rotate | ✓ | White bg + Lanczos3 |

**Zennode defs** (behind `experimental` feature flag):
- `zenfilters.rotate`: angle (-360..360°), mode (crop/deskew/fillclamp/fillblack)
- `zenfilters.warp`: 3×3 matrix, background, interpolation (bilinear/bicubic/robidoux/lanczos3)

### 13.2 zenfilters — Document Module (IMPLEMENTED)
`src/document/` directory:

| Feature | File | Function | Status |
|---------|------|----------|--------|
| Skew detection | `deskew.rs` | `detect_skew_angle()` | ✅ ~0.05° accuracy |
| Homography computation | `homography.rs` | `compute_homography()` | ✅ DLT 4-point |
| Document rectification | `homography.rs` | `rectify_quad()` | ✅ corners → rectangle |
| Quad/boundary detection | `quad.rs` | LSD + polygon fitting | ✅ |
| Line segment detection | `lsd.rs` | LSD algorithm | ✅ |
| Binarization | `otsu.rs` | `otsu_threshold()`, `binarize()` | ✅ |

### 13.3 zenlayout — Orientation & Layout (IMPLEMENTED)
- 8 EXIF orientations with D4 group algebra (`Orientation::then()`)
- `LayoutPlan` fuses crop + orient + resize + pad into streaming execution
- Quarter turns only — sub-90° rotation handled by zenfilters warp

### 13.4 zenresize — Execution (IMPLEMENTED)
- `orient_image()`: materializes all 8 orientations via pixel-perfect copy
- `streaming_from_plan_batched()`: fused streaming layout execution
- Delegates sub-90° transforms to zenfilters

### 13.5 zenpipe — Graph Nodes (IMPLEMENTED)
- `NodeOp::Orient`, `NodeOp::AutoOrient` — EXIF orientation
- `NodeOp::Crop` — streaming crop
- `NodeOp::Resize` — streaming resize
- `NodeOp::Layout` / `NodeOp::LayoutComposite` — full LayoutPlan
- `NodeOp::Filter(Pipeline)` — wraps zenfilters (Warp/Rotate go through this)
- `NodeOp::CropWhitespace` — auto content-bounds detection

### 13.6 What's Needed for the Demo ⬜
- **Enable `experimental` feature** on zenfilters to expose rotate/warp nodes
- **Wire document module** to UI buttons (auto-deskew, auto-crop, perspective)
- **Add rotate/warp to the demo crate's FiltersConverter** (already works via Pipeline)
- **Geometry stage UI**: crop handles, rotation slider, flip buttons, deskew button
- **Document mode UI**: quad detect → rectify → crop → enhance pipeline
- **Expose `crop_whitespace`** node to the demo sidebar

### 13.7 zenblend ✅ (no changes needed)
- Porter-Duff modes, artistic blends — sufficient for current needs
- Gradient masks for graduated filters would be a future enhancement

---

## 14. Editor Modes

The editor serves different use cases. The UI adapts based on the selected mode, surfacing relevant tools and hiding complexity that doesn't apply.

### 14.1 Mode Selection ⬜
A mode selector (tabs or dropdown) at the top of the sidebar:

| Mode | Primary Use | Key Features |
|------|------------|--------------|
| **Edit** | Individual image tuning | All filters, film presets, manual crop, manual adjustments |
| **Document** | Document/whiteboard cleanup | Auto-deskew, perspective, whitespace crop, binarize, enhance |
| **Workflow** | Batch-applicable recipes | Auto-mode filters highlighted, export sets, recipe building |
| **CMS** | Aspect ratio crop sets for content management | Named crop definitions, preview at multiple ratios, JSON export |

### 14.2 Edit Mode (Default) ⬜
Full manual control. All filter groups visible. The current UI is essentially this mode.
- Stage tabs: Geometry → Filters → Export
- All sliders, all presets, full crop/rotate/flip

### 14.3 Document Mode ⬜
Optimized for scanning, whiteboard capture, document photos:
- **Auto pipeline**: detect quad → perspective rectify → deskew → whitespace crop → enhance
- **One-click "Clean Up"** button that runs the full auto pipeline
- **Manual overrides**: adjust each step individually
- **Key filters surfaced**: auto_levels, bilateral (shadow removal), binarize (via otsu), sharpen
- **Hidden**: film presets, creative effects, color grading (not relevant)
- **Export defaults**: PNG lossless for documents, high-quality JPEG for photos

### 14.4 Workflow Mode ⬜
For building reusable recipes applied to many images:
- **Auto-mode filters highlighted**: auto_levels, auto_exposure, auto white balance
- **Toggle: "Auto" vs "Manual"** per filter — auto mode uses scene analysis, manual uses fixed values
- **Broadly-applicable adjustments surfaced**:
  - Sharpness / clarity / brilliance (content-adaptive)
  - Contrast / exposure normalization
  - White balance correction
  - Gamut expansion (for sRGB → P3 workflows)
  - JPEG deblocking (for re-encoding workflows)
  - Noise reduction (content-adaptive)
- **Export sets**: define multiple output formats/sizes
  - e.g., "web" (WebP 80, 1600px), "print" (JXL lossless, full-res), "thumb" (JPEG 70, 200px)
- **Recipe preview**: small strip showing before/after on a sample from the batch
- **Batch apply**: drag folder or select multiple files, apply recipe to all

### 14.5 CMS Mode ⬜
For defining aspect ratio crop sets that a CMS/CDN will use at serve time:
- **Named crop definitions**: "hero" (16:9), "card" (4:3), "avatar" (1:1), "story" (9:16)
- **Live preview**: see all crops simultaneously on the source image
- **Anchor selection**: center, face-detect, rule-of-thirds, manual point
- **Smart crop**: saliency-based anchor when face detection unavailable
  - Uses zensally (zentract plugin) when available, browser AI fallback
- **JSON output**: `{ "crops": { "hero": { "aspect": "16:9", "anchor": [0.5, 0.4] } } }`
- **Compatible with imageflow/zenpipe server** crop API (RIAPI querystring)
- **No filter adjustments** in this mode — crops only (filters are separate concerns)

---

## 15. AI / ML Integration

### 15.1 Architecture ⬜
Two paths for ML-powered features:

**Server-side (zentract)**:
- ONNX inference via zentract plugin system (zentract-abi, zentract-api)
- Face detection and saliency maps (zensally)
- Runs server-side for imageflow/zenpipe server deployments
- Not available in browser (ONNX runtime is native)

**Browser-side (Web AI)**:
- **WebNN API** (Chrome 128+, Edge): hardware-accelerated inference
- **Transformers.js** (Hugging Face): ONNX models in WASM/WebGPU
- **MediaPipe** (Google): face detection, face mesh, selfie segmentation
- **TensorFlow.js**: broad model support, WebGL/WebGPU backends

### 15.2 ML-Powered Features ⬜

| Feature | Use Case | Server (zentract) | Browser (Web AI) |
|---------|----------|-------------------|------------------|
| **Face detection** | Smart crop anchor, portrait mode | zensally | MediaPipe Face Detection |
| **Saliency map** | Smart crop, content-aware | zensally | Transformers.js (DINO/SAM) |
| **Auto white balance** | Workflow mode | custom ONNX | Transformers.js or heuristic |
| **Auto exposure** | Workflow mode | zenfilters auto_exposure | zenfilters (already works) |
| **Auto levels** | Workflow mode | zenfilters auto_levels | zenfilters (already works) |
| **Subject segmentation** | Background blur, selective edit | — | MediaPipe Selfie Segmentation |
| **Super-resolution** | Upscale beyond source pixels | — | Transformers.js (Real-ESRGAN) |
| **JPEG artifact removal** | Deblock/dering | zenfilters bilateral | Transformers.js or zenfilters |
| **Style transfer** | Creative presets beyond LUTs | — | Transformers.js |
| **Scene classification** | Auto-select film preset | — | Transformers.js (CLIP) |

### 15.3 Progressive Enhancement ⬜
- ML features are **optional enhancements**, never required
- Editor works fully without any AI
- When available, ML features appear as "magic wand" buttons alongside manual controls
- **Fallback chain**: WebNN → WebGPU → WASM → manual-only
- Model downloads are lazy (on first use), cached in browser storage
- Show download progress and model size before user commits

### 15.4 Integration Pattern ⬜
```
User clicks "Smart Crop" or "Auto White Balance"
  → Check: zentract available? (server mode)
    → Yes: RPC to server inference
    → No: Check browser AI support
      → WebNN available? Use it
      → WebGPU available? Use Transformers.js
      → Neither: fall back to heuristic (histogram-based, etc.)
  → Result: { anchor: [0.5, 0.4], confidence: 0.92 }
  → Apply to crop/filter as a suggestion (user can override)
```

---

## 16. UX Design Principles

### 16.1 Simplicity Despite Complexity ⬜
The system has 4 modes, 50+ filters, 6 codecs, geometry tools, ML features, batch processing, crop sets, and a procedural recipe system. It must feel simple:

1. **Progressive disclosure**: start with Favorites + one-click presets. Advanced tools are collapsed or in secondary tabs.
2. **Mode-appropriate defaults**: Document mode hides creative filters. Workflow mode highlights auto features. CMS mode shows only crops.
3. **One-click workflows**: "Clean Up Document", "Auto Enhance", "Smart Crop" — single button that chains multiple operations.
4. **Undo is always available**: Ctrl+Z / two-finger-back, or tap the history panel.
5. **No modal dialogs for editing**: everything is inline, non-blocking.
6. **Show results, not controls**: the image is always the largest element. Controls are compact.

### 16.2 Intuitive Editing Flow ⬜
```
Open image → Auto-enhance suggestion (subtle, dismissible)
  → Quick adjustments via Favorites sliders
  → Try film presets (one-tap, see result immediately)
  → Crop/rotate if needed (geometry tab)
  → Export (format comparison, one-click download)

The whole flow should take < 30 seconds for a casual edit.
Advanced users can dive into any subsystem without the simple flow getting in the way.
```

### 16.3 Mobile-First, Desktop-Enhanced
- Touch targets ≥ 44px
- Swipe gestures for undo, mode switching
- Bottom sheet for controls on mobile (thumb-reachable)
- Desktop gets keyboard shortcuts, wider sidebar, split-screen compare

---

## 17. Color Pipeline & HDR

### 17.1 Working Color Space ⬜
The pipeline should support multiple working color spaces, selected automatically or by user:

| Stage | Default Space | Why |
|-------|--------------|-----|
| Decode | Source native (sRGB, P3, Rec.2020, etc.) | Preserve original gamut |
| Geometry | Pass-through (no conversion) | Crop/resize are color-agnostic |
| Filters | Oklab f32 linear (zenfilters native) | Perceptually uniform for adjustments |
| Encode | Target space (user-selected or auto) | Match output requirements |

### 17.2 Target Color Space ⬜
User or workflow selects the output color space:

| Target | CICP | Use Case | Codecs |
|--------|------|----------|--------|
| sRGB | 1/13/0 | Web default, widest compatibility | All |
| Display P3 | 12/16/0 | Apple ecosystem, wide gamut SDR | JPEG, PNG, JXL, AVIF, WebP |
| Rec.2020 | 9/16/0 | HDR workflows, broadcast | JXL, AVIF |
| Rec.2020 PQ | 9/16/0 + TF=16 | HDR10 | JXL, AVIF |
| Rec.2020 HLG | 9/16/0 + TF=18 | HLG broadcast | JXL, AVIF |

- **Auto mode**: match source color space (no unnecessary conversion)
- **CICP signaling**: embed correct CICP values in output (already in zencodec Metadata)
- **ICC fallback**: embed ICC profile when CICP isn't available (JPEG, PNG)
- **Gamut mapping**: use zenpixels-convert for out-of-gamut handling (perceptual, relative colorimetric)

### 17.3 HDR Pipeline ⬜
For HDR sources (PQ/HLG transfer function, wide gamut):

| Feature | Status | Location |
|---------|--------|----------|
| PQ/HLG decode | ✅ | zencodec, zenjxl, zenavif |
| HDR metadata preservation | ⬜ | zencodec::Metadata (CICP, mastering display) |
| Tone mapping (HDR → SDR) | 🔧 | zenfilters (basecurve_tonemap, dt_sigmoid, levels) |
| Inverse tone mapping (SDR → HDR) | ⬜ | Needs gain map or ML approach |
| HDR display | ⬜ | `<canvas>` with HDR color space + HDR CSS media query |

### 17.4 Gain Map Support ⬜
Gain maps enable adaptive SDR/HDR display from a single file:

| Feature | Status | Location |
|---------|--------|----------|
| Gain map detection (probe) | ✅ | zenjpeg (UltraHDR MPF), zenjxl (jhgm box), zenavif (tmap) |
| Gain map metadata extraction | ✅ | zencodec::GainMapParams, zencodec::GainMapPresence |
| Gain map pixel decode | 🔧 | Per-codec (JPEG MPF second image, JXL frame, AVIF tmap item) |
| Gain map application (tone mapping) | ✅ | ultrahdr crate (ISO 21496-1 math) |
| Gain map sidecar through pipeline | ✅ | zenpipe sidecar.rs (SidecarPlan, ProcessedSidecar) |
| Gain map proportional transforms | ✅ | zenlayout IdealLayout::derive_secondary() |
| Gain map re-embedding on encode | 🔧 | zenjpeg (UltraHDR), zenjxl (jhgm), zenavif (tmap) |
| Gain map preview in browser | ⬜ | Needs HDR canvas or gain map JS application |

### 17.5 Bit Depth ⬜
Pipeline should preserve bit depth when possible:

| Depth | Internal Repr | Codecs | Notes |
|-------|--------------|--------|-------|
| 8-bit | u8 (RGBA8) | All | Default, fastest |
| 16-bit | u16 (RGBA16) | PNG, JXL, AVIF, TIFF | Scientific, medical, print |
| Float | f32 (RGBAF32) | JXL, EXR | HDR, compositing, VFX |

- **Current**: pipeline materializes to RGBA8 for display (via pack_rgba/RowConverter)
- **Needed**: encode directly from f32 pipeline output without quantizing to u8
- **Session cache**: can cache at native bit depth (f32 from zenfilters), encode from cache
- **Display**: always convert to 8-bit for canvas (browser limitation for SDR)
- **Export**: user selects target bit depth, encoder handles conversion

### 17.6 Metadata Preservation Goals ⬜
**Decode → preserve → passthrough → encode** for all metadata types:

| Metadata | Decode | Pipeline | Encode | Status |
|----------|--------|----------|--------|--------|
| ICC profile | ✅ all codecs | ⬜ on Editor struct | ⬜ per-codec passthrough | Needs wiring |
| EXIF | ✅ JPEG/AVIF/JXL | ⬜ on Editor struct | ⬜ per-codec passthrough | Needs wiring |
| XMP | ✅ JPEG/JXL | ⬜ on Editor struct | ⬜ per-codec passthrough | Needs wiring |
| CICP | ✅ AVIF/JXL/HEIC | ⬜ on Editor struct | ⬜ per-codec signaling | Needs wiring |
| Gain map params | ✅ all gainmap codecs | ⬜ sidecar tracking | ⬜ re-embedding | Needs wiring |
| Mastering display info | ✅ JXL/AVIF | ⬜ passthrough | ⬜ re-embedding | Needs wiring |
| Orientation (EXIF tag) | ✅ | ✅ applied in pipeline | ✅ cleared on encode | Working |

The E2E pipeline (§1.3) is the prerequisite — `Editor::from_bytes()` must preserve metadata from decode, and `encode_with_metadata()` must pass it through to each codec's encoder.

### 17.7 User Controls (Export Panel) ⬜
Extend the Advanced Preservation Panel (§3.5):
- **Color space dropdown**: Auto / sRGB / Display P3 / Rec.2020
- **Transfer function**: Auto / sRGB gamma / PQ / HLG (when Rec.2020 selected)
- **Bit depth**: Auto / 8 / 16 / Float (grayed out when format doesn't support)
- **Metadata checkboxes**: ICC ☑ / EXIF ☑ / XMP ☑ / CICP ☑ (all on by default)
- **Gain map**: Preserve ☑ / Discard / Reconstruct HDR (when source has gain map)
- **Gamut handling**: Perceptual / Relative Colorimetric / Absolute Colorimetric

---

## 18. Mobile UX Architecture

### 18.1 Core Insight
Users don't think in features — they think in *intents*:
- "Make this look better"
- "Fix this document"
- "Get the right crop for Instagram"
- "Apply my usual look to these 20 photos"

The mobile UX should surface **actions and results**, not control panels. Controls appear only when the user drills in.

### 18.2 Home Screen: Intent Selection
On image load, show the image full-bleed with a bottom sheet containing 4-5 large tap targets:

```
┌────────────────────────┐
│                        │
│    [image full-bleed]  │
│                        │
│                        │
├────────────────────────┤
│  ✨ Enhance   📐 Crop  │
│  🎨 Style    📄 Doc   │
│  💾 Export             │
└────────────────────────┘
```

Each button leads to a **focused sub-screen**, not a settings dump.

### 18.3 Enhance (Primary Flow)
Tap "Enhance" → **auto-enhance applied immediately** (auto_levels + auto_exposure + clarity + vibrance at mild settings). User sees the result.

- **Undo banner**: "Auto-enhanced. [Undo]" — fades after 3s
- **Adjust**: swipe up on bottom sheet to reveal the Favorites sliders
  - Only 5-6 sliders visible: Exposure, Contrast, Saturation, Warmth, Clarity, Vibrance
  - Each slider shows before/after split when being dragged
- **More filters**: "More..." link at the bottom of favorites → opens full filter list
  - Full list is grouped but starts scrolled to the most relevant group
- **Film looks**: horizontal scrollable strip of preset thumbnails above the sliders
  - Tap to apply, tap again to remove
  - Current preset highlighted
- **Done**: tap checkmark → returns to home screen with edits applied

### 18.4 Crop (Gesture-First)
Tap "Crop" → image enters crop mode:

- **Pinch to resize** the crop (not the zoom — the actual crop boundaries)
- **Drag to reposition** the crop window
- **Aspect ratio pills**: row of tappable aspect ratios at the bottom
  - Free | 1:1 | 4:3 | 3:2 | 16:9 | 9:16 | Custom
- **Rotate**: rotation wheel at the bottom (like iOS Photos)
  - Fine-grained (-45° to +45°) with haptic snap at 0°
  - "Auto" button that runs `detect_skew_angle()` and corrects
- **Flip**: H/V flip buttons in the toolbar
- **Reset**: undo crop changes without leaving crop mode
- **Done/Cancel**: checkmark or X

### 18.5 Style (Presets + Creative)
Tap "Style" → horizontal scrollable grid of preset thumbnails (3×N):
- Each thumbnail is the current image with that preset applied (pre-rendered at 48px)
- Tap to apply, shows intensity slider
- Categories: tabs at top (Film, Creative, B&W, Cinematic)
- "Custom" opens the full filter panel

### 18.6 Document Mode
Tap "Doc" → auto-pipeline runs immediately:
1. Detect document quad (LSD + polygon)
2. Show detected corners as draggable points on the image
3. User adjusts corners if needed
4. "Apply" → perspective rectify + deskew + whitespace crop + auto-levels
5. Result shown — user can tweak contrast/sharpness
6. Export as PDF (future) or high-quality image

### 18.7 Export (Bottom Sheet)
Tap "Export" → bottom sheet with:
- Format pills: JPEG | WebP | PNG | JXL (most relevant first)
- Quality slider (single slider, format-specific range)
- Size display: "1.2 MB · 4000×3000" with the encoded preview visible behind the sheet
- "Download" button (large, thumb-friendly)
- "More options" → full export modal (aspect lock, metadata, advanced)

### 18.8 Navigation Patterns
- **Bottom sheet**: all controls live in a draggable bottom sheet (like Google Maps, Apple Photos)
  - Collapsed: 2-3 lines visible (action buttons)
  - Half-open: primary controls (sliders, presets)
  - Full: all options
- **Swipe back**: swipe right to go back one level (standard iOS/Android gesture)
- **Undo trail**: swipe left edge to undo, persistent across sub-screens
- **The image is always visible**: controls overlay on a semi-transparent sheet, never replace the image
- **No tabs switching between unrelated things**: each intent is a focused linear flow

### 18.9 Touch Targets & Gestures
| Gesture | Action |
|---------|--------|
| Single tap | Select/toggle (preset, aspect ratio, button) |
| Tap-hold on image | Show original (release = back to edited) |
| Drag on image | Pan (in crop mode: move crop) |
| Pinch on image | Zoom (in crop mode: resize crop) |
| Swipe up on sheet | Expand controls |
| Swipe down on sheet | Collapse controls |
| Swipe right | Back / undo |
| Double-tap on image | Fit to screen / toggle 1:1 |

Minimum touch target: 44×44pt. Slider tracks: 48pt tall (for thumb accuracy). Spacing between interactive elements: ≥8pt.

### 18.10 Workflow Mode on Mobile
Workflow mode adapts for mobile by becoming a **recipe builder wizard**:

1. "What kind of images?" → Photo / Document / Product / Social
2. "What adjustments?" → toggle switches for each auto-feature
   - Auto exposure ☑, Auto levels ☑, Sharpen ☑, Denoise ☐
3. "What crops?" → select aspect ratios needed
4. "What formats?" → select export formats + quality
5. "Preview" → shows recipe applied to current image
6. "Apply to batch" → file picker for multiple images, progress indicator
7. "Save recipe" → names it automatically ("amber-fox"), stores for reuse

### 18.11 Desktop vs Mobile Differences
| Feature | Mobile | Desktop |
|---------|--------|---------|
| Controls | Bottom sheet | Side panel |
| Crop handles | Drag corners + pinch | Drag corners + mouse |
| Rotation | Wheel gesture | Slider |
| Filter list | Scrollable list | Collapsible groups |
| Presets | Horizontal scroll strip | Grid |
| Export | Bottom sheet + preview | Modal + preview pane |
| Undo | Swipe gesture | Ctrl+Z |
| Compare | Tap-hold | Backslash key or tap-hold |
| Batch | Wizard flow | Drag-and-drop + sidebar recipe |

---

## 19. Touch-Native Design

### 19.1 The Problem with Porting Desktop to Touch
Range `<input>` sliders are terrible on touch:
- Thumb covers the value you're trying to see
- No momentum/physics — feels dead
- 4px track height is impossible to hit accurately
- Browser implementations vary wildly (Chrome vs Safari vs Firefox)
- No haptic feedback at identity/zero/min/max

### 19.2 Custom Slider: Scrub Anywhere
Replace HTML range inputs with a custom touch-native slider:

```
┌──────────────────────────────────┐
│ Exposure                   +1.2 ↺│  ← label line (tap ↺ to reset)
│ ████████████████░░░░░░░░░░░░░░░░│  ← filled track (touch anywhere to scrub)
└──────────────────────────────────┘
```

- **Full-width touch target**: the entire row is draggable, not just a 4px track
- **Scrub from anywhere**: touch down on the row, drag left/right to adjust
  - No need to find and grab a tiny thumb
  - Value changes proportionally to horizontal drag distance
- **Velocity sensitivity**: slow drag = fine adjustment, fast drag = coarse
- **Haptic feedback** (via `navigator.vibrate`):
  - Tick at identity value (the "zero" point)
  - Tick at 0, min, max boundaries
  - Subtle tick at step increments for integer params
- **Visual fill**: colored bar fills from identity outward (like Lightroom mobile)
  - Identity in center for bipolar params (exposure: left=dark, right=bright)
  - Identity at left for unipolar params (clarity: left=0, right=max)
- **Value preview while dragging**: large overlay number above thumb position
  - Prevents thumb from covering the value
  - Fades 0.5s after release
- **Two-finger precision**: pinch horizontally on the slider for 10x finer control
  - Like zooming into the slider's range

### 19.3 Gesture-Driven Filter Adjustment
Beyond sliders — direct manipulation on the image:

- **Swipe up/down on image** in a filter's "direct mode":
  - Select "Exposure" → swipe up on image to brighten, down to darken
  - The entire image surface is the control — much more natural than a slider
  - Shows overlay: "Exposure +0.8" with a vertical bar indicator
  - Release → value commits
  - This is how Snapseed works — proven on billions of installs

- **Direct mode toggle**: tap a filter name (not the slider) to enter direct mode
  - Image gets a subtle overlay indicating direct mode is active
  - Vertical drag = adjust that filter
  - Horizontal drag = pan (still works)
  - Tap elsewhere to exit direct mode

### 19.4 Bottom Sheet Physics
The control sheet should feel physical:

- **Spring animation**: sheet snaps to detents (collapsed, half, full) with spring physics
- **Velocity-based**: fast swipe up → overshoots then settles (momentum)
- **Rubber-band at extremes**: pulling past full-open stretches then bounces back
- **Background dim**: image dims progressively as sheet expands (0% → 30% at full)
- **Detent positions**:
  - Collapsed (80px): just the action buttons row
  - Half (~40vh): primary controls visible, image still mostly visible
  - Full (~85vh): all controls, image peeks above

Implementation: CSS `touch-action: none` on the sheet handle, JS `pointermove` tracking with spring physics on release (simple damped spring: `x += (target - x) * 0.15` per frame).

### 19.5 Swipe Actions
| Gesture | Where | Action |
|---------|-------|--------|
| Swipe right from left edge | Anywhere | Undo last edit |
| Swipe left from right edge | Anywhere | Redo |
| Swipe down on image | Top of image | Dismiss / go back |
| Long-press + drag on filter list | Sidebar | Reorder favorites |
| Two-finger rotate on image | Crop mode | Rotate image (maps to rotation angle) |
| Three-finger pinch | Image | Zoom overview (show whole image briefly) |

### 19.6 Eliminating Tap-Target Problems
- **No buttons smaller than 44×44pt** — audit all UI elements
- **Slider rows: 56pt tall** (label + track + padding)
- **Group headers: 48pt** (easy to tap to collapse/expand)
- **Preset chips: 44pt tall**, spaced 8pt apart
- **Reset icon: 44×44pt touch target** (even if visually 16px, the tap area is larger)
- **Export format tabs: 48pt** with generous padding

### 19.7 Feedback & Responsiveness
- **Instant visual response** to every touch (<16ms):
  - CSS filter preview on slider drag (already implemented)
  - Slider fill color updates in same frame as touch
  - No waiting for worker — visual feedback is synchronous
- **Loading states use skeleton screens**, not spinners where possible
- **Haptic patterns**:
  - Light tap: slider crosses identity
  - Medium tap: mode change, preset selected
  - Heavy tap: export complete, error
- **Audio feedback**: optional subtle click sounds (off by default, in settings)

### 19.8 Accessibility
- **VoiceOver / TalkBack**: all interactive elements have `aria-label`
- **Slider values announced**: "Exposure: plus 1.2 stops"
- **Reduced motion**: respect `prefers-reduced-motion` — disable spring physics, use instant transitions
- **High contrast mode**: boost slider fill contrast, thicker borders
- **Rotor actions** (iOS VoiceOver): increment/decrement sliders via swipe up/down

---

## 20. Apple Photos–Style UX Reference

Apple Photos (iOS/macOS) is the gold standard for touch-native image editing. Key patterns to adopt:

### 20.1 The Filmstrip Scrubber
The most distinctive Apple Photos pattern — **horizontal scrolling filmstrip of adjustments**:

```
   ◄ [Exposure] [Brilliance] [Highlights] [Shadows] [Contrast] [Brightness] ►
        ┌──┐
        │  │  ← vertical dial/scrubber for selected adjustment
        │  │     drag up = increase, down = decrease
        │  │     shows numeric value at top
        └──┘
```

- Adjustments are **icons with labels** in a horizontal scroll strip at the bottom
- Tap one to select it → a **vertical circular dial** appears
- Drag the dial up/down to adjust (not a horizontal slider!)
- The image updates live
- The icon badge shows a yellow dot when non-default
- Swipe the strip to find more adjustments

**Why this works on mobile:**
- Horizontal scroll is natural (thumb sweeps left/right)
- Vertical adjustment avoids conflicting with horizontal scroll
- Large circular dial is easy to grab
- Icons are glanceable — you see what's available without reading
- One-handed operation: thumb reaches the bottom strip

### 20.2 Auto Button
Top of the adjust panel: **"AUTO"** button
- Tap → applies auto_exposure + auto_levels + auto white balance simultaneously
- The adjustments appear on their individual dials (not a black box)
- User can then tweak any individual value
- Tap AUTO again to toggle off (undo auto)

### 20.3 Adjustment Categories (Tabs)
Three tabs at the bottom of the edit screen:
- **Adjust** (slider icon): exposure, brilliance, highlights, shadows, contrast, brightness, black point, saturation, vibrance, warmth, tint, sharpness, definition, noise reduction, vignette
- **Filters** (three-circle icon): preset looks in a horizontal scroll
- **Crop** (crop icon): aspect ratio, straighten, flip, perspective

Each tab replaces the bottom control area. The image stays full-screen above.

### 20.4 The Adjustment Dial
Apple's vertical dial is more than a slider:
- **Circular motion**: feels like turning a physical knob
- **Inertia**: flick and it keeps spinning briefly
- **Tick marks**: visual ruler alongside the dial
- **Snap to zero**: slight resistance at the identity value
- **Range indication**: marks at min and max, current position highlighted
- **Value display**: number appears above the image during drag, fades after

For our implementation:
```
        +2.0
         ↑
    ┌─────────┐
    │  │││││  │  ← tick marks like a ruler
    │  │││││  │
    │  ││●││  │  ← current position dot
    │  │││││  │
    │  │││││  │
    └─────────┘
         ↓
        -2.0
```

### 20.5 Crop Interface (Apple-Style)
- Image floats in the center with crop handles at corners/edges
- **Grid overlay**: rule of thirds lines inside the crop
- **Drag edges/corners** to resize crop
- **Drag inside** to reposition (moves image under the crop frame)
- **Pinch to zoom** within the crop
- **Straighten dial**: horizontal rotation wheel below the image
  - Range: -45° to +45°
  - Tick marks every 1°
  - Snap at 0° with haptic
  - "Auto" button that runs deskew detection
- **Aspect ratio buttons**: row of pills (Original, Square, 4:3, 16:9, etc.)
- **Flip buttons**: horizontal/vertical in the toolbar
- **Perspective correction**: 
  - Vertical slider to correct vertical keystoning
  - Horizontal slider to correct horizontal keystoning
  - Or "Auto" for full perspective correction

### 20.6 Filter Browsing
- Filters shown as **small previews** of the current image (not just names)
- Horizontal scrollable row
- Each preview is ~60×60pt, showing the image with that filter applied
- Tap to apply → image updates live
- Selected filter gets a border highlight
- Intensity slider appears below the selected filter (0-100)
- "Original" is the first option (always visible, deselects all filters)

### 20.7 Mapping to Our System

| Apple Photos | Our Equivalent | Notes |
|-------------|---------------|-------|
| Auto button | `auto_levels` + `auto_exposure` | Run both, show results on individual dials |
| Exposure | `zenfilters.exposure` (stops) | |
| Brilliance | `zenfilters.brilliance` | |
| Highlights | `zenfilters.highlights_shadows` (highlights param) | |
| Shadows | `zenfilters.highlights_shadows` (shadows param) | |
| Contrast | `zenfilters.contrast` | |
| Brightness | Mapped to exposure with smaller range | Or a separate linear brightness |
| Black Point | `zenfilters.black_point` | |
| Saturation | `zenfilters.saturation` | |
| Vibrance | `zenfilters.vibrance` | |
| Warmth | `zenfilters.temperature` | |
| Tint | `zenfilters.tint` | |
| Sharpness | `zenfilters.sharpen` | |
| Definition | `zenfilters.clarity` | Apple's "Definition" ≈ local contrast |
| Noise Reduction | `zenfilters.noise_reduction` | |
| Vignette | `zenfilters.vignette` | |
| Crop/Straighten | `zenfilters.rotate` + crop | |
| Perspective V/H | `zenfilters.warp` (perspective) | |
| Filters | Film presets | Our preset thumbnails |

### 20.8 What Apple Does That We Should Copy
1. **Image is always the hero** — controls are minimal, at the bottom, semi-transparent
2. **One adjustment active at a time** — no overwhelming grid of sliders
3. **Vertical dial, not horizontal slider** — avoids conflict with scrolling
4. **Live preview during drag** — no "apply" button, changes are immediate
5. **Auto as a starting point** — not a replacement for manual control
6. **Non-destructive**: original always preserved, edits stored as a recipe
7. **"Revert to Original"** always available (our tap-hold compare)
8. **Copy/paste edits** between photos (our recipe system)

### 20.9 What We Can Do Better Than Apple
1. **Format comparison** — Apple exports one format; we show JPEG vs WebP vs JXL side by side
2. **CMS crop sets** — Apple has one crop; we define multiple named crops
3. **Film presets with intensity** — Apple's filters are binary; ours have a strength slider
4. **Gain map / HDR pipeline** — Apple handles this silently; we give users control
5. **Open recipe format** — Apple's edits are proprietary; ours are JSON/querystring
6. **Cross-platform** — Apple is Apple-only; we run in any browser
7. **Workflow/batch** — Apple edits one photo; we can build recipes for thousands
8. **Document mode** — Apple has no document scanner UX in the editor

### 20.10 Implementation Priority
For the first touch-native release, implement in this order:
1. **Bottom strip with adjustment icons** (horizontal scroll)
2. **Vertical dial** for the selected adjustment
3. **Auto button** that applies auto_levels + auto_exposure
4. **Film preset thumbnails** (horizontal scroll with previews)
5. **Apple-style crop** with straighten wheel and aspect pills
6. **Compare** via tap-hold (already specced)
7. **Bottom sheet** for additional options

This gives us an Apple Photos–quality editing experience with our superior codec, format, and pipeline capabilities underneath.

---

## 21. Code Architecture for Multi-Layout Support

### 21.1 Current Problem
Every JS module directly manipulates DOM elements by ID (`$('detail-canvas')`, `$('overview-canvas')`, etc.). This makes the code:
- **Tightly coupled to one HTML layout** — can't swap desktop sidebar for mobile bottom sheet
- **Untestable without a browser** — logic and DOM are interleaved
- **Hard to adapt** — adding tablet layout means touching 15 files

### 21.2 Target Architecture: Model → Controller → View

```
┌─────────────────────────────────────────────────┐
│  MODEL (no DOM, no HTML, testable standalone)    │
│                                                  │
│  EditorModel        — source, region, zoom       │
│  AdjustmentModel    — filter values, presets     │
│  ExportModel        — format, quality, dims      │
│  HistoryModel       — undo stack, auto-naming    │
│  RecipeModel        — serialization, persistence │
│  CropSetModel       — named aspect ratio crops   │
│                                                  │
│  All state, all logic, zero DOM references.      │
│  Emits events: 'change', 'render-needed', etc.   │
└────────────────────┬────────────────────────────┘
                     │ events
┌────────────────────▼────────────────────────────┐
│  CONTROLLER (bridges model ↔ view ↔ worker)      │
│                                                  │
│  RenderController   — debounce, send to worker   │
│  GestureController  — drag, pinch, scroll → model│
│  ExportController   — preview, encode, download  │
│                                                  │
│  Knows about the model and the worker.           │
│  Does NOT know about specific DOM elements.      │
│  Receives abstract events from the view.         │
└────────────────────┬────────────────────────────┘
                     │ abstract commands
┌────────────────────▼────────────────────────────┐
│  VIEW (renders model state to DOM)               │
│                                                  │
│  DesktopView        — sidebar + main viewport    │
│  MobileView         — bottom sheet + filmstrip   │
│  TabletView         — hybrid (or reuse desktop)  │
│                                                  │
│  Each view:                                      │
│  - Creates its own DOM elements                  │
│  - Listens to model events, updates DOM          │
│  - Translates DOM events to controller commands  │
│  - No business logic                             │
└─────────────────────────────────────────────────┘
```

### 21.3 Model Layer (Pure Logic, No DOM)

```javascript
// model/editor.js
export class EditorModel extends EventTarget {
  source = null;       // { width, height, name, hash, backend }
  region = { x: 0.25, y: 0.25, w: 0.5, h: 0.5 };
  zoom = 1.0;

  setRegion(r) {
    this.region = { ...r };
    this.dispatchEvent(new Event('region-change'));
  }

  setSource(info) {
    this.source = info;
    this.dispatchEvent(new Event('source-change'));
  }
}

// model/adjustments.js
export class AdjustmentModel extends EventTarget {
  values = {};          // adjustKey → value
  touched = new Set();
  filmPreset = null;
  filmIntensity = 1.0;
  sliderNodes = [];     // from schema
  lastSafe = {};

  set(key, value) {
    this.values[key] = value;
    this.touched.add(key);
    this.dispatchEvent(new CustomEvent('adjust', { detail: { key, value } }));
  }

  reset(key) {
    const node = this.nodeForKey(key);
    if (node) { this.values[key] = node.identity; this.touched.delete(key); }
    this.dispatchEvent(new CustomEvent('adjust', { detail: { key } }));
  }

  resetAll() { /* ... */ }

  // Pure logic: build the adjustments object for the worker
  getFilterAdjustments() {
    const adj = {};
    for (const node of this.sliderNodes) {
      const params = {};
      let changed = false;
      for (const p of node.params) {
        params[p.paramName] = this.values[p.adjustKey];
        if (this.values[p.adjustKey] !== p.identity) changed = true;
      }
      if (changed) adj[node.id] = params;
    }
    return adj;
  }

  snapshotSafe() { this.lastSafe = structuredClone(this.getFilterAdjustments()); }
  restoreSafe() { /* ... apply lastSafe back to values ... */ }
}

// model/recipe.js
export class RecipeModel {
  toJSON() { /* serialize adjustments + geometry + presets + crop sets */ }
  fromJSON(json) { /* restore */ }
  toQuerystring() { /* RIAPI-compatible */ }
}

// model/history.js
export class HistoryModel extends EventTarget {
  stack = [];
  index = -1;
  push(snapshot) { /* ... */ }
  undo() { /* ... */ }
  redo() { /* ... */ }
  // Auto-naming: "amber-fox 14:05"
}
```

**Key property**: all models are testable with plain Node.js — no browser, no DOM, no canvas.

### 21.4 Controller Layer (Model ↔ Worker ↔ View)

```javascript
// controller/render.js
export class RenderController {
  constructor(model, adjustments, workerClient) { /* ... */ }

  scheduleRender() {
    // Debounce, build adjustments, send to worker
    // On result: emit 'overview-rendered', 'detail-rendered' events
  }

  scheduleDetailOnly() { /* ... */ }
}

// controller/gestures.js
export class GestureController {
  constructor(model) { /* ... */ }

  // Called by the view with abstract events, not DOM events
  onDragStart(x, y) { /* ... */ }
  onDragMove(dx, dy) { /* update model.region */ }
  onDragEnd() { /* trigger render */ }
  onZoom(factor, centerX, centerY) { /* update model.region */ }
  onPinch(scale, centerX, centerY) { /* ... */ }
}
```

### 21.5 View Layer (DOM, Swappable)

```javascript
// view/desktop.js — current sidebar layout
export class DesktopView {
  constructor(container, model, adjustments, controller) {
    this.buildSidebar();
    this.buildViewport();
    model.addEventListener('region-change', () => this.updateRegionSelector());
    adjustments.addEventListener('adjust', (e) => this.updateSlider(e.detail.key));
  }

  buildSidebar() { /* create slider groups from adjustments.sliderNodes */ }
  updateSlider(key) { /* update one slider's DOM to match model */ }
  updateRegionSelector() { /* position the crop overlay */ }
}

// view/mobile.js — bottom sheet + filmstrip
export class MobileView {
  constructor(container, model, adjustments, controller) {
    this.buildBottomSheet();
    this.buildFilmstrip();
    // Same model events, completely different DOM
  }

  buildFilmstrip() { /* horizontal icon strip */ }
  buildDial(param) { /* vertical adjustment dial */ }
}

// view/shared.js — common to both (canvas rendering, worker display)
export class ViewportRenderer {
  constructor(overviewCanvas, detailCanvas, model) { /* ... */ }
  displayOverview(imageData) { /* putImageData */ }
  displayDetail(imageData) { /* putImageData */ }
  showUpscaledPreview() { /* draw overview into detail */ }
}
```

### 21.6 Migration Path (Incremental, Not Big-Bang)

Phase 1 — **Extract models** (no visual changes):
- Move `state.js` fields into `EditorModel` + `AdjustmentModel`
- Keep existing modules working by importing from models
- Models emit events, existing modules listen

Phase 2 — **Extract controllers** (no visual changes):
- Move `render.js` logic to `RenderController`
- Move gesture logic from `region.js` to `GestureController`
- Existing view code calls controllers instead of doing logic

Phase 3 — **Desktop view** (refactor existing):
- Move DOM creation from `sidebar.js` into `DesktopView`
- `main.js` becomes: `new DesktopView(container, model, adjustments, controller)`

Phase 4 — **Mobile view** (new):
- `MobileView` creates bottom sheet + filmstrip
- Same models, same controllers, different DOM
- Layout selected by viewport width or user preference

Phase 5 — **Auto-detect**:
```javascript
const View = window.innerWidth < 768 ? MobileView
           : window.innerWidth < 1200 ? TabletView
           : DesktopView;
const view = new View(document.body, model, adjustments, controller);
```

### 21.7 What This Enables
- **Test models in Node.js** without a browser
- **Swap layouts at runtime** (rotate tablet → switch view)
- **Build alternative UIs**: CLI, native app (via wasm), headless batch
- **Keep the worker/WASM layer unchanged** — views never talk to the worker directly
- **Framework migration**: if we ever adopt React/Preact/Svelte, only the view layer changes

---

## 22. Library Evaluation

### 22.1 Constraints
- No build step required (or minimal — current setup is just static files + wasm-pack)
- Must work with Web Workers (our WASM pipeline runs off-thread)
- Bundle size matters — the WASM module is already 5.6 MB
- Progressive enhancement — must work without JS framework for the core pipeline
- Touch-native gestures are a hard requirement

### 22.2 UI Framework

| Library | Size | Build Step | Touch | Fit |
|---------|------|-----------|-------|-----|
| **Preact** | 4 KB | Optional (HTM for no-build) | Manual | ✅ Best fit — tiny, reactive, no build with HTM |
| **Lit** | 7 KB | None (native web components) | Manual | ✅ Good fit — standards-based, no build |
| **Svelte** | 2 KB runtime | Required (compiler) | Manual | ⚠️ Great DX but needs build step |
| **React** | 40 KB | Required | Manual | ❌ Too large, requires build |
| **Vue** | 33 KB | Optional | Manual | ⚠️ Viable but large |
| **Solid** | 7 KB | Required (compiler) | Manual | ⚠️ Great perf but needs build |
| **Vanilla** | 0 KB | None | Manual | ✅ Current approach — works but scaling pain |

**Recommendation: Preact + HTM** (no build step)
```javascript
import { h, render } from 'https://esm.sh/preact';
import { useState, useEffect } from 'https://esm.sh/preact/hooks';
import htm from 'https://esm.sh/htm';
const html = htm.bind(h);

function Slider({ label, value, min, max, step, onChange }) {
  return html`
    <div class="slider-row">
      <div class="slider-label-line">
        <label>${label}</label>
        <span class="val">${value.toFixed(2)}</span>
      </div>
      <input type="range" min=${min} max=${max} step=${step}
             value=${value} onInput=${e => onChange(+e.target.value)} />
    </div>`;
}
```
- 4 KB gzipped, CDN-loaded, zero build config
- Components for each view (DesktopSidebar, MobileSheet, etc.)
- Hooks for model subscription (`useModel`, `useAdjustment`)
- HTM = tagged template literals, no JSX compiler needed

**Alternative: Lit** (web components, also no build)
```javascript
import { LitElement, html, css } from 'https://esm.sh/lit';

class ZenSlider extends LitElement {
  static properties = { label: {}, value: { type: Number }, min: {}, max: {}, step: {} };
  render() {
    return html`<div class="slider-row">...</div>`;
  }
}
customElements.define('zen-slider', ZenSlider);
```
- Native web components — work in any framework or none
- Shadow DOM isolates styles (good for embedding, bad for theming)
- Slightly more boilerplate than Preact

### 22.3 Gesture Libraries

| Library | Size | Touch | Inertia | Pinch | Fit |
|---------|------|-------|---------|-------|-----|
| **@use-gesture** | 10 KB | ✅ | ✅ | ✅ | ✅ Best — unified gesture system |
| **Hammer.js** | 7 KB | ✅ | ❌ | ✅ | ⚠️ Unmaintained since 2016 |
| **interact.js** | 30 KB | ✅ | ✅ | ❌ | ❌ Too large, drag-focused |
| **any-touch** | 4 KB | ✅ | ❌ | ✅ | ✅ Lightweight alternative |
| **Vanilla PointerEvents** | 0 KB | ✅ | Manual | Manual | ✅ Current approach |

**Recommendation: `@use-gesture/vanilla`** (framework-agnostic)
```javascript
import { DragGesture, PinchGesture, WheelGesture } from 'https://esm.sh/@use-gesture/vanilla';

new DragGesture(detailCanvas, ({ delta: [dx, dy], velocity, direction }) => {
  controller.onDragMove(dx, dy, velocity);
});

new PinchGesture(detailCanvas, ({ offset: [scale], origin }) => {
  controller.onPinch(scale, origin[0], origin[1]);
});

new WheelGesture(detailCanvas, ({ delta: [, dy] }) => {
  controller.onZoom(dy);
});
```
- Handles pointer, touch, mouse, wheel uniformly
- Built-in velocity, inertia, and bounds
- Framework-agnostic version (not just React)
- Pinch-to-zoom handled correctly (two-finger distance tracking)

### 22.4 Animation / Physics

| Library | Size | Spring | Inertia | Fit |
|---------|------|--------|---------|-----|
| **popmotion** | 5 KB (standalone) | ✅ | ✅ | ✅ Spring physics for bottom sheet |
| **motion** (framer) | 18 KB | ✅ | ✅ | ⚠️ React-focused |
| **anime.js** | 7 KB | ❌ | ❌ | ❌ Keyframe-based, not physics |
| **CSS spring()** | 0 KB | Upcoming | ❌ | ⬜ Not yet in browsers |
| **Manual spring** | 0 KB | ~20 lines | Manual | ✅ Simple damped spring |

**Recommendation: Manual spring** (it's ~20 lines):
```javascript
function springTo(current, target, velocity, stiffness = 0.15, damping = 0.8) {
  const force = (target - current) * stiffness;
  velocity = (velocity + force) * damping;
  return { value: current + velocity, velocity };
}
```
Only the bottom sheet needs spring physics. Not worth a dependency.

### 22.5 Bottom Sheet

| Library | Size | Physics | Detents | Fit |
|---------|------|---------|---------|-----|
| **bottom-sheet** (npm) | 8 KB | ✅ | ✅ | ⚠️ React-only |
| **@apresentador/bottom-sheet** | 5 KB | ✅ | ✅ | ⚠️ Web component, limited |
| **Custom** | ~150 lines | Manual | Manual | ✅ Full control |

**Recommendation: Custom** — bottom sheets are surprisingly simple:
```javascript
class BottomSheet {
  detents = [80, window.innerHeight * 0.4, window.innerHeight * 0.85];
  // Touch handler + spring snap to nearest detent
}
```
No library handles our specific detent positions + image interaction correctly.

### 22.6 Slider / Dial

| Library | Size | Touch | Dial | Custom Track | Fit |
|---------|------|-------|------|-------------|-----|
| **noUiSlider** | 10 KB | ✅ | ❌ | ✅ | ⚠️ Good slider but no dial |
| **roundSlider** | 15 KB | ✅ | ✅ | ❌ | ⚠️ jQuery dependency |
| **Custom** | ~100 lines per | ✅ | ✅ | ✅ | ✅ Full control |

**Recommendation: Custom** — our sliders need:
- Scrub-anywhere (full row is touch target)
- Fill from identity (bidirectional)
- Haptic feedback at identity
- Velocity sensitivity
- The vertical dial (Apple Photos) is unique enough to need custom code

### 22.7 Recommended Stack

```
Core:        Preact + HTM (4 KB, no build, reactive components)
Gestures:    @use-gesture/vanilla (10 KB, unified touch/mouse/wheel)
Animation:   Manual spring (~20 lines)
Bottom sheet: Custom (~150 lines)
Sliders:     Custom (~100 lines)
State:       Model classes with EventTarget (built-in, 0 KB)

Total added JS: ~15 KB (vs current 0 KB for vanilla)
Total with WASM: 5.6 MB + 15 KB ≈ 5.6 MB (negligible overhead)
```

### 22.8 No-Build Development Flow
```html
<script type="module">
  import { h, render } from 'https://esm.sh/preact@10';
  import { useState } from 'https://esm.sh/preact@10/hooks';
  import htm from 'https://esm.sh/htm@3';
  import { DragGesture } from 'https://esm.sh/@use-gesture/vanilla@10';

  const html = htm.bind(h);
  // ... app code using ESM imports from CDN
</script>
```
- No npm install, no webpack, no vite
- Works with `python3 serve.py`
- CDN modules cached by browser after first load
- Can add a build step later for production (tree-shaking, bundling)

### 22.9 Migration from Vanilla
1. Add Preact + HTM imports (CDN, no install)
2. Convert `sidebar.js` slider generation to Preact components first (biggest win)
3. Convert `export-modal.js` to Preact (complex DOM generation)
4. Keep `worker-client.js`, `state.js` (model), `render.js` (controller) as-is
5. Build `MobileView` as new Preact components from scratch
6. Add `@use-gesture` for unified gesture handling

---

## 23. Multi-Platform Strategy

### 23.1 Platform Targets

| Platform | Distribution | GPU Access | Offline | Native Feel |
|----------|-------------|-----------|---------|-------------|
| Web (browser) | URL, no install | WebGPU/WebGL | Service worker | Good with effort |
| PWA (installed web) | Add to homescreen | WebGPU/WebGL | Full offline | Near-native |
| iOS app | App Store | Metal | Yes | Native |
| Android app | Play Store | Vulkan/GL | Yes | Native |
| macOS app | App Store / DMG | Metal | Yes | Native |
| Windows app | Store / MSI | D3D12/Vulkan | Yes | Native |
| Linux app | Flatpak / AppImage | Vulkan/GL | Yes | Native |
| CLI tool | Cargo install / binary | CPU+SIMD | Yes | N/A |

### 23.2 What's Shared vs Platform-Specific

```
┌─────────────────────────────────────────────────┐
│                SHARED (Rust)                     │
│                                                  │
│  zenpipe pipeline     — decode, filter, encode   │
│  Session caching      — Merkle DAG, LRU          │
│  All zen* codecs      — JPEG, WebP, PNG, JXL...  │
│  zenfilters           — 43 filters + warp        │
│  Editor model logic   — adjustments, recipes     │
│  Encode/decode        — all format support        │
│                                                  │
│  ~95% of the compute, 100% portable Rust         │
└─────────────────────┬───────────────────────────┘
                      │ FFI boundary
┌─────────────────────▼───────────────────────────┐
│           PLATFORM ADAPTATION LAYER              │
│                                                  │
│  WASM + wasm-bindgen    — browser/PWA            │
│  C FFI (imageflow_abi)  — native apps via cdylib │
│  Direct Rust API        — CLI, server, tests     │
│                                                  │
└─────────────────────┬───────────────────────────┘
                      │ UI binding
┌─────────────────────▼───────────────────────────┐
│              UI LAYER (per-platform)             │
│                                                  │
│  Web:     Preact/HTM + @use-gesture              │
│  iOS:     SwiftUI + UIKit gestures               │
│  Android: Jetpack Compose + gesture API          │
│  Desktop: Tauri (web view) or native toolkit     │
│  CLI:     clap + stdin/stdout                    │
│                                                  │
└─────────────────────────────────────────────────┘
```

### 23.3 Option A: Web-First PWA (Recommended Start)

Ship as a PWA with native app wrappers added later:

| Layer | Tech | Notes |
|-------|------|-------|
| Core | Rust → WASM (5.6 MB) | All pipeline logic |
| UI | Preact + HTM | Responsive web components |
| Offline | Service worker | Cache WASM + assets |
| Install | `manifest.json` | Add to homescreen |
| Native wrap | **Capacitor** or **TWA** | Thin native shell around the web app |

**Capacitor** (by Ionic): wraps the web app in a native WebView on iOS/Android/Electron.
- Access to camera, file system, share sheet via plugins
- Same web code, native app distribution
- 95% web, 5% native bridge code
- App Store / Play Store distribution

**Pros**: ship one codebase to all platforms immediately.
**Cons**: WebView performance ceiling (no Metal/Vulkan direct), 60fps limit, no background processing.

### 23.4 Option B: Tauri (Desktop Native + Web)

**Tauri**: Rust backend + system WebView frontend. Much lighter than Electron.

| Layer | Tech | Notes |
|-------|------|-------|
| Core | Rust (direct, no WASM) | Full native SIMD — AVX2, NEON |
| UI | Same Preact web app | Rendered in system WebView |
| IPC | Tauri commands | Rust ↔ JS via typed messages |
| Desktop | macOS/Windows/Linux | Single binary, ~10 MB |
| Mobile | Tauri Mobile (beta) | iOS/Android via native WebView |

```rust
// Tauri command — called from JS
#[tauri::command]
fn render_overview(adjustments: &str) -> Result<Vec<u8>, String> {
    let editor = get_editor()?;
    let adj = parse_adjustments(adjustments)?;
    let out = editor.render_overview(&adj)?;
    Ok(out.data)
}
```

**Pros**: native Rust performance (no WASM overhead), direct SIMD, smaller binary, system integration.
**Cons**: two deployment paths (web + Tauri), slightly different IPC from web worker postMessage.

### 23.5 Option C: Shared Rust Core + Native UI Per Platform

For maximum native feel:

| Platform | UI Framework | Rust Binding |
|----------|-------------|-------------|
| iOS | SwiftUI | `imageflow_abi` (C FFI) or UniFFI |
| Android | Jetpack Compose | JNI via `imageflow_abi` or UniFFI |
| macOS | SwiftUI / AppKit | Same as iOS |
| Windows | WinUI 3 | C FFI |
| Web | Preact | wasm-bindgen |

**UniFFI** (Mozilla): generates Swift/Kotlin bindings from Rust automatically.
```rust
#[uniffi::export]
fn render_overview(adjustments: &str) -> Result<Vec<u8>, EditorError> { ... }
```
Generates: `EditorBinding.swift` + `EditorBinding.kt` — type-safe, no manual FFI.

**Pros**: truly native UI per platform, best performance, best UX.
**Cons**: N separate UI codebases (expensive to maintain).

### 23.6 Option D: Kotlin Multiplatform + Rust Core

Compose Multiplatform (JetBrains) for shared UI across Android/iOS/Desktop:

| Layer | Tech |
|-------|------|
| Core | Rust via C FFI (all platforms) |
| UI | Compose Multiplatform |
| iOS bridge | Kotlin/Native + Rust FFI |
| Desktop | JVM + Rust FFI |

**Pros**: single UI codebase for mobile + desktop (not web).
**Cons**: doesn't help with web, adds JVM dependency.

### 23.7 Recommended Strategy

**Phase 1 — Web PWA** (now):
- Current Preact web app + WASM
- Add `manifest.json` + service worker for installability
- Works on all platforms immediately via browser

**Phase 2 — Tauri Desktop** (when desktop users need native performance):
- Same web UI in Tauri shell
- Rust pipeline runs natively (AVX2/NEON, not WASM)
- macOS + Windows + Linux from same codebase
- IPC adaptor: `tauri::command` wrapping the same `Editor` API

**Phase 3 — Capacitor Mobile** (when app store distribution needed):
- Same web app wrapped for iOS/Android
- Camera and share sheet integration via Capacitor plugins
- Or evaluate Tauri Mobile when it stabilizes

**Phase 4 — Native UI** (only if justified by user base):
- SwiftUI for iOS/macOS via UniFFI
- Compose for Android via UniFFI or JNI
- Consider only if PWA/Capacitor UX is insufficient

### 23.8 Architecture Implications

The MVC architecture from §21 maps directly:

| Layer | Web (WASM) | Tauri (native Rust) | Capacitor | Native |
|-------|-----------|-------------------|-----------|--------|
| Model | JS classes | Rust structs (same code) | JS classes (same) | Swift/Kotlin via UniFFI |
| Controller | JS → worker postMessage | JS → tauri::command | JS → worker postMessage | Native → Rust FFI |
| View | Preact components | Preact in WebView | Preact in WebView | SwiftUI / Compose |
| Pipeline | WASM (simd128) | Native (AVX2/NEON) | WASM (simd128) | Native (AVX2/NEON) |

The key: **the Rust `Editor` API is the same everywhere**. Only the FFI boundary and UI rendering change.

### 23.9 What We Build Now to Enable All Options

1. **Keep the `Editor` Rust API clean and FFI-friendly** — it already is
2. **Don't bake DOM assumptions into model/controller** — the MVC refactor handles this
3. **Use `imageflow_abi` patterns for C FFI** — already exists in the imageflow codebase
4. **Keep WASM and native builds working** — demo crate already builds both
5. **Service worker for PWA** — add `manifest.json` + offline caching
6. **Don't adopt heavy web frameworks** — Preact + HTM stays lightweight enough for WebView embedding

---

## 24. Rust-Side Model & Controller

### 24.1 What Can Live in Rust

```
Currently in JS (~3700 lines)          →  Could be Rust
─────────────────────────────────────────────────────────
state.js (adjustments, region, zoom)   →  EditorModel (Rust struct)
sidebar.js (schema parsing, param     →  SchemaModel (parse once in Rust,
  normalization, slider types)             emit typed param descriptors)
render.js (debounce, scheduling,       →  RenderController (Rust, drives
  pixel ratio calc, safe snapshot)         Session directly, no IPC hop)
region.js (drag math, zoom math,       →  RegionController (Rust, pure math —
  clamp, pinch scale computation)          only gesture → delta → new region)
history.js (undo stack, snapshot)      →  HistoryModel (Rust, serde)
compare.js (original render cache)     →  CompareController (Rust, second
                                           Session with empty adjustments)
export-modal.js (format controls,      →  ExportModel (Rust, format metadata
  estimates, preview encode)               from zencodecs, preview encode)
presets.js (film preset list)          →  PresetModel (Rust, from zenfilters)
user-presets.js (save/load/serialize)  →  RecipeModel (Rust, JSON/QS serde)
css-preview.js (CSS filter approx)     →  stays JS (CSS is browser-only)
toasts.js (error display)             →  stays JS (DOM rendering)
file-load.js (file reading, picsum)    →  stays JS (fetch, File API)
worker-client.js (postMessage)         →  stays JS (or disappears if Rust
                                           drives rendering directly)
```

### 24.2 The Rust Editor as a Full State Machine

```rust
/// The complete editor state — owns all models and controllers.
/// One instance per editing session. Serializable for persistence.
pub struct EditorState {
    // ─── Source ───
    source: Option<SourceImage>,
    source_hash: u64,

    // ─── Models ───
    adjustments: AdjustmentModel,  // all filter values, film preset
    region: RegionModel,           // crop region, zoom level
    history: HistoryModel,         // undo/redo stack with auto-naming
    recipe: RecipeModel,           // serializable edit recipe
    crop_sets: CropSetModel,       // named aspect ratio definitions
    export: ExportModel,           // format, quality, dimensions

    // ─── Pipeline ───
    overview_session: Session,     // cached overview renders
    detail_session: Session,       // cached detail renders
    compare_session: Session,      // cached original (no filters) renders

    // ─── Schema ───
    schema: SchemaModel,           // parsed node registry, param descriptors
}

impl EditorState {
    /// Process a UI command and return what changed.
    /// The view layer only needs to handle the returned
    /// ViewUpdate — it never reads internal state directly.
    pub fn dispatch(&mut self, cmd: Command) -> Vec<ViewUpdate> { ... }
}
```

### 24.3 Command / Update Protocol

Instead of the view calling 15 different Rust functions, there's one `dispatch()` method that takes a `Command` enum and returns a list of `ViewUpdate`s telling the view what to redraw:

```rust
/// Commands from the UI to the editor.
#[derive(Serialize, Deserialize)]
pub enum Command {
    // Source
    InitFromRgba { width: u32, height: u32, data: Vec<u8> },
    InitFromBytes { data: Vec<u8> },

    // Adjustments
    SetParam { key: String, value: f64 },
    SetParamBool { key: String, value: bool },
    SetFilmPreset { id: Option<String>, intensity: f32 },
    ResetParam { key: String },
    ResetAll,

    // Region / navigation
    SetRegion { x: f32, y: f32, w: f32, h: f32 },
    DragStart,
    DragMove { dx_norm: f32, dy_norm: f32 },
    DragEnd,
    Zoom { factor: f32, center_x: f32, center_y: f32 },
    ResetTo1to1 { viewport_w: f32, viewport_h: f32, dpr: f32 },

    // History
    Undo,
    Redo,

    // Compare
    ShowOriginal,
    ShowEdited,

    // Export
    SetExportFormat { format: String },
    SetExportDims { width: u32, height: u32 },
    SetExportOption { key: String, value: serde_json::Value },
    EncodePreview,
    EncodeFull,

    // Recipes
    SaveRecipe { name: String },
    LoadRecipe { json: String },
    ApplyRecipe,

    // Crop sets
    AddCropSet { name: String, aspect: String, anchor: String },
    RemoveCropSet { name: String },

    // Schema
    GetSchema,
    GetPresetList,
}

/// Updates from the editor to the view.
/// The view applies these to the DOM — no business logic.
#[derive(Serialize, Deserialize)]
pub enum ViewUpdate {
    // Pixel data for canvases
    OverviewPixels { data: Vec<u8>, width: u32, height: u32 },
    DetailPixels { data: Vec<u8>, width: u32, height: u32 },
    OriginalPixels { data: Vec<u8>, width: u32, height: u32 },

    // State changes (view updates DOM to match)
    RegionChanged { x: f32, y: f32, w: f32, h: f32 },
    ParamChanged { key: String, value: f64 },
    AllParamsReset,
    FilmPresetChanged { id: Option<String>, intensity: f32 },
    HistoryChanged { can_undo: bool, can_redo: bool, name: String },
    PixelRatio { ratio: f32, src_w: u32, src_h: u32, display_w: f32, dpr: f32 },

    // Source info
    SourceLoaded { width: u32, height: u32, format: String, backend: String },
    MetadataUpgraded { format: String, has_icc: bool, has_exif: bool, has_xmp: bool, has_gain_map: bool },

    // Export results
    EncodePreviewResult { data: Vec<u8>, format: String, size: usize, width: u32, height: u32 },
    EncodeFullResult { data: Vec<u8>, format: String, size: usize, width: u32, height: u32 },

    // Schema / presets (sent once at init)
    Schema { json: String },
    PresetList { json: String },

    // Errors
    Error { message: String, recoverable: bool },

    // Status
    RenderStarted,
    RenderComplete { elapsed_ms: f64 },
}
```

### 24.4 View Layer Becomes Trivially Simple

The JS view layer is now just:

```javascript
// The entire view layer
editor.onUpdate((updates) => {
  for (const u of updates) {
    switch (u.type) {
      case 'DetailPixels':
        ctx.putImageData(new ImageData(new Uint8ClampedArray(u.data), u.width, u.height), 0, 0);
        break;
      case 'ParamChanged':
        slider.value = u.value;
        display.textContent = formatVal(u.value);
        break;
      case 'RegionChanged':
        updateCropOverlay(u.x, u.y, u.w, u.h);
        break;
      case 'Error':
        showToast(u.message);
        break;
      // ... ~20 simple cases, each ~3 lines
    }
  }
});

// Gesture handling — translates DOM events to commands
canvas.addEventListener('pointermove', (e) => {
  editor.dispatch({ type: 'DragMove', dx_norm: dx / canvasW, dy_norm: dy / canvasH });
});
```

### 24.5 Platform Mapping

| Platform | How `dispatch()` is called | How `ViewUpdate` is received |
|----------|---------------------------|------------------------------|
| **Web (worker)** | `postMessage({cmd})` → worker calls `editor.dispatch()` | worker `postMessage({updates})` → main thread applies |
| **Web (main thread)** | Direct `editor.dispatch()` call via wasm-bindgen | Callback, synchronous |
| **Tauri** | `invoke('dispatch', {cmd})` → Rust handler | Event emit or return value |
| **iOS (UniFFI)** | `editor.dispatch(cmd)` in Swift | Callback or async/await |
| **Android (UniFFI)** | `editor.dispatch(cmd)` in Kotlin | Callback or coroutine |
| **CLI** | `editor.dispatch(cmd)` in Rust directly | Print or write to file |

### 24.6 What Stays in JS / Platform Code

Only things that are inherently platform-specific:

| Responsibility | Why Platform-Specific |
|---------------|----------------------|
| DOM rendering | Canvas, putImageData, CSS |
| File I/O | File API, drag-drop, fetch |
| Touch/pointer events | DOM event → abstract Command |
| CSS filter preview | Browser CSS engine |
| Toast/error display | DOM elements |
| Bottom sheet physics | CSS transforms + spring animation |
| Haptic feedback | `navigator.vibrate()` |
| Local storage | `localStorage`, IndexedDB |
| Service worker | PWA offline caching |
| Clipboard | `navigator.clipboard` |

Estimated: **~500 lines of JS** for the full view layer per platform layout (desktop, mobile).
All business logic, state management, undo/redo, recipe serialization, render scheduling, pixel ratio math, debouncing, schema parsing — **all in Rust**.

### 24.7 What This Means for the Worker

The current worker architecture (`postMessage` → worker → WASM → `postMessage` back) maps directly to the Command/Update protocol. The worker becomes a thin dispatcher:

```javascript
// worker.js — entire file
import init, { EditorState } from './pkg/zenpipe_demo.js';
await init();
const editor = new EditorState();

self.onmessage = (e) => {
  const updates = editor.dispatch(e.data);
  // Transfer pixel data buffers for zero-copy
  const transfers = updates
    .filter(u => u.data instanceof Uint8Array)
    .map(u => u.data.buffer);
  self.postMessage(updates, transfers);
};
```

### 24.8 Migration Path

1. **Define `Command` and `ViewUpdate` enums** in Rust (the contract)
2. **Implement `EditorState::dispatch()`** — starts by wrapping the existing `Editor` methods
3. **Add `#[wasm_bindgen]` for `dispatch()`** — takes JSON string, returns JSON string (simple)
4. **Build a thin JS adapter** that converts current module calls into `dispatch()` calls
5. **Gradually move logic from JS modules into Rust** — each module shrinks as its logic moves
6. **Eventually**: JS is only DOM events → `dispatch()` → apply `ViewUpdate` to DOM

---

## 25. Responsiveness Architecture

### 25.1 The Problem
`dispatch(SetParam)` must return in <1ms so the slider tracks the finger. But rendering takes 3-90ms (cache hit vs miss). If `dispatch()` blocks on render, the UI stutters.

### 25.2 Split: Sync State + Async Render

```rust
impl EditorState {
    /// Synchronous — updates state, returns immediate view updates.
    /// NEVER renders pixels. Returns in <0.1ms.
    pub fn dispatch(&mut self, cmd: Command) -> Vec<ViewUpdate> {
        match cmd {
            Command::SetParam { key, value } => {
                self.adjustments.set(&key, value);
                self.render_needed = true;
                // Return the state change — view updates the slider immediately
                vec![ViewUpdate::ParamChanged { key, value }]
            }
            Command::DragMove { dx, dy } => {
                self.region.pan(dx, dy);
                // Return new region — view repositions immediately
                vec![ViewUpdate::RegionChanged { ... }]
            }
            Command::Undo => {
                self.history.undo(&mut self.adjustments);
                vec![ViewUpdate::AllParamsChanged { ... },
                     ViewUpdate::HistoryChanged { ... }]
            }
            _ => { ... }
        }
    }

    /// Asynchronous — runs the pipeline, returns pixel data.
    /// Called by the worker loop, NOT by dispatch().
    /// Checks render_needed flag, returns None if nothing to do.
    pub fn render_if_needed(&mut self) -> Option<Vec<ViewUpdate>> {
        if !self.render_needed { return None; }
        self.render_needed = false;

        let mut updates = vec![ViewUpdate::RenderStarted];

        // Overview render (~3ms from cache, ~90ms cold)
        match self.render_overview() {
            Ok(out) => updates.push(ViewUpdate::OverviewPixels { ... }),
            Err(e) => updates.push(ViewUpdate::Error { ... }),
        }
        // Detail render
        match self.render_detail() {
            Ok(out) => updates.push(ViewUpdate::DetailPixels { ... }),
            Err(e) => updates.push(ViewUpdate::Error { ... }),
        }

        updates.push(ViewUpdate::RenderComplete { elapsed_ms });
        self.adjustments.snapshot_safe();
        Some(updates)
    }
}
```

### 25.3 Worker Loop

```javascript
// worker.js
const editor = new EditorState();
const RENDER_DEBOUNCE_MS = 16; // one frame
let renderTimer = null;

self.onmessage = ({ data: cmd }) => {
  // Dispatch is synchronous — always returns instantly
  const updates = editor.dispatch(cmd);
  if (updates.length > 0) {
    self.postMessage({ type: 'updates', updates });
  }

  // Schedule async render (debounced)
  if (editor.render_needed) {
    clearTimeout(renderTimer);
    renderTimer = setTimeout(() => {
      const renderUpdates = editor.render_if_needed();
      if (renderUpdates) {
        const transfers = collectTransfers(renderUpdates);
        self.postMessage({ type: 'updates', updates: renderUpdates }, transfers);
      }
    }, RENDER_DEBOUNCE_MS);
  }
};
```

### 25.4 Main Thread Timeline

```
User drags slider
  │
  ├─ [0ms]   pointermove fires
  ├─ [0ms]   postMessage({ SetParam, key, value })
  │           (non-blocking — returns immediately)
  │
  ├─ [0ms]   CSS filter preview applied (from last known CSS approximation)
  │           Slider thumb position already updated by browser
  │
  ├─ [<1ms]  Worker: dispatch(SetParam) → [ParamChanged]
  │           Worker posts [ParamChanged] back
  │           Main thread: noop (slider already at correct position)
  │
  ├─ [16ms]  Worker: render debounce fires
  │           render_if_needed() → renders overview + detail
  │
  ├─ [19ms]  Worker posts [OverviewPixels, DetailPixels, RenderComplete]
  │           Main thread: putImageData, clear CSS filter preview
  │
  └─ [19ms]  User sees sharp rendered result
             Total perceived lag: 0ms (CSS preview was instant)
```

### 25.5 What About Long Renders?

Cold renders (cache miss, full decode + resize + filter) can take 50-200ms. During this time:

1. **CSS preview stays visible** — user sees approximate result immediately
2. **New slider events keep arriving** — `dispatch()` updates state instantly
3. **The in-progress render is cancelled** via `enough::Stop` (AtomicBool)
4. **A new render starts** with the latest state after debounce
5. **Only the final state renders** — intermediate positions are skipped

```
Slider drag: 0ms   10ms   20ms   30ms   40ms   50ms   60ms
State:       v=0.1  v=0.3  v=0.5  v=0.7  v=0.9  v=1.0  (stopped)
CSS preview: ✓      ✓      ✓      ✓      ✓      ✓
Render:      start→──────cancelled──────┘      start→──────done(v=1.0)
```

The user sees smooth CSS preview throughout. Only the final value renders through the pipeline.

### 25.6 Direct Mode (Snapseed-Style) on Image

When the user drags on the image to adjust a filter (§19.3), the same split applies:

```
User swipe up on image (Exposure mode)
  │
  ├─ [0ms]   pointermove: dy = -30px
  │           Map to exposure delta: +0.3 stops
  │           dispatch(SetParam { "exposure.stops", 0.3 })
  │
  ├─ [0ms]   CSS: canvas.style.filter = 'brightness(1.23)'
  │           Overlay: "Exposure +0.3" label appears
  │
  ├─ [<1ms]  Worker: state updated, render scheduled
  │
  ├─ [~16ms] Worker: render from cache → pixels back
  │           CSS filter removed, sharp pixels shown
  │
  └─ User perceives zero lag — CSS brightness is a perfect
     approximation for exposure in the ±2 stop range
```

### 25.7 Approximation Quality

Not all filters have good CSS approximations. When they don't, the user sees a brief delay:

| Filter | CSS Approximation | Quality | Lag Perceived |
|--------|-------------------|---------|--------------|
| Exposure | `brightness(pow(2, v))` | Excellent | None |
| Contrast | `contrast(1 + v)` | Good | None |
| Saturation | `saturate(factor)` | Good | None |
| Temperature | — | None | ~16ms (from cache) |
| Clarity | — | None | ~16ms (from cache) |
| Vignette | — | None | ~16ms (from cache) |
| Film preset | — | None | ~16ms (from cache) |

For filters without CSS approximation, 16ms (one frame from cache) is still imperceptible. The upscaled overview preview (§5.1) also provides instant visual feedback during any interaction.

### 25.8 Summary: Where Time Goes

| Operation | Time | Blocks UI? | Solution |
|-----------|------|-----------|----------|
| `dispatch()` (state change) | <0.1ms | No | Synchronous, no render |
| CSS filter preview | <0.1ms | No | Applied in same frame as input |
| Upscaled overview preview | <1ms | No | Canvas drawImage (browser) |
| Render from cache (filter suffix) | 2-5ms | No (worker) | Async in worker |
| Render cold (full pipeline) | 50-200ms | No (worker) | Async, cancellable, CSS preview covers |
| Encode preview (overview size) | 5-50ms | No (worker) | Async, debounced 200ms |
| Encode full-res export | 100ms-10s | No (worker) | Async, progress bar, cancellable |
