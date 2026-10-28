# Native AVIF encoder settings

`CodecConfig::with_avif_encoder(zenavif::AvifEncoderConfig)` preserves the
native configuration through one-shot and streaming dispatch. Use the native
adapter's `inner_mut` to set backend, chroma subsampling, color model and thread
count. Its lossless state is retained, including native lossless requests made
without a generic EncodeRequest lossless flag.

Explicit `avif_quality`, `avif_speed`, `avif_alpha_quality` and `avif_backend`
fields take priority. Generic quality/effort values do not replace a complete
native configuration. The backend-only SVT shortcut keeps its historical 4:2:0
default; it cannot overwrite chroma carried by a native configuration.

`just test-avif-native` encodes strided RGB through dispatch. It verifies exact
source pixels with a native SVT identity 4:4:4 lossless config even when the
request carries generic quality 5, effort 10 and an explicit SVT backend. The
same target checks retained Pulse cancellation and unavailable backend refusal.
`just check-pins` checks the unified zenavif revision across every manifest and
lock graph. The pinned revision is the review in
[zenavif PR 76](https://github.com/imazen/zenavif/pull/76), built on
[SVT PR 28](https://github.com/imazen/zenav1-svt/pull/28).

The graph remains on zencodec 0.1. The separate
[0.2 Pulse migration](https://github.com/imazen/zenpipe/pull/106) is still a draft;
current-main CI that substitutes the migrated zenjpeg into the 0.1 family fails
on missing SharedPulse/EncodeDeclaration/planning APIs. The locked AVIF target
passes. Zensquoosh must keep one compatible codec family until that migration
covers WebP and GIF as well.

The optional root AVIF tuner now shares the normal pinned dependency. Its
former sibling-path alias made the lock depend on another checkout's SVT
revision and produced duplicate native/parser sources. The combined
`avif-autotune,nodes-avif` library suite passes 287 tests after unification.
`just check-standalone-locks` resolves every complete standalone graph with
--locked, catching the missing how-far edge that no-deps metadata missed.
