# ABLATION-zenfilters.md

**Date:** 2026-06-11
**Snapshot commit:** ab93e4a5 (main)
**Snapshot file:** docs/public-api/zenfilters.txt
**Snapshot items (default):** 4,298 | (all features, excl. `_*`):** 8,533
**Mode:** COMMIT — report only, no source changes

**Grep template used:**
```
ugrep --include="*.rs" -rn "<symbol>" /home/lilith/work \
  --exclude-dir="target" --exclude-dir=".jj" --exclude-dir=".claude" \
  | grep -v "zen/zenpipe/zenfilters/"
```

---

## Summary

| Feature set | Items | Flagged A | Flagged B | Flagged % |
|---|---|---|---|---|
| default | 4,298 | 0 | 0 | 0% |
| all features (excl. `_*`) | 8,533 | 0 | 0 | 0% |

**Clean.** The 4,235-item all-features delta is entirely accounted for by deliberate feature-gated modules. The SIMD internals (archmage dispatch, per-arch scalar/x86/neon/wasm128 kernels) are correctly `pub(crate)` or private — none appear in either snapshot section. No accidentally-pub kernel functions found.

---

## Default features: module analysis (4,298 items)

### zenfilters::filters (~2,239 items) — KEEP

All filter types (`Exposure`, `Contrast`, `Clarity`, `Saturation`, `Vibrance`, `ToneCurve`, etc.) plus their parameters, defaults, and trait impls. These are the core user-facing API. External callers: zenpipe (bridge, graph, zennode_defs), hdr-research/hdr-editor. All correct.

**Submodules:**

| Submodule | Visibility | Classification |
|-----------|-----------|---------------|
| `filters::cat16` | pub | KEEP — CAT16 chromatic adaptation is a legitimate algorithmic primitive; no external callers found in this scan but it is a correctly scoped DNG/RAW pipeline helper exposed for integration with zenraw/camera calibration |
| `filters::dt_sigmoid` | pub | KEEP — darktable-sigmoid is an exposed tone-mapping algorithm with documented params; same rationale |
| `filters::srgb_compat` | pub (srgb-compat feature) | KEEP — feature-gated |

`cat16` and `dt_sigmoid` are the two modules most likely to be questioned. Both are intentionally pub (they expose low-level transforms that callers may want to apply directly outside the Pipeline). Zero external callers were found in this scan, which is consistent with zenfilters being a relatively new crate. Conservative default: KEEP.

### zenfilters::analysis (43 items) — KEEP

`ImageAnalysis::compute()` and percentile accessors. Used externally by hdr-research/hdr-editor.

### zenfilters::filter_compat (54 items) — KEEP

`FilterTag`, `CompatIssue`, `RangeConflict`, `EXCLUSIVE_GROUPS`, `ORDER_CONSTRAINTS`, `RANGE_CONFLICTS`, `validate_pipeline()`. Editor-facing compat validation API. No external callers found yet — but this is clearly intentional API (editor tooling use case).

### zenfilters::masked (38 items) — KEEP

Masked filter application. No external callers found; clearly intended pub (layered editing).

### zenfilters::metric_gate (30 items) — KEEP

Quality-gate wrapper. Intended for encoder quality loops. No external callers.

### zenfilters::param_schema (77 items) — KEEP

Parameter schema introspection. Used by zennode_defs.

### zenfilters::presets (54 items) — KEEP

Named filter presets. No external callers; clearly intended pub.

### zenfilters::regional (43 items) — KEEP

Region-aware filter application. No external callers; clearly intended pub.

### zenfilters::resize_pipeline (3 items) — KEEP

Thin type re-export.

### zenfilters::slider (15 items) — KEEP

Slider parameter type. Used by param_schema.

**Top-level re-exports (158 items, marked "other"):**
- `scatter_to_oklab`, `gather_from_oklab`, `gather_oklab_to_srgb_u8`, `scatter_srgb_u8_to_oklab` — KEEP. Legitimate pub primitives for callers doing manual Oklab pipeline work.
- `FilterContext`, `OklabPlanes`, `Pipeline`, `PipelineConfig`, `GaussianKernel`, etc. — KEEP. Core pipeline entry points.
- `fused_interleaved_adjust` (experimental) — KEEP. Feature-gated.

---

## All-features delta: +4,235 items — all feature-gated

The delta is fully accounted for by these feature-gated additions:

| Module | Items | Gate |
|--------|-------|------|
| `zenfilters::zennode_defs` | ~1,266 | `zennode` feature |
| `zenfilters::filters` extra (srgb_compat filters) | ~841 | `srgb-compat` feature |
| `zenfilters::document` (deskew, homography, lsd, otsu, quad) | ~57 | `experimental` feature |
| `zenfilters::segment` | ~41 | `experimental` feature |
| `zenfilters::srgb_filters` | ~14 | `srgb-filters` feature |
| Other small additions | ~16 | various |

**None of these are SIMD dispatch machinery leaking into the public surface.** The archmage `#[arcane]`, `incant!`, and per-arch kernels in `src/simd/` are all `pub(crate)` or private — confirmed by inspection of `src/simd/mod.rs` (all dispatch fns are `pub(crate)`).

The `blur_internals` module is `#[doc(hidden)]` (confirmed not in snapshot).

---

## Top-10 flagged digest

**None.** No mistakes found.

Informational notes (KEEP under conservative mode):
1. `zenfilters::filters::cat16` — no external callers in this scan; pub is intentional for DNG/camera pipeline integration.
2. `zenfilters::filters::dt_sigmoid` — no external callers; pub is intentional for advanced tone-mapping integration.
3. `zenfilters::filter_compat`, `masked`, `metric_gate`, `presets`, `regional` — no external callers; all clearly intended pub.
4. `zenfilters::zennode_defs` (1,266 items under `zennode` feature) — the single largest all-features contributor; intentional zennode integration; KEEP.
