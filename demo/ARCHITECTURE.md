# zenpipe WASM Editor Demo — Architecture

## Overview

Interactive image editor running entirely in the browser via WASM.
All pixel work happens on a Web Worker; the main thread handles only UI
and CSS-approximated previews.

## Dual-View Display

```
+--------------------------------------------------+
|  [Overview]  small, fits viewport, blurred        |
|  ┌────────────────────────────────┐               |
|  │  full image, resized to ~512px │               |
|  │  CSS blur(2px) + filter approx │               |
|  │  ┌──────┐                      │               |
|  │  │region│ draggable selector   │               |
|  │  └──────┘                      │               |
|  └────────────────────────────────┘               |
|                                                    |
|  [Detail]  cropped region at higher resolution     |
|  ┌────────────────────────────────┐               |
|  │  1:1 or 2x crop of the region │               |
|  │  sharp, fully rendered pixels  │               |
|  └────────────────────────────────┘               |
|                                                    |
|  [Sliders]  exposure contrast clarity ...          |
+--------------------------------------------------+
```

### Overview canvas
- Resized to ~512px max dim via Session (geometry prefix cached)
- CSS `filter: blur(2px) brightness(...) contrast(...) saturate(...)` for instant preview
- Draggable rectangle shows the detail region
- On slider change: CSS filter updates instantly, worker re-renders suffix only

### Detail canvas
- Crops a region from the *original* source at higher resolution
- Region position controlled by dragging the rectangle on the overview
- Separate Session with different geometry (crop + constrain to detail canvas size)
- Re-rendered on region move or filter change (fast — Session caches the crop prefix)

## Worker Protocol

```typescript
// Main → Worker
{ id, type: 'init',     data: ArrayBuffer }     // load source image
{ id, type: 'overview', adjustments: {...} }     // render overview (uses Session cache)
{ id, type: 'region',   adjustments: {...},      // render detail region
                         rect: {x,y,w,h} }       // normalized 0..1 coords
{ id, type: 'export',   adjustments: {...},      // full-res export
                         format: 'jpeg'|'png',
                         quality: number }

// Worker → Main
{ id, type: 'result',   data: ImageBitmap,       // transferable
                         width, height }
{ id, type: 'ready',    width, height,            // after init
                         overviewWidth, overviewHeight }
{ id, type: 'error',    message: string }
```

## Render Lifecycle

```
Slider input
  ├─ CSS filter update (sync, <1ms)
  ├─ Show spinner on detail canvas
  └─ requestAnimationFrame
       ├─ postMessage('overview', adjustments)    → Session hit (~3ms)
       │    └─ ImageBitmap → overview canvas
       │         └─ Remove CSS filter, hide spinner
       └─ postMessage('region', adjustments, rect) → Session hit (~1ms for small crop)
            └─ ImageBitmap → detail canvas

Region drag
  ├─ Update rectangle position (sync CSS transform)
  └─ requestAnimationFrame
       └─ postMessage('region', adjustments, rect)
            └─ ImageBitmap → detail canvas
```

## Session Cache Strategy

Two Session instances in the worker:

1. **Overview Session** — source → resize(512) → [filters] → output
   - Geometry prefix (decode + resize to 512px) cached after first render
   - Filter suffix re-runs on each slider change (~3ms from cache)

2. **Region Session** — source → crop(rect) → resize(detail_size) → [filters] → output
   - Geometry prefix (decode + crop + resize) cached per region position
   - On region move: new prefix (cache miss, but crop+resize on pre-decoded source)
   - On filter change at same position: cache hit (~1ms)

## CSS Filter Approximation

Maps zenfilters adjustments to CSS filter() as instant preview:

| Adjustment | CSS filter | Mapping |
|-----------|-----------|---------|
| exposure  | brightness() | `pow(2, exposure)` |
| contrast  | contrast() | `1 + contrast` |
| saturation | saturate() | `1 + saturation + vibrance*0.5` |
| clarity   | (none) | No CSS equivalent — shows as spinner delay |
| shadows   | (none) | No CSS equivalent |
| highlights | (none) | No CSS equivalent |

Filters without CSS equivalents rely on the spinner + fast Session render.
