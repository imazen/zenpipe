# Changelog

## [Unreleased]

### Added
- `ClipartFlatten` filter: flattens AI-clipart "waviness" / bubble-noise inside
  nominally-flat colour regions while keeping crisp edges and intentional shading
  (complement to `BackgroundFlatten`, which only touches the background). Built-in
  OKLab k-means quantizer (with near-duplicate centroid merging) → connected
  regions per palette colour → per-region mean + variance; eases flat-fill
  interiors toward their clean region mean by `strength × region_flatness ×
  boundary_keep × membership`, so shaded regions, region boundaries, and
  anti-aliased/off-colour pixels are preserved. `Describe` schema + 6 tests.
  `examples/clipart_flatten_demo.rs` runs it over a clipart dir with zensim-scored
  before/after/diff output. (f45ca9b)
- `BackgroundFlatten` filter: conservative, automated white-background flattening
  for e-commerce product photos. Estimates the border background and skips
  non-white-background shots (with a central-subject gate that rejects bright
  high-key / sky scenes); grows an edge-seeded flood-fill background mask so only
  border-connected background is touched; feathers the effect to zero at the
  product silhouette; fits a low-order surface so gradient/uneven backgrounds
  flatten uniformly; eases the background to pure white with a shadow-preserving
  soft knee and a max-lift cap; neutralizes background color cast; and removes
  halos/fringes in the silhouette-side band (guided-filter smoothing + overshoot
  clamp + chroma decontamination). `Describe` schema included. (3646257, 87f0201, 6a78411, d3ab689)
- `metric_gate` module: `MetricGated<M>` wraps any filter with a perceptual
  quality gate — apply, score the change with a pluggable `QualityMetric`, then
  binary-search the edit strength back under a just-noticeable threshold (or skip).
  Any `Fn(&OklabPlanes, &OklabPlanes) -> f32` is a metric, so zensim et al. plug
  in without new dependencies; `OklabDeltaMetric` is a zero-dep default. (e9019c5)
- `whitebg_corpus` example (experimental): runs `BackgroundFlatten` over synthetic
  white-bg scenes + CID22 safety samples + a `--input` dir, scores with zensim,
  scales back / skips, and writes before/after/diff images + a CSV report. (d3ab689)

## 0.1.0 — 2026-04-01

Initial release.

- 51 stable filters across exposure, tone, color, detail, effects, and document analysis
- Planar Oklab f32 layout for maximum SIMD throughput via archmage
- FusedAdjust: 10 core operations in a single SIMD pass
- Separable Gaussian blur on L plane (188x faster than naive interleaved)
- Film look presets (20+ cinematic color grades)
- Regional comparison infrastructure (luminance zones, chroma zones, hue sectors)
- Image segmentation and saliency-aware features
- Document analysis: deskew detection, line segment detection, quad detection, homography
- Experimental: Warp with projective transforms and Robidoux interpolation (SIMD-accelerated)
- serde support for filter parameters
- sRGB convenience filters via `srgb-filters` feature
- zennode graph definitions via `zennode` feature
- `no_std + alloc` compatible, `#![forbid(unsafe_code)]`
