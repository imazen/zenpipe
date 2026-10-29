# AVIF auxiliary deadline qualification

Test source f888f86630a9b9d71ab0b82db6ffafa26173d6eb; fixed source
7f3adbd940460562a510950fa002794a629d3692, parent 2b1aeab5.
Both frozen exports use identical committed portable dependency locks.

`just avif-gainmap-pulse-check` retains the existing successful native tmap
embedding test, adds a nested deadline control for both grayscale and RGB,
and runs strict library Clippy. All native builds use std,avif-decode,
avif-encode,jpeg-ultrahdr, no default features, and --locked. Runs are serialized
under run-heavy --mem 3G --jobs 2 and timeout 900.

Before the repair, the new test fails: the grayscale auxiliary encode ignores
an expired deadline and reaches invalid base storage, returning InvalidInput
instead of the original TimedOut. The repaired test passes both channel counts
and inspects the native typed cancellation cause. Strict Clippy passes. The
existing successful tmap embedding test also passes without assertion changes.
The low-level native API accepts StopToken, so this fix forwards cancellation;
it does not claim new native progress reporting or cancellation-latency bounds.

| Run | Result | Seconds | Peak RSS | Minimum available |
|---|---|---:|---:|---:|
| Before, new regression | rc101, expected assertion failure | 17 | 0.95 GiB | 10999 MiB |
| Fixed regression + strict Clippy | rc0 | 11 | 0.86 GiB | 11062 MiB |
| Existing successful embedding control | rc0 | 1 | 0.26 GiB | 11823 MiB |

These wrapper observations describe qualification resource use, not performance
comparisons. The AVIF orientation representation contract remains separate.
