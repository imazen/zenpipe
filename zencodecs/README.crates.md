<!-- GENERATED FROM README.md by zenutils gen-readme-crates.sh — DO NOT EDIT. -->

# zencodecs

Unified image codec dispatch for Rust — one detect-decode-encode API over the
format-specific zen codecs:
[zenjpeg](https://github.com/imazen/zenjpeg),
[zenpng](https://github.com/imazen/zenpng),
[zenwebp](https://github.com/imazen/zenwebp),
[zengif](https://github.com/imazen/zengif),
[zenavif](https://github.com/imazen/zenavif),
[zenjxl](https://github.com/imazen/zenjxl),
[heic](https://github.com/imazen/heic),
[zentiff](https://github.com/imazen/zentiff),
[zenbitmaps](https://github.com/imazen/zenbitmaps),
[zenraw](https://github.com/imazen/zenraw), and
[zenpdf](https://github.com/imazen/zenpdf).
Pure Rust, `#![forbid(unsafe_code)]`, `no_std + alloc`.

Part of the [zenpipe](https://github.com/imazen/zenpipe) monorepo (its standalone
repository now redirects here).

## Quick start

```toml
[dependencies]
# Enable only the codecs you need:
zencodecs = { version = "0.1.0", features = ["jpeg", "webp", "png"] }
```

```rust
use zencodecs::{ImageFormat, DecodeRequest, EncodeRequest, PixelBufferConvertExt};
use zencodecs::pixel::{ImgVec, Rgba};

// Detect format from magic bytes and decode the full first frame:
let data: &[u8] = todo!(); // your image bytes
let decoded = DecodeRequest::new(data).decode_full_frame()?;
println!("{}x{}", decoded.width(), decoded.height());

// Convert to RGBA8 for processing:
let rgba = decoded.into_buffer().to_rgba8();

// Encode typed pixels as WebP:
let pixels = ImgVec::new(vec![Rgba { r: 0u8, g: 0, b: 0, a: 255 }; 100 * 100], 100, 100);
let webp = EncodeRequest::new(ImageFormat::WebP)
    .with_quality(85.0)
    .encode_rgba8(pixels.as_ref())?;
println!("encoded {} bytes", webp.len());
# Ok::<(), whereat::At<zencodecs::CodecError>>(())
```

### Typed encode methods

Each encode method takes a typed `ImgRef<P>`; the dispatch layer converts to
whatever the codec needs natively:

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

### Probing

```rust
use zencodecs::{from_bytes, AllowedFormats};

let info = from_bytes(data, &AllowedFormats::all())?;
println!("{:?} {}x{}", info.format, info.width, info.height);
# Ok::<(), whereat::At<zencodecs::CodecError>>(())
```

### Runtime codec control

Compile-time features decide which codecs are *available*; an `AllowedFormats`
set decides which are *enabled* for a given request — so an image proxy can
restrict codecs per request:

```rust
use zencodecs::{AllowedFormats, ImageFormat, DecodeRequest};

let registry = AllowedFormats::none()
    .with_decode(ImageFormat::Jpeg, true)
    .with_decode(ImageFormat::WebP, true);

let decoded = DecodeRequest::new(data)
    .with_registry(&registry)
    .decode_full_frame()?;
# Ok::<(), whereat::At<zencodecs::CodecError>>(())
```

### Format-specific config

```rust
use zencodecs::{EncodeRequest, ImageFormat};
use zencodecs::config::CodecConfig;

let config = CodecConfig::default();
// .with_jpeg_encoder(...) / .with_avif_speed(...) / etc.

let request = EncodeRequest::new(ImageFormat::Jpeg)
    .with_codec_config(&config)
    .with_quality(92.0);
```

### Cooperative cancellation and limits

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
    .decode_full_frame()?;
# Ok::<(), whereat::At<zencodecs::CodecError>>(())
```

Stop tokens (`enough::Stop`) are forwarded to codecs that support cooperative cancellation.

## What this crate does

- Format detection from magic bytes
- Image probing (dimensions, format, color info) without a full decode
- Typed pixel-buffer encode/decode with automatic format negotiation
- Runtime codec registry (`AllowedFormats`) for per-request codec control
- Resource limits and cooperative cancellation forwarded to codecs
- Format auto-selection and a coefficient-domain transcode surface (JPEG↔JXL, JPEG→JPEG recompress)
- Format-specific codec configuration via `CodecConfig`
- Optional ICC color management via [moxcms](https://github.com/awxkee/moxcms) (`cms` feature)

It does **not** do image processing (resize, crop, rotate) — that lives in
[zenpipe](https://github.com/imazen/zenpipe) / [zenresize](https://github.com/imazen/zenresize) /
[zenfilters](https://github.com/imazen/zenfilters).

## Features

Every codec is feature-gated. Enable only what you need:

| Feature | Codec / role | Decode | Encode | Notes |
|---------|--------------|--------|--------|-------|
| `jpeg` | zenjpeg | Yes | Yes | |
| `jpeg-ultrahdr` | zenjpeg | Yes | Yes | UltraHDR gain-map support |
| `webp` | zenwebp | Yes | Yes | |
| `gif` | zengif | Yes | Yes | |
| `gif-zenquant` | zengif + zenquant | Yes | Yes | Palette quantization (zenquant) |
| `gif-quantizr` | zengif + quantizr | Yes | Yes | Palette quantization (quantizr) |
| `gif-imagequant` | zengif + imagequant | Yes | Yes | Palette quantization (imagequant) |
| `png` | zenpng | Yes | Yes | |
| `png-zenquant` | zenpng + zenquant | Yes | Yes | Palette quantization |
| `avif-decode` | zenavif | Yes | No | |
| `avif-encode` | zenavif | No | Yes | |
| `jxl-decode` | zenjxl | Yes | No | |
| `jxl-encode` | zenjxl | No | Yes | |
| `jxl-jpeg-reconstruct` | zenjxl-decoder | Yes | — | Lossless JXL→JPEG reconstruction (JBRD / brunsli-parity) |
| `jpeg-jxl-transcode` | zenjxl | — | — | Lossless byte-exact JPEG→JXL transcode (+ reconstruct) |
| `transcode-iqa` | zensim + zenjxl | — | — | Quality-targeted transcode (`transcode_to_quality`) |
| `heic-decode` | heic | Yes | No | Pure-Rust HEIC/HEIF (base SDR + gain-map HDR) |
| `tiff` | zentiff | Yes | Yes | |
| `bitmaps` | zenbitmaps | Yes | Yes | PNM/PAM/PFM, BMP, farbfeld |
| `bitmaps-bmp` | zenbitmaps | Yes | Yes | BMP only |
| `raw-decode` | zenraw | Yes | No | RAW/DNG via rawloader (LGPL) |
| `raw-decode-exif` | zenraw | Yes | No | EXIF metadata for RAW/DNG |
| `raw-decode-xmp` | zenraw | Yes | No | XMP metadata for RAW/DNG |
| `raw-decode-gainmap` | zenraw | Yes | No | Gain map from DNG/AMPF |
| `pdf-decode` | zenpdf | Yes | No | Render first PDF page to RGBA8 (hayro engine) |
| `cms` | moxcms | — | — | ICC color management (alias: `moxcms`) |
| `picker` / `picker-api` | zenpicker | — | — | Content-aware auto-format selection (MLP over zenanalyze features) |
| `riapi` | — | — | — | RIAPI codec-key parsing |
| `zennode` | zennode | — | — | Pipeline node definitions |
| `calibrate` | (meta) | — | — | All lossy encoders, for quality calibration |
| `all` | (meta) | Yes | Yes | All shipping codecs and metadata features |

Default features: `jpeg`, `webp`, `gif`, `gif-zenquant`, `png`, `png-zenquant`,
`avif-decode`, `avif-encode`, `jxl-decode`, `bitmaps-bmp`.

The `svg`, `jp2-decode`, `bitmaps-hdr`, `bitmaps-qoi`, and `bitmaps-tga` features
are reserved: enabling one produces a single `compile_error!` naming the backend
that has not been wired in yet.

## License

Dual-licensed: [AGPL-3.0](https://github.com/imazen/zenpipe/blob/main/LICENSE-AGPL3) or [commercial](https://github.com/imazen/zenpipe/blob/main/LICENSE-COMMERCIAL).

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

See [LICENSE-COMMERCIAL](https://github.com/imazen/zenpipe/blob/main/LICENSE-COMMERCIAL) for details.

## Image tech I maintain

| | |
|:--|:--|
| **Codecs** ¹ | [zenjpeg] · [zenpng] · [zenwebp] · [zengif] · [zenavif] · [zenjxl] · [zenbitmaps] · [heic] · [zentiff] · [zenpdf] · [zensvg] · [zenjp2] · [zenraw] · [ultrahdr] |
| Codec internals | [zenjxl-decoder] · [jxl-encoder] · [zenrav1e] · [rav1d-safe] · [zenavif-parse] · [zenavif-serialize] |
| Compression | [zenflate] · [zenzop] · [zenzstd] |
| Processing | [zenresize] · [zenquant] · [zenblend] · [zenfilters] · [zensally] · [zentone] |
| Pixels & color | [zenpixels] · [zenpixels-convert] · [linear-srgb] · [garb] |
| Pipeline & framework | [zenpipe] · [zencodec] · **zencodecs** · [zenlayout] · [zennode] · [zenwasm] · [zentract] |
| Metrics | [zensim] · [fast-ssim2] · [butteraugli] · [zenmetrics] · [resamplescope-rs] |
| Pickers & ML | [zenanalyze] · [zenpredict] · [zenpicker] |
| Products | [Imageflow] image engine ([.NET][imageflow-dotnet] · [Node][imageflow-node] · [Go][imageflow-go]) · [Imageflow Server] · [ImageResizer] (C#) |

<sub>¹ pure-Rust, `#![forbid(unsafe_code)]` codecs, as of 2026</sub>

### General Rust awesomeness

[zenbench] · [archmage] · [magetypes] · [enough] · [whereat] · [cargo-copter]

[Open source](https://www.imazen.io/open-source) · [@imazen](https://github.com/imazen) · [@lilith](https://github.com/lilith) · [lib.rs/~lilith](https://lib.rs/~lilith)

[zenjpeg]: https://github.com/imazen/zenjpeg
[zenpng]: https://github.com/imazen/zenpng
[zenwebp]: https://github.com/imazen/zenwebp
[zengif]: https://github.com/imazen/zengif
[zenavif]: https://github.com/imazen/zenavif
[zenjxl]: https://github.com/imazen/zenjxl
[zenbitmaps]: https://github.com/imazen/zenbitmaps
[heic]: https://github.com/imazen/heic
[zentiff]: https://github.com/imazen/zentiff
[zenpdf]: https://github.com/imazen/zenpdf
[zensvg]: https://github.com/imazen/zenextras
[zenjp2]: https://github.com/imazen/zenextras
[zenraw]: https://github.com/imazen/zenraw
[ultrahdr]: https://github.com/imazen/ultrahdr
[zenjxl-decoder]: https://github.com/imazen/zenjxl-decoder
[jxl-encoder]: https://github.com/imazen/jxl-encoder
[zenrav1e]: https://github.com/imazen/zenrav1e
[rav1d-safe]: https://github.com/imazen/rav1d-safe
[zenavif-parse]: https://github.com/imazen/zenavif-parse
[zenavif-serialize]: https://github.com/imazen/zenavif-serialize
[zenflate]: https://github.com/imazen/zenflate
[zenzop]: https://github.com/imazen/zenzop
[zenzstd]: https://github.com/imazen/zenzstd
[zenresize]: https://github.com/imazen/zenresize
[zenquant]: https://github.com/imazen/zenquant
[zenblend]: https://github.com/imazen/zenblend
[zenfilters]: https://github.com/imazen/zenfilters
[zensally]: https://github.com/imazen/zensally
[zentone]: https://github.com/imazen/zentone
[zenpixels]: https://github.com/imazen/zenpixels
[zenpixels-convert]: https://github.com/imazen/zenpixels
[linear-srgb]: https://github.com/imazen/linear-srgb
[garb]: https://github.com/imazen/garb
[zenpipe]: https://github.com/imazen/zenpipe
[zencodec]: https://github.com/imazen/zencodec
[zenlayout]: https://github.com/imazen/zenlayout
[zennode]: https://github.com/imazen/zennode
[zenwasm]: https://github.com/imazen/zenwasm
[zentract]: https://github.com/imazen/zentract
[zensim]: https://github.com/imazen/zensim
[fast-ssim2]: https://github.com/imazen/fast-ssim2
[butteraugli]: https://github.com/imazen/butteraugli
[zenmetrics]: https://github.com/imazen/zenmetrics
[resamplescope-rs]: https://github.com/imazen/resamplescope-rs
[zenanalyze]: https://github.com/imazen/zenanalyze
[zenpredict]: https://github.com/imazen/zenanalyze
[zenpicker]: https://github.com/imazen/zenanalyze
[zenbench]: https://github.com/imazen/zenbench
[archmage]: https://github.com/imazen/archmage
[magetypes]: https://github.com/imazen/archmage
[enough]: https://github.com/imazen/enough
[whereat]: https://github.com/lilith/whereat
[cargo-copter]: https://github.com/imazen/cargo-copter
[Imageflow]: https://github.com/imazen/imageflow
[Imageflow Server]: https://github.com/imazen/imageflow-dotnet-server
[ImageResizer]: https://github.com/imazen/resizer
[imageflow-dotnet]: https://github.com/imazen/imageflow-dotnet
[imageflow-node]: https://github.com/imazen/imageflow-node
[imageflow-go]: https://github.com/imazen/imageflow-go
