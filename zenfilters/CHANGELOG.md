# Changelog

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
