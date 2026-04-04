//! Updates from the editor to the view layer.
//!
//! The view layer receives `ViewUpdate` values and applies them to the DOM
//! (or native UI). No business logic — just rendering state to pixels/widgets.

use serde::{Deserialize, Serialize};

/// An update from the editor to the view.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ViewUpdate {
    // ─── Pixel data for canvases ───
    /// Rendered overview image (small, resized from full source).
    OverviewPixels {
        #[serde(skip)]
        data: Vec<u8>,
        width: u32,
        height: u32,
    },

    /// Rendered detail image (cropped region at higher resolution).
    DetailPixels {
        #[serde(skip)]
        data: Vec<u8>,
        width: u32,
        height: u32,
    },

    // ─── State changes ───
    /// The detail view region changed.
    RegionChanged { x: f32, y: f32, w: f32, h: f32 },

    /// A single parameter value changed.
    ParamChanged { key: String, value: f64 },

    /// A boolean parameter changed.
    ParamBoolChanged { key: String, value: bool },

    /// All parameters were reset to identity.
    AllParamsReset,

    /// Film preset selection changed.
    FilmPresetChanged {
        id: Option<String>,
        intensity: f32,
    },

    /// Undo/redo state changed.
    HistoryChanged {
        can_undo: bool,
        can_redo: bool,
    },

    // ─── Source info ───
    /// Source image loaded and ready.
    SourceLoaded {
        width: u32,
        height: u32,
    },

    /// Source upgraded with native decode (metadata now available).
    MetadataUpgraded {
        format: String,
        has_icc: bool,
        has_exif: bool,
        has_xmp: bool,
        has_gain_map: bool,
    },

    // ─── Export results ───
    /// Encoded preview result (overview-size encode for export modal).
    EncodePreviewResult {
        #[serde(skip)]
        data: Vec<u8>,
        format: String,
        mime: String,
        size: usize,
        width: u32,
        height: u32,
    },

    /// Full-resolution encode result for download.
    EncodeFullResult {
        #[serde(skip)]
        data: Vec<u8>,
        format: String,
        mime: String,
        size: usize,
        width: u32,
        height: u32,
    },

    // ─── Schema / presets (sent once at init) ───
    /// Filter node schema JSON (for building slider UI).
    Schema { json: String },

    /// Film preset list JSON.
    PresetList { json: String },

    /// Preset thumbnail rendered.
    PresetThumbnail {
        id: String,
        name: String,
        #[serde(skip)]
        data: Vec<u8>,
        width: u32,
        height: u32,
    },

    /// Preset thumbnail render failed.
    PresetThumbnailError { id: String, error: String },

    // ─── Geometry ───
    /// Geometry state changed.
    GeometryChanged,

    // ─── Recipes ───
    /// Recipe saved successfully. JSON is the serialized recipe.
    RecipeSaved { json: String },

    /// Recipe loaded and applied.
    RecipeLoaded,

    // ─── Errors ───
    /// An error occurred. If `recoverable`, the editor auto-reverts to
    /// last safe state.
    Error {
        message: String,
        recoverable: bool,
    },

    // ─── Status ───
    /// A render started (overview + detail).
    RenderStarted,

    /// A render completed.
    RenderComplete { elapsed_ms: f64 },

    /// The editor needs a render (dirty flag is set).
    /// The host should call `render_if_needed()` after debouncing.
    RenderNeeded,
}
