# Session benchmark stack validation — 2026-10-25

Parent revision: 0d8bcd6ebc6e2bc460f87fa09741d5e8bb07f42b.
Candidate: `benches/session_cache.rs` runs the unchanged benchmark registration
inside a named worker with an explicit 8 MiB stack reservation. This addresses
Windows CI's standalone executable stack overflow. The native JPEG/core bridge
pins are recorded in the accompanying manifests and lockfiles.

Command, in an export without sibling path overrides:

```text
run-heavy --mem 12G --jobs 4 -- just session-stack-check
run-heavy: done rc=0 156s | peak-RSS 0.69GiB | min-avail 17146MiB | peak-load 0.94
```

All benchmark bodies compile under Clippy; the 4K decode/crop/resize/filter
comparison executes successfully. The timing run reported only four retained
rounds and 104 noisy rounds, so it is **not performance evidence**. No codec
speed or memory-bound claim follows from this validation. Windows execution is
checked on CI; local execution was x86_64 Linux.

The raw zenbench output is stored alongside this note. The memory receipt is the
whole validation process tree, not a bound on native codec state.
