//! Checked pixel transcoding at a mandatory interpretation suspension.
//!
//! The registered decoder must implement controlled decoding. Preparation binds
//! an actual color transform, descriptive metadata, checked encoder jobs and
//! admitted strip storage before one `Question::Transcode`. Output is returned
//! only after completion and metadata/color postconditions pass. No output
//! writer is awaited while a native encoder retains working state.
//!
//! Managed storage is bounded; native codec/CMS workspace and encoded output
//! allocations are currently unqualified. Offers report that scope as unknown.
//! This route handles one primary image; sequence and auxiliary content refuse.
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::fmt;
use zencodec::decode::{BoundaryAction, Event, InterpretationState};
use zencodec::encode::EncodeOutput;
use zencodec::operation::{
    self, MetadataTransfer, OperationReport, Outcome, PolicyResolver, PolicySet, TranscodeOffer,
};
use zencodec::storage::StorageBudget;
use zencodec::{At, CodecSet, ImageFormat, ImageInfo, Metadata};
use zencodec_io::{
    ControlledEvent, ControlledStepper, DriveOptions, ReadAtCursor, SliceCursor, Yield,
};
use zenpixels::{ColorOrigin, ConvertOptions, PixelDescriptor, PixelFormat};
use zenpixels_convert::{OutputPlan, OutputProfile, cms::PluggableCms};

#[cfg(feature = "checked-transcode-async")]
mod asynchronous;
mod execution;
mod sink;
use sink::{Destination, PreparedRows};

type Cause = Box<dyn core::error::Error + Send + Sync>;
struct ExpectedOutput {
    metadata: Metadata,
    width: u32,
    height: u32,
    descriptor: PixelDescriptor,
    resolution: Option<zencodec::Resolution>,
}
type Held<'a> = ControlledStepper<'a, Box<zencodec::decode::DynControlledDecoder>, Destination>;

/// Stage at which a checked transcode failed. Formatting excludes metadata,
/// input paths, profile contents and application callback error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stage {
    /// Invalid request or a decoder could not be prepared.
    Prepare,
    /// Source parsing or pixel decoding failed.
    Decode,
    /// Conversion, metadata or an encoder candidate could not be prepared.
    Plan,
    /// The application declined, failed, or selected an invalid choice.
    Policy,
    /// Selected conversion or encoding failed. There is no retry.
    Encode,
    /// The completed output did not meet the selected contract.
    Verify,
}

/// A request, protocol or output obligation the adapter could not establish.
/// Native codec/CMS errors retain their own types instead of becoming a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// A zero-sized strip cannot receive decoded rows.
    InvalidStripRows,
    /// Final interpretation had no associated image information.
    MissingImageInfo,
    /// Final interpretation had no output descriptor.
    MissingDescriptor,
    /// A still-image operation tried to replace an adopted interpretation.
    RepeatedInterpretation,
    /// The controlled decoder returned an unexpected terminal/progress event.
    UnexpectedProgress,
    /// A decoder finished without selecting a complete route.
    MissingRoute,
    /// A decoder did not call its destination's finish method.
    MissingEncoderFinish,
    /// Multiple images require an explicit sequence operation.
    UnsupportedSequence,
    /// Auxiliary content requires a preservation/removal route.
    AuxiliaryContent,
    /// The source has not established absence of a gain map.
    IncompleteGainMapDiscovery,
    /// HDR conversion/signaling requires an explicit rendering route.
    HdrRendering,
    /// This adapter does not implement the requested metadata rule.
    UnknownMetadataRule,
    /// The output changed required dimensions or orientation.
    GeometryOrOrientation,
    /// The destination cannot retain the selected semantic alpha channel.
    AlphaRetention,
    /// Encoded precision did not match the selected integer sample depth.
    OutputPrecision,
    /// Reported physical resolution could not be retained.
    PhysicalResolution,
    /// This route cannot verify floating-point container sample interpretation.
    UnqualifiedSampleType,
    /// The output failed exact retention/exclusion of descriptive metadata.
    DescriptiveMetadata,
    /// Output signaling does not establish the selected sample interpretation.
    OutputColor,
    /// Unexpected HDR metadata appeared in the destination.
    OutputHdr,
    /// A required absolute-luminance anchor was lost or changed.
    RenderingAnchor,
    /// Encoded output is too large to publish under the request.
    OutputLimit { actual: usize, limit: usize },
    /// The decoder requested rows outside the admitted geometry/order.
    StripGeometry,
    /// Row-storage size arithmetic overflowed.
    SizeOverflow,
    /// The decoder performed sink work before route selection.
    DecoderBeforeSelection,
    /// A destination was installed or begun more than once.
    RepeatedDestination,
    /// The decoder omitted required final rows.
    MissingRows,
}
impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transcode obligation: {self:?}")
    }
}
impl core::error::Error for Refusal {}

/// A traced native failure plus the bounded decisions that preceded it.
pub struct Error {
    stage: Stage,
    report: Box<OperationReport>,
    cause: Cause,
}
impl Error {
    /// Failing stage without payloads.
    pub fn stage(&self) -> Stage {
        self.stage
    }
    /// Decisions retained even when execution or postcondition checking failed.
    pub fn report(&self) -> &OperationReport {
        &self.report
    }
}
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TranscodeError")
            .field("stage", &self.stage)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "checked transcode failed during {:?}", self.stage)
    }
}
impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

/// Reusable requirements for one primary-image pixel transcode.
///
/// Defaults preserve decoder-reported EXIF/XMP/orientation and current color.
/// Unknown container carriers are not copied. No implicit depth reduction,
/// alpha removal, grayscale conversion, HDR rendering or auxiliary loss.
/// `with_output` makes permitted conversion explicit. The default managed
/// allowance is 64 MiB / 128 allocations, and does not bound native workspace.
#[derive(Clone)]
pub struct TranscodeRequest<'a> {
    decode: operation::DecodeRequest,
    encode: operation::EncodeRequest,
    format: ImageFormat,
    output: Option<(OutputProfile, PixelFormat, ConvertOptions)>,
    metadata: MetadataTransfer,
    storage: StorageBudget,
    cms: Option<&'a dyn PluggableCms>,
    preset: PolicySet,
    strip_rows: u32,
    output_limit: usize,
    io: DriveOptions,
}
impl<'a> TranscodeRequest<'a> {
    /// Preserve current color and reported metadata in this destination format.
    pub fn new(format: ImageFormat) -> Self {
        Self {
            decode: operation::DecodeRequest::new(),
            encode: operation::EncodeRequest::new(format).with_policy_defaults(
                zencodec::encode::EncodePolicy::none()
                    .with_color(zencodec::ColorEmitPolicy::Verbatim),
            ),
            format,
            output: None,
            metadata: MetadataTransfer::PreserveReported,
            storage: StorageBudget::new(64 * 1024 * 1024, 128),
            cms: None,
            preset: PolicySet::new(),
            strip_rows: 64,
            output_limit: 64 * 1024 * 1024,
            io: DriveOptions::default(),
        }
    }
    /// Required source policy/output; uses the same registered controlled path.
    pub fn with_decode(mut self, request: operation::DecodeRequest) -> Self {
        self.decode = request;
        self
    }
    /// Require this fidelity configuration from the destination codec.
    pub fn with_fidelity(mut self, target: zencodec::encode::Fidelity) -> Self {
        self.encode = self.encode.with_fidelity(target);
        self
    }
    /// Permit only these fidelity alternatives in the joint transcode question.
    pub fn preferring_fidelity(mut self, targets: Vec<zencodec::encode::Fidelity>) -> Self {
        self.encode = self.encode.preferring_fidelity(targets);
        self
    }
    /// Required encoder policy; the legacy best-effort setter is never used.
    pub fn with_encode_policy(mut self, policy: zencodec::encode::EncodePolicy) -> Self {
        self.encode = self.encode.with_policy(policy);
        self
    }
    /// Explicit destination samples, color profile and permitted sample changes.
    pub fn with_output(
        mut self,
        profile: OutputProfile,
        format: PixelFormat,
        options: ConvertOptions,
    ) -> Self {
        self.output = Some((profile, format, options));
        self
    }
    /// Explicit descriptive metadata treatment; color and orientation stay required.
    pub fn with_metadata(mut self, policy: MetadataTransfer) -> Self {
        self.metadata = policy;
        self
    }
    /// Inject the color service used during preparation and conversion.
    pub fn with_cms(mut self, cms: &'a dyn PluggableCms) -> Self {
        self.cms = Some(cms);
        self
    }
    /// Shared managed-storage allowance, including held source and row buffers.
    pub fn with_storage(mut self, storage: StorageBudget) -> Self {
        self.storage = storage;
        self
    }
    /// Configure the shared built-in resolver used directly or by delegation.
    pub fn with_policy_set(mut self, policy: PolicySet) -> Self {
        self.preset = policy;
        self
    }
    /// Maximum batch a decoder may hand to the admitted strip storage. Zero refuses.
    pub fn with_strip_rows(mut self, rows: u32) -> Self {
        self.strip_rows = rows;
        self
    }
    /// Refuse to publish a larger encoded output. This is a publication limit,
    /// not a cap on the codec's internal encoded allocation.
    pub fn with_output_limit(mut self, bytes: usize) -> Self {
        self.output_limit = bytes;
        self
    }

    /// Input driver sizes and scoped I/O admission for file/range sources.
    /// This does not turn native codec workspace into admitted storage.
    pub fn with_io_options(mut self, options: DriveOptions) -> Self {
        self.io = options;
        self
    }

    /// Execute with the configured built-in policy and return verified output.
    pub fn transcode(
        &self,
        codecs: &CodecSet,
        input: &[u8],
    ) -> Result<Outcome<EncodeOutput>, At<Error>> {
        let mut resolver = self.preset;
        self.transcode_with(codecs, input, &mut resolver)
    }

    /// Execute with one fallible resolver for discovery and the complete route.
    /// Its own traced error remains inspectable through `Error::source`.
    #[track_caller]
    pub fn transcode_with<R: PolicyResolver + ?Sized>(
        &self,
        codecs: &CodecSet,
        input: &[u8],
        resolver: &mut R,
    ) -> Result<Outcome<EncodeOutput>, At<Error>> {
        let mut cursor = SliceCursor::new(self.io.read_size);
        self.execute(codecs, resolver, |held, stop| {
            Ok(cursor.next(input, held, stop)?)
        })
    }

    /// Execute over the existing random-access contract, including disk files
    /// and batched HTTP/object-store adapters. No seeking abstraction is added.
    pub fn transcode_from_read_at<S: zencodec::source::ReadAt + ?Sized>(
        &self,
        codecs: &CodecSet,
        source: &mut S,
    ) -> Result<Outcome<EncodeOutput>, At<Error>> {
        let mut resolver = self.preset;
        self.transcode_from_read_at_with(codecs, source, &mut resolver)
    }

    /// The same joint resolver and output postconditions over a file/range
    /// source. Source errors retain their native type in the error chain.
    pub fn transcode_from_read_at_with<S, R>(
        &self,
        codecs: &CodecSet,
        source: &mut S,
        resolver: &mut R,
    ) -> Result<Outcome<EncodeOutput>, At<Error>>
    where
        S: zencodec::source::ReadAt + ?Sized,
        R: PolicyResolver + ?Sized,
    {
        let mut cursor = ReadAtCursor::new(self.io);
        self.execute(codecs, resolver, |held, stop| {
            Ok(cursor.next(source, held, stop)?)
        })
    }

    fn execute<R, F>(
        &self,
        codecs: &CodecSet,
        resolver: &mut R,
        mut next: F,
    ) -> Result<Outcome<EncodeOutput>, At<Error>>
    where
        R: PolicyResolver + ?Sized,
        F: FnMut(&mut Held<'_>, &dyn enough::Stop) -> Result<Yield<ControlledEvent>, Cause>,
    {
        let mut execution = execution::Execution::<operation::PreparedEncode>::new(self, codecs);
        let result = (|| {
            let decoder = execution.prepare()?;
            let mut destination = Destination::default();
            let mut held = ControlledStepper::new(decoder, &mut destination);
            loop {
                execution.before_step();
                let event = next(&mut held, codecs.stop())
                    .inspect_err(|error| execution.step_error(error))?;
                if execution.advance(&mut held, event, resolver)? {
                    break;
                }
            }
            drop(held);
            execution.finish(destination)
        })();
        execution.outcome(result)
    }
}

fn require_primary(info: &ImageInfo) -> Result<(), Cause> {
    if !matches!(info.sequence, zencodec::ImageSequence::Single) {
        return Err(Refusal::UnsupportedSequence.into());
    }
    let s = &info.supplements;
    if s.pyramid || s.gain_map || s.depth_map || s.segmentation_mattes || s.auxiliary {
        return Err(Refusal::AuxiliaryContent.into());
    }
    if !matches!(info.gain_map, zencodec::GainMapPresence::Absent) {
        return Err(Refusal::IncompleteGainMapDiscovery.into());
    }
    Ok(())
}
fn origin(info: &ImageInfo) -> ColorOrigin {
    let c = &info.source_color;
    match (&c.icc_profile, c.cicp) {
        (Some(icc), Some(cicp)) => ColorOrigin::from_icc_and_cicp(icc.clone(), cicp),
        (Some(icc), None) => ColorOrigin::from_icc(icc.clone()),
        (None, Some(cicp)) => ColorOrigin::from_cicp(cicp),
        (None, None) => ColorOrigin::assumed(),
    }
    .with_color_authority(c.color_authority)
}
fn output_metadata(
    info: &ImageInfo,
    plan: &OutputPlan,
    policy: MetadataTransfer,
    format: ImageFormat,
) -> Result<Metadata, Cause> {
    if info.source_color.content_light_level.is_some()
        || info.source_color.mastering_display.is_some()
        || matches!(
            plan.output_descriptor().transfer(),
            zenpixels::TransferFunction::Pq | zenpixels::TransferFunction::Hlg
        )
    {
        return Err(Refusal::HdrRendering.into());
    }
    let mut metadata = match policy {
        MetadataTransfer::PreserveReported => info.metadata(),
        MetadataTransfer::DiscardDescriptive => Metadata::none().with_orientation(info.orientation),
        _ => return Err(Refusal::UnknownMetadataRule.into()),
    };
    metadata.icc_profile = plan.metadata().icc.clone();
    metadata.cicp = plan.metadata().cicp;
    metadata.content_light_level = None;
    metadata.mastering_display = None;
    let reconciled = zenmeta::metadata::filter(&metadata, &zencodec::MetadataPolicy::PreserveExact);
    if metadata.exif.is_some() && reconciled.exif.is_none() {
        return Err(Refusal::DescriptiveMetadata.into());
    }
    Ok(crate::dispatch::fold_orientation_into_exif(
        reconciled, format,
    ))
}
fn requires_alpha(descriptor: PixelDescriptor) -> bool {
    matches!(
        descriptor.alpha(),
        Some(zenpixels::AlphaMode::Straight | zenpixels::AlphaMode::Premultiplied)
    )
}
fn verify_output(
    actual: &ImageInfo,
    decoded: &zencodec::decode::OutputInfo,
    expected: &ExpectedOutput,
) -> Result<(), Cause> {
    let (width, height, descriptor) = (expected.width, expected.height, expected.descriptor);
    if expected.resolution.is_some() && actual.resolution != expected.resolution {
        return Err(Refusal::PhysicalResolution.into());
    }
    let expected = &expected.metadata;
    if (actual.width, actual.height) != (width, height)
        || (decoded.width, decoded.height) != (width, height)
        || actual.orientation != expected.orientation
    {
        return Err(Refusal::GeometryOrOrientation.into());
    }
    if requires_alpha(descriptor) && !actual.has_alpha {
        return Err(Refusal::AlphaRetention.into());
    }
    // Packed indices are not color-channel precision. For example, a one-bit
    // PNG palette can preserve arbitrary eight-bit RGB values exactly. Inspect
    // the decoder's native sample representation, without format preferences
    // that could hide a precision loss by requesting an upconversion.
    if decoded.native_format.channel_type() != descriptor.channel_type() {
        return Err(Refusal::OutputPrecision.into());
    }
    let meta = actual.metadata();
    if meta.exif != expected.exif || meta.xmp != expected.xmp {
        return Err(Refusal::DescriptiveMetadata.into());
    }
    let authority = actual.source_color.to_color_context();
    if let Some(icc) = &expected.icc_profile {
        if authority.icc.as_ref() != Some(icc) {
            return Err(Refusal::OutputColor.into());
        }
    } else {
        let cicp = expected.cicp.unwrap_or(zencodec::Cicp::SRGB);
        let implicit_srgb = cicp == zencodec::Cicp::SRGB
            && matches!(actual.format, ImageFormat::Jpeg | ImageFormat::Png)
            && meta.icc_profile.is_none()
            && meta.cicp.is_none();
        if !(authority.cicp == Some(cicp) && meta.icc_profile.is_none()) && !implicit_srgb {
            use zenpixels_convert::icc_profiles::{SynthesizedIcc, synthesize_icc_for_cicp};
            let equivalent = match synthesize_icc_for_cicp(cicp) {
                SynthesizedIcc::Profile(bytes) => {
                    meta.icc_profile.as_deref() == Some(bytes.as_ref())
                        && (authority.icc.is_some() || authority.cicp == Some(cicp))
                }
                _ => false,
            };
            if !equivalent {
                return Err(Refusal::OutputColor.into());
            }
        }
    }
    if meta.content_light_level.is_some() || meta.mastering_display.is_some() {
        return Err(Refusal::OutputHdr.into());
    }
    if expected.diffuse_white.is_some() && meta.diffuse_white != expected.diffuse_white {
        return Err(Refusal::RenderingAnchor.into());
    }
    Ok(())
}
