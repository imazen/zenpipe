use zencodec::encode::{BoxedError, DynEncoder, EncodeOutput, OutputBuffer, OutputTiming};
use zenpixels::{PixelDescriptor, PixelSlice, PixelSliceMut};

#[derive(Clone, Copy, Default)]
pub enum Failure {
    #[default]
    None,
    Push,
    Finish,
}

/// Deliberately permissive about protocol state: the sink must enforce its own
/// state even when a downstream implementation accepts another declaration.
#[derive(Default)]
pub struct TestEncoder {
    pub failure: Failure,
    pub buffered: bool,
    pub output: OutputBuffer,
}

impl TestEncoder {
    fn result(self) -> EncodeOutput {
        EncodeOutput::new(self.output.into_remaining(), zencodec::ImageFormat::Pnm)
    }
}

impl DynEncoder for TestEncoder {
    fn preferred_strip_height(&self) -> u32 {
        1
    }

    fn encode(mut self: Box<Self>, pixels: PixelSlice<'_>) -> Result<EncodeOutput, BoxedError> {
        self.output.vec_mut().push(b'H');
        self.push_rows(pixels)?;
        self.finish()
    }

    fn encode_srgba8(
        self: Box<Self>,
        _: &mut [u8],
        _: bool,
        _: u32,
        _: u32,
        _: u32,
    ) -> Result<EncodeOutput, BoxedError> {
        Err("unused test path".into())
    }

    fn begin(&mut self, _: u32, _: u32, _: PixelDescriptor) -> Result<OutputTiming, BoxedError> {
        self.output.vec_mut().push(b'H');
        Ok(if self.buffered {
            OutputTiming::AT_FINISH
        } else {
            OutputTiming::streaming(0)
        })
    }

    fn pending_output(&self) -> &[u8] {
        if self.buffered {
            &[]
        } else {
            self.output.pending()
        }
    }

    fn consume_output(&mut self, n: usize) {
        self.output.consume(n);
    }

    fn take_output(&mut self, into: &mut Vec<u8>) {
        if !self.buffered {
            self.output.take_into(into);
        }
    }

    fn push_rows(&mut self, rows: PixelSlice<'_>) -> Result<(), BoxedError> {
        if matches!(self.failure, Failure::Push) {
            return Err("injected push failure".into());
        }
        for y in 0..rows.rows() {
            self.output.vec_mut().extend_from_slice(rows.row(y));
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<EncodeOutput, BoxedError> {
        if matches!(self.failure, Failure::Finish) {
            return Err("injected finish failure".into());
        }
        self.output.vec_mut().push(b'F');
        Ok((*self).result())
    }

    fn encode_from(
        self: Box<Self>,
        _: &mut dyn FnMut(u32, PixelSliceMut<'_>) -> usize,
    ) -> Result<EncodeOutput, BoxedError> {
        Err("unused test path".into())
    }
}
