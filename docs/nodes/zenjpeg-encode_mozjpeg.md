# ⚙️ Encode Mozjpeg

> **ID:** `zenjpeg.encode_mozjpeg` · **Role:** encode · **Group:** encode
> **Tags:** `jpeg`, `jpg`, `encode`, `lossy`, `mozjpeg`, `compat`

Mozjpeg-compatible JPEG encoder configuration.

## Parameters

### Quality

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `effort` | int (0 – 2) | 1 | Effort *(optional)* |
| `quality` | float (1.0 – 100.0) | 85.0 | Quality *(optional)* |

### Color

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `subsampling` | string | quarter | Chroma Subsampling *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `mozjpeg.effort` | — | `effort` |
| `mozjpeg.quality` | `mozjpeg.q` | `quality` |
| `mozjpeg.subsampling` | `mozjpeg.ss` | `subsampling` |

**Example:** `?mozjpeg.effort=1&mozjpeg.quality=85.0&mozjpeg.subsampling=quarter`
