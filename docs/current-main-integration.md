# Paired-contract integration

The integration retains the I/O/Pulse branch and current-main behavior, including
the media-suite shell. Local qualification covers the portable codec graph,
publication, pipeline and ImageJob runtime, strict lint, API references, native
consumer compilation, WASM library compilation and both fuzz roots. Detailed
receipts and the initial failures follow below.

This is not full media-suite or release qualification. Five historical HDR tests
are placeholder panics, not codec witnesses (issue 114). Native browser Pulse
cancellation, scheduling/memory measurements and platform CI remain separate.
No crate has been published from this integration.

## Initial source integration

The isolated integration first combined `io-drivers` 5248cabf with main e1875055,
then retained main 704e55c0. Neither parent's work was replaced or rewritten.
The following initial observations are preserved with their later qualification.

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

Retained-publication validation on the resolved export passes 16 unit controls,
35 native controls, strict Clippy (including both public examples), and the
codec-free check. Full API snapshot generation and a subsequent check both
pass; the generated snapshots are retained in docs/public-api. Wrapper receipt
for that batch: rc0, 246 seconds, peak RSS 0.88 GiB, minimum available 13122 MiB.
The broad pipeline runtime and browser-native execution remain separate gates.

The default zenpipe lib/test suite and zenpipe-io-check runtime suites pass:
704 tests across 48 result groups, zero failures and zero ignored. Wrapper:
rc0, 100 seconds, peak RSS 0.67 GiB, minimum available 13358 MiB. This does
not cover the feature-gated ImageJob: subsequent fuzz CI exposed six stale
with_stop calls and a duplicate animation block there. The correction uses
the existing control helper without restarting deadlines; dedicated job and
gain-map feature tests/lint are required before landing.

CI startup also exposed an unsupported sibling_overrides input in the reusable
workflow. zen-workspace PR 3 adds that switch while preserving the default
behavior. Fuzz regression setup now explicitly disables source overrides so
committed dependency graphs, rather than rewritten manifests, are validated.

The combined job/gain-map/all-node/Tokio/schema profile executes 770 runtime
tests with no failures and five inherited ignored placeholder panics. The
placeholders are not codec witnesses (issue 114), and their historical comments
are not current behavior evidence. Strict Clippy subsequently passes after
mechanical fixes to gain-map branching, a schema test and trace-fixture borrows;
no assertions were changed. Full API generation/check now also covers this
consumer feature surface, including ImageJob and tokio_bridge, rather than
hiding it behind the default-only reference. Final lint/API batch: rc0, 36
seconds, peak RSS 0.88 GiB, minimum available 13297 MiB.

Final eacdfc67 checks pass both independent fuzz roots with `--all-targets
--locked`, all nine trace-quality controls, and workspace formatting. The
resource wrapper reports rc0, 44 seconds, peak RSS 0.86 GiB, minimum available
13189 MiB. The final workflow correction selects the pinned API nightly through
the toolchain action's input (not a nonexistent date-named action ref).

## Decode-only subset qualification (2026-10-29)

Main CI exposed encoder helpers with no callers in AVIF, HEIC and JPEG XL
decode-only builds. The helpers now follow the same encoder-feature gates as
the dispatch functions that instantiate them. Public signatures, runtime
contracts and strict lint thresholds are unchanged; no warning allowance was
added. Bitmap subfeatures all imply the common `bitmaps` feature.

At source `6ceb39c0`, an isolated locked export passed the unchanged
`just clippy-codec-subsets` matrix: all nine manifest-derived node codec pairs,
with both zencodecs all-target and zenpipe node-library checks for each.
Wrapper: rc0, 94 seconds, peak RSS 0.87 GiB, minimum available 13400 MiB.
The codec-free all-target strict Clippy control also passes (rc0, 5 seconds,
peak RSS 0.25 GiB, minimum available 13975 MiB). These are build resource
figures, not codec-memory measurements.

This qualifies feature selection/compilation, not runtime image correctness
or browser cancellation. The existing native codec and application gates
remain required.

## Locked browser dependency graph (2026-10-29)

The demo deployment still ran `cargo superwork ci-clone --add-paths` and deleted
its lock after the main CI switched to the portable graph. Its failure in
`rav1d-safe/src/msac.rs` therefore came from a floating sibling source, not
the committed decoder pin. Deployment now retains the manifests and lock and
uses `cargo build --locked`; no codec or threading feature was removed.

At source `726cc103`, `just test-standalone-wasm` passes in an isolated export:
locked wasm32-unknown-unknown checks of the demo crate and size-shim library.
Wrapper rc0, 26 seconds, peak RSS 0.56 GiB, minimum available 13482 MiB.
All six graph-root family audits and workflow YAML parsing also pass. This
qualifies the pinned graph for ordinary WASM compilation; it does not prove
the threaded release linker, generated bindings, or native browser execution.
Those are still deployment/browser gates.
