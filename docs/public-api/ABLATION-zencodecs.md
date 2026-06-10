# ABLATION-zencodecs.md

**Date:** 2026-06-11
**Snapshot commit:** ab93e4a5 (main)
**Snapshot file:** docs/public-api/zencodecs.txt
**Snapshot items (default):** 1,807 | (jxl-encode,cms features):** 1,942
**Mode:** COMMIT — report only, no source changes

**Grep template used:**
```
ugrep --include="*.rs" -rn "<symbol>" /home/lilith/work \
  --exclude-dir="target" --exclude-dir=".jj" --exclude-dir=".claude" \
  | grep -v "zen/zenpipe/"
```

---

## Summary

| Total items | Flagged A | Flagged B | Flagged % |
|---|---|---|---|
| 1,807 (default) | 0 | 0 | 0% |

**Clean.** No public-API mistakes found. All 22 public modules are either core codec API, deliberate integration helpers, or feature-gated extensions. The jxl-encode,cms feature adds 135 items (cms module + JxlEncodeJob/JxlEncoderConfig re-exports) — all deliberate.

---

## Module-by-module analysis

### zencodecs::codec_id — KEEP (24 items)

`CodecId` enum is the codec dispatch key. Used by hdr-corpus-convert and zensquoosh externally.

### zencodecs::config — KEEP (~300 items)

All per-codec config types (jpeg, webp, png, gif, avif decode/encode). Used extensively by hdr-corpus-convert and codec-eval externally.

### zencodecs::color — KEEP (~20 items)

`SourceColorExt::is_srgb()` and `icc_profile_is_srgb()`. Helpers for ICC classification used at zenpipe boundary.

### zencodecs::decision + zencodecs::select — KEEP (~50 items combined)

`FormatDecision` and `ImageFacts` + `select_format_from_intent`. These are the codec selection output types. Used directly by imageflow_compat/execute.rs (confirmed: `use zencodecs::{AllowedFormats, CodecPolicy, ImageFacts, select_format_from_intent}`).

### zencodecs::trace — KEEP (~30 items)

`SelectionTrace` and `SelectionStep`. Required because `FormatDecision::trace` is a pub field of type `Vec<SelectionStep>`. Cannot be hidden without breaking the FormatDecision pub struct.

### zencodecs::depthmap — KEEP (~25 items)

`DecodedDepthMap`, `DepthImage`, `DepthMapMetadata`. Feature-complete depth map extraction API. No external callers found in this scan, but this is a planned external API (HEIC portrait mode, JPEG MPF). Not a leak — deliberate design.

### zencodecs::exif — KEEP (~20 items)

EXIF read/write helpers. Used by zenpipe::sidecar and imageflow_compat. Legitimate pub.

### zencodecs::gainmap — KEEP (~40 items)

`GainMapInfo`, `GainMapParams`, etc. Used by hdr-corpus-convert and ultrahdr. Verified external usage.

### zencodecs::transcode — KEEP (~60 items)

`TranscodeOptions`, `TranscodeOutput`, `SupplementPolicy`, `SupplementSet`, `TranscodeSink`. Used by zenpipe::codec (JBRD path) and zencodecs::v2 transcode API.

### zencodecs::policy, zencodecs::quality, zencodecs::intent — KEEP

Core codec policy/intent types. Used by zenpipe bridge and imageflow_compat.

### zencodecs::pixel — KEEP

`PixelBufferConvertExt`, `PixelBufferConvertTypedExt`. Re-exported at crate root.

### zencodecs::cms (jxl-encode,cms features only) — KEEP

`CicsValues`, `CmsMode`, `PngColorInfo`. Feature-gated; adds 135 items. Deliberate color management API.

### zencodecs::riapi_parse (riapi feature) — KEEP

`parse_codec_keys`, `CodecEngine`. Feature-gated (`riapi`); not in default snapshot. Used by imageflow_compat.

### zencodecs::zennode_defs (zennode feature) — KEEP

Feature-gated zennode integration. Not in default snapshot.

---

## all-features delta (jxl-encode,cms): +135 items

All 135 added items are cms module items or JxlEncodeJob/JxlEncoderConfig re-exports at crate root — deliberate feature-gated additions.

---

## Top-10 flagged digest

**None.** No mistakes found. All modules are intentional public API.

One informational note: `zencodecs::depthmap` has zero external callers in this scan. It is a planned API (not a leak), and KEEP under the conservative default. If this API is not stabilized before 0.2.0, consider `#[doc(hidden)]` (Class A) to reduce surface during the 0.x period.
