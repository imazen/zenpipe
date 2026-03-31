+++
title = "Zenpipe"
description = "Streaming pixel pipeline for Rust — zero-materialization image processing"
template = "landing.html"

[extra]
section_order = ["hero", "features", "easy_command", "final_cta"]

[extra.hero]
title = "Zenpipe"
description = "A streaming pixel pipeline that processes images row-by-row, keeping only the rows needed for the current kernel in memory. 65 composable nodes. 14 codecs. Pure safe Rust."
badge = "Rust · no_std + alloc · #![forbid(unsafe_code)]"
gradient_opacity = 15
cta_buttons = [
    { text = "Get Started", url = "/docs/", style = "primary" },
    { text = "Node Reference", url = "/nodes/", style = "secondary" },
]

[[extra.features]]
title = "Zero Materialization"
desc = "Pull-based DAG streams pixels through crop, resize, composite, filters, and ICC transforms without ever materializing the full image. Only the rows needed for the current kernel exist in memory."
icon = "fa-solid fa-memory"

[[extra.features]]
title = "65 Composable Nodes"
desc = "Resize with 31 filter kernels. 43 photo filters in perceptual Oklab. Porter-Duff compositing. Smart crop with face detection. Whitespace trimming. All wired into a single declarative pipeline."
icon = "fa-solid fa-diagram-project"

[[extra.features]]
title = "14 Image Formats"
desc = "JPEG, PNG, WebP, AVIF, JPEG XL, GIF, HEIC, TIFF, BMP, PNM, farbfeld, QOI, HDR, and TGA. Every codec is a pure-Rust zen crate with streaming decode and encode support."
icon = "fa-solid fa-images"

[[extra.features]]
title = "Pure Safe Rust"
desc = "#![forbid(unsafe_code)] throughout. no_std + alloc core with optional std features. SIMD acceleration via archmage capability tokens — safe by construction, not by audit."
icon = "fa-solid fa-shield-halved"

[[extra.features]]
title = "RIAPI Querystring API"
desc = "Drive the entire pipeline from a URL querystring: ?w=800&h=600&mode=crop&format=webp&qp=high. Drop-in compatible with Imageflow and ImageResizer's widely-deployed API."
icon = "fa-solid fa-link"

[[extra.features]]
title = "Perceptual Quality"
desc = "Filters operate in Oklab color space for perceptually uniform adjustments. Resize in linear light by default. Adaptive quantization. Quality presets from 'lowest' to 'lossless' that map to optimal codec settings."
icon = "fa-solid fa-eye"

[extra.easy_command_section]
title = "Quick Start"
description = "Add zenpipe to your project and start processing images."
tabs = [
    { name = "Cargo.toml", command = "[dependencies]\nzenpipe = { version = \"0.1\", features = [\"job\", \"nodes-all\"] }" },
    { name = "Querystring", command = "# Resize, crop to fill, encode as WebP\n?w=800&h=600&mode=crop&format=webp&qp=high" },
    { name = "docs.rs", link = "https://docs.rs/zenpipe" },
]

[extra.final_cta_section]
title = "Image Processing Without Compromise"
description = "Zenpipe is the engine behind Imageflow v3 — built for servers that process millions of images. Streaming architecture, safe Rust, and a battle-tested querystring API."
button = { text = "Read the Docs", url = "/docs/" }
+++
