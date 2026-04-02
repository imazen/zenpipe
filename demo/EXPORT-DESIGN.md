# Export System Design

## Overview

The export system lets users try different format/quality combinations quickly,
see previews inline, preserve metadata/HDR/gain maps, and build an export history.

## Two-Panel Export

### Panel 1: Quick Export (current modal, enhanced)
- Format tabs: JPEG, WebP, PNG, JXL, GIF (+ AVIF when available)
- Per-format quality controls from zencodecs audit
- Output dimensions with aspect lock
- **Preview pane**: small encoded preview shown inline
  - Overlays: bits per pixel, resolution, file size
  - Estimated full-size file from the small preview's compression ratio
- "Export" button downloads at chosen resolution
- "Add to History" button saves to export history without downloading

### Panel 2: Advanced / Preservation
- **Metadata preservation**: toggle checkboxes
  - EXIF (orientation, camera info)
  - XMP (editing metadata)
  - ICC profile (color management)
  - CICP (HDR signaling)
- **HDR / Gain Map**: (when source has gain map)
  - "Preserve gain map" → embeds gain map for UltraHDR/ISO 21496-1
  - "Discard gain map" → SDR-only output
  - "Reconstruct HDR" → apply gain map for HDR output
  - Preview: show gain-mapped version in browser (if supported)
- **Color space**:
  - sRGB (default, widest compatibility)
  - Display P3 (wider gamut, Apple ecosystem)
  - Rec.2020 (HDR workflows)
- **Bit depth**:
  - 8-bit (default)
  - 16-bit (PNG, JXL, AVIF)
  - Float (JXL)

## Export History (collapsible section below export modal)
- Each export: thumbnail, format badge, file size, dimensions, timestamp
- Click to view full-size in a new tab (blob URL, no download)
- Download button per entry
- "Clear History" button
- Stored in memory (not IndexedDB — lost on page refresh)
- Shows compression ratio trend across attempts

## Codec Support Matrix

| Codec | Engine | Lossless | HDR/WCG | Gain Map | Metadata | Bit Depth |
|-------|--------|----------|---------|----------|----------|-----------|
| JPEG  | zenjpeg (WASM) | No | No | UltraHDR | EXIF,XMP,ICC | 8 |
| WebP  | zenwebp (WASM) | Yes | No | No | EXIF,XMP,ICC | 8 |
| PNG   | zenpng (WASM) | Yes | 16-bit | No | ICC,tEXt | 8,16 |
| JXL   | jxl-encoder (WASM) | Yes | Yes | Yes | EXIF,XMP,ICC | 8,16,float |
| GIF   | zengif (WASM) | Palette | No | No | None | 8 (palette) |
| AVIF  | browser fallback* | Yes | Yes | No | EXIF,XMP | 8,10,12 |

*AVIF encoding via zenrav1e has a build.rs — needs WASM compat verification.

## End-to-End Pipeline for High-Quality Export

For metadata/HDR preservation, the export path must go through zenpipe's
full pipeline (not just `pack_rgba` → encode):

```
source → Session::stream(filters) → zencodec Encoder
                                     ↑
                        metadata passthrough (EXIF, XMP, ICC, CICP)
                        gain map sidecar handling
                        bit depth preservation (no u8 quantization)
```

This means the encode step should receive the `StreamingOutput` directly
(with its metadata and sidecar), not just the materialized RGBA8 pixels.

## Preview Workflow

```
User selects format + quality
  → encode small preview (overview-size, ~512px)
  → display in preview pane with overlay stats
  → show "Est. full size: 2.4 MB" extrapolated from preview bpp
  
User clicks "Export"
  → encode at full/chosen resolution  
  → progress bar (from strip count / total strips)
  → cancellable via enough::Stop
  → add to history
  → auto-download
```

## UX Principles

1. **Try fast, decide slow**: small preview encodes are instant (~50ms for 512px),
   users should be able to flip between formats/qualities rapidly
2. **Show, don't tell**: inline preview with quality artifacts visible at 1:1
3. **No data loss by default**: metadata preserved unless explicitly discarded
4. **History enables comparison**: encode the same image at JPEG 75 and WebP 80,
   compare file sizes and visual quality side by side
5. **Clear browser vs WASM indicator**: "(zen codecs)" or "(browser)" badge per format
