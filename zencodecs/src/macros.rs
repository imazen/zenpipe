//! Internal dispatch helpers.
//!
//! [`dispatch_format!`] collapses the per-format `match ImageFormat { … }` tables
//! that used to hand-write *both* a `#[cfg(feature)]` arm and a matching
//! `#[cfg(not(feature))]` "compiled out" arm for every codec. Each
//! `Variant => "feature" => body` entry expands to that pair — the feature-gated
//! body, plus a compiled-out arm yielding the `unsupported =` value — so the
//! match stays exhaustive whatever the feature set, with half the source.
//!
//! The comma-separated entry list is terminated by `;`; everything after it is
//! emitted verbatim as the trailing arms (`Custom(_)` guards and the `_`
//! fallback):
//!
//! ```ignore
//! dispatch_format! {
//!     format, unsupported = Err(at!(CodecError::UnsupportedFormat(format)));
//!     Jpeg => "jpeg" => crate::codecs::jpeg::probe(data)?,
//!     WebP => "webp" => crate::codecs::webp::probe(data)?;
//!     ImageFormat::Custom(def) if def.name == "dng" => raw::probe(data)?,
//!     _ => return Err(at!(CodecError::UnsupportedFormat(format))),
//! }
//! ```

/// Per-format dispatch `match` with auto-generated compiled-out arms. See the
/// [module docs](self).
macro_rules! dispatch_format {
    (
        $scrutinee:expr, unsupported = $unsupported:expr;
        $( $variant:ident => $feature:literal => $body:expr ),+ ;
        $( $rest:tt )*
    ) => {
        match $scrutinee {
            $(
                #[cfg(feature = $feature)]
                $crate::ImageFormat::$variant => $body,
                #[cfg(not(feature = $feature))]
                $crate::ImageFormat::$variant => $unsupported,
            )+
            $( $rest )*
        }
    };
}

pub(crate) use dispatch_format;
