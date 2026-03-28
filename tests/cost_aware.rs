//! Tests for cost-aware format conversion (issue #5).
//!
//! Validates that the graph compiler uses `ideal_format()` / negotiate logic
//! to avoid unnecessary format conversions, and that conversion costs are
//! recorded in traces.

use hashbrown::HashMap;

use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph, SourceInfo};
use zenpipe::sources::CallbackSource;
use zenpipe::trace::TraceConfig;
use zenpipe::{Source, format};

/// Collect all strips from a source into a flat Vec<u8>.
fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.as_strided_bytes());
    }
    out
}

/// Create a solid-color RGBA8 sRGB source.
fn solid_source(width: u32, height: u32, pixel: [u8; 4]) -> Box<dyn Source> {
    let row_bytes = width as usize * 4;
    let mut rows_produced = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if rows_produced >= height {
                return Ok(false);
            }
            for px in buf[..row_bytes].chunks_exact_mut(4) {
                px.copy_from_slice(&pixel);
            }
            rows_produced += 1;
            Ok(true)
        },
    ))
}

/// Create a solid-color RGBA f32 linear source.
fn solid_f32_linear_source(width: u32, height: u32, pixel: [f32; 4]) -> Box<dyn Source> {
    let row_bytes = width as usize * 16; // 4 channels * 4 bytes/f32
    let mut rows_produced = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBAF32_LINEAR,
        16,
        move |buf| {
            if rows_produced >= height {
                return Ok(false);
            }
            let f32_slice: &mut [f32] = bytemuck::cast_slice_mut(&mut buf[..row_bytes]);
            for px in f32_slice.chunks_exact_mut(4) {
                px.copy_from_slice(&pixel);
            }
            rows_produced += 1;
            Ok(true)
        },
    ))
}

// ==========================================================================
// Test: f32 linear source + Resize avoids u8 sRGB roundtrip
// ==========================================================================

#[test]
fn resize_f32_linear_source_stays_f32() {
    // When the source is already f32 linear, Resize should use ResizeF32Source
    // instead of converting to u8 sRGB and back. The output format should
    // remain RGBAF32_LINEAR.
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize {
        w: 4,
        h: 4,
        filter: None,
        sharpen_percent: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_f32_linear_source(8, 8, [0.5, 0.3, 0.1, 1.0]));

    let mut pipeline = g.compile(sources).unwrap();
    // Output should be f32 linear (not u8 sRGB)
    assert_eq!(
        pipeline.format(),
        format::RGBAF32_LINEAR,
        "f32 linear source should stay f32 after resize"
    );
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    // Verify we got valid f32 pixel data (not garbage)
    for px in f32_data.chunks_exact(4) {
        assert!(px[0] >= 0.0 && px[0] <= 1.0, "R out of range: {}", px[0]);
        assert!(px[1] >= 0.0 && px[1] <= 1.0, "G out of range: {}", px[1]);
        assert!(px[2] >= 0.0 && px[2] <= 1.0, "B out of range: {}", px[2]);
        assert!(
            (px[3] - 1.0).abs() < 0.01,
            "A should be ~1.0, got {}",
            px[3]
        );
    }
}

#[test]
fn resize_f32_linear_traced_shows_no_conversion() {
    // The traced version should show NO implicit ConvertFormat entry
    // when the source is already f32 linear.
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize {
        w: 4,
        h: 4,
        filter: None,
        sharpen_percent: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_f32_linear_source(8, 8, [0.5, 0.3, 0.1, 1.0]));

    let config = TraceConfig::metadata_only();
    let (mut pipeline, trace) = g.compile_traced(sources, &config).unwrap();
    assert_eq!(pipeline.format(), format::RGBAF32_LINEAR);

    let _ = drain(pipeline.as_mut());

    let trace = trace.lock().unwrap();
    let implicit_conversions: Vec<_> = trace
        .entries
        .iter()
        .filter(|e| e.implicit && e.name == "ConvertFormat")
        .collect();
    assert!(
        implicit_conversions.is_empty(),
        "f32 linear source should not trigger implicit format conversion for Resize, \
         but found: {:?}",
        implicit_conversions
            .iter()
            .map(|e| &e.description)
            .collect::<Vec<_>>()
    );
}

// ==========================================================================
// Test: u8 sRGB source + Composite converts to f32 linear premul (required)
// ==========================================================================

#[test]
fn composite_always_converts_to_premul_linear() {
    // Composite MUST convert to RGBAF32_LINEAR_PREMUL regardless of source format.
    let mut g = PipelineGraph::new();
    let bg_src = g.add_node(NodeOp::Source);
    let fg_src = g.add_node(NodeOp::Source);
    let comp = g.add_node(NodeOp::Composite {
        fg_x: 0,
        fg_y: 0,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(fg_src, comp, EdgeKind::Input);
    g.add_edge(bg_src, comp, EdgeKind::Canvas);
    g.add_edge(comp, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(bg_src, solid_source(4, 4, [0, 0, 255, 255]));
    sources.insert(fg_src, solid_source(4, 4, [255, 0, 0, 128]));

    let config = TraceConfig::metadata_only();
    let (mut pipeline, trace) = g.compile_traced(sources, &config).unwrap();
    assert_eq!(pipeline.format(), format::RGBAF32_LINEAR_PREMUL);

    let _ = drain(pipeline.as_mut());

    // Verify implicit conversion was recorded with cost
    let trace = trace.lock().unwrap();
    let conversions: Vec<_> = trace
        .entries
        .iter()
        .filter(|e| e.implicit && e.name == "ConvertFormat")
        .collect();
    assert!(
        !conversions.is_empty(),
        "Composite should insert implicit conversion from u8 sRGB"
    );
    // At least one conversion should have cost recorded
    let has_cost = conversions.iter().any(|e| e.conversion_cost.is_some());
    assert!(
        has_cost,
        "implicit conversions should record ConversionCost"
    );
}

// ==========================================================================
// Test: two adjacent ops with same format don't double-convert
// ==========================================================================

#[test]
fn no_double_conversion_for_same_requirement() {
    // Source(u8 sRGB) → Resize → Resize: only ONE conversion should happen
    // (before the first Resize). The second Resize already has u8 input.
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize1 = g.add_node(NodeOp::Resize {
        w: 6,
        h: 6,
        filter: None,
        sharpen_percent: None,
    });
    let resize2 = g.add_node(NodeOp::Resize {
        w: 4,
        h: 4,
        filter: None,
        sharpen_percent: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize1, EdgeKind::Input);
    g.add_edge(resize1, resize2, EdgeKind::Input);
    g.add_edge(resize2, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(src, solid_source(8, 8, [200, 100, 50, 255]));

    let config = TraceConfig::metadata_only();
    let (mut pipeline, trace) = g.compile_traced(sources, &config).unwrap();
    assert_eq!(pipeline.format(), format::RGBA8_SRGB);

    let _ = drain(pipeline.as_mut());

    let trace = trace.lock().unwrap();
    let conversions: Vec<_> = trace
        .entries
        .iter()
        .filter(|e| e.implicit && e.name == "ConvertFormat")
        .collect();
    // Source is already RGBA8_SRGB, Resize wants RGBA8_SRGB → zero conversions
    assert_eq!(
        conversions.len(),
        0,
        "u8 sRGB source needs no conversion for Resize; got {} conversions: {:?}",
        conversions.len(),
        conversions
            .iter()
            .map(|e| &e.description)
            .collect::<Vec<_>>()
    );
}

// ==========================================================================
// Test: tracing records conversion cost with effort and loss
// ==========================================================================

#[test]
fn trace_records_conversion_cost() {
    // Force a conversion (u8 sRGB → f32 linear premul via Composite)
    // and verify the trace entry has conversion_cost with effort/loss.
    let mut g = PipelineGraph::new();
    let bg_src = g.add_node(NodeOp::Source);
    let fg_src = g.add_node(NodeOp::Source);
    let comp = g.add_node(NodeOp::Composite {
        fg_x: 0,
        fg_y: 0,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(fg_src, comp, EdgeKind::Input);
    g.add_edge(bg_src, comp, EdgeKind::Canvas);
    g.add_edge(comp, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(bg_src, solid_source(4, 4, [0, 0, 0, 255]));
    sources.insert(fg_src, solid_source(4, 4, [255, 255, 255, 255]));

    let config = TraceConfig::metadata_only();
    let (mut pipeline, trace) = g.compile_traced(sources, &config).unwrap();
    let _ = drain(pipeline.as_mut());

    let trace = trace.lock().unwrap();
    let conversion = trace
        .entries
        .iter()
        .find(|e| e.implicit && e.name == "ConvertFormat")
        .expect("should have at least one implicit conversion");

    let cost = conversion
        .conversion_cost
        .expect("ConvertFormat entry should have conversion_cost");
    // u8 sRGB → f32 linear premul has non-zero effort (transfer + depth change)
    assert!(
        cost.effort > 0,
        "conversion from u8 sRGB to f32 linear premul should have non-zero effort, got {}",
        cost.effort
    );
}

// ==========================================================================
// Test: estimate reflects f32 path for f32 linear source + Resize
// ==========================================================================

#[test]
fn estimate_reflects_f32_resize_format() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize {
        w: 4,
        h: 4,
        filter: None,
        sharpen_percent: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);

    let mut source_info = HashMap::new();
    // f32 linear source
    source_info.insert(
        src,
        SourceInfo {
            width: 8,
            height: 8,
            format: format::RGBAF32_LINEAR,
        },
    );

    let est = g.estimate(&source_info).unwrap();
    assert_eq!(
        est.output_format,
        format::RGBAF32_LINEAR,
        "estimate should reflect f32 output for f32 linear input"
    );
}

#[test]
fn estimate_reflects_u8_resize_format() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize {
        w: 4,
        h: 4,
        filter: None,
        sharpen_percent: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);

    let mut source_info = HashMap::new();
    // u8 sRGB source
    source_info.insert(
        src,
        SourceInfo {
            width: 8,
            height: 8,
            format: format::RGBA8_SRGB,
        },
    );

    let est = g.estimate(&source_info).unwrap();
    assert_eq!(
        est.output_format,
        format::RGBA8_SRGB,
        "estimate should reflect u8 output for u8 sRGB input"
    );
}

// ==========================================================================
// Test: conversion cost description includes effort/loss in trace text
// ==========================================================================

#[test]
fn trace_text_includes_cost_info() {
    let mut g = PipelineGraph::new();
    let bg_src = g.add_node(NodeOp::Source);
    let fg_src = g.add_node(NodeOp::Source);
    let comp = g.add_node(NodeOp::Composite {
        fg_x: 0,
        fg_y: 0,
        blend_mode: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(fg_src, comp, EdgeKind::Input);
    g.add_edge(bg_src, comp, EdgeKind::Canvas);
    g.add_edge(comp, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(bg_src, solid_source(4, 4, [0, 0, 0, 255]));
    sources.insert(fg_src, solid_source(4, 4, [255, 255, 255, 255]));

    let config = TraceConfig::metadata_only();
    let (mut pipeline, trace) = g.compile_traced(sources, &config).unwrap();
    let _ = drain(pipeline.as_mut());

    let trace = trace.lock().unwrap();
    let text = trace.to_text();

    // The trace text should include conversion cost info
    assert!(
        text.contains("effort=") && text.contains("loss="),
        "trace text should include effort/loss cost info:\n{text}"
    );
    assert!(
        text.contains("cost: effort="),
        "trace text should include cost line:\n{text}"
    );
}
