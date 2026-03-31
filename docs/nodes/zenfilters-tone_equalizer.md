# ⚙️ Tone Equalizer

> **ID:** `zenfilters.tone_equalizer` · **Role:** filter · **Group:** tone_range
> **Tags:** `tone`, `zone`, `equalizer`, `local`

Zone-based luminance adjustment with edge-aware masking.  Divides the luminance range into 9 zones (one per photographic stop from -8 EV to 0 EV) and applies independent exposure compensation to each. A guided filter creates an edge-preserving mask so adjustments don't cause halos at high-contrast boundaries.  Equivalent to darktable's Tone Equalizer module.

## Parameters

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `edge_preservation` | float (0.0010000000474974513 – 0.10000000149011612) | 0.009999999776482582 | Guided filter eps (smaller = sharper edges in mask) |
| `smoothing` | float (0.0 – 100.0) | 0.0 | Guided filter sigma (0 = auto-size from image) (px) |

### Zones

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `zones` | array | [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] | Exposure compensation per zone in stops (9 zones, dark to bright) (EV) |
