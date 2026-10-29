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
    // XMP's `tiff:Orientation` mirrors the EXIF tag; readers that prefer
    // XMP would otherwise rotate the upright pixels again.
    if m.orientation != Orientation::Identity
        && let Some(xmp) = &m.xmp
        && let Some(fixed) = xmp_orientation_to_1(xmp)
    {
        out.xmp = Some(fixed.into());
    }
    (m.orientation, Some(out))
}

/// Rewrite every single-digit `tiff:Orientation` value in an XMP packet to
/// `1`, in either serialization (`tiff:Orientation="6"` or
/// `<tiff:Orientation>6</tiff:Orientation>`). Same length, so a padded
/// packet keeps its size. `None` when there is nothing to change.
fn xmp_orientation_to_1(xmp: &[u8]) -> Option<alloc::vec::Vec<u8>> {
    const NAME: &[u8] = b"tiff:Orientation";
    let mut out = xmp.to_vec();
    let mut changed = false;
    let mut i = 0;
    while let Some(at) = out[i..].windows(NAME.len()).position(|w| w == NAME) {
        let mut j = i + at + NAME.len();
        i = j;
        let skip_ws = |j: &mut usize, b: &[u8]| {
            while *j < b.len() && b[*j].is_ascii_whitespace() {
                *j += 1;
            }
        };
        skip_ws(&mut j, &out);
        let digit = match out.get(j) {
            // Attribute: `= "6"` or `= '6'`.
            Some(b'=') => {
                j += 1;
                skip_ws(&mut j, &out);
                match out.get(j) {
                    Some(&q @ (b'"' | b'\'')) if out.get(j + 2) == Some(&q) => j + 1,
                    _ => continue,
                }
            }
            // Element: `>6<`, allowing whitespace around the digit.
            Some(b'>') => {
                j += 1;
                skip_ws(&mut j, &out);
                let d = j;
                j += 1;
                skip_ws(&mut j, &out);
                if out.get(j) != Some(&b'<') {
                    continue;
                }
                d
            }
            _ => continue,
        };
        if matches!(out.get(digit), Some(b'2'..=b'8')) {
            out[digit] = b'1';
            changed = true;
        }
    }
    changed.then_some(out)
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
    fn frame(
        &self,
        pixels: PixelSlice<'_>,
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Result<Option<PixelBuffer>, BoxedError> {
        if let Some(pulse) = pulse {
            pulse.check().map_err(|reason| {
                zencodec::CodecError::new(None, zencodec::ErrorCategory::Stopped(reason))
            })?;
        }
        let frame = orient(&pixels, self.orientation);
        if let Some(pulse) = pulse {
            pulse.check().map_err(|reason| {
                zencodec::CodecError::new(None, zencodec::ErrorCategory::Stopped(reason))
            })?;
        }
        Ok(frame)
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
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Result<(), BoxedError> {
        match self.frame(pixels.clone(), pulse)? {
            Some(b) => self.inner.push_frame(b.as_slice(), duration_ms, pulse),
            None => self.inner.push_frame(pixels, duration_ms, pulse),
        }
    }

    fn push_frame_timed(
        &mut self,
        pixels: PixelSlice<'_>,
        duration: zencodec::animation::FrameDuration,
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Result<zencodec::animation::FrameDuration, BoxedError> {
        match self.frame(pixels.clone(), pulse)? {
            Some(b) => self.inner.push_frame_timed(b.as_slice(), duration, pulse),
            None => self.inner.push_frame_timed(pixels, duration, pulse),
        }
    }

    fn push_frame_exact(
        &mut self,
        pixels: PixelSlice<'_>,
        duration: zencodec::animation::FrameDuration,
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Result<(), BoxedError> {
        match self.frame(pixels.clone(), pulse)? {
            Some(b) => self.inner.push_frame_exact(b.as_slice(), duration, pulse),
            None => self.inner.push_frame_exact(pixels, duration, pulse),
        }
    }

    fn finish(
        self: alloc::boxed::Box<Self>,
        pulse: Option<&dyn zencodec::Pulse>,
    ) -> Result<zencodec::encode::EncodeOutput, BoxedError> {
        self.inner.finish(pulse)
    }
}

#[cfg(test)]
mod tests {
    use super::xmp_orientation_to_1;

    #[test]
    fn rewrites_both_xmp_serializations_and_keeps_the_length() {
        let attr = br#"<rdf:Description tiff:Orientation="6" tiff:Make="x"/>"#;
        let got = xmp_orientation_to_1(attr).unwrap();
        assert_eq!(
            got,
            br#"<rdf:Description tiff:Orientation="1" tiff:Make="x"/>"#
        );
        let elem = b"<tiff:Orientation> 8 </tiff:Orientation>";
        assert_eq!(
            xmp_orientation_to_1(elem).unwrap(),
            b"<tiff:Orientation> 1 </tiff:Orientation>"
        );
        let single = b"<x tiff:Orientation = '3'/>";
        assert_eq!(
            xmp_orientation_to_1(single).unwrap(),
            b"<x tiff:Orientation = '1'/>"
        );
    }

    #[test]
    fn leaves_identity_and_other_text_alone() {
        assert!(xmp_orientation_to_1(br#"<x tiff:Orientation="1"/>"#).is_none());
        assert!(xmp_orientation_to_1(br#"<x exif:PixelXDimension="6"/>"#).is_none());
        assert!(xmp_orientation_to_1(b"tiff:Orientation").is_none());
        assert!(xmp_orientation_to_1(br#"<x tiff:Orientation="16"/>"#).is_none());
    }
}

#[cfg(test)]
mod pulse_tests {
    use super::*;
    use alloc::boxed::Box;
    use zencodec::encode::DynAnimationFrameEncoder;
    use zencodec::how_far::{Child, Execution, PhaseSpec, PlanError, Report};

    struct RecordingEncoder {
        expected: Option<usize>,
    }
    impl DynAnimationFrameEncoder for RecordingEncoder {
        fn as_any(&self) -> &dyn core::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
        fn into_any(self: Box<Self>) -> Box<dyn core::any::Any> {
            self
        }
        fn push_frame(
            &mut self,
            pixels: PixelSlice<'_>,
            _: u32,
            pulse: Option<&dyn zencodec::Pulse>,
        ) -> Result<(), BoxedError> {
            assert_eq!(
                pulse.map(|p| p as *const dyn zencodec::Pulse as *const () as usize),
                self.expected
            );
            assert_eq!((pixels.width(), pixels.rows()), (2, 1));
            Ok(())
        }
        fn finish(
            self: Box<Self>,
            pulse: Option<&dyn zencodec::Pulse>,
        ) -> Result<zencodec::encode::EncodeOutput, BoxedError> {
            assert_eq!(
                pulse.map(|p| p as *const dyn zencodec::Pulse as *const () as usize),
                self.expected
            );
            Err(Box::new(zencodec::UnsupportedOperation::AnimationTiming))
        }
    }
    struct TimedOut;
    impl enough::Stop for TimedOut {
        fn check(&self) -> Result<(), enough::StopReason> {
            Err(enough::StopReason::TimedOut)
        }
    }
    impl Report for TimedOut {
        fn advance(&self, _: u64) {}
    }
    impl zencodec::Pulse for TimedOut {
        fn split(
            &self,
            _: Execution,
            _: &[PhaseSpec<'_>],
        ) -> Result<alloc::vec::Vec<Child<'_>>, PlanError> {
            Err(PlanError::Unsupported)
        }
    }

    #[test]
    fn upright_frames_forward_original_optional_pulse() {
        let samples = [1u8, 2, 3, 4, 5, 6];
        let pixels =
            PixelSlice::new(&samples, 1, 2, 3, zenpixels::PixelDescriptor::RGB8_SRGB).unwrap();
        let noop = &zencodec::NoPulse;
        for pulse in [None, Some(noop as &dyn zencodec::Pulse)] {
            let mut encoder = OrientedAnimationEncoder {
                inner: Box::new(RecordingEncoder {
                    expected: pulse.map(|p| p as *const dyn zencodec::Pulse as *const () as usize),
                }),
                orientation: Orientation::Rotate90,
            };
            encoder.push_frame(pixels.clone(), 10, pulse).unwrap();
            assert!(
                Box::new(encoder)
                    .finish(pulse)
                    .unwrap_err()
                    .is::<zencodec::UnsupportedOperation>()
            );
        }
    }

    #[test]
    fn upright_frame_deadline_refuses_before_native_push() {
        let samples = [1u8, 2, 3, 4, 5, 6];
        let pixels =
            PixelSlice::new(&samples, 1, 2, 3, zenpixels::PixelDescriptor::RGB8_SRGB).unwrap();
        for orientation in [Orientation::Identity, Orientation::Rotate90] {
            // Any downstream push would fail its expected-None assertion.
            let mut encoder = OrientedAnimationEncoder {
                inner: Box::new(RecordingEncoder { expected: None }),
                orientation,
            };
            let error = encoder
                .push_frame(pixels.clone(), 10, Some(&TimedOut))
                .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<zencodec::CodecError>()
                    .unwrap()
                    .category(),
                zencodec::ErrorCategory::Stopped(enough::StopReason::TimedOut)
            );
        }
    }
}
