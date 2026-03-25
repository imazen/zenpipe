# Codec Wisdom Tables

Comprehensive reference for transcoding decisions, format selection, metadata
preservation, and codec capabilities across all zen codecs.

All data verified against source code as of 2026-03-25.

---

## Format Properties

| | JPEG | PNG | WebP | GIF | AVIF | JXL | HEIC | TIFF | PNM | BMP | Farbfeld |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Lossy | Y | palette | Y | -- | Y | Y | Y | -- | -- | -- | -- |
| Lossless | -- | Y | Y | Y | feat | Y | -- | Y | Y | Y | Y |
| Alpha | -- | Y | Y | Y | Y | Y | Y | Y | Y | Y | Y |
| Animation | -- | APNG | Y | Y | dec | Y | -- | -- | -- | -- | -- |
| Progressive | Y | Adam7 dec | -- | -- | -- | -- | -- | -- | -- | -- | -- |
| HDR (>8bit) | -- | 16 | -- | -- | 10/12 | f32 | 10 | f32 | f32 | -- | 16 |
| Max depth | 8 | 16 | 8 | 8 | 12 | 32 | 10 | 32 | 32 | 8 | 16 |
| Gray | Y | Y | -- | enc | -- | Y | -- | Y | Y | dec | -- |

"feat" = behind optional feature flag. "dec" = decode only. "enc" = encode only.
"palette" = lossy via palette quantization, not pixel-level lossy compression.

---

## Metadata Read/Write

R = read (decode/probe), W = write (encode), -- = not supported by format.

| | JPEG | PNG | WebP | GIF | AVIF | JXL | HEIC | TIFF | Bitmaps |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| ICC | R/W | R/W | R/W | -- | R/W | R/W | R | R | -- |
| EXIF | R/W | R/W | R/W | -- | R/W | R/W | R | R | -- |
| XMP | R/W | R/W | R/W | -- | R/W | R/W | R | R | -- |
| CICP | -- | R/W | -- | -- | R/W | R/W | R | -- | -- |
| Orientation | EXIF | -- | EXIF | -- | irot/imir | header | irot/imir | EXIF | -- |
| Resolution | JFIF | pHYs | -- | -- | -- | -- | -- | tags | -- |
| cLLi | -- | R/W | -- | -- | R/W | -- | R | -- | -- |
| mDCv | -- | R/W | -- | -- | R/W | -- | R | -- | -- |

---

## Supplements & Sidecars

| | JPEG | PNG | WebP | GIF | AVIF | JXL | HEIC | TIFF | Bitmaps |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| Gain map | UltraHDR | -- | -- | -- | ISO 21496 | jhgm | Apple | -- | -- |
| Depth map | -- | -- | -- | -- | -- | extra ch | Apple | -- | -- |
| Multi-page | -- | -- | -- | -- | grid | -- | grid | IFDs | -- |

---

## Source Quality Detection

Can the codec estimate the source encoding quality from headers alone?

| Codec | Detectable? | Method | Confidence | Scale |
|---|---|---|---|---|
| JPEG | Yes | DQT table matching (IJG/mozjpeg/jpegli families) | Exact for known encoders, +/-5 for unknown | 0-100 |
| PNG | N/A | Always lossless | -- | -- |
| WebP | Yes | VP8 base quantizer index reversal | Reliable for libwebp | 0-100 |
| GIF | N/A | Always lossless | -- | -- |
| AVIF | Partial | AV1 base_q_idx from OBU headers | Approximate when QP found | 0-100 |
| JXL | No | Headers don't expose butteraugli distance | -- | -- |
| HEIC | No | Can't recover HEVC QP from container | -- | -- |
| TIFF | N/A | Always lossless | -- | -- |
| Bitmaps | N/A | Always lossless | -- | -- |

---

## Lossless Detection

Can the codec determine if the source was losslessly encoded?

| Codec | Detectable? | Method |
|---|---|---|
| JPEG | No | JPEG is always lossy |
| PNG | Yes | Always lossless |
| WebP | Yes | VP8L bitstream type vs VP8 |
| GIF | Yes | Always lossless (palette-indexed LZW) |
| AVIF | No | QP=0 necessary but not sufficient; AV1 lossless flag not in lightweight probe |
| JXL | Yes | `!xyb_encoded` — modular pathway = lossless, VarDCT = lossy |
| HEIC | No | HEVC is always lossy |
| TIFF | Yes | Always lossless (standard TIFF; JPEG-in-TIFF detectable via compression tag) |
| Bitmaps | Yes | Always lossless |

---

## Quality Calibration

Generic quality 0-100 maps to codec-native scales via corpus-calibrated
interpolation tables (CID22-512, 209 images, SSIMULACRA2-matched).

| Generic | JPEG | WebP | AVIF | JXL distance | JXL effort | PNG quant |
|---|---|---|---|---|---|---|
| 15 (lowest) | 15 | 15 | 10 | 15.0 | 1 | 0-30 |
| 20 (low) | 20 | 20 | 12 | 12.0 | 2 | 5-40 |
| 34 (med-low) | 34 | 34 | 22 | 7.0 | 3 | 15-60 |
| 55 (medium) | 57 | 53 | 45 | 4.0 | 4 | 30-80 |
| 73 (good) | 73 | 76 | 55 | 2.58 | 5 | 50-100 |
| 91 (high) | 91 | 93 | 78 | 1.0 | 7 | 80-100 |
| 96 (highest) | 96 | 96 | 90 | 0.3 | 8 | 90-100 |
| 100 (lossless) | 100 | 100 | 100 | 0.0 | 9 | 100-100 |

AVIF speed follows inverse pattern: generic 15 → speed 10 (fastest),
generic 96 → speed 3 (slowest).

---

## Effort / Threading

| Codec | Effort range | Meaning | Thread range | Notes |
|---|---|---|---|---|
| JPEG | 0-2 | 3-level quality/speed | (1,1) | Parallel behind feature flag |
| PNG | 0-12 | Compression level | (1,16) | Real multi-threading |
| WebP | 0-10 | Method 0-6 mapped | (1,1) | Single-threaded |
| GIF | -- | No effort knob | (1,1) | -- |
| AVIF | 0-10 | rav1e speed (inverted) | (1,256) | Real multi-threading |
| JXL | 1-10 | Encoder effort | (1,65535) enc / (1,2) dec | Encode: per-thread. Decode: serial/parallel binary |
| HEIC | -- | Decode only | (1,N) | Parallel tile decode behind feature |
| TIFF | -- | No effort knob | (1,1) | -- |
| Bitmaps | -- | No effort knob | (1,1) | -- |

---

## Color Space & Transfer Function

| Codec | Color spaces | Transfer functions | Wide gamut | Bit depth preservation |
|---|---|---|---|---|
| JPEG | sRGB (via ICC) | sRGB | ICC only | 8 in, 8 out |
| PNG | sRGB (cICP for any) | sRGB, PQ, HLG | cICP or ICC | 8↔8, 16↔16 |
| WebP | sRGB (via ICC) | sRGB | ICC only | 8↔8 |
| GIF | sRGB | sRGB | No | 8↔8 |
| AVIF | sRGB, P3, BT.2020 | sRGB, PQ, HLG | Native (CICP) | 8↔8, 16↔16 |
| JXL | Any (structured encoding) | sRGB, PQ, HLG, linear, any | Native | 8↔8, 16↔16, f32↔f32 |
| HEIC | sRGB, BT.2020 (CICP) | sRGB, PQ, HLG | CICP | 10→16 |
| TIFF | sRGB (via ICC) | sRGB, linear | ICC only | 8↔8, 16↔16, f32↔f32 |
| PNM | sRGB assumed | sRGB, linear (PFM) | No | 8↔8, 16↔16, f32↔f32 |
| BMP | sRGB | sRGB | No | 8↔8 |
| Farbfeld | sRGB | sRGB | No | 16↔16 |

---

## Format Selection Preference Order

From `select.rs`, corpus-calibrated:

| Condition | Preference order | Rationale |
|---|---|---|
| Lossless | JXL → WebP → PNG → AVIF | JXL best compression ratio |
| Animation (lossy) | AVIF → WebP → GIF | AVIF best quality/size |
| Animation (lossless) | WebP → GIF | WebP supports lossless animation |
| Alpha (lossy) | JXL → AVIF → WebP → PNG | JPEG can't do alpha |
| Lossy opaque (<3MP) | JXL → AVIF → JPEG → WebP → PNG | AVIF shines on small images |
| Lossy opaque (≥3MP) | JXL → JPEG → AVIF → WebP → PNG | AVIF slower for large images |

---

## Transcoding Decision Matrix

When transcoding from source → target, what should you do?

### Lossy → Lossy

| Rule | Rationale |
|---|---|
| Target quality ≥ source quality estimate | Never re-encode at lower quality than source |
| If source quality unknown, use generic 73 (Good) | Conservative default |
| If `BoolKeep::Keep` and source is lossy, stay lossy | Don't inflate file size with lossless re-encode |
| Preserve ICC profile | Color accuracy |
| Preserve EXIF orientation (apply or embed) | Display correctness |
| Strip EXIF/XMP if policy says so | Privacy/security |

### Lossless → Lossy

| Rule | Rationale |
|---|---|
| Use requested quality (no source quality to match) | Caller controls quality |
| Consider source bit depth for quality floor | 16-bit source → higher quality target |
| Preserve ICC | Color accuracy |
| Gamut map if source is wide gamut and target is sRGB-only | Prevent clipping |

### Lossy → Lossless

| Rule | Rationale |
|---|---|
| Generally wasteful — file will be larger with no quality gain | Lossless can't recover lost data |
| Only useful for archival or when further processing needed | Avoids generation loss |
| Exception: JPEG → lossless JPEG XL (can reconstruct original JPEG bytes) | True lossless JPEG recompression |

### Lossless → Lossless

| Rule | Rationale |
|---|---|
| Preserve bit depth | No unnecessary quantization |
| Preserve color space | No unnecessary gamut mapping |
| Prefer smaller format (JXL > WebP > PNG for lossless) | Pure compression improvement |
| Preserve alpha if present | Don't discard data |
| Preserve animation if present and target supports it | Don't lose frames |

---

## Metadata Preservation Rules

| Field | Default | When to strip | When to preserve |
|---|---|---|---|
| ICC profile | Preserve | `EncodePolicy::strip_all()`, target is sRGB-only and source is sRGB | Always when source has non-sRGB gamut |
| EXIF | Preserve | Privacy-sensitive contexts, `strip_all()` | When orientation matters, when creation date matters |
| XMP | Preserve | Privacy-sensitive, `strip_all()` | When licensing/copyright metadata matters |
| CICP | Preserve | Target doesn't support CICP | HDR content, wide gamut content |
| Orientation | Apply+strip or preserve | After applying rotation to pixels | When lossless transform possible (JPEG) |
| cLLi/mDCv | Preserve | SDR-only output | HDR content preservation |
| Gain map | Preserve if both formats support it | Target doesn't support gain maps | HDR↔SDR adaptive display |

---

## Bit Depth Transcoding

| Source depth | Target depth | Action |
|---|---|---|
| 8 → 8 | Direct | No conversion needed |
| 16 → 16 | Direct | Preserve precision |
| 16 → 8 | Quantize | Quality loss — raise encode quality to compensate |
| 8 → 16 | Promote | No quality gain but enables downstream processing headroom |
| f32 → 8 | Tonemap/clamp + quantize | Significant loss — warn user or require explicit intent |
| f32 → 16 | Quantize | Precision loss — acceptable for most content |
| f32 → f32 | Direct | Only JXL, TIFF, PNM support f32 output |
| HDR (PQ/HLG) → SDR | Tonemap | Requires gain map or tonemapping operator |
| SDR → HDR | Inverse tonemap or gain map | Requires gain map reconstruction |

---

## DPR Quality Adjustment

Device pixel ratio affects optimal quality. At DPR 1.0, browser upscales
each source pixel to 3×3 screen pixels (relative to baseline 3.0),
magnifying artifacts. Higher quality compensates.

```
factor = 3.0 / dpr.clamp(0.1, 12.0)
adjusted = 100.0 - (100.0 - base_quality) / factor
```

| DPR | Base 73 (Good) → | Base 55 (Medium) → |
|---|---|---|
| 1.0 | 91.0 | 85.0 |
| 2.0 | 82.0 | 70.0 |
| 3.0 | 73.0 (no change) | 55.0 |
| 6.0 | 46.0 | 10.0 |

---

## Image Fact Detection

What can be determined from probe (header-only parse)?

| Fact | Available from | Notes |
|---|---|---|
| Dimensions | All codecs | Always available from header |
| Has alpha | All codecs | From color type / bitstream header |
| Has animation | Most codecs | GIF: block scan. WebP: VP8X. AVIF: container. PNG: acTL. JXL: header. |
| Is lossless source | WebP, JXL, format-based | WebP: bitstream type. JXL: xyb flag. Others: inferred from format. |
| Source quality | JPEG, WebP, AVIF (partial) | DQT tables, VP8 QP, AV1 QP |
| HDR content | AVIF, JXL, HEIC, PNG | CICP transfer function (PQ/HLG detection) |
| Gain map present | AVIF, JXL, HEIC, JPEG (UltraHDR) | Container-level detection |
| Color primaries | AVIF, JXL, HEIC, PNG (CICP) | From CICP/NCLX signaling |
| Bit depth | All codecs | From header / color type |
| Orientation | JPEG, AVIF, JXL, HEIC, TIFF | EXIF tag 274, irot/imir, JXL header |

---

## Gain Map Transcoding

| Source | Target | Possible? | Method |
|---|:---:|---|---|
| JPEG (UltraHDR) | AVIF | Yes | Decode gain map → re-encode as tmap auxiliary |
| JPEG (UltraHDR) | JXL | Yes | Decode gain map → re-encode as jhgm box |
| AVIF | JXL | Yes | Extract tmap → convert metadata → jhgm box |
| AVIF | JPEG (UltraHDR) | Yes | Extract tmap → encode as MPF secondary image |
| JXL | AVIF | Yes | Extract jhgm → convert to tmap |
| JXL | JPEG (UltraHDR) | Yes | Extract jhgm → encode as MPF secondary |
| HEIC (Apple) | Any | Partial | Apple gain map format differs from ISO 21496-1 |
| Any | PNG/WebP/GIF/TIFF/BMP | No | Target format has no gain map support |

All gain map transcoding requires:
1. Decode the gain map image (typically 1/4-1/8 resolution)
2. Apply proportional geometric transforms (same as primary image)
3. Convert ISO 21496-1 metadata between format-specific representations
4. Re-encode the gain map (usually lossless or high-quality lossy)
