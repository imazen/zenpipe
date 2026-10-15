//! Re-encodes write upright pixels and orientation 1.
//!
//! The reader-compatibility matrix (imazen/zencodec#132) found orientation
//! metadata unreliable on every format a re-encode can target:
//!
//! - no browser applies WebP EXIF orientation;
//! - WebKit's Linux build, WIC and ImageMagick ignore PNG `eXIf`;
//! - WIC's and ImageIO's default decodes and `sips` don't apply JPEG EXIF
//!   orientation;
//! - HEIF `imir` reads opposite ways in the 2017 and 2022 editions.
//!
//! So an encode whose [`Metadata`] asks for a rotation or flip applies it to
//! the pixels and encodes them with orientation 1: the field becomes
//! `Identity` and a non-identity EXIF Orientation tag is rewritten to 1, so no
//! reader rotates again. (A lossless edit that must keep the stored pixels is
//! not an encode; it goes through the codec's own lossless API.)

use zencodec::{Metadata, Orientation};
use zenpixels::{PixelBuffer, PixelSlice};

/// The orientation an encode must apply to its pixels, and the metadata to
/// encode them with afterwards.
pub(crate) fn upright(meta: Option<&Metadata>) -> (Orientation, Option<Metadata>) {
    let Some(m) = meta else {
        return (Orientation::Identity, None);
    };
    let mut out = m.clone();
    out.orientation = Orientation::Identity;
    // The field is authoritative: after baking, a tag that still says
    // otherwise would make readers rotate a second time.
    if let Some(exif) = &m.exif
        && zencodec::helpers::parse_exif_orientation(exif)
            .is_some_and(|o| o != Orientation::Identity)
        && let Some(fixed) = zencodec::helpers::set_exif_orientation(exif, Orientation::Identity)
    {
        out.exif = Some(fixed.into());
    }
    (m.orientation, Some(out))
}

/// Apply `orientation` to `pixels`, or `None` when there is nothing to do.
pub(crate) fn orient(pixels: &PixelSlice<'_>, orientation: Orientation) -> Option<PixelBuffer> {
    (orientation != Orientation::Identity)
        .then(|| zenpixels_convert::orient::apply_orientation(pixels.clone(), orientation))
}

/// An animation encoder that applies `orientation` to every frame, so the
/// animation is written upright like a still. Built with the canvas
/// dimensions already rotated.
pub(crate) struct OrientedAnimationEncoder {
    pub(crate) inner: alloc::boxed::Box<dyn zencodec::encode::DynAnimationFrameEncoder>,
    pub(crate) orientation: Orientation,
}

impl OrientedAnimationEncoder {
    fn frame<'a>(&self, pixels: PixelSlice<'a>) -> Option<PixelBuffer> {
        orient(&pixels, self.orientation)
    }
}

type BoxedError = alloc::boxed::Box<dyn core::error::Error + Send + Sync>;

impl zencodec::encode::DynAnimationFrameEncoder for OrientedAnimationEncoder {
    fn as_any(&self) -> &dyn core::any::Any {
        self.inner.as_any()
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self.inner.as_any_mut()
    }

    fn into_any(self: alloc::boxed::Box<Self>) -> alloc::boxed::Box<dyn core::any::Any> {
        self.inner.into_any()
    }

    fn begin(
        &mut self,
        width: u32,
        height: u32,
        descriptor: zenpixels::PixelDescriptor,
        frame_count: Option<u32>,
    ) -> Result<zencodec::encode::AnimationOutputTiming, BoxedError> {
        let (w, h) = self.orientation.output_dimensions(width, height);
        self.inner.begin(w, h, descriptor, frame_count)
    }

    fn pending_output(&self) -> &[u8] {
        self.inner.pending_output()
    }

    fn consume_output(&mut self, n: usize) {
        self.inner.consume_output(n)
    }

    fn take_output(&mut self, into: &mut alloc::vec::Vec<u8>) {
        self.inner.take_output(into)
    }

    fn push_frame(
        &mut self,
        pixels: PixelSlice<'_>,
        duration_ms: u32,
        stop: Option<&dyn zencodec::enough::Stop>,
    ) -> Result<(), BoxedError> {
        match self.frame(pixels.clone()) {
            Some(b) => self.inner.push_frame(b.as_slice(), duration_ms, stop),
            None => self.inner.push_frame(pixels, duration_ms, stop),
        }
    }

    fn push_frame_timed(
        &mut self,
        pixels: PixelSlice<'_>,
        duration: zencodec::animation::FrameDuration,
        stop: Option<&dyn zencodec::enough::Stop>,
    ) -> Result<zencodec::animation::FrameDuration, BoxedError> {
        match self.frame(pixels.clone()) {
            Some(b) => self.inner.push_frame_timed(b.as_slice(), duration, stop),
            None => self.inner.push_frame_timed(pixels, duration, stop),
        }
    }

    fn push_frame_exact(
        &mut self,
        pixels: PixelSlice<'_>,
        duration: zencodec::animation::FrameDuration,
        stop: Option<&dyn zencodec::enough::Stop>,
    ) -> Result<(), BoxedError> {
        match self.frame(pixels.clone()) {
            Some(b) => self.inner.push_frame_exact(b.as_slice(), duration, stop),
            None => self.inner.push_frame_exact(pixels, duration, stop),
        }
    }

    fn finish(
        self: alloc::boxed::Box<Self>,
        stop: Option<&dyn zencodec::enough::Stop>,
    ) -> Result<zencodec::encode::EncodeOutput, BoxedError> {
        self.inner.finish(stop)
    }
}
