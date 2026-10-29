# Current-main integration candidate

The isolated integration combines `io-drivers` 5248cabf with main e1875055.
Neither parent is replaced or rewritten. The combined tree is not qualified and
must not be merged while conflicts or validation failures remain.

Source resolutions so far preserve both optional Pulse and current-main behavior:

- Animation/job controls retain Pulse and checked cancellation handling.
- Encode parameters retain orientation selection and tone-map policy together;
  main's adjusted canvas dimensions remain authoritative.
- Dynamic animation retains prefer-streaming configuration and EXIF color-space
  reconciliation alongside checked encode-policy/orientation setters.
- Resource estimates retain explicit lossless configuration and the current
  result wrapper.
- SIMD keeps main's compatibility multiply-add arithmetic and explicit tokens.

The metadata facade now retains main's codec-owned planners with optional
Pulse, and the application service retains typed callback errors. Both parents'
service test names remain. An isolated service build against zencodec main
3f4fd14f and zenpixels f23ac753 passed 13 tests and strict Clippy; this does not
qualify the complete consumer graph. Resource wrapper receipt: rc0, 13 seconds,
peak RSS 0.34 GiB, minimum available 13798 MiB. This is build/test resource usage,
not a codec memory-performance measurement.

All textual conflicts are resolved. Manifest parsing and recipe listing pass.
The candidate retains inventory, tone mapping, metadata rewrite, checked async
transcoding and both parents' validation recipes. Generated API/lock files use
the qualified branch's baseline pending regeneration; they are not evidence of
integration. Full compilation, native gates, graph audit and API regeneration
remain required before landing.

The draft's CI was green against its stacked base, not this combined tree.

An initial isolated resolution refused before compilation: the optional AVIF
tuner still used a sibling filesystem dependency. It now uses the same version
edge and root source patch as the ordinary AVIF adapter; tuning features remain
unchanged. The failed resolution is retained, and all graphs still need audit.

The first successful resolution exposed unintended floating JXL-encoder and
picker edges, introducing old pixel types beside the paired release. Restored
the branch's explicit pins rather than accepting that drift. Zenfilters' resize
dev dependency also still selected 0.2.2; it now selects the workspace's 0.3.1
family. This last test integration needs compilation; no geometry assertion was
changed. The initial lock delta (13 removed, 37 added identities) was rejected.

## Focused qualification (2026-10-29)

The isolated full codec profile (`std,all,jp2-decode,metadata,
checked-transcode-async,tonemap`) compiles. Its resolved graph has one copy each
of zencodec, zenmeta, zenpixels and zenpixels-convert. JPEG XL uses the reviewed
local Pulse/metadata dependency alignment; it is not yet a portable published
pin. The standalone JPEG XL graph remains a separate gate.

At source 37926d70, all four orientation controls and all 27 metadata-scrub
controls pass. These include original optional Pulse forwarding, typed deadline
refusal before native frame input, and native JPEG XL pixel/gain-map retention.
Earlier runs failed on test accessor/static-borrow usage and an obsolete string
limit variant; the corrections preserve geometry and typed MetadataSize checks.
The full logs retain those failures. Final wrapper receipt: rc0, 7 seconds,
peak RSS 2.10 GiB, minimum available 13943 MiB. These are build/test resource
figures, not codec peak-memory or performance measurements.

This does not qualify the full editor, WASM cancellation bridge, collection UI,
all codec test suites, generated API snapshots or independent consumer locks.
The media shell on main remains explicitly modeled. No native media-suite
coverage follows from a successful codec-profile compile.

Strict Clippy (`--lib --tests`, same complete feature profile) also passes:
rc0, 7 seconds, peak RSS 0.61 GiB, minimum available 13832 MiB. The source
override must follow `cargo clippy`, rather than precede the external subcommand;
the latter lost the override and selected the old JPEG XL/core graph. Both
failed attempts remain in the receipts; no old-core assertion was changed.

The standalone JPEG XL collection check exposed a separate native pin constraint:
zenpipe's `ce1a28b4` decoder lacks the frame/layer inspection methods used by the
collection adapter. Its original `fe819360` collection decoder must be retained
and qualified with the new shared contract before the application claims layer
support. Ordinary metadata/profile compilation did not cover this feature.

## Portable graph follow-through

The candidate now selects merged zenjxl efa533d0 and jxl-encoder 11c6eb0c,
retains the collection-capable decoder fe819360, and aligns explicit core
edges at 3f4fd14f. Portable resolution and consumer validation are pending.
The pin guard's self-tests pass, but its real-tree check finds the independent
wasm-size-shim still pinned to an older AVIF/core cohort. That failure remains
a landing gate; the harness must be aligned and qualified, not excluded.

Portable resolution at 5440ee60 has one core/meta/pixel/converter family and
uses only remote Git codec sources. The lock delta includes current-main
requirements that the branch lock predates: zenfilters requires libblur 0.24.0,
zenpipe-cmd requires clap 4.6.7, and zenpipe/zenfilters require zenbench 0.1.10
(resolved 0.1.11). The retained collection decoder requires smallvec 1.16.2
and num-derive 0.5.1; the new encoder requires Brotli 9. These are dependency
requirements, not evidence of native behavior. The optional AVIF tuner also
adds its AOM/SVT backend lock entries; enabling that tuner remains unqualified.

Current main's media shell is retained in the integration merge. Its 34 Node
controller tests pass after resolving the additive recipe/ignore conflicts.
The family guard now includes the independent WASM size harness, and all eight
guard self-tests pass. The size harness's source patches are aligned with the
root; its lock resolution and native/WASM compilation remain pending.

The portable 5440ee60 codec-profile run passed compilation, all four orientation
controls, all 27 metadata-scrub controls and strict Clippy for lib/tests. No
local JXL override was used. Wrapper receipt: rc0, 316 seconds, peak RSS
0.99 GiB, minimum available 12689 MiB. These describe build/test resource use;
codec peak-memory and performance remain unmeasured by this run. The root lock
now records this resolved graph. Independent roots and full API/editor tests
remain gates before landing.

All six portable application graph roots now resolve and pass the shared-family
and decoder-pin gates, including the formerly omitted WASM size harness.
Demo and codec-fuzz initially refused the new decoder's smallvec requirement;
targeted updates to 1.16.4 fixed resolution without a blanket update. Both
failed logs remain. Final wrapper: rc0, 5 seconds, peak RSS 0.12 GiB, minimum
available 14248 MiB.

The codec-fuzz lock had omitted its manifest's TIFF/PDF/SVG/RAW/HEIC closure;
resolution adds that closure (193 added identities, seven removed). The service
lock likewise adds its current native metadata planner dependencies. These
resolutions do not qualify execution of every optional feature or fuzz target.
The separate API snapshot runner and browser/WASM execution remain unqualified.

The resolved f2aee6b7 export passes `cargo check -p zenpipe --all-targets
--locked` and the size harness's library check for `wasm32-unknown-unknown`.
Wrapper receipt: rc0, 57 seconds, peak RSS 0.56 GiB, minimum available
13599 MiB. This proves target compilation, not browser execution or native
Pulse cancellation inside WASM. No runtime or pixel guarantee follows from
that compile gate. Publication tests and API generation are the next gates.
