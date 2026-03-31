# ⚙️ Black Point

> **ID:** `zenfilters.black_point` · **Role:** filter · **Group:** tone_range

Black point adjustment on Oklab L channel.  Remaps the shadow floor. A black point of 0.05 means values that were L=0.05 become L=0.0, and the range is stretched accordingly.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `level` | float (0.0 – 0.5) | 0.0 | Black point level (0 = no change, 0.1 = crush bottom 10%) |
