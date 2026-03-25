# zencodecs

Unified image codec dispatch for Rust. Thin layer over format-specific encoders and decoders:
[zenjpeg](https://github.com/imazen/zenjpeg),
[zenwebp](https://github.com/imazen/zenwebp),
[zengif](https://github.com/imazen/zengif),
[zenavif](https://github.com/imazen/zenavif),
[zenpng](https://github.com/imazen/zenpng),
[zenjxl](https://github.com/imazen/zenjxl),
and [heic-decoder](https://github.com/imazen/heic-decoder-rs) (Imazen fork).

## Usage

```rust
use zencodecs::{ImageFormat, DecodeRequest, EncodeRequest, PixelBufferConvertExt};
use zencodecs::pixel::{ImgVec, Rgba};

// Detect format from magic bytes and decode
let data: &[u8] = todo!(); // your image bytes
let decoded = DecodeRequest::new(data).decode()?;
println!("{}x{}", decoded.width(), decoded.height());

// Convert to RGBA8 for processing
let rgba = decoded.into_buffer().to_rgba8();

// Encode as WebP from typed pixel data
let pixels = ImgVec::new(vec![Rgba { r: 0u8, g: 0, b: 0, a: 255 }; 100*100], 100, 100);
let webp = EncodeRequest::new(ImageFormat::WebP)
    .with_quality(85.0)
    .encode_rgba8(pixels.as_ref())?;
println!("Encoded {} bytes", webp.len());
# Ok::<(), zencodecs::CodecError>(())
```

### Typed Encode Methods

Each encode method takes a typed `ImgRef<P>`:

```rust
req.encode_rgb8(img)       // ImgRef<Rgb<u8>>
req.encode_rgba8(img)      // ImgRef<Rgba<u8>>
req.encode_bgra8(img)      // ImgRef<Bgra<u8>>
req.encode_bgrx8(img)      // ImgRef<Bgra<u8>> — alpha ignored
req.encode_gray8(img)      // ImgRef<Gray<u8>>
req.encode_rgb_f32(img)    // ImgRef<Rgb<f32>> — linear light
req.encode_rgba_f32(img)   // ImgRef<Rgba<f32>> — linear light
req.encode_gray_f32(img)   // ImgRef<Gray<f32>> — linear light
```

The dispatch layer handles pixel format conversion to whatever the codec needs natively.

### Probing

```rust
use zencodecs::from_bytes;

let info = from_bytes(data)?;
println!("{:?} {}x{}", info.format, info.width, info.height);
# Ok::<(), zencodecs::CodecError>(())
```

### Runtime Codec Control

```rust
use zencodecs::{CodecRegistry, ImageFormat, DecodeRequest};

let registry = CodecRegistry::none()
    .with_decode(ImageFormat::Jpeg, true)
    .with_decode(ImageFormat::WebP, true);

let decoded = DecodeRequest::new(data)
    .with_registry(&registry)
    .decode()?;
# Ok::<(), zencodecs::CodecError>(())
```

### Format-Specific Config

```rust
use zencodecs::{EncodeRequest, ImageFormat};
use zencodecs::config::CodecConfig;

let config = CodecConfig::default();
// .with_jpeg_encoder(...)
// .with_avif_speed(...)
// etc.

let request = EncodeRequest::new(ImageFormat::Jpeg)
    .with_codec_config(&config)
    .with_quality(92.0);
```

### Cooperative Cancellation and Limits

```rust
use zencodecs::{DecodeRequest, Limits};

let limits = Limits {
    max_width: Some(4096),
    max_height: Some(4096),
    max_pixels: Some(16_000_000),
    max_memory_bytes: Some(256_000_000),
    ..Default::default()
};

let decoded = DecodeRequest::new(data)
    .with_limits(&limits)
    .decode()?;
# Ok::<(), zencodecs::CodecError>(())
```

Stop tokens (`enough::Stop`) are forwarded to codecs that support cooperative cancellation.

## Features

Every codec is feature-gated. Enable only what you need:

```toml
[dependencies]
zencodecs = { version = "0.1", features = ["jpeg", "webp", "png"] }
```

| Feature | Codec | Decode | Encode | Notes |
|---------|-------|--------|--------|-------|
| `jpeg` | zenjpeg | Yes | Yes | |
| `jpeg-ultrahdr` | zenjpeg | Yes | Yes | UltraHDR gain map support |
| `webp` | zenwebp | Yes | Yes | |
| `gif` | zengif | Yes | Yes | |
| `gif-zenquant` | zengif + zenquant | Yes | Yes | Palette quantization via zenquant |
| `gif-quantizr` | zengif + quantizr | Yes | Yes | Palette quantization via quantizr |
| `gif-imagequant` | zengif + imagequant | Yes | Yes | Palette quantization via imagequant |
| `png` | zenpng | Yes | Yes | |
| `png-zenquant` | zenpng + zenquant | Yes | Yes | Palette quantization |
| `avif-decode` | zenavif | Yes | No | |
| `avif-encode` | zenavif | No | Yes | |
| `jxl-decode` | zenjxl | Yes | No | |
| `jxl-encode` | zenjxl | No | Yes | |
| `heic-decode` | heic-decoder (Imazen fork) | Yes | No | |
| `bitmaps` | zenbitmaps | Yes | Yes | PNM/PAM/PFM, BMP, Farbfeld |
| `bitmaps-bmp` | zenbitmaps | Yes | Yes | BMP only |
| `tiff` | zentiff | Yes | Yes | |
| `raw-decode` | zenraw | Yes | No | RAW/DNG via rawloader (LGPL) |
| `raw-decode-exif` | zenraw | Yes | No | EXIF metadata for RAW/DNG |
| `raw-decode-xmp` | zenraw | Yes | No | XMP metadata for RAW/DNG |
| `raw-decode-gainmap` | zenraw | Yes | No | Gain map from DNG/AMPF |
| `riapi` | — | — | — | RIAPI codec key parsing |
| `zennode` | zennode | — | — | Pipeline node definitions |
| `calibrate` | (meta) | — | — | All lossy encoders for quality calibration |
| `all` | (meta) | Yes | Yes | All codecs and features |

Default features: `jpeg`, `webp`, `gif`, `gif-zenquant`, `png`, `png-zenquant`, `avif-decode`, `avif-encode`, `jxl-decode`, `heic-decode`, `bitmaps-bmp`.

## What This Crate Does

- Format detection from magic bytes
- Image probing (dimensions, format, color info) without full decode
- Typed pixel buffer encode/decode with automatic format negotiation
- Runtime codec registry for per-request codec control
- Resource limits and cooperative cancellation forwarded to codecs
- Format-specific codec configuration via `CodecConfig`

## What This Crate Does Not Do

- Image processing (resize, crop, rotate)
- Color management (ICC profile application)

## License

Sustainable, large-scale open source work requires a funding model, and I have been
doing this full-time for 15 years. If you are using this for closed-source development
AND make over $1 million per year, you'll need to buy a commercial license at
https://www.imazen.io/pricing

Commercial licenses are similar to the Apache 2 license but company-specific, and on
a sliding scale. You can also use this under the AGPL v3.
