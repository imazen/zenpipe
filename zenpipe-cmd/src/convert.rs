//! Convert CLI arguments into zennode NodeInstance list and CodecIntent.
//!
//! Strategy: build an RIAPI querystring from CLI flags and let
//! `full_registry().from_querystring()` parse it into nodes. This avoids
//! duplicating parameter mapping logic. For flags without RIAPI equivalents,
//! create nodes directly via NodeDef::create().

use crate::args::{Operations, OutputOptions};
use crate::error::CliError;
use zencodecs::{CodecIntent, FormatChoice};
use zennode::NodeInstance;

/// Build pipeline nodes from CLI operations.
pub fn ops_to_nodes(ops: &Operations) -> Result<Vec<Box<dyn NodeInstance>>, CliError> {
    let mut qs_parts: Vec<String> = Vec::new();

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
            "auto" => {
                // Auto-deskew: not yet mapped to RIAPI; skip for now.
            }
            _ => {
                // Arbitrary rotation not yet in RIAPI; skip for now.
            }
        }
    }

    // ── Crop ──
    if let Some(ref crop) = ops.crop {
        if crop == "auto" {
            qs_parts.push("trim.threshold=80".into());
        } else if crop.contains('%') {
            // Percentage crop: parse x%,y%,w%,h%
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
            // Pixel crop: x,y,w,h
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
        // Pad values: single or top,right,bottom,left
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
    // Exposure maps to s.brightness (the RIAPI key for zenfilters.exposure)
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

    // Report querystring parsing warnings as stderr hints.
    for w in &result.warnings {
        eprintln!("warning: querystring: {}", w.message);
    }

    let mut nodes = result.instances;

    // ── Filters created directly as node instances ──
    // These don't have RIAPI keys, so we create them via ParamMap.
    let registry = zenpipe::full_registry();

    if let Some(v) = ops.brightness {
        // brightness is a CSS-like -100..100 linear offset.
        // Map it to exposure stops: brightness/50 gives a rough mapping.
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
            v.abs(), // highlight_recovery takes 0..1 strength
        )?);
    }
    if let Some(v) = ops.shadows {
        nodes.push(create_filter_node(
            &registry,
            "zenfilters.shadow_lift",
            "strength",
            v.abs(), // shadow_lift takes 0..1 strength
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
    // Auto enhance = auto_exposure + auto_levels + clarity + vibrance
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

    Ok(nodes)
}

/// Create a filter node with a single f32 parameter.
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

/// Create a filter node with default parameters (for boolean-style filters like auto_exposure).
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
    // Percentage: "50%"
    if let Some(pct) = value.strip_suffix('%') {
        let _: f32 = pct
            .parse()
            .map_err(|_| CliError::Operation(format!("resize: invalid percentage '{value}'")))?;
        // RIAPI doesn't have a direct percentage mode; compute later.
        // For now, use zoom.
        qs.push(format!("zoom={}", pct.parse::<f32>().unwrap() / 100.0));
        return Ok(());
    }

    // Suffixed modes: WxH!, WxH^, WxH#
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

/// Build a CodecIntent from output options and the output file extension.
pub fn build_intent(
    output_ext: &str,
    output_opts: &OutputOptions,
) -> Result<CodecIntent, CliError> {
    let format_registry = zencodec::ImageFormatRegistry::common();

    let format = if output_ext == "-" {
        // stdout: caller must specify format or we default to JPEG.
        None
    } else {
        let fmt = format_registry.from_extension(output_ext).ok_or_else(|| {
            CliError::Input(format!(
                "unknown output format for extension '.{output_ext}'"
            ))
        })?;
        Some(FormatChoice::Specific(fmt))
    };

    let mut intent = CodecIntent {
        format,
        quality_fallback: output_opts.quality,
        lossless: if output_opts.lossless {
            Some(zencodecs::BoolKeep::True)
        } else {
            None
        },
        ..CodecIntent::default()
    };

    // Codec-specific hints via per-format BTreeMaps.
    if let Some(ref sub) = output_opts.jpeg_subsampling {
        intent.hints.jpeg.insert("subsampling".into(), sub.clone());
    }
    if output_opts.jpeg_progressive {
        intent
            .hints
            .jpeg
            .insert("progressive".into(), "true".into());
    }
    if let Some(dist) = output_opts.jxl_distance {
        intent.hints.jxl.insert("distance".into(), dist.to_string());
    }
    if let Some(speed) = output_opts.avif_speed {
        intent.hints.avif.insert("speed".into(), speed.to_string());
    }
    if let Some(effort) = output_opts.effort {
        // Push effort to all codec hint maps.
        let effort_str = effort.to_string();
        intent
            .hints
            .jpeg
            .insert("effort".into(), effort_str.clone());
        intent.hints.png.insert("effort".into(), effort_str.clone());
        intent
            .hints
            .webp
            .insert("effort".into(), effort_str.clone());
        intent
            .hints
            .avif
            .insert("effort".into(), effort_str.clone());
        intent.hints.jxl.insert("effort".into(), effort_str.clone());
        intent.hints.gif.insert("effort".into(), effort_str);
    }

    Ok(intent)
}

/// Build metadata policy from output options.
pub fn metadata_policy(output_opts: &OutputOptions) -> zenpipe::job::MetadataPolicy {
    if output_opts.preserve {
        return zenpipe::job::MetadataPolicy::PreserveAll;
    }
    // --strip with no argument or --strip all → StripAll
    if let Some(ref what) = output_opts.strip {
        match what.as_deref() {
            None | Some("all") => return zenpipe::job::MetadataPolicy::StripAll,
            Some("exif") => return zenpipe::job::MetadataPolicy::WebDefault,
            Some("icc") => return zenpipe::job::MetadataPolicy::StripAll,
            _ => {}
        }
    }
    // --keep implies preserve what's listed, strip the rest.
    if output_opts.keep.is_some() {
        return zenpipe::job::MetadataPolicy::WebDefault;
    }
    zenpipe::job::MetadataPolicy::WebDefault
}

/// Build gain map mode from output options.
pub fn gain_map_mode(output_opts: &OutputOptions) -> zenpipe::job::GainMapMode {
    match output_opts.hdr.as_deref() {
        Some("strip") => zenpipe::job::GainMapMode::Discard,
        Some("preserve") | None => zenpipe::job::GainMapMode::Preserve,
        Some("tonemap") => zenpipe::job::GainMapMode::Preserve, // TODO: actual tonemap mode
        Some("reconstruct") => zenpipe::job::GainMapMode::Preserve, // TODO: reconstruct mode
        _ => zenpipe::job::GainMapMode::Preserve,
    }
}

/// Build the ZenFiltersConverter for the pipeline bridge.
pub struct ZenFiltersConverter;

impl zenpipe::bridge::NodeConverter for ZenFiltersConverter {
    fn can_convert(&self, schema_id: &str) -> bool {
        zenfilters::zennode_defs::is_zenfilters_node(schema_id)
    }

    fn convert(&self, node: &dyn NodeInstance) -> zenpipe::PipeResult<zenpipe::graph::NodeOp> {
        let filter = zenfilters::zennode_defs::node_to_filter(node).ok_or_else(|| {
            zenpipe::PipeError::Op(format!(
                "zenfilters converter: unrecognized node '{}'",
                node.schema().id
            ))
        })?;

        let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default())
            .map_err(|e| {
                zenpipe::PipeError::Op(format!("zenfilters pipeline creation failed: {e:?}"))
            })?;
        pipeline.push(filter);
        Ok(zenpipe::graph::NodeOp::Filter(pipeline))
    }

    fn convert_group(
        &self,
        nodes: &[&dyn NodeInstance],
    ) -> zenpipe::PipeResult<zenpipe::graph::NodeOp> {
        let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default())
            .map_err(|e| {
                zenpipe::PipeError::Op(format!("zenfilters pipeline creation failed: {e:?}"))
            })?;

        for node in nodes {
            let filter = zenfilters::zennode_defs::node_to_filter(*node).ok_or_else(|| {
                zenpipe::PipeError::Op(format!(
                    "zenfilters converter: unrecognized node '{}'",
                    node.schema().id
                ))
            })?;
            pipeline.push(filter);
        }

        Ok(zenpipe::graph::NodeOp::Filter(pipeline))
    }

    fn fuse_group(
        &self,
        nodes: &[&dyn NodeInstance],
    ) -> zenpipe::PipeResult<Option<zenpipe::graph::NodeOp>> {
        if nodes.len() < 2 {
            return Ok(None);
        }

        let mut pipeline = zenfilters::Pipeline::new(zenfilters::PipelineConfig::default())
            .map_err(|e| {
                zenpipe::PipeError::Op(format!("zenfilters pipeline creation failed: {e:?}"))
            })?;

        for node in nodes {
            if let Some(filter) = zenfilters::zennode_defs::node_to_filter(*node) {
                pipeline.push(filter);
            } else {
                return Ok(None);
            }
        }

        Ok(Some(zenpipe::graph::NodeOp::Filter(pipeline)))
    }
}
