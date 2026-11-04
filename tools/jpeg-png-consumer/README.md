# Frozen JPEG/PNG consumer graph

The dependency recipe for zencodec's checked JPEG/PNG guide (the site's
`checked_registry`, `checked_default`, `checked_web`, `checked_file` and
`checked_async` examples). `src/guide.rs` is the guide, copied verbatim from
zencodec `docs/examples/checked_operations.rs` at the pinned revision.

```sh
cargo test --locked
cargo tree --locked -i zencodec   # exactly one zencodec
```

Outside this repository, keep the manifest and `Cargo.lock` as they are. The
`[patch.crates-io]` block is part of the recipe: patches apply only from the
top-level workspace, so a consumer of zencodecs repeats them. Every pin is a
review revision, not a release.
