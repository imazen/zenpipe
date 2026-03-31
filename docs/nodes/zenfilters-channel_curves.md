# ⚙️ Channel Curves

> **ID:** `zenfilters.channel_curves` · **Role:** filter · **Group:** color
> **Tags:** `color`, `curves`, `channel`, `rgb`

Per-channel tone curves applied independently to R, G, B in sRGB space.  Unlike ToneCurve which operates on Oklab L (preserving color ratios), ChannelCurves enables independent tonal correction of each color channel. Each channel has its own 256-entry LUT mapping sRGB [0,1] to [0,1].  The node accepts control points as comma-separated "x:y" pairs per channel. Default is identity: "0:0,1:1".

## Parameters

### Blue

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `blue_points` | string | 0:0,1:1 | Blue channel control points as "x:y" pairs, comma-separated |

### Green

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `green_points` | string | 0:0,1:1 | Green channel control points as "x:y" pairs, comma-separated |

### Red

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `red_points` | string | 0:0,1:1 | Red channel control points as "x:y" pairs, comma-separated |
