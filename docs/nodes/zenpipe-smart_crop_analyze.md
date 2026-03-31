# ⚙️ Smart Crop Analyze

> **ID:** `zenpipe.smart_crop_analyze` · **Role:** analysis · **Group:** analysis
> **Tags:** `crop`, `smart`, `focus`, `analysis`

Content-aware smart crop using focus rectangles.  Materializes the upstream image, computes the optimal crop rectangle based on focus regions and target aspect ratio, then crops. Uses `zenlayout::smart_crop::compute_crop` for the crop computation.  Not directly addressable from RIAPI — created programmatically by `expand_zen()` when `c.focus` specifies rectangle coordinates.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `rects_csv` | string | — | Focus rectangles as a comma-separated list of percentage coordinates.  Groups of 4 values: x1,y1,x2,y2 (0-100 range). Multiple rects are concatenated: "20,30,80,90,10,10,40,40" = two rects. |
| `target_h` | int (0 – 65535) | 0 | Target height for aspect ratio computation. (px) |
| `target_w` | int (0 – 65535) | 0 | Target width for aspect ratio computation. (px) |
| `zoom` | bool | False | Whether to use maximal (tight/zoom) crop mode instead of minimal. |
