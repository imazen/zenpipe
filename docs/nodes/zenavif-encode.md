# ⚙️ Avif Encode

> **ID:** `zenavif.encode` · **Role:** encode · **Group:** encode
> **Tags:** `avif`, `encode`, `av1`

AVIF encoding node.

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `alpha_color_mode` | string | clean | Alpha Color Mode *(optional)* |
| `color_model` | string | ycbcr | Color Model *(optional)* |
| `lossless` | bool | False | Lossless *(optional)* |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `alpha_quality` | float (0.0 – 100.0) | 0.0 | Alpha Quality *(optional)* |
| `bit_depth` | string | auto | Bit Depth *(optional)* |
| `effort` | int (0 – 10) | 6 | Generic effort (0 = fastest, 10 = best compression).  Inverted from rav1e speed: effort 0 = speed 10, effort 10 = speed 1. Prefer this over `speed` for codec-agnostic pipelines. *(optional)* |
| `quality` | float (1.0 – 100.0) | 75.0 | Quality *(optional)* |
| `speed` | int (1 – 10) | 4 | rav1e-native speed (1 = slowest, 10 = fastest). Inverse of effort. *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `avif.alpha_color_mode` | `avif.alpha_mode` | `alpha_color_mode` |
| `avif.alpha_quality` | `avif.aq` | `alpha_quality` |
| `avif.depth` | — | `bit_depth` |
| `avif.color_model` | — | `color_model` |
| `avif.effort` | — | `effort` |
| `avif.lossless` | — | `lossless` |
| `avif.q` | `avif.quality` | `quality` |
| `avif.speed` | — | `speed` |

**Example:** `?avif.alpha_color_mode=clean&avif.alpha_quality=value&avif.depth=auto`
