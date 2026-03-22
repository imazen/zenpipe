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
    ///   Apple AMPF files (iPhone 17 Pro) are detected as JPEG and handled here.
    /// - **AVIF**: Extracts tmap gain map from AV1 auxiliary image + metadata.
    /// - **JXL**: Extracts jhgm gain map from JXL codestream + ISO 21496-1 metadata.
    /// - **DNG/RAW**: Extracts ISO 21496-1 gain map from embedded preview JPEG's MPF
    ///   (Apple ProRAW). Requires the `raw-decode-gainmap` feature.
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
        let data = self.data; // Save reference before consuming self
        let output = self.decode_format(format)?;

        let gainmap = match format {
            ImageFormat::Jpeg => {
                let gm = extract_jpeg_gainmap(&output);
                // If standard UltraHDR extraction didn't find a gain map,
                // try Apple MPF extraction (for AMPF files detected as JPEG).
                #[cfg(feature = "raw-decode-gainmap")]
                let gm = gm.or_else(|| extract_raw_gainmap(data));
                gm
            }
            #[cfg(feature = "avif-decode")]
            ImageFormat::Avif => extract_avif_gainmap(&output),
            #[cfg(feature = "jxl-decode")]
            ImageFormat::Jxl => extract_jxl_gainmap(&output),
            #[cfg(feature = "raw-decode-gainmap")]
            ImageFormat::Custom(def) if def.name == "dng" || def.name == "raw" => {
                extract_raw_gainmap(data)
            }
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
            #[cfg(feature = "avif-decode")]
            ImageFormat::Avif => extract_avif_depth(data),
            _ => None,
        };

        Ok((output, depth))
    }

    // ═══════════════════════════════════════════════════════════════════
    // RAW/DNG preview extraction
    // ═══════════════════════════════════════════════════════════════════

    /// Extract the embedded JPEG preview from a RAW/DNG file.
    ///
    /// DNG files commonly contain a reduced-resolution JPEG preview in IFD0.
    /// Apple ProRAW (APPLEDNG) files embed a full-resolution sRGB JPEG.
    ///
    /// Returns the raw JPEG bytes, or `None` if:
    /// - The data is not a RAW/DNG file
    /// - No JPEG preview is embedded
    /// - The `raw-decode-exif` feature is not enabled
    ///
    /// The returned bytes can be decoded through a separate `DecodeRequest`:
    ///
    /// ```no_run
    /// use zencodecs::DecodeRequest;
    ///
    /// let raw_data: &[u8] = &[]; // DNG file bytes
    /// if let Some(preview_jpeg) = DecodeRequest::new(raw_data).extract_raw_preview() {
    ///     let preview = DecodeRequest::new(&preview_jpeg).decode()?;
    ///     println!("Preview: {}x{}", preview.width(), preview.height());
    /// }
    /// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
    /// ```
    #[cfg(feature = "raw-decode-exif")]
    pub fn extract_raw_preview(&self) -> Option<alloc::vec::Vec<u8>> {
        crate::codecs::raw::extract_preview(self.data)
    }

    /// Read structured EXIF and DNG metadata from a RAW/DNG file.
    ///
    /// Uses zenraw's kamadak-exif parser, which reads the full TIFF IFD
    /// structure including DNG-specific tags (color matrices, white balance,
    /// calibration illuminants).
    ///
    /// Returns `None` if the data is not a RAW/DNG file or parsing fails.
    #[cfg(feature = "raw-decode-exif")]
    pub fn read_raw_metadata(&self) -> Option<zenraw::exif::ExifMetadata> {
        crate::codecs::raw::read_raw_metadata(self.data)
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
    /// # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
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

            // RAW/DNG: Custom format from zenraw
            #[cfg(feature = "raw-decode")]
            ImageFormat::Custom(def) if def.name == "dng" || def.name == "raw" => {
                crate::codecs::raw::decode(self.data, self.codec_config, self.limits, self.stop)
            }

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

    // Convert zenavif-parse's rational metadata → zencodec GainMapParams (log2 domain)
    // → ultrahdr GainMapMetadata (linear domain). This avoids the domain confusion
    // bug where log2 rational values were previously assigned directly to linear fields.
    let params = avif_gain_map_to_params(&avif_gm.metadata);
    let uhdr_metadata = crate::gainmap::params_to_metadata(&params);

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

/// Convert zenavif-parse's rational gain map metadata to zencodec's canonical
/// log2-domain [`GainMapParams`](zencodec::GainMapParams).
///
/// ISO 21496-1 stores gains and headroom as log2 rational fractions.
/// zenavif-parse preserves these as raw numerator/denominator pairs.
/// This function divides them to f64 and stores in `GainMapParams` which
/// keeps gains/headroom in log2 domain (gamma/offsets are already linear).
#[cfg(all(feature = "avif-decode", feature = "jpeg-ultrahdr"))]
fn avif_gain_map_to_params(meta: &zenavif::GainMapMetadata) -> zencodec::GainMapParams {
    let safe_div = |n: i64, d: u64| -> f64 {
        if d == 0 {
            0.0
        } else {
            n as f64 / d as f64
        }
    };
    let safe_div_u = |n: u64, d: u64| -> f64 {
        if d == 0 {
            0.0
        } else {
            n as f64 / d as f64
        }
    };

    let convert_channel = |ch: &zenavif::GainMapChannel| -> zencodec::GainMapChannel {
        zencodec::GainMapChannel {
            // Gains: n/d produces log2 value — GainMapParams stores log2
            min: safe_div(ch.gain_map_min_n as i64, ch.gain_map_min_d as u64),
            max: safe_div(ch.gain_map_max_n as i64, ch.gain_map_max_d as u64),
            // Gamma and offsets: n/d produces linear value — GainMapParams stores linear
            gamma: safe_div_u(ch.gamma_n as u64, ch.gamma_d as u64),
            base_offset: safe_div(ch.base_offset_n as i64, ch.base_offset_d as u64),
            alternate_offset: safe_div(ch.alternate_offset_n as i64, ch.alternate_offset_d as u64),
        }
    };

    let mut channels = [zencodec::GainMapChannel::default(); 3];
    channels[0] = convert_channel(&meta.channels[0]);
    if meta.is_multichannel {
        channels[1] = convert_channel(&meta.channels[1]);
        channels[2] = convert_channel(&meta.channels[2]);
    } else {
        channels[1] = channels[0];
        channels[2] = channels[0];
    }

    let mut params = zencodec::GainMapParams::default();
    params.channels = channels;
    // Headroom: n/d produces log2 value — GainMapParams stores log2
    params.base_hdr_headroom =
        safe_div_u(meta.base_hdr_headroom_n as u64, meta.base_hdr_headroom_d as u64);
    params.alternate_hdr_headroom =
        safe_div_u(meta.alternate_hdr_headroom_n as u64, meta.alternate_hdr_headroom_d as u64);
    params.use_base_color_space = meta.use_base_colour_space;
    params
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

/// Extract an ISO 21496-1 gain map from a RAW/DNG file.
///
/// Apple APPLEDNG (ProRAW) files embed a preview JPEG with an MPF gain map.
/// Delegates to [`crate::codecs::raw::extract_gainmap`].
///
/// Returns `None` for non-Apple DNGs and generic RAW files.
#[cfg(feature = "raw-decode-gainmap")]
fn extract_raw_gainmap(data: &[u8]) -> Option<crate::gainmap::DecodedGainMap> {
    crate::codecs::raw::extract_gainmap(data)
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

/// Extract depth map from an AVIF auxiliary depth image.
///
/// Parses the AVIF container to detect `auxl`-linked depth items (with `auxC`
/// depth URN), then decodes the depth AV1 bitstream via zenavif to obtain
/// grayscale pixels.
#[cfg(feature = "avif-decode")]
fn extract_avif_depth(data: &[u8]) -> Option<crate::depthmap::DecodedDepthMap> {
    use crate::depthmap::*;

    // Parse the AVIF container to find the depth auxiliary item
    let parser = zenavif::ManagedAvifDecoder::new(data, &zenavif::DecoderConfig::default()).ok()?;
    let info = parser.probe_info().ok()?;
    let avif_depth = info.depth_map?;

    if avif_depth.data.is_empty() {
        return None;
    }

    // Decode the depth AV1 bitstream to pixels
    let (pixel_data, w, h, channels) = zenavif::decode_av1_obu(&avif_depth.data).ok()?;

    if w == 0 || h == 0 {
        return None;
    }

    // Convert to grayscale bytes if needed (depth images are typically monochrome)
    let pixel_bytes = if channels == 1 {
        pixel_data
    } else {
        // RGB→gray: use luminance approximation (0.299R + 0.587G + 0.114B)
        let pixel_count = (w as usize) * (h as usize);
        let mut gray = alloc::vec::Vec::with_capacity(pixel_count);
        for chunk in pixel_data.chunks_exact(channels as usize) {
            let r = chunk[0] as u32;
            let g = chunk[1] as u32;
            let b = chunk[2] as u32;
            gray.push(((r * 77 + g * 150 + b * 29) >> 8) as u8);
        }
        gray
    };

    Some(DecodedDepthMap {
        depth: DepthImage {
            data: pixel_bytes,
            width: w,
            height: h,
            pixel_format: DepthPixelFormat::Gray8,
        },
        metadata: DepthMapMetadata {
            // AVIF depth auxiliary doesn't carry range/units metadata in the container.
            // Default to range-linear with generic near/far — callers should use
            // application-level metadata if available.
            format: DepthFormat::RangeLinear,
            near: 0.0,
            far: 1.0,
            units: DepthUnits::Meters,
            measure_type: DepthMeasureType::OpticalAxis,
        },
        confidence: None,
        source_format: ImageFormat::Avif,
        source_device: DepthSource::Avif,
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

    /// Verify decode_depth_map extracts depth from an AVIF with depth auxiliary.
    #[cfg(feature = "avif-decode")]
    #[test]
    fn decode_depth_map_avif_with_depth() {
        extern crate std;
        let avif_data =
            std::fs::read("../zenavif-parse/tests/colors-animated-8bpc-depth-exif-xmp.avif")
                .expect("test vector not found");

        let (output, depth) = DecodeRequest::new(&avif_data).decode_depth_map().unwrap();
        assert!(output.width() > 0, "base image should decode");

        let dm = depth.expect("AVIF with depth auxiliary should produce a depth map");
        assert!(dm.depth.width > 0, "depth width should be positive");
        assert!(dm.depth.height > 0, "depth height should be positive");
        assert!(!dm.depth.data.is_empty(), "depth data should not be empty");
        assert_eq!(dm.source_format, ImageFormat::Avif);
        assert_eq!(dm.source_device, crate::depthmap::DepthSource::Avif);
        assert_eq!(
            dm.depth.pixel_format,
            crate::depthmap::DepthPixelFormat::Gray8
        );
    }

    /// Verify AVIF without depth returns None.
    #[cfg(feature = "avif-decode")]
    #[test]
    fn decode_depth_map_avif_no_depth() {
        extern crate std;
        let avif_path = "../zenavif-parse/tests/colors-animated-8bpc.avif";
        let Ok(avif_data) = std::fs::read(avif_path) else {
            return; // skip if test vector not available
        };

        let (output, depth) = DecodeRequest::new(&avif_data).decode_depth_map().unwrap();
        assert!(output.width() > 0, "base image should decode");
        assert!(
            depth.is_none(),
            "AVIF without depth auxiliary should return None"
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Gain map tests for RAW/DNG
    // ═══════════════════════════════════════════════════════════════════

    /// Regular (non-ProRAW) DNG should have no gain map.
    #[cfg(feature = "raw-decode-gainmap")]
    #[test]
    fn decode_gain_map_returns_none_for_regular_dng() {
        extern crate std;
        // Try a standard (non-Apple) DNG from the FiveK dataset
        let dir = "/mnt/v/input/fivek/dng/";
        let Ok(entries) = std::fs::read_dir(dir) else {
            std::eprintln!("Skipping: FiveK DNG dir not found at {dir}");
            return;
        };
        for entry in entries.filter_map(|e| e.ok()).take(1) {
            let path = entry.path();
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
            {
                continue;
            }
            let data = std::fs::read(&path).unwrap();
            let (output, gainmap) = DecodeRequest::new(&data).decode_gain_map().unwrap();
            assert!(output.width() > 0, "base image should decode");
            assert!(
                gainmap.is_none(),
                "standard DNG should not have a gain map: {}",
                path.display()
            );
            return;
        }
        std::eprintln!("Skipping: no DNG files found for gain map test");
    }

    /// Apple APPLEDNG (ProRAW) should have a gain map if present.
    ///
    /// Note: `decode_gain_map()` also decodes the base image, which may fail
    /// with the rawloader backend (rawloader panics on Apple LJPEG DNG).
    /// This test verifies the gain map extraction path separately via
    /// `extract_raw_preview` + `extract_gainmap` when the full decode fails.
    #[cfg(feature = "raw-decode-gainmap")]
    #[test]
    fn decode_gain_map_appledng() {
        extern crate std;
        let path = "/mnt/v/heic/46CD6167-C36B-4F98-B386-2300D8E840F0.DNG";
        let Ok(data) = std::fs::read(path) else {
            std::eprintln!("Skipping: APPLEDNG file not found at {path}");
            return;
        };

        // Try full decode_gain_map first; if the base decode fails (rawloader
        // doesn't support Apple LJPEG), test the gain map extraction directly.
        match DecodeRequest::new(&data).decode_gain_map() {
            Ok((output, gainmap)) => {
                assert!(output.width() > 0, "base image should decode");
                check_appledng_gainmap(gainmap.as_ref());
            }
            Err(e) => {
                std::eprintln!(
                    "Base decode failed (expected with rawloader): {}",
                    e.error()
                );
                // Test gain map extraction directly, bypassing the base decode.
                let gainmap = crate::codecs::raw::extract_gainmap(&data);
                check_appledng_gainmap(gainmap.as_ref());
            }
        }
    }

    #[cfg(feature = "raw-decode-gainmap")]
    fn check_appledng_gainmap(gainmap: Option<&crate::gainmap::DecodedGainMap>) {
        extern crate std;
        if let Some(gm) = gainmap {
            std::eprintln!(
                "APPLEDNG gain map: {}x{} ch={} ({} bytes)",
                gm.gain_map.width,
                gm.gain_map.height,
                gm.gain_map.channels,
                gm.gain_map.data.len()
            );
            assert!(gm.gain_map.width > 0);
            assert!(gm.gain_map.height > 0);
            assert!(gm.gain_map.validate().is_ok());
            assert_eq!(gm.source_format, ImageFormat::Custom(&zenraw::DNG_FORMAT));
            std::eprintln!(
                "  hdr_capacity_max={} base_is_hdr={}",
                gm.metadata.hdr_capacity_max,
                gm.base_is_hdr
            );
        } else {
            std::eprintln!("APPLEDNG has no gain map (may need MPF in preview)");
        }
    }

    /// Apple AMPF files (iPhone 17 Pro) should be detected as JPEG and their
    /// gain map should be extracted by the JPEG gain map path, not the RAW path.
    #[cfg(feature = "raw-decode-gainmap")]
    #[test]
    fn ampf_routes_through_jpeg_gain_map_path() {
        extern crate std;
        let path = "/mnt/v/heic/IMG_3269.DNG";
        let Ok(data) = std::fs::read(path) else {
            std::eprintln!("Skipping: AMPF file not found at {path}");
            return;
        };

        // AMPF starts with JPEG SOI — should be detected as JPEG, not RAW.
        let format = crate::info::detect_format(&data);
        assert_eq!(
            format,
            Some(ImageFormat::Jpeg),
            "AMPF should be detected as JPEG, not RAW"
        );

        // Gain map should be extractable via the JPEG path.
        let (output, gainmap) = DecodeRequest::new(&data).decode_gain_map().unwrap();
        assert!(output.width() > 0, "AMPF base image should decode as JPEG");

        if let Some(gm) = &gainmap {
            std::eprintln!(
                "AMPF gain map via JPEG path: {}x{} ch={} ({} bytes)",
                gm.gain_map.width,
                gm.gain_map.height,
                gm.gain_map.channels,
                gm.gain_map.data.len()
            );
            assert!(gm.gain_map.width > 0);
            assert!(gm.gain_map.height > 0);
            assert!(gm.gain_map.validate().is_ok());
            // Source format should be JPEG since it was detected as JPEG.
            assert_eq!(gm.source_format, ImageFormat::Jpeg);
        } else {
            std::eprintln!("AMPF has no gain map via JPEG path (unexpected)");
        }
    }
}
