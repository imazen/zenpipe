# ⚙️ Encode Tiff

> **ID:** `zentiff.encode` · **Role:** encode · **Group:** encode
> **Tags:** `codec`, `tiff`, `lossless`, `encode`

TIFF encoding with compression and predictor options.

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `big_tiff` | bool | False | Big Tiff |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `compression` | string | lzw | Compression |
| `predictor` | string | horizontal | Predictor |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `tiff.big_tiff` | — | `big_tiff` |
| `tiff.compression` | — | `compression` |
| `tiff.predictor` | — | `predictor` |

**Example:** `?tiff.big_tiff=value&tiff.compression=lzw&tiff.predictor=horizontal`
