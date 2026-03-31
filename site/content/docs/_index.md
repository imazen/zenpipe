+++
title = "Documentation"
description = "Zenpipe documentation — architecture, formats, and querystring API"
sort_by = "weight"
template = "section.html"
+++

Zenpipe is a streaming pixel pipeline that composes image operations into a pull-based DAG. Operations run row-by-row — only the rows needed for the current kernel exist in memory at any time.

## Architecture

```
Decode → Orient → Crop/Region → Resize/Constrain → Filters → Composite → Encode
```

### Memory Model

| Strategy | When | Examples |
|----------|------|---------|
| **Zero materialization** | No pixel neighborhood needed | Crop, per-pixel transforms, ICC, horizontal flip |
| **Windowed** | Small neighborhood | Blur, sharpen, bilateral — strip + 2x overlap rows |
| **Full materialization** | Whole image needed | Orientation, content analysis, whitespace crop |

### Zen Crate Stack

Zenpipe composes these crates into a unified pipeline:

- **zencodec** / **zencodecs** — decode/encode traits and unified format dispatch
- **zenresize** — streaming resize with 31 filter kernels (V-first, row push/pull, ring buffer)
- **zenlayout** — constraint modes, orientation, smart crop geometry
- **zenblend** — Porter-Duff + artistic blend modes on premultiplied linear f32 RGBA
- **zenfilters** — 43 photo filters on planar Oklab f32 with SIMD
- **zenpixels** / **zenpixels-convert** — pixel buffers, color context, row format conversion
- **moxcms** — ICC color management
- **zennode** — declarative self-documenting node definitions
