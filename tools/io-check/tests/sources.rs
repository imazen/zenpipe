//! Pipeline operators over upstream strips that do not line up with their own
//! output strips, as `DecodeSource` delivers them (one native batch split into
//! strips of any height). Codec-free: exact rows against the authored samples.

use zenpipe::sources::{CropSource, MaterializedSource};
use zenpipe::{PixelDescriptor, Source};

const W: u32 = 23;
const H: u32 = 150;

/// RGB8 samples unique per row and column.
fn samples() -> Vec<u8> {
    let mut out = Vec::with_capacity((W * H * 3) as usize);
    for y in 0..H {
        for x in 0..W {
            out.extend_from_slice(&[y as u8, (y >> 8) as u8 ^ x as u8, (x * 11) as u8]);
        }
    }
    out
}

fn collect(source: &mut dyn Source) -> (Vec<u8>, u32) {
    let mut out = Vec::new();
    let mut rows = 0;
    while let Some(strip) = source.next().unwrap() {
        for r in 0..strip.rows() {
            out.extend_from_slice(strip.row(r));
        }
        rows += strip.rows();
    }
    (out, rows)
}

#[test]
fn crop_keeps_every_row_across_unaligned_upstream_strips() {
    let data = samples();
    let mut wrong = Vec::new();
    for upstream_rows in [1, 7, 16, 17, 64, 150] {
        for (x, y, w, h) in [
            (0, 0, W, H),
            (2, 3, 19, 100),
            (1, 16, 21, 33),
            (5, 5, 3, 1),
            (0, 149, W, 1),
            (4, 61, 9, 89),
        ] {
            let upstream =
                MaterializedSource::from_data(data.clone(), W, H, PixelDescriptor::RGB8_SRGB)
                    .with_strip_height(upstream_rows);
            let mut crop = CropSource::new(Box::new(upstream), x, y, w, h).unwrap();
            let (got, rows) = collect(&mut crop);
            let mut expected = Vec::new();
            for row in y..y + h {
                let start = ((row * W + x) * 3) as usize;
                expected.extend_from_slice(&data[start..start + (w * 3) as usize]);
            }
            if rows != h || got != expected {
                wrong.push(format!(
                    "upstream strips of {upstream_rows}: crop ({x},{y},{w},{h}) gave {rows} rows, \
                     samples equal: {}",
                    got == expected
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
