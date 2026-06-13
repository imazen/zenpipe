//! PDF decode integration tests (zenpdf adapter — decode-only Custom format).

#![cfg(feature = "pdf-decode")]

use zencodecs::{DecodeRequest, ImageFormat};

const TEST_PDF: &[u8] = include_bytes!("images/test.pdf");

/// Detection + probe: a `%PDF-` signature routes to the zenpdf Custom format
/// and probe reports page-0 dimensions without rendering.
#[test]
fn pdf_detect_and_probe() {
    let info = DecodeRequest::new(TEST_PDF)
        .probe()
        .expect("PDF probe failed");
    match info.format {
        ImageFormat::Custom(def) => assert_eq!(def.name, "pdf", "expected PDF Custom format"),
        other => panic!("expected PDF Custom format, got {other:?}"),
    }
    assert_eq!(info.format.mime_type(), "application/pdf");
    assert!(
        info.width > 0 && info.height > 0,
        "probe dims must be > 0, got {}x{}",
        info.width,
        info.height
    );
}

/// Full decode renders the first page to pixels.
#[test]
fn pdf_decode_renders_first_page() {
    let out = DecodeRequest::new(TEST_PDF)
        .decode_full_frame()
        .expect("PDF decode failed");
    assert!(
        out.width() > 0 && out.height() > 0,
        "rendered dims must be > 0, got {}x{}",
        out.width(),
        out.height()
    );
    match out.format() {
        ImageFormat::Custom(def) => assert_eq!(def.name, "pdf"),
        other => panic!("expected PDF Custom format, got {other:?}"),
    }
}

/// A `%PDF-` header followed by garbage must error cleanly, never panic.
#[test]
fn pdf_garbage_does_not_panic() {
    let bytes = b"%PDF-1.7\ngarbage that is not a real pdf body";
    let _ = DecodeRequest::new(bytes).probe();
    let _ = DecodeRequest::new(bytes).decode_full_frame();
}
