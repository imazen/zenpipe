+++
title = "Constrain"
description = "zenresize.constrain — resize node"
weight = 30

[taxonomies]
tags = ["resize", "geometry", "scale"]

[extra]
node_id = "zenresize.constrain"
role = "resize"
group = "geometry"
+++

Constrain image dimensions with resize, crop, or pad modes.  The unified resize/layout node — combines layout geometry with resampling hints in a single node, matching imageflow's ergonomic Constrain API.  Optional parameters (`Option<T>`) use `None` for "not specified" — the engine picks sensible defaults based on the operation.  # Gravity  Two ways to specify gravity: - **Named anchor** (`gravity`): "center", "top_left", "bottom_right", etc. - **Percentage** (`gravity_x`/`gravity_y`): 0.0–1.0, overrides named anchor when set.  If `gravity_x` and `gravity_y` are both set, the named `gravity` is ignored.  # JSON API  Simple (named anchor): ```json { "w": 800, "h": 600, "mode": "fit_crop", "gravity": "top_left" } ```  Precise (percentage gravity): ```json { "w": 800, "h": 600, "mode": "fit_crop", "gravity_x": 0.33, "gravity_y": 0.0, "down_filter": "lanczos", "unsharp_percent": 15.0 } ```  RIAPI: `?w=800&h=600&mode=fit_crop&anchor=top_left&down.filter=lanczos`

## Parameters

### Position

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `canvas_color` | string | — | Fill color for exterior padding regions added around the image.  Used by pad modes (fit_pad, within_pad, pad_within) to fill the canvas area outside the image content. Does NOT affect pixels inside the image — use `matte_color` for alpha compositing.  None = transparent. Accepts "transparent", "white", "black", or hex "#RRGGBB" / "#RRGGBBAA". *(optional)* |
| `gravity` | string | center | Named anchor for crop/pad positioning.  Controls which part of the image is preserved when cropping, or where the image is positioned when padding.  Values: "center", "top_left", "top", "top_right", "left", "right", "bottom_left", "bottom", "bottom_right".  Overridden by `gravity_x`/`gravity_y` when both are set. |
| `gravity_x` | float (0.0 – 1.0) | 0.5 | Horizontal gravity (0.0 = left, 0.5 = center, 1.0 = right).  When both `gravity_x` and `gravity_y` are set, they override the named `gravity` anchor. Use for precise positioning beyond the 9 cardinal points (e.g., rule-of-thirds at 0.33). *(optional)* |
| `gravity_y` | float (0.0 – 1.0) | 0.5 | Vertical gravity (0.0 = top, 0.5 = center, 1.0 = bottom).  When both `gravity_x` and `gravity_y` are set, they override the named `gravity` anchor. *(optional)* |

### Quality

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `down_filter` | string | — | Downscale resampling filter. None = auto (Robidoux).  31 filters available: "robidoux", "robidoux_sharp", "robidoux_fast", "lanczos", "lanczos_sharp", "lanczos2", "lanczos2_sharp", "ginseng", "ginseng_sharp", "mitchell", "catmull_rom", "cubic", "cubic_sharp", "cubic_b_spline", "hermite", "triangle", "linear", "box", "fastest", "jinc", "n_cubic", "n_cubic_sharp", etc. *(optional)* |
| `scaling_colorspace` | string | — | Color space for resampling math. None = auto ("linear" for most operations).  - "linear" — resize in linear light (gamma-correct, default) - "srgb" — resize in sRGB gamma space (faster, less correct)  RIAPI: `?down.colorspace=linear` or `?up.colorspace=srgb` *(optional)* |
| `up_filter` | string | — | Upscale resampling filter. None = auto (Ginseng).  Same filter names as down_filter. *(optional)* |

### Dimensions

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `h` | int (0 – 65535) | 0 | Target height in pixels. None = unconstrained (derive from width + aspect ratio).  RIAPI: `?h=600` or `?height=600` or `?maxheight=600` (legacy, implies mode=within). (px) *(optional)* |
| `w` | int (0 – 65535) | 0 | Target width in pixels. None = unconstrained (derive from height + aspect ratio).  RIAPI: `?w=800` or `?width=800` or `?maxwidth=800` (legacy, implies mode=within). (px) *(optional)* |

### Kernel Shape

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `kernel_lobe_ratio` | float (0.0 – 1.0) | 0.0 | Negative-lobe ratio for kernel sharpening (zero-cost).  Adjusts the resampling kernel's negative lobes during weight computation. 0.0 = flatten (maximum smoothness), above filter's natural ratio = sharpen. Zero additional cost — changes the filter shape, not a separate processing step.  For post-resize unsharp mask (separate pass), use `unsharp_percent`. *(optional)* |
| `kernel_width_scale` | float (0.10000000149011612 – 4.0) | 1.0 | Kernel width scale factor (zero-cost).  Multiplies the resampling kernel window width. >1.0 = softer (wider window, less aliasing), <1.0 = sharper (narrower, aliasing risk). Combined with blur. Zero additional cost. *(optional)* |

### Alpha

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `matte_color` | string | — | Background color for alpha compositing (matte behind transparent pixels).  Applied during resize to prevent halo artifacts at transparent edges. Separate from `canvas_color` which fills exterior padding regions. When set, transparent pixels are composited against this color before the resampling kernel samples them.  None = no matte (preserve transparency). "white" is common for JPEG output. *(optional)* |

### Layout

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `mode` | string | within | Constraint mode controlling how the image fits the target dimensions.  - "distort" / "stretch" — stretch to exact dimensions, ignoring aspect ratio - "within" / "max" — fit inside target, never upscale (default) - "fit" — fit inside target, may upscale - "within_crop" / "crop" — fill target by cropping, never upscale - "fit_crop" — fill target by cropping, may upscale - "fit_pad" / "pad" — fit inside target, pad to exact dimensions - "within_pad" — fit inside target without upscale, pad to exact dimensions - "pad_within" — never upscale, always pad to exact canvas - "aspect_crop" / "aspectcrop" — crop to target aspect ratio without resizing - "larger_than" — upscale if needed to meet target, never downscale |
| `scale` | string | — | Scale control: when to allow scaling.  - "down" / "downscaleonly" — only downscale, never upscale - "up" / "upscaleonly" — only upscale, never downscale - "both" — allow both (default) - "canvas" / "upscalecanvas" — upscale canvas (pad) only  RIAPI: `?scale=down` *(optional)* |
| `zoom` | float (0.10000000149011612 – 10.0) | 1.0 | Device pixel ratio / zoom multiplier.  Multiplies target dimensions. 2.0 means w and h are doubled. RIAPI: `?zoom=2`, `?dpr=2x`, `?dppx=1.5` *(optional)* |

### Post-Processing

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `post_blur` | float (0.0 – 100.0) | 0.0 | Post-resize Gaussian blur sigma (real cost).  Applied as a separable H+V pass after resize. NOT equivalent to `kernel_width_scale` (which changes the kernel itself at zero cost). *(optional)* |
| `unsharp_percent` | float (0.0 – 100.0) | 0.0 | Post-resize unsharp mask strength (0 = none, 100 = maximum).  Applied as a separate pass AFTER resampling. Adds real computational cost proportional to output dimensions. For zero-cost sharpening that adjusts the resampling kernel itself, use `kernel_lobe_ratio`.  None = no unsharp mask. (%) *(optional)* |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `resample_when` | string | size_differs | When to apply resampling.  - "size_differs" — only resample when dimensions change (default) - "size_differs_or_sharpening_requested" — resample when dimensions change or when sharpening is requested (allows sharpening without resize) - "always" — always resample, even at identity dimensions *(optional)* |
| `sharpen_when` | string | downscaling | When to apply sharpening.  - "downscaling" — sharpen only when downscaling (default) - "upscaling" — sharpen only when upscaling - "size_differs" — sharpen whenever dimensions change - "always" — always sharpen, even at identity dimensions *(optional)* |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `bgcolor` | `canvas_color` | `canvas_color` |
| `down.filter` | — | `down_filter` |
| `anchor` | — | `gravity` |
| `h` | `height`, `maxheight` | `h` |
| `lobe_ratio` | `kernel_lobe_ratio` | `kernel_lobe_ratio` |
| `matte` | `matte_color`, `s.matte` | `matte_color` |
| `mode` | — | `mode` |
| `resample_when` | — | `resample_when` |
| `scale` | — | `scale` |
| `down.colorspace` | `up.colorspace` | `scaling_colorspace` |
| `sharpen_when` | — | `sharpen_when` |
| `f.sharpen` | `unsharp` | `unsharp_percent` |
| `up.filter` | — | `up_filter` |
| `w` | `width`, `maxwidth` | `w` |
| `zoom` | `dpr`, `dppx` | `zoom` |

**Example:** `?bgcolor=value&down.filter=value&anchor=center`

