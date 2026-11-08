# Explicit JPEG probe budgets — 2026-11-08

A screened11,339×14,173 JPEG (160,707,647 pixels) fails ImageJob probing at
120,000,000 pixels even when its caller explicitly allows200,000,000. The
failure occurs before decode: the free metadata probe constructs a default
JPEG configuration and loses the request budget.

DecodeRequest probing now forwards explicit limits to the internal JPEG
header parser and checks the returned ImageInfo against all applicable image
limits. Its estimate uses the same probe. ImageJob probe/run both construct
that existing limited request. Public signatures and free-function/default
budgets are unchanged. No global limit is raised.

Two header-only regressions change the dimensions in a generated tiny JPEG;
they do not decode nonexistent large pixel data. They require the200MP probe
and estimate to succeed, default/120MP/narrow-width rejection to remain, and
a disabled registry to reject. The ImageJob regression exercises its separate
entry point. Existing stop/limit controls remain intact.

On Core Ultra7 265K, run-heavy `just jpeg-probe-limit-check` completes118s,
peakRSS0.82GiB, min-available22,545MiB, peak-load17.41: two new codec controls,
25 existing stop/limit controls and six job JPEG controls pass. Four pre-existing
PNG ignores remain unchanged. Strict Clippy via `jpeg-probe-limit-clippy`
passes both configured graphs in18s, peakRSS0.59GiB, min-available22,977MiB,
peak-load14.72. The first orchestration attempt omitted required WebP/GIF
features and stopped before running tests; its log is retained.

An independent consumer built against the repaired owned source performs the
actual large JPEG decode, strict-sRGB conversion, aspect-preserving Lanczos3
resize and lossless PNG encode to1024×1280. It takes1s under run-heavy,
peakRSS0.95GiB, min-available24,777MiB, peak-load13.72, with a6GiB process cap.
The preview is1,188,377 bytes, SHA256
`3b04d55c9060dc9305055b1766ae6a7ab78fd13f38d70af24e97b6e0723f959f`.
An independent PNG reader reproduces all3,932,160 owned RGB bytes, SHA256
`99d3aa3c2e9ecd20d4e0544953d30317b973ba7a001061804c9c34a94bcef39e`.
The consumer separately bounds decoded codec output and final encoded bytes;
a16MiB shared codec budget correctly refuses the482,122,941-byte decoded frame.
These are bounded correctness witnesses, not throughput or corpus-wide claims.

Consumer evidence and logs are retained on i265 under
`/mnt/v/output/zensally/unsplash-screen/2026-11-08/` and
`~/tmp/zensally-composition-*probe-limit*20261108.log`.
