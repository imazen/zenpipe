# Node Ordering, Coalescing, and Optimization

## Three ordering strategies

```
1. Preserve order    — JSON steps, Rust API (user knows what they want)
2. Canonical sort    — RIAPI (keys have no order, use convention)
3. Optimize          — reorder for speed while preserving visual equivalence
```

### RIAPI canonical sort

RIAPI querystring keys have no meaningful order. `?w=800&crop=10,10,500,400`
is the same as `?crop=10,10,500,400&w=800`. The parser sorts into canonical
phase order (imageflow convention):

```
ExifOrient → Crop → Constrain/Resize → Filters → Sharpen → Encode
```

### JSON steps — user order preserved

```json
[
  {"constrain": {"w": 800, "h": 600}},
  {"sharpen": {"amount": 0.5}},
  {"encode": {"format": "jpeg", "quality": 85}}
]
```

Executed in declaration order. The bridge coalesces adjacent compatible nodes
but never reorders.

### JSON DAG — explicit topology

```json
{
  "nodes": {
    "source": {"decode": {}},
    "resize": {"constrain": {"w": 800}},
    "encode": {"encode_jpeg": {"quality": 85}}
  },
  "edges": [["source", "resize"], ["resize", "encode"]]
}
```

User controls topology completely.

## Where nodes get reattached

Seven places in the system where node order/grouping changes:

### 1. RIAPI canonical sort

- **Owner:** zenpipe Job layer (RIAPI parser) or zenlayout::riapi
- **Input:** unordered querystring keys
- **Output:** nodes sorted by canonical phase order
- **Why:** RIAPI keys have no meaningful position

### 2. Bridge coalescing

- **Owner:** zenpipe::bridge::convert::coalesce()
- **Input:** user-ordered node list
- **Output:** PipelineSteps (Single or Coalesced groups)
- **Rule:** adjacent nodes with same `CoalesceInfo.group` get merged
- **Groups:**
  - `"layout_plan"`: crop + orient + flip + rotate + constrain → single LayoutPlan
  - `"fused_adjust"`: exposure + contrast + saturation + ... → single FusedAdjust
- **Key constraint:** non-adjacent same-group nodes DON'T fuse:
  ```
  [exposure, crop, contrast] → [exposure, crop, contrast]  (3 ops)
  [exposure, contrast, crop] → [exposure+contrast, crop]   (2 ops)
  ```

### 3. Geometry fusion

- **Owner:** zenpipe::bridge::geometry::compile_geometry_run()
- **Input:** coalesced group of geometry nodes
- **Output:** single `NodeOp::Layout { plan, filter }`
- **Uses:** zenlayout::Pipeline to compose crop+orient+resize into one pass
  - D4 orientation composition (flip+rotate → single transform)
  - Crop before/after resize handling
  - Constraint resolution (fit/within/exact + aspect ratio)

### 4. Filter fusion

- **Owner:** NodeConverter::fuse_group() (implemented by zenfilters)
- **Input:** coalesced group of filter nodes
- **Output:** single `NodeOp::Filter(zenfilters::Pipeline)`
- **Uses:** zenfilters FusedAdjust for SIMD batch processing
- **Knows:** which filters can fuse (same Oklab pass) vs which need
  separate passes (neighborhood filters like sharpen)

### 5. Composite/blend reattachment

- **Owner:** zenpipe bridge DAG builder
- **Input:** Composite node with two inputs (primary + overlay)
- **Output:** `NodeOp::Composite { source, canvas, blend_mode }`
- **DAG example:**
  ```
  [decode_main] → [resize] → [composite] → [encode]
  [decode_watermark] → [resize_small] ↗
  ```

### 6. Sidecar derivation

- **Owner:** zenpipe::sidecar::SidecarPlan
- **Input:** primary pipeline's geometry transforms
- **Output:** proportional transforms for the gain map
- **Not reordering** — deriving a parallel pipeline from the primary.
  If primary does crop+resize, sidecar does proportional_crop+proportional_resize.

### 7. Encode/decode separation

- **Owner:** zenpipe::bridge::compile_nodes()
- **Input:** mixed node list
- **Output:** pixel nodes in graph + encode/decode nodes extracted separately
- **Rule:** Encode nodes produce `EncodeConfig`, not `NodeOp`. Decode hint
  nodes produce decoder configuration. Neither enters the pixel graph.

## Optimization engine

A third ordering mode beyond "preserve" and "canonical sort": reorder for
speed while preserving visual equivalence.

### Safe reorderings (lossless or nearly so)

| Reordering | Level | Rationale |
|-----------|-------|-----------|
| Move crop BEFORE resize | Speed | Fewer pixels to resample |
| Move crop BEFORE per-pixel filter | Speed | Fewer pixels to process |
| Swap adjacent per-pixel filters | Lossless | Commutative in linear light |
| Move orient BEFORE crop | Lossless | Just transforms coordinates |

### Unsafe reorderings (changes output)

| Reordering | Why |
|-----------|-----|
| Move resize BEFORE sharpen | Destroys detail that sharpen enhances |
| Move filter BEFORE neighborhood op | Sharpen sees different input |
| Move crop AFTER composite | Changes what's visible |
| Reorder non-commutative filters | Output differs (though perceptually close) |

### Nearly lossless

| Reordering | Impact |
|-----------|--------|
| Move crop before resize | ≤1px border difference from interpolation |
| Swap per-pixel filters | Mathematically different but perceptually identical |

### Optimization levels

```rust
pub enum OptimizationLevel {
    /// No reordering. User order preserved exactly.
    None,
    /// Lossless reorderings only (commutative swaps, orient coordinate rewrites).
    Lossless,
    /// Nearly-lossless reorderings (crop before resize — ≤1px border difference).
    Speed,
}
```

### Decision criteria from NodeSchema

The optimizer uses only schema metadata — no pixel knowledge:

- `role` — what kind of operation (Filter, Geometry, Orient, Encode)
- `format.changes_dimensions` — does it resize/crop
- `format.is_neighborhood` — does it read adjacent pixels (sharpen, blur)
- `coalesce.group` — what can fuse together

```rust
fn can_move_before(a: &NodeSchema, b: &NodeSchema, level: OptimizationLevel) -> bool {
    match (a.role, b.role) {
        // Crop can move before any per-pixel filter
        (Geometry, Filter) if a.format.changes_dimensions
            && !b.format.is_neighborhood => level >= Speed,

        // Per-pixel filters can swap if same coalesce group
        (Filter, Filter) if !a.format.is_neighborhood
            && !b.format.is_neighborhood
            && a.coalesce.map(|c| c.group) == b.coalesce.map(|c| c.group)
            => level >= Lossless,

        // Orient can move before crop (coordinate rewrite)
        (Orient, Geometry) => level >= Lossless,

        _ => false,
    }
}
```

## Ownership boundaries

```
zennode (schema types, validation)
  - NodeSchema, FormatHint, CoalesceInfo — vocabulary
  - Validator: "suboptimal order detected" (schema-level warnings)
  - No pixel knowledge, no graph editing

zenfilters, zenresize, etc. (declare schemas)
  - Fill in CoalesceInfo, FormatHint on their NodeSchemas
  - Implement apply(), to_encoder_config()
  - No graph knowledge

zenpipe (reads schemas, optimizes, executes)
  - Optimizer: reorder nodes using schema metadata
  - Coalescer: group adjacent same-group nodes
  - Geometry fusion: compose via zenlayout::Pipeline
  - Filter fusion: compose via NodeConverter::fuse_group()
  - Streaming execution
```

No circular dependencies. zennode provides vocabulary, crates declare facts,
zenpipe consumes them.

## Full pipeline with all ordering systems

```
Input source            Ordering strategy
─────────────          ──────────────────
RIAPI querystring  →   canonical sort (convention)
JSON steps         →   preserve order
JSON DAG           →   preserve topology
Rust API           →   preserve call order
                        │
                        ▼
              ┌─── optimize_node_order(level) ───┐
              │  Reorder for speed if requested   │
              │  (RIAPI: always Speed)            │
              │  (JSON: user choice, default None)│
              └──────────────┬───────────────────┘
                             │
                             ▼
              ┌─── coalesce (bridge) ───────────┐
              │  Group adjacent same-group nodes  │
              │  (layout_plan, fused_adjust)      │
              └──────────────┬──────────────────┘
                             │
                             ▼
              ┌─── geometry_fusion ─────────────┐
              │  Coalesced geometry → LayoutPlan  │
              │  (zenlayout composition)          │
              └──────────────┬──────────────────┘
                             │
                             ▼
              ┌─── filter_fusion ───────────────┐
              │  Coalesced filters → Pipeline     │
              │  (zenfilters FusedAdjust)          │
              └──────────────┬──────────────────┘
                             │
                             ▼
                     PipelineGraph
                     (streaming execution)
```
