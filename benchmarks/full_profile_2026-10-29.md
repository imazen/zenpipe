# Full zencodecs profile and nested RAW gain-map controls

Source `3ae7877066826c656d0a22ffa19b993d304bd55f`. Portable roots select
HEIC `f97f1a82` and RAW `e2c2be83`; core remains `feb09f9f`. Root package
resolution changes only heic/heic-core/zenraw. The other four graph roots change
unused patch bookkeeping only. Both source/family validators pass.

```sh
just api-profile-check
just raw-gainmap-controls
```

The complete API profile (`all,jp2-decode,metadata,checked-transcode-async`)
compiles and passes strict library Clippy. No codec feature was excluded to
obtain this result. Generated API snapshots are still a separate gate.

The former embedded RAW gain-map JPEG call passed None where an owned Pulse
was required. It now receives the operation's retained control and limits;
private extraction returns Result<Option<_>> so nested decode failures propagate.
The focused control encodes a real 4x4 JPEG and verifies ordinary dimensions,
exact timeout category through the native cause chain, pixel-limit refusal,
and malformed-JPEG failure. It does not exercise a complete Apple DNG container
or claim a combined allocation bound across all extraction/conversion stages.

Receipt: `run-heavy: done rc=0 4s | peak-RSS 0.70GiB | min-avail 14875MiB | peak-load 0.74`.
Prior runs preserve the stale None type error, one unmigrated direct fixture call,
and a documentation-list Clippy error. No behavioral assertions were weakened.
The two existing AVIF encode-orientation carrier failures remain unresolved.

## Generated API surface

Both `just api-doc` and `just api-doc-check` pass in an isolated export of
`f6f8bf75`, with the complete configured feature profile. Generated public and
internal snapshots are copied back verbatim. Receipt: `run-heavy: done rc=0 83s | peak-RSS 0.87GiB | min-avail 14057MiB | peak-load 1.49`.
This qualifies generation and freshness, not platform runtime behavior.
