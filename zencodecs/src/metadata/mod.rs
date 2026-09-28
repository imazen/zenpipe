//! Explicit, bounded metadata rewriting of encoded images.
//!
//! [`ScrubRequest::plan`] borrows image payloads and finishes validation before
//! producing a [`Plan`]. Inspect its report, stream its chunks to a sink, or
//! explicitly collect an encoded output. No operation decodes image pixels.
//! ICC profiles remain opaque; this is not a steganography/privacy certificate.
use alloc::{borrow::Cow, vec::Vec};
use core::ops::Range;
use zencodec::{MetadataPolicy, Orientation};

mod exif;
#[cfg(feature = "jpeg")]
mod jpeg;
mod jxl;
mod png;

/// A failure produces no output plan and leaves the input untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Invalid container framing or metadata.
    Malformed(&'static str),
    /// Preserving the rendering contract requires an unsupported operation.
    Unsupported(&'static str),
    /// A configured resource budget was exceeded.
    Limit(&'static str),
    /// An allocation failed.
    Allocation,
    /// An explicitly injected service refused its input.
    Service(&'static str),
    /// Cooperative cancellation was requested.
    Cancelled,
}
impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "metadata rewrite: {self:?}")
    }
}
impl core::error::Error for Error {}

/// Compression of metadata, never image samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Compression {
    /// PNG iCCP / compressed text.
    Zlib,
    /// JPEG XL brob metadata boxes.
    Brotli,
}

/// Optional runtime services, supplied at the application boundary.
///
/// No XML, Brotli, deflate, or CMS implementation becomes a dependency of this
/// module. Implementations must enforce `limit` *during* parsing/decompression,
/// honor cancellation, and bound intermediate allocations. Returned byte lengths
/// are checked again by the caller, but that cannot bound a service's own memory.
/// Services never receive pixel buffers. Unknown ICC profiles must return `None`.
pub trait Services {
    /// Merge already-filtered descriptive XMP with required output signaling.
    /// Preserve the required packet's properties exactly. The codec validates
    /// gain-map parameters and discovery after this callback; retaining arbitrary
    /// private properties remains the application's explicit policy decision.
    fn merge_xmp(
        &self,
        _descriptive: &[u8],
        _required: &[u8],
        _limit: usize,
        _stop: &dyn enough::Stop,
    ) -> Result<Vec<u8>, Error> {
        Err(Error::Unsupported("XMP merge service not supplied"))
    }
    /// Rewrite selected XMP properties and return serialized XML, or remove it.
    /// The application owns retention semantics; preserve unknown properties only
    /// when explicitly intended. Default refuses, rather than preserving a packet.
    fn rewrite_xmp(
        &self,
        _xml: &[u8],
        _limit: usize,
        _stop: &dyn enough::Stop,
    ) -> Result<Option<Vec<u8>>, Error> {
        Err(Error::Unsupported("XMP editing service not supplied"))
    }
    /// Replace only a recognized, rendering-equivalent ICC profile.
    /// `None` means unrecognized/unchanged, not removal. No arbitrary sanitization.
    fn normalize_icc(
        &self,
        _icc: &[u8],
        _limit: usize,
        _stop: &dyn enough::Stop,
    ) -> Result<Option<Vec<u8>>, Error> {
        Ok(None)
    }
    /// Decompress a metadata packet, bounding expansion while producing output.
    fn decompress(
        &self,
        _kind: Compression,
        _data: &[u8],
        _limit: usize,
        _stop: &dyn enough::Stop,
    ) -> Result<Vec<u8>, Error> {
        Err(Error::Unsupported(
            "metadata decompression service not supplied",
        ))
    }
    /// Compress metadata after an explicit edit/normalization.
    fn compress(
        &self,
        _kind: Compression,
        _data: &[u8],
        _limit: usize,
        _stop: &dyn enough::Stop,
    ) -> Result<Vec<u8>, Error> {
        Err(Error::Unsupported(
            "metadata compression service not supplied",
        ))
    }
}

/// Disposition of an input carrier. Retaining ICC never certifies its privacy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Action {
    /// Retain the original carrier bytes.
    Keep,
    /// Omit the input carrier.
    Remove,
    /// Serialize filtered metadata or generate a replacement carrier.
    Rewrite,
    /// Retain profile contents without certifying their privacy.
    OpaqueIcc,
    /// Explicitly relinquish byte-exact JPEG reconstruction.
    RemoveJpegReconstruction,
    /// Preserve an image codestream without decoding or certifying it.
    OpaqueImageData,
}

/// One complete input carrier, including its framing bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub(crate) range: Range<usize>,
    pub(crate) carrier: &'static str,
    pub(crate) action: Action,
}
impl Entry {
    /// Byte range in the original input. Entries include image payload carriers.
    pub fn source_range(&self) -> Range<usize> {
        self.range.clone()
    }
    /// Stable human-readable carrier category.
    pub fn carrier(&self) -> &'static str {
        self.carrier
    }
    /// What the plan does to this carrier.
    pub fn action(&self) -> Action {
        self.action
    }
}

/// Validated rewrite. Chunks borrow unmodified encoded data from the input.
#[derive(Debug)]
pub struct Plan<'a> {
    parts: Vec<Cow<'a, [u8]>>,
    entries: Vec<Entry>,
    size: usize,
    rewritten: usize,
}
impl<'a> Plan<'a> {
    fn new() -> Self {
        Self {
            parts: Vec::new(),
            entries: Vec::new(),
            size: 0,
            rewritten: 0,
        }
    }
    /// Every input carrier, including removed data and opaque retained profiles.
    pub fn report(&self) -> &[Entry] {
        &self.entries
    }
    /// Complete output in order. Use `try_for_each(|b| writer.write_all(b))`.
    /// A sink failure may leave partial output in the caller's sink.
    pub fn chunks(&self) -> impl Iterator<Item = &[u8]> {
        self.parts.iter().map(|p| p.as_ref())
    }
    /// Exact encoded output length, without allocating or writing output.
    pub fn output_len(&self) -> usize {
        self.size
    }
    /// Explicitly allocate and collect the encoded output once.
    pub fn to_vec(&self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        out.try_reserve_exact(self.size)
            .map_err(|_| Error::Allocation)?;
        for p in self.chunks() {
            out.extend_from_slice(p);
        }
        Ok(out)
    }
    fn record(
        &mut self,
        req: &ScrubRequest<'_>,
        range: Range<usize>,
        carrier: &'static str,
        action: Action,
        bytes: Option<Cow<'a, [u8]>>,
    ) -> Result<(), Error> {
        req.check()?;
        if self.entries.len() >= req.max_carriers {
            return Err(Error::Limit("carrier count"));
        }
        self.entries.try_reserve(1).map_err(|_| Error::Allocation)?;
        self.entries.push(Entry {
            range,
            carrier,
            action,
        });
        if let Some(bytes) = bytes {
            self.size = self
                .size
                .checked_add(bytes.len())
                .ok_or(Error::Limit("output bytes"))?;
            if self.size > req.max_output {
                return Err(Error::Limit("output bytes"));
            }
            if matches!(bytes, Cow::Owned(_)) {
                self.rewritten = self
                    .rewritten
                    .checked_add(bytes.len())
                    .ok_or(Error::Limit("rewritten metadata"))?;
                req.packet(self.rewritten)?;
            }
            self.parts.try_reserve(1).map_err(|_| Error::Allocation)?;
            self.parts.push(bytes);
        }
        Ok(())
    }
    fn keep(
        &mut self,
        req: &ScrubRequest<'a>,
        range: Range<usize>,
        name: &'static str,
    ) -> Result<(), Error> {
        let bytes = &req.input[range.clone()];
        self.record(req, range, name, Action::Keep, Some(Cow::Borrowed(bytes)))
    }
    fn remove(
        &mut self,
        req: &ScrubRequest<'_>,
        range: Range<usize>,
        name: &'static str,
    ) -> Result<(), Error> {
        self.record(req, range, name, Action::Remove, None)
    }
}

/// Metadata-only request. Defaults: remove descriptive metadata, preserve color
/// and orientation, 512 MiB input/output, 16 MiB metadata, 65,536 carriers.
/// Unsupported formats fail rather than falling back to decoding/re-encoding.
#[derive(Clone)]
pub struct ScrubRequest<'a> {
    input: &'a [u8],
    policy: MetadataPolicy,
    services: Option<&'a dyn Services>,
    edit_xmp: bool,
    normalize_icc: bool,
    remove_reconstruction: bool,
    max_input: usize,
    max_output: usize,
    max_metadata: usize,
    max_carriers: usize,
    stop: &'a dyn enough::Stop,
}
impl<'a> ScrubRequest<'a> {
    /// Borrow encoded input. No parsing or allocation occurs yet.
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            policy: MetadataPolicy::ColorAndRotation,
            services: None,
            edit_xmp: false,
            normalize_icc: false,
            remove_reconstruction: false,
            max_input: 512 << 20,
            max_output: 512 << 20,
            max_metadata: 16 << 20,
            max_carriers: 65_536,
            stop: &enough::Unstoppable,
        }
    }
    /// EXIF retention. Source XMP retention requires explicit `with_xmp_editing`.
    /// Dropping rendering dependencies is refused; it needs a pixel operation.
    pub fn with_policy(mut self, policy: MetadataPolicy) -> Self {
        self.policy = policy;
        self
    }
    /// Inject optional packet editing/compression services at runtime.
    pub fn with_services(mut self, services: &'a dyn Services) -> Self {
        self.services = Some(services);
        self
    }
    /// Explicitly replace the ordinary XMP disposition with the editor's result.
    pub fn with_xmp_editing(mut self, enabled: bool) -> Self {
        self.edit_xmp = enabled;
        self
    }
    /// Invoke a known-profile normalizer; unknown profiles remain unchanged.
    pub fn with_icc_normalization(mut self, enabled: bool) -> Self {
        self.normalize_icc = enabled;
        self
    }
    /// Allow removal of JXL's JPEG reconstruction data. Original JPEG byte-exact
    /// reconstruction is then unavailable; the JXL image codestream is unchanged.
    pub fn with_jpeg_reconstruction_removal(mut self, enabled: bool) -> Self {
        self.remove_reconstruction = enabled;
        self
    }
    /// Bound encoded input and output independently.
    pub fn with_byte_limits(mut self, input: usize, output: usize) -> Self {
        self.max_input = input;
        self.max_output = output;
        self
    }
    /// Bound each parsed/expanded packet and total regenerated metadata.
    pub fn with_metadata_limit(mut self, bytes: usize) -> Self {
        self.max_metadata = bytes;
        self
    }
    /// Bound container carrier count (including image-data chunks).
    pub fn with_carrier_limit(mut self, count: usize) -> Self {
        self.max_carriers = count;
        self
    }
    /// Cooperative cancellation, checked between carriers and long byte walks.
    pub fn with_stop(mut self, stop: &'a dyn enough::Stop) -> Self {
        self.stop = stop;
        self
    }
    /// Validate and plan the entire rewrite before any output is emitted.
    pub fn plan(&self) -> Result<Plan<'a>, Error> {
        self.check()?;
        if self.input.len() > self.max_input {
            return Err(Error::Limit("input bytes"));
        }
        if self.policy.fields().xmp.keeps() && !self.edit_xmp {
            return Err(Error::Unsupported(
                "source XMP retention requires an explicit editor",
            ));
        }
        if (self.edit_xmp || self.normalize_icc) && self.services.is_none() {
            return Err(Error::Unsupported("metadata service not supplied"));
        }
        if self.input.starts_with(b"\x89PNG\r\n\x1a\n") {
            return png::plan(self);
        }
        if self.input.starts_with(b"\0\0\0\x0cJXL \r\n\x87\n")
            || self.input.starts_with(b"\xff\x0a")
        {
            return jxl::plan(self);
        }
        #[cfg(feature = "jpeg")]
        if self.input.starts_with(b"\xff\xd8") {
            return jpeg::plan(self);
        }
        Err(Error::Unsupported(
            "encoded metadata rewrite for this format/build",
        ))
    }
    fn check(&self) -> Result<(), Error> {
        self.stop.check().map_err(|_| Error::Cancelled)
    }
    fn packet(&self, size: usize) -> Result<(), Error> {
        if size > self.max_metadata {
            Err(Error::Limit("metadata bytes"))
        } else {
            Ok(())
        }
    }
    fn services(&self) -> Result<&dyn Services, Error> {
        self.services
            .ok_or(Error::Unsupported("metadata service not supplied"))
    }
    fn xmp(&self, xml: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        self.packet(xml.len())?;
        if !self.edit_xmp {
            return Ok(None);
        }
        let out = self
            .services()?
            .rewrite_xmp(xml, self.max_metadata, self.stop)?;
        if let Some(ref bytes) = out {
            self.packet(bytes.len())?;
        }
        self.check()?;
        Ok(out)
    }
    fn icc(&self, bytes: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        self.packet(bytes.len())?;
        if matches!(self.policy.fields().icc, zencodec::IccRetention::Drop) {
            return Err(Error::Unsupported(
                "removing an ICC profile requires explicit color conversion",
            ));
        }
        if !self.normalize_icc {
            return Ok(None);
        }
        let out = self
            .services()?
            .normalize_icc(bytes, self.max_metadata, self.stop)?;
        if let Some(ref bytes) = out {
            self.packet(bytes.len())?;
            if bytes.len() < 128 || bytes.get(36..40) != Some(b"acsp") {
                return Err(Error::Service("normalizer returned an invalid ICC header"));
            }
        }
        self.check()?;
        Ok(out)
    }
    fn decompress(&self, kind: Compression, bytes: &[u8]) -> Result<Vec<u8>, Error> {
        self.packet(bytes.len())?;
        let out = self
            .services()?
            .decompress(kind, bytes, self.max_metadata, self.stop)?;
        self.packet(out.len())?;
        self.check()?;
        Ok(out)
    }
    fn exif(&self, bytes: &[u8]) -> Result<Vec<u8>, Error> {
        self.packet(bytes.len())?;
        exif::validate(self, bytes)?;
        let exif = zencodec::exif::Exif::parse(bytes).ok_or(Error::Malformed("EXIF"))?;
        let orientation = exif.orientation().unwrap_or(Orientation::Identity);
        let filtered = exif.filtered(&self.policy.fields().exif);
        if filtered.orientation().unwrap_or(Orientation::Identity) != orientation {
            return Err(Error::Unsupported(
                "removing orientation requires explicit pixel transformation",
            ));
        }
        Ok(filtered.to_bytes())
    }
}

fn be32(bytes: &[u8]) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        bytes
            .get(..4)
            .ok_or(Error::Malformed("truncated integer"))?
            .try_into()
            .unwrap(),
    ))
}
