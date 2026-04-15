//! Document detection, deskew, and perspective correction.
//!
//! This module provides classical (non-ML) algorithms for document scanning:
//!
//! - **LSD** — Line Segment Detector for finding edges and boundaries
//! - **Otsu** — Optimal binarization threshold for text/background separation
//! - **Quad detection** — Find the 4 corners of a document in an image
//! - **Homography** — Compute perspective correction from detected corners
//! - **Deskew** — Detect and correct text rotation angle
//!
//! # Pipeline
//!
//! ```text
//! L plane → LSD segments → quad detection → homography → Warp::projective()
//!                                              ↑
//!                     Otsu binarize → projection profile → deskew angle → Warp::deskew()
//! ```
//!
//! These functions operate on the L (lightness) channel of Oklab planes.
//! Use them with [`FilterContext`](crate::FilterContext) for zero-allocation
//! scratch buffers.

pub mod deskew;
pub mod homography;
pub mod lsd;
pub mod otsu;
pub mod quad;

pub use deskew::detect_skew_angle;
pub use homography::{compute_homography, rectify_quad};
pub use lsd::{LineSegment, detect_line_segments};
pub use otsu::{binarize, otsu_threshold};
pub use quad::{DocumentQuad, find_document_quad, score_quad};
