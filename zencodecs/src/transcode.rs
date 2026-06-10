//! Transcode API and streaming decode→encode bridge.
//!
//! ## Transcode function
//!
//! [`transcode()`] provides a high-level one-call transcode: decode the input,
//! re-encode to the target format specified by a [`FormatDecision`], and return
//! the encoded bytes. Metadata (EXIF, ICC, XMP) is roundtripped by default.
//!
//! ## TranscodeSink
//!
//! [`TranscodeSink`] is the low-level streaming bridge. It implements
//! [`DecodeRowSink`] and forwards decoded strips directly to an encoder's
//! `push_rows()`, converting pixel formats per-strip via `adapt_for_encode`.
//! No full-image buffer is ever allocated by the sink — only a strip-sized
//! conversion buffer when the decoded pixel format doesn't match the
//! encoder's native format.
//!
//! Codecs that need the full image (WebP, AVIF) buffer internally in their
//! `push_rows()` implementation. That's the codec's concern, not the
//! pipeline's.

use alloc::boxed::Box;
use alloc::vec::Vec;

use zencodec::MetadataPolicy;
use zencodec::decode::{DecodeRowSink, SinkError};
use zencodec::encode::{DynEncoder, EncodeOutput};
use zenpixels::{PixelDescriptor, PixelSliceMut};

use crate::decision::FormatDecision;
use crate::error::Result;
use crate::{AllowedFormats, CodecError, ImageFormat};
use whereat::at;

// ═══════════════════════════════════════════════════════════════════════
// TranscodeOptions, SupplementPolicy, SupplementSet
// ═══════════════════════════════════════════════════════════════════════

/// Options controlling a transcode operation.
///
/// Controls metadata roundtrip, supplement handling, and alpha compositing.
#[derive(Clone, Debug)]
pub struct TranscodeOptions {
    /// Metadata to embed in the output (EXIF, ICC, XMP).
    ///
    /// - `None` (default): extract metadata from the source and roundtrip it.
    /// - `Some(meta)`: use the provided metadata instead of the source's.
    pub metadata: Option<zencodec::Metadata>,

    /// Retention policy applied to the embedded metadata.
    ///
    /// Defaults to [`MetadataPolicy::PreserveExact`] (verbatim roundtrip, with a
    /// stale EXIF orientation tag reconciled against the authoritative field).
    /// Set [`MetadataPolicy::Web`] to strip GPS/camera/timestamps/XMP for web
    /// publishing while keeping orientation, rights, and color signaling.
    pub metadata_policy: MetadataPolicy,

    /// How to handle container supplements (gain maps, depth maps, etc.)
    /// during transcode.
    pub supplements: SupplementPolicy,

    /// Matte color for alpha compositing when encoding to a format without
    /// alpha (e.g., RGBA source → JPEG output).
    ///
    /// `None` defaults to white `[255, 255, 255]`.
    pub matte: Option<[u8; 3]>,
}

impl Default for TranscodeOptions {
    fn default() -> Self {
        Self {
            metadata: None,
            // No implicit privacy choice: roundtrip verbatim by default. Callers
            // publishing to the web should set `MetadataPolicy::Web`.
            metadata_policy: MetadataPolicy::PreserveExact,
            supplements: SupplementPolicy::default(),
            matte: None,
        }
    }
}

/// What to do with container supplements (gain maps, depth maps, etc.)
/// during transcode.
#[derive(Clone, Copy, Debug, Default)]
pub enum SupplementPolicy {
    /// Roundtrip all supplements the target format supports.
    ///
    /// Gain maps, depth maps, and auxiliary images are extracted from the
    /// source container and re-embedded in the output container.
    /// Supplements that the target format can't represent are silently dropped.
    #[default]
    Preserve,

    /// Strip all supplements. Output contains only the primary image + metadata.
    Strip,

    /// Preserve only specific supplement types.
    Only(SupplementSet),
}

/// Bitflag set of supplement types.
///
/// Used with [`SupplementPolicy::Only`] to selectively preserve supplements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupplementSet(u32);

impl SupplementSet {
    /// UltraHDR / ISO 21496-1 gain map.
    pub const GAIN_MAP: Self = Self(1);
    /// Depth / disparity map.
    pub const DEPTH_MAP: Self = Self(2);
    /// Embedded thumbnail.
    pub const THUMBNAIL: Self = Self(4);

    /// Check whether a specific supplement type is in this set.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// Combine two supplement sets (union).
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Check whether this set is empty.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::ops::BitOr for SupplementSet {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TranscodeOutput
// ═══════════════════════════════════════════════════════════════════════

/// The result of a [`transcode()`] operation.
#[derive(Clone, Debug)]
pub struct TranscodeOutput {
    /// The encoded image bytes.
    pub data: Vec<u8>,
    /// The output format.
    pub format: ImageFormat,
    /// The MIME type of the output format.
    pub mime_type: &'static str,
}

// ═══════════════════════════════════════════════════════════════════════
// transcode() — high-level one-call transcode
// ═══════════════════════════════════════════════════════════════════════

/// Transcode an image: decode from `data`, re-encode to the format and
/// quality specified by `decision`.
///
/// This is the primary transcode entry point. Metadata (EXIF, ICC, XMP)
/// is roundtripped from the source unless overridden in `opts`.
///
/// # Current implementation
///
/// Uses the one-shot decode + encode path internally: decodes the full
/// image, then re-encodes it. This materializes the entire image in memory.
///
/// # TODO
///
/// Wire through [`TranscodeSink`] for true zero-materialization streaming.
/// The API shape is stable; only the internals will change.
///
/// # Example
///
/// ```rust,ignore
/// use zencodecs::{transcode, TranscodeOptions, FormatDecision, AllowedFormats};
/// use zencodecs::quality::QualityIntent;
/// use zencodecs::ImageFormat;
///
/// let decision = FormatDecision {
///     format: ImageFormat::WebP,
///     quality: QualityIntent::from_quality(80.0),
///     lossless: false,
///     hints: Default::default(),
///     matte: None,
///     trace: Vec::new(),
/// };
///
/// let output = zencodecs::transcode(
///     &jpeg_bytes,
///     &decision,
///     &TranscodeOptions::default(),
///     &AllowedFormats::all(),
/// )?;
/// assert_eq!(output.format, ImageFormat::WebP);
/// ```
pub fn transcode(
    data: &[u8],
    decision: &FormatDecision,
    opts: &TranscodeOptions,
    registry: &AllowedFormats,
) -> Result<TranscodeOutput> {
    // Determine whether we need gain map data from the decode side.
    let wants_gain_map = match opts.supplements {
        SupplementPolicy::Preserve => true,
        SupplementPolicy::Only(set) => set.contains(SupplementSet::GAIN_MAP),
        SupplementPolicy::Strip => false,
    };

    // Step 1: Decode the source image (full materialization for now)
    let decoded = crate::DecodeRequest::new(data)
        .with_registry(registry)
        .with_gain_map_extraction(wants_gain_map)
        .decode_full_frame()?;

    // Step 2: Determine metadata to embed
    let metadata = match opts.metadata.clone() {
        Some(m) => m,
        None => {
            // Roundtrip metadata from source via probe
            match crate::info::from_bytes_with_registry(data, registry) {
                Ok(info) => info.metadata(),
                Err(_) => {
                    // No metadata to roundtrip — proceed without it
                    zencodec::Metadata::none()
                }
            }
        }
    };

    // Step 3: Encode to the target format
    let format = decision.format;
    if !registry.can_encode(format) {
        return Err(at!(CodecError::DisabledFormat(format)));
    }

    // Build the encode request from the decision
    let mut request = crate::EncodeRequest::new(format)
        .with_quality(decision.quality.quality)
        .with_metadata(metadata)
        .with_metadata_policy(opts.metadata_policy)
        .with_registry(registry);

    if decision.lossless {
        request = request.with_lossless(true);
    }
    if let Some(effort) = decision.quality.effort {
        request = request.with_effort(effort);
    }

    let buffer = decoded.into_buffer();
    let encode_output = request.encode(buffer.as_slice(), buffer.descriptor().has_alpha())?;

    Ok(TranscodeOutput {
        data: encode_output.into_vec(),
        format,
        mime_type: format.mime_type(),
    })
}

// ═══════════════════════════════════════════════════════════════════════
// transcode_to_quality — hit a zensim Profile-A QualityTarget, one-shot
// ═══════════════════════════════════════════════════════════════════════

/// What a transcode quality target means relative to the (already lossy) source,
/// in **zensim Profile-A** units (0–100, higher = closer to the original).
///
/// The source is itself lossy, so the two readings differ — see
/// [`TRANSCODE-GRID.md`](https://github.com/imazen/zenpipe/blob/main/zencodecs/TRANSCODE-GRID.md).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QualityTarget {
    /// zensim-A vs the **original** image (the default meaning). Because the source
    /// already discarded detail, the achievable target is clamped to the source's
    /// inferred quality-vs-original floor — the output is never promised more
    /// fidelity than the source itself has. One number, comparable across sources.
    Absolute(f32),
    /// zensim-A vs the **decoded source pixels** — a cap on generation loss. No
    /// floor model; not comparable across sources of different starting quality.
    Relative(f32),
}

impl QualityTarget {
    /// The bare zensim-A score (0–100), regardless of reference.
    pub fn score(self) -> f32 {
        match self {
            Self::Absolute(t) | Self::Relative(t) => t,
        }
    }
}

/// Transcode `data` to `target` at the **smallest byte size meeting a zensim
/// Profile-A [`QualityTarget`]**, using the best method for the source→dest pair.
///
/// **One-shot** — the result is produced by either a coefficient-domain transcode
/// or a single picker-predicted encode; there is no bisection search.
///
/// 1. **Coefficient-domain** (no pixel round-trip, no generation loss) where the
///    pair shares a DCT representation:
///    - **JPEG→JPEG** — [`zenjpeg::recompress`] (re-quantize coefficients).
///    - **JPEG→JXL** — [`zenjxl::jpeg_lossy`] (coarsen + lossless floor; needs
///      `transcode-iqa`).
///    - **JXL→JPEG** — [`reconstruct_jpeg_from_jxl`] when the JXL is a JBRD
///      transcode (byte-exact); otherwise falls through to the JPEG picker.
/// 2. **One-shot picker** otherwise — decode the source once, ask the dest codec's
///    learned picker for the knob that lands on `target`, and encode once. See
///    the `TRANSCODE-GRID.md` doc for the per-dest predictor.
pub fn transcode_to_quality(
    data: &[u8],
    target: ImageFormat,
    quality: QualityTarget,
    opts: &TranscodeOptions,
    registry: &AllowedFormats,
) -> Result<TranscodeOutput> {
    let source =
        crate::info::detect_format(data).ok_or_else(|| at!(CodecError::UnrecognizedFormat))?;

    // 1. Coefficient-domain native paths — no pixel round-trip.
    match (source, target) {
        #[cfg(feature = "jpeg")]
        (ImageFormat::Jpeg, ImageFormat::Jpeg) => return recompress_jpeg_to_jpeg(data, quality),
        #[cfg(all(feature = "jpeg", feature = "transcode-iqa", feature = "jxl-decode"))]
        (ImageFormat::Jpeg, ImageFormat::Jxl) => return recompress_jpeg_to_jxl(data, quality),
        #[cfg(feature = "jxl-jpeg-reconstruct")]
        (ImageFormat::Jxl, ImageFormat::Jpeg) => {
            // Byte-exact iff the JXL is a JBRD JPEG transcode; otherwise fall
            // through to the one-shot JPEG picker on the decoded pixels.
            if let Some(jpeg) = reconstruct_jpeg_from_jxl(data)? {
                return Ok(TranscodeOutput {
                    data: jpeg,
                    format: ImageFormat::Jpeg,
                    mime_type: ImageFormat::Jpeg.mime_type(),
                });
            }
        }
        _ => {}
    }

    // 2. One-shot picker: decode source → dest picker → encode once.
    transcode_via_picker(data, target, quality, opts, registry)
}

/// One-shot picker dispatch: decode the source to pixels once, ask the dest
/// codec's learned picker for the knob that lands on `target`, and encode once.
///
/// Wired column-by-column (feature-aware pickers first); see the build order in
/// `TRANSCODE-GRID.md`.
fn transcode_via_picker(
    data: &[u8],
    target: ImageFormat,
    quality: QualityTarget,
    opts: &TranscodeOptions,
    registry: &AllowedFormats,
) -> Result<TranscodeOutput> {
    let _ = (data, quality, opts, registry);
    Err(at!(CodecError::UnsupportedOperation {
        format: target,
        detail: "one-shot picker not yet wired for this dest. Planned predictors: \
                 AVIF (zenavif auto_tune), JPEG (zenjpeg encode/picker), WebP \
                 (zenwebp picker), JXL (calibrated_jxl_quality). See \
                 TRANSCODE-GRID.md.",
    }))
}

/// JPEG→JPEG: coefficient-domain recompression to a zensim Profile-A target.
/// Returns the source bytes unchanged when recompression wouldn't beat it
/// (`RecompressResult::NoOp`), preserving the no-size-regression invariant.
///
/// `zenjpeg::recompress` targets absolute zensim-A vs the original and floor-clamps
/// to the source, so [`QualityTarget::Absolute`] is exact; a
/// [`Relative`](QualityTarget::Relative) target is driven at the same numeric level
/// (re-quantizing the same coefficients in place).
#[cfg(feature = "jpeg")]
fn recompress_jpeg_to_jpeg(data: &[u8], quality: QualityTarget) -> Result<TranscodeOutput> {
    use zenjpeg::recompress::{RecompressOptions, recompress};
    let result = recompress(data, &RecompressOptions::new(quality.score())).map_err(|e| {
        at!(CodecError::InvalidInput(alloc::format!(
            "jpeg recompress: {e}"
        )))
    })?;
    let bytes = result
        .output_bytes()
        .map(<[u8]>::to_vec)
        .unwrap_or_else(|| data.to_vec());
    Ok(TranscodeOutput {
        data: bytes,
        format: ImageFormat::Jpeg,
        mime_type: ImageFormat::Jpeg.mime_type(),
    })
}

/// JPEG→JXL: hand zenjxl's coefficient-domain recompressor a **zensim Profile-A**
/// scorer and let it hit `quality`, with the lossless transcode as the floor.
/// Uses the existing [`zenjxl::jpeg_lossy`] system — no decode→re-encode here.
/// JPEG is opaque, so RGB8 scoring is exact.
///
/// The [`QualityTarget`] maps onto zenjxl's native target:
/// [`Absolute`](QualityTarget::Absolute) → `Inferred` (predict the source floor vs
/// the original; if the source quality can't be read, fall back to a relative
/// target at the same level); [`Relative`](QualityTarget::Relative) → `Relative`
/// (vs the source's own decoded pixels).
#[cfg(all(feature = "transcode-iqa", feature = "jxl-decode"))]
fn recompress_jpeg_to_jxl(data: &[u8], quality: QualityTarget) -> Result<TranscodeOutput> {
    use zenjxl::jpeg_lossy::{
        InferredMetric, JpegRecompressMethod, QualityTarget as JxlTarget,
        recompress_jpeg_lossy_target,
    };
    use zensim::{Zensim, ZensimProfile};

    // RelativeScorer = Fn(ref_rgb8, dist_rgb8, w, h) -> f32 over the packed RGB8
    // buffers zenjxl decodes internally. Higher zensim-A = better.
    let metric = Zensim::new(ZensimProfile::A);
    let scorer = move |r: &[u8], d: &[u8], w: u32, h: u32| -> f32 {
        let (pw, ph) = (w as usize, h as usize);
        let rs = zensim::RgbSlice::new(bytemuck::cast_slice(r), pw, ph);
        let ds = zensim::RgbSlice::new(bytemuck::cast_slice(d), pw, ph);
        metric
            .compute(&rs, &ds)
            .map(|x| x.score() as f32)
            .unwrap_or(0.0)
    };

    let jxl_target = match quality {
        // Absolute: aim vs the original via the source's inferred floor. If the
        // source quality can't be read (adaptive-quant / jpegli sources), fall
        // back to the same level taken relative to the source's own pixels.
        QualityTarget::Absolute(t) => {
            JxlTarget::inferred_preliminary(data, InferredMetric::ZensimA, t).unwrap_or(
                JxlTarget::Relative {
                    level: t,
                    higher_is_better: true,
                },
            )
        }
        QualityTarget::Relative(t) => JxlTarget::Relative {
            level: t,
            higher_is_better: true,
        },
    };
    let result =
        recompress_jpeg_lossy_target(data, JpegRecompressMethod::Auto, jxl_target, &scorer, 7);
    // Auto = min(coarsen, re-encode); never larger/worse than the lossless floor.
    let bytes = result.map_err(|e| {
        at!(CodecError::InvalidInput(alloc::format!(
            "jpeg→jxl recompress: {e}"
        )))
    })?;
    Ok(TranscodeOutput {
        data: bytes,
        format: ImageFormat::Jxl,
        mime_type: ImageFormat::Jxl.mime_type(),
    })
}

// ═══════════════════════════════════════════════════════════════════════
// Lossless byte-exact JPEG↔JXL transcode (JBRD / brunsli-parity)
// ═══════════════════════════════════════════════════════════════════════

/// Losslessly transcode a baseline/extended JPEG to JPEG XL so the **exact
/// original JPEG bitstream** can later be recovered with
/// [`reconstruct_jpeg_from_jxl`] (the brunsli-equivalent JBRD path). The JXL is
/// typically ~20% smaller than the source JPEG and carries the reconstruction
/// data; no pixels are re-encoded and no quality is lost.
///
/// `effort` (1–10) trades encode time for output size; it does **not** affect the
/// reconstructed bytes — the round-trip is byte-exact at every effort.
///
/// Returns [`CodecError::InvalidInput`] for JPEGs outside the JBRD boundary (CMYK,
/// arithmetic coding, 12-bit, chroma sampling factors > 2). For those, fall back
/// to a lossy route ([`transcode_to_quality`]) or keep the source JPEG.
///
/// Requires the `jpeg-jxl-transcode` feature.
#[cfg(feature = "jpeg-jxl-transcode")]
pub fn transcode_jpeg_to_jxl_lossless(jpeg: &[u8], effort: u8) -> Result<TranscodeOutput> {
    use zenjxl::LosslessConfig;
    let data = LosslessConfig::new()
        .with_effort(effort)
        .encode_jpeg_transcode(jpeg)
        .map_err(|e| {
            at!(CodecError::InvalidInput(alloc::format!(
                "jpeg→jxl lossless transcode: {e}"
            )))
        })?;
    Ok(TranscodeOutput {
        data,
        format: ImageFormat::Jxl,
        mime_type: ImageFormat::Jxl.mime_type(),
    })
}

/// Reconstruct the **byte-exact original JPEG** from a JPEG XL produced by a
/// lossless JPEG transcode (a JBRD reconstruction box is present) — the inverse
/// of [`transcode_jpeg_to_jxl_lossless`]. Pure Rust; no external `djxl`.
///
/// - `Ok(Some(jpeg))` — the JXL carried reconstruction data; the bytes are the
///   exact original JPEG.
/// - `Ok(None)` — a valid JXL that is **not** a JPEG transcode (no JBRD box), so
///   there is no original JPEG to recover. Decode it as pixels instead (e.g. via
///   [`DecodeRequest`](crate::DecodeRequest)).
///
/// Requires the `jxl-jpeg-reconstruct` feature (decode-side only — independent of
/// the JPEG/JXL encoders).
#[cfg(feature = "jxl-jpeg-reconstruct")]
pub fn reconstruct_jpeg_from_jxl(jxl: &[u8]) -> Result<Option<Vec<u8>>> {
    zenjxl_decoder::reconstruct_jpeg(jxl).map_err(|e| {
        at!(CodecError::InvalidInput(alloc::format!(
            "jxl→jpeg reconstruct: {e}"
        )))
    })
}

// ═══════════════════════════════════════════════════════════════════════
// TranscodeSink — streaming decode→encode bridge
// ═══════════════════════════════════════════════════════════════════════

/// Streaming transcode sink: forwards decoded strips to an encoder.
///
/// Created via [`TranscodeSink::new`] with a [`StreamingEncoder`] from
/// [`EncodeRequest::build_streaming_encoder`].
///
/// [`StreamingEncoder`]: crate::dispatch::StreamingEncoder
/// [`EncodeRequest::build_streaming_encoder`]: crate::EncodeRequest::build_streaming_encoder
///
/// # Example
///
/// ```rust,ignore
/// // Build the encoder
/// let se = EncodeRequest::new(ImageFormat::Jpeg)
///     .with_quality(85.0)
///     .build_streaming_encoder(width, height)?;
///
/// // Create sink and decode through it
/// let mut sink = TranscodeSink::new(se.encoder, se.supported);
/// DecodeRequest::new(data).push_decode(&mut sink)?;
///
/// // Finalize
/// let output = sink.finish_encode()?;
/// ```
pub struct TranscodeSink<'a> {
    encoder: Option<Box<dyn DynEncoder + 'a>>,
    supported: &'static [PixelDescriptor],
    /// Scratch buffer for receiving decoded rows from the decoder.
    /// The decoder writes into this via `provide_next_buffer`, and
    /// we forward it to the encoder on the *next* call (or on finish).
    strip_buf: Vec<u8>,
    /// Metadata for the pending (written but not yet forwarded) strip.
    pending: Option<PendingStrip>,
}

/// Metadata for a strip that the decoder has written but we haven't
/// forwarded to the encoder yet.
struct PendingStrip {
    width: u32,
    height: u32,
    descriptor: PixelDescriptor,
}

impl<'a> TranscodeSink<'a> {
    /// Create a new streaming transcode sink.
    ///
    /// `encoder` — the `DynEncoder` to push strips into.
    /// `supported` — the encoder's supported pixel descriptors
    ///   (from `EncoderConfig::supported_descriptors()`).
    pub fn new(encoder: Box<dyn DynEncoder + 'a>, supported: &'static [PixelDescriptor]) -> Self {
        Self {
            encoder: Some(encoder),
            supported,
            strip_buf: Vec::new(),
            pending: None,
        }
    }

    /// Finalize encoding and return the output.
    ///
    /// Must be called after `push_decode` completes (which calls
    /// `DecodeRowSink::finish` internally). Consumes the encoder
    /// via `DynEncoder::finish()`.
    pub fn finish_encode(
        mut self,
    ) -> core::result::Result<EncodeOutput, Box<dyn core::error::Error + Send + Sync>> {
        let encoder =
            self.encoder
                .take()
                .ok_or_else(|| -> Box<dyn core::error::Error + Send + Sync> {
                    "encoder already finished".into()
                })?;
        encoder.finish()
    }

    /// Forward the pending strip (if any) to the encoder.
    fn flush_pending(&mut self) -> core::result::Result<(), SinkError> {
        let pending = match self.pending.take() {
            Some(p) => p,
            None => return Ok(()),
        };

        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| -> SinkError { "encoder already finished".into() })?;

        let bpp = pending.descriptor.bytes_per_pixel();
        let stride = pending.width as usize * bpp;
        let data_len = stride * pending.height as usize;
        let strip_data = &self.strip_buf[..data_len];

        // Adapt pixel format per-strip — zero-copy when format already matches
        let adapted = zenpixels_convert::adapt::adapt_for_encode(
            strip_data,
            pending.descriptor,
            pending.width,
            pending.height,
            stride,
            self.supported,
        )
        .map_err(|e| -> SinkError { alloc::format!("adapt: {e}").into() })?;

        let adapted_stride = adapted.width as usize * adapted.descriptor.bytes_per_pixel();
        let pixel_slice = zenpixels::PixelSlice::new(
            &adapted.data,
            adapted.width,
            adapted.rows,
            adapted_stride,
            adapted.descriptor,
        )
        .map_err(|e| -> SinkError { alloc::format!("pixel slice: {e}").into() })?;

        encoder
            .push_rows(pixel_slice)
            .map_err(|e| -> SinkError { alloc::format!("push_rows: {e}").into() })
    }
}

impl DecodeRowSink for TranscodeSink<'_> {
    fn begin(
        &mut self,
        _width: u32,
        _height: u32,
        _descriptor: PixelDescriptor,
    ) -> core::result::Result<(), SinkError> {
        self.pending = None;
        self.strip_buf.clear();
        Ok(())
    }

    fn provide_next_buffer(
        &mut self,
        _y: u32,
        height: u32,
        width: u32,
        descriptor: PixelDescriptor,
    ) -> core::result::Result<PixelSliceMut<'_>, SinkError> {
        // The previous buffer (if any) has been fully written by the decoder.
        // Forward it to the encoder before providing the next buffer.
        self.flush_pending()?;

        let bpp = descriptor.bytes_per_pixel();
        let stride = width as usize * bpp;
        let needed = stride * height as usize;

        // Resize strip_buf for this strip
        self.strip_buf.resize(needed, 0);
        self.pending = Some(PendingStrip {
            width,
            height,
            descriptor,
        });

        PixelSliceMut::new(
            &mut self.strip_buf[..needed],
            width,
            height,
            stride,
            descriptor,
        )
        .map_err(|e| -> SinkError { alloc::format!("pixel slice: {e}").into() })
    }

    fn finish(&mut self) -> core::result::Result<(), SinkError> {
        // Forward the last strip
        self.flush_pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcode_sink_construction() {
        // Verify the type compiles and basic construction works.
        // Full integration requires a real encoder, tested in integration tests.
        assert!(core::mem::size_of::<TranscodeSink<'_>>() > 0);
    }

    #[test]
    fn supplement_set_operations() {
        let set = SupplementSet::GAIN_MAP | SupplementSet::DEPTH_MAP;
        assert!(set.contains(SupplementSet::GAIN_MAP));
        assert!(set.contains(SupplementSet::DEPTH_MAP));
        assert!(!set.contains(SupplementSet::THUMBNAIL));

        let empty = SupplementSet(0);
        assert!(empty.is_empty());
        assert!(!set.is_empty());

        assert_eq!(
            SupplementSet::GAIN_MAP.union(SupplementSet::THUMBNAIL),
            SupplementSet::GAIN_MAP | SupplementSet::THUMBNAIL
        );
    }

    #[test]
    fn transcode_options_default() {
        let opts = TranscodeOptions::default();
        assert!(opts.metadata.is_none());
        assert!(opts.matte.is_none());
        assert!(matches!(opts.supplements, SupplementPolicy::Preserve));
        // Verbatim by default — no implicit privacy choice.
        assert!(matches!(
            opts.metadata_policy,
            MetadataPolicy::PreserveExact
        ));
    }

    #[cfg(feature = "jxl-jpeg-reconstruct")]
    #[test]
    fn reconstruct_rejects_non_transcode_input() {
        // Garbage / non-JXL input must not panic: it returns an error or Ok(None),
        // never bogus JPEG bytes.
        let r = reconstruct_jpeg_from_jxl(&[0xFF, 0x0A, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05]);
        assert!(matches!(r, Err(_) | Ok(None)));
    }

    #[cfg(feature = "jpeg-jxl-transcode")]
    #[test]
    fn jpeg_jxl_lossless_roundtrips_or_rejects() {
        // Brunsli-equivalent contract: a lossless JPEG→JXL transcode either cleanly
        // rejects (outside the JBRD boundary) or reconstructs the ORIGINAL JPEG
        // byte-for-byte. Never silent corruption.
        let jpeg: &[u8] = include_bytes!("../tests/images/ultrahdr_sample.jpg");
        match transcode_jpeg_to_jxl_lossless(jpeg, 7) {
            Ok(out) => {
                assert_eq!(out.format, ImageFormat::Jxl);
                let recon = reconstruct_jpeg_from_jxl(&out.data)
                    .expect("reconstruct must not error on our own transcode")
                    .expect("our lossless transcode embeds a JBRD reconstruction box");
                assert_eq!(
                    recon, jpeg,
                    "lossless JPEG→JXL→JPEG must reconstruct byte-for-byte"
                );
            }
            // Outside the JBRD boundary (multi-image / unsupported sampling, etc.):
            // a clean rejection is contract-correct — never a corrupt result.
            Err(_) => {}
        }
    }

    /// Round-trip: encode a tiny JPEG, transcode to WebP, verify output.
    #[cfg(all(feature = "jpeg", feature = "webp"))]
    #[test]
    fn transcode_jpeg_to_webp() {
        use crate::quality::QualityIntent;

        // Create a small test image
        let img = imgref::ImgVec::new(
            alloc::vec![
                rgb::Rgb {
                    r: 128u8,
                    g: 64,
                    b: 32
                };
                10 * 10
            ],
            10,
            10,
        );

        // Encode to JPEG first
        let jpeg_output = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(75.0)
            .encode(zenpixels::PixelSlice::from(img.as_ref()).erase(), false)
            .unwrap();
        assert!(!jpeg_output.data().is_empty());

        // Now transcode JPEG → WebP
        let decision = FormatDecision {
            format: ImageFormat::WebP,
            quality: QualityIntent::from_quality(80.0),
            lossless: false,
            hints: Default::default(),
            matte: None,
            trace: alloc::vec::Vec::new(),
        };

        let output = transcode(
            jpeg_output.data(),
            &decision,
            &TranscodeOptions::default(),
            &AllowedFormats::all(),
        )
        .unwrap();

        assert_eq!(output.format, ImageFormat::WebP);
        assert_eq!(output.mime_type, "image/webp");
        assert!(!output.data.is_empty());

        // Verify we can decode the transcoded output
        let decoded = crate::DecodeRequest::new(&output.data)
            .decode_full_frame()
            .unwrap();
        assert_eq!(decoded.width(), 10);
        assert_eq!(decoded.height(), 10);
    }

    /// JPEG→JXL via zenjxl's `jpeg_lossy` coefficient-domain recompressor with a
    /// zensim-A target. Exercises the existing system end-to-end and verifies a
    /// decodable JXL at the source dimensions — no zencodecs-side re-encode loop.
    #[cfg(all(feature = "transcode-iqa", feature = "jpeg", feature = "jxl-decode"))]
    #[test]
    fn transcode_to_quality_jpeg_to_jxl() {
        // Gradient + XOR high-frequency content so the source JPEG has real
        // structure for the recompressor to act on.
        let (w, h) = (64usize, 64usize);
        let mut px = alloc::vec::Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                px.push(rgb::Rgb {
                    r: (x * 4) as u8,
                    g: (y * 4) as u8,
                    b: ((x ^ y) * 4) as u8,
                });
            }
        }
        let img = imgref::ImgVec::new(px, w, h);
        let jpeg = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(90.0)
            .encode(zenpixels::PixelSlice::from(img.as_ref()).erase(), false)
            .unwrap();

        let out = transcode_to_quality(
            jpeg.data(),
            ImageFormat::Jxl,
            QualityTarget::Absolute(85.0),
            &TranscodeOptions::default(),
            &AllowedFormats::all(),
        )
        .unwrap();
        assert_eq!(out.format, ImageFormat::Jxl);
        assert!(!out.data.is_empty());

        let decoded = crate::DecodeRequest::new(&out.data)
            .decode_full_frame()
            .unwrap();
        assert_eq!(decoded.width() as usize, w);
        assert_eq!(decoded.height() as usize, h);
    }

    /// Round-trip: encode a tiny image, transcode keeping the same format.
    #[cfg(feature = "jpeg")]
    #[test]
    fn transcode_jpeg_to_jpeg() {
        use crate::quality::QualityIntent;

        let img = imgref::ImgVec::new(
            alloc::vec![
                rgb::Rgb {
                    r: 200u8,
                    g: 100,
                    b: 50
                };
                8 * 8
            ],
            8,
            8,
        );

        let jpeg_output = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(90.0)
            .encode(zenpixels::PixelSlice::from(img.as_ref()).erase(), false)
            .unwrap();

        let decision = FormatDecision {
            format: ImageFormat::Jpeg,
            quality: QualityIntent::from_quality(70.0),
            lossless: false,
            hints: Default::default(),
            matte: None,
            trace: alloc::vec::Vec::new(),
        };

        let output = transcode(
            jpeg_output.data(),
            &decision,
            &TranscodeOptions::default(),
            &AllowedFormats::all(),
        )
        .unwrap();

        assert_eq!(output.format, ImageFormat::Jpeg);
        assert_eq!(output.mime_type, "image/jpeg");
        assert!(!output.data.is_empty());

        // Lower quality should produce fewer bytes
        assert!(
            output.data.len() <= jpeg_output.data().len(),
            "q70 ({}) should be <= q90 ({})",
            output.data.len(),
            jpeg_output.data().len(),
        );
    }

    /// Verify probe() returns correct info for a JPEG.
    #[cfg(feature = "jpeg")]
    #[test]
    fn probe_jpeg_returns_correct_info() {
        let img = imgref::ImgVec::new(
            alloc::vec![
                rgb::Rgb {
                    r: 100u8,
                    g: 150,
                    b: 200
                };
                16 * 12
            ],
            16,
            12,
        );
        let jpeg_output = crate::EncodeRequest::new(ImageFormat::Jpeg)
            .with_quality(80.0)
            .encode(zenpixels::PixelSlice::from(img.as_ref()).erase(), false)
            .unwrap();

        let info = crate::probe(jpeg_output.data(), &AllowedFormats::all()).unwrap();
        assert_eq!(info.width, 16);
        assert_eq!(info.height, 12);
        assert_eq!(info.format, ImageFormat::Jpeg);
        assert!(!info.has_alpha);
    }

    /// Verify probe() returns correct info for a PNG.
    #[cfg(feature = "png")]
    #[test]
    fn probe_png_returns_correct_info() {
        // Create a minimal PNG via the png crate
        let mut buf = alloc::vec::Vec::new();
        let mut encoder = png::Encoder::new(&mut buf, 20, 15);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&alloc::vec![128u8; 20 * 15 * 4])
            .unwrap();
        writer.finish().unwrap();

        let info = crate::probe(&buf, &AllowedFormats::all()).unwrap();
        assert_eq!(info.width, 20);
        assert_eq!(info.height, 15);
        assert_eq!(info.format, ImageFormat::Png);
        assert!(info.has_alpha);
    }
}
