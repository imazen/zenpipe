//! Editor model layer — pure state, no rendering, no platform dependencies.
//!
//! Each model owns a slice of editor state and provides methods to
//! query and mutate it. Models emit no events — the [`EditorState`]
//! mediates between commands, models, and view updates.

pub mod adjustment;
pub mod export;
pub mod geometry;
pub mod history;
pub mod recipe;
pub mod region;
pub mod schema;

pub use adjustment::AdjustmentModel;
pub use export::ExportModel;
pub use geometry::GeometryModel;
pub use history::HistoryModel;
pub use recipe::Recipe;
pub use region::RegionModel;
pub use schema::SchemaModel;
