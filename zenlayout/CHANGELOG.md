# Changelog

## 0.2.0

### Added
- **RIAPI query string parsing** — `riapi` feature (`?w=800&h=600&mode=crop`)
  - Full mode/scale matrix: Max, Pad, Crop, Stretch, AspectCrop combined with DownscaleOnly, Both, UpscaleOnly, UpscaleCanvas
  - Crop, anchor/gravity, orientation (srotate/sflip/rotate/flip), zoom/DPR
  - Background color via hex or CSS3 named colors
  - Non-layout keys preserved in `extras` for downstream consumers
  - Comprehensive parity tests against legacy RIAPI behavior
- **Smart crop module** for content-aware cropping (`smart-crop` feature, experimental)
- **SVG visualization** of layout pipeline steps (`svg` feature)
- **`PadWithin` constraint mode** — never upscales, always pads to target canvas
- **`NonFiniteFloat` error variant** — NaN/Inf rejected at all API boundaries
- **NaN/Inf validation** at `Constraint::compute()`, `Instructions::to_pipeline()`, and region resolution
- `#[non_exhaustive]` on all public enums and structs
- `Default` on most public types (excludes `DecoderRequest`, `DecoderOffer`, `LayoutPlan`)
- Layout pipeline examples with SVG diagrams (`doc/layout-examples.md`)
- AGPL-3.0-or-later license file
- CI: 6-platform matrix (Linux/macOS/Windows, x64/arm64), i686 cross-compilation, WASM, MSRV check, Codecov

### Fixed
- `no_std` compilation — float math (`round`/`floor`/`ceil`) via internal `F64Ext` trait
- `UpscaleOnly` and `UpscaleCanvas` scale modes in RIAPI layer (previously unimplemented)

### Changed
- Edition 2024, MSRV 1.89
- `compute_layout_sequential` gated behind `alloc` feature (uses `Vec`)

## 0.1.0

Initial release. Pure geometry image layout computation with constraint modes,
EXIF orientation (D4 dihedral group), and decoder negotiation. `no_std + alloc`,
`#![forbid(unsafe_code)]`.
