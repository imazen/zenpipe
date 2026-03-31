+++
title = "Node Reference"
description = "All pipeline nodes organized by stage — decode through encode"
sort_by = "weight"
template = "section.html"
+++

Auto-generated from [zennode](https://github.com/imazen/zennode) schemas. Each node documents its parameters, defaults, accepted values, and RIAPI querystring keys.

## Pipeline Stages

```
Decode → Orient & Crop → Resize & Layout → Filters → Composite → Quantize → Encode
```

### 1. Decode (5 nodes)

| Node | Description |
|------|-------------|
| [Decode JPEG](zenjpeg-decode) | JPEG decoder with strictness levels and auto-orientation |
| [Decode WebP](zenwebp-decode) | WebP decoder with upsampling control |
| [Decode JXL](zenjxl-decode) | JPEG XL decoder with orientation and intensity target |
| [Decode HEIC](heic-decode) | HEIC/HEIF decoder with gain map and depth extraction |
| [Frame Select](zenpipe-riapi-frame) | RIAPI frame/page selection for multi-frame formats |

### 2. Orient & Crop (14 nodes)

| Node | Description |
|------|-------------|
| [Orient](zenlayout-orient) | Apply EXIF orientation transform |
| [Auto-Rotate](zenpipe-riapi-autorotate) | RIAPI auto-rotation from EXIF |
| [Crop](zenlayout-crop) | Crop to pixel rectangle |
| [Crop Percent](zenlayout-crop_percent) | Crop by percentage coordinates |
| [Crop Margins](zenlayout-crop_margins) | Crop by margin offsets |
| [Crop (RIAPI)](zenpipe-riapi-crop) | RIAPI crop querystring |
| [Region](zenlayout-region) | Extract a viewport region |
| [Flip H](zenlayout-flip_h) | Horizontal flip |
| [Flip V](zenlayout-flip_v) | Vertical flip |
| [Flip (RIAPI)](zenpipe-riapi-flip) | RIAPI flip querystring |
| [Rotate 90](zenlayout-rotate_90) | 90-degree rotation |
| [Rotate 180](zenlayout-rotate_180) | 180-degree rotation |
| [Rotate 270](zenlayout-rotate_270) | 270-degree rotation |
| [Rotate (RIAPI)](zenpipe-riapi-rotate) | RIAPI rotation querystring |

### 3. Resize & Layout (6 nodes)

| Node | Description |
|------|-------------|
| [Constrain](zenresize-constrain) | Unified resize/layout with constraint modes (fit, crop, pad, etc.) |
| [Resize](zenresize-resize) | Forced resize to exact dimensions |
| [Expand Canvas](zenlayout-expand_canvas) | Add padding around the image |
| [Output Limits](zenlayout-output_limits) | Restrict maximum output dimensions |
| [Crop Whitespace](zenpipe-crop_whitespace) | Detect and trim uniform borders |
| [Smart Crop](zenpipe-smart_crop_analyze) | Content-aware crop using focus rectangles or face detection |

### 4. Filters (53 nodes)

#### Tone & Exposure

| Node | Description |
|------|-------------|
| [Exposure](zenfilters-exposure) | Exposure compensation in stops |
| [Contrast](zenfilters-contrast) | Contrast adjustment |
| [Fused Adjust](zenfilters-fused_adjust) | Combined exposure, contrast, highlights, shadows, temperature, tint, saturation |
| [Parametric Curve](zenfilters-parametric_curve) | Parametric tone curve with control points |
| [Tone Curve](zenfilters-tone_curve) | Custom tone curve via control points |
| [Sigmoid](zenfilters-sigmoid) | Sigmoid tone mapping |

#### Tone Mapping

| Node | Description |
|------|-------------|
| [Basecurve Tone Map](zenfilters-basecurve_tonemap) | Camera-style base curve tone mapping with presets |
| [DtSigmoid](zenfilters-dt_sigmoid) | darktable-style sigmoid tone mapper |
| [Levels](zenfilters-levels) | Input/output levels with gamma |

#### Tone Range

| Node | Description |
|------|-------------|
| [Highlights / Shadows](zenfilters-highlights_shadows) | Independent highlights and shadows recovery |
| [Black Point](zenfilters-black_point) | Set black point level |
| [White Point](zenfilters-white_point) | Set white point level |
| [Whites / Blacks](zenfilters-whites_blacks) | Whites and blacks adjustment |
| [Shadow Lift](zenfilters-shadow_lift) | Lift shadow detail |
| [Highlight Recovery](zenfilters-highlight_recovery) | Recover clipped highlights |
| [Tone Equalizer](zenfilters-tone_equalizer) | Multi-band tone adjustment |
| [Local Tone Map](zenfilters-local_tone_map) | Local contrast and tone mapping |

#### Color

| Node | Description |
|------|-------------|
| [Saturation](zenfilters-saturation) | Saturation multiplier |
| [Vibrance](zenfilters-vibrance) | Intelligent saturation that protects skin tones |
| [Temperature](zenfilters-temperature) | White balance temperature |
| [Tint](zenfilters-tint) | Green-magenta tint |
| [Hue Rotate](zenfilters-hue_rotate) | Rotate hue by degrees |
| [HSL Adjust](zenfilters-hsl_adjust) | Per-hue HSL adjustments |
| [Color Grading](zenfilters-color_grading) | Split-tone color grading (shadows/midtones/highlights) |
| [Color Matrix](zenfilters-color_matrix) | 5x5 color transformation matrix |
| [Channel Curves](zenfilters-channel_curves) | Per-channel RGB curves |
| [Grayscale](zenfilters-grayscale) | Convert to grayscale with algorithm selection |
| [BW Mixer](zenfilters-bw_mixer) | Black-and-white channel mixer |
| [Sepia](zenfilters-sepia) | Sepia tone filter |
| [Gamut Expand](zenfilters-gamut_expand) | Expand compressed gamut |
| [Camera Calibration](zenfilters-camera_calibration) | Camera color calibration profiles |
| [Remove Alpha](zenpipe-remove_alpha) | Remove alpha channel |

#### Detail

| Node | Description |
|------|-------------|
| [Sharpen](zenfilters-sharpen) | Unsharp mask sharpening |
| [Adaptive Sharpen](zenfilters-adaptive_sharpen) | Edge-aware adaptive sharpening |
| [Clarity](zenfilters-clarity) | Local contrast / clarity |
| [Blur](zenfilters-blur) | Gaussian blur in Oklab |
| [Median Blur](zenfilters-median_blur) | Median filter for noise removal |
| [Bilateral](zenfilters-bilateral) | Edge-preserving bilateral filter |
| [Noise Reduction](zenfilters-noise_reduction) | Noise reduction |
| [Brilliance](zenfilters-brilliance) | Apple-style brilliance adjustment |
| [Edge Detect](zenfilters-edge_detect) | Edge detection (Sobel, Laplacian, Canny) |
| [Texture](zenfilters-texture) | Texture enhancement |

#### Effects

| Node | Description |
|------|-------------|
| [Alpha](zenfilters-alpha) | Alpha channel multiplier |
| [Bloom](zenfilters-bloom) | Light bloom / glow |
| [Chromatic Aberration](zenfilters-chromatic_aberration) | Simulated chromatic aberration |
| [Grain](zenfilters-grain) | Film grain simulation |
| [Vignette](zenfilters-vignette) | Darken edges |
| [Devignette](zenfilters-devignette) | Remove lens vignetting |
| [Dehaze](zenfilters-dehaze) | Remove haze / fog |
| [Invert](zenfilters-invert) | Invert colors |

#### Canvas

| Node | Description |
|------|-------------|
| [Fill Rect](zenpipe-fill_rect) | Fill a rectangle with a solid color |
| [Round Corners](zenpipe-round_corners) | Round image corners |

#### Auto

| Node | Description |
|------|-------------|
| [Auto Exposure](zenfilters-auto_exposure) | Automatic exposure correction |
| [Auto Levels](zenfilters-auto_levels) | Automatic levels correction |

### 5. Composite (2 nodes)

| Node | Description |
|------|-------------|
| [Composite](zenpipe-composite) | Porter-Duff compositing with blend modes |
| [Overlay](zenpipe-overlay) | Overlay image with blend modes |

### 6. Quantize (1 node)

| Node | Description |
|------|-------------|
| [Quantize](zenquant-quantize) | Palette quantization for GIF/PNG |

### 7. Encode (11 nodes)

| Node | Description |
|------|-------------|
| [Quality Intent](zencodecs-quality_intent) | Format selection and quality profile mapping |
| [Encode JPEG](zenjpeg-encode) | JPEG encoding with jpegli |
| [Encode MozJPEG](zenjpeg-encode_mozjpeg) | JPEG encoding with MozJPEG |
| [Encode PNG](zenpng-encode) | PNG encoding with optional lossy quantization |
| [Encode WebP Lossy](zenwebp-encode_lossy) | WebP lossy encoding |
| [Encode WebP Lossless](zenwebp-encode_lossless) | WebP lossless encoding |
| [Encode AVIF](zenavif-encode) | AVIF encoding |
| [Encode JXL](zenjxl-encode) | JPEG XL encoding |
| [Encode GIF](zengif-encode) | GIF encoding with animation support |
| [Encode TIFF](zentiff-encode) | TIFF encoding |
| [Encode BMP](zenbitmaps-encode_bmp) | BMP encoding |
