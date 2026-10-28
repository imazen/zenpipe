# Publication qualification repairs

The portable consumer experiment uses shared core f02a17ec, paired pixels
283655c5, AVIF/parse b25f7604, rav1d b19aec65 and extras a5e79261. These source
identities do not imply a qualified consumer graph. The frozen initial run
retains all its failures: publication controls passed 15 unit and 35 native
cases, while full API, AVIF orientation, graph delta admission and browser
controls failed independently. No original assertion or feature was removed.

## Browser fallback preview

The retained demo failure reached `encode_preview` on the canvas backend, but
the worker rejected every preview unless WASM was loaded. The modal's existing
error path then hid the image and reset all three statistics. Export already
used the browser canvas encoder; preview now uses the same available browser
facility and passes its actual MIME to the modal, including a browser format
fallback. This is browser encoding, not native codec qualification.

`just checked-preview-source` runs two Node protocol controls against the real
worker with a stubbed canvas. Both fail on the immutable former worker with
`Encode preview requires WASM backend` and pass after the repair. They verify
dispatch, settings, dimensions, bytes and actual MIME; they do not verify image
pixels. The new `demo/tests/encode-preview.spec.js` forces the canvas backend,
checks JPEG bytes and independent browser readback dimensions, then exercises
the original preview/statistics assertions. It is prepared but unrun. The
original 10-second assertion and existing PNG-download skip are unchanged.

## Compiler and runner diagnostics

The prepared first compiler command is
`just checked-publication-diagnostic RESULTS`. It retains
`all,jp2-decode,metadata,checked-transcode-async`, writes raw Cargo JSON and
stderr, and extracts rendered diagnostics with implementation source spans.
It is unrun. The former snapshot wrapper's E0046 summary does not identify the
first implementation, so another wrapper-only attempt cannot settle ownership.

Immutable source already identifies two missing required methods:

- heic ac5c7cb9, `src/codec.rs:610`: `HeicDecodeJob` implements `with_stop`
  at line616 but no `with_pulse`.
- zenraw 50f9c694, `src/zencodec_impl.rs:381`: `RawDecodeJob` implements
  `with_stop` at line387 but no `with_pulse`.

Shared core f02 requires the retained Pulse at
`src/traits/decoding.rs:165`; its cancellation-only adapter is deprecated.
The owned forwarding jobs `IgnoresMetadataJob` and `CannotPlanJob` already
forward `with_pulse`. No missing owned implementation was found. Neither
sibling workspace nor native pin was changed. A sibling migration must retain
the actual `SharedPulse` through jobs and sessions, update native polling and
planning/progress boundaries, and preserve its existing cancellation controls.
Simply storing another cancellation-only token does not satisfy that contract.
Root must route that implementation/requalification to the owning repositories.

The runner now names the actual `svg` feature and retains the checksum target's
required `all` feature. The archived guide receives its exact immutable core
revision in `GITHUB_SHA`. A first narrow browser preview check precedes the
unchanged full suite. These repaired Rust/browser commands remain unrun.

## Orientation contract remains unresolved

The [core correctness model](https://github.com/imazen/zencodec/blob/f02a17ec6001a9efcad1267f92a3ea928ed06e57/docs/correctness-model.md#orientation-one-authority-reconciled)
makes `Metadata::orientation` authoritative and says the codec embeds its value.
The [shared specification](https://github.com/imazen/zencodec/blob/f02a17ec6001a9efcad1267f92a3ea928ed06e57/docs/spec.md)
offers orientation hints for decoding, not an encode representation selector.
The two existing AVIF assertions require a retained Rotate90/non-Identity tag.
Both stay intact and failing in the frozen receipt.

The [b25 still encoder](https://github.com/imazen/zenavif/blob/b25f7604fbb71ead6517ec27fa52646770e3606a/src/codec/encode_job.rs#L154)
instead bakes rotation into pixels and reconciles EXIF/XMP to identity. Its
animation route applies container transforms. Treating either representation as
acceptable would change the existing consumer expectation; no such test change
has been made.

Three prepared witnesses use a non-square 32×24 asymmetric image. The explicit
Rotate90 and EXIF-only cases compare exact decoded samples against a separately
decoded identity baseline with a manually computed clockwise permutation,
verify dimensions, and require Correct not to rotate the result again. Direct
container inspection checks coded dimensions, irot/imir and the retained EXIF
tag. The EXIF-prefix parser check is strict: the known serializer issue may
remain a separate failure, not a reason to omit that check. These witnesses
test the current baked route alongside the original tag assertions; they do
not decide which contract should win. They remain uncompiled/unrun and do not
certify native RGB fidelity or an independent third-party decoder.

The third witness prepares the native tag-preserving escape hatch:
`AvifEncoderConfig::inner_mut()` selects the underlying `rotation(3)` while
metadata remains identity, and decode explicitly selects Preserve or Correct.
The backend validates quarter turns0–3 despite the outer rotation method's
degree wording;3 represents270 degrees CCW. This route bypasses the ordinary
metadata orientation channel and is not a codec-agnostic per-request selector.
Passing Rotate90 metadata still invokes unconditional baking. A configured
encoder in a codec registry does not remove that per-image impedance.

The smallest native change that meets the existing tag-carriage expectation
would use the already available irot/imir lowering for still metadata, retain
the stored pixels and reconcile EXIF to the carried orientation. Keeping both
representations selectable would require a reviewed native configuration mode;
portable selection would need a shared encode contract as well. The native
source records reader-compatibility reasons for its current baking behavior,
so root must resolve that choice before any native change or assertion update.
No speculative repin or native API modification was made here.

## Exact dependency admission

The [52-record ledger](publication-deltas-2026-10-28.md) traces every added and
removed identity to immutable parent manifest requirements. Twenty-seven package
manifests plus the inherited hayro workspace manifest were verified: registry
archives match frozen lock checksums, Git content comes from exact revisions.
The admission configuration names exact versions, sources, checksums, graph
roots and change directions, bound to the original lock hashes and chosen
native revisions. No crate-name wildcard was added. Nine source-only Python
controls preserve the old refusal and check exact admission, checksum/version,
direction, root, baseline, source-scope and multiplicity refusal. This reviews
dependency movement; it does not turn the frozen native/API failures into passes.
