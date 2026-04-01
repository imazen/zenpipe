# zencodecs ![CI](https://img.shields.io/github/actions/workflow/status/imazen/zencodecs/ci.yml?style=flat-square&label=CI) ![crates.io](https://img.shields.io/crates/v/zencodecs?style=flat-square) [![lib.rs](https://img.shields.io/crates/v/zencodecs?style=flat-square&label=lib.rs&color=blue)](https://lib.rs/crates/zencodecs) ![docs.rs](https://img.shields.io/docsrs/zencodecs?style=flat-square) ![license](https://img.shields.io/crates/l/zencodecs?style=flat-square)

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

## Image tech I maintain

| | |
|:--|:--|
| State of the art codecs* | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] ([rav1d-safe] · [zenrav1e] · [zenavif-parse] · [zenavif-serialize]) · [zenjxl] ([jxl-encoder] · [zenjxl-decoder]) · [zentiff] · [zenbitmaps] · [heic] · [zenraw] · [zenpdf] · [ultrahdr] · [mozjpeg-rs] · [webpx] |
| Compression | [zenflate] · [zenzop] |
| Processing | [zenresize] · [zenfilters] · [zenquant] · [zenblend] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [resamplescope-rs] · [codec-eval] · [codec-corpus] |
| Pixel types & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline | [zenpipe] · [zencodec] · **zencodecs** · [zenlayout] · [zennode] |
| ImageResizer | [ImageResizer] (C#) — 24M+ NuGet downloads across all packages |
| [Imageflow][] | Image optimization engine (Rust) — [.NET][imageflow-dotnet] · [node][imageflow-node] · [go][imageflow-go] — 9M+ NuGet downloads across all packages |
| [Imageflow Server][] | [The fast, safe image server](https://www.imazen.io/) (Rust+C#) — 552K+ NuGet downloads, deployed by Fortune 500s and major brands |

<sub>* as of 2026</sub>

### General Rust awesomeness

[archmage] · [magetypes] · [enough] · [whereat] · [zenbench] · [cargo-copter]

[And other projects](https://www.imazen.io/open-source) · [GitHub @imazen](https://github.com/imazen) · [GitHub @lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith) · [NuGet](https://www.nuget.org/profiles/imazen) (over 30 million downloads / 87 packages)

## License

Dual-licensed: [AGPL-3.0](LICENSE-AGPL3) or [commercial](LICENSE-COMMERCIAL).

I've maintained and developed open-source image server software — and the 40+
library ecosystem it depends on — full-time since 2011. Fifteen years of
continual maintenance, backwards compatibility, support, and the (very rare)
security patch. That kind of stability requires sustainable funding, and
dual-licensing is how we make it work without venture capital or rug-pulls.
Support sustainable and secure software; swap patch tuesday for patch leap-year.

[Our open-source products](https://www.imazen.io/open-source)

**Your options:**

- **Startup license** — $1 if your company has under $1M revenue and fewer
  than 5 employees. [Get a key →](https://www.imazen.io/pricing)
- **Commercial subscription** — Governed by the Imazen Site-wide Subscription
  License v1.1 or later. Apache 2.0-like terms, no source-sharing requirement.
  Sliding scale by company size.
  [Pricing & 60-day free trial →](https://www.imazen.io/pricing)
- **AGPL v3** — Free and open. Share your source if you distribute.

See [LICENSE-COMMERCIAL](LICENSE-COMMERCIAL) for details.

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
[zenjxl]: https://github.com/imazen/zenjxl
[zentiff]: https://github.com/imazen/zentiff
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic-decoder-rs
[zenraw]: https://github.com/imazen/zenraw
[zenpdf]: https://github.com/imazen/zenpdf
[ultrahdr]: https://github.com/imazen/ultrahdr
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenrav1e]: https://github.com/imazen/zenrav1e
[mozjpeg-rs]: https://github.com/imazen/mozjpeg-rs
[zenavif-parse]: https://github.com/imazen/zenavif-parse
[zenavif-serialize]: https://github.com/imazen/zenavif-serialize
[webpx]: https://github.com/imazen/webpx
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenresize]: https://github.com/imazen/zenresize
[zenfilters]: https://github.com/imazen/zenfilters
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zensim]: https://github.com/imazen/zensim
[fast-ssim2]: https://github.com/imazen/fast-ssim2
[butteraugli]: https://github.com/imazen/butteraugli
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zenlayout]: https://github.com/imazen/zenlayout
[zennode]: https://github.com/imazen/zennode
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-server
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
[ImageResizer]: https://github.com/imazen/resizer
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[zenbench]: https://github.com/imazen/zenbench
[cargo-copter]: https://github.com/imazen/cargo-copter
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[codec-eval]: https://github.com/imazen/codec-eval
[codec-corpus]: https://github.com/imazen/codec-corpus
