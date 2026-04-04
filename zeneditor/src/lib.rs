//! Platform-agnostic image editor — model, controller, and Command/ViewUpdate protocol.
//!
//! `zeneditor` owns all editor state and logic:
//! - **Models**: adjustments, region, export, history, schema
//! - **Pipeline**: rendering via zenpipe Sessions, filter conversion
//! - **Encoding**: RGBA8 → compressed formats via zencodecs
//! - **Protocol**: `Command`/`ViewUpdate` enums for cross-boundary IPC
//!
//! The crate is UI-agnostic. Platform layers (WASM, Tauri, native) provide:
//! - Image decoding (browser decode, zencodecs, platform APIs)
//! - DOM/UI rendering (canvas, SwiftUI, Compose)
//! - Gesture translation (pointer events → Commands)
//! - File I/O (File API, native FS)
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │  zeneditor (this crate)                  │
//! │                                          │
//! │  EditorState                             │
//! │    ├── AdjustmentModel (filter values)   │
//! │    ├── RegionModel (crop/zoom)           │
//! │    ├── ExportModel (format/quality)      │
//! │    ├── HistoryModel (undo/redo)          │
//! │    ├── SchemaModel (node descriptors)    │
//! │    ├── Session × 2 (overview + detail)   │
//! │    │                                     │
//! │    ├── dispatch(Command) → [ViewUpdate]  │
//! │    └── render_if_needed() → [ViewUpdate] │
//! └──────────────────┬───────────────────────┘
//!                    │
//! ┌──────────────────▼───────────────────────┐
//! │  Platform layer (WASM / Tauri / native)  │
//! │  - Decode, UI, gestures, file I/O        │
//! └──────────────────────────────────────────┘
//! ```

pub mod command;
pub mod decode;
pub mod encode;
pub mod model;
pub mod pipeline;
pub mod state;
pub mod view_update;

pub use command::Command;
pub use decode::{DecodedImage, NativeDecodeOutput};
pub use encode::EncodeResult;
pub use model::{
    AdjustmentModel, ExportModel, GeometryModel, HistoryModel, Recipe, RegionModel, SchemaModel,
};
pub use pipeline::RenderOutput;
pub use state::EditorState;
pub use view_update::ViewUpdate;
