# Changelog

All notable changes to `zencodecs` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/); this crate is pre-1.0, so any
0.x bump may break the API.

## [Unreleased]

### Added
- **Metadata retention policy** (zencodec 0.1.21 adoption):
  `EncodeRequest::with_metadata_policy(MetadataPolicy)` and
  `TranscodeOptions::metadata_policy`. Defaults to `MetadataPolicy::PreserveExact`
  (verbatim embed, with a stale EXIF orientation tag reconciled — the
  double-rotation guard); set `MetadataPolicy::Web` to strip
  GPS/camera/timestamps/XMP for web publishing. `MetadataPolicy` is re-exported at
  the crate root.
- **Color-emission policy** convenience:
  `EncodeRequest::with_color_emit_policy(ColorEmitPolicy)` — merges into the encode
  policy's `color` field; codecs resolve it via `EncodePolicy::resolve_color` +
  `resolve_color_emit`. `ColorEmitPolicy` re-exported.
- **Quality-targeted transcode**: `transcode_to_quality(data, target, target_zq, ..)`
  — coefficient-domain, no pixel re-encode. JPEG→JPEG via `zenjpeg::recompress`;
  JPEG→JXL lossy via `zenjxl::jpeg_lossy` driven by a zensim Profile-A scorer
  (feature `transcode-iqa`).
- **Lossless byte-exact JPEG↔JXL transcode** (JBRD / brunsli-parity):
  - `transcode_jpeg_to_jxl_lossless(jpeg, effort)` — JPEG→JXL, byte-exactly
    recoverable (feature `jpeg-jxl-transcode`, via
    `zenjxl::LosslessConfig::encode_jpeg_transcode`).
  - `reconstruct_jpeg_from_jxl(jxl)` — the inverse JXL→JPEG reconstruction (feature
    `jxl-jpeg-reconstruct`, via `zenjxl_decoder::reconstruct_jpeg`; decode-side only,
    needs neither zenjpeg nor the JXL encoder).
  - New optional dependency `zenjxl-decoder` (with its `jpeg` feature for the JBRD box).

### Changed
- Migrated the encode dispatch (`dispatch.rs`, `dyn_dispatch.rs`) off the
  now-deprecated `EncodeJob::with_metadata` to `EncodeJob::with_metadata_policy` —
  eliminates 4 deprecation warnings and routes the retention policy to the codec
  boundary.
- `TranscodeOptions` gained a hand-written `Default` (was derived) because
  `MetadataPolicy` has no `Default` by design (metadata retention is an explicit
  privacy choice).

### Fixed
- `tests/metadata_conformance.rs`: bind the parsed `Exif` before calling
  `copyright()`, which now returns a borrowing `Cow` in zencodec 0.1.21 (was a
  borrow-after-move, E0515).
