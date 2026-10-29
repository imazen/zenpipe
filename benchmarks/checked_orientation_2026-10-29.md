# Application checked orientation

Source ab7d2f14; isolated export pins shared core ab133d2c and native AVIF
0237c6b3. No path patches. Production and test source match the candidate;
manifest source replacements are recorded separately in the landing commit.

`cargo test -p zencodecs --no-default-features --features
std,avif-encode,avif-decode --test checked_orientation` passes (one test,
preserve/bake cases); matching strict Clippy passes. run-heavy 3G/jobs2,
timeout900: rc0/12s, peak RSS 0.36GiB, minimum available 11919MiB.

Six portable roots resolve. Codec-free check and strict library/test Clippy with
std,jpeg,png,jpeg-ultrahdr,avif-encode,avif-decode pass. Wrapper rc0/13s,
peak RSS 0.86GiB, minimum available 13083MiB. Lock identity changes are only
core-family and AVIF-family replacements; no unrelated version/source changes.

The first export orchestration failed before compilation because it addressed
the workspace instead of its colocated object store. Its rc127 receipt remains.
These are validation resources, not encode or compile-time benchmarks.
Existing default-orientation carrier failures remain unmodified. Native codec
qualification supplies broader orientation coverage; this gate tests application
forwarding and decoded dimensions/orientation, not independent reader parity.

Full configured API generation and freshness check pass: rc0/36s,
peak RSS 0.60GiB, minimum available 13269MiB. Generated zencodecs surface
contains the new method; other generated surfaces are unchanged.
