//! Integration test: zennode bridge + JPEG codec round-trip.
//!
//! Validates the full path from zennode node instances through the bridge
//! compiler, streaming through a real JPEG encoder.
//!
//! Run: `cargo test --features zennode --test bridge_codec -- --nocapture`

#![cfg(feature = "zennode")]

use zennode::NodeDef;
use zencodec::decode::{DecodeJob, DecoderConfig};
use zenjpeg::JpegDecoderConfig;
use zenjpeg::encoder::ChromaSubsampling;

use zenpipe::bridge;
use zenpipe::codec::EncoderSink;
use zenpipe::sources::CallbackSource;
use zenpipe::{Source, execute, format};

/// Create a gradient source of the given dimensions.
fn gradient_source(w: u32, h: u32) -> Box<dyn Source> {
    let mut row_idx = 0u32;
    Box::new(CallbackSource::new(
        w,
        h,
        format::RGBA8_SRGB,
        16,
        move |buf| {
            if row_idx >= h {
                return Ok(false);
            }
            for x in 0..w as usize {
                let i = x * 4;
                buf[i] = (x * 255 / w as usize) as u8;
                buf[i + 1] = (row_idx * 255 / h) as u8;
                buf[i + 2] = 128;
                buf[i + 3] = 255;
            }
            row_idx += 1;
            Ok(true)
        },
    ))
}

/// Create an encoder sink for the given dimensions.
fn make_encoder_sink(w: u32, h: u32) -> EncoderSink<'static> {
    let enc_config = zenjpeg::JpegEncoderConfig::ycbcr(80.0, ChromaSubsampling::Quarter);
    let enc_job = {
        use zencodec::encode::EncoderConfig as _;
        enc_config.job()
    };
    let enc_job = zencodec::encode::EncodeJob::with_canvas_size(enc_job, w, h);
    let dyn_enc: Box<dyn zencodec::encode::DynEncoder + Send> = {
        let concrete = zencodec::encode::EncodeJob::encoder(enc_job).expect("encoder");
        Box::new(SendEncoderShim(concrete))
    };
    EncoderSink::new(dyn_enc, format::RGBA8_SRGB)
}

// ============================================================================
// Test: zennode bridge decode → constrain → encode round-trip
// ============================================================================

#[test]
fn bridge_decode_resize_encode_jpeg() {
    let src_w = 512u32;
    let src_h = 384u32;
    let dst_w = 200u32;
    let dst_h = 150u32;

    // 1. Build zennode nodes: [Decode, Constrain(w=200, h=150, mode=fit)]
    let decode_node = zennode::nodes::DECODE_NODE.create_default().unwrap();

    let mut constrain_params = zennode::ParamMap::new();
    constrain_params.insert("w".into(), zennode::ParamValue::U32(dst_w));
    constrain_params.insert("h".into(), zennode::ParamValue::U32(dst_h));
    constrain_params.insert("mode".into(), zennode::ParamValue::Str("fit".into()));
    constrain_params.insert("filter".into(), zennode::ParamValue::Str("lanczos".into()));
    let constrain_node = MockConstrainNode::boxed(constrain_params);

    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![decode_node, constrain_node];

    // 2. Create a gradient source at source dimensions.
    let source = gradient_source(src_w, src_h);

    // 3. Build pipeline via bridge.
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();

    // Verify decode config was extracted.
    assert_eq!(result.decode_config.hdr_mode, "sdr_only");

    // Pipeline output should be constrained to dst dimensions.
    assert_eq!(result.source.width(), dst_w);
    assert_eq!(result.source.height(), dst_h);

    // 4. Connect to encoder sink and execute.
    let mut pipeline_source = result.source;
    let mut sink = make_encoder_sink(dst_w, dst_h);
    execute(pipeline_source.as_mut(), &mut sink).expect("pipeline execution");

    // 5. Get output and verify it's valid JPEG.
    let output = sink.take_output().expect("encoder output");
    let out_bytes = output.into_vec();
    assert!(out_bytes.len() > 100, "output too small: {} bytes", out_bytes.len());

    let verify_config = JpegDecoderConfig::default();
    let verify_info = verify_config.job().probe(&out_bytes).expect("output should be valid JPEG");
    assert_eq!(verify_info.width, dst_w);
    assert_eq!(verify_info.height, dst_h);

    eprintln!(
        "Bridge codec: {}x{} → {}x{}, output {} KB",
        src_w, src_h, dst_w, dst_h,
        out_bytes.len() / 1024,
    );
}

// ============================================================================
// Test: bridge passthrough (no processing) → encode
// ============================================================================

#[test]
fn bridge_passthrough_encode_jpeg() {
    let w = 128u32;
    let h = 96u32;

    // Only a decode node — pixel pipeline is passthrough.
    let decode_node = zennode::nodes::DECODE_NODE.create_default().unwrap();
    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![decode_node];

    let source = gradient_source(w, h);
    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    assert_eq!(result.source.width(), w);
    assert_eq!(result.source.height(), h);

    let mut pipeline_source = result.source;
    let mut sink = make_encoder_sink(w, h);
    execute(pipeline_source.as_mut(), &mut sink).expect("pipeline execution");

    let output = sink.take_output().expect("encoder output");
    let out_bytes = output.into_vec();
    let verify_info = JpegDecoderConfig::default()
        .job()
        .probe(&out_bytes)
        .expect("valid JPEG");
    assert_eq!(verify_info.width, w);
    assert_eq!(verify_info.height, h);
}

// ============================================================================
// Test: bridge with materialize → verify pixel dimensions
// ============================================================================

#[test]
fn bridge_materialize_after_resize() {
    use zenpipe::Source as _;

    let src_w = 400u32;
    let src_h = 300u32;
    let dst_w = 100u32;
    let dst_h = 75u32;

    let decode_node = zennode::nodes::DECODE_NODE.create_default().unwrap();
    let mut constrain_params = zennode::ParamMap::new();
    constrain_params.insert("w".into(), zennode::ParamValue::U32(dst_w));
    constrain_params.insert("h".into(), zennode::ParamValue::U32(dst_h));
    constrain_params.insert("mode".into(), zennode::ParamValue::Str("fit".into()));
    constrain_params.insert("filter".into(), zennode::ParamValue::Str("robidoux".into()));
    let constrain_node = MockConstrainNode::boxed(constrain_params);

    let nodes: Vec<Box<dyn zennode::NodeInstance>> = vec![decode_node, constrain_node];
    let source = gradient_source(src_w, src_h);

    let result = bridge::build_pipeline(source, &nodes, &[]).unwrap();
    let mat = result.materialize().unwrap();

    assert_eq!(mat.pixels.width(), dst_w);
    assert_eq!(mat.pixels.height(), dst_h);
    assert!(!mat.pixels.data().is_empty());
    assert_eq!(mat.decode_config.hdr_mode, "sdr_only");
}

// ============================================================================
// Mock constrain node — zenresize doesn't have zennode_defs yet
// ============================================================================

static CONSTRAIN_SCHEMA: zennode::NodeSchema = zennode::NodeSchema {
    id: "zenresize.constrain",
    label: "Constrain",
    description: "Resize within constraints",
    group: zennode::NodeGroup::Geometry,
    role: zennode::NodeRole::Geometry,
    params: &[],
    tags: &[],
    coalesce: Some(zennode::CoalesceInfo {
        group: "layout_plan",
        fusable: true,
        is_target: false,
    }),
    format: zennode::FormatHint {
        preferred: zennode::PixelFormatPreference::Any,
        alpha: zennode::AlphaHandling::Process,
        changes_dimensions: true,
        is_neighborhood: false,
    },
    version: 1,
    compat_version: 1,
};

struct MockConstrainNode {
    params: zennode::ParamMap,
}

impl MockConstrainNode {
    fn boxed(params: zennode::ParamMap) -> Box<dyn zennode::NodeInstance> {
        Box::new(Self { params })
    }
}

impl zennode::NodeInstance for MockConstrainNode {
    fn schema(&self) -> &'static zennode::NodeSchema {
        &CONSTRAIN_SCHEMA
    }

    fn to_params(&self) -> zennode::ParamMap {
        self.params.clone()
    }

    fn get_param(&self, name: &str) -> Option<zennode::ParamValue> {
        self.params.get(name).cloned()
    }

    fn set_param(&mut self, name: &str, value: zennode::ParamValue) -> bool {
        self.params.insert(name.into(), value);
        true
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_boxed(&self) -> Box<dyn zennode::NodeInstance> {
        Box::new(Self {
            params: self.params.clone(),
        })
    }
}

// ============================================================================
// Send shim for encoder
// ============================================================================

struct SendEncoderShim<E>(E);

impl<E: zencodec::encode::Encoder + Send> zencodec::encode::DynEncoder for SendEncoderShim<E> {
    fn preferred_strip_height(&self) -> u32 {
        self.0.preferred_strip_height()
    }

    fn encode(
        self: Box<Self>,
        pixels: zenpixels::PixelSlice<'_>,
    ) -> Result<zencodec::encode::EncodeOutput, zencodec::encode::BoxedError> {
        self.0
            .encode(pixels)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }

    fn encode_srgba8(
        self: Box<Self>,
        data: &mut [u8],
        make_opaque: bool,
        width: u32,
        height: u32,
        stride_pixels: u32,
    ) -> Result<zencodec::encode::EncodeOutput, zencodec::encode::BoxedError> {
        self.0
            .encode_srgba8(data, make_opaque, width, height, stride_pixels)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }

    fn push_rows(
        &mut self,
        rows: zenpixels::PixelSlice<'_>,
    ) -> Result<(), zencodec::encode::BoxedError> {
        self.0
            .push_rows(rows)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }

    fn finish(
        self: Box<Self>,
    ) -> Result<zencodec::encode::EncodeOutput, zencodec::encode::BoxedError> {
        self.0
            .finish()
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }

    fn encode_from(
        self: Box<Self>,
        source: &mut dyn FnMut(u32, zenpixels::PixelSliceMut<'_>) -> usize,
    ) -> Result<zencodec::encode::EncodeOutput, zencodec::encode::BoxedError> {
        self.0
            .encode_from(source)
            .map_err(|e| Box::new(e) as zencodec::encode::BoxedError)
    }
}
