# Reviewed portable publication dependency deltas

Each of the 52 added/removed identity records is scoped to its lock, direction,
version, source and checksum in [the admission file](../scripts/qualify/publication-deltas-2026-10-28.json).
The frozen original locks are hash-checked before that admission applies.
No crate-name wildcard is added. These are dependency reasons, not native pixel qualification.

Registry parent manifests were extracted from official crate archives after matching
their complete SHA-256 to the frozen lock. Git parents were read as immutable Git
blobs at the locked revision; inherited hayro requirements were resolved from its
same-revision workspace manifest. Source manifest identities are recorded in
[the manifest proof index](publication-delta-manifests-2026-10-28.json).

| Lock | Change | Exact package | Parent edges and manifest requirements |
|---|---|---|---|
| Cargo.lock | added | bitvec 1.1.1 | tiff 0.11.3 → lerc-rs 0.7.0 (`0.7.0`, dependencies); lerc-rs 0.7.0 → bitvec 1.1.1 (`1.0.1`, dependencies) |
| Cargo.lock | added | fax 0.3.0 | tiff 0.11.3 → fax 0.3.0 (`0.3.0`, dependencies) |
| Cargo.lock | added | fearless_simd 0.7.0 | zenpdf 0.2.0 → hayro 0.8.0 (`0.8.0`, dependencies); hayro 0.8.0 → vello_cpu 0.3.0 (`0.3`, dependencies); vello_cpu 0.3.0 → vello_common 0.3.0 (`0.3.0`, dependencies); vello_common 0.3.0 → fearless_simd 0.7.0 (`0.7.0`, dependencies) |
| Cargo.lock | added | fearless_simd 1.2.0 | hayro-syntax 0.8.0 → hayro-jbig2 0.3.1 (`0.3.1`, dependencies); hayro-jbig2 0.3.1 → fearless_simd 1.2.0 (`1.0.0`, dependencies) |
| Cargo.lock | added | funty 2.0.0 | tiff 0.11.3 → lerc-rs 0.7.0 (`0.7.0`, dependencies); lerc-rs 0.7.0 → bitvec 1.1.1 (`1.0.1`, dependencies); bitvec 1.1.1 → funty 2.0.0 (`^2.0`, dependencies) |
| Cargo.lock | added | glifo 0.4.0 | zenpdf 0.2.0 → hayro 0.8.0 (`0.8.0`, dependencies); hayro 0.8.0 → vello_cpu 0.3.0 (`0.3`, dependencies); vello_cpu 0.3.0 → glifo 0.4.0 (`0.4.0`, dependencies) |
| Cargo.lock | added | hayro 0.8.0 | zenpdf 0.2.0 → hayro 0.8.0 (`0.8.0`, dependencies) |
| Cargo.lock | added | hayro-ccitt 0.4.0 | hayro-syntax 0.8.0 → hayro-ccitt 0.4.0 (`0.4.0`, dependencies) |
| Cargo.lock | added | hayro-interpret 0.8.0 | zenpdf 0.2.0 → hayro 0.8.0 (`0.8.0`, dependencies); hayro 0.8.0 → hayro-interpret 0.8.0 (`0.8.0`, dependencies) |
| Cargo.lock | added | hayro-jbig2 0.3.1 | hayro-syntax 0.8.0 → hayro-jbig2 0.3.1 (`0.3.1`, dependencies) |
| Cargo.lock | added | hayro-jpeg2000 0.4.1 | zenjp2 0.1.0 → hayro-jpeg2000 0.4.1 (`0.4.1`, dependencies) |
| Cargo.lock | added | hayro-jpeg2000 0.4.1 | hayro-syntax 0.8.0 → hayro-jpeg2000 0.4.1 (`0.4.1`, dependencies) |
| Cargo.lock | added | lerc-rs 0.7.0 | tiff 0.11.3 → lerc-rs 0.7.0 (`0.7.0`, dependencies) |
| Cargo.lock | added | likely_stable 0.1.3 | rav1d-safe 0.7.0 → likely_stable 0.1.3 (`0.1.3`, dependencies) |
| Cargo.lock | added | lzma-rust2 0.24.0 | tiff 0.11.3 → lzma-rust2 0.24.0 (`0.24.0`, dependencies) |
| Cargo.lock | added | radium 0.7.0 | tiff 0.11.3 → lerc-rs 0.7.0 (`0.7.0`, dependencies); lerc-rs 0.7.0 → bitvec 1.1.1 (`1.0.1`, dependencies); bitvec 1.1.1 → radium 0.7.0 (`0.7`, dependencies) |
| Cargo.lock | added | rav1d-disjoint-mut 0.4.0 | rav1d-safe 0.7.0 → rav1d-disjoint-mut 0.4.0 (`0.4.0`, dependencies) |
| Cargo.lock | added | read-fonts 0.43.3 | zenpdf 0.2.0 → hayro 0.8.0 (`0.8.0`, dependencies); hayro 0.8.0 → hayro-interpret 0.8.0 (`0.8.0`, dependencies); hayro-interpret 0.8.0 → skrifa 0.46.2 (`0.46.0`, dependencies); skrifa 0.46.2 → read-fonts 0.43.3 (`0.43.2`, dependencies) |
| Cargo.lock | added | resvg 0.49.0 | zensvg 0.1.0 → resvg 0.49.0 (`0.49.0`, dependencies) |
| Cargo.lock | added | skrifa 0.46.2 | zenpdf 0.2.0 → hayro 0.8.0 (`0.8.0`, dependencies); hayro 0.8.0 → hayro-interpret 0.8.0 (`0.8.0`, dependencies); hayro-interpret 0.8.0 → skrifa 0.46.2 (`0.46.0`, dependencies) |
| Cargo.lock | added | tap 1.0.1 | tiff 0.11.3 → lerc-rs 0.7.0 (`0.7.0`, dependencies); lerc-rs 0.7.0 → bitvec 1.1.1 (`1.0.1`, dependencies); bitvec 1.1.1 → tap 1.0.1 (`1`, dependencies) |
| Cargo.lock | added | usvg 0.49.0 | zensvg 0.1.0 → usvg 0.49.0 (`0.49.0`, dependencies) |
| Cargo.lock | added | vello_common 0.3.0 | zenpdf 0.2.0 → hayro 0.8.0 (`0.8.0`, dependencies); hayro 0.8.0 → vello_cpu 0.3.0 (`0.3`, dependencies); vello_cpu 0.3.0 → vello_common 0.3.0 (`0.3.0`, dependencies) |
| Cargo.lock | added | vello_cpu 0.3.0 | zenpdf 0.2.0 → hayro 0.8.0 (`0.8.0`, dependencies); hayro 0.8.0 → vello_cpu 0.3.0 (`0.3`, dependencies) |
| Cargo.lock | added | weezl 0.2.1 | tiff 0.11.3 → weezl 0.2.1 (`0.2.1`, dependencies) |
| Cargo.lock | added | wyz 0.5.1 | tiff 0.11.3 → lerc-rs 0.7.0 (`0.7.0`, dependencies); lerc-rs 0.7.0 → bitvec 1.1.1 (`1.0.1`, dependencies); bitvec 1.1.1 → wyz 0.5.1 (`0.5`, dependencies) |
| Cargo.lock | removed | fearless_simd 0.4.1 | hayro-syntax 0.7.2 → hayro-jbig2 0.3.0 (`0.3.0`, dependencies); hayro-jbig2 0.3.0 → fearless_simd 0.4.1 (`0.4.0`, dependencies) |
| Cargo.lock | removed | font-types 0.11.3 | zenpdf 0.2.0 → hayro 0.7.1 (`0.7.1`, dependencies); hayro 0.7.1 → hayro-interpret 0.7.0 (`0.7.0`, dependencies); hayro-interpret 0.7.0 → skrifa 0.42.1 (`0.42.0`, dependencies); skrifa 0.42.1 → read-fonts 0.39.2 (`0.39.2`, dependencies); read-fonts 0.39.2 → font-types 0.11.3 (`0.11.3`, dependencies) |
| Cargo.lock | removed | glifo 0.1.1 | zenpdf 0.2.0 → hayro 0.7.1 (`0.7.1`, dependencies); hayro 0.7.1 → vello_cpu 0.0.8 (`0.0.8`, dependencies); vello_cpu 0.0.8 → glifo 0.1.1 (`0.1.0`, dependencies) |
| Cargo.lock | removed | hayro 0.7.1 | zenpdf 0.2.0 → hayro 0.7.1 (`0.7.1`, dependencies) |
| Cargo.lock | removed | hayro-ccitt 0.3.0 | hayro-syntax 0.7.2 → hayro-ccitt 0.3.0 (`0.3.0`, dependencies) |
| Cargo.lock | removed | hayro-interpret 0.7.0 | zenpdf 0.2.0 → hayro 0.7.1 (`0.7.1`, dependencies); hayro 0.7.1 → hayro-interpret 0.7.0 (`0.7.0`, dependencies) |
| Cargo.lock | removed | hayro-jbig2 0.3.0 | hayro-syntax 0.7.2 → hayro-jbig2 0.3.0 (`0.3.0`, dependencies) |
| Cargo.lock | removed | hayro-jpeg2000 0.3.5 | hayro-syntax 0.7.2 → hayro-jpeg2000 0.3.5 (`0.3.5`, dependencies) |
| Cargo.lock | removed | rav1d-disjoint-mut 0.3.2 | rav1d-safe 0.6.0 → rav1d-disjoint-mut 0.3.2 (`0.3.2`, dependencies) |
| Cargo.lock | removed | read-fonts 0.39.2 | zenpdf 0.2.0 → hayro 0.7.1 (`0.7.1`, dependencies); hayro 0.7.1 → hayro-interpret 0.7.0 (`0.7.0`, dependencies); hayro-interpret 0.7.0 → skrifa 0.42.1 (`0.42.0`, dependencies); skrifa 0.42.1 → read-fonts 0.39.2 (`0.39.2`, dependencies) |
| Cargo.lock | removed | resvg 0.48.1 | zensvg 0.1.0 → resvg 0.48.1 (`0.48.1`, dependencies) |
| Cargo.lock | removed | skrifa 0.42.1 | zenpdf 0.2.0 → hayro 0.7.1 (`0.7.1`, dependencies); hayro 0.7.1 → hayro-interpret 0.7.0 (`0.7.0`, dependencies); hayro-interpret 0.7.0 → skrifa 0.42.1 (`0.42.0`, dependencies) |
| Cargo.lock | removed | usvg 0.48.1 | zensvg 0.1.0 → usvg 0.48.1 (`0.48.1`, dependencies) |
| Cargo.lock | removed | vello_common 0.0.8 | zenpdf 0.2.0 → hayro 0.7.1 (`0.7.1`, dependencies); hayro 0.7.1 → vello_cpu 0.0.8 (`0.0.8`, dependencies); vello_cpu 0.0.8 → vello_common 0.0.8 (`0.0.8`, dependencies) |
| Cargo.lock | removed | vello_common 0.0.9 | zenpdf 0.2.0 → hayro 0.7.1 (`0.7.1`, dependencies); hayro 0.7.1 → vello_cpu 0.0.8 (`0.0.8`, dependencies); vello_cpu 0.0.8 → glifo 0.1.1 (`0.1.0`, dependencies); glifo 0.1.1 → vello_common 0.0.9 (`0.0.9`, dependencies) |
| Cargo.lock | removed | vello_cpu 0.0.8 | zenpdf 0.2.0 → hayro 0.7.1 (`0.7.1`, dependencies); hayro 0.7.1 → vello_cpu 0.0.8 (`0.0.8`, dependencies) |
| demo/crate/Cargo.lock | added | likely_stable 0.1.3 | rav1d-safe 0.7.0 → likely_stable 0.1.3 (`0.1.3`, dependencies) |
| demo/crate/Cargo.lock | added | rav1d-disjoint-mut 0.4.0 | rav1d-safe 0.7.0 → rav1d-disjoint-mut 0.4.0 (`0.4.0`, dependencies) |
| demo/crate/Cargo.lock | added | rustc_version 0.4.1 | rav1d-safe 0.7.0 → likely_stable 0.1.3 (`0.1.3`, dependencies); likely_stable 0.1.3 → rustc_version 0.4.1 (`0.4`, build-dependencies) |
| demo/crate/Cargo.lock | added | semver 1.0.28 | rav1d-safe 0.7.0 → likely_stable 0.1.3 (`0.1.3`, dependencies); likely_stable 0.1.3 → rustc_version 0.4.1 (`0.4`, build-dependencies); rustc_version 0.4.1 → semver 1.0.28 (`1.0`, dependencies) |
| demo/crate/Cargo.lock | removed | rav1d-disjoint-mut 0.3.2 | rav1d-safe 0.6.0 → rav1d-disjoint-mut 0.3.2 (`0.3.2`, dependencies) |
| zencodecs/fuzz/Cargo.lock | added | likely_stable 0.1.3 | rav1d-safe 0.7.0 → likely_stable 0.1.3 (`0.1.3`, dependencies) |
| zencodecs/fuzz/Cargo.lock | added | rav1d-disjoint-mut 0.4.0 | rav1d-safe 0.7.0 → rav1d-disjoint-mut 0.4.0 (`0.4.0`, dependencies) |
| zencodecs/fuzz/Cargo.lock | added | rustc_version 0.4.1 | rav1d-safe 0.7.0 → likely_stable 0.1.3 (`0.1.3`, dependencies); likely_stable 0.1.3 → rustc_version 0.4.1 (`0.4`, build-dependencies) |
| zencodecs/fuzz/Cargo.lock | added | semver 1.0.28 | rav1d-safe 0.7.0 → likely_stable 0.1.3 (`0.1.3`, dependencies); likely_stable 0.1.3 → rustc_version 0.4.1 (`0.4`, build-dependencies); rustc_version 0.4.1 → semver 1.0.28 (`1.0`, dependencies) |
| zencodecs/fuzz/Cargo.lock | removed | rav1d-disjoint-mut 0.3.2 | rav1d-safe 0.6.0 → rav1d-disjoint-mut 0.3.2 (`0.3.2`, dependencies) |

The new roots require hayro 0.8, resvg/usvg 0.49 and JP2 0.4.1. Their former
0.7/0.48/0.3 parent trees explain the removals. The TIFF fork adds LERC/LZMA
and uses fax 0.3; b19 requires rav1d-disjoint-mut 0.4 and likely_stable. The
latter's build dependency explains rustc_version and semver in demo/fuzz graphs.
All 52 records are accounted for by these immutable parent chains; accepting
these identities does not accept the unresolved AVIF orientation/API/browser gates.
