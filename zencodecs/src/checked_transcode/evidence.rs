//! Metadata evidence retained with the adopted native route.
use super::*;
use zencodec::inventory::{Disposition, PartTag};

pub(super) struct Epoch {
    width: u32,
    height: u32,
    format: ImageFormat,
    color: zencodec::decode::SourceColor,
    orientation: zencodec::Orientation,
    resolution: Option<zencodec::Resolution>,
    embedded: zencodec::decode::EmbeddedMetadata,
}
impl Epoch {
    fn capture(info: &ImageInfo) -> Self {
        Self {
            width: info.width,
            height: info.height,
            format: info.format,
            color: info.source_color.clone(),
            orientation: info.orientation,
            resolution: info.resolution,
            embedded: info.embedded_metadata.clone(),
        }
    }
    pub(super) fn check(&self, info: &ImageInfo) -> Result<(), Cause> {
        require_primary(info)?;
        self.check_fields(info)
    }
    fn check_fields(&self, info: &ImageInfo) -> Result<(), Cause> {
        if (self.width, self.height, self.format) != (info.width, info.height, info.format)
            || self.color != info.source_color
            || self.orientation != info.orientation
            || self.resolution != info.resolution
            || self.embedded != info.embedded_metadata
        {
            return Err(Refusal::LateMetadata.into());
        }
        Ok(())
    }
}

// Qualification is deliberately restricted to the native JPEG/PNG services
// reviewed at the manifest pins. Inventory proves structural coverage, not
// fidelity or freedom from identifying content. Unknown/trailing bytes are not
// copied. A different registered implementation must honor these trait contracts.
pub(super) fn qualify(
    codecs: &CodecSet,
    input: Option<&[u8]>,
    info: &ImageInfo,
    treatment: MetadataTransfer,
) -> Result<Epoch, Cause> {
    let epoch = Epoch::capture(info);
    let Some(input) = input else {
        // No implicit replay, retained input allocation or source pass. Streaming
        // discard relies on the controlled decoder's final color/orientation
        // contract, with subsequent events/Done checked against this epoch.
        return if treatment == MetadataTransfer::PreserveReported {
            Err(Refusal::MetadataCompleteness.into())
        } else {
            Ok(epoch)
        };
    };
    if !matches!(info.format, ImageFormat::Jpeg | ImageFormat::Png) {
        return Err(Refusal::MetadataCompleteness.into());
    }
    let inspector = codecs.decode_job(info.format)?;
    let inventory = inspector.inventory(input)?;
    inventory.validate()?;
    if inventory.format() != info.format || inventory.input_len() != input.len() as u64 {
        return Err(Refusal::MetadataCompleteness.into());
    }
    // PNG's probe reports only the pre-IDAT snapshot. Its inventory follows
    // finish_metadata too. Refuse any surfaced late metadata rather than
    // silently treating that early snapshot as complete. JPEG probe_full at
    // this pin already reads native preserved extras through primary EOI.
    if info.format == ImageFormat::Png {
        let cutoff = inventory
            .parts()
            .iter()
            .find(|part| part.parent().is_none() && part.tag() == &PartTag::FourCc(*b"IDAT"))
            .ok_or(Refusal::MetadataCompleteness)?
            .range()
            .start;
        if inventory.parts().iter().any(|part| {
            part.range().start > cutoff && matches!(part.disposition(), Disposition::Metadata(_))
        }) {
            return Err(Refusal::MetadataCompleteness.into());
        }
    }
    let complete = inspector.probe_full(input)?;
    if info.format == ImageFormat::Jpeg {
        require_primary(&complete)?;
    }
    // PNG's header probe does not qualify container scope: the controlled
    // Final already did that for the source. Do not turn probe Unknown into
    // either a positive scope claim or a false refusal of that retained route.
    // Compare actual carriers/context, not just their presence (split ICC can
    // produce a present but incomplete header result). A different decode
    // policy or applied orientation also refuses when it cannot be reconciled.
    epoch.check_fields(&complete)?;
    Ok(epoch)
}

// Preservation on a file/range source retains the very bytes already supplied
// to the driver. One admitted compressed-input copy; no replay or hidden
// source pass. Current native controlled JPEG/PNG read the whole file in order.
// Prefix growth admits both old and new buffers before copying. A sparse or
// still-unknown-length source remains unqualified rather than filling gaps
// with another read. DiscardDescriptive does not acquire this extra copy.
pub(super) struct Input<'a> {
    resident: Option<&'a [u8]>,
    capture: bool,
    total: Option<usize>,
    prefix: usize,
    bytes: Option<zencodec::storage::OwnedStorage<u8>>,
    budget: StorageBudget,
}
impl<'a> Input<'a> {
    pub(super) fn new(
        resident: Option<&'a [u8]>,
        treatment: MetadataTransfer,
        budget: StorageBudget,
    ) -> Self {
        Self {
            resident,
            capture: resident.is_none() && treatment == MetadataTransfer::PreserveReported,
            total: None,
            prefix: 0,
            bytes: None,
            budget,
        }
    }
    fn length(&mut self, total: u64) -> Result<(), Cause> {
        if self.capture {
            let total = usize::try_from(total)?;
            if self.total.is_some_and(|old| old != total) {
                return Err(Refusal::LateMetadata.into());
            }
            if total < self.prefix {
                return Err(Refusal::LateMetadata.into());
            }
            self.total = Some(total);
        }
        Ok(())
    }
    fn before_read(&mut self, hint: Option<u64>, offset: u64, len: usize) -> Result<(), Cause> {
        if self.capture {
            if let Some(hint) = hint {
                self.length(hint)?;
            }
            // Detection can request a prefix before asking the source length.
            // Grow only this admitted observation buffer; never read ahead.
            let end = usize::try_from(offset)?
                .checked_add(len)
                .ok_or(Refusal::SizeOverflow)?;
            let needed = self.total.unwrap_or(end);
            if self.bytes.as_ref().is_none_or(|bytes| bytes.len() < needed) {
                let mut replacement = self.budget.bytes(needed)?;
                if let Some(previous) = &self.bytes {
                    replacement.as_mut_slice()[..self.prefix]
                        .copy_from_slice(&previous.as_slice()[..self.prefix]);
                }
                self.bytes = Some(replacement);
            }
        }
        Ok(())
    }
    fn record(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Cause> {
        if !self.capture {
            return Ok(());
        }
        let offset = usize::try_from(offset)?;
        let end = offset
            .checked_add(bytes.len())
            .ok_or(Refusal::SizeOverflow)?;
        let retained = self
            .bytes
            .as_mut()
            .ok_or(Refusal::MetadataCompleteness)?
            .as_mut_slice();
        if offset > self.prefix || end > retained.len() {
            return Err(Refusal::MetadataCompleteness.into());
        }
        let overlap = self.prefix.min(end);
        if retained[offset..overlap] != bytes[..overlap - offset] {
            return Err(Refusal::LateMetadata.into());
        }
        retained[offset..end].copy_from_slice(bytes);
        self.prefix = self.prefix.max(end);
        Ok(())
    }
    pub(super) fn complete(&self) -> Option<&[u8]> {
        self.resident.or_else(|| {
            (self.total == Some(self.prefix))
                .then(|| self.bytes.as_ref().map(|b| &b.as_slice()[..self.prefix]))
                .flatten()
        })
    }
    pub(super) fn release_capture(&mut self) {
        self.capture = false;
        self.bytes = None;
    }
}

// Boxing the original source error keeps it inspectable across the admission
// adapter. Box<dyn Error> alone does not implement Error for the ReadAt bound.
#[derive(Debug)]
pub(super) struct ReadError(Cause);
impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("transcode source evidence failed")
    }
}
impl core::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}
impl From<Cause> for ReadError {
    fn from(cause: Cause) -> Self {
        Self(cause)
    }
}

pub(super) struct Source<'a, 'input, S: ?Sized> {
    pub(super) source: &'a mut S,
    pub(super) input: &'a mut Input<'input>,
}
impl<S: zencodec::source::ReadAt + ?Sized> zencodec::source::ReadAt for Source<'_, '_, S> {
    type Error = ReadError;
    fn len(&mut self) -> Result<u64, ReadError> {
        let len = self.source.len().map_err(|e| ReadError(Box::new(e)))?;
        self.input.length(len)?;
        Ok(len)
    }
    fn len_hint(&self) -> Option<u64> {
        self.source.len_hint()
    }
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, ReadError> {
        self.input
            .before_read(self.source.len_hint(), offset, out.len())?;
        let n = self
            .source
            .read_at(offset, out)
            .map_err(|e| ReadError(Box::new(e)))?;
        let bytes = out
            .get(..n)
            .ok_or_else(|| ReadError(Refusal::MetadataCompleteness.into()))?;
        self.input.record(offset, bytes)?;
        if n < out.len() && offset.checked_add(n as u64) == Some(self.input.prefix as u64) {
            self.input.length(self.input.prefix as u64)?;
        }
        Ok(n)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), ReadError> {
        self.input
            .before_read(self.source.len_hint(), offset, out.len())?;
        self.source
            .read_exact_at(offset, out)
            .map_err(|e| ReadError(Box::new(e)))?;
        self.input.record(offset, out)?;
        Ok(())
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), ReadError> {
        for (offset, bytes) in requests.iter() {
            self.input
                .before_read(self.source.len_hint(), *offset, bytes.len())?;
        }
        self.source
            .read_many_at(requests)
            .map_err(|e| ReadError(Box::new(e)))?;
        for (offset, bytes) in requests {
            self.input.record(*offset, bytes)?;
        }
        Ok(())
    }
}
#[cfg(feature = "checked-transcode-async")]
impl<S: zencodec_io::tokio_io::AsyncSource + ?Sized> zencodec_io::tokio_io::AsyncSource
    for Source<'_, '_, S>
{
    fn len_hint(&self) -> Option<u64> {
        self.source.len_hint()
    }
    async fn len(&mut self) -> Result<u64, Cause> {
        let len = self.source.len().await?;
        self.input.length(len)?;
        Ok(len)
    }
    async fn read_ranges(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), Cause> {
        for (offset, bytes) in requests.iter() {
            self.input
                .before_read(self.source.len_hint(), *offset, bytes.len())?;
        }
        self.source.read_ranges(requests).await?;
        for (offset, bytes) in requests {
            self.input.record(*offset, bytes)?;
        }
        Ok(())
    }
    async fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<usize, Cause> {
        self.input
            .before_read(self.source.len_hint(), offset, out.len())?;
        let n = self.source.read_at(offset, out).await?;
        let bytes = out.get(..n).ok_or(Refusal::MetadataCompleteness)?;
        self.input.record(offset, bytes)?;
        if n == 0 && offset == self.input.prefix as u64 {
            self.input.length(offset)?;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Reads {
        bytes: &'static [u8],
        reads: usize,
    }
    impl zencodec::source::ReadAt for Reads {
        type Error = core::convert::Infallible;
        fn len(&mut self) -> Result<u64, Self::Error> {
            Ok(self.bytes.len() as u64)
        }
        fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Self::Error> {
            self.reads += 1;
            out.copy_from_slice(&self.bytes[offset as usize..offset as usize + out.len()]);
            Ok(())
        }
    }
    #[test]
    fn admitted_observation_uses_only_requested_reads_and_releases_the_copy() {
        use zencodec::source::ReadAt;
        let budget = StorageBudget::new(32, 3);
        let mut input = Input::new(None, MetadataTransfer::PreserveReported, budget.clone());
        let mut source = Reads {
            bytes: b"abcdefgh",
            reads: 0,
        };
        let mut adapter = Source {
            source: &mut source,
            input: &mut input,
        };
        adapter.read_exact_at(0, &mut [0; 2]).unwrap();
        assert!(adapter.input.complete().is_none());
        assert_eq!(adapter.len().unwrap(), 8);
        adapter.read_exact_at(2, &mut [0; 6]).unwrap();
        assert_eq!(adapter.input.complete(), Some(&b"abcdefgh"[..]));
        assert_eq!(adapter.source.reads, 2);
        assert_eq!(budget.reserved_bytes(), 8);
        adapter.input.release_capture();
        assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
    }
    #[test]
    fn short_positional_read_records_eof_without_an_extra_length_request() {
        use zencodec::source::ReadAt;
        let mut input = Input::new(
            None,
            MetadataTransfer::PreserveReported,
            StorageBudget::new(32, 3),
        );
        let mut source = Reads {
            bytes: b"abcdef",
            reads: 0,
        };
        let mut adapter = Source {
            source: &mut source,
            input: &mut input,
        };
        let mut out = [0; 16];
        assert_eq!(adapter.read_at(0, &mut out).unwrap(), 6);
        assert_eq!(adapter.input.complete(), Some(&b"abcdef"[..]));
        assert_eq!(adapter.source.reads, 1);
    }
    #[test]
    fn capture_admission_precedes_reads_and_changed_overlap_refuses() {
        use zencodec::source::ReadAt;
        let budget = StorageBudget::new(1, 1);
        let mut input = Input::new(None, MetadataTransfer::PreserveReported, budget.clone());
        let mut source = Reads {
            bytes: b"ab",
            reads: 0,
        };
        let mut adapter = Source {
            source: &mut source,
            input: &mut input,
        };
        assert!(adapter.read_exact_at(0, &mut [0; 2]).is_err());
        assert_eq!(adapter.source.reads, 0);
        assert_eq!((budget.reserved_bytes(), budget.reserved_objects()), (0, 0));
        let mut input = Input::new(
            None,
            MetadataTransfer::PreserveReported,
            StorageBudget::new(8, 2),
        );
        input.before_read(Some(2), 0, 2).unwrap();
        input.record(0, b"ab").unwrap();
        let cause = input.record(1, b"c").unwrap_err();
        assert_eq!(
            zencodec::find_cause::<Refusal>(cause.as_ref()),
            Some(&Refusal::LateMetadata)
        );
        assert_eq!(input.complete(), Some(&b"ab"[..]));
    }
}
