//! Image decoding.

pub use zencodec::decode::DecodeOutput;

use crate::config::CodecConfig;
use crate::error::Result;
use crate::estimate::{ComputeEnvironment, ImageCharacteristics, ResourceEstimate};
use crate::macros::dispatch_format;
use crate::policy::CodecPolicy;
use crate::{AllowedFormats, CodecError, ImageFormat, ImageInfo, Limits, StopToken};
use whereat::at;
use zencodec::decode::DecodePolicy;
use zencodec::{GainMapRender, OrientationHint};

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
    /// How a gain-map (HDR) image is rendered: SDR base (default),
    /// reconstructed HDR pixels, or surfaced components. See
    /// [`with_gain_map_render`](Self::with_gain_map_render).
    gain_map_render: GainMapRender,
    /// Whether the decoder bakes EXIF/container orientation into the pixels.
    /// Default: [`OrientationHint::Preserve`]. See
    /// [`with_orientation`](Self::with_orientation).
    orientation: OrientationHint,
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
            gain_map_render: GainMapRender::BaseOnly,
            orientation: OrientationHint::Preserve,
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

    /// Select how a gain-map (HDR) image is rendered by [`decode_full_frame`](Self::decode_full_frame).
    ///
    /// - [`GainMapRender::BaseOnly`] (default): decode the SDR base image only.
    /// - [`GainMapRender::ReconstructHdr`]: apply the gain map and return
    ///   reconstructed HDR pixels (a float pixel format) plus a content-light-level
    ///   / mastering-display envelope on the output [`ImageInfo`]. Honored only by
    ///   decoders that advertise HDR reconstruction — currently JPEG UltraHDR; other
    ///   formats return [`CodecError::UnsupportedOperation`] rather than silently
    ///   handing back an SDR buffer. Prefer the [`reconstruct_hdr`](Self::reconstruct_hdr)
    ///   shorthand.
    /// - [`GainMapRender::Components`]: surface the gain map alongside the SDR base —
    ///   equivalent to [`with_gain_map_extraction(true)`](Self::with_gain_map_extraction).
    pub fn with_gain_map_render(mut self, render: GainMapRender) -> Self {
        self.gain_map_render = render;
        self
    }

    /// Control whether the decoder bakes EXIF/container orientation into the
    /// returned pixels.
    ///
    /// - [`OrientationHint::Preserve`] (default): return pixels as stored; the
    ///   orientation lives in the metadata tag (right for tag-carrying targets).
    /// - [`OrientationHint::Correct`]: bake orientation into the pixels and
    ///   report [`Orientation::Identity`](zencodec::Orientation::Identity) — the
    ///   right choice for targets that can't carry an orientation tag (PNG) or
    ///   for display-ready output.
    ///
    /// Currently honored by the JPEG and HEIC decoders (the EXIF-orientation
    /// formats); other adapters ignore it (no stored orientation to bake).
    pub fn with_orientation(mut self, hint: OrientationHint) -> Self {
        self.orientation = hint;
        self
    }

    /// Reconstruct HDR from an embedded gain map and return HDR pixels.
    ///
    /// Shorthand for [`with_gain_map_render`](Self::with_gain_map_render) with
    /// [`GainMapRender::ReconstructHdr`]. `target_headroom` of `None` reconstructs
    /// at the gain map's encoded maximum (full reconstruction, the right choice for
    /// transcoding to native HDR); `Some(h)` renders for a display with `h`× SDR-white
    /// headroom. The decoded [`DecodeOutput`] carries HDR pixels and a
    /// content-light-level / mastering-display envelope on its [`ImageInfo`].
    ///
    /// Currently honored by JPEG UltraHDR (requires the `jpeg-ultrahdr` feature);
    /// other formats return [`CodecError::UnsupportedOperation`] until their
    /// decoders advertise HDR reconstruction.
    pub fn reconstruct_hdr(mut self, target_headroom: Option<f32>) -> Self {
        self.gain_map_render = GainMapRender::ReconstructHdr { target_headroom };
        self
    }

    /// Resolve format (auto-detect or explicit) and check registry.
    fn resolve_format(&self) -> Result<ImageFormat> {
        let default_registry = AllowedFormats::all();
        let registry = self.registry.unwrap_or(&default_registry);
        let format = match self.format {
            Some(f) => f,
            None => crate::info::detect_format(self.data)
                .ok_or_else(|| at!(CodecError::UnrecognizedFormat))?,
        };
        // `AllowedFormats::can_decode` gates `ImageFormat::Custom` (RAW/DNG/
        // PDF) by name, same as any named format — no bypass, whether the
        // format came from an explicit `with_format(Custom(...))` or from
        // auto-detection. `AllowedFormats::none()` denies Custom formats too;
        // `all()` allows every compiled-in one (named or Custom).
        if !registry.can_decode(format) {
            return Err(at!(CodecError::DisabledFormat(format)));
        }
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
    /// The returned [`DecodedGainMap`] carries the gain map pixels + ISO 21496-1
    /// metadata; reconstruct the alternate rendition with
    /// [`zenjpeg::ultrahdr::apply_gainmap()`].
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
        // Borrowed before `self` is consumed below; only the raw-decode-gainmap
        // match arms read it, so it's unused when that feature is off.
        #[cfg_attr(not(feature = "raw-decode-gainmap"), allow(unused_variables))]
        let data = self.data;
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

    /// Estimate decode peak memory + wall time **without decoding**.
    ///
    /// Probes the header for dimensions, then queries the calibrated cross-codec
    /// decode cost model. Returns [`ResourceEstimate::unknown`] only when the
    /// format's decoder isn't compiled in. Use it for admission control: compare
    /// the est against your budget before committing to the decode. The decode
    /// path also enforces `Limits::max_memory_bytes` against this estimate
    /// automatically (see [`with_limits`](Self::with_limits)).
    pub fn estimate(&self) -> Result<ResourceEstimate> {
        let format = self.resolve_format()?;
        Ok(self.decode_estimate(format)?.1)
    }

    /// Probe dims + build the `(image, decode estimate)` pair for `format`.
    fn decode_estimate(
        &self,
        format: ImageFormat,
    ) -> Result<(ImageCharacteristics, ResourceEstimate)> {
        let info = crate::info::probe_format(self.data, format)?;
        // The pipeline decodes to a 4-byte RGBA8/BGRA8 frame; size the estimate to
        // that intermediate (conservative for narrower native outputs).
        let image = ImageCharacteristics::new(
            info.width,
            info.height,
            zenpixels::PixelDescriptor::RGBA8_SRGB,
        );
        let est = crate::estimate::estimate_decode(format, &image, &ComputeEnvironment::new());
        Ok((image, est))
    }

    /// Pre-flight memory admission: when `limits.max_memory_bytes` is set, reject
    /// (before constructing any decoder) if the calibrated decode est exceeds it.
    fn check_decode_admission(&self, format: ImageFormat) -> Result<()> {
        let Some(limits) = self.limits else {
            return Ok(());
        };
        if limits.max_memory_bytes.is_none() {
            return Ok(());
        }
        let (image, est) = self.decode_estimate(format)?;
        crate::estimate::check_estimate_against_limits(&est, &image, limits)
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
        // Pre-flight: reject over-budget decodes before constructing the decoder.
        self.check_decode_admission(format)?;
        let dp = self.decode_policy;
        // ReconstructHdr returns HDR pixels only from decoders that advertise the
        // capability — JPEG UltraHDR and HEIC (Apple/Samsung gain maps); every
        // other format errors here rather than silently returning an SDR buffer
        // mislabeled as HDR.
        if matches!(self.gain_map_render, GainMapRender::ReconstructHdr { .. })
            && !matches!(format, ImageFormat::Jpeg | ImageFormat::Heic)
        {
            return Err(at!(CodecError::UnsupportedOperation {
                format,
                detail: "HDR reconstruction (GainMapRender::ReconstructHdr) is not \
                         supported for this format",
            }));
        }
        dispatch_format! {
            format, unsupported = Err(at!(CodecError::UnsupportedFormat(format)));
            Jpeg => "jpeg" => crate::codecs::jpeg::decode(
                self.data,
                self.codec_config,
                self.limits,
                self.stop,
                dp,
                self.gain_map_render,
                self.orientation,
            ),
            WebP => "webp" => crate::codecs::webp::decode(
                self.data,
                self.codec_config,
                self.limits,
                self.stop,
                dp,
            ),
            Gif => "gif" => crate::codecs::gif::decode(self.data, self.limits, self.stop, dp),
            Png => "png" => crate::codecs::png::decode(self.data, self.limits, self.stop, dp),
            Avif => "avif-decode" => crate::codecs::avif_dec::decode(
                self.data,
                self.codec_config,
                self.limits,
                self.stop,
                dp,
                self.extract_gain_map,
            ),
            Jxl => "jxl-decode" => crate::codecs::jxl_dec::decode(
                self.data,
                self.limits,
                self.stop,
                dp,
                self.extract_gain_map,
            ),
            Heic => "heic-decode" => crate::codecs::heic::decode(
                self.data,
                self.limits,
                self.stop,
                dp,
                self.gain_map_render,
                self.extract_gain_map,
                self.orientation,
            ),
            Pnm => "bitmaps" => crate::codecs::pnm::decode(self.data, self.limits, self.stop, dp),
            Bmp => "bitmaps-bmp" => crate::codecs::bmp::decode(self.data, self.limits, self.stop, dp),
            Farbfeld => "bitmaps" => crate::codecs::farbfeld::decode(self.data, self.limits, self.stop, dp),
            Tiff => "tiff" => crate::codecs::tiff::decode(self.data, self.limits, self.stop, dp),
            Qoi => "bitmaps-qoi" => crate::codecs::qoi::decode(self.data, self.limits, self.stop, dp),
            Tga => "bitmaps-tga" => crate::codecs::tga::decode(self.data, self.limits, self.stop, dp),
            Hdr => "bitmaps-hdr" => crate::codecs::hdr::decode(self.data, self.limits, self.stop, dp);
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
            // PDF: Custom format from zenpdf (renders page 0)
            #[cfg(feature = "pdf-decode")]
            ImageFormat::Custom(def) if def.name == "pdf" => {
                crate::codecs::pdf::decode(self.data, self.limits, self.stop, dp)
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

    // `GainMapMetadata` is an alias for `GainMapParams` (ultrahdr-core 0.5),
    // so the parsed params are used directly.
    let uhdr_metadata = source.metadata.params.clone();

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

/// Extract a gain map from a JXL DecodeOutput's extras, if present.
#[cfg(all(feature = "jxl-decode", feature = "jpeg-ultrahdr"))]
fn extract_jxl_gainmap(output: &DecodeOutput) -> Option<crate::gainmap::DecodedGainMap> {
    use crate::gainmap::{DecodedGainMap, GainMap};

    // zenjxl attaches the gain map as a `zencodec::gainmap::GainMapSource`
    // (codec.rs:1441 → bundle_to_gain_map_source). Look up that type, NOT
    // the older `zenjxl::GainMapBundle` — same bug shape as AVIF had.
    let source = output.extras::<zencodec::gainmap::GainMapSource>()?;

    // `GainMapMetadata` is an alias for `GainMapParams` (ultrahdr-core 0.5),
    // so the parsed params are used directly.
    let metadata = source.metadata.params.clone();

    // Decode the bare JXL codestream to get gain map pixels.
    use alloc::vec::Vec;
    let gm_output = zenjxl::decode(&source.data, None, &[]).ok()?;
    use zenpixels_convert::PixelBufferConvertTypedExt as _;
    let gm_rgb8 = gm_output.pixels.to_rgb8();
    let gm_ref = gm_rgb8.as_imgref();
    let gm_w = gm_ref.width() as u32;
    let gm_h = gm_ref.height() as u32;
    let gm_bytes: Vec<u8> = bytemuck::cast_slice(gm_ref.buf()).to_vec();

    // Collapse to single-channel when provably achromatic — the shared
    // load-bearing analysis (R==G==B over every pixel) is the predicate.
    use zenpixels_convert::PixelSliceLoadBearingExt as _;
    let is_gray = gm_rgb8.as_slice().determine_load_bearing().uses_chroma == Some(false);
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

    // ═══════════════════════════════════════════════════════════════════
    // Custom-format (RAW/DNG) registry gating — decode side. `resolve_format`
    // used to special-case `ImageFormat::Custom` as a fail-open bypass: even
    // `AllowedFormats::none()` (nothing allowed) would still let a Custom
    // format straight through to the decoder. Pin the deny direction here;
    // the allow direction (registry.rs's `all_allows_compiled_custom_raw_dng`)
    // already covers `can_decode` itself.
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    #[cfg(feature = "raw-decode")]
    fn custom_format_none_denies_dng_decode_no_bypass() {
        // Garbage bytes: if the old fail-open bypass were still in place,
        // this would reach zenraw's decoder (and fail with a *different*
        // error) instead of being rejected by the registry up front.
        let not_really_dng = b"not a real DNG file at all";
        let registry = AllowedFormats::none();

        let result = DecodeRequest::new(not_really_dng)
            .with_format(ImageFormat::Custom(&zenraw::DNG_FORMAT))
            .with_registry(&registry)
            .decode_full_frame();

        assert!(
            matches!(
                result.as_ref().map_err(|e| e.error()),
                Err(CodecError::DisabledFormat(_))
            ),
            "AllowedFormats::none() must reject an explicit Custom format before \
             it ever reaches the codec: {result:?}"
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
