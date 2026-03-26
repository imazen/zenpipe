# zenpipe

Streaming pixel pipeline with zero-materialization execution. Pull-based DAG of
image operations — decode, resize, filter, composite, encode — with only the
rows needed for the current kernel in memory at any time.

## Architecture

```mermaid
graph LR
    subgraph Input
        A[Compressed bytes] --> B[zencodec decoder]
    end
    subgraph Pipeline
        B --> C[DecoderSource]
        C --> D[Layout / Resize]
        D --> E[Format convert]
        E --> F[Filters]
        F --> G[Composite]
        G --> H[Output]
    end
    subgraph Output
        H --> I[EncoderSink]
        I --> J[zencodec encoder]
        J --> K[Encoded bytes]
    end
```

### Pull model

The sink pulls strips from the output source. Each source pulls from its
upstream source on demand. Only the rows currently needed exist in memory.

```mermaid
sequenceDiagram
    participant Sink as EncoderSink
    participant Resize as ResizeSource
    participant Decode as DecoderSource
    participant Codec as zencodec

    loop for each output strip
        Sink->>Resize: next()?
        loop fill ring buffer
            Resize->>Decode: next()?
            Decode->>Codec: next_batch()
            Codec-->>Decode: decoded rows
            Decode-->>Resize: Strip (16 rows)
        end
        Resize-->>Sink: Strip (output rows)
        Sink->>Sink: push rows to encoder
    end
    Sink->>Sink: finish()
```

### Memory model

Most operations stream — only resize ring buffers and neighborhood
filter windows allocate beyond the current strip.

```mermaid
graph TD
    subgraph "Zero materialization (streaming)"
        Crop[Crop]
        Resize[Resize — ring buffer ≈21 rows]
        Composite[Composite — synced strip pull]
        PixelOps[Per-pixel transforms]
        Filters[Per-pixel filters]
        ICC[ICC transform]
        Flip[Horizontal flip]
    end
    subgraph "Windowed materialization"
        Blur[Neighborhood filters — strip + 2×overlap rows]
    end
    subgraph "Full materialization"
        Orient[Axis-swap orientation]
        Analyze[Content analysis]
        CropWS[Whitespace crop]
        Custom[Materialize barrier]
    end
```

## Pipeline graph

Build a DAG of operations, validate, estimate memory, compile to a
pull chain, execute.

```rust
let mut graph = PipelineGraph::new();
let src = graph.add_node(NodeOp::Source);
let resize = graph.add_node(NodeOp::Resize {
    w: 800, h: 600,
    filter: ResampleFilter::Robidoux,
    sharpen_percent: 0.0,
});
let out = graph.add_node(NodeOp::Output);

graph.add_edge(src, resize, EdgeKind::Input);
graph.add_edge(resize, out, EdgeKind::Input);

// Check resource budget before executing
let estimate = graph.estimate(&source_info)?;
estimate.check(&limits)?;

// Compile and execute
let mut sources = HashMap::new();
sources.insert(src, decoded_source);
let mut pipeline = graph.compile(sources)?;

let mut sink = EncoderSink::new(encoder, output_format);
zenpipe::execute(pipeline.as_mut(), &mut sink)?;
```

## Node types

```mermaid
graph TD
    NodeOp[NodeOp]

    NodeOp --> Sources[Sources]
    Sources --> Source["Source — external input"]

    NodeOp --> Geometry[Geometry]
    Geometry --> Layout["Layout — crop+orient+resize fused"]
    Geometry --> LayoutComposite["LayoutComposite — layout + background"]
    Geometry --> CropN["Crop"]
    Geometry --> ResizeN["Resize"]
    Geometry --> Constrain["Constrain — fit/within/exact"]
    Geometry --> ResizeAdv["ResizeAdvanced"]
    Geometry --> OrientN["Orient — EXIF 1-8"]
    Geometry --> AutoOrient["AutoOrient"]

    NodeOp --> PixelOps[Pixel operations]
    PixelOps --> Transform["PixelTransform — fusible per-pixel ops"]
    PixelOps --> FilterN["Filter — zenfilters pipeline"]
    PixelOps --> IccN["IccTransform — ICC profile conversion"]

    NodeOp --> Alpha[Alpha]
    Alpha --> RemoveAlpha["RemoveAlpha — composite on matte"]
    Alpha --> AddAlpha["AddAlpha — RGB → RGBA"]

    NodeOp --> CompOps[Compositing]
    CompOps --> CompositeN["Composite — Porter-Duff blend"]
    CompOps --> OverlayN["Overlay — watermark at offset"]

    NodeOp --> Barriers[Barriers]
    Barriers --> Materialize["Materialize — full-frame transform"]
    Barriers --> AnalyzeN["Analyze — content-adaptive"]
    Barriers --> CropWS["CropWhitespace"]

    NodeOp --> Terminal[Terminal]
    Terminal --> OutputN["Output"]
```

## Zen crate integration

```mermaid
graph TB
    zenpipe((zenpipe))

    zencodec[zencodec — decode/encode]
    zenresize[zenresize — streaming resize + layout]
    zenblend[zenblend — Porter-Duff + artistic blend modes]
    zenfilters[zenfilters — photo filters on Oklab f32]
    zenpixels[zenpixels — pixel buffers + color context]
    zenpixels_convert[zenpixels-convert — row format conversion]
    zennode[zennode — declarative node definitions]
    moxcms[moxcms — ICC color management]

    zenpipe --> zencodec
    zenpipe --> zenresize
    zenpipe --> zenblend
    zenpipe --> zenfilters
    zenpipe --> zenpixels
    zenpipe --> zenpixels_convert
    zenpipe --> zennode
    zenpipe --> moxcms
```

| Crate | Role in pipeline |
|-------|-----------------|
| zencodec | DecoderSource wraps streaming decoder; EncoderSink wraps encoder |
| zenresize | Layout, Resize, Constrain nodes — streaming ring-buffer resize |
| zenblend | Composite node — blend modes on premultiplied linear f32 RGBA |
| zenfilters | Filter node — photo adjustments on Oklab f32 (per-pixel streams, neighborhood windows) |
| zenpixels | Strip type, ColorContext (ICC/CICP), metadata propagation |
| zenpixels-convert | Automatic row-level format conversion between nodes |
| zennode | Bridge: declarative node instances → PipelineGraph (optional) |
| moxcms | IccTransform node — row-by-row ICC profile conversion (optional) |

## Bridge layer (zennode → PipelineGraph)

When the `zennode` feature is enabled, declarative node definitions compile
into an executable pipeline graph with automatic fusion:

```mermaid
flowchart LR
    A["zennode instances"] --> B["separate by role
    (decode / process / encode)"]
    B --> C["coalesce adjacent
    same-group nodes"]
    C --> D["geometry fusion
    (crop+orient+flip → LayoutPlan)"]
    D --> E["filter fusion
    (exposure+contrast+... → FusedAdjust)"]
    E --> F["PipelineGraph"]
    F --> G["compile()"]
    G --> H["Box&lt;dyn Source&gt;"]
```

## Format conversion

Pixel format conversions happen automatically between nodes. Adjacent
PixelTransform nodes fuse into a single pass with ping-pong buffers.

Formats flow through the pipeline as `PixelDescriptor` values carrying
channel type (U8/U16/F32), layout (RGB/RGBA), alpha mode
(straight/premultiplied), transfer function (sRGB/linear/PQ/HLG),
and color primaries (BT.709/P3/BT.2020).

## Animation

Frame-by-frame processing for animated GIF/WebP/PNG:

1. Decode one composited frame
2. Process through per-frame pipeline (resize, filter, etc.)
3. Encode processed frame
4. Repeat

```rust
zenpipe::transcode(gif_decoder, webp_encoder, |frame_source, idx| {
    // Build per-frame pipeline, return compiled Source
})?;
```

## Resource estimation

```rust
let estimate = graph.estimate(&source_info)?;
println!("streaming: {} bytes", estimate.streaming_bytes);
println!("materialized: {} bytes", estimate.materialization_bytes);
println!("peak: {} bytes", estimate.peak_memory_bytes());

// Enforce limits before execution
estimate.check(&Limits {
    max_megapixels: Some(100),
    max_memory_bytes: Some(512 * 1024 * 1024),
    ..Default::default()
})?;
```

## Features

- `default = ["std"]` — enables zenfilters and moxcms
- `zennode` — bridge from declarative node definitions
- `nodes-all` — all codec node converters (jpeg, png, webp, gif, avif, jxl, tiff, bmp, resize, filters, etc.)
- `no_std + alloc` compatible for core pipeline

`#![forbid(unsafe_code)]` — pure safe Rust throughout.

## License

Dual-licensed: [AGPL-3.0](LICENSE-AGPL3) or [commercial](LICENSE-COMMERCIAL).

I've maintained and developed open-source image server software — and the 40+
library ecosystem it depends on — full-time since 2011. Fifteen years of
continual maintenance, backwards compatibility, support, and the (very rare)
security patch. That kind of stability requires sustainable funding, and
dual-licensing is how we make it work without venture capital or rug-pulls.
Support sustainable and secure software; swap patch tuesday for patch leap-year.

[Our open-source products](https://www.imazen.io/open-source)

**Your options:**

- **Startup license** — $1 if your company has under $1M revenue and fewer
  than 5 employees. [Get a key →](https://www.imazen.io/pricing)
- **Commercial subscription** — Governed by the Imazen Site-wide Subscription
  License v1.1 or later. Apache 2.0-like terms, no source-sharing requirement.
  Sliding scale by company size.
  [Pricing & 60-day free trial →](https://www.imazen.io/pricing)
- **AGPL v3** — Free and open. Share your source if you distribute.

See [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.
