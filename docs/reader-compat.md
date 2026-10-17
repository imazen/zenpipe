# What image readers do with the signals we write

Measured behaviour of third-party decoders, browsers and OS image stacks
on orientation, colour and HDR gain-map signalling, as of the
compat-matrix round-14 run (2026-10-17). This is the record zenpipe and
zencodecs encode decisions are checked against. Every claim below comes
from the run data listed under [Data and provenance](#data-and-provenance);
nothing is quoted from vendor documentation.

The companion reference site (imazen/image-compat, caniuse-style matrix
with per-cell evidence) is built from the same runs. Corpus-photo
results (real camera files) and the corpus census are not in this
document.

## How to read the verdicts

- **Fixtures** are synthetic files authored by Zen encoders plus
  byte-level container writers (120 files: JPEG, PNG, WebP, AVIF,
  JPEG XL, HEIC), each carrying exactly one signal under test, plus
  conflict fixtures carrying two. Every fixture has an expected
  rendering (`correct`) and named wrong renderings (for example
  `stored`, `p3-as-srgb`, `orient1`, `mc-709-decode`).
- **Verdicts** name the hypothesis the reader's output matches. The
  "raw" reading scores the emitted pixels as they are; the "tagged"
  reading first interprets the output file's own colour declaration
  (PNG cICP > iCCP > sRGB > gAMA+cHRM). A reader that passes pixels
  through unconverted but copies the source profile onto its output
  reads `p3-as-srgb` raw and `p3-as-p3` tagged; both are reported.
- **`not-measurable`** means the correct rendering and a wrong one are
  pixel-identical on that fixture (for example "honoured sRGB" versus
  "ignored sRGB"), so the cell cannot attribute behaviour.
- Distances are 8-bit levels, patch-centre mean absolute difference on
  the pattern fixtures and whole-image p99 on the HEIC remux fixtures.
  The decode-noise threshold is 2 levels on synthetic fixtures; the HEIC
  remuxes carry a cross-decoder bound of 24 levels (see Limitations).
- Reader "modes": `-best` is the library's own orientation-plus-colour
  option path (vips `autorotate` + `icc_transform`, Pillow ImageCms, Qt
  `convertToColorSpace`, ImageMagick/GraphicsMagick `-auto-orient` +
  `-profile sRGB`, djxl `--color_space`); `-raw` is the explicit
  no-transform path; the plain name is the library's default call.
  For the OS stacks: `imageio` is a default `CGImageSource` decode,
  `imageio-best` adds the orientation and colour options, `imageio-raw`
  requests no transform; `wic`, `wic-best`, `wic-raw` are the
  equivalents on Windows Imaging Component.

## Reader versions

| Column set | Readers |
|---|---|
| LTS image (Ubuntu 24.04) | libjpeg-turbo 2.1.5 (djpeg), Netpbm 11.5.2 (pngtopam), libwebp 1.3.2 (dwebp), libheif 1.17.6 (heif-dec), libavif 1.0.4 (avifdec), libjxl 0.7.0 (djxl), ImageMagick 6.9.12-98, GraphicsMagick 1.3.42, libvips 8.15.1, FFmpeg 6.1.1, Pillow 10.2.0, OpenCV 4.6.0, GDK-Pixbuf 2.42.10, Qt 6.4.2, LibRaw 0.21.2, libultrahdr 1.4.0 |
| Latest image (Arch) | libjpeg-turbo 3.2.0, Netpbm 10.86.50, libwebp 1.6.0, libheif 1.23.2, libavif 1.4.2, libjxl 0.12.0, ImageMagick 7.1.2-30, GraphicsMagick 1.3.48, libvips 8.18.6, FFmpeg 9.0.1, Pillow 12.3.0, OpenCV 5.0.0, GDK-Pixbuf 2.44.7, Qt 6.11.2, LibRaw 0.22.2, libultrahdr 2.0.2 |
| Browser engines (Playwright 1.63, Linux) | Chromium 153.0.8010.12, Firefox 155.0 (default profile with colour management off, and with `gfx.color_management.mode=2`), WebKit 26.6; each in element-screenshot and canvas-readback modes |
| Shipped browsers (Linux) | Chrome 155.0.8059.39, Firefox 157.0.1 (screenshot mode) |
| macOS 27, Apple silicon | ImageIO (default, best, raw), NSImage (AppKit), Quick Look thumbnails (`qlmanage -t`), sips (plain conversion and `--matchTo sRGB`) |
| Windows 11, x64 | WIC (default, best, raw) with AV1 Video Extension 2.0.35, HEIF Image Extension 1.2.48 and the third-party jxl-winthumb JPEG XL codec; Chrome 155.0.8059.79; Edge 155.0.4283.45 (screenshot mode) |
| Zen | zencodecs managed decode (`zen`) and raw decode (`zen-raw`), probe binary `8377b4fa…` |

Safari on macOS is not in this round (the WebDriver column needs a
session started from the Mac's own login session; see Limitations).

## Orientation

### JPEG EXIF orientation

Applied by: Chromium, Firefox (all profiles), WebKit, Chrome 155,
Firefox 157, Edge 155, ffmpeg, OpenCV default, Pillow, Qt, vips, the
`-best` paths of GraphicsMagick, ImageMagick and vips, ImageMagick
`-auto-orient`, NSImage, Quick Look, `imageio-best`, `wic-best`, zen.

Stored pixels (orientation ignored): djpeg, gdk-pixbuf, GraphicsMagick
and ImageMagick default, OpenCV `IMREAD_UNCHANGED`, vips `-raw`,
ImageIO default and raw, sips, WIC default and raw, zen-raw.

On a conflicting XMP `tiff:Orientation`, every orienting reader renders
the EXIF value.

### PNG `eXIf` orientation

Applied by: Chromium, Firefox (all profiles), Chrome 155, Firefox 157,
Edge 155, OpenCV default, Pillow, vips, GraphicsMagick `-best`,
NSImage, Quick Look, `imageio-best`, zen; in the latest image also
ffmpeg 9.0.1 and ImageMagick 7 `-auto-orient`/`-best`.

Ignored by: **WebKit** (both modes), gdk-pixbuf, GraphicsMagick default,
ImageMagick 6 (all modes), Qt (both modes), ffmpeg 6.1.1, pngtopam,
ImageIO default and raw, sips, **WIC in every mode including `-best`**,
zen-raw.

### WebP EXIF orientation

Applied by: GraphicsMagick `-best`, Pillow, vips (default and `-best`),
NSImage, Quick Look, `imageio-best`, zen; in the latest image also
ImageMagick 7 `-auto-orient`/`-best`, ffmpeg 9.0.1 and OpenCV 5.0.

Ignored by **every browser measured** (Chromium 153, Firefox 155/157,
WebKit 26.6, Chrome 155 on Linux and Windows, Edge 155), dwebp, ffmpeg
6.1.1, ImageMagick 6, GraphicsMagick default, OpenCV 4.6, vips `-raw`,
ImageIO default and raw, sips, WIC (all modes), zen-raw.

### AVIF `irot` / `imir`

Applied by: Chromium, Firefox, Chrome 155, Firefox 157, Edge 155,
gdk-pixbuf, GraphicsMagick, ImageMagick, vips (all modes), NSImage,
Quick Look, `imageio-best`, WIC (all modes), zen; in the latest image
also avifdec 1.4.2, ffmpeg 9.0.1 and Pillow 12.3.

Not applied by: avifdec 1.0.4, ffmpeg 6.1.1, **WebKit 26.6 (Linux
build, both modes)**, ImageIO default and raw, sips, zen-raw.

`imir` axis semantics (ISO/IEC 23008-12): axis 0 mirrors top/bottom
(EXIF 4), axis 1 mirrors left/right (EXIF 2). Every applying reader
agrees except zen's AVIF path, which swaps them (`avif-imir0` renders
as EXIF 2, `avif-imir1` as EXIF 4; zenavif issue 54, fixed on zenavif
main at `505793b`, not yet in the pinned revision).

The Exif item inside an AVIF is informational; `irot`/`imir` are the
display directive. Every reader leaves an Exif-only orientation alone,
except GraphicsMagick `-best`, which applies it, and on a file carrying
both `irot=3` and Exif 6 composes the two (`both`, a double rotation).

### HEIC `irot` / `imir` and the Exif item

`irot`/`imir` honoured by every libheif-based reader (heif-dec, vips,
gdk-pixbuf, GraphicsMagick, ImageMagick), ffmpeg 9.0.1 (its own HEIF
demuxer and HEVC decoder), WIC (all modes), NSImage, Quick Look,
`imageio-best`, zen. Untransformed raster: zen-raw, ImageIO default
and raw, sips.

When the file carries an Exif orientation item and **no** `irot`, the
libheif family, ffmpeg, WIC and zen leave the raster alone (the
expected reading). **Apple's decoders apply the Exif item**: NSImage,
Quick Look and `imageio-best` render it rotated (`exif-applied`).
GraphicsMagick `-best` does the same, and with `irot=3` plus Exif 6 it
composes both (`double-applied`).

### JPEG XL orientation

Codestream orientation (`jxl-orient1..8`): applied by djxl (both
modes), ffmpeg, vips (all modes), GraphicsMagick `-best`, WebKit 26.6,
Chrome 155, NSImage, Quick Look, `imageio-best`, WIC (jxl-winthumb,
all modes), zen; in the latest image also gdk-pixbuf 2.44, OpenCV 5.0
and ImageMagick 7 `-auto-orient`/`-best`. Raw: GraphicsMagick default,
ImageMagick 7 default, ImageIO default and raw, sips, zen-raw.

An Exif box with the codestream orientation at 1: ignored by every
JPEG XL reader, as the codestream wins, except ImageMagick 7
`-auto-orient`/`-best`, which apply the Exif value.

## Colour

### Where colour management happens

A reader "manages" a signal when its emitted pixels are converted to
sRGB. Readers that copy the declaration onto their output without
converting are listed as "tag carried"; a downstream consumer that
honours the tag still ends up right, one that ignores it does not.

| Signal | Converted to sRGB by | Tag carried, pixels untouched | Ignored outright |
|---|---|---|---|
| JPEG ICC (v2 and v4) | Chromium, Firefox cm=2, WebKit, Chrome 155, Firefox 157, Edge 155, all `-best` library paths, gdk-pixbuf 2.44, ImageIO default/best, NSImage, sips `--matchTo`, WIC default/best, zen | ffmpeg, GraphicsMagick, ImageMagick, vips, vips-raw, Pillow, Qt, ImageIO raw, sips | djpeg, Firefox default profile, OpenCV, gdk-pixbuf 2.42, WIC raw, zen-raw |
| PNG `iCCP` | same set as JPEG ICC | same | same, plus pngtopam |
| WebP `iCCP` | Chromium, Firefox cm=2, Chrome 155, Firefox 157, Edge 155, GraphicsMagick/ImageMagick/Pillow/vips `-best`, gdk-pixbuf 2.44, ImageIO default/best, NSImage, sips `--matchTo`, **`wic-best` only**, zen | ffmpeg, GraphicsMagick, ImageMagick, vips, Pillow, ImageIO raw, sips | **WebKit 26.6**, dwebp, OpenCV, WIC default/raw, Firefox default, zen-raw |
| PNG `cICP` | Chromium, Firefox cm=2, Chrome 155, Firefox 157, Edge 155, ImageIO default/best, NSImage, sips `--matchTo`, zen | ffmpeg, ImageIO raw, sips | **WebKit**, **WIC (all modes)**, every `-best` library path (ICC only), gdk-pixbuf 2.44, GraphicsMagick, ImageMagick, Pillow, Qt, vips, OpenCV, pngtopam, zen-raw |
| PNG `gAMA`+`cHRM` (P3 primaries, 2.2 power) | Chromium, Firefox cm=2, Chrome 155, Firefox 157, Edge 155, Qt `-best`, ImageIO default/best, NSImage, sips `--matchTo` | ffmpeg, GraphicsMagick (both modes), Qt, ImageIO raw, sips; ImageMagick copies `cHRM` but not `gAMA` | **WebKit**, **WIC**, vips/Pillow/GraphicsMagick/ImageMagick `-best`, gdk-pixbuf, OpenCV, **zen** (its PNG path treats the pair as untagged), zen-raw |
| AVIF ICC | Chromium, Firefox cm=2, Chrome 155, Firefox 157, Edge 155, GraphicsMagick/ImageMagick/vips `-best`, Pillow 12.3 `-best`, gdk-pixbuf 2.44, ImageIO default/best, NSImage, sips `--matchTo`, zen | avifdec, ffmpeg, GraphicsMagick, ImageMagick, vips, ImageIO raw, sips | **WebKit 26.6 (Linux)**, Firefox default; **WIC** reads `other` (see the matrix section) |
| AVIF `nclx` (P3 primaries) | Chromium, Firefox cm=2, Chrome 155, Firefox 157, Edge 155, ImageIO default/best, NSImage, sips `--matchTo`, **WIC (all modes)**, zen | avifdec, ffmpeg, ImageIO raw, sips | WebKit 26.6, every `-best` library path, gdk-pixbuf 2.44, vips, GraphicsMagick, ImageMagick |
| JPEG XL ICC and enum primaries | WebKit 26.6, Chrome 155, GraphicsMagick `-best`, vips `-best`, ImageMagick 7 `-best`, djxl 0.12 `--color_space` (but see mis-tag below), gdk-pixbuf 2.44 (ICC only), ImageIO default/best, NSImage, sips `--matchTo`, `wic-best`, zen | djxl 0.7 and 0.12 default, ffmpeg, GraphicsMagick, ImageMagick 7, vips, ImageIO raw, sips | OpenCV 5.0, WIC default/raw, zen-raw |
| HEIC `prof` ICC (grid item) | GraphicsMagick/ImageMagick/vips `-best`, gdk-pixbuf 2.44, zen | heif-dec, vips, vips-raw, ImageMagick, GraphicsMagick, ffmpeg 9.0.1 | gdk-pixbuf 2.42, zen-raw; the OS stacks are undecidable here (Limitations) |
| HEIC `nclx` (P3 primaries) | zen | — | every libheif-based reader renders the raster unconverted (`stored`); OS stacks undecidable |
| EXIF `ColorSpace`, `InteropIndex`, APP14 Adobe | nobody | — | everybody, zen included |

Notes on individual readers:

- **Firefox**: the Playwright default profile has colour management
  off, so it renders every signal raw (`gfx.color_management.mode=0`
  read back from the profile). With mode 2 it manages everything above,
  including PNG `cICP` and `gAMA`+`cHRM`. The shipped Firefox 157 on
  Linux manages all of them out of the box.
- **WebKit 26.6 (Linux Playwright build)** manages ICC on JPEG, PNG and
  JPEG XL exactly, and passes WebP ICC, every AVIF colour signal, PNG
  `cICP` and PNG `gAMA`+`cHRM` through raw. These are per-format gaps,
  not an engine-wide one. Whether Safari on macOS behaves the same is
  not measured yet.
- **Chromium 153 / Chrome 155 / Edge 155** manage every signal
  measured, including `gAMA`+`cHRM` with the declared power curve
  (exact match to the 2.2-gamma hypothesis; the sRGB-curve alternative
  is 3 levels away).
- **The library `-best` paths** (vips, Pillow, Qt, ImageMagick,
  GraphicsMagick) are ICC-only: PNG `cICP` and AVIF `nclx` pass raw.
  Qt `-best` is the one library path that honours `gAMA`+`cHRM`.
- **djxl 0.12 with `--color_space`** converts to sRGB and then writes
  the Display P3 profile onto the output (`mis-tagged`): the pixels are
  right, the tag is wrong. djxl 0.7 ignores the option silently.
- **sips `--matchTo sRGB`** converts correctly, but on a `gAMA`+`cHRM`
  source it copies the source chunks onto the converted output, the
  same mis-tag shape. ICC, `cICP` and `nclx` sources get a correct sRGB
  tag.
- **ImageIO raw** and plain **sips** leave pixels alone and write the
  source profile onto the output (raw `p3-as-srgb`, tagged `p3-as-p3`).
- **Quick Look** thumbnails are a rendered pipeline, not a decode: on
  P3 sources the pixels match no sRGB hypothesis (108 levels out) and
  the output is tagged Display P3, under which they read correctly.
- **WIC** converts ICC on JPEG and PNG in default and best mode, WebP
  ICC only in best mode, honours AVIF `nclx`, and ignores PNG `cICP`
  and `gAMA`+`cHRM` in every mode. On the Adobe-RGB JPEG its output
  sits 3 to 5 levels from the correct rendering (`other`).
- **zen** manages ICC, `nclx`, `cICP` and JPEG XL enum primaries, and
  treats PNG `gAMA`+`cHRM`, EXIF colour tags and APP14 as untagged.

### Precedence on conflicting declarations

| Fixture | Spec reading | Who follows it | Who does not |
|---|---|---|---|
| PNG `iCCP` P3 + `cICP` sRGB | `cICP` wins (sRGB) | Chromium, Firefox cm=2, Chrome 155, Firefox 157, Edge 155, ImageIO default/best, NSImage, sips `--matchTo`, zen, and every raw path | WebKit, all `-best` library paths, WIC default/best (`iCCP` wins) |
| PNG `iCCP` sRGB + `cICP` P3 | `cICP` wins (P3) | Chromium, Firefox cm=2, Chrome 155, Firefox 157, Edge 155, ImageIO default/best, NSImage, sips `--matchTo`, zen | WebKit, all `-best` library paths, WIC (sRGB, from `iCCP` or raw) |
| PNG `sRGB` chunk + `iCCP` P3 | `iCCP` wins | every colour-managing reader | — |
| AVIF ICC P3 + `nclx` sRGB | ICC | every colour-managing reader | — |
| JPEG ICC P3 + EXIF `ColorSpace`=sRGB | ICC | every colour-managing reader | — |
| HEIC `nclx` sRGB + `prof` P3 | spec-silent; `prof` treated as correct | GraphicsMagick/ImageMagick/vips `-best`, gdk-pixbuf 2.44, zen (`icc-wins`) | heif-dec, vips, vips-raw, gdk-pixbuf 2.42, ImageMagick, GraphicsMagick, zen-raw (`nclx-wins`, which here equals no conversion) |
| HEIC `nclx` sRGB **limited range** + `prof` P3 | as above | GraphicsMagick/ImageMagick/vips `-best`, gdk-pixbuf 2.44 | libheif family honours the `nclx` including the range flag; **zen applies the ICC but drops the range flag** (`icc-range-ignored`, 0.09 levels from that hypothesis, 9.7 from either correct reading) |

### AVIF matrix coefficients, subsampling, bit depth, range

Five fixtures signal the matrix in both the `nclx` box and the AV1
sequence header: 8-bit 4:4:4 BT.709 matrix, 8-bit 4:2:0 BT.601, 8-bit
4:2:0 BT.709, 10-bit 4:4:4 BT.601, 8-bit 4:4:4 BT.601 limited range.
Two reference hypotheses (`mc-709-decode`, `mc-601-decode`) are built
by dumping the coded planes and converting with the named matrix in
reference code, never through a reader under test.

Every AVIF reader in the LTS and latest images honours the signalled
matrix and the signalled range (105 and 110 decoded cells, all inside
the correct pair). So do the browsers, Chrome and Edge on Windows, and
every macOS path.

**WIC (AV1 Video Extension 2.0.35) selects the matrix from primaries
and bit depth, not from the signal.** On every 8-bit AVIF with BT.709
(sRGB) primaries its output matches the BT.601-coded content converted
with the BT.709 matrix, within 1 level, whatever the `nclx` box or the
sequence header says (`mc-709-decode` on the 4:2:0 BT.601 and on the
limited-range fixture, on `avif-cicp-srgb` and `avif-untagged` in every
mode, and on `avif-icc-srgb` in best and raw mode; WIC default on that
last file sits 3 levels out and reads `other`). On P3-primaries and
10-bit files WIC decodes correctly (the correct rendering is 33 to 39
levels from the BT.709-matrix hypothesis on every one of these files). Writing an `nclx` box makes no difference. The
orientation axis is unaffected: WIC applies `irot`/`imir` correctly on
every fixture while mis-decoding the colour (`orient8-mc-709` and so
on; `orientation_ok=true`).

The ICC-only P3 AVIF (`avif-icc-p3`) reads `other` in WIC default and
best mode, 35 levels from the correct rendering: the ICC is honoured on
top of the wrong matrix. WIC raw reads `mc-709-decode` on it.

Gain-map AVIF rows separate multi-image handling from decoding:
ImageMagick 6 and 7 decode both the primary and the gain-map item
(their default output naming emits two files, so a single-output
adapter sees nothing); GraphicsMagick 1.3.42 refuses multi-image AVIF
at library level ("Number of images is not supported"); Qt in these
images has no AVIF delegate at all.

### HEIC colour rows

The HEIC fixtures are remuxes of one iPhone photo (no `nclx` in the
original; `prof` ICCs on tile items). The injected `nclx` is
`*/13/6` full range, so on the sRGB variants "honoured" and "ignored"
are pixel-identical and every reader reads `not-measurable`. The
limited-range variant makes `nclx` carry a real transform (9.6 levels).

- `heic-colr-p3` (`nclx` P3): **zen is the only reader whose pixels
  are converted** (1 level from correct, 36 from stored). The whole
  libheif family renders the raster unconverted.
- `heic-icc-p3` (grid `prof` P3, no `nclx`): pixels converted only by
  the `-best` paths, gdk-pixbuf 2.44 and zen; the rest of the libheif
  family passes the raster through and copies the profile onto the
  output, which reads `correct` once the tag is honoured.
- `heic-remux-base` (the donor's real per-tile Apple ICCs): the same
  split, except **zen and zen-raw read `stored` both ways**: zen honours
  grid-level `colr` and a single-item `prof` but not per-tile `prof`
  boxes in a 48-tile grid.
- `heic-tile-base` (the full 4032×3024 tile grid, carrying a malformed
  zero-length `colr prof`): vips 8.15 and 8.18 fail with `bad seek`;
  ImageIO reports "no image in source"; Quick Look hangs on it (now
  bounded to 120 s in the macOS driver and recorded as an error);
  heif-dec, gdk-pixbuf, ImageMagick, GraphicsMagick, zen and zen-raw
  decode it.

## HDR and gain maps

Reference: zen's reconstruction (full boost, linear light, stored
orientation). Differences are mean |Δlog2| in stops over the frame.

| Fixture | zen | libultrahdr `ultrahdr_app` | ImageIO / Core Image (macOS 27) | Others |
|---|---|---|---|---|
| UltraHDR JPEG, MPF + `hdrgm` XMP | reconstructs | HDR out, 0.033 from zen | both apply the map: ImageIO 0.166 and Core Image 0.170 from zen (their peak 3.26× against zen's 4.58×) | every other reader renders the SDR base |
| UltraHDR JPEG, ISO 21496-1 APP2 only (no XMP) | **detects but renders flat**: zencodecs reads the XMP `hdrgm` form only | HDR out (1.377 from zen's flat reference) | both apply it: ImageIO 1.41 and Core Image 1.33 from zen's flat reference | SDR base |
| UltraHDR JPEG with a corrupt MPF index | base decodes; HDR probe fails | HDR out on 1.4.0 and 2.0.2 alike | — | — |
| AVIF `tmap` (branded, `altr` group) | reconstructs | — | both apply it: Core Image 0.013 from zen, ImageIO 0.050 | avifdec 1.4.2 lists the gain map and its `tonemap --headroom 2` lands 0.031 from zen; avifdec 1.0.4 silent; WIC's 8-bit output matches no SDR hypothesis (39 levels, the matrix issue above) |
| AVIF `tmap` written by the archived zenavif serializer (no `tmap` brand, no `altr`) | reconstructs | — | not applied (1.65 and 1.68 from zen) | avifdec 1.4.2 reports the gain map absent; `avifgainmaputil` does not recognize it |
| JPEG XL `jhgm` | reconstructs | — | not applied (1.72 and 1.69 from zen) | jxlinfo sees the box; djxl and every other reader decode the base only |

NSImage's 8-bit output on the UltraHDR JPEGs and the branded `tmap`
AVIF matches neither the SDR base nor the 4× boosted hypothesis (113
and 45 levels out): it renders with the map applied through its own
tone pipeline. Quick Look and sips render the SDR base.

Authoring note (measured on the zen side): `zenjpeg::ultrahdr` writes
the `hdrgm` XMP even when asked for `GainMapEncodingFormat::Iso21496`;
the ISO-only fixture is produced by stripping the XMP APP1 after
encoding. zenavif `11033c9` writes the AVIF Exif item as a two-extent
`iloc`; exiftool reads it, exiv2 reports it corrupt, and ffmpeg 6.1.1
and 9.0.1 both reject it, so the fixtures are repacked to one extent
(zenavif `5e493d39` writes a single extent natively).

## Alpha, animation, depth

- Alpha is honoured by every decoding reader on PNG, 16-bit PNG, lossy
  WebP, AVIF and JPEG XL, with two exceptions: OpenCV `IMREAD_COLOR`
  flattens it (`IMREAD_UNCHANGED` keeps it) and ffmpeg flattens
  `avif-alpha`. Browser screenshot modes composite transparency, so
  alpha is only measurable through the canvas readback.
- APNG: every decoding reader renders the first frame; WebKit's
  screenshot path renders something else (239 levels out, a compositing
  artefact; its canvas readback is frame 0); gdk-pixbuf 2.42 and 2.44
  fail on it.
- Palette PNG with `tRNS`: decoded by every library reader and by the
  canvas readbacks; the screenshot paths and OpenCV's flattening decode
  composite it opaque.
- 10-bit AVIF decodes correctly wherever AVIF decodes at all.

## JPEG XL coverage

Decoded by: djxl, ffmpeg (6.1.1 crashes on the two fixtures carrying
extra boxes, Exif and `jhgm`; 9.0.1 decodes them), vips, GraphicsMagick,
**WebKit 26.6 (16 of 16 fixtures, both modes)**, **Chrome 155 on Linux
and Windows (16 of 16)**, every macOS path (ImageIO, NSImage, Quick
Look, sips), WIC through the third-party jxl-winthumb codec, zen; in
the latest image also OpenCV 5.0, gdk-pixbuf 2.44 and ImageMagick 7.

Not decoded by: Chromium 153 (Playwright build), Firefox 155 and 157,
**Edge 155**, OpenCV 4.6, ImageMagick 6, gdk-pixbuf 2.42, Pillow, Qt.

## Latest versus LTS

Same adapters against the newer image: 185 cell differences across 71
fixtures. 147 are `error → ok` (Pillow gains AVIF; OpenCV 5.0 gains
JPEG XL; ImageMagick 7 gains JPEG XL and PNG `eXIf`; ffmpeg 9.0.1 and
the libheif 1.23 family decode the HEIC set; ffmpeg 9.0.1 opens the
repacked Exif AVIFs), 2 are `crash → ok` (ffmpeg on the two JPEG XL
box fixtures), and 34 are verdict moves:

| Move | n | Who |
|---|---|---|
| `p3-as-srgb → p3-as-p3` | 10 | gdk-pixbuf 2.44.7 is colour-managed (2.42 was raw on everything); djxl 0.12 honours `--color_space` |
| `orient1 → orient6/8/3/4/2` | 17 | avifdec 1.4.2 and ffmpeg 9.0.1 apply AVIF `irot`/`imir`; ffmpeg, ImageMagick 7 `-auto-orient`/`-best` and OpenCV 5.0 apply PNG and WebP `eXIf` |
| `none → single-rot` | 2 | avifdec and ffmpeg apply `irot` on the `irot`+Exif fixture |
| `stored → correct` | 2 | gdk-pixbuf colour-manages `heic-icc-p3` and `heic-remux-base` |
| `nclx-wins → icc-wins` | 2 | gdk-pixbuf on both HEIC conflict rows |
| `adobe-as-srgb → adobe-as-adobe` | 1 | gdk-pixbuf on the Adobe-RGB JPEG |

## Implications for zenpipe output

These follow from the tables above; each names the measurement it
rests on.

1. **Bake orientation into the pixels and write orientation 1.** No
   browser applies WebP EXIF orientation; WebKit, WIC (all modes), Qt
   and ImageMagick 6 ignore PNG `eXIf`; the default ImageIO and WIC
   decode calls ignore JPEG EXIF too. zenpipe already re-encodes with
   the orientation applied and EXIF/XMP orientation rewritten to 1.
2. **For wide-gamut output, an ICC profile is the only signal every
   colour-managing reader honours on JPEG, PNG and WebP.** PNG `cICP`
   alone is ignored by WebKit, WIC and every library `-best` path;
   `gAMA`+`cHRM` is ignored by WebKit, WIC and zen itself. Writing
   `cICP` next to an ICC profile is safe: the readers that honour
   `cICP` follow the spec precedence, and the rest fall back to the
   profile. For AVIF, `nclx` is read more widely than ICC (WIC reads
   `nclx` P3 correctly and misreads an ICC-only P3 file); write both,
   which zenavif does from `c6c037c0`.
3. **8-bit AVIF with BT.709 primaries is decoded with the BT.709 matrix
   by WIC regardless of signalling.** No container or stream signal
   changes that. Encoding such content with the BT.709 matrix for real,
   and signalling it, would make WIC and every signal-honouring reader
   agree; that is a design inference, not a measurement (no fixture
   carries genuinely BT.709-coded content yet, because the pinned
   zenavif always converts with BT.601).
4. **UltraHDR JPEG needs the `hdrgm` XMP form for zen's own reader.**
   zencodecs detects the ISO 21496-1 APP2 form but reconstructs flat;
   libultrahdr and Core Image apply it. Until that is fixed, write the
   XMP (or both).
5. **AVIF gain maps need the `tmap` brand and the `altr` group.** The
   archived serializer's layout is reconstructed by zen only; avifdec
   1.4.2 and Core Image do not recognize it.
6. **JPEG XL reaches Safari/WebKit 26.6, Chrome 155, macOS, and
   Windows only through a third-party WIC codec.** Firefox 157 and
   Edge 155 do not decode it.
7. **zen's own gaps measured here**: AVIF `imir` axes swapped (fixed
   upstream, pin pending); HEIC `nclx` range flag dropped when an ICC
   is also present; per-tile `prof` ICCs in HEIC grids not applied;
   PNG `gAMA`+`cHRM` not applied; ISO-only UltraHDR flat.

## Limitations

- **HEIC colour on the OS decoders is undecidable in this round.** The
  hypotheses for the HEIC remuxes are rendered through libde265, and
  ImageIO, NSImage, sips and WIC decode the same HEVC 21 to 29 levels
  away from that base, which is also the size of the P3-versus-sRGB
  separation. Their HEIC colour cells read `other` or
  `not-measurable` and attribute nothing. The fix is a per-decoder
  unmanaged reference (decoding the same remuxes with each OS decoder
  in no-transform mode), done for the in-image decoders and not yet for
  the OS ones.
- Safari is not measured: `safaridriver` started from an SSH session
  on macOS 27 fails to launch its HTTP service; the column needs a run
  from the Mac's own login session.
- Firefox's colour-managed behaviour is emulated with
  `gfx.color_management.mode=2`; real desktop display profiles are
  outside the harness. The shipped Firefox 157 run confirms the
  managed readings on Linux.
- Screenshot-mode cells cannot measure alpha. Canvas readbacks can,
  but WebKit's canvas readback is nondeterministic run to run
  (occasional all-zero buffers, reported as `flaky` and excluded from
  error counts).
- No CMYK, arithmetic-coded or 12-bit JPEG fixtures (Zen encoders
  cannot author them; third-party authoring is out of scope).
- JPEG XL ICC profiles live inside the codestream, so the byte-level
  ICC identity gate that checks every other fixture does not cover
  them.
- The matrix runs on Linux x86-64 containers, one Mac mini and one
  Windows workstation; GPU-path and HDR-display behaviour is not
  measured.

## Data and provenance

Tool: `compat-matrix` (`tools/compat-matrix` in the imazen-26
repository; Rust, generates the fixtures, drives the adapters, scores
the cells and verifies the run with `matrix --check`). Round-14 build:
commit `8649df2a15fa`, binary sha256 `8377b4fa18c7acc8…`, rustc 1.99.0.
Every cell JSON records the producer image id (or `host`), the adapter
script hash and the fixture bytes hash; `--check` refuses cells whose
identities are not in the run's `platform.txt`.

Run directories (dev workstation, `/mnt/v/output/imazen-26-compat/`,
mirrored to `tower:/mnt/user/coefficient/output/imazen-26-compat-archive-2026-10-07/r14/`):

| Run | Contents |
|---|---|
| `2026-10-07-r14/` | LTS image, 120 fixtures × 35 adapters (4,200 cells), fixtures archive, `hdr.tsv`, metadata-reader table |
| `2026-10-07-r14-latest/` | the same against the latest image |
| `2026-10-07-shipped-r10/` | Chrome 155 and Firefox 157 on Linux (scored with the round-14 tool) |
| `2026-10-07-platforms-r10/` | macOS 27 (7 adapters) and Windows 11 (5 adapters), 1,440 cells (scored with the round-14 tool) |

Each run holds `matrix.tsv` (one row per cell: status, raw and tagged
verdicts, distances, output tag, orientation grade), `versions.tsv`,
`summary.tsv`, `calibration.tsv`, `platform.txt` and the per-cell JSON
and PNG outputs. The TSVs are generated data and stay on block storage;
this document is the versioned record.

The site build (`imazen/image-compat`, `site/build.py`) reads the same
four runs.
