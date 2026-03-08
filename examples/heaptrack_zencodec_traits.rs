//! Heaptrack profiling: exercise zencodec-types trait interface for all codecs.
//!
//! Run with:
//!   cargo build --release --example heaptrack_zencodec_traits --features "jpeg,webp,gif,gif-quantizr,png,avif-decode,avif-encode,std"
//!   heaptrack target/release/examples/heaptrack_zencodec_traits 2>&1 | tee /tmp/heaptrack_zencodec.log

use std::borrow::Cow;

use zc::decode::DecodeJob as _;
use zc::decode::DecoderConfig as _;
use zc::encode::EncodeJob as _;
use zc::encode::EncoderConfig as _;

fn generate_rgb8(width: u32, height: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            data.push(((x * 255) / width) as u8);
            data.push(((y * 255) / height) as u8);
            data.push(128u8);
        }
    }
    data
}

fn generate_rgba8(width: u32, height: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            data.push(((x * 255) / width) as u8);
            data.push(((y * 255) / height) as u8);
            data.push(128u8);
            data.push(255u8);
        }
    }
    data
}

/// Simple row sink that collects decoded data and counts rows.
struct CollectingSink {
    rows_received: u32,
    buffer: Vec<u8>,
}

impl CollectingSink {
    fn new() -> Self {
        Self {
            rows_received: 0,
            buffer: Vec::new(),
        }
    }
}

impl zc::decode::DecodeRowSink for CollectingSink {
    fn provide_next_buffer(
        &mut self,
        _y: u32,
        height: u32,
        width: u32,
        descriptor: zenpixels::PixelDescriptor,
    ) -> Result<zenpixels::PixelSliceMut<'_>, zc::decode::SinkError> {
        let row_bytes = width as usize * descriptor.bytes_per_pixel();
        let total = row_bytes * height as usize;
        let start = self.buffer.len();
        self.buffer.resize(start + total, 0u8);
        self.rows_received += height;
        zenpixels::PixelSliceMut::new(
            &mut self.buffer[start..],
            width,
            height,
            row_bytes,
            descriptor,
        )
        .map_err(|e| Box::new(e) as zc::decode::SinkError)
    }
}

/// Run encode + decode via dyn dispatch (DynEncoder/DynDecoder)
fn roundtrip_dyn(
    enc_config: &dyn zc::encode::DynEncoderConfig,
    dec_config: &dyn zc::decode::DynDecoderConfig,
    pixels: &zenpixels::PixelBuffer,
    limits: &zc::ResourceLimits,
    label: &str,
) {
    let width = pixels.width();
    let height = pixels.height();
    eprintln!("--- {label}: dyn encode {width}x{height} ---");

    // Encode via DynEncoderConfig → DynEncodeJob → DynEncoder
    let mut job = enc_config.dyn_job();
    job.set_limits(*limits);
    let encoder = job.into_encoder().unwrap_or_else(|e| panic!("{label} dyn_encoder: {e}"));
    let pixel_slice = pixels.as_slice();
    let encoded = encoder
        .encode(pixel_slice)
        .unwrap_or_else(|e| panic!("{label} dyn_encode: {e}"));

    let enc_bytes = encoded.data();
    eprintln!(
        "  encoded: {} bytes (format: {:?})",
        enc_bytes.len(),
        encoded.format()
    );

    // Decode via DynDecoderConfig → DynDecodeJob → DynDecoder
    eprintln!("--- {label}: dyn decode ---");
    let mut job = dec_config.dyn_job();
    job.set_limits(*limits);

    let info = job
        .probe(enc_bytes)
        .unwrap_or_else(|e| panic!("{label} dyn_probe: {e}"));
    eprintln!(
        "  probed: {}x{} format={:?}",
        info.width, info.height, info.format
    );

    let data_cow: Cow<'_, [u8]> = Cow::Borrowed(enc_bytes);
    // Request u8 descriptors only — avoids bytemuck cast_vec alignment bug in JPEG f32 path
    let preferred_u8 = &[
        zenpixels::PixelDescriptor::RGB8_SRGB,
        zenpixels::PixelDescriptor::RGBA8_SRGB,
    ];
    let dec = job
        .into_decoder(data_cow, preferred_u8)
        .unwrap_or_else(|e| panic!("{label} dyn_decoder: {e}"));

    let output = dec.decode().unwrap_or_else(|e| panic!("{label} dyn_decode: {e}"));
    eprintln!(
        "  decoded: {}x{} descriptor={:?}",
        output.width(),
        output.height(),
        output.descriptor()
    );
}

/// Run encode then push_decode via concrete codec types
fn push_decode_jpeg(
    enc_bytes: &[u8],
    limits: &zc::ResourceLimits,
) {
    #[cfg(feature = "jpeg")]
    {
        let dec = zenjpeg::JpegDecoderConfig::new();
        let job = dec.job().with_limits(*limits);
        let data_cow: Cow<'_, [u8]> = Cow::Borrowed(enc_bytes);
        let preferred = zenjpeg::JpegDecoderConfig::supported_descriptors();

        let mut sink = CollectingSink::new();
        match job.push_decoder(data_cow, &mut sink, preferred) {
            Ok(info) => {
                eprintln!(
                    "  JPEG push_decode: {}x{} rows={}",
                    info.width, info.height, sink.rows_received
                );
            }
            Err(e) => eprintln!("  JPEG push_decode: {e}"),
        }
    }
    #[cfg(not(feature = "jpeg"))]
    { let _ = (enc_bytes, limits); }
}

fn main() {
    let width = 1024u32;
    let height = 768u32;
    let rgb_data = generate_rgb8(width, height);
    let rgba_data = generate_rgba8(width, height);

    let limits = zc::ResourceLimits::none()
        .with_max_pixels(100_000_000)
        .with_max_memory(512 * 1024 * 1024)
        .with_max_width(16384)
        .with_max_height(16384);

    let rgb_buf = zenpixels::PixelBuffer::from_vec(
        rgb_data,
        width,
        height,
        zenpixels::PixelDescriptor::RGB8_SRGB,
    )
    .expect("rgb buffer");
    let rgba_buf = zenpixels::PixelBuffer::from_vec(
        rgba_data,
        width,
        height,
        zenpixels::PixelDescriptor::RGBA8_SRGB,
    )
    .expect("rgba buffer");

    // --- JPEG via concrete types + dyn dispatch ---
    #[cfg(feature = "jpeg")]
    {
        use zc::encode::Encoder as _;
        let enc = zenjpeg::JpegEncoderConfig::new().with_generic_quality(85.0);
        let dec = zenjpeg::JpegDecoderConfig::new();

        // Dyn dispatch roundtrip
        roundtrip_dyn(&enc, &dec, &rgb_buf, &limits, "JPEG");

        // Concrete push_decode (streaming)
        eprintln!("--- JPEG: concrete encode + push_decode ---");
        let job = enc.job().with_limits(limits);
        let encoder = job.encoder().expect("jpeg encoder");
        let encoded = encoder.encode(rgb_buf.as_slice()).expect("jpeg encode");
        push_decode_jpeg(encoded.data(), &limits);
    }

    // --- WebP ---
    #[cfg(feature = "webp")]
    {
        let enc = zenwebp::WebpEncoderConfig::lossy().with_generic_quality(75.0);
        let dec = zenwebp::WebpDecoderConfig::new();
        roundtrip_dyn(&enc, &dec, &rgb_buf, &limits, "WebP");
    }

    // --- GIF ---
    #[cfg(feature = "gif")]
    {
        let enc = zengif::GifEncoderConfig::new();
        let dec = zengif::GifDecoderConfig::new();
        roundtrip_dyn(&enc, &dec, &rgba_buf, &limits, "GIF");
    }

    // --- PNG ---
    #[cfg(feature = "png")]
    {
        let enc = zenpng::PngEncoderConfig::new();
        let dec = zenpng::PngDecoderConfig::new();
        roundtrip_dyn(&enc, &dec, &rgba_buf, &limits, "PNG");
    }

    // --- AVIF ---
    #[cfg(all(feature = "avif-encode", feature = "avif-decode"))]
    {
        let enc = zenavif::AvifEncoderConfig::new().with_generic_quality(50.0);
        let dec = zenavif::AvifDecoderConfig::new();
        roundtrip_dyn(&enc, &dec, &rgb_buf, &limits, "AVIF");
    }

    eprintln!("\n=== All zencodec-types trait roundtrips complete ===");
}
