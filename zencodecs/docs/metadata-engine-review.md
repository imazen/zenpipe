# Shared metadata engines and PR review

2026-09-28. Design review, not an implemented replacement or a claim that every
open PR has received a complete correctness/security review. This covers the
metadata stack and adjacent color/HDR contracts. Keep #87 draft until the
contract and ownership issues below are resolved.

## Recommendation

Own the EXIF/TIFF metadata, XMP data model/serialization, and supported MakerNote
semantics. Consolidate existing code before writing more parsers. Keep a small,
replaceable XML parser as a private dependency initially. A fork is an available
maintenance tool if a demonstrated requirement cannot be fixed upstream; owning
our public model does not require immediately owning XML tokenization.

Our maintenance problem is already multiple implementations: zencodec EXIF,
zenraw TIFF/EXIF and Apple parsing, ultrahdr-core Apple parsing, zenraw prefix-based
XMP access, the namespace-aware zencodec XMP reader, zenjpeg component filtering,
and zencodecs' additional EXIF preflight and JPEG rewriting. Buying another large
metadata library would not automatically remove those overlapping contracts.

The durable objective is one interpretation and one rewriting policy for each
format, independently of which codec or entry point exposed the packet.

## Findings with concrete evidence

### 1. The same malformed EXIF has different outcomes

Against the implementation pinned by #87, a JPEG containing an EXIF orientation
entry with an invalid TIFF field type produces:

```text
zenjpeg::container::metadata::filter_for_gain_map: Ok(12)
zencodecs::metadata::ScrubRequest::plan: Err(Malformed("EXIF field type"))
```

This was reproduced, not inferred from names. The former uses the salvaging EXIF
reader and drops the unrecognized entry; the latter has a separate strict
preflight. The result demonstrates missing shared validation, not a demonstrated
pixel-rendering exploit for that malformed packet. Strict rewriting must use the
same diagnostic-producing parser across both paths. Inspection should still be
able to expose the raw malformed entry.

Reproducer: minimal JPEG SOI/APP1/SOS/EOI, with APP1 `Exif\0\0` followed by:

```rust
let mut tiff = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec();
tiff[12] = 255; // orientation's TIFF type: invalid
```

### 2. The coordinated consumer graph omits reviewed safety work

The #84/#87 manifests pin HEIC `ac5c7cb9409875bbb09b121dee35a0d658954bf1` and
UltraHDR `633f71e9d0c8fc82cc29b58ffaa92d367d82f9f9`.

- HEIC #51 head `e8008485391a990fecf686dc2a7d349156078456` is not an ancestor
  of the selected HEIC revision. GitHub compare reports diverged histories.
  The selected `src/codec.rs` still uses `let Ok(gain_map)` and
  `gain_map_params_from(...).unwrap_or_default()` on the component path.
- The selected UltraHDR revision is two commits behind #35. It does not contain
  that PR's borrowing Apple inspection/hardening implementation.

These fixes must be integrated with the coordinated version updates and tested
through the actual consumer pins. Matching dependency versions and passing tests
in the individual PR branches do not prove inclusion. This does not imply #87
claims HEIC encoded scrubbing: it explicitly does not support that format. It is
an integration defect in the broader consumer graph, missed when assembling it.

### 3. Automatic tone mapping has an undisclosed large temporary

zenpipe #48's `measure_pq_content_peak` allocates a full `RGBF32_LINEAR`
`PixelBuffer`, gathers and linearizes the image, then calls
`ContentLightLevel::measure`. The tone-mapping pass follows. That is a 12-byte
per-pixel temporary and additional image work, despite the rowwise final kernel.
At 24 megapixels the temporary alone is about 288 MB (decimal).

Peak policy must be explicit: caller-supplied value, specified metadata/fallback
policy, or requested measurement. Measurement can reduce rows without storing a
full linear image, but it remains a distinct pass unless the chosen algorithm
can operate without a global peak. Also replace the format-name SDR-only test
with the selected output rendition/capability: JPEG can have a gain map.

## Disposition of the related PRs

| PR | Keep | Change before treating it as the lasting architecture |
|---|---|---|
| [zencodec #123](https://github.com/imazen/zencodec/pull/123), [#124](https://github.com/imazen/zencodec/pull/124) | Color/Interop retention corrections, explicit identity and attribution distinctions | Use one tag/schema inventory for type/location validation, interpretation and retention. Avoid expanding parallel handwritten tag lists. Treat incomplete rendering metadata as a rewrite error, not merely privacy-safe omission. |
| [zencodec #125](https://github.com/imazen/zencodec/pull/125) | Borrowed entry iteration | It exposes entries surviving the parser, not every source entry. Preserve raw locations, duplicate occurrences, directory edges and diagnostics in the underlying view. Do not build forensic completeness on this iterator alone. |
| [zencodec #128](https://github.com/imazen/zencodec/pull/128) | Namespace-aware XMP, rendering guards, honest coverage findings | Audit entries currently store formatted path/kind/value strings. Make borrowed raw and typed values primary; format only at presentation. Separate structural, semantic and byte-level comparison. Use a shared document for reading and editing. |
| [zencodec #130](https://github.com/imazen/zencodec/pull/130), [zenjpeg #213](https://github.com/imazen/zenjpeg/pull/213) | Integrations that unify tested type graphs | They combine implementations; they do not solve duplicated ownership. Verify required behavior at pinned consumer revisions, including HEIC/UltraHDR. |
| [zenjpeg #210](https://github.com/imazen/zenjpeg/pull/210) | Both-component filtering, namespace-aware gain-map parsing, preservation tests | Unify its component filter with #87's JPEG plan. It eagerly reserves/copies the encoded input and uses weaker EXIF validation. Keep assembly and marker framing in the JPEG owner. |
| [ultrahdr #35](https://github.com/imazen/ultrahdr/pull/35) | Borrowed Apple entries and explicit malformed/duplicate HDR refusal | Move general MakerNote inspection into the shared engine; keep HDR interpretation/math here. Consolidate zenraw's Apple parser. Replace the filtered base + second base-without-ICC + assembly copies with one component plan. `Option` still conflates absence and malformed interpretation at some parsing boundaries; retain diagnostics. |
| [heic #51](https://github.com/imazen/heic/pull/51), [#52](https://github.com/imazen/heic/pull/52) | #51's required-decode error propagation and authoritative-ISO behavior; #52's version alignment | Integrate both. Neither pin compatibility alone nor invented default gain-map parameters is sufficient. |
| [zenpipe #84](https://github.com/imazen/zenpipe/pull/84), [#87](https://github.com/imazen/zenpipe/pull/87) | Close gain-map retention bypasses; inspectable borrowed rewrite plans; preservation tests | Resolve metadata once and pass prepared results to container adapters. Replace the ambiguous use of `MetadataPolicy::PreserveExact` in a scrubber. Separate disposition, coverage and cost in reports; replace the xmpkit workaround adapter as the proposed permanent engine. |
| [zenpixels #78](https://github.com/imazen/zenpixels/pull/78), [zenavif #51](https://github.com/imazen/zenavif/pull/51) | Actual current ICC authority and distinction between encoded YCbCr signaling and decoded RGB | Make these contracts the shared assumptions for normalization. Audit metadata must retain source facts while output metadata describes the current pixels. A metadata rewrite cannot substitute for a CMS transform. |
| [zenpixels #79](https://github.com/imazen/zenpixels/pull/79) | Exact known-profile matching, borrowed canonical bytes, no CMS | Keep it narrow and injected. Do not broaden recognition into an unproven rendering-equivalence heuristic. Arbitrary ICC sanitization remains excluded. |
| [zenpipe #86](https://github.com/imazen/zenpipe/pull/86), [#48](https://github.com/imazen/zenpipe/pull/48) | #86's explicit SDR conversion and HDR refusal; #48's need to fix unsupported HDR output | One explicit conversion/display policy must own the decision. Do not layer an automatic tone-map fallback or automatic peak scan beneath the explicit contract. |
| [zenjxl #20](https://github.com/imazen/zenjxl/pull/20) | Source/current color distinctions, exact timing, all-frame preservation tests | Share those invariants with scrubbing. Timing/orientation/color are rendering dependencies, not arbitrary descriptive fields. This review did not re-audit the complete animation implementation. |

zenraw has no corresponding metadata-consolidation PR. Its current
`xmp::get_xmp_property` searches literal prefixes and quote patterns. It must
migrate to the namespace-aware engine, along with its TIFF/Apple readers; fixing
only the open JPEG/HEIC PRs would leave a separate interpretation in production.

## Engine boundaries

A possible home is one `zenmetadata` crate/repository with feature-gated modules
for EXIF/TIFF metadata, XMP and vendor notes. The name is provisional. Do not
create a chain of tiny crates solely to house a few shared enums.

- One bounded borrowed document retains source ranges, byte order, original
  occurrence, raw value and diagnostics. A typed view interprets it. Editing is
  an explicit overlay; serialization plans borrow untouched payloads where valid.
- EXIF inspection and strict rewriting use the same walker. Inspection returns
  readable data plus precise faults; rewriting refuses unresolved faults relevant
  to its guarantees. Salvage must never silently turn into validated input.
- XMP supports namespace-expanded names, scalar/URI values, Seq/Bag/Alt,
  language alternatives, structures and qualifiers. Unknown supported properties
  survive requested preservation. Unsupported forms retain raw evidence and
  namespace context; edits that cannot preserve their meaning refuse.
- MakerNote support starts with Apple because we already need its HDR semantics.
  A vendor decoder declares offset base, supported layouts, interpretation and
  relocation support. Unknown notes remain opaque. Readability does not imply
  writability; copying bytes after relocating an outer TIFF can corrupt offsets.
  Do not promise arbitrary Nikon/Canon/Sony/etc. rewrite coverage at launch.
- Container owners locate packets and implement framing, CRCs, MPF offsets and
  reconstruction dependencies. zencodecs coordinates whole-image operations.
  Typed gain-map math remains with its existing owner; a metadata library should
  not depend on codecs, pixel conversion or an HDR reconstruction engine.
- Tag knowledge should be maintained once as static schema data where useful:
  directory, numeric ID, legal value forms, interpretation and retention class.
  Check generated tables into source; do not add a heavy build-time generator.

The public engine must not expose roxmltree/xmpkit types. That keeps a parser
replacement or focused fork from becoming an ecosystem API migration. The
current xmpkit example remains useful differential-test material; its successful
supported-subset tests are not evidence that it is a general lossless editor.

Adobe separates the XMP data/serialization model, property schemas and file
storage. That is also a useful implementation boundary. We need XMP semantics,
not a general RDF reasoner: [Adobe specifications](https://developer.adobe.com/xmp/docs/xmp-specifications/).
[kamadak-exif](https://github.com/kamadak/exif-rs) remains a useful independent
reader/oracle; our editing, provenance and coverage requirements need evaluation
beyond its parsing API. Vendor relocation deserves independent fixtures and
oracles; see [ExifTool's discussion of MakerNote offsets](https://exiftool.org/faq.html#Q8).

## Dependency and compile-time contract

Prefer a peer engine wired at the application/codec boundary over a mandatory
`zencodec -> full editor -> XML/compressors` chain. Keep raw carrier transport and
small rendering contracts usable without a full editor. Codecs needing typed
metadata extraction should use the same engine directly, with relevant features.
Runtime dispatch belongs at an explicit packet/operation boundary, not per tag,
row or pixel. Known ICC normalization stays separately injectable.

There is a real migration cost: zencodec currently performs EXIF filtering in
its default metadata path. We cannot remove that dependency direction by drawing
a diagram. Preserve current entry points while introducing one implementation,
then move preparation to the orchestration boundary in a coordinated API change.
Any temporary re-export dependency must be measured and identified as a bridge;
no copied parser or `include!` duplication across crates to fake a flat graph.

Freeze an exact approved revision manifest for the stack; test those revisions
without sibling overrides. Verify required regression behavior as well as type
compatibility. Root lockfiles pin applications; published libraries need tested
version requirements and compatible public types, not an assumption that their
own Cargo.lock controls downstream resolution.

Measure fresh builds for traits-only, ordinary encode/decode, inspection,
editing and vendor extensions, including the actual consumer graph and longest
normal dependency path. The #87 results isolate the new scrubber after the
metadata integration; they do not establish the cost of a future engine or the
whole stack relative to pre-metadata code. Treat those as separate comparisons.

## Contract to settle first

Use one parsed representation for these operations:

```text
borrowed packets + source locations
            |
   bounded document + diagnostics
       /          |            \
 inspect/diff   rendering     explicit edits
                requirements     |
                       validated rewrite plan
                                  |
                        container-owned output
```

Separate three questions in a result:

1. Disposition: retained, removed, rewritten, synthesized.
2. Coverage: interpreted, opaque, malformed, unsupported, truncated/limited.
3. Cost: metadata parsing, encoded-payload walks/CRCs, decompression, owned bytes;
   any pixel operation is a separate explicitly requested plan.

Resolve required rendering information before destructive edits. If Apple HDR
headroom is required, extraction must succeed and the destination must represent
its meaning before the source MakerNote can disappear. Renaming that data to a
standard carrier is not a license to approximate its interpretation silently.

No field-level source of truth should be a formatted string. Inspection can show
one while the model retains exact integers/rationals/float bits, byte order,
qualifiers, duplicates and source ranges. Semantic comparison needs explicit
rules; a structural or byte diff must remain available separately.

## Sequence and acceptance gates

1. Integrate the omitted HEIC/UltraHDR fixes and add a consumer regression that
   fails on the currently selected pins. This is correctness work independent of
   choosing a new crate name.
2. Settle the shared borrowed document, diagnostics and rewrite contract using
   existing EXIF/Apple cases. Consolidate those readers first; remove the extra
   preflight only once the shared strict path covers its cases.
3. Build the owned XMP model/editor on a private XML parser. Promote it only after
   arrays, qualifiers, unknown properties, namespace aliases, duplicate fields,
   malformed input and repeat edits have round-trip/differential evidence.
4. Move JPEG component planning to its owner and share it between assembly and
   scrubbing. Keep PNG/JXL rewrite code with reusable container primitives rather
   than proliferating independent format validators.
5. Expand vendor coverage according to real callers and fixtures. A complete
   MakerNote ecosystem is an ongoing product commitment, not a release checkbox.

Acceptance includes: identical malformed-input outcomes across entry points;
precise reported coverage; no silently lost unknown values in preservation mode;
explicit refusal of unsafe relocation; idempotent normalization; unchanged image
payloads and display results for scrub-only operations; bounded parsing and
expansion; caller cancellation; and no unexpected image-sized allocations or
pixel passes. Test oracles stay in development dependencies/tools. Compile cost
and actual dependency depth are measured before moving more callers.
