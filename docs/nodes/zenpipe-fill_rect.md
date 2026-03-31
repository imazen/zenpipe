# ⚙️ Fill Rect

> **ID:** `zenpipe.fill_rect` · **Role:** filter · **Group:** canvas
> **Tags:** `fill`, `rect`, `draw`, `canvas`

Fill a rectangle with a solid color.  Materializes the upstream image, draws the rectangle, then re-streams.  JSON: `{ "x1": 10, "y1": 10, "x2": 100, "y2": 100, "color": [255, 0, 0, 255] }`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `color_a` | int (0 – 255) | 255 | Fill color alpha channel. |
| `color_b` | int (0 – 255) | 0 | Fill color blue channel. |
| `color_g` | int (0 – 255) | 0 | Fill color green channel. |
| `color_r` | int (0 – 255) | 0 | Fill color red channel. |
| `x1` | int (0 – 65535) | 0 | X1 |
| `x2` | int (0 – 65535) | 0 | X2 |
| `y1` | int (0 – 65535) | 0 | Y1 |
| `y2` | int (0 – 65535) | 0 | Y2 |
