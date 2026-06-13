//! PDF decode adapter -- renders PDF pages via zenpdf through the trait interface.
//!
//! Decode-only, like the RAW adapter: PDF is a document format, so zencodecs
//! treats it as an `ImageFormat::Custom(&zenpdf::PDF_FORMAT)` and renders the
//! first page. zenpdf reports the page count through `ImageSequence::Multi`, so
//! callers can discover multi-page documents from the probe/decode `ImageInfo`.
//! Default render is page 0 at 72 DPI with a white background.

use alloc::borrow::Cow;

use crate::error::Result;
use crate::limits::to_resource_limits;
use crate::{CodecError, DecodeOutput, ImageInfo, Limits, StopToken};
use whereat::at;
use zencodec::decode::{Decode, DecodeJob as _, DecoderConfig as _};

/// The `ImageFormat` for PDF files, re-exported from zenpdf.
pub(crate) fn pdf_format() -> zencodec::ImageFormat {
    zencodec::ImageFormat::Custom(&zenpdf::PDF_FORMAT)
}

/// Detect PDF from the `%PDF-` signature.
pub(crate) fn detect_pdf(data: &[u8]) -> bool {
    data.len() >= 5 && data[..5] == *b"%PDF-"
}

fn map_err(e: impl core::error::Error + Send + Sync + 'static) -> whereat::At<CodecError> {
    at!(CodecError::Codec {
        format: pdf_format(),
        source: alloc::boxed::Box::new(e),
    })
}

/// Probe PDF metadata (page-0 dimensions + page count) without rendering.
pub(crate) fn probe(data: &[u8]) -> Result<ImageInfo> {
    let job = zenpdf::PdfDecoderConfig::new().job();
    job.probe(data).map_err(map_err)
}

/// Render the first PDF page to RGBA8 pixels.
pub(crate) fn decode(
    data: &[u8],
    limits: Option<&Limits>,
    stop: Option<StopToken>,
    decode_policy: Option<zencodec::decode::DecodePolicy>,
) -> Result<DecodeOutput> {
    let mut job = zenpdf::PdfDecoderConfig::new().job();
    if let Some(lim) = limits {
        job = job.with_limits(to_resource_limits(lim));
    }
    if let Some(s) = stop {
        job = job.with_stop(s);
    }
    if let Some(dp) = decode_policy {
        job = job.with_policy(dp);
    }
    job.decoder(Cow::Borrowed(data), &[])
        .map_err(map_err)?
        .decode()
        .map_err(map_err)
}
