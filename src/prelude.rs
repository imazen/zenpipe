//! Crate-internal prelude: alloc re-exports + common imports.
//!
//! Use `use crate::prelude::*;` instead of individual `use alloc::{...}` imports.

pub use alloc::boxed::Box;
pub use alloc::string::String;
pub use alloc::string::ToString;
pub use alloc::vec;
pub use alloc::vec::Vec;
