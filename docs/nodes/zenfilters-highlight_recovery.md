# ⚙️ Highlight Recovery

> **ID:** `zenfilters.highlight_recovery` · **Role:** filter · **Group:** tone_range

Automatic soft-clip recovery for blown highlights.  Analyzes the L histogram to detect blown highlight content, then applies a proportional soft knee compression. Images with properly exposed highlights are barely affected.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `strength` | float (0.0 – 1.0) | 0.0 | Recovery strength (0 = off, 1 = full) |
