//! Tests for wide-gamut and HDR pixel format support.
//!
//! Validates that P3, BT.2020, PQ, and HLG data flows through the pipeline
//! correctly — format tags are preserved, conversions insert when needed,
//! and gamut-agnostic operations don't corrupt wide-gamut data.

use hashbrown::HashMap;

use zenpipe::format;
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::ops::RowConverterOp;
use zenpipe::sources::{CallbackSource, CropSource, MaterializedSource, TransformSource};
use zenpipe::{
    AlphaMode, ChannelLayout, ChannelType, ColorPrimaries, PixelFormat, Source, TransferFunction,
};

/// Collect all strips from a source into a flat Vec<u8>.
fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.data());
    }
    out
}

// =========================================================================
// Format descriptor construction
// =========================================================================

/// Display P3 RGBA f32, linear light, straight alpha.
const P3_RGBAF32_LINEAR: PixelFormat = PixelFormat::new(
    ChannelType::F32,
    ChannelLayout::Rgba,
    Some(AlphaMode::Straight),
    TransferFunction::Linear,
)
.with_primaries(ColorPrimaries::DisplayP3);

/// BT.2020 RGBA f32, linear light, straight alpha.
const BT2020_RGBAF32_LINEAR: PixelFormat = PixelFormat::new(
    ChannelType::F32,
    ChannelLayout::Rgba,
    Some(AlphaMode::Straight),
    TransferFunction::Linear,
)
.with_primaries(ColorPrimaries::Bt2020);

/// BT.2020 RGBA f32, PQ transfer (HDR10).
const BT2020_RGBAF32_PQ: PixelFormat = PixelFormat::new(
    ChannelType::F32,
    ChannelLayout::Rgba,
    Some(AlphaMode::Straight),
    TransferFunction::Pq,
)
.with_primaries(ColorPrimaries::Bt2020);

/// Display P3 RGBA8, sRGB transfer.
const P3_RGBA8_SRGB: PixelFormat = PixelFormat::new(
    ChannelType::U8,
    ChannelLayout::Rgba,
    Some(AlphaMode::Straight),
    TransferFunction::Srgb,
)
.with_primaries(ColorPrimaries::DisplayP3);

/// Create a solid-color source with a given pixel format.
fn solid_f32_source(width: u32, height: u32, pixel: [f32; 4], fmt: PixelFormat) -> Box<dyn Source> {
    let row_floats = width as usize * 4;
    let row_bytes = row_floats * 4;
    let mut rows_produced = 0u32;
    Box::new(CallbackSource::new(width, height, fmt, 16, move |buf| {
        if rows_produced >= height {
            return Ok(false);
        }
        let f32_row: &mut [f32] = bytemuck::cast_slice_mut(&mut buf[..row_bytes]);
        for px in f32_row.chunks_exact_mut(4) {
            px.copy_from_slice(&pixel);
        }
        rows_produced += 1;
        Ok(true)
    }))
}

fn solid_u8_source(width: u32, height: u32, pixel: [u8; 4], fmt: PixelFormat) -> Box<dyn Source> {
    let row_bytes = width as usize * 4;
    let mut rows_produced = 0u32;
    Box::new(CallbackSource::new(width, height, fmt, 16, move |buf| {
        if rows_produced >= height {
            return Ok(false);
        }
        for px in buf[..row_bytes].chunks_exact_mut(4) {
            px.copy_from_slice(&pixel);
        }
        rows_produced += 1;
        Ok(true)
    }))
}

// =========================================================================
// P3 passthrough tests
// =========================================================================

#[test]
fn p3_linear_passthrough_crop() {
    // P3 linear f32 data through crop should preserve format and values exactly.
    let pixel = [0.8f32, 0.2, 0.5, 1.0];
    let src = solid_f32_source(8, 8, pixel, P3_RGBAF32_LINEAR);
    let mut crop = CropSource::new(src, 2, 2, 4, 4).unwrap();

    assert_eq!(crop.width(), 4);
    assert_eq!(crop.height(), 4);
    assert_eq!(crop.format(), P3_RGBAF32_LINEAR);

    let data = drain(&mut crop);
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    for px in f32_data.chunks_exact(4) {
        assert_eq!(px, pixel);
    }
}

#[test]
fn p3_linear_passthrough_materialize() {
    // Materializing P3 data should preserve format tag.
    let pixel = [0.3f32, 0.7, 0.1, 1.0];
    let src = solid_f32_source(4, 4, pixel, P3_RGBAF32_LINEAR);
    let mut mat = MaterializedSource::from_source(src).unwrap();

    assert_eq!(mat.format(), P3_RGBAF32_LINEAR);
    let data = drain(&mut mat);
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    for px in f32_data.chunks_exact(4) {
        assert_eq!(px, pixel);
    }
}

// =========================================================================
// BT.2020 passthrough tests
// =========================================================================

#[test]
fn bt2020_linear_passthrough_crop() {
    let pixel = [0.9f32, 0.1, 0.4, 1.0];
    let src = solid_f32_source(8, 8, pixel, BT2020_RGBAF32_LINEAR);
    let mut crop = CropSource::new(src, 0, 0, 4, 4).unwrap();

    assert_eq!(crop.format(), BT2020_RGBAF32_LINEAR);
    let data = drain(&mut crop);
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    for px in f32_data.chunks_exact(4) {
        assert_eq!(px, pixel);
    }
}

// =========================================================================
// Cross-gamut conversion tests
// =========================================================================

#[test]
fn srgb_to_p3_conversion() {
    // sRGB linear → P3 linear via RowConverterOp (gamut matrix).
    let op = RowConverterOp::new(format::RGBAF32_LINEAR, P3_RGBAF32_LINEAR);
    assert!(op.is_some(), "sRGB → P3 conversion should be supported");

    let pixel = [0.5f32, 0.5, 0.5, 1.0];
    let src = solid_f32_source(4, 2, pixel, format::RGBAF32_LINEAR);
    let mut transform = TransformSource::new(src).push(op.unwrap());

    assert_eq!(transform.format(), P3_RGBAF32_LINEAR);
    let data = drain(&mut transform);
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    // Neutral gray should map to neutral gray across gamuts.
    for px in f32_data.chunks_exact(4) {
        assert!(
            (px[0] - 0.5).abs() < 0.05,
            "R should be ~0.5, got {}",
            px[0]
        );
        assert!(
            (px[1] - 0.5).abs() < 0.05,
            "G should be ~0.5, got {}",
            px[1]
        );
        assert!(
            (px[2] - 0.5).abs() < 0.05,
            "B should be ~0.5, got {}",
            px[2]
        );
        assert!((px[3] - 1.0).abs() < 0.001);
    }
}

#[test]
fn p3_to_srgb_conversion() {
    let op = RowConverterOp::new(P3_RGBAF32_LINEAR, format::RGBAF32_LINEAR);
    assert!(op.is_some(), "P3 → sRGB conversion should be supported");
}

#[test]
fn srgb_to_bt2020_conversion() {
    let op = RowConverterOp::new(format::RGBAF32_LINEAR, BT2020_RGBAF32_LINEAR);
    assert!(
        op.is_some(),
        "sRGB → BT.2020 conversion should be supported"
    );
}

#[test]
fn p3_to_bt2020_conversion() {
    let op = RowConverterOp::new(P3_RGBAF32_LINEAR, BT2020_RGBAF32_LINEAR);
    assert!(op.is_some(), "P3 → BT.2020 conversion should be supported");
}

#[test]
fn srgb_to_p3_roundtrip() {
    // sRGB → P3 → sRGB should be near-identity for in-gamut colors.
    let to_p3 = RowConverterOp::must(format::RGBAF32_LINEAR, P3_RGBAF32_LINEAR);
    let to_srgb = RowConverterOp::must(P3_RGBAF32_LINEAR, format::RGBAF32_LINEAR);

    let pixel = [0.3f32, 0.6, 0.1, 0.8];
    let src = solid_f32_source(4, 2, pixel, format::RGBAF32_LINEAR);
    let mut transform = TransformSource::new(src).push(to_p3).push(to_srgb);

    assert_eq!(transform.format(), format::RGBAF32_LINEAR);
    let data = drain(&mut transform);
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    for px in f32_data.chunks_exact(4) {
        assert!(
            (px[0] - pixel[0]).abs() < 0.01,
            "R roundtrip: {} vs {}",
            px[0],
            pixel[0]
        );
        assert!(
            (px[1] - pixel[1]).abs() < 0.01,
            "G roundtrip: {} vs {}",
            px[1],
            pixel[1]
        );
        assert!(
            (px[2] - pixel[2]).abs() < 0.01,
            "B roundtrip: {} vs {}",
            px[2],
            pixel[2]
        );
        assert!(
            (px[3] - pixel[3]).abs() < 0.001,
            "A roundtrip: {} vs {}",
            px[3],
            pixel[3]
        );
    }
}

// =========================================================================
// Graph auto-conversion with wide gamut
// =========================================================================

#[test]
fn graph_auto_converts_p3_to_srgb_for_resize() {
    // P3 source → Resize node (requires RGBA8_SRGB) → should auto-convert.
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let resize = g.add_node(NodeOp::Resize {
        w: 2,
        h: 2,
        filter: None,
        sharpen_percent: None,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, resize, EdgeKind::Input);
    g.add_edge(resize, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    sources.insert(
        src,
        solid_u8_source(8, 8, [128, 64, 32, 255], P3_RGBA8_SRGB),
    );

    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!(pipeline.width(), 2);
    assert_eq!(pipeline.height(), 2);
    // Resize output is RGBA8_SRGB (BT.709) since the resizer is sRGB-internal.
    assert_eq!(pipeline.format(), format::RGBA8_SRGB);

    let data = drain(pipeline.as_mut());
    assert_eq!(data.len(), 2 * 2 * 4);
}

#[test]
fn graph_composite_mixed_gamut() {
    // P3 foreground over sRGB background — both auto-convert to linear premul.
    let mut g = PipelineGraph::new();
    let bg = g.add_node(NodeOp::Source);
    let fg = g.add_node(NodeOp::Source);
    let comp = g.add_node(NodeOp::Composite { fg_x: 0, fg_y: 0 });
    let out = g.add_node(NodeOp::Output);

    g.add_edge(bg, comp, EdgeKind::Canvas);
    g.add_edge(fg, comp, EdgeKind::Input);
    g.add_edge(comp, out, EdgeKind::Input);

    let mut sources = HashMap::new();
    // sRGB background
    sources.insert(
        bg,
        solid_f32_source(4, 4, [0.0, 0.0, 0.0, 1.0], format::RGBAF32_LINEAR),
    );
    // P3 foreground (opaque red)
    sources.insert(
        fg,
        solid_f32_source(4, 4, [1.0, 0.0, 0.0, 1.0], P3_RGBAF32_LINEAR),
    );

    let mut pipeline = g.compile(sources).unwrap();
    // Output is premul linear (BT.709 primaries since that's the default premul format)
    assert_eq!(pipeline.width(), 4);
    assert_eq!(pipeline.height(), 4);

    let data = drain(pipeline.as_mut());
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    // Opaque fg over black bg = fg value (converted to BT.709 premul linear)
    for px in f32_data.chunks_exact(4) {
        // P3 red (1,0,0) in BT.709 linear will have some green/blue due to gamut mapping
        // but R should still dominate
        assert!(px[0] > 0.5, "R should be significant, got {}", px[0]);
        assert!(px[3] > 0.99, "A should be ~1.0, got {}", px[3]);
    }
}

// =========================================================================
// HDR format support
// =========================================================================

#[test]
fn pq_format_passthrough() {
    // PQ data through crop preserves format tag.
    let pixel = [0.5f32, 0.3, 0.1, 1.0];
    let src = solid_f32_source(8, 8, pixel, BT2020_RGBAF32_PQ);
    let mut crop = CropSource::new(src, 2, 2, 4, 4).unwrap();

    assert_eq!(crop.format(), BT2020_RGBAF32_PQ);
    let data = drain(&mut crop);
    let f32_data: &[f32] = bytemuck::cast_slice(&data);
    for px in f32_data.chunks_exact(4) {
        assert_eq!(px, pixel);
    }
}

#[test]
fn pq_to_linear_conversion_exists() {
    // PQ → linear BT.2020 conversion should be available.
    let op = RowConverterOp::new(BT2020_RGBAF32_PQ, BT2020_RGBAF32_LINEAR);
    assert!(
        op.is_some(),
        "PQ → linear BT.2020 conversion should be supported"
    );
}

// =========================================================================
// 16-bit format support
// =========================================================================

#[test]
fn rgba16_source_through_pipeline() {
    // RGBA16 sRGB source through crop — validates non-8-bit channel support.
    let rgba16_srgb = PixelFormat::new(
        ChannelType::U16,
        ChannelLayout::Rgba,
        Some(AlphaMode::Straight),
        TransferFunction::Srgb,
    );

    let row_bytes = 4 * 8; // 4 pixels × 8 bytes/pixel (4 channels × 2 bytes)
    let mut rows_produced = 0u32;
    let src: Box<dyn Source> = Box::new(CallbackSource::new(4, 4, rgba16_srgb, 16, move |buf| {
        if rows_produced >= 4 {
            return Ok(false);
        }
        // Fill with mid-gray: 32768 = 0x8000
        for chunk in buf[..row_bytes].chunks_exact_mut(2) {
            chunk.copy_from_slice(&32768u16.to_ne_bytes());
        }
        rows_produced += 1;
        Ok(true)
    }));

    let mut crop = CropSource::new(src, 1, 1, 2, 2).unwrap();
    assert_eq!(crop.format(), rgba16_srgb);
    assert_eq!(crop.width(), 2);
    assert_eq!(crop.height(), 2);

    let data = drain(&mut crop);
    assert_eq!(data.len(), 2 * 2 * 8); // 2×2 pixels × 8 bytes/pixel
}

// =========================================================================
// Grayscale support
// =========================================================================

#[test]
fn gray_to_rgba_conversion() {
    // Gray8 sRGB → RGBA8 sRGB conversion should be supported.
    let gray8_srgb = PixelFormat::new(
        ChannelType::U8,
        ChannelLayout::Gray,
        None,
        TransferFunction::Srgb,
    );

    let op = RowConverterOp::new(gray8_srgb, format::RGBA8_SRGB);
    assert!(op.is_some(), "Gray8 → RGBA8 conversion should be supported");
}
