# Native codec heads against the zencodec 0.2 core, re-resolved 2026-10-28

Observed 2026-10-28 between 19:13 and 19:25 UTC, against io-drivers `363f7c50`. Heads move: resolve them again
before relying on this table.

```text
gh api repos/imazen/<repo>/commits/<default-branch> -q .sha
gh pr list -R imazen/<repo> --state open --json number,headRefOid,title
gh api "repos/imazen/<repo>/contents/Cargo.toml?ref=<rev>" -q .content | base64 -d | grep -E '^(zencodec|zencodec-io|zenmeta)'
```

All five graph roots (the workspace, `fuzz/`, `demo/crate/`, `tools/metadata-services/` and `zencodecs/fuzz/`) pin
the same native revisions. Each lockfile holds one core family, at `6214c93c` (`just check-core-family`).

The question for each native: is there an already-qualified head this graph can use? A head qualifies when it
meets three conditions:
- it implements the current contract;
- it names the core family by version edges, so the root patch resolves it to `6214c93c`;
- it keeps the coordinated rav1d-safe revision (AVIF).

## The contract change

**Through `7cc3d3ec` and `d327434b`:** zencodec's jobs require `with_stop`, and `with_pulse` has a default.

**From `f73a7e0e` (2026-10-27), an ancestor of our core `6214c93c`:**
- `with_pulse` is required, and `with_stop` has the default;
- the animation methods take a per-call `Option<&dyn Pulse>`.

A native written against the older contract fails here in two ways:
- E0046: `with_pulse` is not implemented.
- E0053: `render_next_frame`, `push_frame`, `finish` and `step` have changed signatures.

CI run 40693652904 at `363f7c50` shows both.

## Native families needing standalone qualification (initial observation, with AVIF follow-up below)

| Native | Pin | Current heads | Why none fits this graph | Effect here |
|---|---|---|---|---|
| zenavif | `a7638f6e` | main `f6a6ec53`; draft PR #75 `b25f7604` | see below | avif features: 13 compile errors (CI) |
| heic | `ac5c7cb9` | main `a9ea9f67`; no open PR | main pins zencodec and zenmeta by git at `d327434b`, which predates the contract change | `heic-decode`: compile error (CI) |
| zenextras | `109a9ec3` | main `3b1aa3cb`; draft PR #53 `1af38ae8` (zenivf and zenwebm only) | no head migrates zenpdf or zentiff; the native lane is migrating them | `pdf-decode`: E0046 (CI); `tiff`: 2 errors (CI) |
| zenraw | `50f9c694` | main `9d4560a4` (24 commits ahead); no open PR; no branch targets the current core | see below | `raw-decode*`: cannot build (source-traced; CI never reached that step) |

**zenavif:**
- main names zencodec-io and zencodec-media by git at `7cc3d3ec`, before the contract change.
- PR #75 adopts the contract (`f73a7e0e`). Its git zencodec-io edge is a
  dev-dependency; its git zencodec-media edge is optional and enabled by media,
  which this consumer does not enable. Neither establishes a second core copy
  in the current zenpipe feature graph. The candidate moves rav1d-safe to
  `b19aec65`, off coordinated `dfd01e32`; see the source audit below.

**zenraw:**
- The pin implements `with_stop`, not `with_pulse`.
- main adds Pulse progress, but names zencodec and zenmeta by git at `d327434b`. That is 419 commits before
  `6214c93c` and before the contract change, and it would split the core family here.

## Migrated: pins already at their qualified heads

| Native | Pin | Relation to current heads |
|---|---|---|
| zenjpeg | `d099267c` | main |
| zengif | `efb7e43b` | PR #32 head |
| zenjxl | `a76ac76d` | PR #32 head |
| zenwebp | `5a6cce27` | PR #105 head; its native CI is green |
| zenbitmaps | `9ad4582b` | PR #34 head, 4 commits ahead of main |
| zenpng | `a2a9dc6d` | 23 commits ahead of main |
| ultrahdr-core | `61097504` | Pulse adapter branch, 8 commits ahead of main `b2dbf952` |

For zenpng, PR #47's head `b5e67cf3` is 7 commits past the pin and is not qualified for this graph.

## Second copies inside the graphs

Each of these comes from a native's own edge, not from zenpipe's, and none splits the core family.

In the root, `fuzz`, `demo/crate` and `zencodecs/fuzz` locks:
- zenyuv 0.1.3 from crates.io (zenwebp's edge), beside the zenyuv in zenjpeg `d099267c`.
- zenquant 0.1.4 from git (zengif's edge), beside the 0.1.3 from crates.io that zenpng uses. In the root lock,
  zenpipe and zenfilters use the 0.1.3 copy too.

In the root, `demo/crate` and `zencodecs/fuzz` locks:
- zenavif-serialize from zenavif `98c8a501` (zenravif's edge), beside zenavif `a7638f6e`.

In the root lock only:
- zenpredict from zenanalyze `05de3cbc` (zensim's edge), beside `6bbcacb1`.
- zenflate 0.3.6 from git (zensally-zentract's edge), beside 0.4.1.
- zenlayout 0.2.2 from crates.io (zensally's edge), beside the workspace's own zenlayout.

Unifying any of them needs the owning native's manifest, or a patch whose source is proven identical. Neither is done
here.


## AVIF candidate follow-up (2026-10-28, source audit; no consumer repin)

The initial CI failure and receipts above remain failures. At pin `a7638f6e`,
`src/codec/anim_decoder.rs:94` implements `render_next_frame` with
`Option<&dyn zencodec::enough::Stop>`; the core trait requires
`Option<&dyn zencodec::Pulse>`. The return type `At<CodecError>` is its associated
error and is not the mismatch. Similar old Stop signatures remain in animation
encoding and incremental step.

Open draft [PR75](https://github.com/imazen/zenavif/pull/75), immutable head
`b25f7604fbb71ead6517ec27fa52646770e3606a`, carries matching Pulse signatures.
Core `f552d338` pins both zenavif and zenmp4 to it; its
[native AV1 policy/guide gate](https://github.com/imazen/zencodec/actions/runs/40767258147/job/123095875607)
passes. This is a qualified harness candidate, not qualification of zenpipe's
complete standalone feature graph.

The PR's own standalone ultrahdr split is a different graph: its root patches
ultrahdr to `10a79c3a`. Zenpipe already patches ultrahdr-core to `61097504`, whose
core dependency is registry `>=0.2.0, <0.3.0`; dependency patch tables do not
propagate into consumers. Keep that distinction when resolving the follow-up.

A candidate consumer migration must move all five zenavif patch entries and
zencodecs' direct zenavif-parse edge together. The parser's source tree is
identical at the two revisions, but git source identities still differ.
`b25f7604` selects rav1d-safe `b19aec65`; coordinate that intentional decoder
change with the pin checker and unchanged native pixel oracles. Current
consumer features do not enable the candidate's media edge, and the existing
zenavif-serialize source is separately selected through zenravif. No automatic
zenmp4/serialize repin or feature addition follows from the harness.

Keep the frozen publication API run at its original pins. If that full API gate
confirms the remaining blocker, qualify the graph migration separately: inspect
all five resolved graphs for one core/pixel/how-far/parser source, then retain
the full API, AVIF and other native-family failures. No new build or graph
resolution was performed for this source audit.
