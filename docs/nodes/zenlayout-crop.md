# ⚙️ Crop

> **ID:** `zenlayout.crop` · **Role:** orient · **Group:** geometry
> **Tags:** `crop`, `geometry`

Crop the image to a pixel rectangle.  Specifies origin (x, y) and dimensions (w, h) in post-orientation source coordinates. For percentage-based cropping, use [`CropPercent`] or [`CropMargins`] instead.  RIAPI: `?crop=10,10,90,90` JSON: `{ "x": 10, "y": 10, "w": 80, "h": 80 }`

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `h` | int (0 – 4294967295) | 0 | Height of the crop region in pixels. (px) |
| `w` | int (0 – 4294967295) | 0 | Width of the crop region in pixels. (px) |
| `x` | int (0 – 4294967295) | 0 | Left edge X coordinate in pixels. (px) |
| `y` | int (0 – 4294967295) | 0 | Top edge Y coordinate in pixels. (px) |
