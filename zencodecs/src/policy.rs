//! Per-request codec filtering policy.
//!
//! [`CodecPolicy`] composes with the registry to control which codecs are
//! available for a specific operation. It provides killbits, allowlists,
//! preference ordering, and format restrictions.

use crate::ImageFormat;
use crate::codec_id::CodecId;
use crate::format_set::FormatSet;
use alloc::vec::Vec;

/// Per-request codec filtering policy.
///
/// Controls which codec implementations are available for a given operation.
/// Applied on top of the registry — the registry determines what's *compiled in*,
/// the policy determines what's *allowed for this request*.
///
/// # Evaluation order
///
/// 1. **Killbits** (disabled) — explicitly rejected codecs. Always wins.
/// 2. **Allowlist** (allowed) — if present, only these codecs pass.
/// 3. **Preferences** — per-format priority bonuses.
/// 4. **Format restrictions** — which output formats are candidates for auto-selection.
///
/// # Examples
///
/// ```
/// use zencodecs::policy::{CodecPolicy};
/// use zencodecs::codec_id::CodecId;
///
/// // Disable zenjpeg decoder, force fallback to any alternative
/// let policy = CodecPolicy::new()
///     .with_disabled(CodecId::ZenjpegDecode);
///
/// // Only allow pure-Rust codecs
/// let policy = CodecPolicy::pure_rust();
/// ```
#[derive(Clone, Debug, Default)]
pub struct CodecPolicy {
    /// Explicitly disabled codec IDs. Checked first.
    disabled: Vec<CodecId>,
    /// If Some, only these IDs are allowed. If None, all non-disabled are allowed.
    allowed: Option<Vec<CodecId>>,
    /// Per-format preference order. Position 0 gets +1000, 1 gets +900, etc.
    preferences: Vec<(ImageFormat, Vec<CodecId>)>,
    /// Allow fallback to next decoder on error? Default: true.
    fallback_on_error: bool,
    /// Allowed output formats for auto-selection. None = all registered.
    allowed_formats: Option<FormatSet>,
}

impl CodecPolicy {
    /// Empty policy — everything allowed, no preferences.
    pub fn new() -> Self {
        Self {
            disabled: Vec::new(),
            allowed: None,
            preferences: Vec::new(),
            fallback_on_error: true,
            allowed_formats: None,
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Builders
    // ═══════════════════════════════════════════════════════════════════

    /// Disable a specific codec implementation.
    pub fn with_disabled(mut self, id: CodecId) -> Self {
        if !self.disabled.contains(&id) {
            self.disabled.push(id);
        }
        self
    }

    /// Set an explicit allowlist. Only these codec IDs will be available.
    /// Killbits still apply on top of the allowlist.
    pub fn with_allowed(mut self, ids: &[CodecId]) -> Self {
        self.allowed = Some(ids.to_vec());
        self
    }

    /// Set preference order for a format. Position 0 gets the highest bonus.
    ///
    /// When multiple codecs handle the same format, preferences re-order
    /// them. The base priority from registration is adjusted by:
    /// `+1000` for position 0, `+900` for position 1, etc.
    pub fn with_preference(mut self, format: ImageFormat, order: &[CodecId]) -> Self {
        // Replace existing preference for this format
        self.preferences.retain(|(f, _)| *f != format);
        self.preferences.push((format, order.to_vec()));
        self
    }

    /// Enable or disable fallback on decode error. Default: true.
    ///
    /// When enabled, if a decoder fails, the next-priority decoder for the
    /// same format is tried. When disabled, the first error is final.
    pub fn with_fallback(mut self, enabled: bool) -> Self {
        self.fallback_on_error = enabled;
        self
    }

    /// Restrict which output formats are candidates for auto-selection.
    ///
    /// When set, `EncodeRequest::auto()` only considers these formats.
    /// When None, all registered formats are candidates.
    pub fn with_allowed_formats(mut self, formats: FormatSet) -> Self {
        self.allowed_formats = Some(formats);
        self
    }

    // ═══════════════════════════════════════════════════════════════════
    // Presets
    // ═══════════════════════════════════════════════════════════════════

    /// Only pure-Rust codec implementations (no C/C++ FFI).
    pub fn pure_rust() -> Self {
        // All current zen codecs are pure Rust, so this is a no-op for now.
        // When C-based alternatives are added, this will exclude them.
        Self::new()
    }

    /// Codecs safe for wasm32 targets (no threading, no C).
    pub fn wasm_compatible() -> Self {
        // Same as pure_rust for now — zen codecs handle wasm32 internally.
        Self::new()
    }

    /// Web-safe output formats only (JPEG, PNG, GIF).
    pub fn web_safe_output() -> Self {
        Self::new().with_allowed_formats(FormatSet::web_safe())
    }

    /// Modern web output formats (JPEG, PNG, GIF, WebP, AVIF, JXL).
    pub fn modern_web_output() -> Self {
        Self::new().with_allowed_formats(FormatSet::modern_web())
    }

    // ═══════════════════════════════════════════════════════════════════
    // Query
    // ═══════════════════════════════════════════════════════════════════

    /// Whether a specific codec ID is allowed by this policy.
    pub fn is_codec_allowed(&self, id: CodecId) -> bool {
        // Killbits win
        if self.disabled.contains(&id) {
            return false;
        }
        // Allowlist gate
        if let Some(ref allowed) = self.allowed {
            return allowed.contains(&id);
        }
        true
    }

    /// Compute effective priority for a codec entry.
    ///
    /// Adds preference bonus to the base priority from registration.
    pub fn effective_priority(&self, id: CodecId, base_priority: i32, format: ImageFormat) -> i32 {
        base_priority + self.preference_bonus(id, format)
    }

    /// Preference bonus for a codec ID in a given format.
    ///
    /// Position 0 → +1000, position 1 → +900, etc. Not in list → 0.
    fn preference_bonus(&self, id: CodecId, format: ImageFormat) -> i32 {
        for (fmt, order) in &self.preferences {
            if *fmt == format
                && let Some(pos) = order.iter().position(|&x| x == id)
            {
                return (1000 - (pos as i32) * 100).max(0);
            }
        }
        0
    }

    /// Whether a format is allowed for auto-selection.
    pub fn is_format_allowed(&self, format: ImageFormat) -> bool {
        match &self.allowed_formats {
            Some(set) => set.contains(format),
            None => true,
        }
    }

    /// Whether fallback on error is enabled.
    pub fn fallback_enabled(&self) -> bool {
        self.fallback_on_error
    }

    /// The allowed formats set, if any.
    pub fn allowed_formats(&self) -> Option<&FormatSet> {
        self.allowed_formats.as_ref()
    }

    // ═══════════════════════════════════════════════════════════════════
    // Composition
    // ═══════════════════════════════════════════════════════════════════

    /// Merge two policies.
    ///
    /// - `disabled`: union (both killbit sets apply)
    /// - `allowed`: intersection (if both present), or whichever is present
    /// - `preferences`: `other` overrides `self` for any format in `other`
    /// - `fallback_on_error`: disabled if either side disables it
    /// - `allowed_formats`: intersection (if both present)
    pub fn merge(mut self, other: CodecPolicy) -> Self {
        // Disabled: union
        for id in other.disabled {
            if !self.disabled.contains(&id) {
                self.disabled.push(id);
            }
        }

        // Allowed: intersect
        self.allowed = match (self.allowed, other.allowed) {
            (Some(a), Some(b)) => Some(a.into_iter().filter(|id| b.contains(id)).collect()),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        // Preferences: other overrides
        for (format, order) in other.preferences {
            self.preferences.retain(|(f, _)| *f != format);
            self.preferences.push((format, order));
        }

        // Fallback: disabled if either disables
        self.fallback_on_error = self.fallback_on_error && other.fallback_on_error;

        // Allowed formats: intersect
        self.allowed_formats = match (self.allowed_formats, other.allowed_formats) {
            (Some(a), Some(b)) => Some(a.intersection(&b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_policy_allows_everything() {
        let policy = CodecPolicy::new();
        assert!(policy.is_codec_allowed(CodecId::ZenjpegDecode));
        assert!(policy.is_codec_allowed(CodecId::ZenwebpEncode));
        assert!(policy.is_format_allowed(ImageFormat::Jpeg));
        assert!(policy.fallback_enabled());
    }

    #[test]
    fn killbit_disables_codec() {
        let policy = CodecPolicy::new().with_disabled(CodecId::ZenjpegDecode);
        assert!(!policy.is_codec_allowed(CodecId::ZenjpegDecode));
        assert!(policy.is_codec_allowed(CodecId::ZenjpegEncode));
    }

    #[test]
    fn allowlist_restricts() {
        let policy = CodecPolicy::new().with_allowed(&[CodecId::ZenjpegDecode, CodecId::PngDecode]);
        assert!(policy.is_codec_allowed(CodecId::ZenjpegDecode));
        assert!(policy.is_codec_allowed(CodecId::PngDecode));
        assert!(!policy.is_codec_allowed(CodecId::ZenwebpDecode));
    }

    #[test]
    fn killbit_overrides_allowlist() {
        let policy = CodecPolicy::new()
            .with_allowed(&[CodecId::ZenjpegDecode, CodecId::PngDecode])
            .with_disabled(CodecId::ZenjpegDecode);
        assert!(!policy.is_codec_allowed(CodecId::ZenjpegDecode));
        assert!(policy.is_codec_allowed(CodecId::PngDecode));
    }

    #[test]
    fn preference_bonus() {
        let policy =
            CodecPolicy::new().with_preference(ImageFormat::Jpeg, &[CodecId::ZenjpegDecode]);
        let priority = policy.effective_priority(CodecId::ZenjpegDecode, 100, ImageFormat::Jpeg);
        assert_eq!(priority, 1100); // 100 base + 1000 bonus
    }

    #[test]
    fn preference_no_bonus_for_other_format() {
        let policy =
            CodecPolicy::new().with_preference(ImageFormat::Jpeg, &[CodecId::ZenjpegDecode]);
        // PNG codec gets no JPEG preference bonus
        let priority = policy.effective_priority(CodecId::PngDecode, 100, ImageFormat::Png);
        assert_eq!(priority, 100);
    }

    #[test]
    fn allowed_formats() {
        let policy = CodecPolicy::web_safe_output();
        assert!(policy.is_format_allowed(ImageFormat::Jpeg));
        assert!(policy.is_format_allowed(ImageFormat::Png));
        assert!(policy.is_format_allowed(ImageFormat::Gif));
        assert!(!policy.is_format_allowed(ImageFormat::WebP));
        assert!(!policy.is_format_allowed(ImageFormat::Avif));
    }

    #[test]
    fn merge_disabled_union() {
        let a = CodecPolicy::new().with_disabled(CodecId::ZenjpegDecode);
        let b = CodecPolicy::new().with_disabled(CodecId::PngDecode);
        let merged = a.merge(b);
        assert!(!merged.is_codec_allowed(CodecId::ZenjpegDecode));
        assert!(!merged.is_codec_allowed(CodecId::PngDecode));
    }

    #[test]
    fn merge_allowed_intersect() {
        let a = CodecPolicy::new().with_allowed(&[CodecId::ZenjpegDecode, CodecId::PngDecode]);
        let b = CodecPolicy::new().with_allowed(&[CodecId::PngDecode, CodecId::ZenwebpDecode]);
        let merged = a.merge(b);
        assert!(!merged.is_codec_allowed(CodecId::ZenjpegDecode));
        assert!(merged.is_codec_allowed(CodecId::PngDecode));
        assert!(!merged.is_codec_allowed(CodecId::ZenwebpDecode));
    }

    #[test]
    fn merge_fallback_disabled_either() {
        let a = CodecPolicy::new().with_fallback(false);
        let b = CodecPolicy::new();
        assert!(!a.merge(b).fallback_enabled());

        let a = CodecPolicy::new();
        let b = CodecPolicy::new().with_fallback(false);
        assert!(!a.merge(b).fallback_enabled());
    }

    #[test]
    fn merge_formats_intersect() {
        let a = CodecPolicy::new().with_allowed_formats(FormatSet::modern_web());
        let b = CodecPolicy::new().with_allowed_formats(FormatSet::web_safe());
        let merged = a.merge(b);
        assert!(merged.is_format_allowed(ImageFormat::Jpeg));
        assert!(!merged.is_format_allowed(ImageFormat::WebP));
    }

    #[test]
    fn merge_preferences_override() {
        let a = CodecPolicy::new().with_preference(ImageFormat::Jpeg, &[CodecId::ZenjpegDecode]);
        let b = CodecPolicy::new().with_preference(ImageFormat::Jpeg, &[]);
        let merged = a.merge(b);
        // b's empty preference overrides a's
        assert_eq!(
            merged.effective_priority(CodecId::ZenjpegDecode, 100, ImageFormat::Jpeg),
            100
        );
    }
}
