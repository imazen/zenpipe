# ⚙️ Fused Adjust

> **ID:** `zenfilters.fused_adjust` · **Role:** filter · **Group:** tone
> **Tags:** `fused`, `adjust`, `exposure`, `contrast`, `saturation`

Fused per-pixel adjustment: applies all per-pixel operations in a single pass over the data, avoiding repeated plane traversal.  Equivalent to chaining Exposure + Contrast + BlackPoint + WhitePoint + Saturation + Temperature + Tint + HighlightsShadows + Dehaze + Vibrance, but runs ~3x faster because it only scans the planes once.

## Parameters

### Tone

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `black_point` | float (0.0 – 0.5) | 0.0 | Black point level (0 = no change) |
| `contrast` | float (-1.0 – 1.0) | 0.0 | Contrast (-1 to 1) |
| `dehaze` | float (0.0 – 1.0) | 0.0 | Dehaze strength |
| `exposure` | float (-5.0 – 5.0) | 0.0 | Exposure in stops (EV) |
| `highlights` | float (-1.0 – 1.0) | 0.0 | Highlights recovery |
| `shadows` | float (-1.0 – 1.0) | 0.0 | Shadows recovery |
| `white_point` | float (0.5 – 2.0) | 1.0 | White point level (1 = no change) |

### Color

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `saturation` | float (0.0 – 2.0) | 1.0 | Linear saturation factor (×) |
| `temperature` | float (-1.0 – 1.0) | 0.0 | Temperature shift (negative = cool, positive = warm) |
| `tint` | float (-1.0 – 1.0) | 0.0 | Tint shift (negative = green, positive = magenta) |
| `vibrance` | float (-1.0 – 1.0) | 0.0 | Vibrance (smart saturation) |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `vibrance_protection` | float (0.5 – 4.0) | 2.0 | Vibrance protection exponent |
