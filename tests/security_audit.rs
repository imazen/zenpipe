//! Regression tests for the 2026-05-06 security audit.
//!
//! Each test corresponds to a CRITICAL or HIGH finding. Failures here mean
//! a previously-fixed defense was reintroduced.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::limits::{
    AllocationTracker, checked_buffer_size, checked_dim_add, checked_stride_buffer,
};
use zenpipe::sources::{CropSource, ExpandCanvasSource, MaterializedSource};
use zenpipe::{PipeError, format};

// -----------------------------------------------------------------------
// C1: dfs_cycle_check must not blow the stack on a deep linear DAG.
// -----------------------------------------------------------------------

#[test]
fn audit_c1_deep_linear_dag_does_not_stack_overflow() {
    // A linear chain of N RowConverter nodes used to recurse N levels deep.
    // 50_000 frames is comfortably above the typical OS stack budget but
    // still terminates quickly via the iterative cycle-check.
    const N: usize = 50_000;
    let mut g = PipelineGraph::new();

    // Source node first.
    let src = g.add_node(NodeOp::Source);
    let mut prev = src;
    for _ in 0..N {
        // Use FillRect as a pass-through node; any non-Source node works.
        let id = g.add_node(NodeOp::FillRect {
            x1: 0,
            y1: 0,
            x2: 1,
            y2: 1,
            color: [0, 0, 0, 0],
        });
        g.add_edge(prev, id, EdgeKind::Input);
        prev = id;
    }
    let out = g.add_node(NodeOp::Output);
    g.add_edge(prev, out, EdgeKind::Input);

    // validate() runs the cycle check. Should error out via MAX_GRAPH_DEPTH
    // or succeed cleanly; either way it must NOT abort the process.
    let res = g.validate();
    // The graph is acyclic, so a too-deep complaint is acceptable; what
    // matters is that we returned at all instead of stack-overflowing.
    let _ = res;
}

// -----------------------------------------------------------------------
// C2: checked_buffer_size / checked_stride_buffer reject overflow.
// -----------------------------------------------------------------------

#[test]
fn audit_c2_checked_buffer_size_rejects_overflow() {
    // On 64-bit the wrap is harder to hit, but 2^63 * 4 saturates usize on
    // both 32-bit and 64-bit.
    let huge = u32::MAX;
    let res = checked_buffer_size(huge, huge, 4);
    assert!(matches!(
        res.as_ref().map_err(|e| e.error()),
        Err(PipeError::LimitExceeded(_))
    ));
}

#[test]
fn audit_c2_checked_stride_buffer_rejects_overflow() {
    let res = checked_stride_buffer(usize::MAX / 2, u32::MAX);
    assert!(matches!(
        res.as_ref().map_err(|e| e.error()),
        Err(PipeError::LimitExceeded(_))
    ));
}

#[test]
fn audit_c2_checked_dim_add_rejects_wrap() {
    assert!(checked_dim_add(u32::MAX, 1).is_err());
    assert_eq!(checked_dim_add(100, 200).unwrap(), 300);
}

// -----------------------------------------------------------------------
// H1: AllocationTracker must enforce limit under concurrency.
// -----------------------------------------------------------------------

#[test]
fn audit_h1_allocation_tracker_enforces_limit_concurrently() {
    let limit_bytes = 1024 * 1024; // 1 MB
    let tracker = Arc::new(AllocationTracker::new(limit_bytes));

    // Each thread tries to grab `limit_bytes / 2 + 1` bytes. With a TOCTOU
    // race-prone implementation, both could pass the check and exceed the
    // limit by ~limit_bytes/2. With the compare_exchange loop, at most one
    // wins and the other surfaces a LimitExceeded error.
    let total_grabbed = Arc::new(AtomicU64::new(0));
    let mut handles = vec![];
    for _ in 0..16 {
        let t = Arc::clone(&tracker);
        let g = Arc::clone(&total_grabbed);
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                if let Ok(_guard) = t.allocate(limit_bytes / 2 + 1) {
                    g.fetch_add(limit_bytes / 2 + 1, Ordering::SeqCst);
                    // hold briefly
                    std::thread::yield_now();
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // The tracker may briefly hit but must never EXCEED the limit at any
    // point. Peak should be <= limit_bytes.
    assert!(
        tracker.peak_bytes() <= limit_bytes,
        "peak {} exceeds limit {}",
        tracker.peak_bytes(),
        limit_bytes
    );
}

#[test]
fn audit_h1_allocation_tracker_rejects_u64_overflow() {
    let tracker = Arc::new(AllocationTracker::new(1024));
    // Ask for u64::MAX. checked_add(current, u64::MAX) overflows -> error.
    assert!(tracker.allocate(u64::MAX).is_err());
}

// -----------------------------------------------------------------------
// H2: CropSource must reject u32 wrap on x+w / y+h.
// -----------------------------------------------------------------------

#[test]
fn audit_h2_crop_rejects_x_plus_w_wrap() {
    let src = MaterializedSource::from_data(vec![0u8; 4 * 4], 4, 4, format::RGBA8_SRGB);
    let res = CropSource::new(Box::new(src), u32::MAX, 0, 1, 1);
    assert!(matches!(
        res.as_ref().map_err(|e| e.error()),
        Err(PipeError::DimensionMismatch(_))
    ));
}

#[test]
fn audit_h2_crop_rejects_y_plus_h_wrap() {
    let src = MaterializedSource::from_data(vec![0u8; 4 * 4], 4, 4, format::RGBA8_SRGB);
    let res = CropSource::new(Box::new(src), 0, u32::MAX, 1, 1);
    assert!(matches!(
        res.as_ref().map_err(|e| e.error()),
        Err(PipeError::DimensionMismatch(_))
    ));
}

// -----------------------------------------------------------------------
// H3: ExpandCanvas i32::MIN place_x must use unsigned_abs, not negation.
// -----------------------------------------------------------------------

#[test]
fn audit_h3_expand_canvas_accepts_i32_min_place() {
    // Construction with place_x = i32::MIN previously wrapped through
    // `(-place_x) as u32` and produced skip_x = 0x8000_0000.
    // Now uses unsigned_abs and should produce skip_x = 2^31 cleanly.
    let src = MaterializedSource::from_data(vec![0u8; 4 * 4], 4, 4, format::RGBA8_SRGB);
    let res = ExpandCanvasSource::new(Box::new(src), 8, 8, i32::MIN, 0, [0, 0, 0, 0]);
    // Construction succeeds (saturating_sub gives content_w = 0). The key
    // is that we don't panic from the negation.
    assert!(res.is_ok());
}

// -----------------------------------------------------------------------
// H7: ExpandCanvas canvas dims with u32 wrap are rejected at compile.
// -----------------------------------------------------------------------

// Covered indirectly by audit_c2_checked_dim_add_rejects_wrap and the
// graph compile path that wraps the addition in checked_add — exercised
// via the existing graph fuzz tests.
