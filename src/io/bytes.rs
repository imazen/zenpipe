//! Owned resident input for pipelines that outlive the caller's borrow.

use alloc::sync::Arc;
use alloc::vec::Vec;

use zencodec::source::{ReadAt, SliceReadAt, SourceError};

/// Immutable bytes shared by reference count: the zero-copy resident source
/// for pipelines whose operators box a `'static` upstream.
///
/// Drivers detect it through [`ReadAt::as_slice`] and read it in place, as
/// they do [`SliceReadAt`]. Cloning shares the allocation.
///
/// This retains the complete encoded input for as long as any clone or the
/// decoding source lives. It removes a copy; it does not make the operation's
/// memory bounded or streaming. Use a file or range source when the encoded
/// input itself must not be resident, and remember that decoders which
/// retain their whole input before the final interpretation (the current
/// JPEG and PNG routes) hold it either way.
#[derive(Clone, Debug)]
pub struct SharedBytes(Arc<[u8]>);

impl SharedBytes {
    /// Share `bytes` without copying an `Arc<[u8]>`; a `Vec` moves once.
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self(bytes.into())
    }

    /// The shared bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for SharedBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

impl From<Arc<[u8]>> for SharedBytes {
    fn from(bytes: Arc<[u8]>) -> Self {
        Self(bytes)
    }
}

impl ReadAt for SharedBytes {
    type Error = SourceError;

    fn len(&mut self) -> Result<u64, SourceError> {
        Ok(self.0.len() as u64)
    }

    fn len_hint(&self) -> Option<u64> {
        Some(self.0.len() as u64)
    }

    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, SourceError> {
        SliceReadAt::new(&self.0).read_at(offset, out)
    }

    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), SourceError> {
        SliceReadAt::new(&self.0).read_exact_at(offset, out)
    }

    fn as_slice(&self) -> Option<&[u8]> {
        Some(&self.0)
    }
}
