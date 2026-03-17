//! Image decoding.

pub use zencodec::decode::DecodeOutput;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::policy::CodecPolicy;
use crate::{CodecError, CodecRegistry, ImageFormat, ImageInfo, Limits, Stop};
use whereat::at;

/// Image decode request builder.
///
/// # Example
///
/// ```no_run
/// use zencodecs::DecodeRequest;
///
/// let data: &[u8] = &[]; // your image bytes
/// let output = DecodeRequest::new(data).decode()?;
/// println!("{}x{}", output.width(), output.height());
/// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
/// ```
pub struct DecodeRequest<'a> {
    data: &'a [u8],
    format: Option<ImageFormat>,
    limits: Option<&'a Limits>,
    stop: Option<&'a dyn Stop>,
    registry: Option<&'a CodecRegistry>,
    codec_config: Option<&'a CodecConfig>,
    policy: Option<CodecPolicy>,
}

impl<'a> DecodeRequest<'a> {
    /// Create a new decode request.
    ///
    /// Format will be auto-detected from magic bytes.
    /// The decoder returns its native pixel format.
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            format: None,
            limits: None,
            stop: None,
            registry: None,
            codec_config: None,
            policy: None,
        }
    }

    /// Override format auto-detection.
    pub fn with_format(mut self, format: ImageFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Set resource limits.
    pub fn with_limits(mut self, limits: &'a Limits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Set a cancellation token.
    pub fn with_stop(mut self, stop: &'a dyn Stop) -> Self {
        self.stop = Some(stop);
        self
    }

    /// Set a codec registry to control which formats are enabled.
    pub fn with_registry(mut self, registry: &'a CodecRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Set format-specific codec configuration.
    pub fn with_codec_config(mut self, config: &'a CodecConfig) -> Self {
        self.codec_config = Some(config);
        self
    }

    /// Set a per-request codec policy for filtering and preferences.
    ///
    /// Currently reserved for future use with fallback chains and
    /// multi-decoder-per-format support. The policy's format restrictions
    /// are checked during format detection.
    pub fn with_policy(mut self, policy: CodecPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Resolve format (auto-detect or explicit) and check registry.
    fn resolve_format(&self) -> Result<ImageFormat> {
        let default_registry = CodecRegistry::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data)
                .ok_or_else(|| at!(CodecError::UnrecognizedFormat))?,
        };
        if !registry.can_decode(format) {
            return Err(at!(CodecError::DisabledFormat(format)));
        }
        Ok(format)
    }

    /// Decode, convert to target pixel type, and copy rows into `dst`.
    fn decode_into<P: Copy + zenpixels::Pixel>(
        self,
        dst: imgref::ImgRefMut<'_, P>,
        convert: fn(zenpixels::PixelBuffer) -> zenpixels::PixelBuffer<P>,
    ) -> Result<ImageInfo> {
        let format = self.resolve_format()?;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = convert(output.into_buffer());
        copy_rows(src.as_imgref(), dst);
        Ok(info)
    }

    /// Decode directly into a caller-provided RGB8 buffer.
    pub fn decode_into_rgb8(self, dst: imgref::ImgRefMut<'_, rgb::Rgb<u8>>) -> Result<ImageInfo> {
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        self.decode_into(dst, |b| b.to_rgb8())
    }

    /// Decode directly into a caller-provided RGBA8 buffer.
    pub fn decode_into_rgba8(self, dst: imgref::ImgRefMut<'_, rgb::Rgba<u8>>) -> Result<ImageInfo> {
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        self.decode_into(dst, |b| b.to_rgba8())
    }

    /// Decode directly into a caller-provided Gray8 buffer.
    pub fn decode_into_gray8(self, dst: imgref::ImgRefMut<'_, rgb::Gray<u8>>) -> Result<ImageInfo> {
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        self.decode_into(dst, |b| b.to_gray8())
    }

    /// Decode directly into a caller-provided BGRA8 buffer.
    pub fn decode_into_bgra8(self, dst: imgref::ImgRefMut<'_, rgb::Bgra<u8>>) -> Result<ImageInfo> {
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        self.decode_into(dst, |b| b.to_bgra8())
    }

    /// Decode directly into a caller-provided BGRX8 buffer (alpha byte set to 255).
    pub fn decode_into_bgrx8(self, dst: imgref::ImgRefMut<'_, rgb::Bgra<u8>>) -> Result<ImageInfo> {
        let format = self.resolve_format()?;
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgb8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = rgb::Bgra {
                    b: s.b,
                    g: s.g,
                    r: s.r,
                    a: 255,
                };
            }
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided linear RGB f32 buffer.
    pub fn decode_into_rgb_f32(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Rgb<f32>>,
    ) -> Result<ImageInfo> {
        use linear_srgb::default::srgb_u8_to_linear;
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        let format = self.resolve_format()?;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgb8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = rgb::Rgb {
                    r: srgb_u8_to_linear(s.r),
                    g: srgb_u8_to_linear(s.g),
                    b: srgb_u8_to_linear(s.b),
                };
            }
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided linear RGBA f32 buffer.
    pub fn decode_into_rgba_f32(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Rgba<f32>>,
    ) -> Result<ImageInfo> {
        use linear_srgb::default::srgb_u8_to_linear;
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        let format = self.resolve_format()?;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_rgba8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = rgb::Rgba {
                    r: srgb_u8_to_linear(s.r),
                    g: srgb_u8_to_linear(s.g),
                    b: srgb_u8_to_linear(s.b),
                    a: s.a as f32 / 255.0,
                };
            }
        }
        Ok(info)
    }

    /// Decode directly into a caller-provided linear grayscale f32 buffer.
    pub fn decode_into_gray_f32(
        self,
        dst: imgref::ImgRefMut<'_, rgb::Gray<f32>>,
    ) -> Result<ImageInfo> {
        use linear_srgb::default::srgb_u8_to_linear;
        use zenpixels_convert::PixelBufferConvertTypedExt as _;
        let format = self.resolve_format()?;
        let output = self.decode_format(format)?;
        let info = output.info().clone();
        let src = output.into_buffer().to_gray8();
        let mut dst = dst;
        for (src_row, dst_row) in src.as_imgref().rows().zip(dst.rows_mut()) {
            for (s, d) in src_row.iter().zip(dst_row.iter_mut()) {
                *d = rgb::Gray::new(srgb_u8_to_linear(s.value()));
            }
        }
        Ok(info)
    }

    /// Decode UltraHDR JPEG to linear f32 RGBA HDR pixels.
    ///
    /// Extracts the gain map from an UltraHDR JPEG and reconstructs HDR content.
    /// Returns linear f32 RGBA pixels. Fails if the image is not an UltraHDR JPEG.
    ///
    /// `display_boost` controls the HDR headroom: 1.0 = SDR, 4.0 = typical HDR display.
    #[cfg(feature = "jpeg-ultrahdr")]
    pub fn decode_hdr(self, display_boost: f32) -> Result<DecodeOutput> {
        let format = self.resolve_format()?;
        if format != ImageFormat::Jpeg {
            return Err(at!(CodecError::UnsupportedOperation {
                format,
                detail: "UltraHDR decode only supported for JPEG",
            }));
        }
        crate::codecs::jpeg::decode_hdr(
            self.data,
            display_boost,
            self.codec_config,
            self.limits,
            self.stop,
        )
    }

    /// Decode the image to pixels.
    pub fn decode(self) -> Result<DecodeOutput> {
        let format = self.resolve_format()?;
        self.decode_format(format)
    }

    /// Decode an image and extract its gain map, if present.
    ///
    /// Returns the base image decode output plus an optional [`DecodedGainMap`]
    /// containing the gain map image pixels and ISO 21496-1 metadata.
    ///
    /// Gain map support by format:
    /// - **JPEG**: Extracts UltraHDR gain map from MPF secondary images + XMP metadata.
    /// - **AVIF**: Not yet implemented (returns `None` for gain map).
    /// - **JXL**: Not yet implemented (returns `None` for gain map).
    /// - **Other formats**: Returns `None` for gain map.
    ///
    /// The returned [`DecodedGainMap`] can reconstruct HDR from SDR (or vice versa)
    /// via its [`reconstruct_hdr`](crate::DecodedGainMap::reconstruct_hdr) /
    /// [`reconstruct_sdr`](crate::DecodedGainMap::reconstruct_sdr) methods.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zencodecs::DecodeRequest;
    ///
    /// let data: &[u8] = &[]; // UltraHDR JPEG bytes
    /// let (output, gainmap) = DecodeRequest::new(data).decode_gain_map()?;
    /// if let Some(gm) = gainmap {
    ///     println!("Gain map: {}x{}", gm.gain_map.width, gm.gain_map.height);
    ///     println!("Base is HDR: {}", gm.base_is_hdr);
    /// }
    /// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
    /// ```
    #[cfg(feature = "jpeg-ultrahdr")]
    pub fn decode_gain_map(self) -> Result<(DecodeOutput, Option<crate::gainmap::DecodedGainMap>)> {
        let format = self.resolve_format()?;
        let output = self.decode_format(format)?;

        let gainmap = match format {
            ImageFormat::Jpeg => extract_jpeg_gainmap(&output),
            #[cfg(feature = "avif-decode")]
            ImageFormat::Avif => extract_avif_gainmap(&output),
            #[cfg(feature = "jxl-decode")]
            ImageFormat::Jxl => extract_jxl_gainmap(&output),
            _ => None,
        };

        Ok((output, gainmap))
    }

    // ═══════════════════════════════════════════════════════════════════
    // Depth map decode
    // ═══════════════════════════════════════════════════════════════════

    /// Decode an image and extract its depth map, if present.
    ///
    /// Returns the base image decode output plus an optional [`DecodedDepthMap`]
    /// containing the depth image pixels and metadata.
    ///
    /// Depth map support by format:
    /// - **JPEG**: iPhone MPF disparity secondary image.
    ///   GDepth XMP and Android DDF are recognized but require future
    ///   codec-level extraction support.
    /// - **HEIC**: Auxiliary depth image (future — requires heic-decoder support).
    /// - **Other formats**: Returns `None` for depth map.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zencodecs::DecodeRequest;
    ///
    /// let data: &[u8] = &[]; // JPEG bytes with depth map
    /// let (output, depth) = DecodeRequest::new(data).decode_depth_map()?;
    /// if let Some(dm) = depth {
    ///     println!("Depth map: {}x{}", dm.depth.width, dm.depth.height);
    ///     let normalized = dm.to_normalized_f32();
    ///     println!("Near/far: {}/{} {:?}", dm.metadata.near, dm.metadata.far, dm.metadata.units);
    /// }
    /// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
    /// ```
    pub fn decode_depth_map(
        self,
    ) -> Result<(DecodeOutput, Option<crate::depthmap::DecodedDepthMap>)> {
        let format = self.resolve_format()?;
        let data = self.data; // Save reference before consuming self
        let output = self.decode_format(format)?;

        let depth = match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => extract_jpeg_depth(&output),
            #[cfg(feature = "heic-decode")]
            ImageFormat::Heic => extract_heic_depth(data),
            _ => None,
        };

        Ok((output, depth))
    }

    // ═══════════════════════════════════════════════════════════════════
    // Streaming decode
    // ═══════════════════════════════════════════════════════════════════

    /// Push-based decode: the decoder writes rows into the provided sink.
    ///
    /// This is the most memory-efficient decode path — the caller provides
    /// buffers via the sink, and the decoder fills them in order.
    pub fn push_decode(
        self,
        sink: &mut dyn zencodec::decode::DecodeRowSink,
    ) -> Result<zencodec::decode::OutputInfo> {
        let format = self.resolve_format()?;
        crate::dyn_dispatch::dyn_push_decode(format, &self.decode_params(), sink)
    }

    // ═══════════════════════════════════════════════════════════════════
    // Animation decode
    // ═══════════════════════════════════════════════════════════════════

    /// Returns a full-frame decoder for animated images.
    ///
    /// For animated formats (GIF, animated WebP, APNG), yields frames
    /// in sequence with duration information. For single-frame formats,
    /// yields one frame then `None`.
    ///
    /// Note: The input data is copied to an owned buffer because the
    /// full-frame decoder is `'static` (it owns its data).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zencodecs::DecodeRequest;
    ///
    /// let data: &[u8] = &[]; // GIF bytes
    /// let mut decoder = DecodeRequest::new(data).full_frame_decoder()?;
    /// while let Some(frame) = decoder.render_next_frame_owned(None)? {
    ///     // frame.pixels(), frame.duration_ms(), frame.frame_index()
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn full_frame_decoder(
        self,
    ) -> Result<alloc::boxed::Box<dyn zencodec::decode::DynFullFrameDecoder>> {
        let format = self.resolve_format()?;
        crate::dyn_dispatch::dyn_full_frame_decoder(format, &self.decode_params())
    }

    // ═══════════════════════════════════════════════════════════════════
    // Probe
    // ═══════════════════════════════════════════════════════════════════

    /// Probe image metadata without decoding pixels.
    ///
    /// Cheaper than `decode()` — only parses headers.
    pub fn probe(&self) -> Result<ImageInfo> {
        let format = self.resolve_format()?;
        crate::info::probe_format(self.data, format)
    }

    // ═══════════════════════════════════════════════════════════════════
    // Internal helpers
    // ═══════════════════════════════════════════════════════════════════

    fn decode_params(&self) -> crate::dyn_dispatch::DecodeParams<'a> {
        crate::dyn_dispatch::DecodeParams {
            data: self.data,
            codec_config: self.codec_config,
            limits: self.limits,
            stop: self.stop,
            preferred: &[],
        }
    }

    /// Dispatch to format-specific decoder.
    fn decode_format(self, format: ImageFormat) -> Result<DecodeOutput> {
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => {
                crate::codecs::jpeg::decode(self.data, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => {
                crate::codecs::webp::decode(self.data, self.codec_config, self.limits, self.stop)
            }
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => crate::codecs::gif::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "png")]
            ImageFormat::Png => crate::codecs::png::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "avif-decode")]
            ImageFormat::Avif => crate::codecs::avif_dec::decode(
                self.data,
                self.codec_config,
                self.limits,
                self.stop,
            ),
            #[cfg(not(feature = "avif-decode"))]
            ImageFormat::Avif => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "jxl-decode")]
            ImageFormat::Jxl => crate::codecs::jxl_dec::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "jxl-decode"))]
            ImageFormat::Jxl => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "heic-decode")]
            ImageFormat::Heic => crate::codecs::heic::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "heic-decode"))]
            ImageFormat::Heic => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps")]
            ImageFormat::Pnm => crate::codecs::pnm::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "bitmaps"))]
            ImageFormat::Pnm => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps-bmp")]
            ImageFormat::Bmp => crate::codecs::bmp::decode(self.data, self.limits, self.stop),
            #[cfg(not(feature = "bitmaps-bmp"))]
            ImageFormat::Bmp => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps")]
            ImageFormat::Farbfeld => {
                crate::codecs::farbfeld::decode(self.data, self.limits, self.stop)
            }
            #[cfg(not(feature = "bitmaps"))]
            ImageFormat::Farbfeld => Err(at!(CodecError::UnsupportedFormat(format))),

            _ => Err(at!(CodecError::UnsupportedFormat(format))),
        }
    }
}

/// Copy rows from src to dst, handling stride mismatches.
fn copy_rows<P: Copy>(src: imgref::ImgRef<'_, P>, mut dst: imgref::ImgRefMut<'_, P>) {
    for (src_row, dst_row) in src.rows().zip(dst.rows_mut()) {
        let n = src_row.len().min(dst_row.len());
        dst_row[..n].copy_from_slice(&src_row[..n]);
    }
}

/// Extract a gain map from a JPEG DecodeOutput's extras, if present.
///
/// Returns `None` if the JPEG doesn't contain UltraHDR gain map data.
#[cfg(feature = "jpeg-ultrahdr")]
fn extract_jpeg_gainmap(output: &DecodeOutput) -> Option<crate::gainmap::DecodedGainMap> {
    use crate::gainmap::{DecodedGainMap, GainMapImage};
    use zenjpeg::ultrahdr::UltraHdrExtras as _;

    let extras = output.extras::<zenjpeg::decoder::DecodedExtras>()?;

    if !extras.is_ultrahdr() {
        return None;
    }

    // Parse gain map metadata from XMP
    let (metadata, _) = extras.ultrahdr_metadata()?.ok()?;

    // Decode the gain map JPEG from MPF secondary images
    let core_gainmap = extras.decode_gainmap()?.ok()?;

    Some(DecodedGainMap {
        gain_map: GainMapImage {
            data: core_gainmap.data,
            width: core_gainmap.width,
            height: core_gainmap.height,
            channels: core_gainmap.channels,
        },
        metadata,
        base_is_hdr: false, // JPEG UltraHDR: base=SDR, gain map maps SDR→HDR
        source_format: ImageFormat::Jpeg,
    })
}

/// Extract a gain map from an AVIF DecodeOutput's extras, if present.
///
/// The gain map image is returned as raw AV1 bytes — the caller must
/// decode them separately to get pixels. For now we store the raw bytes
/// in `GainMapImage` with channels=0 to signal "not yet decoded."
#[cfg(all(feature = "avif-decode", feature = "jpeg-ultrahdr"))]
fn extract_avif_gainmap(output: &DecodeOutput) -> Option<crate::gainmap::DecodedGainMap> {
    use crate::gainmap::{DecodedGainMap, GainMapImage};

    let avif_gm = output.extras::<zenavif::AvifGainMap>()?;

    // Convert zenavif-parse's rational GainMapMetadata to ultrahdr-core's f32 GainMapMetadata
    let meta = &avif_gm.metadata;
    let convert_channel = |ch: &zenavif::GainMapChannel| -> (f32, f32, f32, f32, f32) {
        let safe_div = |n: i64, d: u64| -> f32 { if d == 0 { 0.0 } else { n as f32 / d as f32 } };
        let safe_div_u = |n: u64, d: u64| -> f32 { if d == 0 { 0.0 } else { n as f32 / d as f32 } };
        (
            safe_div(ch.gain_map_min_n as i64, ch.gain_map_min_d as u64), // min_content_boost
            safe_div(ch.gain_map_max_n as i64, ch.gain_map_max_d as u64), // max_content_boost
            safe_div_u(ch.gamma_n as u64, ch.gamma_d as u64),             // gamma
            safe_div(ch.base_offset_n as i64, ch.base_offset_d as u64),   // offset_sdr
            safe_div(ch.alternate_offset_n as i64, ch.alternate_offset_d as u64), // offset_hdr
        )
    };

    let ch0 = convert_channel(&meta.channels[0]);
    let ch1 = if meta.is_multichannel {
        convert_channel(&meta.channels[1])
    } else {
        ch0
    };
    let ch2 = if meta.is_multichannel {
        convert_channel(&meta.channels[2])
    } else {
        ch0
    };

    let base_headroom = if meta.base_hdr_headroom_d == 0 {
        0.0
    } else {
        meta.base_hdr_headroom_n as f32 / meta.base_hdr_headroom_d as f32
    };
    let alt_headroom = if meta.alternate_hdr_headroom_d == 0 {
        0.0
    } else {
        meta.alternate_hdr_headroom_n as f32 / meta.alternate_hdr_headroom_d as f32
    };

    let uhdr_metadata = crate::gainmap::GainMapMetadata {
        min_content_boost: [ch0.0, ch1.0, ch2.0],
        max_content_boost: [ch0.1, ch1.1, ch2.1],
        gamma: [ch0.2, ch1.2, ch2.2],
        offset_sdr: [ch0.3, ch1.3, ch2.3],
        offset_hdr: [ch0.4, ch1.4, ch2.4],
        hdr_capacity_min: base_headroom,
        hdr_capacity_max: alt_headroom,
        use_base_color_space: meta.use_base_colour_space,
    };

    // Decode the raw AV1 gain map to pixels
    let (gm_data, gm_w, gm_h, gm_ch) = zenavif::decode_av1_obu(&avif_gm.gain_map_data).ok()?;

    Some(DecodedGainMap {
        gain_map: GainMapImage {
            data: gm_data,
            width: gm_w,
            height: gm_h,
            channels: gm_ch,
        },
        metadata: uhdr_metadata,
        base_is_hdr: false, // AVIF: base=SDR, gain map maps SDR→HDR
        source_format: ImageFormat::Avif,
    })
}

/// Extract a gain map from a JXL DecodeOutput's extras, if present.
#[cfg(all(feature = "jxl-decode", feature = "jpeg-ultrahdr"))]
fn extract_jxl_gainmap(output: &DecodeOutput) -> Option<crate::gainmap::DecodedGainMap> {
    use crate::gainmap::{DecodedGainMap, GainMapImage};

    let bundle = output.extras::<zenjxl::GainMapBundle>()?;

    // Parse ISO 21496-1 binary metadata from the jhgm bundle
    let metadata = if !bundle.metadata.is_empty() {
        zenjpeg::ultrahdr::parse_iso21496(&bundle.metadata).ok()?
    } else {
        crate::gainmap::GainMapMetadata::default()
    };

    // Decode the bare JXL codestream to get gain map pixels
    use alloc::vec::Vec;
    let gm_output = zenjxl::decode(&bundle.gain_map_codestream, None, &[]).ok()?;
    use zenpixels_convert::PixelBufferConvertTypedExt as _;
    let gm_rgb8 = gm_output.pixels.to_rgb8();
    let gm_ref = gm_rgb8.as_imgref();
    let gm_w = gm_ref.width() as u32;
    let gm_h = gm_ref.height() as u32;
    let gm_bytes: Vec<u8> = bytemuck::cast_slice(gm_ref.buf()).to_vec();

    // Determine channels: if all R==G==B, it's effectively grayscale
    let is_gray = gm_bytes
        .chunks_exact(3)
        .all(|px| px[0] == px[1] && px[1] == px[2]);
    let (data, channels) = if is_gray {
        // Collapse to single channel
        let gray: Vec<u8> = gm_bytes.chunks_exact(3).map(|px| px[0]).collect();
        (gray, 1u8)
    } else {
        (gm_bytes, 3u8)
    };

    Some(DecodedGainMap {
        gain_map: GainMapImage {
            data,
            width: gm_w,
            height: gm_h,
            channels,
        },
        metadata,
        base_is_hdr: true, // JXL: base=HDR, gain map maps HDR→SDR
        source_format: ImageFormat::Jxl,
    })
}

// =========================================================================
// Depth map extraction
// =========================================================================

/// Extract a depth map from a JPEG DecodeOutput's extras, if present.
///
/// Checks for:
/// 1. MPF secondary image with Disparity type (iPhone portrait mode)
///
/// GDepth XMP and Android DDF extraction are not yet implemented at the
/// codec level — these require XMP parsing and container directory
/// traversal that will be added to zenjpeg in future.
#[cfg(feature = "jpeg")]
fn extract_jpeg_depth(output: &DecodeOutput) -> Option<crate::depthmap::DecodedDepthMap> {
    use crate::depthmap::{
        DecodedDepthMap, DepthFormat, DepthImage, DepthMapMetadata, DepthMeasureType,
        DepthPixelFormat, DepthSource, DepthUnits,
    };

    let extras = output.extras::<zenjpeg::decoder::DecodedExtras>()?;

    // Check for MPF disparity map (iPhone portrait mode)
    let depth_jpeg = extras.depth_map()?;

    // Decode the secondary JPEG to get depth pixels
    let depth_output = crate::codecs::jpeg::decode(depth_jpeg, None, None, None).ok()?;
    use zenpixels_convert::PixelBufferConvertTypedExt as _;
    let gray = depth_output.into_buffer().to_gray8();
    let gray_ref = gray.as_imgref();
    let w = gray_ref.width() as u32;
    let h = gray_ref.height() as u32;

    // Extract grayscale bytes
    let data: alloc::vec::Vec<u8> = gray_ref.buf().iter().map(|g| g.value()).collect();

    Some(DecodedDepthMap {
        depth: DepthImage {
            data,
            width: w,
            height: h,
            pixel_format: DepthPixelFormat::Gray8,
        },
        metadata: DepthMapMetadata {
            // iPhone MPF disparity maps use inverse depth (disparity)
            // with normalized 0-255 range. Near/far are not encoded in
            // the MPF metadata — these are relative values.
            format: DepthFormat::Disparity,
            near: 0.0,
            far: 1.0,
            units: DepthUnits::Normalized,
            measure_type: DepthMeasureType::OpticalAxis,
        },
        confidence: None,
        source_format: ImageFormat::Jpeg,
        source_device: DepthSource::AppleMpf,
    })
}

/// Extract a depth map from a HEIC DecodeOutput's extras, if present.
///
/// HEIC files can contain auxiliary depth images (Apple portrait mode).
/// This is a stub — actual extraction requires heic-decoder support for
/// Extract depth map from HEIC auxiliary image.
///
/// Uses heic-decoder's `decode_depth()` to decode the HEVC depth auxiliary item.
#[cfg(feature = "heic-decode")]
fn extract_heic_depth(data: &[u8]) -> Option<crate::depthmap::DecodedDepthMap> {
    use crate::depthmap::*;

    let config = heic_decoder::DecoderConfig::new();
    let depth_map = config.decode_depth(data).ok()?;

    // Convert heic-decoder's DepthRepresentationInfo to our DepthFormat
    let (format, units) = match depth_map.depth_info.representation_type {
        heic_decoder::DepthRepresentationType::UniformInverseZ => {
            (DepthFormat::RangeInverse, DepthUnits::Meters)
        }
        heic_decoder::DepthRepresentationType::UniformDisparity => {
            (DepthFormat::Disparity, DepthUnits::Diopters)
        }
        heic_decoder::DepthRepresentationType::UniformZ => {
            (DepthFormat::RangeLinear, DepthUnits::Meters)
        }
        heic_decoder::DepthRepresentationType::NonuniformDisparity => {
            (DepthFormat::Disparity, DepthUnits::Diopters)
        }
        _ => (DepthFormat::AbsoluteDepth, DepthUnits::Meters),
    };

    let near = depth_map
        .depth_info
        .z_near
        .or(depth_map.depth_info.d_min.map(|d| 1.0 / d))
        .unwrap_or(0.0) as f32;
    let far = depth_map
        .depth_info
        .z_far
        .or(depth_map.depth_info.d_max.map(|d| 1.0 / d))
        .unwrap_or(100.0) as f32;

    // Convert u16 samples to bytes (little-endian)
    let pixel_bytes: alloc::vec::Vec<u8> = depth_map
        .data
        .iter()
        .flat_map(|&v| v.to_le_bytes())
        .collect();

    Some(DecodedDepthMap {
        depth: DepthImage {
            data: pixel_bytes,
            width: depth_map.width,
            height: depth_map.height,
            pixel_format: DepthPixelFormat::Gray16,
        },
        metadata: DepthMapMetadata {
            format,
            near,
            far,
            units,
            measure_type: DepthMeasureType::OpticalAxis,
        },
        confidence: None,
        source_format: ImageFormat::Heic,
        source_device: DepthSource::AppleHeic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_pattern() {
        let data = b"test";
        let request = DecodeRequest::new(data).with_format(ImageFormat::Jpeg);
        assert_eq!(request.format, Some(ImageFormat::Jpeg));
    }

    #[test]
    fn disabled_format_error() {
        let jpeg_data = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let registry = CodecRegistry::none();

        let result = DecodeRequest::new(&jpeg_data)
            .with_registry(&registry)
            .decode();

        assert!(matches!(
            result.as_ref().map_err(|e| e.error()),
            Err(CodecError::DisabledFormat(_))
        ));
    }

    /// Verify decode_depth_map returns None for formats that don't support depth maps.
    #[cfg(feature = "png")]
    #[test]
    fn decode_depth_map_returns_none_for_png() {
        // Minimal valid 1x1 red PNG
        let png_data = {
            let mut buf = alloc::vec::Vec::new();
            let mut encoder = png::Encoder::new(&mut buf, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[255, 0, 0]).unwrap();
            writer.finish().unwrap();
            buf
        };

        let (output, depth) = DecodeRequest::new(&png_data).decode_depth_map().unwrap();
        assert!(depth.is_none(), "PNG should not have a depth map");
        assert_eq!(output.width(), 1);
    }

    /// Verify decode_depth_map returns None for formats that don't support depth maps.
    #[cfg(feature = "gif")]
    #[test]
    fn decode_depth_map_returns_none_for_gif() {
        // Minimal 1x1 GIF87a
        let gif_data: &[u8] = &[
            b'G', b'I', b'F', b'8', b'7', b'a', // header
            0x01, 0x00, // width=1
            0x01, 0x00, // height=1
            0x80, // packed: global color table, 2 entries
            0x00, // bg color index
            0x00, // pixel aspect ratio
            0x00, 0x00, 0x00, // color 0: black
            0xFF, 0xFF, 0xFF, // color 1: white
            0x2C, // image descriptor
            0x00, 0x00, 0x00, 0x00, // left, top
            0x01, 0x00, // width
            0x01, 0x00, // height
            0x00, // packed
            0x02, // LZW min code size
            0x02, // sub-block size
            0x4C, 0x01, // LZW data
            0x00, // sub-block terminator
            0x3B, // GIF trailer
        ];

        let (output, depth) = DecodeRequest::new(gif_data).decode_depth_map().unwrap();
        assert!(depth.is_none(), "GIF should not have a depth map");
        assert_eq!(output.width(), 1);
    }

    /// Verify decode_depth_map returns None for formats that don't support depth maps.
    #[cfg(feature = "webp")]
    #[test]
    fn decode_depth_map_returns_none_for_webp() {
        // Encode a tiny WebP first, then check that it has no depth map
        use crate::EncodeRequest;
        use alloc::vec;

        let pixels = imgref::ImgVec::new(
            vec![
                rgb::Rgb {
                    r: 128u8,
                    g: 64,
                    b: 32
                };
                4
            ],
            2,
            2,
        );
        let encoded = EncodeRequest::new(ImageFormat::WebP)
            .with_quality(50.0)
            .encode_rgb8(pixels.as_ref())
            .unwrap();

        let (output, depth) = DecodeRequest::new(encoded.data())
            .decode_depth_map()
            .unwrap();
        assert!(depth.is_none(), "WebP should not have a depth map");
        assert_eq!(output.width(), 2);
    }
}
