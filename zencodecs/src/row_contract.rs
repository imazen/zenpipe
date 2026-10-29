//! Enforce the shared row protocol around dispatched codec implementations.

use crate::{ImageFormat, Limits};
use alloc::boxed::Box;
use alloc::vec::Vec;
use zencodec::encode::{BoxedError, DynEncoder, EncodeOutput, OutputTiming};
use zenpixels::{PixelDescriptor, PixelSlice, PixelSliceMut};

/// Empty inputs accepted by the bitmap encoders stay accepted.
/// Other formats require a nonempty canvas; validate before backend calls
/// that may otherwise panic while constructing row or plane views.
pub(crate) fn check_dimensions(
    format: ImageFormat,
    width: u32,
    height: u32,
) -> core::result::Result<(), BoxedError> {
    let invalid = match format {
        ImageFormat::Jpeg
        | ImageFormat::Png
        | ImageFormat::WebP
        | ImageFormat::Gif
        | ImageFormat::Avif
        | ImageFormat::Jxl
        | ImageFormat::Tiff => width == 0 || height == 0,
        ImageFormat::Farbfeld => width == 0,
        _ => false,
    };
    if invalid {
        Err(
            crate::CodecError::InvalidInput("empty canvas is not supported by this encoder".into())
                .into(),
        )
    } else {
        Ok(())
    }
}

pub(crate) struct RowEncoder {
    inner: Box<dyn DynEncoder + Send>,
    format: ImageFormat,
    limits: Limits,
    orientation: zencodec::Orientation,
    declaration: Option<(u32, u32, PixelDescriptor)>,
    layout: Option<(u32, PixelDescriptor)>,
    timing: Option<OutputTiming>,
    rows: u32,
    sent: u64,
    started: bool,
    failed: bool,
    empty: bool,
}

impl RowEncoder {
    pub(crate) fn new(
        inner: Box<dyn DynEncoder + Send>,
        format: ImageFormat,
        limits: Limits,
        orientation: zencodec::Orientation,
    ) -> Self {
        Self {
            inner,
            format,
            limits,
            orientation,
            declaration: None,
            layout: None,
            timing: None,
            rows: 0,
            sent: 0,
            started: false,
            failed: false,
            empty: false,
        }
    }

    fn dimensions(&self, width: u32, height: u32) -> core::result::Result<(), BoxedError> {
        let (w, h) = self.orientation.output_dimensions(width, height);
        check_dimensions(self.format, w, h)?;
        self.limits.check_dimensions(w, h)?;
        Ok(())
    }

    fn pending_limit(&self) -> core::result::Result<(), BoxedError> {
        let total = self
            .sent
            .checked_add(self.inner.pending_output().len() as u64)
            .ok_or("encoded byte count overflow")?;
        self.limits.check_output_size(total)?;
        Ok(())
    }

    fn one_shot(&self) -> core::result::Result<(), BoxedError> {
        if self.started || self.declaration.is_some() || self.failed {
            Err("one-shot encode after row encoding started".into())
        } else {
            Ok(())
        }
    }
}

impl DynEncoder for RowEncoder {
    fn preferred_strip_height(&self) -> u32 {
        self.inner.preferred_strip_height()
    }

    fn encode(
        self: Box<Self>,
        pixels: PixelSlice<'_>,
    ) -> core::result::Result<EncodeOutput, BoxedError> {
        self.one_shot()?;
        self.dimensions(pixels.width(), pixels.rows())?;
        let limits = self.limits;
        let output = self.inner.encode(pixels)?;
        limits.check_output_size(output.len() as u64)?;
        Ok(output)
    }

    fn encode_srgba8(
        self: Box<Self>,
        data: &mut [u8],
        opaque: bool,
        width: u32,
        height: u32,
        stride: u32,
    ) -> core::result::Result<EncodeOutput, BoxedError> {
        self.one_shot()?;
        self.dimensions(width, height)?;
        let limits = self.limits;
        let output = self
            .inner
            .encode_srgba8(data, opaque, width, height, stride)?;
        limits.check_output_size(output.len() as u64)?;
        Ok(output)
    }

    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: PixelDescriptor,
    ) -> core::result::Result<OutputTiming, BoxedError> {
        if self.started || self.declaration.is_some() || self.failed {
            return Err("begin after an earlier declaration or rows".into());
        }
        self.dimensions(width, height)?;
        // Empty canvases have no row payload. Keep the backend untouched and
        // use its one-shot empty encode at finish, with no bytes exposed early.
        let empty =
            (width == 0 || height == 0) && self.orientation == zencodec::Orientation::Identity;
        let mut timing = if empty {
            OutputTiming::AT_FINISH
        } else {
            self.inner.begin(width, height, descriptor)?
        };
        if timing.streams() && timing.lag_rows().is_some_and(|lag| lag >= height) {
            timing = OutputTiming::AT_FINISH;
        }
        self.pending_limit()?;
        self.empty = empty;
        self.declaration = Some((width, height, descriptor));
        self.layout = Some((width, descriptor));
        self.timing = Some(timing);
        Ok(timing)
    }

    fn pending_output(&self) -> &[u8] {
        if !self.failed && self.timing.is_some_and(|t| t.streams()) {
            self.inner.pending_output()
        } else {
            &[]
        }
    }

    fn consume_output(&mut self, n: usize) {
        let n = n.min(self.pending_output().len());
        if n == 0 {
            return;
        }
        match self.sent.checked_add(n as u64) {
            Some(sent) => {
                self.sent = sent;
                self.inner.consume_output(n);
            }
            None => self.failed = true,
        }
    }

    fn take_output(&mut self, into: &mut Vec<u8>) {
        let n = self.pending_output().len();
        if n == 0 {
            return;
        }
        match self.sent.checked_add(n as u64) {
            Some(sent) => {
                self.sent = sent;
                self.inner.take_output(into);
            }
            None => self.failed = true,
        }
    }

    fn push_rows(&mut self, rows: PixelSlice<'_>) -> core::result::Result<(), BoxedError> {
        if self.failed {
            return Err("row encoding already failed".into());
        }
        let result = (|| {
            let layout = (rows.width(), rows.descriptor());
            if self.layout.is_some_and(|l| l != layout) {
                return Err("rows differ from the declared width or descriptor".into());
            }
            let count = self
                .rows
                .checked_add(rows.rows())
                .ok_or("row count overflow")?;
            if self.declaration.is_some_and(|(_, h, _)| count > h) {
                return Err("more rows than declared".into());
            }
            if self.declaration.is_none() {
                self.dimensions(rows.width(), count.max(1))?;
            }
            if !self.empty {
                self.inner.push_rows(rows)?;
            }
            self.layout = Some(layout);
            self.rows = count;
            self.started = true;
            self.pending_limit()
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn finish(self: Box<Self>) -> core::result::Result<EncodeOutput, BoxedError> {
        if self.failed {
            return Err("row encoding already failed".into());
        }
        if self.declaration.is_some_and(|(_, h, _)| self.rows != h) {
            return Err("fewer rows than declared".into());
        }
        if self.declaration.is_none() && !self.started {
            return Err("finish before rows or declaration".into());
        }
        if let Some((width, _)) = self.layout {
            self.dimensions(width, self.rows)?;
        }
        let Self {
            inner,
            limits,
            sent,
            declaration,
            empty,
            ..
        } = *self;
        let output = if empty {
            let (width, height, descriptor) =
                declaration.ok_or("missing empty canvas declaration")?;
            let stride = (width as usize)
                .checked_mul(descriptor.bytes_per_pixel())
                .ok_or("row size overflow")?;
            inner.encode(PixelSlice::new(&[], width, height, stride, descriptor)?)?
        } else {
            inner.finish()?
        };
        let total = sent
            .checked_add(output.len() as u64)
            .ok_or("encoded byte count overflow")?;
        limits.check_output_size(total)?;
        Ok(output)
    }

    fn encode_from(
        self: Box<Self>,
        source: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize,
    ) -> core::result::Result<EncodeOutput, BoxedError> {
        self.one_shot()?;
        let limits = self.limits;
        let output = self.inner.encode_from(source)?;
        limits.check_output_size(output.len() as u64)?;
        Ok(output)
    }
}
