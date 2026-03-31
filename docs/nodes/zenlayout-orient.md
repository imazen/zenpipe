# ⚙️ Orient

> **ID:** `zenlayout.orient` · **Role:** orient · **Group:** geometry
> **Tags:** `orient`, `exif`, `geometry`

Apply EXIF orientation correction.  Orientation values 1-8 follow the EXIF standard: 1 = identity, 2 = flip-H, 3 = rotate-180, 4 = flip-V, 5 = transpose, 6 = rotate-90, 7 = transverse, 8 = rotate-270.  RIAPI: `?autorotate=true` (uses embedded EXIF), `?srotate=90` JSON: `{ "orientation": 6 }`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `orientation` | int (1 – 8) | 1 | EXIF orientation value (1-8). 1 = no transformation. |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `srotate` | — | `orientation` |

**Example:** `?srotate=1`
