//! Explicit, bounded metadata rewriting of encoded images.
//!
//! Container rewriting belongs to each codec. This module dispatches to enabled
//! codecs and reexports the shared request results and application services.
//! No operation decodes image pixels. ICC profiles remain opaque.
pub use zencodec::metadata_rewrite::{Action, Compression, Entry, Error, Plan, Services};
use zencodec::{MetadataPolicy, metadata_rewrite::Request};

/// Metadata-only request dispatched to enabled codecs.
///
/// Defaults preserve color and orientation and remove descriptive metadata.
/// Enable `metadata` together with the desired codec features. A format without
/// an enabled rewrite implementation returns [`Error::Unsupported`].
#[derive(Clone)]
pub struct ScrubRequest<'a> {
    request: Request<'a>,
}
impl<'a> ScrubRequest<'a> {
    /// Borrow encoded input. No parsing or allocation occurs yet.
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            request: Request::new(input),
        }
    }
    /// EXIF retention. Source XMP retention requires explicit `with_xmp_editing`.
    /// Dropping rendering dependencies is refused; it needs a pixel operation.
    pub fn with_policy(mut self, policy: MetadataPolicy) -> Self {
        self.request = self.request.with_policy(policy);
        self
    }
    /// Inject optional packet editing/compression services at runtime.
    pub fn with_services(mut self, services: &'a dyn Services) -> Self {
        self.request = self.request.with_services(services);
        self
    }
    /// Explicitly replace the ordinary XMP disposition with the editor's result.
    pub fn with_xmp_editing(mut self, enabled: bool) -> Self {
        self.request = self.request.with_xmp_editing(enabled);
        self
    }
    /// Invoke a known-profile normalizer; unknown profiles remain unchanged.
    pub fn with_icc_normalization(mut self, enabled: bool) -> Self {
        self.request = self.request.with_icc_normalization(enabled);
        self
    }
    /// Allow removal of JXL's JPEG reconstruction data. Original JPEG byte-exact
    /// reconstruction is then unavailable; the JXL image codestream is unchanged.
    pub fn with_jpeg_reconstruction_removal(mut self, enabled: bool) -> Self {
        self.request = self.request.with_jpeg_reconstruction_removal(enabled);
        self
    }
    /// Bound encoded input and output independently.
    pub fn with_byte_limits(mut self, input: usize, output: usize) -> Self {
        self.request = self.request.with_byte_limits(input, output);
        self
    }
    /// Bound each parsed/expanded packet and total regenerated metadata.
    pub fn with_metadata_limit(mut self, bytes: usize) -> Self {
        self.request = self.request.with_metadata_limit(bytes);
        self
    }
    /// Bound container carrier count (including image-data chunks).
    pub fn with_carrier_limit(mut self, count: usize) -> Self {
        self.request = self.request.with_carrier_limit(count);
        self
    }
    /// Cooperative cancellation, checked between carriers and long byte walks.
    pub fn with_stop(mut self, stop: &'a dyn enough::Stop) -> Self {
        self.request = self.request.with_stop(stop);
        self
    }
    /// Validate and plan the entire rewrite before any output is emitted.
    ///
    /// Cancellation and the input budget are checked here, before format
    /// detection; the codec's planner repeats them through zenmeta and also
    /// checks that required services were supplied.
    pub fn plan(&self) -> Result<Plan<'a>, Error> {
        self.request.stop().check().map_err(|reason| match reason {
            enough::StopReason::Cancelled => Error::Cancelled,
            other => Error::Stopped(other),
        })?;
        if self.request.input().len() > self.request.max_input() {
            return Err(Error::Limit(zencodec::LimitKind::InputSize));
        }
        #[cfg(feature = "png")]
        if self.request.input().starts_with(b"\x89PNG\r\n\x1a\n") {
            return zenpng::metadata::rewrite(&self.request);
        }
        #[cfg(any(feature = "jxl-decode", feature = "jxl-encode"))]
        if self
            .request
            .input()
            .starts_with(b"\0\0\0\x0cJXL \r\n\x87\n")
            || self.request.input().starts_with(b"\xff\x0a")
        {
            return zenjxl::metadata::rewrite(&self.request);
        }
        #[cfg(feature = "jpeg")]
        if self.request.input().starts_with(b"\xff\xd8") {
            return zenjpeg::metadata::rewrite(&self.request);
        }
        Err(Error::Unsupported(
            "encoded metadata rewrite for this format/build",
        ))
    }
}
