# ABLATION-zenpipe.md

**Date:** 2026-06-11
**Snapshot commit:** ab93e4a5 (main)
**Snapshot file:** docs/public-api/zenpipe.txt
**Snapshot items (default):** 3,114
**Mode:** COMMIT — report only, no source changes

**Grep template used:**
```
ugrep --include="*.rs" -rn "<symbol>" /home/lilith/work \
  --exclude-dir="target" --exclude-dir=".jj" --exclude-dir=".claude" \
  | grep -v "zen/zenpipe/"
```

---

## Summary

| Total items | Flagged A (doc(hidden)/deprecated) | Flagged B (pub→pub(crate) at next minor) | Flagged % |
|---|---|---|---|
| 3,114 | 0 | ~18 items across 3 areas | ~0.6% |

The vast majority of the zenpipe public surface is correct: the `bridge`, `graph`, `ops`, `sources`, `trace`, `watermark`, `sidecar`, `animation`, `session`, `orchestrate`, `cache`, `zennode_defs`, and `format` modules are all either intentional integration points (imageflow v3 backend) or data types reachable from pub field access. Conservative scan found three areas with no verified external consumers.

---

## Module-by-module analysis

### zenpipe::bridge (75 items) — KEEP

All bridge items are the core integration surface. `imageflow_compat/execute.rs` calls `build_pipeline`, `build_pipeline_traced`; `orchestrate.rs` calls `compile_nodes`; job.rs uses `NodeConverter`. No items removed.

**One candidate — B:** Three functions are defined in `bridge/mod.rs` but never called from outside it (as of this scan):

| Item | Evidence |
|------|----------|
| `bridge::record_snapshot` | Defined at bridge/mod.rs:342, only called in examples within bridge/mod.rs doc comments |
| `bridge::record_dag_snapshot` | Defined at bridge/mod.rs:387, same pattern |
| `bridge::build_riapi_trace` | Defined at bridge/mod.rs:542, zero call sites outside bridge/mod.rs — imageflow_compat/riapi.rs does NOT call it |

These three are tracing helpers intended to be called by imageflow v3 once RIAPI tracing is wired; they are `#[cfg(feature = "std")]` gated. Classification: **B at next minor** (move to `pub(crate)` or keep pub until an external caller exists). Not urgent — they do not pollute the top-level namespace.

### zenpipe::execute_layout (16 items) — MIXED

This module is `pub` in lib.rs but has zero external callers (ugrep across /home/lilith/work confirms). Internal usage: `graph.rs` calls `config_from_plan`, `streaming_from_plan_batched`, `orient_image`.

| Item | Classification |
|------|---------------|
| `execute`, `execute_layout`, `execute_with_offer`, `execute_secondary`, all `_with_background` variants | KEEP — the entry-point fns are reasonable to expose for crate consumers who want to execute a layout on raw bytes |
| `orient_image`, `fill_canvas`, `place_on_canvas`, `replicate_edges` | **B at next minor** — strip-internal helpers; no external callers found |
| `streaming_from_plan`, `streaming_from_plan_batched` | **B at next minor** — returns `zenresize::streaming::StreamingResize` directly; no external callers found |
| `config_from_plan` | **B at next minor** — internal config builder; no external callers found |

~8 items, representing ~0.3% of total. Action: move to `pub(crate)` when refactoring execute_layout for streaming strip integration.

### zenpipe::cache — geometry_split / prefix_hash / subtree_hash (root re-exports)

These three are re-exported at the crate root via `pub use cache::{PipelineCache, geometry_split, prefix_hash, subtree_hash}` (feature-gated `zennode`). They are also accessible via `zenpipe::cache::*`. No external callers found in /home/lilith/work excluding zenpipe/.

`CacheSource`, `CachedPixels`, and `PipelineCache` are the documented external cache API (editor use case, documented in cache.rs). The three hash/split utility functions are internal implementation details of `session.rs::Session::process_with_cache`.

**Classification B at next minor:** `geometry_split`, `prefix_hash`, `subtree_hash` — move from `pub use` to `pub(crate) use` (or remove the crate-root re-export; they remain accessible via `zenpipe::cache::*` for anyone who explicitly needs them).

### All other modules — KEEP

| Module | Items | Status |
|--------|-------|--------|
| zenpipe::graph | 124 | KEEP — NodeOp variants must be pub for external NodeConverter trait impls |
| zenpipe::sources | 202 | KEEP — Source trait impls used externally |
| zenpipe::trace | 171 | KEEP — TraceConfig, FullPipelineTrace returned from pub fns |
| zenpipe::watermark | 53 | KEEP — Documented geometry API, no external callers found but clearly intended pub |
| zenpipe::ops | 39 | KEEP — PixelOp trait required because NodeOp::PixelTransform(Box<dyn PixelOp>) is pub |
| zenpipe::zennode_defs | 511 | KEEP — Feature-gated zennode integration |
| zenpipe::animation | 33 | KEEP — Core animation transcode API |
| zenpipe::sidecar | 31 | KEEP — Gain-map / depth sidecar pipeline |
| zenpipe::orchestrate | 48 | KEEP — Feature-gated high-level process API |
| zenpipe::limits | 93 | KEEP — Deadline, AllocationTracker, Limits |
| zenpipe::session | 2 | KEEP |
| zenpipe::lossless | 7 | KEEP |
| zenpipe::srcset | 3 | KEEP |
| zenpipe::codec | 16 | KEEP |
| zenpipe::format | 23 | KEEP |

---

## Top-10 flagged digest

1. `execute_layout::orient_image` — no external callers; strip internal
2. `execute_layout::fill_canvas` — no external callers; strip internal
3. `execute_layout::place_on_canvas` — no external callers; strip internal
4. `execute_layout::replicate_edges` — no external callers; strip internal
5. `execute_layout::streaming_from_plan` — no external callers; exposes zenresize internal type directly
6. `execute_layout::streaming_from_plan_batched` — same
7. `execute_layout::config_from_plan` — no external callers; internal graph helper
8. `bridge::build_riapi_trace` — defined but never called from outside bridge/mod.rs as of this scan
9. `bridge::record_snapshot` — same; trace helper only used in doc examples within its own file
10. `bridge::record_dag_snapshot` — same
11. `cache::geometry_split` (root re-export) — implementation detail re-exported at crate root
12. `cache::prefix_hash` (root re-export) — same
13. `cache::subtree_hash` (root re-export) — same

All are Class B (pub→pub(crate) at next minor). Zero Class A (no doc(hidden)/deprecated proposals) — these are new enough that deprecation is not warranted.
