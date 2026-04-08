//! Convert CLI arguments into zennode NodeInstance list and ImageJob configuration.
//!
//! Strategy: build an RIAPI querystring from CLI flags and let
//! `full_registry().from_querystring()` parse it into nodes. For flags
//! without RIAPI equivalents, create nodes directly via NodeDef::create().

use crate::args::{Operations, OutputOptions};
use crate::error::CliError;
use zennode::NodeInstance;

/// Result of converting CLI operations to pipeline nodes.
pub struct ConvertedOps {
    /// Pipeline nodes.
    pub nodes: Vec<Box<dyn NodeInstance>>,
    /// Secondary input files to add to ImageJob (io_id, file_path).
    pub extra_inputs: Vec<(i32, String)>,
}

/// Build pipeline nodes from CLI operations.
pub fn ops_to_nodes(ops: &Operations) -> Result<ConvertedOps, CliError> {
    let mut extra_inputs: Vec<(i32, String)> = Vec::new();
    let mut qs_parts: Vec<String> = Vec::new();
    let mut need_deskew = ops.deskew;
    let mut arbitrary_rotate: Option<String> = None;

    // If --qs was provided, start with that.
    if let Some(ref qs) = ops.qs {
        qs_parts.push(qs.clone());
    }

    // ── Orientation (before geometry) ──
    if let Some(ref orient) = ops.orient {
        match orient.as_str() {
            "auto" => qs_parts.push("autorotate=true".into()),
            v => qs_parts.push(format!("srotate={v}")),
        }
    }

    // ── Flip ──
    if let Some(ref flip) = ops.flip {
        qs_parts.push(format!("sflip={flip}"));
    }

    // ── Rotate ──
    if let Some(ref rotate) = ops.rotate {
        match rotate.as_str() {
            "90" => qs_parts.push("srotate=90".into()),
            "180" => qs_parts.push("srotate=180".into()),
            "270" => qs_parts.push("srotate=270".into()),
            "auto" => need_deskew = true,
            _ => {
                // Arbitrary angle — handled below as zenfilters.rotate node.
                arbitrary_rotate = Some(rotate.clone());
            }
        }
    }

    // ── Crop ──
    if let Some(ref crop) = ops.crop {
        if crop == "auto" {
            qs_parts.push("trim.threshold=80".into());
        } else if crop.contains('%') {
            let values: Vec<&str> = crop.split(',').collect();
            if values.len() == 4 {
                qs_parts.push(format!(
                    "crop={},{},{},{}",
                    values[0].trim_end_matches('%'),
                    values[1].trim_end_matches('%'),
                    values[2].trim_end_matches('%'),
                    values[3].trim_end_matches('%')
                ));
            } else {
                return Err(CliError::Operation(format!(
                    "crop: expected x%,y%,w%,h%, got '{crop}'"
                )));
            }
        } else {
            qs_parts.push(format!("crop={crop}"));
        }
    }

    // ── Resize ──
    if let Some(ref resize) = ops.resize {
        parse_resize(resize, &mut qs_parts)?;
    }

    // ── Resampling filter ──
    if let Some(ref filter) = ops.filter {
        qs_parts.push(format!("down.filter={filter}"));
        qs_parts.push(format!("up.filter={filter}"));
    }

    // ── Padding ──
    if let Some(ref pad) = ops.pad {
        let parts: Vec<&str> = pad.split(',').collect();
        match parts.len() {
            1 => {
                let v = parts[0];
                qs_parts.push(format!("s.pad.top={v}"));
                qs_parts.push(format!("s.pad.right={v}"));
                qs_parts.push(format!("s.pad.bottom={v}"));
                qs_parts.push(format!("s.pad.left={v}"));
            }
            4 => {
                qs_parts.push(format!("s.pad.top={}", parts[0]));
                qs_parts.push(format!("s.pad.right={}", parts[1]));
                qs_parts.push(format!("s.pad.bottom={}", parts[2]));
                qs_parts.push(format!("s.pad.left={}", parts[3]));
            }
            _ => {
                return Err(CliError::Operation(format!(
                    "pad: expected single value or top,right,bottom,left, got '{pad}'"
                )));
            }
        }
    }

    if let Some(ref bg) = ops.bg {
        qs_parts.push(format!("bgcolor={bg}"));
    }

    // ── Filters with RIAPI keys ──
    if let Some(v) = ops.exposure {
        qs_parts.push(format!("s.brightness={v}"));
    }
    if let Some(v) = ops.contrast {
        qs_parts.push(format!("s.contrast={v}"));
    }
    if let Some(v) = ops.saturation {
        qs_parts.push(format!("s.saturation={v}"));
    }
    if ops.grayscale {
        qs_parts.push("s.grayscale=true".into());
    }
    if let Some(v) = ops.sepia {
        qs_parts.push(format!("s.sepia={v}"));
    }
    if ops.invert {
        qs_parts.push("s.invert=true".into());
    }

    // Build nodes from the combined querystring.
    let qs = qs_parts.join("&");
    let registry = zenpipe::full_registry();

    let result = if qs.is_empty() {
        zennode::KvResult {
            instances: Vec::new(),
            warnings: Vec::new(),
        }
    } else {
        registry.from_querystring(&qs)
    };

    for w in &result.warnings {
        eprintln!("warning: querystring: {}", w.message);
    }

    let mut nodes = result.instances;

    // ── Filters without RIAPI keys — created directly via NodeDef::create() ──
    if let Some(v) = ops.brightness {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.exposure",
            "stops",
            v / 50.0,
        )?);
    }
    if let Some(v) = ops.vibrance {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.vibrance",
            "amount",
            v,
        )?);
    }
    if let Some(v) = ops.temperature {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.temperature",
            "shift",
            v,
        )?);
    }
    if let Some(v) = ops.tint {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.tint",
            "shift",
            v,
        )?);
    }
    if let Some(v) = ops.clarity {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.clarity",
            "amount",
            v,
        )?);
    }
    if let Some(v) = ops.blur {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.blur",
            "sigma",
            v,
        )?);
    }
    if let Some(v) = ops.denoise {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.noise_reduction",
            "luminance",
            v,
        )?);
    }
    if let Some(v) = ops.highlights {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.highlight_recovery",
            "strength",
            v.abs(),
        )?);
    }
    if let Some(v) = ops.shadows {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.shadow_lift",
            "strength",
            v.abs(),
        )?);
    }
    if let Some(v) = ops.black_point {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.black_point",
            "level",
            v,
        )?);
    }
    if let Some(v) = ops.white_point {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.white_point",
            "level",
            v,
        )?);
    }
    if let Some(v) = ops.vignette {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.vignette",
            "strength",
            v,
        )?);
    }
    if let Some(v) = ops.dehaze {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.dehaze",
            "strength",
            v,
        )?);
    }
    if let Some(v) = ops.grain {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.grain",
            "amount",
            v,
        )?);
    }
    if let Some(ref sharpen) = ops.sharpen {
        let amount: f32 = sharpen
            .split(',')
            .next()
            .unwrap_or(sharpen)
            .parse()
            .map_err(|_| CliError::Operation(format!("sharpen: invalid value '{sharpen}'")))?;
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.sharpen",
            "amount",
            amount,
        )?);
    }
    if ops.auto_exposure {
        nodes.push(create_bool_filter_node(
            &registry,
            "zenfilters.auto_exposure",
        )?);
    }
    if ops.auto_levels {
        nodes.push(create_bool_filter_node(
            &registry,
            "zenfilters.auto_levels",
        )?);
    }
    if ops.auto_enhance {
        if ops.exposure.is_none() && !ops.auto_exposure {
            nodes.push(create_bool_filter_node(
                &registry,
                "zenfilters.auto_exposure",
            )?);
        }
        if !ops.auto_levels {
            nodes.push(create_bool_filter_node(
                &registry,
                "zenfilters.auto_levels",
            )?);
        }
        if ops.clarity.is_none() {
            nodes.push(create_filter_node(
                &registry,
                "zenfilters.clarity",
                "amount",
                0.3,
            )?);
        }
        if ops.vibrance.is_none() {
            nodes.push(create_filter_node(
                &registry,
                "zenfilters.vibrance",
                "amount",
                0.3,
            )?);
        }
    }

    // ── Auto white balance ──
    if ops.auto_wb {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.auto_white_balance",
            "strength",
            1.0,
        )?);
    }

    // ── Film look preset ──
    if let Some(ref preset_arg) = ops.preset {
        let (preset_name, strength) = if let Some((name, intensity)) = preset_arg.split_once(',') {
            let s: f32 = intensity.parse().map_err(|_| {
                CliError::Operation(format!("preset: invalid intensity '{intensity}'"))
            })?;
            (name, s)
        } else {
            (preset_arg.as_str(), 1.0)
        };
        let def = registry
            .get("zenfilters.film_look")
            .ok_or_else(|| CliError::Operation("film_look node not found in registry".into()))?;
        let mut params = zennode::ParamMap::new();
        params.insert(
            "preset".into(),
            zennode::ParamValue::Str(preset_name.into()),
        );
        params.insert("strength".into(), zennode::ParamValue::F32(strength));
        nodes.push(
            def.create(&params)
                .map_err(|e| CliError::Operation(format!("preset '{preset_name}': {e}")))?,
        );
    }

    // ── Colorspace (gamut expansion) ──
    if let Some(ref cs) = ops.colorspace {
        match cs.as_str() {
            "srgb" => {} // no-op, pipeline is sRGB by default
            "p3" => {
                nodes.push(create_filter_node(
                    &registry,
                    "zenfilters.gamut_expand",
                    "strength",
                    1.0,
                )?);
            }
            other => {
                return Err(CliError::Operation(format!(
                    "colorspace '{other}' not supported (use srgb or p3)"
                )));
            }
        }
    }

    // ── Deskew (arbitrary rotation with deskew mode) ──
    if need_deskew {
        // Deskew with mode=1 (white fill). Angle 0 = auto-detect is not
        // yet supported; this sets up the node for manual angle override.
        let def = registry
            .get("zenfilters.rotate")
            .ok_or_else(|| CliError::Operation("rotate node not found in registry".into()))?;
        let mut params = zennode::ParamMap::new();
        params.insert("angle".into(), zennode::ParamValue::F32(0.0));
        params.insert("mode".into(), zennode::ParamValue::I32(1)); // deskew mode
        nodes.push(
            def.create(&params)
                .map_err(|e| CliError::Operation(format!("deskew: {e}")))?,
        );
    }

    // ── Arbitrary rotation ──
    if let Some(ref angle_str) = arbitrary_rotate {
        let angle: f32 = angle_str
            .parse()
            .map_err(|_| CliError::Operation(format!("rotate: invalid angle '{angle_str}'")))?;
        let def = registry
            .get("zenfilters.rotate")
            .ok_or_else(|| CliError::Operation("rotate node not found in registry".into()))?;
        let mut params = zennode::ParamMap::new();
        params.insert("angle".into(), zennode::ParamValue::F32(angle));
        params.insert("mode".into(), zennode::ParamValue::I32(0)); // crop mode
        nodes.push(
            def.create(&params)
                .map_err(|e| CliError::Operation(format!("rotate {angle}: {e}")))?,
        );
    }

    // ── Clean-doc (composite pipeline: deskew + auto-crop + auto-levels) ──
    if ops.clean_doc {
        if !need_deskew {
            // Add deskew if not already added
            if let Some(def) = registry.get("zenfilters.rotate") {
                let mut params = zennode::ParamMap::new();
                params.insert("angle".into(), zennode::ParamValue::F32(0.0));
                params.insert("mode".into(), zennode::ParamValue::I32(1));
                if let Ok(node) = def.create(&params) {
                    nodes.push(node);
                }
            }
        }
        // Auto whitespace crop
        if ops.crop.is_none() {
            if let Some(def) = registry.get("zenpipe.crop_whitespace") {
                if let Ok(node) = def.create_default() {
                    nodes.push(node);
                }
            }
        }
        // Auto levels
        if !ops.auto_levels {
            nodes.push(create_bool_filter_node(
                &registry,
                "zenfilters.auto_levels",
            )?);
        }
    }

    // ── Overlay (secondary input) ──
    if let Some(ref overlay_arg) = ops.overlay {
        let parts: Vec<&str> = overlay_arg.split(',').collect();
        let path = parts
            .first()
            .ok_or_else(|| CliError::Operation("overlay: missing path".into()))?
            .to_string();
        let x: i32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let y: i32 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        let opacity: f32 = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(1.0);

        // Use io_id 100 for the overlay input (arbitrary, avoids conflict with 0/1).
        let overlay_io_id: i32 = 100;
        extra_inputs.push((overlay_io_id, path));

        let def = registry
            .get("zenpipe.overlay")
            .ok_or_else(|| CliError::Operation("overlay node not found in registry".into()))?;
        let mut params = zennode::ParamMap::new();
        params.insert("io_id".into(), zennode::ParamValue::I32(overlay_io_id));
        params.insert("x".into(), zennode::ParamValue::I32(x));
        params.insert("y".into(), zennode::ParamValue::I32(y));
        params.insert("opacity".into(), zennode::ParamValue::F32(opacity));
        nodes.push(
            def.create(&params)
                .map_err(|e| CliError::Operation(format!("overlay: {e}")))?,
        );
    }

    // ── Fill (solid color) ──
    if let Some(ref color) = ops.fill {
        let (r, g, b) = parse_color_rgb(color)?;
        let def = registry
            .get("zenpipe.fill_rect")
            .ok_or_else(|| CliError::Operation("fill_rect node not found in registry".into()))?;
        let mut params = zennode::ParamMap::new();
        params.insert("x1".into(), zennode::ParamValue::U32(0));
        params.insert("y1".into(), zennode::ParamValue::U32(0));
        params.insert("x2".into(), zennode::ParamValue::U32(65535));
        params.insert("y2".into(), zennode::ParamValue::U32(65535));
        params.insert("color_r".into(), zennode::ParamValue::U32(r as u32));
        params.insert("color_g".into(), zennode::ParamValue::U32(g as u32));
        params.insert("color_b".into(), zennode::ParamValue::U32(b as u32));
        params.insert("color_a".into(), zennode::ParamValue::U32(255));
        nodes.push(
            def.create(&params)
                .map_err(|e| CliError::Operation(format!("fill: {e}")))?,
        );
    }

    Ok(ConvertedOps {
        nodes,
        extra_inputs,
    })
}

/// Parse a color name or #RRGGBB hex to (r, g, b).
fn parse_color_rgb(s: &str) -> Result<(u8, u8, u8), CliError> {
    match s.to_lowercase().as_str() {
        "white" => Ok((255, 255, 255)),
        "black" => Ok((0, 0, 0)),
        "red" => Ok((255, 0, 0)),
        "green" => Ok((0, 128, 0)),
        "blue" => Ok((0, 0, 255)),
        "transparent" => Ok((0, 0, 0)),
        hex if hex.starts_with('#') && hex.len() == 7 => {
            let r = u8::from_str_radix(&hex[1..3], 16)
                .map_err(|_| CliError::Operation(format!("invalid color '{s}'")))?;
            let g = u8::from_str_radix(&hex[3..5], 16)
                .map_err(|_| CliError::Operation(format!("invalid color '{s}'")))?;
            let b = u8::from_str_radix(&hex[5..7], 16)
                .map_err(|_| CliError::Operation(format!("invalid color '{s}'")))?;
            Ok((r, g, b))
        }
        _ => Err(CliError::Operation(format!(
            "unknown color '{s}' (use a name or #RRGGBB)"
        ))),
    }
}

fn create_filter_node(
    registry: &zennode::NodeRegistry,
    node_id: &str,
    param_name: &str,
    value: f32,
) -> Result<Box<dyn NodeInstance>, CliError> {
    let def = registry
        .get(node_id)
        .ok_or_else(|| CliError::Operation(format!("filter '{node_id}' not found in registry")))?;
    let mut params = zennode::ParamMap::new();
    params.insert(param_name.into(), zennode::ParamValue::F32(value));
    def.create(&params)
        .map_err(|e| CliError::Operation(format!("filter '{node_id}': {e}")))
}

fn create_bool_filter_node(
    registry: &zennode::NodeRegistry,
    node_id: &str,
) -> Result<Box<dyn NodeInstance>, CliError> {
    let def = registry
        .get(node_id)
        .ok_or_else(|| CliError::Operation(format!("filter '{node_id}' not found in registry")))?;
    def.create_default()
        .map_err(|e| CliError::Operation(format!("filter '{node_id}': {e}")))
}

/// Parse --resize value into RIAPI querystring parts.
fn parse_resize(value: &str, qs: &mut Vec<String>) -> Result<(), CliError> {
    if let Some(pct) = value.strip_suffix('%') {
        let _: f32 = pct
            .parse()
            .map_err(|_| CliError::Operation(format!("resize: invalid percentage '{value}'")))?;
        qs.push(format!("zoom={}", pct.parse::<f32>().unwrap() / 100.0));
        return Ok(());
    }

    let (dims, mode) = if let Some(d) = value.strip_suffix('!') {
        (d, Some("distort"))
    } else if let Some(d) = value.strip_suffix('^') {
        (d, Some("crop"))
    } else if let Some(d) = value.strip_suffix('#') {
        (d, Some("pad"))
    } else {
        (value, None)
    };

    // Parse WxH or just W. Width-only is valid — zenpipe handles it natively.
    if let Some((w_str, h_str)) = dims.split_once('x') {
        let w: u32 = w_str
            .parse()
            .map_err(|_| CliError::Operation(format!("resize: invalid width '{w_str}'")))?;
        let h: u32 = h_str
            .parse()
            .map_err(|_| CliError::Operation(format!("resize: invalid height '{h_str}'")))?;
        qs.push(format!("w={w}"));
        qs.push(format!("h={h}"));
    } else {
        let w: u32 = dims
            .parse()
            .map_err(|_| CliError::Operation(format!("resize: invalid width '{dims}'")))?;
        qs.push(format!("w={w}"));
    }

    if let Some(m) = mode {
        qs.push(format!("mode={m}"));
    }

    Ok(())
}

/// Configure an ImageJob's output settings from CLI output options.
pub fn apply_output_opts<'a>(
    job: zenpipe::job::ImageJob<'a>,
    output_ext: &str,
    opts: &OutputOptions,
) -> zenpipe::job::ImageJob<'a> {
    let mut job = job.with_output_extension(output_ext);

    if let Some(q) = opts.quality {
        job = job.with_quality(q);
    }
    if opts.lossless {
        job = job.with_lossless(true);
    }
    if let Some(effort) = opts.effort {
        job = job.with_codec_hint("effort", &effort.to_string());
    }

    // Codec-specific hints.
    if let Some(ref sub) = opts.jpeg_subsampling {
        job = job.with_codec_hint_for("jpeg", "subsampling", sub);
    }
    if opts.jpeg_progressive {
        job = job.with_codec_hint_for("jpeg", "progressive", "true");
    }
    if let Some(dist) = opts.jxl_distance {
        job = job.with_codec_hint_for("jxl", "distance", &dist.to_string());
    }
    if let Some(speed) = opts.avif_speed {
        job = job.with_codec_hint_for("avif", "speed", &speed.to_string());
    }

    // Metadata policy.
    job = job.with_metadata_policy(metadata_policy(opts));
    job = job.with_gain_map_mode(gain_map_mode(opts));

    job
}

fn metadata_policy(opts: &OutputOptions) -> zenpipe::job::MetadataPolicy {
    if opts.preserve {
        return zenpipe::job::MetadataPolicy::PreserveAll;
    }
    if let Some(ref what) = opts.strip {
        match what.as_deref() {
            None | Some("all") => return zenpipe::job::MetadataPolicy::StripAll,
            Some("exif") => return zenpipe::job::MetadataPolicy::WebDefault,
            Some("icc") => return zenpipe::job::MetadataPolicy::StripAll,
            _ => {}
        }
    }
    zenpipe::job::MetadataPolicy::WebDefault
}

fn gain_map_mode(opts: &OutputOptions) -> zenpipe::job::GainMapMode {
    match opts.hdr.as_deref() {
        Some("strip") => zenpipe::job::GainMapMode::Discard,
        _ => zenpipe::job::GainMapMode::Preserve,
    }
}
