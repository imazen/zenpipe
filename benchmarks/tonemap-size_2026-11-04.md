# What the `tonemap` feature costs

Measured 2026-11-04 on r7900x (Ryzen 9 7900X, 10 build jobs), from 5bd3c1d plus the change that makes
`tonemap` a default. A small binary encodes RGBA8 to JPEG through `zencodecs` with features `jpeg`
and `png`, built with `lto = "thin"`, `strip = true`, `codegen-units = 16`, from a clean target.

| opt-level | Without | With | Added | Clean build, without | Clean build, with |
|---|---:|---:|---:|---:|---:|
| 3 | 3,307,160 B | 3,499,944 B | 192,784 B (5.8%) | 11.9 s | 11.8 s |
| s | 2,788,368 B | 2,940,088 B | 151,720 B (5.4%) | 10.4 s | 10.4 s |

Sizes are exact. Build times are one run each, so they only show that the difference is smaller
than the noise of a single run. The feature compiles one more crate, `zentone`: 52 against 51.
A binary that also carries other codecs adds the same code, so its percentage is smaller.

To drop it, build `zencodecs` with `default-features = false`. `zenpipe`'s `job` feature always
enables it, because a job has no other way to render HDR for an SDR-only format.
