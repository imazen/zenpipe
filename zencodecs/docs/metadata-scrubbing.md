# Metadata scrubbing and normalization

This work is metadata-only: no pixel decoding, scans, color conversion,
quantization, tone mapping, or implicit orientation baking. ICC sanitization is
out of scope. Retained ICC profiles are opaque and may contain identifying text.

## Contract and release gates

1. A request first produces a bounded plan over borrowed encoded bytes. Planning
   validates container framing and required rendering contracts before emitting
   anything. A caller can review every retained, removed, and regenerated carrier.
2. Output chunks borrow unmodified image payloads. Collecting output explicitly
   allocates one encoded buffer; writing chunks to a caller sink does not.
3. Every unsupported display contract fails explicitly. Unknown metadata is
   removed, but unknown critical image structures are rejected. No metadata parse
   failure may silently become absent metadata or default HDR parameters.
4. ICC is retained by default. Optional normalization only replaces recognized,
   rendering-equivalent profiles; an unrecognized profile remains unchanged.
   Caller-supplied runtime services avoid a codec-to-editor/CMS dependency edge.
5. Source XMP is removed by default. Selective retention uses an explicitly
   supplied bounded editor that returns serialized XMP. Required gain-map
   signaling is separate from optional descriptive XMP.
6. Compressed metadata requires a bounded decompressor only when interpreting or
   editing it. Removing a known private compressed carrier does not inflate it.
7. Reports describe metadata carriers, not steganography or all possible
   identifying information in pixels, image bitstreams, or opaque profiles.

## Implementation tiers

| Tier | Required behavior | Regression evidence |
|---|---|---|
| Minimal | Policy enforcement, EXIF orientation/rights, private carrier removal, bounded plans and runtime services | Seeded privacy markers; malformed/truncated inputs; no image-byte changes; sink and collected output agree |
| Unified HDR/color | Retain authoritative rendering dependencies; normalize only recognized ICC; regenerate gain-map discovery/offsets; refuse unsupported contracts | Both gain-map components scrubbed; parameters and decoded HDR preserved; unknown profiles unchanged; no pixel hooks invoked |
| JXL | Box and compressed-box policy; preserve codestream; explicitly relinquish JPEG reconstruction when removing its data; preserve gain-map/HDR interpretation | Brob/JBRD/unknown boxes; codestream identity; valid decoder output; bounded metadata expansion |
| PNG SDR/HDR | Chunk policy and CRC validation; preserve palette/transparency/animation; validate color/HDR combinations; keep sample payloads | IDAT/fdAT identity; all-frame decode comparison; PQ/HLG U16 samples; illegal/duplicate/unknown critical chunks fail |

Tests must distinguish changed carrier bytes from changed rendering. Structural
metadata diffs are not RDF graph equivalence. Rewriting EXIF/XMP must never be
used to conceal incomplete coverage. Animation tests inspect every frame, not
only the poster. Existing metadata conformance tests remain the integration gate.

## Build and performance gates

No new mandatory dependency and no new library layer below zencodecs. Optional
editors/decompressors are wired at the application boundary. Measure three fresh
target-directory builds before/after with identical features and pins; record
medians, all runs, dependency counts, and longest normal dependency path. Compare
minimal, JPEG/HDR, JXL, and PNG feature sets separately. Run timed builds serially.
Report warm builds separately; do not credit unrelated dependency migrations.

Keep tests and reproducible measurements with each PR. Stack integration work on
zenpipe #86; integrate the earlier gain-map policy #84 without reverting the
coordinated media dependency graph. Do not publish crates as part of this work.

## Calling the API

Enable `zencodecs/metadata`; add `jpeg` for JPEG rewriting. PNG and JXL container
rewriting need no pixel codec feature. The normal default build is unchanged.

```rust
use zencodecs::metadata::ScrubRequest;

let plan = ScrubRequest::new(encoded)
    .with_byte_limits(128 << 20, 128 << 20)
    .with_metadata_limit(1 << 20)
    .plan()?;
for entry in plan.report() {
    println!("{:?} {} {:?}", entry.action(), entry.carrier(), entry.source_range());
}
// No second image-sized allocation. A sink error can leave a partial file.
plan.chunks().try_for_each(|bytes| output.write_all(bytes))?;
// Alternatively: let encoded_output = plan.to_vec()?;
```

`with_policy` controls EXIF retention. Source XMP is removed unless
`with_xmp_editing(true)` explicitly delegates its retention to `Services`.
The default retains color and orientation. Dropping a nonidentity EXIF
orientation or an explicit ICC profile is refused: do that through a pixel
conversion, with the resulting color/orientation described correctly.
`PreserveExact` is not a byte-copy mode here: source-XMP retention requires an
editor, and the scrubber always removes private/unknown container carriers.
Use the existing encode/transcode retention API for a preservation workflow.

The plan report accounts for container carriers, including image data. Regenerated
carriers have an empty source range. It is separate from property-level inspection
and diffing in `zencodec::metadata_audit` and namespace-aware `zencodec::xmp`.
Those APIs inspect EXIF/MakerNote envelopes and XMP properties; this scrubber
reuses the existing EXIF reader/writer and removes MakerNotes through retention
policy. It does not add a new vendor-specific MakerNote interpreter.

## Coverage and explicit refusals

- JPEG: strip comments/private APP markers, descriptive XMP and trailers; prune
  EXIF; retain validated ICC chunk sequences and Adobe coding signals. JFIF
  thumbnails are removed. Gain maps support one MPF primary plus one secondary
  gain-map image. Both are scrubbed, and MPF offsets/XMP discovery are regenerated.
  Present ISO parameters are authoritative; invalid ISO never falls back to XMP.
  Conflicting ISO/XMP parameters, unsupported direction/alternate color space,
  unindexed secondary JPEGs, other MPF layouts, and extended-XMP editing refuse.
  ISO/XMP comparison permits the legacy serializer's decimal rounding (1e-6).
- JXL: preserve image codestreams, orientation/color carried within them, and
  validated uncompressed `jhgm` envelopes. Drop unknown/private boxes and XMP;
  compressed EXIF requires a bounded Brotli service. Compressed `jhgm` and
  codestream ICC normalization refuse. Removing `jbrd` or `brob(jbrd)` requires
  `with_jpeg_reconstruction_removal(true)`: byte-exact JPEG reconstruction is lost.
  Codestreams remain opaque; planning does not validate their decoded contents.
- PNG: validate CRCs, framing/order, singleton color/HDR chunks, and APNG sequence
  numbers. Preserve IDAT/fdAT, palette/transparency, frame geometry/timing,
  sBIT/pHYs/bKGD, and existing SDR/HDR signals. Unknown primaries/transfers remain
  unknown. Normalize the descriptive iCCP name without inflating the profile.
  Only explicit known-profile normalization decompresses ICC. This is not a full
  image decoder or a validator for every chromaticity/mastering-display value.
- Other formats currently return `Unsupported`; existing codec retention still
  applies to their encode/transcode routes. AVIF/WebP/HEIC container-only rewriting
  is not claimed by these four tiers.

The mandatory cost is an encoded-container walk. PNG CRC validation reads payload
bytes, including IDAT. JPEG marker discovery and gain-map validation make multiple
encoded-byte walks. No path performs pixel scans, transforms, quantization, or
image decoding. Metadata parsing/serialization allocates within configured packet
and carrier bounds; the report and chunk index scale with carrier count. These are
not streaming-input parsers: the input must remain available for borrowed output.

## Optional application wiring

`tools/metadata-services` is a standalone, unpublished application/test harness.
It depends directly on zencodecs, zenpixels-convert, xmpkit, XML parsers/writers,
and compressors, then implements `Services`. None is a new dependency of the
zencodecs metadata feature. The shared known-profile helper only recognizes the
bundled Display P3, Adobe RGB, and Rec.2020 profiles, allowing changes to volatile
header identity fields; unknown profiles remain unchanged. Arbitrary ICC profile
sanitization is deliberately absent.

```sh
cargo run --manifest-path tools/metadata-services/Cargo.toml -- \
  input.jpg output.jpg --keep-rights
# Additional explicit choices:
# --normalize-icc
# --remove-jpeg-reconstruction
```

The output must not already exist. The example retains creator/rights fields when
requested, not arbitrary descriptive properties. XML preflight rejects DTDs,
excessive nesting/nodes, prefix rebinding, alternate RDF subjects, and duplicate
properties. Structured/qualified rights beyond plain values, RDF arrays and
`xml:lang` refuse. Compressed metadata has expansion bounds; Brotli history is
bounded separately (up to 4 MiB, with a 64 KiB minimum workspace allowance).

Tests exposed xmpkit 0.1.6 limitations: copying through generic `XmpValue::Array`
loses Seq/Alt/language semantics; editing its parsed document preserves them.
Its serializer can mishandle aliases for built-in namespaces, and deletion of
some unknown namespace properties is incomplete. Output is reparsed and checked
against the policy; unsupported edits fail instead of returning a packet with
private fields. The merger uses parsed XML and quick-xml serialization, preserving
array kinds and qualifiers. It emits the toolkit's built-in prefixes so its own
output can be edited again. No global namespace registry is mutated.

The optional adapter is an example with explicit supported-input limits, not a
replacement for a general RDF editor. The runtime boundary lets an application
supply a different editor without changing or recompiling the codec dependency
chain. Core audit/diff does not require this editor and remains available even
when an edit is refused.
