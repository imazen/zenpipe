//! Canvas extend fill modes — replicate / mirror / repeat (zenpipe#23).
//! Pixel-exact against a hand-computed reference, through both the
//! `ExpandCanvasSource` API and the `NodeOp::ExtendCanvas` graph op.

use hashbrown::HashMap;
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::{CallbackSource, CanvasFill, ExpandCanvasSource};
use zenpipe::{Source, format};

/// `w`×`h` RGBA8 source whose pixel at (x, y) is [x, y, 7, 255] — every
/// pixel is unique, so any wrong mapping is visible.
fn coord_source(w: u32, h: u32) -> Box<dyn Source> {
    let row_bytes = w as usize * 4;
    let mut y = 0u32;
    Box::new(CallbackSource::new(
        w,
        h,
        format::RGBA8_SRGB,
        3, // tiny strips: forces the prefix/ring bookkeeping across strip boundaries
        move |buf| {
            if y >= h {
                return Ok(false);
            }
            for (x, px) in buf[..row_bytes]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .enumerate()
            {
                px.copy_from_slice(&[x as u8, y as u8, 7, 255]);
            }
            y += 1;
            Ok(true)
        },
    ))
}

fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        let stride = strip.stride();
        let row_bytes = strip.width() as usize * 4;
        let bytes = strip.as_strided_bytes();
        for r in 0..strip.rows() as usize {
            out.extend_from_slice(&bytes[r * stride..r * stride + row_bytes]);
        }
    }
    out
}

/// Reference coordinate mapping for one axis.
fn map(fill: CanvasFill, t: i64, len: i64) -> i64 {
    match fill {
        CanvasFill::Solid(_) => unreachable!(),
        CanvasFill::Replicate => t.clamp(0, len - 1),
        CanvasFill::Mirror => {
            let p = t.rem_euclid(2 * len);
            if p >= len { 2 * len - 1 - p } else { p }
        }
        CanvasFill::Repeat => t.rem_euclid(len),
    }
}

/// Expected canvas for `w`×`h` content padded by (l, t, r, b) with `fill`.
fn expected(w: u32, h: u32, l: u32, t: u32, r: u32, b: u32, fill: CanvasFill) -> Vec<u8> {
    let (cw, ch) = (w + l + r, h + t + b);
    let mut out = Vec::with_capacity((cw * ch * 4) as usize);
    for y in 0..ch as i64 {
        for x in 0..cw as i64 {
            let sx = map(fill, x - l as i64, w as i64);
            let sy = map(fill, y - t as i64, h as i64);
            out.extend_from_slice(&[sx as u8, sy as u8, 7, 255]);
        }
    }
    out
}

fn check(w: u32, h: u32, l: u32, t: u32, r: u32, b: u32, fill: CanvasFill) {
    let src = coord_source(w, h);
    let mut canvas =
        ExpandCanvasSource::new(src, w + l + r, h + t + b, l as i32, t as i32, [9, 9, 9, 9])
            .unwrap()
            .with_fill(fill);
    assert_eq!(canvas.fill(), fill);
    let got = drain(&mut canvas);
    let want = expected(w, h, l, t, r, b, fill);
    assert_eq!(
        got.len(),
        want.len(),
        "{fill:?} {w}x{h} pad {l},{t},{r},{b}: length"
    );
    if got != want {
        let cw = (w + l + r) as usize;
        for (i, (g, e)) in got
            .as_chunks::<4>()
            .0
            .iter()
            .zip(want.as_chunks::<4>().0.iter())
            .enumerate()
        {
            assert_eq!(
                g,
                e,
                "{fill:?} {w}x{h} pad {l},{t},{r},{b}: first mismatch at ({}, {})",
                i % cw,
                i / cw
            );
        }
    }
}

#[test]
fn mirror_is_pixel_exact_all_sides() {
    check(5, 4, 2, 3, 2, 3, CanvasFill::Mirror);
    // Padding wider/taller than the content (multiple reflection periods).
    check(3, 2, 7, 5, 8, 6, CanvasFill::Mirror);
    // Horizontal only / vertical only.
    check(6, 5, 4, 0, 4, 0, CanvasFill::Mirror);
    check(6, 5, 0, 4, 0, 4, CanvasFill::Mirror);
    // Bottom-only (ring buffer path) and top-only (prefix path).
    check(4, 7, 0, 0, 0, 5, CanvasFill::Mirror);
    check(4, 7, 0, 5, 0, 0, CanvasFill::Mirror);
}

#[test]
fn repeat_is_pixel_exact_all_sides() {
    check(5, 4, 2, 3, 2, 3, CanvasFill::Repeat);
    check(3, 2, 7, 5, 8, 6, CanvasFill::Repeat);
    check(6, 5, 4, 0, 4, 0, CanvasFill::Repeat);
    // top == 0: only the leading `bottom` rows are buffered.
    check(4, 7, 0, 0, 0, 5, CanvasFill::Repeat);
    check(4, 7, 0, 0, 0, 20, CanvasFill::Repeat);
    // top > 0: whole content buffered.
    check(4, 7, 0, 5, 0, 0, CanvasFill::Repeat);
}

#[test]
fn replicate_is_pixel_exact_all_sides() {
    check(5, 4, 2, 3, 2, 3, CanvasFill::Replicate);
    check(3, 2, 7, 5, 8, 6, CanvasFill::Replicate);
}

#[test]
fn solid_fill_is_unchanged_and_from_name_parses() {
    let src = coord_source(3, 2);
    let mut canvas = ExpandCanvasSource::new(src, 5, 4, 1, 1, [1, 2, 3, 4]).unwrap();
    let got = drain(&mut canvas);
    assert_eq!(&got[0..4], &[1, 2, 3, 4]);
    assert_eq!(&got[(5 + 1) * 4..(5 + 2) * 4], &[0, 0, 7, 255]);

    let bg = [1, 2, 3, 4];
    assert_eq!(
        CanvasFill::from_name("solid", bg),
        Some(CanvasFill::Solid(bg))
    );
    assert_eq!(
        CanvasFill::from_name("Mirror", bg),
        Some(CanvasFill::Mirror)
    );
    assert_eq!(CanvasFill::from_name("tile", bg), Some(CanvasFill::Repeat));
    assert_eq!(
        CanvasFill::from_name("copy", bg),
        Some(CanvasFill::Replicate)
    );
    assert_eq!(CanvasFill::from_name("bogus", bg), None);
}

#[test]
fn extend_canvas_graph_op_matches_source() {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let ext = g.add_node(NodeOp::ExtendCanvas {
        left: 3,
        top: 2,
        right: 4,
        bottom: 5,
        fill: CanvasFill::Mirror,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, ext, EdgeKind::Input);
    g.add_edge(ext, out, EdgeKind::Input);
    let mut sources = HashMap::new();
    sources.insert(src, coord_source(6, 4));
    let mut pipeline = g.compile(sources).unwrap();
    assert_eq!((pipeline.width(), pipeline.height()), (13, 11));
    let got = drain(pipeline.as_mut());
    assert_eq!(got, expected(6, 4, 3, 2, 4, 5, CanvasFill::Mirror));
}
