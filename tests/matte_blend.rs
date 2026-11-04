//! Mattes blend in linear light, except where browser matching is asked for.

use hashbrown::HashMap;

use zenpipe::Source;
use zenpipe::format;
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::ops::MatteBlend;
use zenpipe::sources::CallbackSource;

fn flatten(pixel: [u8; 4], matte: [u8; 3], blend: MatteBlend) -> [u8; 3] {
    let mut emitted = false;
    let source = CallbackSource::new(1, 1, format::RGBA8_SRGB, 16, move |buffer| {
        if emitted {
            return Ok(false);
        }
        buffer[..4].copy_from_slice(&pixel);
        emitted = true;
        Ok(true)
    });
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let rm = g.add_node(NodeOp::RemoveAlpha { matte, blend });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, rm, EdgeKind::Input);
    g.add_edge(rm, out, EdgeKind::Input);
    let mut sources = HashMap::new();
    sources.insert(src, Box::new(source) as Box<dyn Source>);
    let mut pipeline = g.compile(sources).unwrap();
    let strip = pipeline.next().unwrap().unwrap();
    let row = strip.as_strided_bytes();
    [row[0], row[1], row[2]]
}

#[test]
fn encoded_keeps_the_browser_formula() {
    assert_eq!(
        flatten([255, 0, 0, 128], [255, 255, 255], MatteBlend::Encoded),
        [255, 127, 127]
    );
    for alpha in (0..=255u8).step_by(5) {
        let inv = 255 - u32::from(alpha);
        let expected = |p: u8, m: u8| {
            ((u32::from(p) * u32::from(alpha) + u32::from(m) * inv + 127) / 255) as u8
        };
        assert_eq!(
            flatten([200, 90, 10, alpha], [30, 140, 250], MatteBlend::Encoded),
            [expected(200, 30), expected(90, 140), expected(10, 250)]
        );
    }
}

#[test]
fn linear_agrees_with_composite_onto() {
    let options = zenpixels_convert::policy::ConvertOptions::permissive().with_alpha_policy(
        zenpixels_convert::policy::AlphaPolicy::CompositeOnto {
            r: 30,
            g: 140,
            b: 250,
        },
    );
    let mut converter = zenpixels_convert::RowConverter::new_explicit(
        format::RGBA8_SRGB,
        format::RGB8_SRGB,
        &options,
    )
    .unwrap();
    for alpha in (0..=255u8).step_by(5) {
        let pixel = [200, 90, 10, alpha];
        let mut expected = [0u8; 3];
        converter.convert_row(&pixel, &mut expected, 1);
        assert_eq!(flatten(pixel, [30, 140, 250], MatteBlend::Linear), expected);
    }
}

#[test]
fn half_red_on_white_differs_between_blends() {
    assert_eq!(
        flatten([255, 0, 0, 128], [255, 255, 255], MatteBlend::Linear),
        [255, 187, 187]
    );
}
