# Native JPEG XL access

Open a JPEG XL file and select **Native JXL** in the inspector. A JPEG XL
layer in an existing project can also be selected as the source.

- **Source frame** lists the regular frames the decoder exposes, including
  zero-duration update frames. Select a frame to inspect its encoded extent,
  signed origin, per-channel blend, reference slots, clamp, duration and
  timecode. The frame preview uses the local extent before compositing.
- **Preview channel** shows color, an individual color component, or an extra
  channel. Raw views show the decoded sample range. **Download F32 samples**
  writes the selected plane in stored orientation as little-endian float32.
  File plane 0 is interleaved color. Other files contain one extra channel
  each, in source order. Frame fields give the extent, channel count, color
  model and profile.
- **Add frame as layer** imports a color preview for a supported Replace
  or Blend frame as an editable source-over layer, using its display-oriented
  position and dimensions. The layer retains the exact source JPEG XL and a
  regular-frame index. Reopening resolves that index through the same decoder
  path. Native blend settings remain available in the inspector.
- **Image header fields** includes stored orientation, precision, profile,
  tone mapping, animation clock, preview dimensions, and extra-channel
  parameters. **Download embedded ICC** retrieves the profile bytes when the
  source has an embedded profile.
- **Container boxes** lists every box in the source envelope, including
  application and unknown types. **Download box** writes its exact header
  and payload. **Reconstruct original JPEG** recovers the JPEG on sources
  with reconstruction data. Explicit source artifacts have their own file
  extensions. **Save project** downloads `.project.jxl` and **Export JXL**
  downloads `.jxl`.

## Scope

The native module uses the decoder's qualified local F32 path from
[zenjxl-decoder PR 75](https://github.com/imazen/zenjxl-decoder/pull/75),
pinned to `907b91b9015b739e8ce34c17a45538e5a3158ad6`. It loads when native
tools are requested. It uses a separate dependency graph because that
revision requires Archmage 0.9.30 while the editor codec uses 0.9.33.

| JPEG XL area | Current editor interaction |
| --- | --- |
| Regular frame geometry and color | Inspect, view local pixels, extract F32, add qualified frames to the editing stack |
| Add, multiply and alpha-weighted-add data | Inspect blend and reference fields, view raw components, extract F32 |
| Extra channels | Inspect type, precision, association, shift, spot color and CFA fields. View and extract decoder F32 output when supported |
| Color and HDR | Inspect source color and tone mapping, download embedded ICC, render a color-managed sRGB display preview |
| Animation | Inspect frame order, duration, timecodes and the source clock. Original playback data stays in the preserved source |
| Container and recovery | Inspect and export encoded boxes, preserve unknown boxes, reconstruct supported JPEG sources |
| Native output | Save a standard final presentation with zero-duration replacement updates when a compatibility decode matches the editor canvas |

Full syntax-level editing remains open. Reference-only and LF frames stay
inside the decoder. The current raw API excludes progressive flush and random
seeking. Animation timeline authoring, reference-graph and per-channel blend
editing, embedded preview extraction, full-precision edited HDR export,
container authoring controls, and editing of splines, patches, noise and
VarDCT coefficients still need dedicated interfaces. Each source file keeps
those original structures byte for byte.

## Verification

`just browser` checks a native crop with origin (9, 7), size 5 × 4, reference
slot 1 and straight alpha. It compares all 20 raw alpha floats with the fixture,
checks the saved source bytes, and opens the downloaded project again. Both
decoder workers run in Chromium. `just oracle test-results` checks the source
and edited presentation using upstream jxl-rs and pinned djxl v0.12.

`just browser-jpeg <carrier> <icc>` also checks exact JPEG reconstruction and
ICC download. Make the carrier from the pinned `dist/sample.jpg` with
libjxl v0.12's `cjxl --lossless_jpeg=1`, and extract the JPEG's ICC independently
for the expected profile.
