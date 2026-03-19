//! Crate-internal prelude: alloc re-exports + common imports.
//!
//! instead of individual `use alloc::{...}` imports.

pub use alloc::boxed::Box;
pub use alloc::string::String;
pub use alloc::sync::Arc;
pub use alloc::vec;
pub use alloc::vec::Vec;
