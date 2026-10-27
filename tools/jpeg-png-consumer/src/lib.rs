//! A frozen consumer graph for zencodec's checked JPEG/PNG guide.
//!
//! Every crate resolves from pinned git revisions or crates.io: no zenpipe
//! workspace membership and no sibling-checkout paths. `tests/guide.rs`
//! compiles the guide verbatim (`src/guide.rs`) and runs its recipes.
#![forbid(unsafe_code)]
