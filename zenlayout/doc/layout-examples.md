# Layout Pipeline Examples

Visual walkthroughs of common layout operations. Each diagram shows the
pipeline steps computed by `Pipeline::plan()` and `IdealLayout::finalize()`.

Blue = image content. Faded blue = discarded image area (crop).
White = padding. Dashed = edge-replicated extension.

---

## Fit (downscale to target)

The most common operation. Scales the image to fit within the target
dimensions, preserving aspect ratio.

```rust
Pipeline::new(4000, 3000).fit(800, 600)
```

<img src="svg/fit.svg" alt="Fit: 4000x3000 to 800x600" width="400"/>

---

## FitCrop (fill target, crop overflow)

Scales to fill the target, then center-crops the excess. No padding,
no letterboxing.

```rust
Pipeline::new(1920, 1080).fit_crop(500, 500)
```

<img src="svg/fit_crop.svg" alt="FitCrop: 1920x1080 to 500x500" width="400"/>

---

## FitPad (fit within target, pad remainder)

Scales to fit, then pads with the canvas color to reach the exact
target size. Useful for fixed-size slots (thumbnails, social cards).

```rust
Pipeline::new(1600, 900).fit_pad(400, 400)
```

<img src="svg/fit_pad.svg" alt="FitPad: 1600x900 to 400x400" width="400"/>

---

## Crop + Fit

Explicit source crop followed by a resize. The crop selects a
sub-region before any scaling.

```rust
Pipeline::new(1000, 800)
    .crop_pixels(100, 50, 600, 500)
    .fit(300, 250)
```

<img src="svg/crop_resize.svg" alt="Crop then resize" width="400"/>

---

## WithinCrop (downscale only, crop to target ratio)

Like FitCrop but never upscales. If the source is already smaller than
the target, crops to the target aspect ratio without scaling.

```rust
Pipeline::new(800, 600).within_crop(400, 400)
```

<img src="svg/within_crop.svg" alt="WithinCrop: 800x600 to 400x400" width="400"/>

---

## Orientation + Resize

EXIF orientation is applied before computing the resize. The Source
panel shows the raw (pre-rotation) dimensions; the Orient step shows
the logical dimensions after rotation.

```rust
Pipeline::new(4000, 3000)     // raw EXIF: landscape
    .auto_orient(6)            // Rotate90 → portrait
    .fit(600, 800)
```

<img src="svg/orient_resize.svg" alt="Orient and resize" width="400"/>

---

## Rotate 90 + Resize

Manual rotation composes the same way as EXIF orientation. The
pipeline swaps dimensions at the Orient step.

```rust
Pipeline::new(1920, 1080)
    .rotate_90()
    .fit(540, 960)
```

<img src="svg/rotate_90.svg" alt="Rotate 90 and resize" width="400"/>

---

## Rotate 180

Rotation by 180 degrees does not swap dimensions. The Orient step
shows the same size with the transformation noted.

```rust
Pipeline::new(800, 600)
    .rotate_180()
    .fit(400, 300)
```

<img src="svg/rotate_180.svg" alt="Rotate 180 and resize" width="400"/>

---

## Flip Horizontal

Flips compose with other orientation commands into a single transform.

```rust
Pipeline::new(800, 600)
    .flip_h()
    .fit(400, 300)
```

<img src="svg/flip_h.svg" alt="Flip horizontal and resize" width="400"/>

---

## Full Pipeline (orient + crop + pad)

Combining orientation, cropping, and padding. The pipeline applies
each transformation in sequence: orient the raw image, crop in the
oriented coordinate space, resize, then place on a padded canvas.

```rust
Pipeline::new(4000, 3000)
    .auto_orient(6)
    .crop_pixels(200, 200, 2600, 2600)
    .fit_pad(800, 800)
```

<img src="svg/orient_crop_pad.svg" alt="Full pipeline" width="400"/>

---

## Region Viewport (mixed crop + pad)

A region viewport can crop one edge and pad another in a single
operation. Here the viewport extends 50px left of the source (padding)
while cropping the right side at x=600.

```rust
Pipeline::new(800, 600)
    .region_viewport(-50, 0, 600, 600, CanvasColor::black())
```

<img src="svg/region_viewport.svg" alt="Region viewport: mixed crop and pad" width="400"/>

---

## Region Pad + Resize

Uniform padding via region, then scaled down by a constraint.
The padding scales proportionally with the content.

```rust
Pipeline::new(800, 600)
    .region_pad(50, CanvasColor::white())
    .fit(450, 350)
```

<img src="svg/region_pad.svg" alt="Region pad with resize" width="400"/>

---

## Region Percentage Crop

Crop using percentage coordinates. 10% from each edge selects the
center 80% of the image, then resized to the target.

```rust
Pipeline::new(1000, 500)
    .region(Region {
        left: RegionCoord::pct(0.1),
        top: RegionCoord::pct(0.1),
        right: RegionCoord::pct(0.9),
        bottom: RegionCoord::pct(0.9),
        color: CanvasColor::Transparent,
    })
    .fit(400, 200)
```

<img src="svg/region_pct_crop.svg" alt="Region percentage crop" width="400"/>

---

## MCU Edge Extension

For JPEG and other block-based codecs, dimensions must be multiples of
the MCU size (typically 8 or 16). `Align::Extend` rounds up and
replicates edge pixels into the extension area. The codec encodes the
extended image; the decoder can crop back to `content_size`.

```rust
Pipeline::new(801, 601)
    .output_limits(OutputLimits {
        max: None,
        min: None,
        align: Some(Align::uniform_extend(16)),
    })
```

<img src="svg/mcu_extend.svg" alt="MCU edge extension" width="400"/>

---

## Generating these diagrams

```rust
use zenlayout::{Pipeline, DecoderOffer, svg::render_layout_svg};

let (ideal, req) = Pipeline::new(4000, 3000)
    .fit(800, 600)
    .plan()
    .unwrap();

let offer = DecoderOffer::full_decode(4000, 3000);
let plan = ideal.finalize(&req, &offer);

let svg = render_layout_svg(&ideal, &plan);
std::fs::write("layout.svg", &svg).unwrap();
```

The SVGs adapt to light and dark mode via `prefers-color-scheme`.
Padding areas remain white in both themes.
