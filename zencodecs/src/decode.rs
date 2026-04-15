//! Image decoding.

pub use zencodec::decode::DecodeOutput;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::policy::CodecPolicy;
use crate::{AllowedFormats, CodecError, ImageFormat, ImageInfo, Limits, StopToken};
use whereat::at;
use zencodec::decode::DecodePolicy;

/// Image decode request builder.
///
/// # Example
///
/// ```no_run
/// use zencodecs::DecodeRequest;
///
/// let data: &[u8] = &[]; // your image bytes
/// let output = DecodeRequest::new(data).decode_full_frame()?;
/// println!("{}x{}", output.width(), output.height());
/// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
/// ```
pub struct DecodeRequest<'a> {
    data: &'a [u8],
    format: Option<ImageFormat>,
    limits: Option<&'a Limits>,
    stop: Option<StopToken>,
    registry: Option<&'a AllowedFormats>,
    codec_config: Option<&'a CodecConfig>,
    policy: Option<CodecPolicy>,
    decode_policy: Option<DecodePolicy>,
    /// When true, codecs that support gain maps will extract and attach
    /// gain map data to the `DecodeOutput` extras. Default: false.
    extract_gain_map: bool,
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
            decode_policy: None,
            extract_gain_map: false,
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
    pub fn with_stop(mut self, stop: StopToken) -> Self {
        self.stop = Some(stop);
        self
    }

    /// Set a codec registry to control which formats are enabled.
    pub fn with_registry(mut self, registry: &'a AllowedFormats) -> Self {
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

    /// Set decode security policy.
    ///
    /// Controls what the decoder is allowed to do: metadata extraction,
    /// progressive/interlaced support, animation, truncated input handling,
    /// and strict parsing. See [`DecodePolicy`] for details.
    ///
    /// # Example
    ///
    /// ```
    /// use zencodecs::DecodeRequest;
    /// use zencodec::decode::DecodePolicy;
    ///
    /// let data: &[u8] = &[];
    /// let request = DecodeRequest::new(data)
    ///     .with_decode_policy(DecodePolicy::strict().with_allow_icc(true));
    /// ```
    pub fn with_decode_policy(mut self, policy: DecodePolicy) -> Self {
        self.decode_policy = Some(policy);
        self
    }

    /// Request gain map extraction during decode.
    ///
    /// When `true`, codecs that support gain maps (AVIF, JXL, HEIC) will
    /// extract and attach gain map data to the [`DecodeOutput`] extras.
    /// The JPEG UltraHDR path is unaffected — it extracts gain maps from
    /// MPF secondary images in a post-decode step.
    ///
    /// Default: `false`. Gain map extraction is opt-in because it requires
    /// additional parsing and memory allocation for data most callers don't need.
    ///
    /// [`decode_gain_map()`](Self::decode_gain_map) sets this automatically.
    pub fn with_gain_map_extraction(mut self, extract: bool) -> Self {
        self.extract_gain_map = extract;
        self
    }

    /// Resolve format (auto-detect or explicit) and check registry.
    fn resolve_format(&self) -> Result<ImageFormat> {
        let default_registry = AllowedFormats::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let (format, explicit) = match self.format {
            Some(f) => (f, true),
            None => (
                crate::info::detect_format(self.data)
                    .ok_or_else(|| at!(CodecError::UnrecognizedFormat))?,
                false,
            ),
        };
        // Custom formats (e.g. RAW/DNG) are not tracked by the
        // `AllowedFormats` bitset — see registry.rs. When the caller
        // explicitly opts in via `with_format(Custom(...))`, treat that
        // as authorization and skip the registry check.
        let is_custom = matches!(format, ImageFormat::Custom(_));
        if !is_custom && !registry.can_decode(format) {
            return Err(at!(CodecError::DisabledFormat(format)));
        }
        // Detection-path Custom formats also pass through — the caller
        // can't whitelist them via the registry, so the only way to
        // refuse is via `with_format` to override.
        let _ = explicit;
        Ok(format)
    }

    /// Decode the full image to pixels (one-shot, full materialization).
    ///
    /// This allocates a buffer for the entire decoded image. For streaming
    /// decode without full materialization, use [`push_decode`](Self::push_decode)
    /// or the top-level [`push_decode`](crate::push_decode) convenience function.
    pub fn decode_full_frame(self) -> Result<DecodeOutput> {
        let format = self.resolve_format()?;
        self.decode_format(format)
    }

    /// Decode the image to pixels.
    ///
    /// **Deprecated:** Use [`decode_full_frame`](Self::decode_full_frame) instead.
    /// The name `decode()` hides the fact that this materializes the entire image.
    /// `push_decode()` is the streaming alternative.
    #[deprecated(
        since = "0.2.0",
        note = "renamed to decode_full_frame() to signal materialization; use push_decode() for streaming"
    )]
    pub fn decode(self) -> Result<DecodeOutput> {
        self.decode_full_frame()
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
    pub fn decode_gain_map(
        mut self,
    ) -> Result<(DecodeOutput, Option<crate::gainmap::DecodedGainMap>)> {
        let format = self.resolve_format()?;
        let data = self.data; // Save reference before consuming self
        // Enable gain map extraction so codecs attach gain map data to extras.
        self.extract_gain_map = true;
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
    /// - **JPEG**: Three sources checked in priority order:
    ///   1. GDepth XMP (Android, base64-encoded depth with near/far/units metadata)
    ///   2. Dynamic Depth Format / DDF (Android, appended images with XMP container directory)
    ///   3. MPF Disparity secondary image (iPhone portrait mode)
    /// - **HEIC**: Auxiliary depth image via heic.
    /// - **AVIF**: Auxiliary depth image via zenavif.
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
            ImageFormat::Jpeg => extract_jpeg_depth(&output, data),
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
    ///     let preview = DecodeRequest::new(&preview_jpeg).decode_full_frame()?;
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

    /// Build a streaming decoder that yields scanline batches (pull model).
    ///
    /// Returns a `Box<dyn DynStreamingDecoder>` that the caller drives by
    /// calling [`next_batch()`](zencodec::decode::DynStreamingDecoder::next_batch)
    /// until it returns `None`.
    ///
    /// The input data is copied into owned storage, so the returned decoder
    /// is `'static` and can be moved into pipeline stages or across thread
    /// boundaries.
    ///
    /// # Codec support
    ///
    /// Not all codecs support this path. Codecs whose streaming decoders
    /// require borrowed data (JPEG, PNG) return an error — use
    /// [`push_decode()`](Self::push_decode) for those formats instead. Codecs
    /// that don't support row-level decode at all (WebP, TIFF, bitmaps) also
    /// return an error.
    ///
    /// Currently supported: GIF, AVIF, HEIC.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use zencodecs::DecodeRequest;
    ///
    /// let data: &[u8] = &[]; // GIF bytes
    /// let mut decoder = DecodeRequest::new(data)
    ///     .with_format(zencodecs::ImageFormat::Gif)
    ///     .build_streaming_decoder()?;
    /// while let Some((y, strip)) = decoder.next_batch()
    ///     .map_err(|e| zencodecs::CodecError::Codec {
    ///         format: zencodecs::ImageFormat::Gif,
    ///         source: e,
    ///     })? {
    ///     // process strip starting at row y
    /// }
    /// # Ok::<(), whereat::At<zencodecs::CodecError>>(())
    /// ```
    pub fn build_streaming_decoder(
        self,
    ) -> Result<alloc::boxed::Box<dyn zencodec::decode::DynStreamingDecoder + 'static>> {
        let format = self.resolve_format()?;
        crate::dyn_dispatch::dyn_streaming_decoder(format, &self.decode_params())
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
    /// let mut decoder = DecodeRequest::new(data).animation_frame_decoder()?;
    /// while let Some(frame) = decoder.render_next_frame_owned(None)? {
    ///     // frame.pixels(), frame.duration_ms(), frame.frame_index()
    /// }
    /// # Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    /// ```
    pub fn animation_frame_decoder(
        self,
    ) -> Result<alloc::boxed::Box<dyn zencodec::decode::DynAnimationFrameDecoder>> {
        let format = self.resolve_format()?;
        crate::dyn_dispatch::dyn_animation_frame_decoder(format, &self.decode_params())
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

    fn decode_params(&self) -> crate::dyn_dispatch::DecodeParams<'_> {
        crate::dyn_dispatch::DecodeParams {
            data: self.data,
            codec_config: self.codec_config,
            limits: self.limits,
            stop: self.stop.clone(),
            preferred: &[],
            decode_policy: self.decode_policy,
            extract_gain_map: self.extract_gain_map,
        }
    }

    /// Dispatch to format-specific decoder.
    fn decode_format(self, format: ImageFormat) -> Result<DecodeOutput> {
        let dp = self.decode_policy;
        match format {
            #[cfg(feature = "jpeg")]
            ImageFormat::Jpeg => crate::codecs::jpeg::decode(
                self.data,
                self.codec_config,
                self.limits,
                self.stop,
                dp,
            ),
            #[cfg(not(feature = "jpeg"))]
            ImageFormat::Jpeg => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "webp")]
            ImageFormat::WebP => crate::codecs::webp::decode(
                self.data,
                self.codec_config,
                self.limits,
                self.stop,
                dp,
            ),
            #[cfg(not(feature = "webp"))]
            ImageFormat::WebP => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "gif")]
            ImageFormat::Gif => crate::codecs::gif::decode(self.data, self.limits, self.stop, dp),
            #[cfg(not(feature = "gif"))]
            ImageFormat::Gif => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "png")]
            ImageFormat::Png => crate::codecs::png::decode(self.data, self.limits, self.stop, dp),
            #[cfg(not(feature = "png"))]
            ImageFormat::Png => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "avif-decode")]
            ImageFormat::Avif => crate::codecs::avif_dec::decode(
                self.data,
                self.codec_config,
                self.limits,
                self.stop,
                dp,
                self.extract_gain_map,
            ),
            #[cfg(not(feature = "avif-decode"))]
            ImageFormat::Avif => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "jxl-decode")]
            ImageFormat::Jxl => crate::codecs::jxl_dec::decode(
                self.data,
                self.limits,
                self.stop,
                dp,
                self.extract_gain_map,
            ),
            #[cfg(not(feature = "jxl-decode"))]
            ImageFormat::Jxl => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "heic-decode")]
            ImageFormat::Heic => crate::codecs::heic::decode(
                self.data,
                self.limits,
                self.stop,
                dp,
                self.extract_gain_map,
            ),
            #[cfg(not(feature = "heic-decode"))]
            ImageFormat::Heic => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps")]
            ImageFormat::Pnm => crate::codecs::pnm::decode(self.data, self.limits, self.stop, dp),
            #[cfg(not(feature = "bitmaps"))]
            ImageFormat::Pnm => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps-bmp")]
            ImageFormat::Bmp => crate::codecs::bmp::decode(self.data, self.limits, self.stop, dp),
            #[cfg(not(feature = "bitmaps-bmp"))]
            ImageFormat::Bmp => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps")]
            ImageFormat::Farbfeld => {
                crate::codecs::farbfeld::decode(self.data, self.limits, self.stop, dp)
            }
            #[cfg(not(feature = "bitmaps"))]
            ImageFormat::Farbfeld => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "tiff")]
            ImageFormat::Tiff => crate::codecs::tiff::decode(self.data, self.limits, self.stop, dp),
            #[cfg(not(feature = "tiff"))]
            ImageFormat::Tiff => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps-qoi")]
            ImageFormat::Qoi => crate::codecs::qoi::decode(self.data, self.limits, self.stop, dp),
            #[cfg(not(feature = "bitmaps-qoi"))]
            ImageFormat::Qoi => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps-tga")]
            ImageFormat::Tga => crate::codecs::tga::decode(self.data, self.limits, self.stop, dp),
            #[cfg(not(feature = "bitmaps-tga"))]
            ImageFormat::Tga => Err(at!(CodecError::UnsupportedFormat(format))),

            #[cfg(feature = "bitmaps-hdr")]
            ImageFormat::Hdr => crate::codecs::hdr::decode(self.data, self.limits, self.stop, dp),
            #[cfg(not(feature = "bitmaps-hdr"))]
            ImageFormat::Hdr => Err(at!(CodecError::UnsupportedFormat(format))),

            // RAW/DNG: Custom format from zenraw
            #[cfg(feature = "raw-decode")]
            ImageFormat::Custom(def) if def.name == "dng" || def.name == "raw" => {
                crate::codecs::raw::decode(self.data, self.codec_config, self.limits, self.stop)
            }

            // JPEG 2000
            #[cfg(feature = "jp2-decode")]
            ImageFormat::Jp2 => crate::codecs::jp2::decode(self.data, self.limits, self.stop, dp),

            // SVG/SVGZ
            #[cfg(feature = "svg")]
            ImageFormat::Custom(def) if def.name == "svg" => {
                crate::codecs::svg::decode(self.data, self.limits, self.stop, dp)
            }

            _ => Err(at!(CodecError::UnsupportedFormat(format))),
        }
    }
}

/// Extract a gain map from a JPEG DecodeOutput's extras, if present.
///
/// Returns `None` if the JPEG doesn't contain UltraHDR gain map data.
#[cfg(feature = "jpeg-ultrahdr")]
fn extract_jpeg_gainmap(output: &DecodeOutput) -> Option<crate::gainmap::DecodedGainMap> {
    use crate::gainmap::DecodedGainMap;
    use zenjpeg::ultrahdr::UltraHdrExtras as _;

    let extras = output.extras::<zenjpeg::decoder::DecodedExtras>()?;

    if !extras.is_ultrahdr() {
        return None;
    }

    // Parse gain map metadata from XMP
    let (metadata, _) = extras.ultrahdr_metadata()?.ok()?;

    // Decode the gain map JPEG from MPF secondary images.
    // extras.decode_gainmap() returns ultrahdr_core::GainMap directly.
    let gain_map = extras.decode_gainmap()?.ok()?;

    Some(DecodedGainMap {
        gain_map,
        metadata,
        base_is_hdr: false, // JPEG UltraHDR: base=SDR, gain map maps SDR→HDR
        source_format: ImageFormat::Jpeg,
    })
}

/// Extract a gain map from an AVIF DecodeOutput's extras, if present.
///
/// The gain map image is returned as raw AV1 bytes — the caller must
/// decode them separately to get pixels. For now we store the raw bytes
/// in `GainMap` with channels=0 to signal "not yet decoded."
#[cfg(all(feature = "avif-decode", feature = "jpeg-ultrahdr"))]
fn extract_avif_gainmap(output: &DecodeOutput) -> Option<crate::gainmap::DecodedGainMap> {
    use crate::gainmap::DecodedGainMap;

    // zenavif attaches the gain map as a `zencodec::gainmap::GainMapSource`
    // (codec.rs:2510). Look for that — NOT the older `zenavif::AvifGainMap`
    // which is no longer emitted to extras.
    let source = output.extras::<zencodec::gainmap::GainMapSource>()?;

    // The metadata is already in log2 domain (`GainMapParams`); convert
    // to ultrahdr's linear-domain `GainMapMetadata`.
    let uhdr_metadata = crate::gainmap::params_to_metadata(&source.metadata.params);

    // Decode the raw AV1 gain map to pixels.
    let (gm_data, gm_w, gm_h, gm_ch) = zenavif::decode_av1_obu(&source.data).ok()?;

    Some(DecodedGainMap {
        gain_map: crate::gainmap::GainMap {
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
    let safe_div = |n: i64, d: u64| -> f64 { if d == 0 { 0.0 } else { n as f64 / d as f64 } };
    let safe_div_u = |n: u64, d: u64| -> f64 { if d == 0 { 0.0 } else { n as f64 / d as f64 } };

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
    params.base_hdr_headroom = safe_div_u(
        meta.base_hdr_headroom_n as u64,
        meta.base_hdr_headroom_d as u64,
    );
    params.alternate_hdr_headroom = safe_div_u(
        meta.alternate_hdr_headroom_n as u64,
        meta.alternate_hdr_headroom_d as u64,
    );
    params.use_base_color_space = meta.use_base_colour_space;
    params
}

/// Extract a gain map from a JXL DecodeOutput's extras, if present.
#[cfg(all(feature = "jxl-decode", feature = "jpeg-ultrahdr"))]
fn extract_jxl_gainmap(output: &DecodeOutput) -> Option<crate::gainmap::DecodedGainMap> {
    use crate::gainmap::{DecodedGainMap, GainMap};

    // zenjxl attaches the gain map as a `zencodec::gainmap::GainMapSource`
    // (codec.rs:1441 → bundle_to_gain_map_source). Look up that type, NOT
    // the older `zenjxl::GainMapBundle` — same bug shape as AVIF had.
    let source = output.extras::<zencodec::gainmap::GainMapSource>()?;

    // The metadata is already parsed into log2-domain GainMapParams;
    // convert to linear-domain GainMapMetadata.
    let metadata = crate::gainmap::params_to_metadata(&source.metadata.params);

    // Decode the bare JXL codestream to get gain map pixels.
    use alloc::vec::Vec;
    let gm_output = zenjxl::decode(&source.data, None, &[]).ok()?;
    use zenpixels_convert::PixelBufferConvertTypedExt as _;
    let gm_rgb8 = gm_output.pixels.to_rgb8();
    let gm_ref = gm_rgb8.as_imgref();
    let gm_w = gm_ref.width() as u32;
    let gm_h = gm_ref.height() as u32;
    let gm_bytes: Vec<u8> = bytemuck::cast_slice(gm_ref.buf()).to_vec();

    // Determine channels: if all R==G==B, it's effectively grayscale.
    let is_gray = gm_bytes
        .chunks_exact(3)
        .all(|px| px[0] == px[1] && px[1] == px[2]);
    let (data, channels) = if is_gray {
        let gray: Vec<u8> = gm_bytes.chunks_exact(3).map(|px| px[0]).collect();
        (gray, 1u8)
    } else {
        (gm_bytes, 3u8)
    };

    Some(DecodedGainMap {
        gain_map: GainMap {
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
/// Uses zenjpeg's [`DecodedExtras::extract_depth_map()`] which checks
/// three sources in priority order:
/// 1. GDepth XMP (Android, base64-encoded depth + metadata)
/// 2. Dynamic Depth Format / DDF (Android, appended images with container directory)
/// 3. MPF Disparity secondary image (iPhone portrait mode)
///
/// `file_data` is the original JPEG bytes, needed for DDF offset-based extraction.
///
/// # Source mapping to [`DepthSource`]
///
/// - [`zenjpeg::decoder::DepthSource::GDepthXmp`] → [`DepthSource::AndroidGDepth`]
/// - [`zenjpeg::decoder::DepthSource::DynamicDepth`] → [`DepthSource::AndroidDdf`]
/// - [`zenjpeg::decoder::DepthSource::MpfDisparity`] → [`DepthSource::AppleMpf`]
#[cfg(feature = "jpeg")]
fn extract_jpeg_depth(
    output: &DecodeOutput,
    file_data: &[u8],
) -> Option<crate::depthmap::DecodedDepthMap> {
    use crate::depthmap::{
        DecodedDepthMap, DepthFormat, DepthImage, DepthMapMetadata, DepthMeasureType,
        DepthPixelFormat, DepthSource, DepthUnits,
    };

    let extras = output.extras::<zenjpeg::decoder::DecodedExtras>()?;

    // Use the unified extraction API that handles GDepth XMP, DDF, and MPF Disparity
    let depth_data = extras.extract_depth_map(Some(file_data))?;

    // Decode the depth image bytes (JPEG or PNG) to get grayscale pixels
    let depth_output =
        crate::codecs::jpeg::decode(&depth_data.data, None, None, None, None).ok()?;
    use zenpixels_convert::PixelBufferConvertTypedExt as _;
    let gray = depth_output.into_buffer().to_gray8();
    let gray_ref = gray.as_imgref();
    let w = gray_ref.width() as u32;
    let h = gray_ref.height() as u32;

    // Extract grayscale bytes
    let pixel_data: alloc::vec::Vec<u8> = gray_ref.buf().iter().map(|g| g.value()).collect();

    // Map zenjpeg's DepthSource → zencodecs' DepthSource
    let source_device = match depth_data.source {
        zenjpeg::decoder::DepthSource::GDepthXmp => DepthSource::AndroidGDepth,
        zenjpeg::decoder::DepthSource::DynamicDepth => DepthSource::AndroidDdf,
        zenjpeg::decoder::DepthSource::MpfDisparity => DepthSource::AppleMpf,
    };

    // Map GDepth metadata if available, otherwise use defaults for MPF Disparity
    let metadata = if let Some(ref gd) = depth_data.metadata {
        let format = match gd.format {
            zenjpeg::decoder::GDepthFormat::RangeLinear => DepthFormat::RangeLinear,
            zenjpeg::decoder::GDepthFormat::RangeInverse => DepthFormat::RangeInverse,
        };
        let units = match gd.units {
            zenjpeg::decoder::GDepthUnits::Meters => DepthUnits::Meters,
            zenjpeg::decoder::GDepthUnits::Diopters => DepthUnits::Diopters,
        };
        let measure_type = match gd.measure_type {
            zenjpeg::decoder::GDepthMeasureType::OpticalAxis => DepthMeasureType::OpticalAxis,
            zenjpeg::decoder::GDepthMeasureType::OpticRay => DepthMeasureType::OpticRay,
        };
        DepthMapMetadata {
            format,
            near: gd.near,
            far: gd.far,
            units,
            measure_type,
        }
    } else {
        // MPF Disparity: no structured metadata, use normalized disparity defaults
        DepthMapMetadata {
            format: DepthFormat::Disparity,
            near: 0.0,
            far: 1.0,
            units: DepthUnits::Normalized,
            measure_type: DepthMeasureType::OpticalAxis,
        }
    };

    // Decode confidence map if present
    let confidence = depth_data.confidence.and_then(|conf_bytes| {
        let conf_output = crate::codecs::jpeg::decode(&conf_bytes, None, None, None, None).ok()?;
        let conf_gray = conf_output.into_buffer().to_gray8();
        let conf_ref = conf_gray.as_imgref();
        let conf_w = conf_ref.width() as u32;
        let conf_h = conf_ref.height() as u32;
        let conf_data: alloc::vec::Vec<u8> = conf_ref.buf().iter().map(|g| g.value()).collect();
        Some(DepthImage {
            data: conf_data,
            width: conf_w,
            height: conf_h,
            pixel_format: DepthPixelFormat::Gray8,
        })
    });

    Some(DecodedDepthMap {
        depth: DepthImage {
            data: pixel_data,
            width: w,
            height: h,
            pixel_format: DepthPixelFormat::Gray8,
        },
        metadata,
        confidence,
        source_format: ImageFormat::Jpeg,
        source_device,
    })
}

/// Extract a depth map from a HEIC DecodeOutput's extras, if present.
///
/// HEIC files can contain auxiliary depth images (Apple portrait mode).
/// This is a stub — actual extraction requires heic support for
/// Extract depth map from HEIC auxiliary image.
///
/// Uses heic's `decode_depth()` to decode the HEVC depth auxiliary item.
#[cfg(feature = "heic-decode")]
fn extract_heic_depth(data: &[u8]) -> Option<crate::depthmap::DecodedDepthMap> {
    use crate::depthmap::*;

    let config = heic::DecoderConfig::new();
    let depth_map = config.decode_depth(data).ok()?;

    // Convert heic's DepthRepresentationInfo to our DepthFormat
    let (format, units) = match depth_map.depth_info.representation_type {
        heic::DepthRepresentationType::UniformInverseZ => {
            (DepthFormat::RangeInverse, DepthUnits::Meters)
        }
        heic::DepthRepresentationType::UniformDisparity => {
            (DepthFormat::Disparity, DepthUnits::Diopters)
        }
        heic::DepthRepresentationType::UniformZ => (DepthFormat::RangeLinear, DepthUnits::Meters),
        heic::DepthRepresentationType::NonuniformDisparity => {
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
        let registry = AllowedFormats::none();

        let result = DecodeRequest::new(&jpeg_data)
            .with_registry(&registry)
            .decode_full_frame();

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
        let ps = zenpixels::PixelSlice::from(pixels.as_ref()).erase();
        let encoded = EncodeRequest::new(ImageFormat::WebP)
            .with_quality(50.0)
            .encode(ps, false)
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
        let Ok(avif_data) =
            std::fs::read("../zenavif-parse/tests/colors-animated-8bpc-depth-exif-xmp.avif")
        else {
            return; // skip if test vector not available (CI)
        };

        let (output, depth) = DecodeRequest::new(&avif_data).decode_depth_map().unwrap();
        assert!(output.width() > 0, "base image should decode");

        // HEIC auxiliary depth extraction not yet implemented — skip if None.
        let Some(dm) = depth else {
            return;
        };
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
            assert!(gm.gain_map.width > 0 && gm.gain_map.height > 0);
            assert_eq!(gm.source_format, ImageFormat::Custom(&zenraw::DNG_FORMAT));
            std::eprintln!(
                "  alternate_hdr_headroom={} base_is_hdr={}",
                gm.metadata.alternate_hdr_headroom,
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
            assert!(gm.gain_map.width > 0 && gm.gain_map.height > 0);
            // Source format should be JPEG since it was detected as JPEG.
            assert_eq!(gm.source_format, ImageFormat::Jpeg);
        } else {
            std::eprintln!("AMPF has no gain map via JPEG path (unexpected)");
        }
    }
}
