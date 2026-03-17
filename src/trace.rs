//! Selection decision tracing.
//!
//! [`SelectionTrace`] records the steps taken during codec/format selection
//! for debugging and auditability.

use crate::codec_id::CodecId;
use crate::ImageFormat;
use alloc::string::String;
use alloc::vec::Vec;

/// Audit trail for codec and format selection decisions.
///
/// Every selection operation (format auto-select, encoder/decoder lookup)
/// records its reasoning as a sequence of [`SelectionStep`]s. This enables
/// debugging ("why was AVIF chosen instead of WebP?") without log-level
/// printf debugging.
///
/// # Example
///
/// ```
/// use zencodecs::trace::SelectionTrace;
///
/// let trace = SelectionTrace::new();
/// // After a selection operation, inspect the steps:
/// // for step in trace.steps() { ... }
/// ```
#[derive(Clone, Debug, Default)]
pub struct SelectionTrace {
    steps: Vec<SelectionStep>,
}

/// A single step in a selection decision.
#[derive(Clone, Debug)]
pub enum SelectionStep {
    /// A format was chosen.
    FormatChosen {
        format: ImageFormat,
        reason: &'static str,
    },
    /// A format was skipped during auto-selection.
    FormatSkipped {
        format: ImageFormat,
        reason: &'static str,
    },
    /// An encoder was chosen.
    EncoderChosen {
        id: CodecId,
        priority: i32,
        reason: &'static str,
    },
    /// An encoder was skipped (policy, capability, etc.).
    EncoderSkipped {
        id: CodecId,
        reason: &'static str,
    },
    /// A decoder was chosen.
    DecoderChosen {
        id: CodecId,
        priority: i32,
        reason: &'static str,
    },
    /// A decoder was skipped.
    DecoderSkipped {
        id: CodecId,
        reason: &'static str,
    },
    /// A decoder was tried and failed.
    DecoderFailed {
        id: CodecId,
        error: String,
    },
    /// Fallback from one decoder to another.
    FallbackAttempt {
        from: CodecId,
        to: CodecId,
    },
    /// Informational note.
    Info {
        message: &'static str,
    },
}

impl SelectionTrace {
    /// Create an empty trace.
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    /// Record a step.
    pub fn push(&mut self, step: SelectionStep) {
        self.steps.push(step);
    }

    /// All recorded steps.
    pub fn steps(&self) -> &[SelectionStep] {
        &self.steps
    }

    /// The format that was ultimately chosen, if any.
    pub fn chosen_format(&self) -> Option<ImageFormat> {
        self.steps.iter().find_map(|s| match s {
            SelectionStep::FormatChosen { format, .. } => Some(*format),
            _ => None,
        })
    }

    /// The encoder that was ultimately chosen, if any.
    pub fn chosen_encoder(&self) -> Option<CodecId> {
        self.steps.iter().find_map(|s| match s {
            SelectionStep::EncoderChosen { id, .. } => Some(*id),
            _ => None,
        })
    }

    /// The decoder that was ultimately chosen, if any.
    pub fn chosen_decoder(&self) -> Option<CodecId> {
        self.steps.iter().find_map(|s| match s {
            SelectionStep::DecoderChosen { id, .. } => Some(*id),
            _ => None,
        })
    }

    /// Whether any decoder failed during selection (indicating fallback was attempted).
    pub fn had_failures(&self) -> bool {
        self.steps.iter().any(|s| matches!(s, SelectionStep::DecoderFailed { .. }))
    }

    /// Number of steps recorded.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the trace is empty.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

impl core::fmt::Display for SelectionStep {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::FormatChosen { format, reason } => {
                write!(f, "[chosen] {format:?}: {reason}")
            }
            Self::FormatSkipped { format, reason } => {
                write!(f, "[skip]   {format:?}: {reason}")
            }
            Self::EncoderChosen { id, priority, reason } => {
                write!(f, "[chosen] {id} (priority {priority}): {reason}")
            }
            Self::EncoderSkipped { id, reason } => {
                write!(f, "[skip]   {id}: {reason}")
            }
            Self::DecoderChosen { id, priority, reason } => {
                write!(f, "[chosen] {id} (priority {priority}): {reason}")
            }
            Self::DecoderSkipped { id, reason } => {
                write!(f, "[skip]   {id}: {reason}")
            }
            Self::DecoderFailed { id, error } => {
                write!(f, "[fail]   {id}: {error}")
            }
            Self::FallbackAttempt { from, to } => {
                write!(f, "[fall]   {from} -> {to}")
            }
            Self::Info { message } => {
                write!(f, "[info]   {message}")
            }
        }
    }
}

impl core::fmt::Display for SelectionTrace {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for step in &self.steps {
            writeln!(f, "{step}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_roundtrip() {
        let mut trace = SelectionTrace::new();
        assert!(trace.is_empty());

        trace.push(SelectionStep::FormatSkipped {
            format: ImageFormat::Avif,
            reason: "not allowed by policy",
        });
        trace.push(SelectionStep::FormatChosen {
            format: ImageFormat::Jpeg,
            reason: "first available lossy format",
        });
        trace.push(SelectionStep::EncoderChosen {
            id: CodecId::ZenjpegEncode,
            priority: 100,
            reason: "highest priority",
        });

        assert_eq!(trace.len(), 3);
        assert_eq!(trace.chosen_format(), Some(ImageFormat::Jpeg));
        assert_eq!(trace.chosen_encoder(), Some(CodecId::ZenjpegEncode));
        assert!(!trace.had_failures());
    }

    #[test]
    fn trace_with_failures() {
        let mut trace = SelectionTrace::new();
        trace.push(SelectionStep::DecoderFailed {
            id: CodecId::ZenjpegDecode,
            error: "corrupt header".into(),
        });
        trace.push(SelectionStep::FallbackAttempt {
            from: CodecId::ZenjpegDecode,
            to: CodecId::Custom("zune-jpeg"),
        });
        assert!(trace.had_failures());
    }

    #[test]
    fn display_format() {
        let step = SelectionStep::FormatChosen {
            format: ImageFormat::WebP,
            reason: "best lossy alpha compression",
        };
        let s = alloc::format!("{step}");
        assert!(s.contains("chosen"));
        assert!(s.contains("WebP"));
    }
}
