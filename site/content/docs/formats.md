+++
title = "Supported Formats"
description = "Image formats supported by zenpipe — decode and encode capabilities"
weight = 10
+++

| Format | MIME Type | Extensions | Alpha | Lossless | Animation | Decode | Encode |
|--------|----------|------------|-------|----------|-----------|--------|--------|
| **JPEG** | `image/jpeg` | jpg, jpeg, jpe, jfif | -- | -- | -- | yes | yes |
| **PNG** | `image/png` | png | yes | yes | yes | yes | yes |
| **GIF** | `image/gif` | gif | yes | yes | yes | yes | yes |
| **WebP** | `image/webp` | webp | yes | yes | yes | yes | yes |
| **AVIF** | `image/avif` | avif | yes | yes | yes | yes | -- |
| **JPEG XL** | `image/jxl` | jxl | yes | yes | yes | yes | -- |
| **HEIC** | `image/heif` | heic, heif, hif | yes | -- | -- | -- | -- |
| **BMP** | `image/bmp` | bmp | yes | yes | -- | yes | yes |
| **TIFF** | `image/tiff` | tiff, tif | yes | yes | -- | yes | yes |
| **PNM** | `image/x-portable-anymap` | pnm, ppm, pgm, pbm, pam, pfm | yes | yes | -- | yes | yes |
| **farbfeld** | `image/x-farbfeld` | ff | yes | yes | -- | yes | yes |
| **QOI** | `image/x-qoi` | qoi | yes | yes | -- | -- | -- |
| **HDR** | `image/vnd.radiance` | hdr, rgbe, pic | -- | -- | -- | -- | -- |
| **TGA** | `image/x-tga` | tga, targa, icb, vda, vst | yes | yes | -- | -- | -- |

All codecs are pure Rust zen crates. Format auto-detection uses magic bytes, not file extensions.
