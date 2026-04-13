//! `no_std`-compatible float math via `num-traits`.
//!
//! Re-exports the [`Float`] trait which provides `sin()`, `cos()`, `floor()`,
//! `ceil()`, `round()`, `abs()` etc. on both `f32` and `f64`. In `no_std`
//! mode, these are backed by `libm` (pure Rust, IEEE 754 compliant).
//! With `std`, they delegate to the standard library.

pub(crate) use num_traits::Float;
