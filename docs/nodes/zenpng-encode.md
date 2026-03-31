# ⚙️ Encode Png

> **ID:** `zenpng.encode` · **Role:** encode · **Group:** encode
> **Tags:** `codec`, `png`, `lossless`, `encode`

PNG encoding with quality, lossless mode, and compression options.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `effort` | int (0 – 12) | 5 | Generic effort (0 = no compression, 12 = extreme).  Maps to zenpng's Compression enum: 0 = None, 1 = Fastest, 2 = Turbo, 3 = Fast, 4 = Balanced, 5 = Thorough, 6 = High, 7 = Aggressive, 8 = Intense, 9 = Crush, 10 = Maniac, 11 = Brag, 12+ = Minutes. *(optional)* |
| `lossless` | bool | True | Lossless *(optional)* |
| `min_quality` | int (0 – 100) | 0 | Min Quality *(optional)* |
| `png_quality` | int (0 – 100) | 0 | PNG Quality *(optional)* |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `max_deflate` | bool | False | Max Deflate *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `png.effort` | — | `effort` |
| `png.lossless` | — | `lossless` |
| `png.max_deflate` | — | `max_deflate` |
| `png.min_quality` | — | `min_quality` |
| `png.quality` | — | `png_quality` |

**Example:** `?png.effort=5&png.lossless=True&png.max_deflate=value`
