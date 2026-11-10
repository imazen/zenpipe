//! Hidden working formats answer to the precision rule, at `compile` and `estimate` alike.

use hashbrown::HashMap;
use whereat::at;
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::{CallbackSource, MaterializedSource};
use zenpipe::{PipeError, PixelFormat, Source, SourceInfo};
use zenpixels::PixelDescriptor as D;
use zenpixels::precision::{
    AlphaContent, ConversionDecision, ConversionPolicy, ConversionPreset, ConversionQuestion,
    DecidedBy, SourcePrecision,
};

whereat::define_at_crate_info!();

fn resize_graph(then_constrain_to_eight: bool) -> (PipelineGraph, usize) {
    let mut graph = PipelineGraph::new();
    let source = graph.add_node(NodeOp::Source);
    let resize = graph.add_node(NodeOp::Resize {
        w: 2,
        h: 2,
        filter: None,
        sharpen_percent: None,
    });
    graph.add_edge(source, resize, EdgeKind::Input);
    let mut last = resize;
    if then_constrain_to_eight {
        let orient = graph.add_node(NodeOp::Orient(zenlayout::Orientation::Rotate180));
        graph.add_edge(last, orient, EdgeKind::Input);
        last = orient;
    }
    let output = graph.add_node(NodeOp::Output);
    graph.add_edge(last, output, EdgeKind::Input);
    (graph, source)
}

fn info(format: PixelFormat) -> SourceInfo {
    SourceInfo {
        width: 4,
        height: 4,
        format,
    }
}

fn rgba16_source(values: [u16; 4]) -> Box<dyn Source> {
    let mut rows = 0;
    Box::new(CallbackSource::new(
        4,
        4,
        D::RGBA16_SRGB,
        16,
        move |buf: &mut [u8]| {
            if rows >= 4 {
                return Ok(false);
            }
            for pixel in buf[..32].chunks_exact_mut(8) {
                for (sample, value) in pixel.chunks_exact_mut(2).zip(values) {
                    sample.copy_from_slice(&value.to_ne_bytes());
                }
            }
            rows += 1;
            Ok(true)
        },
    ))
}

fn opaque_real_16_bit() -> SourcePrecision {
    SourcePrecision::from_storage(D::RGBA16_SRGB).with_alpha(AlphaContent::Opaque)
}

#[test]
fn an_8bit_source_resizes_in_8bit_as_it_always_did() {
    let (graph, source) = resize_graph(true);
    let infos = HashMap::from([(source, info(D::RGBA8_SRGB))]);
    let plan = graph.working_format_plan(&infos).unwrap().unwrap();
    assert_eq!(plan.steps[0].output, D::RGBA8_SRGB);
    assert!(plan.steps.iter().all(|s| s.effect.is_lossless()));
    let estimate = graph.estimate(&infos).unwrap();
    assert_eq!(estimate.output_format, D::RGBA8_SRGB);
}

#[test]
fn a_real_16bit_source_resizes_in_f32_and_compile_and_estimate_agree() {
    let (graph, source) = resize_graph(false);
    let graph = graph.with_source_precision(source, opaque_real_16_bit());
    let infos = HashMap::from([(source, info(D::RGBA16_SRGB))]);
    let plan = graph.working_format_plan(&infos).unwrap().unwrap();
    assert_eq!(plan.steps[0].output, D::RGBAF32_LINEAR);
    assert!(plan.steps[0].effect.is_lossless());
    let estimate = graph.estimate(&infos).unwrap();
    assert_eq!(estimate.output_format, D::RGBAF32_LINEAR);

    let compiled = graph
        .compile(HashMap::from([(source, rgba16_source([1, 2, 3, 65535]))]))
        .unwrap();
    assert_eq!(compiled.format(), D::RGBAF32_LINEAR);
}

#[test]
fn the_f32_resize_keeps_a_value_eight_bits_would_round() {
    // 1234 isn't on the 8-bit grid: 8 bits would return 1285. A flat image stays flat in a resize.
    let (graph, source) = resize_graph(false);
    let graph = graph.with_source_precision(source, opaque_real_16_bit());
    let compiled = graph
        .compile(HashMap::from([(
            source,
            rgba16_source([1234, 1234, 1234, 65535]),
        )]))
        .unwrap();
    let frame = MaterializedSource::from_source(compiled).unwrap();
    let mut back = [0u8; 8];
    zenpixels_convert::RowConverter::new(D::RGBAF32_LINEAR, D::RGBA16_SRGB)
        .unwrap()
        .try_convert_row(&frame.data()[..16], &mut back, 1)
        .unwrap();
    let red = u16::from_ne_bytes([back[0], back[1]]);
    assert!(red.abs_diff(1234) <= 1, "came back as {red}");

    // The named narrowing shows what that kept.
    let (graph, source) = resize_graph(false);
    let graph = graph
        .with_source_precision(source, opaque_real_16_bit())
        .with_conversion_policy(
            ConversionPolicy::new().with_preset(ConversionPreset::DropPrecisionForMemory),
        );
    let compiled = graph
        .compile(HashMap::from([(
            source,
            rgba16_source([1234, 1234, 1234, 65535]),
        )]))
        .unwrap();
    assert_eq!(compiled.format(), D::RGBA8_SRGB);
    let frame = MaterializedSource::from_source(compiled).unwrap();
    assert_eq!(
        frame.data()[0],
        5,
        "1234 of 65535 rounds to 5 of 255, which is 1285"
    );
}

#[test]
fn a_real_16bit_source_with_no_deeper_path_refuses_before_any_pixels() {
    let (graph, source) = resize_graph(true);
    let graph = graph.with_source_precision(source, opaque_real_16_bit());
    let infos = HashMap::from([(source, info(D::RGBA16_SRGB))]);
    let refusal = graph.estimate(&infos).unwrap_err().to_string();
    assert!(refusal.contains("16 significant bits"), "{refusal}");

    let refusal = graph
        .compile(HashMap::from([(source, rgba16_source([1, 2, 3, 65535]))]))
        .err()
        .unwrap()
        .to_string();
    assert!(refusal.contains("16 significant bits"), "{refusal}");
}

#[test]
fn a_preset_that_says_so_narrows_and_the_plan_records_it() {
    let (graph, source) = resize_graph(true);
    let graph = graph
        .with_source_precision(source, opaque_real_16_bit())
        .with_conversion_policy(
            ConversionPolicy::new().with_preset(ConversionPreset::DropPrecisionForMemory),
        );
    let infos = HashMap::from([(source, info(D::RGBA16_SRGB))]);
    let plan = graph.working_format_plan(&infos).unwrap().unwrap();
    assert_eq!(plan.steps[0].output, D::RGBA8_SRGB);
    assert_eq!(plan.steps[1].effect.bits_dropped, 8);
    assert!(
        graph
            .compile(HashMap::from([(source, rgba16_source([1, 2, 3, 65535]))]))
            .is_ok()
    );
}

#[test]
fn a_resolver_chooses_and_can_refuse_with_its_own_error() {
    let (graph, source) = resize_graph(false);
    let graph = graph
        .with_source_precision(source, opaque_real_16_bit())
        .with_conversion_resolver(|q: ConversionQuestion<'_>| {
            Ok(ConversionDecision::Choose(
                q.admitted().next().unwrap().index(),
            ))
        });
    let infos = HashMap::from([(source, info(D::RGBA16_SRGB))]);
    let plan = graph.working_format_plan(&infos).unwrap().unwrap();
    assert_eq!(plan.steps[0].decided_by, DecidedBy::Resolver);
    assert_eq!(plan.steps[0].output, D::RGBA8_SRGB);

    let (graph, source) = resize_graph(false);
    let graph = graph.with_conversion_resolver(|_: ConversionQuestion<'_>| {
        Err(at!(PipeError::Op("tenant rule".into())))
    });
    let infos = HashMap::from([(source, info(D::RGBA8_SRGB))]);
    assert!(
        graph
            .estimate(&infos)
            .unwrap_err()
            .to_string()
            .contains("tenant rule")
    );
}

#[test]
fn a_floor_binds_a_resolver() {
    let (graph, source) = resize_graph(false);
    let graph = graph
        .with_source_precision(source, opaque_real_16_bit())
        .with_conversion_policy(ConversionPolicy::new().with_min_bits(12))
        .with_conversion_resolver(|q: ConversionQuestion<'_>| {
            Ok(ConversionDecision::Choose(
                q.admitted().next().unwrap().index(),
            ))
        });
    let infos = HashMap::from([(source, info(D::RGBA16_SRGB))]);
    let plan = graph.working_format_plan(&infos).unwrap().unwrap();
    assert_eq!(plan.steps[0].output, D::RGBAF32_LINEAR);
}

#[test]
fn a_scan_clears_8bit_content_in_16bit_storage() {
    let scanned = |values| {
        MaterializedSource::from_source(rgba16_source(values))
            .unwrap()
            .scan_source_precision(None)
            .unwrap()
    };
    let eight = scanned([257, 514, 771, 65535]);
    assert_eq!(eight.significant_bits(), 8);

    let (graph, source) = resize_graph(true);
    let graph = graph.with_source_precision(source, eight);
    let infos = HashMap::from([(source, info(D::RGBA16_SRGB))]);
    let plan = graph.working_format_plan(&infos).unwrap().unwrap();
    assert!(plan.steps.iter().all(|s| s.effect.is_lossless()));

    assert_eq!(scanned([1, 2, 3, 65535]).significant_bits(), 16);
}

#[test]
fn a_composite_carries_the_deeper_of_its_sources() {
    let mut graph = PipelineGraph::new();
    let fg = graph.add_node(NodeOp::Source);
    let bg = graph.add_node(NodeOp::Source);
    let composite = graph.add_node(NodeOp::Composite {
        fg_x: 0,
        fg_y: 0,
        blend_mode: None,
    });
    graph.add_edge(fg, composite, EdgeKind::Input);
    graph.add_edge(bg, composite, EdgeKind::Canvas);
    let orient = graph.add_node(NodeOp::Orient(zenlayout::Orientation::Rotate180));
    graph.add_edge(composite, orient, EdgeKind::Input);
    let output = graph.add_node(NodeOp::Output);
    graph.add_edge(orient, output, EdgeKind::Input);
    let graph = graph.with_source_precision(bg, opaque_real_16_bit());
    let infos = HashMap::from([(fg, info(D::RGBA8_SRGB)), (bg, info(D::RGBA16_SRGB))]);
    assert!(graph.estimate(&infos).is_err());
}
