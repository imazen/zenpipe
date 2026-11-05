# What the `tonemap` feature costs

Measured 2026-11-04 on r7900x (Ryzen 9 7900X, 10 build jobs), from 5bd3c1d plus the change that makes
`tonemap` a default. A small binary encodes RGBA8 to JPEG through `zencodecs` with features `jpeg`
and `png`, built with `lto = "thin"`, `strip = true`, `codegen-units = 16`, from a clean target.

| opt-level | Without | With | Added | Clean build, without | Clean build, with |
|---|---:|---:|---:|---:|---:|
| 3 | 3,307,160 B | 3,499,944 B | 192,784 B (5.8%) | 11.9 s | 11.8 s |
| s | 2,788,368 B | 2,940,088 B | 151,720 B (5.4%) | 10.4 s | 10.4 s |

Sizes are exact. Build times are one run each, so they only show that the difference is smaller
than the noise of a single run. This first version compiled one more crate, `zentone`: 52 against 51.
A binary that also carries other codecs adds the same code, so its percentage is smaller.

To drop it, build `zencodecs` with `default-features = false`. `zenpipe`'s `job` feature always
enables it, because a job has no other way to render HDR for an SDR-only format.

## After replacing `zentone` with `zenpixels_convert::hdr::Bt2446A`

The same binary and profile. BT.2446 Method A won the 2026-06-22 shootout of 20 curves over 76
gain-mapped images, at mean ΔE2000 3.77 against 8.16 for the next curve family
(`zentone/benchmarks/shootout_2026-06-22_findings_v2.md`).

| opt-level | Without | With | Added | Clean build, without | Clean build, with |
|---|---:|---:|---:|---:|---:|
| 3 | 3,307,160 B | 3,404,192 B | 97,032 B (2.9%) | 12.0 s | 12.2 s |
| s | 2,788,368 B | 2,894,056 B | 105,688 B (3.8%) | 10.8 s | 10.9 s |

It compiles no extra crate. 30,683 bytes of symbols are the curve's scalar `f32x16` tier.
