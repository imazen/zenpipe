use zenpipe::ops::{NormalizeU8ToF32, QuantizeF32ToU8, SrgbToLinearPremul, UnpremulLinearToSrgb};
use zenpipe::sources::{CallbackSource, CropSource, MaterializedSource, TransformSource};
use zenpipe::{PipeError, PixelFormat, Source, StripRef};

/// Collect all strips from a source into a flat Vec<u8>.
fn drain(source: &mut dyn Source) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(Some(strip)) = source.next() {
        out.extend_from_slice(strip.data);
    }
    out
}

/// Create a solid-color RGBA8 source.
fn solid_rgba8(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> CallbackSource {
    let pixel = [r, g, b, a];
    let row_bytes = width as usize * 4;
    let mut rows_produced = 0u32;
    CallbackSource::new(width, height, PixelFormat::Rgba8, 16, move |buf| {
        if rows_produced >= height {
            return Ok(false);
        }
        for px in buf[..row_bytes].chunks_exact_mut(4) {
            px.copy_from_slice(&pixel);
        }
        rows_produced += 1;
        Ok(true)
    })
}

#[test]
fn callback_source_dimensions() {
    let src = solid_rgba8(64, 48, 255, 0, 0, 255);
    assert_eq!(src.width(), 64);
    assert_eq!(src.height(), 48);
    assert_eq!(src.format(), PixelFormat::Rgba8);
}

#[test]
fn callback_source_yields_all_rows() {
    let mut src = solid_rgba8(8, 32, 128, 64, 32, 255);
    let data = drain(&mut src);
    // 8 pixels * 4 bytes * 32 rows = 1024 bytes
    assert_eq!(data.len(), 8 * 4 * 32);
    // Every pixel should be [128, 64, 32, 255]
    for px in data.chunks_exact(4) {
        assert_eq!(px, [128, 64, 32, 255]);
    }
}

#[test]
fn callback_source_strips_cover_image() {
    let mut src = solid_rgba8(4, 50, 0, 0, 0, 255);
    let mut total_rows = 0u32;
    let mut strip_count = 0u32;
    while let Ok(Some(strip)) = src.next() {
        assert_eq!(strip.width, 4);
        assert_eq!(strip.y, total_rows);
        total_rows += strip.height;
        strip_count += 1;
    }
    assert_eq!(total_rows, 50);
    // With strip_height=16: 16+16+16+2 = 4 strips
    assert_eq!(strip_count, 4);
}

#[test]
fn from_data_source() {
    let width = 4u32;
    let height = 3u32;
    let mut data = vec![0u8; width as usize * height as usize * 4];
    // Row 0: red, Row 1: green, Row 2: blue
    for x in 0..width as usize {
        data[x * 4] = 255; // R
        data[x * 4 + 3] = 255; // A
    }
    let row1 = width as usize * 4;
    for x in 0..width as usize {
        data[row1 + x * 4 + 1] = 255; // G
        data[row1 + x * 4 + 3] = 255; // A
    }
    let row2 = width as usize * 4 * 2;
    for x in 0..width as usize {
        data[row2 + x * 4 + 2] = 255; // B
        data[row2 + x * 4 + 3] = 255; // A
    }

    let mut src = CallbackSource::from_data(&data, width, height, PixelFormat::Rgba8, 16);
    let out = drain(&mut src);
    assert_eq!(out, data);
}

#[test]
fn transform_roundtrip_srgb_linear() {
    // sRGB u8 → linear premul f32 → back to sRGB u8 should be near-identity
    let mut src = TransformSource::new(Box::new(solid_rgba8(4, 2, 200, 100, 50, 255)))
        .push(SrgbToLinearPremul)
        .push(UnpremulLinearToSrgb);

    assert_eq!(src.width(), 4);
    assert_eq!(src.height(), 2);
    assert_eq!(src.format(), PixelFormat::Rgba8);

    let data = drain(&mut src);
    assert_eq!(data.len(), 4 * 4 * 2);
    for px in data.chunks_exact(4) {
        // Allow ±1 for rounding
        assert!((px[0] as i16 - 200).unsigned_abs() <= 1, "R: {}", px[0]);
        assert!((px[1] as i16 - 100).unsigned_abs() <= 1, "G: {}", px[1]);
        assert!((px[2] as i16 - 50).unsigned_abs() <= 1, "B: {}", px[2]);
        assert_eq!(px[3], 255);
    }
}

#[test]
fn transform_normalize_quantize_roundtrip() {
    let mut src = TransformSource::new(Box::new(solid_rgba8(2, 2, 128, 64, 32, 200)))
        .push(NormalizeU8ToF32)
        .push(QuantizeF32ToU8);

    assert_eq!(src.format(), PixelFormat::Rgba8);
    let data = drain(&mut src);
    for px in data.chunks_exact(4) {
        assert_eq!(px, [128, 64, 32, 200]);
    }
}

#[test]
fn transform_passthrough_no_ops() {
    let mut src = TransformSource::new(Box::new(solid_rgba8(4, 4, 42, 42, 42, 255)));
    let data = drain(&mut src);
    assert_eq!(data.len(), 4 * 4 * 4);
    for px in data.chunks_exact(4) {
        assert_eq!(px, [42, 42, 42, 255]);
    }
}

#[test]
fn crop_basic() {
    // 8x8 image, crop (2, 1, 4, 3) → 4x3 sub-image
    let width = 8u32;
    let height = 8u32;
    let mut data = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let i = (y * width as usize + x) * 4;
            data[i] = x as u8;
            data[i + 1] = y as u8;
            data[i + 2] = 0;
            data[i + 3] = 255;
        }
    }

    let src = CallbackSource::from_data(&data, width, height, PixelFormat::Rgba8, 16);
    let mut crop = CropSource::new(Box::new(src), 2, 1, 4, 3).unwrap();

    assert_eq!(crop.width(), 4);
    assert_eq!(crop.height(), 3);

    let out = drain(&mut crop);
    assert_eq!(out.len(), 4 * 3 * 4);

    // Verify the cropped region: x=[2,5], y=[1,3]
    for cy in 0..3usize {
        for cx in 0..4usize {
            let i = (cy * 4 + cx) * 4;
            assert_eq!(out[i], (cx + 2) as u8, "x at ({cx},{cy})");
            assert_eq!(out[i + 1], (cy + 1) as u8, "y at ({cx},{cy})");
        }
    }
}

#[test]
fn crop_out_of_bounds_error() {
    let src = solid_rgba8(8, 8, 0, 0, 0, 255);
    let result = CropSource::new(Box::new(src), 6, 0, 4, 4);
    assert!(result.is_err());
}

#[test]
fn materialize_from_source() {
    let src = solid_rgba8(4, 8, 10, 20, 30, 255);
    let mut mat = MaterializedSource::from_source(Box::new(src)).unwrap();

    assert_eq!(mat.width(), 4);
    assert_eq!(mat.height(), 8);

    let data = drain(&mut mat);
    assert_eq!(data.len(), 4 * 8 * 4);
    for px in data.chunks_exact(4) {
        assert_eq!(px, [10, 20, 30, 255]);
    }
}

#[test]
fn materialize_with_transform() {
    // Materialize and then flip horizontally within each row
    let width = 4u32;
    let height = 2u32;
    let bpp = 4usize;
    let mut data = vec![0u8; width as usize * height as usize * bpp];
    for y in 0..height as usize {
        for x in 0..width as usize {
            data[(y * width as usize + x) * bpp] = x as u8;
            data[(y * width as usize + x) * bpp + 3] = 255;
        }
    }

    let src = CallbackSource::from_data(&data, width, height, PixelFormat::Rgba8, 16);
    let mut mat =
        MaterializedSource::from_source_with_transform(Box::new(src), |buf, w, _h, _fmt| {
            let width = *w as usize;
            let row_bytes = width * bpp;
            let rows = buf.len() / row_bytes;
            for y in 0..rows {
                let start = y * row_bytes;
                let row = &mut buf[start..start + row_bytes];
                // Reverse pixel order within row
                for x in 0..width / 2 {
                    let a = x * bpp;
                    let b = (width - 1 - x) * bpp;
                    for c in 0..bpp {
                        row.swap(a + c, b + c);
                    }
                }
            }
        })
        .unwrap();

    let out = drain(&mut mat);
    // Row pixels should be reversed: [3,2,1,0] instead of [0,1,2,3]
    for y in 0..height as usize {
        for x in 0..width as usize {
            let i = (y * width as usize + x) * bpp;
            assert_eq!(out[i], (width as usize - 1 - x) as u8);
        }
    }
}

#[test]
fn execute_pipeline() {
    // Test the full execute() function with a collecting sink
    let mut src = solid_rgba8(4, 4, 255, 0, 0, 255);
    let mut sink = CollectSink::new();

    zenpipe::execute(&mut src, &mut sink).unwrap();

    assert_eq!(sink.data.len(), 4 * 4 * 4);
    assert!(sink.finished);
    for px in sink.data.chunks_exact(4) {
        assert_eq!(px, [255, 0, 0, 255]);
    }
}

/// A sink that collects all strip data for testing.
struct CollectSink {
    data: Vec<u8>,
    finished: bool,
}

impl CollectSink {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            finished: false,
        }
    }
}

impl zenpipe::Sink for CollectSink {
    fn consume(&mut self, strip: &StripRef<'_>) -> Result<(), PipeError> {
        self.data.extend_from_slice(strip.data);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), PipeError> {
        self.finished = true;
        Ok(())
    }
}

#[test]
fn transform_chain_format_progression() {
    // Build a chain: Rgba8 → normalize → Rgbaf32Srgb → quantize → Rgba8
    // Verify intermediate format tracking
    let src = solid_rgba8(2, 1, 100, 150, 200, 255);
    let t1 = TransformSource::new(Box::new(src)).push(NormalizeU8ToF32);
    assert_eq!(t1.format(), PixelFormat::Rgbaf32Srgb);
    let t2 = t1.push(QuantizeF32ToU8);
    assert_eq!(t2.format(), PixelFormat::Rgba8);
}

#[test]
fn strip_buf_basic() {
    use zenpipe::StripBuf;

    let mut buf = StripBuf::new(4, 3, PixelFormat::Rgba8);
    assert_eq!(buf.rows_filled(), 0);
    assert_eq!(buf.capacity_rows(), 3);
    assert_eq!(buf.stride(), 16);

    let row = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    assert!(buf.push_row(&row));
    assert_eq!(buf.rows_filled(), 1);
    assert_eq!(buf.row(0), &row);

    assert!(buf.push_row(&row));
    assert!(buf.push_row(&row));
    assert_eq!(buf.rows_filled(), 3);
    // Buffer full
    assert!(!buf.push_row(&row));
}

#[test]
fn materialize_from_data() {
    let data = vec![255u8; 4 * 4 * 4]; // 4x4 white RGBA
    let mut src = MaterializedSource::from_data(data.clone(), 4, 4, PixelFormat::Rgba8);
    let out = drain(&mut src);
    assert_eq!(out, data);
}

#[test]
fn crop_full_image() {
    // Cropping the full image should be identity
    let data = vec![42u8; 8 * 8 * 4];
    let src = CallbackSource::from_data(&data, 8, 8, PixelFormat::Rgba8, 16);
    let mut crop = CropSource::new(Box::new(src), 0, 0, 8, 8).unwrap();
    let out = drain(&mut crop);
    assert_eq!(out.len(), 8 * 8 * 4);
    assert!(out.iter().all(|&b| b == 42));
}

#[test]
fn small_strip_height() {
    // Strip height of 1 — process one row at a time
    // CallbackSource uses the strip_height we give it
    let mut src = CallbackSource::new(4, 4, PixelFormat::Rgba8, 1, {
        let mut row = 0u32;
        move |buf| {
            if row >= 4 {
                return Ok(false);
            }
            buf[..16].fill(99);
            // Set alpha
            for i in (3..16).step_by(4) {
                buf[i] = 255;
            }
            row += 1;
            Ok(true)
        }
    });

    let mut strip_count = 0;
    while let Ok(Some(strip)) = src.next() {
        assert_eq!(strip.height, 1);
        strip_count += 1;
    }
    assert_eq!(strip_count, 4);
}
