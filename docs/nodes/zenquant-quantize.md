# ⚙️ Quantize

> **ID:** `zenquant.quantize` · **Role:** quantize · **Group:** quantize
> **Tags:** `quantize`, `palette`, `indexed`

Palette quantization with perceptual masking.

## Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `dither_strength` | float (0.0 – 1.0) | 0.5 | Dithering |
| `max_colors` | int (2 – 256) | 256 | Max Colors (colors) |
| `quality` | string | best | Quality |

## RIAPI Querystring Keys

| Key | Aliases | Parameter |
|-----|---------|-----------|
| `quant.dither_strength` | `dither_strength` | `dither_strength` |
| `quant.max_colors` | `max_colors` | `max_colors` |
| `quant.quality` | — | `quality` |

**Example:** `?quant.dither_strength=0.5&quant.max_colors=256&quant.quality=best`
