//! A registry's erased encoder job as a typed job for the planned row driver.
//!
//! `row_encode::encode_job` takes a typed [`EncodeJob`]. Registries and
//! checked requests hand out `Box<dyn DynEncodeJob>`, which does not implement
//! that trait. These wrappers forward every call, including `plan_encode` and
//! `begin_declared`, and keep native causes reachable through
//! [`ErasedError::source`](core::error::Error::source).

use alloc::boxed::Box;
use core::any::Any;

use zencodec::decode::ErasedError;
use zencodec::encode::{
    BoxedError, DynEncodeJob, DynEncoder, EncodeDeclaration, EncodeJob, EncodeOutput, EncodePlan,
    EncodePolicy, Encoder, OutputTiming,
};
use zencodec::output_patch::{OutputAccess, OutputFinish};
use zencodec::{
    Metadata, MetadataPolicy, Resolution, ResolutionError, ResourceLimits, SharedPulse,
    UnsupportedOperation,
};
use zenpixels::{PixelDescriptor, PixelSlice, PixelSliceMut};

fn erased(error: BoxedError) -> ErasedError {
    ErasedError::new(error)
}

/// An erased encoder job usable wherever a typed [`EncodeJob`] is required.
pub struct ErasedJob<'a>(Box<dyn DynEncodeJob + 'a>);

impl<'a> ErasedJob<'a> {
    /// Wrap a job from a registry or a prepared checked request.
    pub fn new(job: Box<dyn DynEncodeJob + 'a>) -> Self {
        Self(job)
    }

    /// The erased job.
    pub fn into_inner(self) -> Box<dyn DynEncodeJob + 'a> {
        self.0
    }
}

/// No animation encoder exists behind an [`ErasedJob`]; use
/// `DynEncodeJob::into_animation_frame_encoder` directly for sequences.
#[derive(Debug)]
pub enum NoAnimation {}

impl EncodeJob for ErasedJob<'_> {
    type Error = ErasedError;
    type Enc = ErasedEncoder;
    type AnimationFrameEnc = NoAnimation;

    fn with_pulse(mut self, pulse: SharedPulse) -> Self {
        self.0.set_pulse(pulse);
        self
    }

    fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.0.set_limits(limits);
        self
    }

    fn try_set_policy(&mut self, policy: EncodePolicy) -> Result<(), UnsupportedOperation> {
        // The typed signature cannot carry the native refusal; keep its
        // operation when it is one, otherwise report the policy refusal.
        self.0.try_set_policy(policy).map_err(|e| {
            e.downcast_ref::<UnsupportedOperation>()
                .copied()
                .unwrap_or(UnsupportedOperation::EncodePolicy)
        })
    }

    fn try_set_resolution(&mut self, resolution: Resolution) -> Result<(), ResolutionError> {
        self.0.try_set_resolution(resolution).map_err(|e| {
            e.downcast_ref::<ResolutionError>()
                .copied()
                .unwrap_or(ResolutionError::Unsupported)
        })
    }

    fn with_metadata_policy(mut self, meta: Metadata, policy: MetadataPolicy) -> Self {
        self.0.set_metadata_policy(meta, policy);
        self
    }

    #[allow(deprecated)]
    fn with_metadata(mut self, meta: Metadata) -> Self {
        self.0.set_metadata(meta);
        self
    }

    fn with_canvas_size(mut self, width: u32, height: u32) -> Self {
        self.0.set_canvas_size(width, height);
        self
    }

    fn with_loop_count(mut self, count: Option<u32>) -> Self {
        self.0.set_loop_count(count);
        self
    }

    fn extensions(&self) -> Option<&dyn Any> {
        self.0.extensions()
    }

    fn extensions_mut(&mut self) -> Option<&mut dyn Any> {
        self.0.extensions_mut()
    }

    fn plan_encode(&self, declaration: &EncodeDeclaration<'_>) -> Result<EncodePlan, BoxedError> {
        self.0.plan_encode(declaration)
    }

    fn encoder(self) -> Result<ErasedEncoder, ErasedError> {
        self.0.into_encoder().map(ErasedEncoder).map_err(erased)
    }

    fn animation_frame_encoder(self) -> Result<NoAnimation, ErasedError> {
        Err(erased(Box::new(UnsupportedOperation::AnimationEncode)))
    }
}

/// The encoder an [`ErasedJob`] constructs.
pub struct ErasedEncoder(Box<dyn DynEncoder>);

impl ErasedEncoder {
    /// The erased encoder.
    pub fn into_inner(self) -> Box<dyn DynEncoder> {
        self.0
    }
}

impl Encoder for ErasedEncoder {
    type Error = ErasedError;

    fn reject(op: UnsupportedOperation) -> ErasedError {
        erased(Box::new(op))
    }

    fn preferred_strip_height(&self) -> u32 {
        self.0.preferred_strip_height()
    }

    fn encode(self, pixels: PixelSlice<'_>) -> Result<EncodeOutput, ErasedError> {
        self.0.encode(pixels).map_err(erased)
    }

    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> Result<OutputTiming, ErasedError> {
        self.0.begin(width, height, descriptor).map_err(erased)
    }

    fn begin_declared(
        &mut self,
        declaration: &EncodeDeclaration<'_>,
    ) -> Result<EncodePlan, ErasedError> {
        self.0.begin_declared(declaration).map_err(erased)
    }

    fn begin_output(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
        access: OutputAccess,
    ) -> Result<OutputTiming, ErasedError> {
        self.0
            .begin_output(width, height, descriptor, access)
            .map_err(erased)
    }

    fn finish_output(self) -> Result<OutputFinish<EncodeOutput>, ErasedError> {
        self.0.finish_output().map_err(erased)
    }

    fn pending_output(&self) -> &[u8] {
        self.0.pending_output()
    }

    fn consume_output(&mut self, n: usize) {
        self.0.consume_output(n);
    }

    fn push_rows(&mut self, rows: PixelSlice<'_>) -> Result<(), ErasedError> {
        self.0.push_rows(rows).map_err(erased)
    }

    fn finish(self) -> Result<EncodeOutput, ErasedError> {
        self.0.finish().map_err(erased)
    }

    fn encode_from(
        self,
        source: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize,
    ) -> Result<EncodeOutput, ErasedError> {
        self.0.encode_from(source).map_err(erased)
    }
}
