//! Tests for fan-out (TeeSource / TeeCursor) — one source feeding multiple outputs.

use hashbrown::HashMap;

use zenpipe::Source;
use zenpipe::format;
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::{CallbackSource, CropSource, TeeSource};

/// Collect all strips from a source into a flat Vec<u8>.
fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.as_strided_bytes());
    }
    out
}

/// Create a gradient source where pixel[0] = x coord, pixel[1] = y coord.
fn gradient_source(width: u32, height: u32) -> Box<dyn Source> {
    let mut row_idx = 0u32;
    Box::new(CallbackSource::new(
        width,
        height,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if row_idx >= height {
                return Ok(false);
            }
            for x in 0..width as usize {
                let i = x * 4;
                buf[i] = x as u8;
                buf[i + 1] = row_idx as u8;
                buf[i + 2] = 42;
                buf[i + 3] = 255;
            }
            row_idx += 1;
            Ok(true)
        },
    ))
}

/// Create a solid-color RGBA8 source.
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

// =========================================================================
// Basic TeeSource / TeeCursor tests
// =========================================================================

#[test]
fn tee_basic_single_cursor() {
    let src = solid_source(4, 4, [100, 200, 50, 255]);
    let tee = TeeSource::new(src).unwrap();

    assert_eq!(tee.width(), 4);
    assert_eq!(tee.height(), 4);
    assert_eq!(tee.format(), format::RGBA8_SRGB);

    let mut cursor = tee.cursor();
    let data = drain(&mut cursor);
    assert_eq!(data.len(), 4 * 4 * 4);
    for px in data.chunks_exact(4) {
        assert_eq!(px, [100, 200, 50, 255]);
    }
}

#[test]
fn tee_two_cursors_identical() {
    let src = gradient_source(8, 8);
    let tee = TeeSource::new(src).unwrap();

    let mut cursor_a = tee.cursor();
    let mut cursor_b = tee.cursor();

    let data_a = drain(&mut cursor_a);
    let data_b = drain(&mut cursor_b);

    assert_eq!(data_a.len(), data_b.len());
    assert_eq!(data_a, data_b);
    assert_eq!(data_a.len(), 8 * 8 * 4);
}

#[test]
fn tee_many_cursors() {
    let src = solid_source(4, 4, [42, 42, 42, 255]);
    let tee = TeeSource::new(src).unwrap();

    // 10 cursors all reading the same data
    let results: Vec<Vec<u8>> = (0..10)
        .map(|_| {
            let mut c = tee.cursor();
            drain(&mut c)
        })
        .collect();

    for (i, data) in results.iter().enumerate() {
        assert_eq!(data.len(), 4 * 4 * 4, "cursor {i} wrong length");
        assert_eq!(&results[0], data, "cursor {i} data differs");
    }
}

#[test]
fn tee_cursor_strip_height() {
    let src = solid_source(4, 10, [1, 2, 3, 255]);
    let tee = TeeSource::new(src).unwrap();

    // Strip height 3: should produce strips of 3, 3, 3, 1
    let mut cursor = tee.cursor_with_strip_height(3);
    let mut strip_heights = Vec::new();
    while let Ok(Some(strip)) = cursor.next() {
        strip_heights.push(strip.rows());
    }
    assert_eq!(strip_heights, vec![3, 3, 3, 1]);
}

#[test]
fn tee_cursor_y_offsets() {
    let src = solid_source(4, 8, [0, 0, 0, 255]);
    let tee = TeeSource::new(src).unwrap();

    let mut cursor = tee.cursor(); // strip_height = 16, but image is 8 rows
    let strip = cursor.next().unwrap().unwrap();
    assert_eq!(strip.rows(), 8); // single strip covers the whole image

    let next = cursor.next().unwrap();
    assert!(next.is_none());
}

// =========================================================================
// Fan-out with different downstream pipelines
// =========================================================================

#[test]
fn tee_different_crops() {
    // One source, two different crop regions.
    let src = gradient_source(8, 8);
    let tee = TeeSource::new(src).unwrap();

    // Crop top-left 4×4
    let cursor_a = tee.cursor();
    let mut crop_a = CropSource::new(Box::new(cursor_a), 0, 0, 4, 4).unwrap();

    // Crop bottom-right 4×4
    let cursor_b = tee.cursor();
    let mut crop_b = CropSource::new(Box::new(cursor_b), 4, 4, 4, 4).unwrap();

    let data_a = drain(&mut crop_a);
    let data_b = drain(&mut crop_b);

    assert_eq!(data_a.len(), 4 * 4 * 4);
    assert_eq!(data_b.len(), 4 * 4 * 4);

    // Top-left crop: pixel (0,0) should have x=0, y=0
    assert_eq!(data_a[0], 0); // x=0
    assert_eq!(data_a[1], 0); // y=0

    // Bottom-right crop: pixel (0,0) in crop = (4,4) in original
    assert_eq!(data_b[0], 4); // x=4
    assert_eq!(data_b[1], 4); // y=4
}

#[test]
fn tee_cursor_into_graph() {
    // Two graph pipelines fed from the same TeeSource.
    let src = solid_source(8, 8, [128, 64, 32, 255]);
    let tee = TeeSource::new(src).unwrap();

    // Graph A: resize to 4×4
    let mut g_a = PipelineGraph::new();
    let src_a = g_a.add_node(NodeOp::Source);
    let resize_a = g_a.add_node(NodeOp::Resize {
        w: 4,
        h: 4,
        filter: None,
        sharpen_percent: None,
    });
    let out_a = g_a.add_node(NodeOp::Output);
    g_a.add_edge(src_a, resize_a, EdgeKind::Input);
    g_a.add_edge(resize_a, out_a, EdgeKind::Input);

    let mut sources_a = HashMap::new();
    sources_a.insert(src_a, Box::new(tee.cursor()) as Box<dyn Source>);
    let mut pipeline_a = g_a.compile(sources_a).unwrap();

    // Graph B: resize to 2×2
    let mut g_b = PipelineGraph::new();
    let src_b = g_b.add_node(NodeOp::Source);
    let resize_b = g_b.add_node(NodeOp::Resize {
        w: 2,
        h: 2,
        filter: None,
        sharpen_percent: None,
    });
    let out_b = g_b.add_node(NodeOp::Output);
    g_b.add_edge(src_b, resize_b, EdgeKind::Input);
    g_b.add_edge(resize_b, out_b, EdgeKind::Input);

    let mut sources_b = HashMap::new();
    sources_b.insert(src_b, Box::new(tee.cursor()) as Box<dyn Source>);
    let mut pipeline_b = g_b.compile(sources_b).unwrap();

    // Both pipelines produce output
    assert_eq!(pipeline_a.width(), 4);
    assert_eq!(pipeline_a.height(), 4);
    assert_eq!(pipeline_b.width(), 2);
    assert_eq!(pipeline_b.height(), 2);

    let data_a = drain(pipeline_a.as_mut());
    let data_b = drain(pipeline_b.as_mut());

    assert_eq!(data_a.len(), 4 * 4 * 4);
    assert_eq!(data_b.len(), 2 * 2 * 4);

    // Both should be roughly the same color (solid input)
    for px in data_a.chunks_exact(4) {
        assert!((px[0] as i16 - 128).unsigned_abs() <= 2);
        assert!((px[1] as i16 - 64).unsigned_abs() <= 2);
        assert!((px[2] as i16 - 32).unsigned_abs() <= 2);
    }
    for px in data_b.chunks_exact(4) {
        assert!((px[0] as i16 - 128).unsigned_abs() <= 2);
        assert!((px[1] as i16 - 64).unsigned_abs() <= 2);
        assert!((px[2] as i16 - 32).unsigned_abs() <= 2);
    }
}

// =========================================================================
// Edge cases
// =========================================================================

#[test]
fn tee_1x1_image() {
    let src = solid_source(1, 1, [255, 0, 0, 255]);
    let tee = TeeSource::new(src).unwrap();

    let mut cursor = tee.cursor();
    let data = drain(&mut cursor);
    assert_eq!(data, [255, 0, 0, 255]);
}

#[test]
fn tee_cursor_exhausted_returns_none() {
    let src = solid_source(2, 2, [0, 0, 0, 255]);
    let tee = TeeSource::new(src).unwrap();
    let mut cursor = tee.cursor();

    // Drain everything
    drain(&mut cursor);

    // Subsequent calls return None
    assert!(cursor.next().unwrap().is_none());
    assert!(cursor.next().unwrap().is_none());
}

#[test]
fn tee_preserves_format() {
    // Wide-gamut format should be preserved through tee.
    let p3_linear = zenpipe::PixelFormat::new(
        zenpipe::ChannelType::F32,
        zenpipe::ChannelLayout::Rgba,
        Some(zenpipe::AlphaMode::Straight),
        zenpipe::TransferFunction::Linear,
    )
    .with_primaries(zenpipe::ColorPrimaries::DisplayP3);

    let row_bytes = 4 * 16; // 4 pixels × 16 bytes (RGBA f32)
    let mut rows_produced = 0u32;
    let src: Box<dyn Source> = Box::new(CallbackSource::new(4, 4, p3_linear, 16, move |buf| {
        if rows_produced >= 4 {
            return Ok(false);
        }
        let f32_row: &mut [f32] = bytemuck::cast_slice_mut(&mut buf[..row_bytes]);
        for px in f32_row.chunks_exact_mut(4) {
            px.copy_from_slice(&[0.5f32, 0.3, 0.1, 1.0]);
        }
        rows_produced += 1;
        Ok(true)
    }));

    let tee = TeeSource::new(src).unwrap();
    assert_eq!(tee.format(), p3_linear);

    let mut cursor = tee.cursor();
    assert_eq!(cursor.format(), p3_linear);

    let data = drain(&mut cursor);
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    for px in f32_data.chunks_exact(4) {
        assert_eq!(px, [0.5f32, 0.3, 0.1, 1.0]);
    }
}
