# ABLATION-zenlayout.md

**Date:** 2026-06-11
**Snapshot commit:** ab93e4a5 (main)
**Snapshot file:** docs/public-api/zenlayout.txt
**Snapshot items (default):** 1,915 | (all features, excl. `_*`):** 2,351
**Mode:** COMMIT — report only, no source changes

**Grep template used:**
```
ugrep --include="*.rs" -rn "<symbol>" /home/lilith/work \
  --exclude-dir="target" --exclude-dir=".jj" --exclude-dir=".claude" \
  | grep -v "zen/zenpipe/zenlayout/"
```

---

## Summary

| Feature set | Items | Flagged A | Flagged B | Flagged % |
|---|---|---|---|---|
| default | 1,915 | 0 | 0 | 0% |
| all features (excl. `_*`) | 2,351 | 0 | 0 | 0% |

**Clean.** zenlayout is a geometry-only crate with no pixel operations. Its public surface is consistently correct. All modules are either core layout math or feature-gated extensions. The all-features delta of 436 items is fully accounted for by the `riapi`, `smart-crop`, and `svg` feature flags.

---

## Default features: module analysis (1,915 items)

### zenlayout::constraint (~380 items) — KEEP

Core public API: `Constraint`, `ConstraintMode`, `Layout`, `CanvasColor`, `Gravity`, `SourceCrop`, `LayoutError`, `Rect`, `Size`. Heavily used by zenpipe (`execute_layout.rs`, `graph.rs` NodeOp::Constrain), zenjpeg tests, imageflow_compat. All correct.

### zenlayout::dimension (~80 items) — KEEP

Dimension arithmetic helpers. Used internally by constraint and plan modules.

### zenlayout::orientation (~60 items) — KEEP

`Orientation` enum with all 8 EXIF orientations. Used by zenpipe::graph (NodeOp::Orient), zencodec, heic, zenjpeg. All correct.

### zenlayout::plan (~1,400 items) — KEEP

The largest module: `LayoutPlan`, `IdealLayout`, `DecoderRequest`, `DecoderOffer`, `Region`, `LayoutPlanStep`, and geometry computation methods. Used extensively by zenpipe::execute_layout and zenpipe::graph. This is the core layout computation engine — all items are intentionally pub.

---

## All-features delta: +436 items — all feature-gated

| Module | Items | Gate | Notes |
|--------|-------|------|-------|
| `zenlayout::riapi` | ~312 | `riapi` feature | RIAPI querystring parser, `parse()`, `CFocus`, `ParseWarning`, instruction set |
| `zenlayout::smart_crop` | ~122 | `smart-crop` feature | `SmartCropInput`, `HeatMap`, `CropConfig`, `FocusRect`, `AspectRatio` |
| `zenlayout::svg` | ~2 | `svg` feature | SVG path helper |

All three are deliberate feature-gated additions. `zenlayout::riapi` is used by `imageflow_compat/riapi.rs` (expand_zen path). `zenlayout::smart_crop` is used by zenpipe::session and zensally. Both are confirmed legitimate.

---

## Special note: no serde or SIMD expansion

zenlayout has no SIMD (pure math, no pixel ops) and no serde feature. The all-features section does not add any auto-derived impls — the 436-item delta is purely the three feature-gated modules above. No anomaly.

---

## Top-10 flagged digest

**None.** No mistakes found.

Informational note: `zenlayout::riapi::CFocus` enum and `zenlayout::riapi::instructions::CFocus` both appear in the all-features snapshot (the instructions submodule re-exports the same type). This is a minor redundancy in the snapshot but not a public-API mistake. The type appears once at `riapi::CFocus` and once under `riapi::instructions::CFocus` — if the instructions module re-export is unintentional, consider checking whether `pub use` in `riapi/instructions.rs` was intended. Not flagged under conservative mode.
