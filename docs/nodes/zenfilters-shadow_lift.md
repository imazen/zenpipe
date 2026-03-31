# ⚙️ Shadow Lift

> **ID:** `zenfilters.shadow_lift` · **Role:** filter · **Group:** tone_range

Automatic toe-curve recovery for crushed shadows.  Analyzes the L histogram to detect crushed shadow content, then applies a proportional toe lift curve. Images with properly exposed shadows are barely affected.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `strength` | float (0.0 – 1.0) | 0.0 | Lift strength (0 = off, 1 = full) |
