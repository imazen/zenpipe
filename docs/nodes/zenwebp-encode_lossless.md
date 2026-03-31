# ⚙️ Encode Webp Lossless

> **ID:** `zenwebp.encode_lossless` · **Role:** encode · **Group:** encode
> **Tags:** `webp`, `lossless`, `encode`

WebP lossless (VP8L) encode node.

## Parameters

### Alpha

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `alpha_quality` | int (0 – 100) | 0 | Alpha Quality *(optional)* |

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `effort` | int (0 – 10) | 5 | Generic effort (0 = fastest, 10 = best compression).  Maps to WebP method 0-6 internally. *(optional)* |
| `method` | int (0 – 6) | 0 | VP8L compression method (0 = fastest, 6 = best). Separate from effort — method controls the algorithm tier, effort controls quality within that tier. *(optional)* |
| `near_lossless` | int (0 – 100) | 0 | Near-Lossless *(optional)* |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `exact` | bool | False | Exact *(optional)* |

### Target

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `target_size` | int (0 – 4294967295) | 0 | Target Size *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `webp.alpha_quality` | `webp.aq` | `alpha_quality` |
| `webp.effort` | — | `effort` |
| `webp.exact` | — | `exact` |
| `webp.method` | — | `method` |
| `webp.near_lossless` | `webp.nl` | `near_lossless` |
| `webp.target_size` | — | `target_size` |

**Example:** `?webp.alpha_quality=400&webp.effort=5&webp.exact=value`
