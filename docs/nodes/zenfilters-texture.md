# ⚙️ Texture

> **ID:** `zenfilters.texture` · **Role:** filter · **Group:** detail

Fine detail contrast enhancement (smaller scale than clarity).  Similar to Clarity but targets higher-frequency detail like skin pores, fabric weave, and individual leaves. Mirrors Lightroom's Texture slider.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `amount` | float (-1.0 – 1.0) | 0.0 | Enhancement amount (positive = sharpen, negative = soften) |
| `sigma` | float (0.5 – 8.0) | 1.5 | Fine-scale blur sigma (coarse blur is 2x this) (px) |
