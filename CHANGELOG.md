# Changelog

All notable changes to the zenpipe workspace are documented here, per crate.
(Started 2026-06-11; earlier history lives in git log.)

## zenpipe

### [Unreleased]

#### Fixed

- **Gain-map sidecars resample in encoded space** (#41, b938a2b0): sidecar
  pixels are log2-quantized gain values, not color — Skia's SkGainmapShader
  and libultrahdr interpolate them raw. The decode path now labels sidecar
  strips `TransferFunction::Linear`; `NodeOp::Layout` derives its working
  format from the source transfer (`RGBA8_LINEAR` for Linear sources, so
  zenresize does raw u8↔f32 with no gamma round-trip); `ResizeSource`
  carries the working format instead of hardcoding `RGBA8_SRGB`. Previously
  resized Preserve jobs bounced gain values through the sRGB EOTF/OETF.
- **Gain-map re-embed repacks the materialized sidecar** (#41, b938a2b0):
  tight 1-/3-channel packing driven by ISO 21496-1
  `GainMapParams::is_single_channel()` (metadata-driven, not pixel
  inspection), fixing the latent corruption where a resized RGBA sidecar
  was re-embedded as raw RGBA bytes labeled `channels: 3`.

## zencodecs

### [Unreleased]

#### Added

- **CI now compiles and tests the gain-map/UltraHDR/raw surface** (#38):
  new workflow run + `just test-gainmap-surface` covering
  `jpeg-ultrahdr`/`raw-decode-gainmap` and the avif-less codec set —
  these targets sat uncompilable for weeks with nothing on CI building
  them. Widen to `all,cms,std` once the zencodec↔zenavif drift settles.
- `local-fixtures` feature: caller-controlled gate for tests reading
  dev-workstation-only fixtures (`icc_srgb` reads sibling jpegli-cpp ICC
  profile trees); CI never enables it, `just test-local-fixtures` does.

#### Changed

- Feature-conditional test hygiene so every CI feature combination runs
  green instead of failing on tests for codecs that are compiled out:
  `regress` requires `all` (its checksum baselines are recorded under the
  full set per its docs), `stop_and_limits` requires `jpeg,webp,gif` with
  avif legs cfg-gated, selection/encode unit tests gate on the codec
  corpus they exercise, and the avif trace test gates on `nodes-avif`.
- `metadata_conformance`: PNG `orient_from_exif` promoted Gap → Ok —
  zenpng now normalizes the eXIf orientation tag into `info.orientation`
  on decode (stricter pin; regression-guarded both directions).
- `png_capability::png_cicp_chunk_round_trips`: expect
  `matrix_coefficients = 0` after the cICP round-trip — PNG-3 §11.3.2.6
  requires matrix 0 (RGB storage); echoing the source's matrix 9
  verbatim, as the test originally pinned, was a spec violation. The
  encoded chunk is `[9, 16, 0, 1]` (verified at the byte level).
- `icc_srgb`: the 8 expectations stale against the zenpixels-convert
  0.2.13 normalized hash DB flipped to `false` with measured ground truth
  (#42, via zenpixels `icc-gen --bin probe42`): e-sRGB and the v4
  LUT/preference profiles have →sRGB identity errors of ~9–112 u8 steps
  (real transforms the old recognizer silently skipped); the v5/iccMAX
  trio doesn't parse under the production CMS at all. The 0.2.13
  narrowing was correct on every count.

#### Fixed

- **UltraHDR encode derives the color gamut from CICP metadata** (#40):
  `encode_ultrahdr_rgb_f32` / `encode_ultrahdr_rgba_f32` previously ignored
  their metadata parameter and hardcoded BT.709. CICP color primaries 1/2 →
  BT.709, 12 → Display P3, 9 → BT.2100; an explicit code outside the three
  UltraHDR gamuts is an encode error (`UnsupportedOperation`) rather than a
  silent BT.709 fallback, which would compute wrong gain-map luma.
