# Strict color-fit handoff

Base: `2908ce178f86196da1b72e557ac5194d1fcfa2b5`.
ARM CI run 40953108209, job 123726114838, failed the established
`strict_allows_fits_that_lose_nothing` assertion: sRGB-to-PNM returned native
`EncodePolicy` after successful pipeline color-fit admission.

Both fitted dispatch paths enforce strictness before conversion, then forwarded
that same pipeline requirement to the native checked setter. PNM inherits the
setter that refuses every nonempty policy. The fix clears only the already
admitted strict field; native color-carrier and ICC/EXIF/XMP requirements remain
unchanged. The retained-job dispatch path performs no color fitting and still
forwards the complete policy. No public signature or dependency changed.

The eight original color-fit tests remain enabled with unchanged assertions.
The positive control additionally encodes through begin/push_rows/finish and
checks exact decoded samples. A ninth control requires both dispatch paths to
refuse unsupported native ICC/EXIF/XMP emission even after strict fit succeeds.
`just test-strict-color-fit` passes all nine tests and strict library/test Clippy.
This qualifies PNG/PNM color-fit behavior, not every codec/configuration.

Validation ran on zen-arm-big, stable Rust 1.96, isolated source export outside
workspace overrides, run-heavy 3G/jobs2, with debug info/incremental disabled.
Root Cargo.lock remained SHA256
`156919d26eade90e9b4faab1e8e5a7fa4a19ecf76f4ebdec9c311cfd810976b6`.

The first local run stopped at a compile error in the new control: it called
encode on StreamingEncoder rather than using its encoder field. Correcting the
new fixture to the real row protocol preserved all original assertions.
First receipt: rc101, 221s, peak-RSS 0.54GiB, min-avail 14251MiB, load 2.60.
Corrected receipt: rc0, 35s, peak-RSS 0.50GiB, min-avail 14061MiB, load 1.56.
These measure validation resources, not codec speed or workspace bounds.
Logs: `~/tmp/zenpipe-strict-fit-20261029/{full,fixed}.log` on zen-arm-big and
r7900x; original CI failure retained separately. Full-platform requalification
runs in CI after landing.
