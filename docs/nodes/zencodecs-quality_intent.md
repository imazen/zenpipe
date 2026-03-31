# ⚙️ Quality Intent Node

> **ID:** `zencodecs.quality_intent` · **Role:** encode · **Group:** encode
> **Tags:** `quality`, `auto`, `format`, `encode`

Format selection and quality profile for encoding (zennode node).  This node controls output format selection and quality. It supports both RIAPI querystring keys and JSON API fields, matching imageflow's established `EncoderPreset::Auto` / `EncoderPreset::Format` ergonomics.  **RIAPI**: `?qp=high&accept.webp=true&accept.avif=true` **JSON**: `{ "profile": "high", "allow_webp": true, "allow_avif": true }`  When `format` is empty (default), the pipeline auto-selects the best format from the allowed set. When `format` is set (e.g., "jpeg"), that format is used directly.  The `profile` field accepts both named presets and numeric values: - Named: lowest, low, medium_low, medium, good, high, highest, lossless - Numeric: 0-100 (mapped to codec-specific quality scales)  Convert to [`CodecIntent`] via [`to_codec_intent()`](QualityIntentNode::to_codec_intent).

## Parameters

### Allowed Formats

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `allow_avif` | bool | False | Allow AVIF output. Must be explicitly enabled. |
| `allow_color_profiles` | bool | False | Allow non-sRGB color profiles in the output. |
| `allow_jxl` | bool | False | Allow JPEG XL output. Must be explicitly enabled. |
| `allow_webp` | bool | False | Allow WebP output. Must be explicitly enabled. |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dpr` | float (0.5 – 10.0) | 1.0 | Device pixel ratio for quality adjustment.  Higher DPR screens tolerate lower quality (smaller pixels). Default 1.0 = no adjustment. (×) |
| `format` | string | — | Explicit output format. Empty = auto-select from allowed formats.  Values: "jpeg", "png", "webp", "gif", "avif", "jxl", "keep", or "". "keep" preserves the source format. "thumbnail" is an alias for the key. |
| `lossless` | string | — | Global lossless preference. Empty = default (lossy).  Accepts "true", "false", or "keep" (match source losslessness). |
| `profile` | string | high | Quality profile: named preset or numeric 0-100.  Named presets: "lowest", "low", "medium_low", "medium", "good", "high", "highest", "lossless". Numeric: "0" to "100" (codec-specific mapping). |
| `quality_fallback` | float (0.0 – 100.0) | 0.0 | Legacy quality fallback (0-100). Used when `qp` is not set.  RIAPI: `?quality=85` — sets a numeric quality as fallback for all codecs. Prefer `qp` (quality profile) for new code. *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `accept.avif` | — | `allow_avif` |
| `accept.color_profiles` | — | `allow_color_profiles` |
| `accept.jxl` | — | `allow_jxl` |
| `accept.webp` | — | `allow_webp` |
| `qp.dpr` | `qp.dppx`, `dpr`, `dppx` | `dpr` |
| `format` | `thumbnail` | `format` |
| `lossless` | — | `lossless` |
| `qp` | — | `profile` |
| `quality` | — | `quality_fallback` |

**Example:** `?accept.avif=value&accept.color_profiles=value&accept.jxl=value`
