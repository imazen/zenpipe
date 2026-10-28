//! A `Layout` node whose plan keeps the size moves samples exactly (zenpipe
//! issue 107). Codec-free: every expected canvas is authored here from the
//! EXIF orientation definitions, not taken from zenpipe.

use hashbrown::HashMap;
use zenlayout::{CanvasColor, LayoutPlan, Orientation, Rect, Size};
use zenpipe::graph::{EdgeKind, NodeOp, PipelineGraph};
use zenpipe::sources::MaterializedSource;
use zenpipe::{Source, format};

const ORIENTATIONS: [Orientation; 8] = [
    Orientation::Identity,
    Orientation::FlipH,
    Orientation::Rotate180,
    Orientation::FlipV,
    Orientation::Transpose,
    Orientation::Rotate90,
    Orientation::Transverse,
    Orientation::Rotate270,
];

/// Where sample `(x, y)` of a `w`×`h` raster lands under EXIF orientation `o`.
fn turn(o: Orientation, x: u32, y: u32, w: u32, h: u32) -> (u32, u32) {
    match o {
        Orientation::Identity => (x, y),
        Orientation::FlipH => (w - 1 - x, y),
        Orientation::Rotate180 => (w - 1 - x, h - 1 - y),
        Orientation::FlipV => (x, h - 1 - y),
        Orientation::Transpose => (y, x),
        Orientation::Rotate90 => (h - 1 - y, x),
        Orientation::Transverse => (h - 1 - y, w - 1 - x),
        Orientation::Rotate270 => (y, w - 1 - x),
        _ => unreachable!("eight EXIF orientations"),
    }
}

fn turned(o: Orientation, w: u32, h: u32) -> Size {
    if o.swaps_axes() {
        Size::new(h, w)
    } else {
        Size::new(w, h)
    }
}

/// Opaque RGBA8 samples unique per row and column.
fn samples(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            out.extend_from_slice(&[x as u8, y as u8, (x * 7 + y * 13) as u8, 255]);
        }
    }
    out
}

/// Run `plan` through a Source → Layout → Output graph.
fn run_layout(plan: LayoutPlan, w: u32, h: u32, data: Vec<u8>) -> (u32, u32, Vec<u8>) {
    let mut g = PipelineGraph::new();
    let src = g.add_node(NodeOp::Source);
    let layout = g.add_node(NodeOp::Layout {
        plan,
        filter: zenresize::Filter::Robidoux,
    });
    let out = g.add_node(NodeOp::Output);
    g.add_edge(src, layout, EdgeKind::Input);
    g.add_edge(layout, out, EdgeKind::Input);
    let mut sources: HashMap<usize, Box<dyn Source>> = HashMap::new();
    sources.insert(
        src,
        Box::new(MaterializedSource::from_data(
            data,
            w,
            h,
            format::RGBA8_SRGB,
        )),
    );
    let mut pipeline = g.compile(sources).unwrap();
    let (ow, oh) = (pipeline.width(), pipeline.height());
    let mut pixels = Vec::new();
    while let Some(strip) = pipeline.next().unwrap() {
        for r in 0..strip.rows() {
            pixels.extend_from_slice(strip.row(r));
        }
    }
    (ow, oh, pixels)
}

/// Trim, orient, then place on a canvas of `bg` (negative offsets cut the
/// content at the top-left edge).
fn authored(
    src: &[u8],
    w: u32,
    trim: Rect,
    o: Orientation,
    canvas: Size,
    place: (i32, i32),
    bg: [u8; 4],
) -> Vec<u8> {
    let mut out = bg.repeat((canvas.width * canvas.height) as usize);
    for ty in 0..trim.height {
        for tx in 0..trim.width {
            let (dx, dy) = turn(o, tx, ty, trim.width, trim.height);
            let (cx, cy) = (dx as i64 + place.0 as i64, dy as i64 + place.1 as i64);
            if cx < 0 || cy < 0 || cx >= canvas.width as i64 || cy >= canvas.height as i64 {
                continue;
            }
            let s = (((trim.y + ty) * w + trim.x + tx) * 4) as usize;
            let d = ((cy as u32 * canvas.width + cx as u32) * 4) as usize;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    out
}

#[test]
fn a_size_preserving_layout_moves_every_sample_exactly() {
    // Odd sizes: the last 16-row source strip is short, and a trim starts
    // and ends inside strips.
    let (w, h) = (29u32, 19u32);
    let data = samples(w, h);
    let bg = [10, 20, 30, 40];
    let mut wrong = Vec::new();
    for o in ORIENTATIONS {
        let full = Rect::new(0, 0, w, h);
        let trim = Rect::new(3, 5, 17, 11);
        let t = turned(o, trim.width, trim.height);
        let cases = [
            ("orientation", full, None, turned(o, w, h), (0, 0)),
            ("trim", trim, Some(trim), t, (0, 0)),
            (
                "trim+pad",
                trim,
                Some(trim),
                Size::new(t.width + 5, t.height + 3),
                (1, 2),
            ),
            ("trim+negative placement", trim, Some(trim), t, (-2, -1)),
        ];
        for (name, region, plan_trim, canvas, place) in cases {
            let mut plan = LayoutPlan::identity(turned(o, region.width, region.height))
                .with_remaining_orientation(o)
                .with_canvas(canvas)
                .with_placement(place.0, place.1)
                .with_canvas_color(CanvasColor::Srgb {
                    r: bg[0],
                    g: bg[1],
                    b: bg[2],
                    a: bg[3],
                });
            if let Some(r) = plan_trim {
                plan = plan.with_trim(r);
            }
            let expected = authored(&data, w, region, o, canvas, place, bg);
            let (ow, oh, got) = run_layout(plan, w, h, data.clone());
            if (ow, oh) != (canvas.width, canvas.height) {
                wrong.push(format!(
                    "{o:?} {name}: {ow}x{oh}, expected {}x{}",
                    canvas.width, canvas.height
                ));
                continue;
            }
            let differ = got.iter().zip(&expected).filter(|(a, b)| a != b).count();
            let max = got
                .iter()
                .zip(&expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            if got.len() != expected.len() || differ != 0 {
                wrong.push(format!(
                    "{o:?} {name}: {differ}/{} samples differ (max {max}), {} bytes for {}",
                    expected.len(),
                    got.len(),
                    expected.len()
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
