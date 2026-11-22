# JPEG XL Photo Studio

A browser photo editor with a canvas, movable and reorderable layers, live
adjustments, undo and redo, editable SVG source, and non-destructive JPEG XL
projects. Source images stay in the browser. A dedicated Web Worker runs the
Rust JPEG XL encoder and decoder.

## Run

Install the Rust `wasm32-unknown-unknown` target, wasm-bindgen CLI 0.2.129,
and `just`. From this directory, run `just build` and `just serve`. The static
`dist/` directory can also be served by another HTTP server with
`application/wasm` enabled. The build pins the public-domain
[Grand Prismatic Spring](https://npgallery.nps.gov/AssetDetail/fbf8deda-a0c6-422e-b8ff-4275dd14f289)
sample by NPS / David Restivo by SHA-256.

Open a JPEG XL, JPEG, PNG, WebP, AVIF or SVG. Add layers from the inspector,
move them on the canvas, and set their opacity, position, scale, rotation
and adjustments. Exposure, contrast, and saturation use the same ZenFilters
Oklab pipeline as Zenpipe's `zeneditor`. The worker filters a separate raster
and the project stores the source and parameter values. Select an SVG to edit
its markup. Keyboard controls
include Ctrl/⌘+O, Ctrl/⌘+S, Ctrl/⌘+Z, Ctrl/⌘+Shift+Z, 0 to fit,
and arrow keys to nudge a layer when the canvas has focus. Shift nudges
by 10 pixels. Space shows the original while held.

**Save project** creates a `.project.jxl` with a standard JPEG XL
presentation and a private editing payload. Reopen it through **Open image**.
**Export JXL** writes a standalone presentation. **Download source** retrieves
the selected layer's imported file byte for byte, including the first SVG
source and the full original JPEG XL codestream and metadata.

## File format

The version 1 profile uses four skippable application boxes in an ISO/IEC
18181-2 container. Image assets and vector renditions are lossless.

| Box | Contents |
| --- | --- |
| `zned` | UTF-8 JSON with version, canvas dimensions, original layer ID, and layers ordered bottom to top. Each layer has a UUID, name, media type, pixel dimensions, visibility, opacity, transform, and adjustment values. An optional `filterEngine` records the pixel interpretation. |
| `znas` | 36 ASCII bytes of layer UUID followed by the exact imported raster or SVG file bytes. |
| `znvs` | The same UUID prefix followed by the current UTF-8 SVG source when the user has edited it. |
| `znrv` | The same UUID prefix followed by the SVG's lossless JPEG XL raster rendition. |

SVG renders run in an isolated browser image context. Reopening uses the
stored rendition. Editing renders a new image and preserves the imported SVG
bytes. The UI exposes source as text.

A project can use cumulative native JPEG XL frames for up to eight visible
layers and 16 million frame pixels. Previous frames have zero duration and
store reference canvases. The encoder can write changed regions using native
cropped replacement. The final frame is the single visible presentation.
The worker compares the decoded native result against the flattened canvas
byte for byte. The ordinary still path covers all other cases. Exact original
layer samples and editable settings remain in the payload either way.

A single unchanged JPEG XL layer reuses its original codestream for both save
and export. This retains source precision, orientation, ICC, EXIF, XMP,
animation and any existing native frames. The original bytes also remain
available in the project payload. An edited presentation uses lossless 8-bit
sRGB RGBA from Canvas 2D with orientation applied. Source-specific metadata
such as gain maps, ICC profiles and JPEG reconstruction data stay with the
source bytes because they describe the original image. Unrecognized boxes
in the primary JXL are also retained in the outer envelope. Third-party JXL
frames open as a coalesced raster layer. Source-local editable frame pixels
require decoder support for uncoalesced output.

## Codec pins and validation

`filters-wasm/Cargo.lock` uses Zenpipe's ZenPixels Convert and ZenPixels
0.3.1 revision `18afc75f1fd40c7feafeef6deae4df8e97e5e1b9`. The codec uses
Zencodec's plane-capable 0.3.1 revision. Separate Wasm modules preserve both
dependency sets. New projects use `zenfilters-v1`. Earlier projects keep their
original Canvas 2D filter semantics.

`Cargo.lock` records all Git revisions. The worker's pixel views use
ZenPixels 0.4 PR 165 at `2099470337b8f2089a49f208d6b6bbacd2f8e485`.
Zencodec PR 214 at `b0ae9ae2211726c14b7d8fe696fb3c374acc8f83`
and the encoder's ZenPixels Convert use the compatible ZenPixels 0.3.1
cohort at `fde59b96632557592cffa9b7301acd8db96ed078`. The two pixel
crate versions meet at the editor's byte-row boundary. The native
Zencodec `DecodeOutput` size assertion remains in force. On x86-64,
`PixelBuffer` measures 72 bytes in the compatible cohort and 64 bytes in 0.4. PR 214 needs plane
types gated behind `planar` in 0.4. The direct 0.4-plus-planar pairing
compiled for wasm32, where the native size assertion is inactive.

Run `just test` and `just wasm-audit`. With Chromium installed via
`npm ci` and `npx playwright install chromium --only-shell`, start
`just serve` and run `just browser`. The browser test forces the feature
probe false in its own fixture server to exercise the compatibility worker.

For the legacy decode check set `DJXL` to djxl v0.12.0 at `a7a9c78`
and `JXL_RS` to the upstream jxl-rs CLI at
`f370be423933628eb7f783f1b61d4c765eca5f79`. Run
`just oracle test-results` after the browser test. It checks the native
layer presentation, ordinary still, and odd strided RGBA exactly against
the source pixels captured in the browser test. The oracle script uses Pillow.
