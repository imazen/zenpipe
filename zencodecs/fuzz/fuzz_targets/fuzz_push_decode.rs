//! Fuzz target: streaming push decode path.
//!
//! Uses a counting sink to exercise the streaming decode code path,
//! which is different from full-frame decode.
#![no_main]

use libfuzzer_sys::fuzz_target;
use zencodec::decode::{DecodeRowSink, SinkError};
use zencodecs::{AllowedFormats, DecodeRequest, Limits};
use zenpixels::{PixelDescriptor, PixelSliceMut};

struct CountingSink {
    buf: Vec<u8>,
    rows: u32,
    max_rows: u32,
    width: u32,
    bpp: usize,
}

impl DecodeRowSink for CountingSink {
    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Result<(), SinkError> {
        if height > self.max_rows || width > 1024 {
            return Err("dimensions exceed fuzz limit".into());
        }
        self.width = width;
        self.bpp = descriptor.bytes_per_pixel() as usize;
        // Pre-allocate one strip buffer (reused for each strip)
        let strip_bytes = width as usize * self.bpp * 16; // 16 rows max strip
        self.buf.resize(strip_bytes.min(1024 * 1024), 0);
        Ok(())
    }

    fn provide_next_buffer(
        &mut self,
        _y: u32,
        height: u32,
        width: u32,
        descriptor: PixelDescriptor,
    ) -> Result<PixelSliceMut<'_>, SinkError> {
        self.rows = self.rows.saturating_add(height);
        if self.rows > self.max_rows {
            return Err("row count exceeds fuzz limit".into());
        }
        let stride = width as usize * self.bpp;
        let needed = stride * height as usize;
        if needed > self.buf.len() {
            self.buf.resize(needed, 0);
        }
        PixelSliceMut::new(&mut self.buf[..needed], width, height, stride, descriptor)
            .map_err(|e| -> SinkError { alloc::format!("{e}").into() })
    }

    fn finish(&mut self) -> Result<(), SinkError> {
        Ok(())
    }
}

extern crate alloc;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::none()
        .with_max_width(4096)
        .with_max_height(4096)
        .with_max_pixels(4_000_000)
        .with_max_memory_bytes(64 * 1024 * 1024)
        .with_max_frames(50);
    let mut sink = CountingSink {
        buf: Vec::new(),
        rows: 0,
        max_rows: 1024,
        width: 0,
        bpp: 4,
    };
    let _ = DecodeRequest::new(data)
        .with_limits(&limits)
        .with_registry(&AllowedFormats::all())
        .push_decode(&mut sink);
});
