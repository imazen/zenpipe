# Executable codec operation matrix

The registry in [operation_matrix.rs](../tests/operation_matrix.rs) generates every
ordered source/destination pair for the enabled configurations. Expectations are
separate from advertised capability flags. This is a finite RGB8 test catalog,
not exhaustive testing of all files, pixel types, policies or encoder settings.

## Run and inspect

- `just codec-operation-matrix`: still routes, checked fidelity selection,
  distinct-frame animation, typed refusals and structural prefix boundaries.
- `just codec-operation-matrix-oracles`: also read source encodes and transcode
  outputs with ImageMagick and libjxl. Set `ZEN_CODEC_MATRIX_MAGICK` and
  `ZEN_CODEC_MATRIX_DJXL` explicitly when using other executable names.
- `just codec-operation-matrix-inventory`: the same catalog with inventory,
  HEIC and RAW enabled, exercising typed input limits and TIFF/RAW coexistence.
- `just metadata-codec-ci-check`: the existing codec integration suites.
- `just metadata-pin-check`: reject disagreement across standalone consumer
  manifests and lockfiles, including shared metadata type identity.

`ZEN_CODEC_MATRIX_REPORT` saves the still/fidelity table as TSV. Each row names
operation, source, target, fixture, law, outcome and detail. `FAIL` and `BLOCKED`
fail the test; `API_GAP` and `UNEXECUTED` are never passes. The route-count assertion
keeps failed sources in the denominator. Oracle programs are caller-selected:
missing requested executables fail, while unrequested readers produce explicit
`UNEXECUTED` source rows and are not used on transcode outputs.

## Enumerated scope

| Axis | Cases |
|---|---|
| Configurations | JPEG; PNG; lossless WebP; GIF/zenquant; lossless JXL; AVIF/zenravif, SVT 4:2:0, AOM 4:2:0; PNM; farbfeld; BMP; QOI; TGA; Radiance HDR; TIFF |
| Source pixels | RGB8: 1x1 packed, 7x5 with 12 padding bytes per row, 17x9 with 6 padding bytes per row |
| Still routes | 15 x 15 x 3 = 675, including self-transcodes |
| Target queries | Lossless, native quality 80, approximate SSIMULACRA2 80, Butteraugli 1, Zensim B 80, plus ordered selection |
| Selected targets | Actually encode padded RGB8; compare exact samples when lossless is confirmed |
| Native backend routes | Explicit decode/convert/encode for SVT and AOM destinations; convenience dispatch for other destinations |
| Inventory | Callable disabled-feature refusal; enabled inspection and typed input-cap rejection |
| Animation | WebP millisecond exactness; APNG rational exactness; distinct accepted frames around typed refusal; GIF advertised rational-timing refusal with legacy encoding intact |
| Prefix boundaries | PNM missing its final sample byte; complete payload; complete payload followed by 1024 trailing bytes |

Lossless source encodes and destination routes compare exact RGB8 samples.
Lossy routes require successful decoding and correct dimensions. Independent
readers also compare exact samples for lossless outputs; this is not a lossy
quality benchmark. Radiance HDR appears as a lossy RGB8 route, not a claim that
RGBE preserves arbitrary floating-point samples. The existing metadata suite
retains its own per-field and ordered-pair expectations; it is not replaced by
the RGB8 table.

## Findings and contract boundaries

The matrix exposed dropped JXL lossless requests, missing QOI/TGA/HDR runtime
allowlist entries, TGA/ICO sniffing ambiguity, and RAW detection claiming ordinary
TIFF. The integration forwards JXL lossless mode, includes compiled bitmap
formats, and shares the core RAW detector's distinguishing signatures. Core ICO
sniffing now requires a nonzero directory count. Generic TIFF remains TIFF;
explicit RAW selection remains available.

Typed animation checks exposed WebP and PNG native wrappers that concealed the
shared unsupported cause after erasure. The codec fixes retain that cause without
changing error messages or relaxing existing rejection expectations. AVIF tests
also cover quantization state after refusal and clearing native lossless mode
when switching fidelity targets.

PNM currently returns unknown transfer for its decoded buffer while its output
preflight advertises sRGB. The prefix oracle checks the current unknown-transfer
sample representation; it does not treat absence of color interpretation as a
color-delivery guarantee. This discrepancy needs a codec color-policy decision
and consistent probe/decode signaling; guessing a transfer in this harness would
hide the question.

## Remaining axes and next contract

Not covered by the all-pairs table: HEIC/RAW/JPEG2000/PDF/SVG/EXR source fixtures,
RGBA/hidden RGB, high-precision and native HDR domains, all policy combinations,
all animation clocks and loop/disposal semantics, all auxiliary/gain-map routes,
allocation-failure injection, and arbitrary malformed files. Existing per-codec
and metadata tests cover portions of these axes; they are not counted as complete
matrix cells here. Hardware HEIC execution requires its native platform.

The table reports one explicit fidelity `API_GAP` per configuration: configuration
confirmation cannot admit requirements against unseen pixels. The next zencodec
contract should accept actual input properties and required outcomes, distinguish
sample fidelity from display equivalence, report conversion/signaling requirements,
and use effective backend/configuration capabilities. Static discovery remains
useful but cannot be the admission oracle.

Other breaking targets already queued in zencodec: authoritative final color
context separate from provenance; compatibility separate from extraction and
content admission; rational timing with ordinary quantization and explicit exact
requests; frame-aware sinks; explicit sequence/auxiliary transcode handling.
Ambiguous container sniffing also needs candidate/admission semantics rather than
an unconditional first-match promise. Keep ergonomic CodecSet still decoding.

The shared design and adversarial-law audit are in
[zencodec's operation matrix](https://github.com/imazen/zencodec/blob/review/operation-matrix-fidelity/docs/codec-operation-matrix.md)
and [fidelity selection](https://github.com/imazen/zencodec/blob/review/operation-matrix-fidelity/docs/fidelity-selection.md).

## Recorded local run (2026-10-14)

Both inventory modes executed all 675/675 registered routes with independent
readers enabled. Each TSV contains 920 PASS, 1 EXPECTED_REJECTION, 15 API_GAP,
0 FAIL, 0 BLOCKED and 0 UNEXECUTED rows. Animation and structural prefix tests
also passed. The 15 gaps are the pixel-dependent admission requirement described
above; they are not successful delivery checks.

Large generated reports stay outside git. Verified on `dev`:

- `/mnt/v/output/zenpipe/operation-matrix/no-inventory.tsv`: SHA-256 `74472918538851806efcdf8a219187c18c455d37b304d3efae4c2f4f2d966583`.
- `/mnt/v/output/zenpipe/operation-matrix/inventory.tsv`: SHA-256 `5bcefcfc04b0281742078c73a89e1f102b9a4d65d8ad9e1b92ce536488b8166f`.

Reproduce with the recipes above and the committed Cargo.lock. Independent
readers used ImageMagick 7 and libjxl djxl; CI selects its installed executables
explicitly. The local run is not proof that pending remote platform jobs passed.
