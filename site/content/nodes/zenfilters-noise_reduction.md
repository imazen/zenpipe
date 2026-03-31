+++
title = "Noise Reduction"
description = "zenfilters.noise_reduction — filter node"
weight = 400

[taxonomies]
tags = []

[extra]
node_id = "zenfilters.noise_reduction"
role = "filter"
group = "detail"
stage = "Filters"
+++

Wavelet-based luminance and chroma noise reduction.  Uses an a trous wavelet decomposition with soft thresholding. Chroma denoising uses a higher effective threshold since chroma noise is typically more objectionable.

## Parameters

### Main

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `chroma` | float (0.0 – 1.0) | 0.0 | Chroma noise reduction strength |
| `luminance` | float (0.0 – 1.0) | 0.0 | Luminance noise reduction strength |

### Advanced

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `chroma_detail` | float (0.0 – 1.0) | 0.5 | Chroma detail preservation (higher = keep more color detail) |
| `detail` | float (0.0 – 1.0) | 0.5 | Luminance detail preservation (higher = keep more detail) |
| `luminance_contrast` | float (0.0 – 1.0) | 0.5 | Luminance contrast preservation in denoised areas |
| `scales` | int (1 – 6) | 4 | Number of wavelet scales |

