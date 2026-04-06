//! Commands from the UI to the editor.
//!
//! The view layer translates user interactions (slider drags, button taps,
//! gesture events) into `Command` values. The editor processes each command
//! synchronously (< 0.1 ms) and returns [`ViewUpdate`]s.
//!
//! Pixel-producing work (rendering, encoding) is NOT triggered by commands.
//! Instead, commands mark the editor as needing a render, and the host calls
//! [`EditorState::render_if_needed()`] on its own schedule (e.g. after a
//! debounce timer in a web worker, or on a dedicated thread in a native app).

use serde::{Deserialize, Serialize};

/// A command from the view layer to the editor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    // ─── Source ───
    /// Initialize from pre-decoded RGBA8 sRGB pixels (e.g. browser decode).
    InitFromRgba {
        width: u32,
        height: u32,
        /// RGBA8 pixel data, `width * height * 4` bytes.
        /// When sent via JSON, this is base64-encoded. For direct calls,
        /// use [`EditorState::init_from_rgba()`] instead.
        #[serde(skip)]
        data: Vec<u8>,
    },

    /// Initialize from raw image bytes — zeneditor decodes natively.
    ///
    /// This is the authoritative path. The frontend may optionally call
    /// `InitFromRgba` first for a fast browser-decoded preview, then
    /// `InitFromBytes` to replace it with the high-quality native decode.
    InitFromBytes {
        #[serde(skip)]
        data: Vec<u8>,
    },

    /// Upgrade the source with natively-decoded pixels + metadata.
    /// Called after background native decode completes.
    UpgradeSource {
        width: u32,
        height: u32,
        #[serde(skip)]
        data: Vec<u8>,
        #[serde(skip)]
        metadata: Option<zencodec::Metadata>,
        #[serde(skip)]
        format: Option<zencodec::ImageFormat>,
    },

    // ─── Adjustments ───
    /// Set a filter parameter value.
    SetParam { key: String, value: f64 },

    /// Set a boolean filter parameter.
    SetParamBool { key: String, value: bool },

    /// Set the film look preset and intensity.
    SetFilmPreset {
        id: Option<String>,
        intensity: f32,
    },

    /// Reset a single parameter to its identity value.
    ResetParam { key: String },

    /// Reset all parameters to identity.
    ResetAll,

    // ─── Region / Navigation ───
    /// Set the detail view region directly (normalized 0..1 coordinates).
    SetRegion { x: f32, y: f32, w: f32, h: f32 },

    /// Begin a drag gesture on the detail view.
    DragStart,

    /// Continue a drag gesture. Deltas are normalized to source image size.
    DragMove { dx_norm: f32, dy_norm: f32 },

    /// End a drag gesture.
    DragEnd,

    /// Zoom by a factor around a normalized center point.
    Zoom {
        factor: f32,
        center_x: f32,
        center_y: f32,
    },

    /// Reset to 1:1 pixel ratio (one source pixel = one device pixel).
    ResetTo1to1 {
        viewport_w: f32,
        viewport_h: f32,
        dpr: f32,
    },

    // ─── History ───
    /// Undo the last edit.
    Undo,

    /// Redo the last undone edit.
    Redo,

    // ─── Export ───
    /// Set the export format (e.g. "jpeg", "webp", "png").
    SetExportFormat { format: String },

    /// Set export dimensions.
    SetExportDims { width: u32, height: u32 },

    /// Set a format-specific export option (e.g. quality, effort, lossless).
    SetExportOption { key: String, value: serde_json::Value },

    /// Request an encode at overview size (for inline preview in export modal).
    EncodePreview,

    /// Request a full-resolution encode for download.
    EncodeFull,

    /// Set HDR handling mode.
    SetHdrMode {
        mode: crate::model::export::HdrMode,
    },

    /// Set metadata preservation policy.
    SetMetadataPolicy {
        policy: crate::model::export::MetadataPolicy,
    },

    /// Set output color space.
    SetColorspace {
        target: crate::model::export::ColorspaceTarget,
    },

    // ─── Geometry ───
    /// Set the crop mode.
    SetCrop {
        crop: crate::model::geometry::CropMode,
    },

    /// Set rotation.
    SetRotation {
        rotation: crate::model::geometry::RotationMode,
    },

    /// Set flip state.
    SetFlip { horizontal: bool, vertical: bool },

    /// Set EXIF orientation handling.
    SetOrientation {
        mode: crate::model::geometry::OrientMode,
    },

    /// Lock crop aspect ratio (§11.2: 1:1, 4:3, 16:9, free).
    SetAspectRatio {
        ratio: Option<crate::model::geometry::AspectRatio>,
    },

    // ─── Compare (§5.2.1) ───
    /// Show original (unedited) image for comparison (tap-hold).
    ShowOriginal,

    /// Return to showing edited image.
    ShowEdited,

    // ─── Compound operations (§18.3, §18.6) ───
    /// One-click auto enhance (auto_levels + auto_exposure + clarity + vibrance).
    AutoEnhance,

    /// Document cleanup pipeline (deskew + perspective + crop + auto-levels).
    CleanDocument,

    // ─── Recipe (§12) ───
    /// Save current state as a recipe. Returns ViewUpdate::RecipeSaved.
    SaveRecipe { name: Option<String> },

    /// Load a recipe from JSON and apply it.
    LoadRecipe { json: String },

    // ─── Schema / Presets ───
    /// Request the filter node schema (sent once at init).
    GetSchema,

    /// Request the list of film presets.
    GetPresetList,

    /// Request preset thumbnail renders.
    RenderPresetThumbnails { thumb_size: u32 },
}
