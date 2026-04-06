//! The complete editor state machine.
//!
//! Owns all models, Sessions, and rendering logic. Provides two entry points:
//!
//! - [`dispatch()`] — synchronous command processing (< 0.1ms), updates state,
//!   returns immediate [`ViewUpdate`]s. Never renders pixels.
//!
//! - [`render_if_needed()`] — runs the pipeline when dirty, returns pixel data.
//!   Called by the host on its own schedule (after debouncing).
//!
//! For direct use (testing, native apps), public methods on each model are
//! also available.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use zenpipe::Source;
use zenpipe::format::RGBA8_SRGB;
use zenpipe::orchestrate::ProcessConfig;
use zenpipe::session::Session;
use zenpipe::sources::MaterializedSource;

use crate::command::Command;
#[cfg(feature = "encode")]
use crate::encode::{EncodeResult, encode_pixels};
use crate::model::{
    AdjustmentModel, ExportModel, HistoryModel, RegionModel, SchemaModel,
};
use crate::pipeline::{RenderOutput, make_source_info, pack_rgba};
use crate::view_update::ViewUpdate;

/// AtomicBool-backed cancellation token for `enough::Stop`.
struct AtomicStop(Arc<AtomicBool>);

impl enough::Stop for AtomicStop {
    fn check(&self) -> Result<(), enough::StopReason> {
        if self.0.load(Ordering::Relaxed) {
            Err(enough::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Cached rendered pixels for re-encoding with different codec settings.
struct PreEncodeCache {
    key: u64,
    data: Vec<u8>,
    width: u32,
    height: u32,
}

/// The complete editor state — owns all models, Sessions, and controllers.
///
/// One instance per editing session. The view layer interacts with it via
/// [`dispatch()`] (for state changes) and [`render_if_needed()`] (for pixels).
pub struct EditorState {
    // ─── Source ───
    source_pixels: Option<MaterializedSource>,
    source_width: u32,
    source_height: u32,
    source_hash: u64,

    // ─── Models ───
    pub adjustments: AdjustmentModel,
    pub geometry: crate::model::GeometryModel,
    pub region: RegionModel,
    pub export: ExportModel,
    pub history: HistoryModel,
    pub schema: SchemaModel,

    // ─── Metadata ───
    metadata: Option<zencodec::Metadata>,
    source_format: Option<zencodec::ImageFormat>,

    // ─── Pipeline ───
    overview_session: Session,
    detail_session: Session,
    overview_max: u32,
    detail_max: u32,

    // ─── Cancellation ───
    overview_cancel: Arc<AtomicBool>,
    detail_cancel: Arc<AtomicBool>,

    // ─── Caches ───
    pre_encode_cache: Option<PreEncodeCache>,

    // ─── Dirty flags ───
    render_needed: bool,
    detail_only: bool,

    // ─── Last safe state (for error recovery) ───
    last_safe: Option<BTreeMap<String, serde_json::Value>>,
}

impl EditorState {
    /// Create a new editor with the given view dimensions.
    ///
    /// `overview_max` controls the max dimension of the overview output.
    /// `detail_max` controls the max dimension of the detail output.
    ///
    /// Call [`init_from_rgba()`] or dispatch `InitFromRgba` to load an image.
    #[cfg(feature = "std")]
    pub fn new(overview_max: u32, detail_max: u32) -> Self {
        let schema = SchemaModel::from_registry();
        let mut adjustments = AdjustmentModel::default();
        adjustments.init_from_schema(&schema);

        Self {
            source_pixels: None,
            source_width: 0,
            source_height: 0,
            source_hash: 0,
            adjustments,
            geometry: crate::model::GeometryModel::default(),
            region: RegionModel::default(),
            export: ExportModel::default(),
            history: HistoryModel::default(),
            schema,
            metadata: None,
            source_format: None,
            overview_session: Session::new(128 * 1024 * 1024),
            detail_session: Session::new(128 * 1024 * 1024),
            overview_max,
            detail_max,
            overview_cancel: Arc::new(AtomicBool::new(false)),
            detail_cancel: Arc::new(AtomicBool::new(false)),
            pre_encode_cache: None,
            render_needed: false,
            detail_only: false,
            last_safe: None,
        }
    }

    /// Initialize from pre-decoded RGBA8 sRGB pixels.
    pub fn init_from_rgba(&mut self, pixels: Vec<u8>, width: u32, height: u32) {
        self.source_pixels =
            Some(MaterializedSource::from_data(pixels, width, height, RGBA8_SRGB));
        self.source_width = width;
        self.source_height = height;
        self.source_hash = compute_hash(width, height, None);
        self.region.set_source_dims(width, height);
        self.metadata = None;
        self.source_format = None;
        self.pre_encode_cache = None;
        self.render_needed = true;
        self.detail_only = false;
    }

    /// Upgrade the source with natively-decoded pixels + metadata.
    pub fn upgrade_source(
        &mut self,
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        metadata: zencodec::Metadata,
        format: zencodec::ImageFormat,
    ) {
        self.source_pixels =
            Some(MaterializedSource::from_data(pixels, width, height, RGBA8_SRGB));
        self.source_width = width;
        self.source_height = height;
        self.metadata = Some(metadata);
        self.source_format = Some(format);
        self.pre_encode_cache = None;
        self.source_hash = compute_hash(width, height, Some(format));
        self.region.set_source_dims(width, height);
        self.render_needed = true;
        self.detail_only = false;
    }

    /// Initialize from raw image bytes — decode natively via zencodecs.
    ///
    /// This is the authoritative decode path. Returns metadata info on success.
    /// The frontend may have already called `init_from_rgba()` with a fast
    /// browser-decoded preview; this replaces those pixels with the correct
    /// high-quality decode and preserves metadata.
    #[cfg(feature = "decode")]
    pub fn init_from_bytes(&mut self, bytes: &[u8]) -> Result<crate::decode::NativeDecodeOutput, String> {
        let decoded = crate::decode::decode_native(bytes)?;
        let output_info = crate::decode::NativeDecodeOutput {
            data: Vec::new(), // don't clone the pixels for the return value
            width: decoded.width,
            height: decoded.height,
            metadata: decoded.metadata.clone(),
            format: decoded.format,
            has_gain_map: decoded.has_gain_map,
        };
        self.upgrade_source(
            decoded.data,
            decoded.width,
            decoded.height,
            decoded.metadata,
            decoded.format,
        );
        Ok(output_info)
    }

    // ─── Accessors ───

    pub fn source_width(&self) -> u32 {
        self.source_width
    }

    pub fn source_height(&self) -> u32 {
        self.source_height
    }

    pub fn metadata(&self) -> Option<&zencodec::Metadata> {
        self.metadata.as_ref()
    }

    pub fn source_format(&self) -> Option<zencodec::ImageFormat> {
        self.source_format
    }

    pub fn has_source(&self) -> bool {
        self.source_pixels.is_some()
    }

    pub fn render_needed(&self) -> bool {
        self.render_needed
    }

    pub fn overview_cache_len(&self) -> usize {
        self.overview_session.cache_len()
    }

    pub fn detail_cache_len(&self) -> usize {
        self.detail_session.cache_len()
    }

    // ─── Command dispatch ───

    /// Process a UI command synchronously. Returns immediate view updates.
    ///
    /// **Never renders pixels.** If the command changes state that requires
    /// a render, sets `render_needed = true` and returns a `RenderNeeded`
    /// update. The host should then call [`render_if_needed()`] after debouncing.
    pub fn dispatch(&mut self, cmd: Command) -> Vec<ViewUpdate> {
        match cmd {
            Command::InitFromRgba {
                width,
                height,
                data,
            } => {
                self.init_from_rgba(data, width, height);
                vec![
                    ViewUpdate::SourceLoaded {
                        width,
                        height,
                    },
                    ViewUpdate::RenderNeeded,
                ]
            }

            Command::InitFromBytes { data } => {
                #[cfg(feature = "decode")]
                {
                    match self.init_from_bytes(&data) {
                        Ok(info) => {
                            let has_icc = info.metadata.icc_profile.is_some();
                            let has_exif = info.metadata.exif.is_some();
                            let has_xmp = info.metadata.xmp.is_some();
                            vec![
                                ViewUpdate::SourceLoaded {
                                    width: info.width,
                                    height: info.height,
                                },
                                ViewUpdate::MetadataUpgraded {
                                    format: info.format.extension().to_string(),
                                    has_icc,
                                    has_exif,
                                    has_xmp,
                                    has_gain_map: info.has_gain_map,
                                },
                                ViewUpdate::RenderNeeded,
                            ]
                        }
                        Err(e) => vec![ViewUpdate::Error {
                            message: e,
                            recoverable: false,
                        }],
                    }
                }
                #[cfg(not(feature = "decode"))]
                {
                    let _ = data;
                    vec![ViewUpdate::Error {
                        message: "decode feature not enabled".to_string(),
                        recoverable: false,
                    }]
                }
            }

            Command::UpgradeSource {
                width,
                height,
                data,
                metadata,
                format,
            } => {
                if let (Some(meta), Some(fmt)) = (metadata, format) {
                    let has_icc = meta.icc_profile.is_some();
                    let has_exif = meta.exif.is_some();
                    let has_xmp = meta.xmp.is_some();
                    let format_str = fmt.extension().to_string();
                    self.upgrade_source(data, width, height, meta, fmt);
                    vec![
                        ViewUpdate::MetadataUpgraded {
                            format: format_str,
                            has_icc,
                            has_exif,
                            has_xmp,
                            has_gain_map: false,
                        },
                        ViewUpdate::RenderNeeded,
                    ]
                } else {
                    vec![]
                }
            }

            Command::SetParam { key, value } => {
                let changed = self.adjustments.set(&key, value);
                if changed {
                    self.render_needed = true;
                    self.detail_only = false;
                    vec![
                        ViewUpdate::ParamChanged {
                            key,
                            value,
                        },
                        ViewUpdate::RenderNeeded,
                    ]
                } else {
                    vec![]
                }
            }

            Command::SetParamBool { key, value } => {
                let changed = self.adjustments.set_bool(&key, value);
                if changed {
                    self.render_needed = true;
                    self.detail_only = false;
                    vec![
                        ViewUpdate::ParamBoolChanged { key, value },
                        ViewUpdate::RenderNeeded,
                    ]
                } else {
                    vec![]
                }
            }

            Command::SetFilmPreset { id, intensity } => {
                self.adjustments.film_preset = id.clone();
                self.adjustments.film_preset_intensity = intensity;
                self.render_needed = true;
                self.detail_only = false;
                vec![
                    ViewUpdate::FilmPresetChanged { id, intensity },
                    ViewUpdate::RenderNeeded,
                ]
            }

            Command::ResetParam { key } => {
                self.adjustments.reset(&key, &self.schema);
                let value = self.adjustments.get(&key, &self.schema);
                self.render_needed = true;
                self.detail_only = false;
                vec![
                    ViewUpdate::ParamChanged { key, value },
                    ViewUpdate::RenderNeeded,
                ]
            }

            Command::ResetAll => {
                self.adjustments.reset_all(&self.schema);
                self.render_needed = true;
                self.detail_only = false;
                vec![ViewUpdate::AllParamsReset, ViewUpdate::RenderNeeded]
            }

            Command::SetRegion { x, y, w, h } => {
                self.region.set(x, y, w, h);
                self.render_needed = true;
                self.detail_only = true;
                vec![
                    ViewUpdate::RegionChanged {
                        x: self.region.x,
                        y: self.region.y,
                        w: self.region.w,
                        h: self.region.h,
                    },
                    ViewUpdate::RenderNeeded,
                ]
            }

            Command::DragStart => {
                // Snapshot for potential undo
                vec![]
            }

            Command::DragMove { dx_norm, dy_norm } => {
                self.region.pan(dx_norm, dy_norm);
                self.render_needed = true;
                self.detail_only = true;
                vec![
                    ViewUpdate::RegionChanged {
                        x: self.region.x,
                        y: self.region.y,
                        w: self.region.w,
                        h: self.region.h,
                    },
                    ViewUpdate::RenderNeeded,
                ]
            }

            Command::DragEnd => {
                // Commit the drag — nothing special to do
                vec![]
            }

            Command::Zoom {
                factor,
                center_x,
                center_y,
            } => {
                self.region.zoom(factor, center_x, center_y);
                self.render_needed = true;
                self.detail_only = true;
                vec![
                    ViewUpdate::RegionChanged {
                        x: self.region.x,
                        y: self.region.y,
                        w: self.region.w,
                        h: self.region.h,
                    },
                    ViewUpdate::RenderNeeded,
                ]
            }

            Command::ResetTo1to1 {
                viewport_w,
                viewport_h,
                dpr,
            } => {
                self.region.reset_to_1to1(viewport_w, viewport_h, dpr);
                self.render_needed = true;
                self.detail_only = true;
                vec![
                    ViewUpdate::RegionChanged {
                        x: self.region.x,
                        y: self.region.y,
                        w: self.region.w,
                        h: self.region.h,
                    },
                    ViewUpdate::RenderNeeded,
                ]
            }

            Command::Undo => {
                if let Some(snapshot) = self.history.undo() {
                    self.adjustments.restore(&snapshot);
                    self.render_needed = true;
                    self.detail_only = false;
                    vec![
                        ViewUpdate::AllParamsReset,
                        ViewUpdate::HistoryChanged {
                            can_undo: self.history.can_undo(),
                            can_redo: self.history.can_redo(),
                        },
                        ViewUpdate::RenderNeeded,
                    ]
                } else {
                    vec![]
                }
            }

            Command::Redo => {
                if let Some(snapshot) = self.history.redo() {
                    self.adjustments.restore(&snapshot);
                    self.render_needed = true;
                    self.detail_only = false;
                    vec![
                        ViewUpdate::AllParamsReset,
                        ViewUpdate::HistoryChanged {
                            can_undo: self.history.can_undo(),
                            can_redo: self.history.can_redo(),
                        },
                        ViewUpdate::RenderNeeded,
                    ]
                } else {
                    vec![]
                }
            }

            Command::SetExportFormat { format } => {
                self.export.format = format;
                vec![]
            }

            Command::SetExportDims { width, height } => {
                self.export.width = width;
                self.export.height = height;
                vec![]
            }

            Command::SetExportOption { key, value } => {
                self.export.set_option(&key, value);
                vec![]
            }

            Command::EncodePreview | Command::EncodeFull => {
                // Encoding is handled by dedicated methods, not dispatch.
                // The host calls encode_preview() or encode_full() directly.
                vec![]
            }

            Command::SetHdrMode { mode } => {
                self.export.hdr_mode = mode;
                vec![]
            }

            Command::SetMetadataPolicy { policy } => {
                self.export.metadata_policy = policy;
                vec![]
            }

            Command::SetColorspace { target } => {
                self.export.colorspace = target;
                vec![]
            }

            Command::SetCrop { crop } => {
                self.geometry.crop = crop;
                self.render_needed = true;
                self.detail_only = false;
                vec![ViewUpdate::GeometryChanged, ViewUpdate::RenderNeeded]
            }

            Command::SetRotation { rotation } => {
                self.geometry.rotation = rotation;
                self.render_needed = true;
                self.detail_only = false;
                vec![ViewUpdate::GeometryChanged, ViewUpdate::RenderNeeded]
            }

            Command::SetFlip {
                horizontal,
                vertical,
            } => {
                self.geometry.flip_h = horizontal;
                self.geometry.flip_v = vertical;
                self.render_needed = true;
                self.detail_only = false;
                vec![ViewUpdate::GeometryChanged, ViewUpdate::RenderNeeded]
            }

            Command::SetOrientation { mode } => {
                self.geometry.orientation = mode;
                self.render_needed = true;
                self.detail_only = false;
                vec![ViewUpdate::GeometryChanged, ViewUpdate::RenderNeeded]
            }

            Command::SetAspectRatio { ratio } => {
                self.geometry.aspect_ratio = ratio;
                vec![ViewUpdate::GeometryChanged]
            }

            Command::ShowOriginal => {
                // The host should display the pre-rendered original.
                // No render needed — the original is cached separately.
                vec![]
            }

            Command::ShowEdited => {
                // Return to showing edited image.
                vec![]
            }

            Command::AutoEnhance => {
                // TODO: apply auto_levels + auto_exposure + clarity + vibrance
                // at mild settings (§18.3). Requires auto-analysis nodes.
                vec![]
            }

            Command::CleanDocument => {
                // TODO: run deskew + perspective + auto-crop + auto-levels (§18.6).
                // Requires document analysis nodes from zenfilters.
                vec![]
            }

            Command::SaveRecipe { name } => {
                let recipe = self.save_recipe(name);
                vec![ViewUpdate::RecipeSaved {
                    json: recipe.to_json_pretty(),
                }]
            }

            Command::LoadRecipe { json } => {
                match crate::model::Recipe::from_json(&json) {
                    Ok(recipe) => {
                        self.apply_recipe(&recipe);
                        self.render_needed = true;
                        self.detail_only = false;
                        vec![
                            ViewUpdate::RecipeLoaded,
                            ViewUpdate::AllParamsReset,
                            ViewUpdate::GeometryChanged,
                            ViewUpdate::RenderNeeded,
                        ]
                    }
                    Err(e) => vec![ViewUpdate::Error {
                        message: e,
                        recoverable: false,
                    }],
                }
            }

            Command::GetSchema => {
                vec![ViewUpdate::Schema {
                    json: self.schema.schema_json().to_string(),
                }]
            }

            Command::GetPresetList => {
                #[cfg(feature = "std")]
                {
                    vec![ViewUpdate::PresetList {
                        json: Self::preset_list_json(),
                    }]
                }
                #[cfg(not(feature = "std"))]
                {
                    vec![ViewUpdate::PresetList {
                        json: "[]".to_string(),
                    }]
                }
            }

            Command::RenderPresetThumbnails { .. } => {
                // Handled by dedicated method render_preset_thumbnails()
                vec![]
            }
        }
    }

    // ─── Rendering ───

    /// Render overview and/or detail if the dirty flag is set.
    ///
    /// Returns `None` if nothing to render. Called by the host after debouncing.
    #[cfg(feature = "std")]
    pub fn render_if_needed(&mut self) -> Option<Vec<ViewUpdate>> {
        if !self.render_needed {
            return None;
        }
        self.render_needed = false;

        let start = std::time::Instant::now();
        let mut updates = vec![ViewUpdate::RenderStarted];

        if !self.detail_only {
            match self.render_overview_internal() {
                Ok(out) => updates.push(ViewUpdate::OverviewPixels {
                    data: out.data,
                    width: out.width,
                    height: out.height,
                }),
                Err(e) => updates.push(ViewUpdate::Error {
                    message: format!("Overview: {e}"),
                    recoverable: true,
                }),
            }
        }

        match self.render_detail_internal() {
            Ok(out) => updates.push(ViewUpdate::DetailPixels {
                data: out.data,
                width: out.width,
                height: out.height,
            }),
            Err(e) => updates.push(ViewUpdate::Error {
                message: format!("Detail: {e}"),
                recoverable: true,
            }),
        }

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        updates.push(ViewUpdate::RenderComplete { elapsed_ms });

        // Snapshot safe adjustments after successful render
        self.last_safe = Some(self.adjustments.to_pipeline_format(&self.schema));
        self.detail_only = false;

        Some(updates)
    }

    /// Render the overview (small resized image) with current adjustments.
    #[cfg(feature = "std")]
    pub fn render_overview(&mut self) -> Result<RenderOutput, String> {
        self.render_overview_internal()
    }

    /// Render the detail region with current adjustments.
    #[cfg(feature = "std")]
    pub fn render_detail(&mut self) -> Result<RenderOutput, String> {
        self.render_detail_internal()
    }

    /// Render at a specific max dimension (for export at arbitrary size).
    #[cfg(feature = "std")]
    pub fn render_at_size(&mut self, max_dim: u32) -> Result<RenderOutput, String> {
        let source = self.source_box()?;
        let adj = self.adjustments.to_pipeline_format(&self.schema);

        let mut nodes: Vec<Box<dyn zennode::NodeInstance>> = Vec::new();

        if max_dim > 0 && (self.source_width > max_dim || self.source_height > max_dim) {
            nodes.push(Box::new(zenpipe::zennode_defs::Constrain {
                w: Some(max_dim),
                h: Some(max_dim),
                mode: "within".into(),
                ..Default::default()
            }));
        }

        crate::pipeline::append_film_look(
            &mut nodes,
            &adj,
            self.adjustments.film_preset.as_deref(),
            self.adjustments.film_preset_intensity,
        );
        crate::pipeline::append_filter_nodes(&mut nodes, &adj);

        let info = make_source_info(self.source_width, self.source_height);
        let converters: &[&dyn zenpipe::bridge::NodeConverter] =
            &[&crate::pipeline::FILTERS_CONVERTER];
        let config = ProcessConfig {
            nodes: &nodes,
            converters,
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };

        self.overview_cancel.store(true, Ordering::Relaxed);
        let fresh = Arc::new(AtomicBool::new(false));
        self.overview_cancel = Arc::clone(&fresh);
        let stop = AtomicStop(fresh);

        let output = self
            .overview_session
            .stream_stoppable(source, &config, None, self.source_hash, &stop)
            .map_err(|e| format!("render: {e}"))?;

        let mat = MaterializedSource::from_source_stoppable(output.source, &stop)
            .map_err(|e| format!("materialize: {e}"))?;

        Ok(RenderOutput {
            width: mat.width(),
            height: mat.height(),
            data: pack_rgba(&mat),
        })
    }

    /// Render all film preset thumbnails.
    #[cfg(feature = "std")]
    pub fn render_preset_thumbnails(
        &mut self,
        thumb_size: u32,
    ) -> Vec<(String, String, Result<RenderOutput, String>)> {
        zenfilters::filters::FilmPreset::ALL
            .iter()
            .map(|preset| {
                let result = self.render_single_preset(preset.id(), thumb_size);
                (preset.id().to_string(), preset.name().to_string(), result)
            })
            .collect()
    }

    /// List all available film preset IDs and names.
    #[cfg(feature = "std")]
    pub fn list_presets() -> Vec<(String, String)> {
        zenfilters::filters::FilmPreset::ALL
            .iter()
            .map(|p| (p.id().to_string(), p.name().to_string()))
            .collect()
    }

    #[cfg(feature = "std")]
    fn preset_list_json() -> String {
        let presets = Self::list_presets();
        let entries: Vec<serde_json::Value> = presets
            .into_iter()
            .map(|(id, name)| serde_json::json!({"id": id, "name": name}))
            .collect();
        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into())
    }

    // ─── Encoding ───

    /// Encode at overview size for inline preview in the export modal.
    #[cfg(feature = "encode")]
    pub fn encode_preview(&mut self) -> Result<EncodeResult, String> {
        self.encode_at_overview_size(
            &self.export.format.clone(),
            &self.export.options.clone(),
        )
    }

    /// Encode at full resolution for download.
    #[cfg(feature = "encode")]
    pub fn encode_full(&mut self) -> Result<EncodeResult, String> {
        let max_dim = self.export.max_dim();
        let format = self.export.format.clone();
        let options = self.export.options.clone();
        self.encode_at_size(max_dim, &format, &options)
    }

    /// Encode at overview size with specific format and options.
    #[cfg(feature = "encode")]
    pub fn encode_at_overview_size(
        &mut self,
        format: &str,
        options: &serde_json::Value,
    ) -> Result<EncodeResult, String> {
        let adj = self.adjustments.to_pipeline_format(&self.schema);
        let key = pre_encode_key(
            &adj,
            self.adjustments.film_preset.as_deref(),
            self.overview_max,
        );

        if let Some(ref cache) = self.pre_encode_cache {
            if cache.key == key {
                return encode_pixels(
                    &cache.data,
                    cache.width,
                    cache.height,
                    format,
                    options,
                    self.metadata.as_ref(),
                );
            }
        }

        let rendered = self.render_overview_internal()?;
        self.pre_encode_cache = Some(PreEncodeCache {
            key,
            data: rendered.data.clone(),
            width: rendered.width,
            height: rendered.height,
        });
        encode_pixels(
            &rendered.data,
            rendered.width,
            rendered.height,
            format,
            options,
            self.metadata.as_ref(),
        )
    }

    /// Encode at requested size with specific format and options.
    #[cfg(feature = "encode")]
    pub fn encode_at_size(
        &mut self,
        max_dim: u32,
        format: &str,
        options: &serde_json::Value,
    ) -> Result<EncodeResult, String> {
        let adj = self.adjustments.to_pipeline_format(&self.schema);
        let key = pre_encode_key(&adj, self.adjustments.film_preset.as_deref(), max_dim);

        if let Some(ref cache) = self.pre_encode_cache {
            if cache.key == key {
                return encode_pixels(
                    &cache.data,
                    cache.width,
                    cache.height,
                    format,
                    options,
                    self.metadata.as_ref(),
                );
            }
        }

        let rendered = self.render_at_size(max_dim)?;
        self.pre_encode_cache = Some(PreEncodeCache {
            key,
            data: rendered.data.clone(),
            width: rendered.width,
            height: rendered.height,
        });
        encode_pixels(
            &rendered.data,
            rendered.width,
            rendered.height,
            format,
            options,
            self.metadata.as_ref(),
        )
    }

    // ─── Cancellation ───

    /// Cancel any in-progress overview render.
    pub fn cancel_overview(&self) {
        self.overview_cancel.store(true, Ordering::Relaxed);
    }

    /// Cancel any in-progress detail render.
    pub fn cancel_detail(&self) {
        self.detail_cancel.store(true, Ordering::Relaxed);
    }

    // ─── History ───

    /// Push the current adjustments onto the undo stack.
    /// Call this after a logical edit completes (e.g. slider release).
    pub fn push_history(&mut self) {
        self.history.push(self.adjustments.snapshot());
    }

    // ─── Recipe ───

    /// Save current state as a recipe.
    pub fn save_recipe(&self, name: Option<String>) -> crate::model::Recipe {
        crate::model::recipe::snapshot_recipe(
            &self.geometry,
            &self.adjustments,
            &self.export,
            name,
        )
    }

    /// Apply a recipe, overwriting current adjustments, geometry, and export.
    pub fn apply_recipe(&mut self, recipe: &crate::model::Recipe) {
        // Geometry
        self.geometry = recipe.geometry.clone();

        // Adjustments: set from recipe's flat map
        for (key, val) in &recipe.adjustments {
            match val {
                crate::model::adjustment::ParamValue::Number(n) => {
                    self.adjustments.set(key, *n);
                }
                crate::model::adjustment::ParamValue::Bool(b) => {
                    self.adjustments.set_bool(key, *b);
                }
            }
        }

        // Film preset
        self.adjustments.film_preset = recipe.film_preset.clone();
        self.adjustments.film_preset_intensity = recipe.film_preset_intensity;

        // Export (only override fields present in recipe)
        recipe.apply_export(&mut self.export);

        self.pre_encode_cache = None;
    }

    // ─── Internal rendering methods ───

    #[cfg(feature = "std")]
    fn render_overview_internal(&mut self) -> Result<RenderOutput, String> {
        let source = self.source_box()?;
        let adj = self.adjustments.to_pipeline_format(&self.schema);

        let mut nodes: Vec<Box<dyn zennode::NodeInstance>> = Vec::new();

        nodes.push(Box::new(zenpipe::zennode_defs::Constrain {
            w: Some(self.overview_max),
            h: Some(self.overview_max),
            mode: "within".into(),
            ..Default::default()
        }));

        crate::pipeline::append_film_look(
            &mut nodes,
            &adj,
            self.adjustments.film_preset.as_deref(),
            self.adjustments.film_preset_intensity,
        );
        crate::pipeline::append_filter_nodes(&mut nodes, &adj);

        let info = make_source_info(self.source_width, self.source_height);
        let converters: &[&dyn zenpipe::bridge::NodeConverter] =
            &[&crate::pipeline::FILTERS_CONVERTER];
        let config = ProcessConfig {
            nodes: &nodes,
            converters,
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };

        self.overview_cancel.store(true, Ordering::Relaxed);
        let fresh = Arc::new(AtomicBool::new(false));
        self.overview_cancel = Arc::clone(&fresh);
        let stop = AtomicStop(fresh);

        let output = self
            .overview_session
            .stream_stoppable(source, &config, None, self.source_hash, &stop)
            .map_err(|e| format!("overview render: {e}"))?;

        let mat = MaterializedSource::from_source_stoppable(output.source, &stop)
            .map_err(|e| format!("overview materialize: {e}"))?;

        Ok(RenderOutput {
            width: mat.width(),
            height: mat.height(),
            data: pack_rgba(&mat),
        })
    }

    #[cfg(feature = "std")]
    fn render_detail_internal(&mut self) -> Result<RenderOutput, String> {
        let source = self.source_box()?;
        let adj = self.adjustments.to_pipeline_format(&self.schema);

        let (crop_x, crop_y, crop_w, crop_h) = self.region.to_crop_pixels();

        let mut nodes: Vec<Box<dyn zennode::NodeInstance>> = Vec::new();

        nodes.push(Box::new(zenpipe::zennode_defs::Crop {
            x: crop_x,
            y: crop_y,
            w: crop_w,
            h: crop_h,
        }));

        if crop_w > self.detail_max || crop_h > self.detail_max {
            nodes.push(Box::new(zenpipe::zennode_defs::Constrain {
                w: Some(self.detail_max),
                h: Some(self.detail_max),
                mode: "within".into(),
                ..Default::default()
            }));
        }

        crate::pipeline::append_film_look(
            &mut nodes,
            &adj,
            self.adjustments.film_preset.as_deref(),
            self.adjustments.film_preset_intensity,
        );
        crate::pipeline::append_filter_nodes(&mut nodes, &adj);

        let info = make_source_info(self.source_width, self.source_height);
        let converters: &[&dyn zenpipe::bridge::NodeConverter] =
            &[&crate::pipeline::FILTERS_CONVERTER];
        let config = ProcessConfig {
            nodes: &nodes,
            converters,
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };

        let region_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.source_hash.hash(&mut h);
            crop_x.hash(&mut h);
            crop_y.hash(&mut h);
            crop_w.hash(&mut h);
            crop_h.hash(&mut h);
            h.finish()
        };

        self.detail_cancel.store(true, Ordering::Relaxed);
        let fresh = Arc::new(AtomicBool::new(false));
        self.detail_cancel = Arc::clone(&fresh);
        let stop = AtomicStop(fresh);

        let output = self
            .detail_session
            .stream_stoppable(source, &config, None, region_hash, &stop)
            .map_err(|e| format!("detail render: {e}"))?;

        let mat = MaterializedSource::from_source_stoppable(output.source, &stop)
            .map_err(|e| format!("detail materialize: {e}"))?;

        Ok(RenderOutput {
            width: mat.width(),
            height: mat.height(),
            data: pack_rgba(&mat),
        })
    }

    #[cfg(feature = "std")]
    fn render_single_preset(
        &mut self,
        preset_id: &str,
        thumb_size: u32,
    ) -> Result<RenderOutput, String> {
        let source = self.source_box()?;

        let mut nodes: Vec<Box<dyn zennode::NodeInstance>> = Vec::new();

        nodes.push(Box::new(zenpipe::zennode_defs::Constrain {
            w: Some(thumb_size),
            h: Some(thumb_size),
            mode: "within".into(),
            ..Default::default()
        }));

        nodes.push(Box::new(zenfilters::zennode_defs::FilmLook {
            preset: preset_id.to_string(),
            strength: 1.0,
        }));

        let info = make_source_info(self.source_width, self.source_height);
        let converters: &[&dyn zenpipe::bridge::NodeConverter] =
            &[&crate::pipeline::FILTERS_CONVERTER];
        let config = ProcessConfig {
            nodes: &nodes,
            converters,
            hdr_mode: "sdr_only",
            source_info: &info,
            trace_config: None,
        };

        let output = self
            .overview_session
            .stream(source, &config, None, self.source_hash)
            .map_err(|e| format!("preset {preset_id}: {e}"))?;

        let mat = MaterializedSource::from_source(output.source)
            .map_err(|e| format!("preset {preset_id} materialize: {e}"))?;

        Ok(RenderOutput {
            width: mat.width(),
            height: mat.height(),
            data: pack_rgba(&mat),
        })
    }

    fn source_box(&self) -> Result<Box<MaterializedSource>, String> {
        self.source_pixels
            .as_ref()
            .map(|s| Box::new(s.clone()))
            .ok_or_else(|| "No source image loaded".to_string())
    }
}

fn compute_hash(width: u32, height: u32, format: Option<zencodec::ImageFormat>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(f) = format {
        b"native".hash(&mut h);
        f.extension().hash(&mut h);
    }
    width.hash(&mut h);
    height.hash(&mut h);
    h.finish()
}

#[cfg(feature = "encode")]
fn pre_encode_key(
    adjustments: &BTreeMap<String, serde_json::Value>,
    film_preset: Option<&str>,
    max_dim: u32,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (k, v) in adjustments {
        k.hash(&mut h);
        v.to_string().hash(&mut h);
    }
    film_preset.hash(&mut h);
    max_dim.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rgba(w: u32, h: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for pixel in data.chunks_exact_mut(4) {
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
            pixel[3] = 255;
        }
        data
    }

    #[test]
    fn editor_init_and_render() {
        let pixels = solid_rgba(200, 150, 128, 128, 128);
        let mut state = EditorState::new(512, 800);
        state.init_from_rgba(pixels, 200, 150);
        assert_eq!(state.source_width(), 200);
        assert_eq!(state.source_height(), 150);
        assert!(state.has_source());

        let out = state.render_overview().unwrap();
        assert!(out.width <= 200);
        assert!(out.height <= 200);
        assert_eq!(out.data.len(), (out.width * out.height * 4) as usize);
    }

    #[test]
    fn dispatch_set_param_marks_dirty() {
        let pixels = solid_rgba(100, 100, 128, 128, 128);
        let mut state = EditorState::new(100, 100);
        state.init_from_rgba(pixels, 100, 100);

        // Clear the initial dirty flag
        let _ = state.render_if_needed();
        assert!(!state.render_needed());

        let updates = state.dispatch(Command::SetParam {
            key: "zenfilters.exposure.stops".into(),
            value: 1.0,
        });

        assert!(state.render_needed());
        assert!(updates.iter().any(|u| matches!(u, ViewUpdate::RenderNeeded)));
    }

    #[test]
    fn render_with_filter() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut state = EditorState::new(200, 400);
        state.init_from_rgba(pixels, 400, 300);

        state.adjustments.set("zenfilters.exposure.stops", 1.0);
        state.render_needed = true;

        let updates = state.render_if_needed().unwrap();
        assert!(updates
            .iter()
            .any(|u| matches!(u, ViewUpdate::OverviewPixels { .. })));
        assert!(updates
            .iter()
            .any(|u| matches!(u, ViewUpdate::DetailPixels { .. })));
    }

    #[test]
    fn render_overview_cache_hit() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut state = EditorState::new(200, 400);
        state.init_from_rgba(pixels, 400, 300);

        state.adjustments.set("zenfilters.exposure.stops", 0.5);
        let _out1 = state.render_overview().unwrap();
        assert_eq!(state.overview_session.cache_len(), 1);

        state.adjustments.set("zenfilters.exposure.stops", 1.0);
        let _out2 = state.render_overview().unwrap();
        assert_eq!(state.overview_session.cache_len(), 1);
    }

    #[test]
    fn render_detail_default_region() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut state = EditorState::new(200, 400);
        state.init_from_rgba(pixels, 400, 300);

        let out = state.render_detail().unwrap();
        assert!(out.width > 0);
        assert!(out.height > 0);
    }

    #[test]
    fn cancel_then_render_succeeds() {
        let pixels = solid_rgba(400, 300, 128, 128, 128);
        let mut state = EditorState::new(200, 400);
        state.init_from_rgba(pixels, 400, 300);

        state.adjustments.set("zenfilters.exposure.stops", 0.5);
        state.cancel_overview();

        let result = state.render_overview();
        assert!(result.is_ok(), "render after cancel should succeed");
    }

    #[test]
    fn dispatch_reset_all() {
        let mut state = EditorState::new(100, 100);
        state.init_from_rgba(solid_rgba(100, 100, 128, 128, 128), 100, 100);
        state.adjustments.set("zenfilters.exposure.stops", 1.5);

        let updates = state.dispatch(Command::ResetAll);
        assert!(updates
            .iter()
            .any(|u| matches!(u, ViewUpdate::AllParamsReset)));
    }

    #[test]
    fn dispatch_set_region() {
        let mut state = EditorState::new(100, 100);
        state.init_from_rgba(solid_rgba(100, 100, 128, 128, 128), 100, 100);

        let updates = state.dispatch(Command::SetRegion {
            x: 0.1,
            y: 0.2,
            w: 0.3,
            h: 0.4,
        });

        assert!(updates
            .iter()
            .any(|u| matches!(u, ViewUpdate::RegionChanged { .. })));
        assert!((state.region.x - 0.1).abs() < 1e-6);
    }

    #[test]
    fn undo_redo() {
        let mut state = EditorState::new(100, 100);
        state.init_from_rgba(solid_rgba(100, 100, 128, 128, 128), 100, 100);

        // Push initial state, then modify and push again
        state.push_history(); // snapshot 0: initial
        state.adjustments.set("zenfilters.exposure.stops", 1.0);
        state.push_history(); // snapshot 1: with exposure

        // Undo should revert to snapshot 0
        let updates = state.dispatch(Command::Undo);
        assert!(updates.iter().any(|u| matches!(
            u,
            ViewUpdate::HistoryChanged {
                can_undo: false,
                can_redo: true,
            }
        )));

        // Redo should restore snapshot 1
        let updates = state.dispatch(Command::Redo);
        assert!(updates.iter().any(|u| matches!(
            u,
            ViewUpdate::HistoryChanged {
                can_undo: true,
                can_redo: false,
            }
        )));
    }

    #[cfg(feature = "encode")]
    #[test]
    fn encode_jpeg() {
        let mut state = EditorState::new(100, 100);
        state.init_from_rgba(solid_rgba(64, 48, 128, 64, 200), 64, 48);

        state.export.format = "jpeg".to_string();
        state.export.options = serde_json::json!({"quality": 85});

        let result = state.encode_preview().unwrap();
        assert_eq!(&result.data[..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn recipe_save_load_round_trip() {
        let mut state = EditorState::new(100, 100);
        state.init_from_rgba(solid_rgba(100, 100, 128, 128, 128), 100, 100);

        // Set some adjustments
        state.adjustments.set("zenfilters.exposure.stops", 1.5);
        state.adjustments.film_preset = Some("portra".into());
        state.adjustments.film_preset_intensity = 0.8;
        state.geometry.flip_h = true;
        state.export.hdr_mode = crate::model::export::HdrMode::Tonemap;

        // Save recipe
        let updates = state.dispatch(Command::SaveRecipe {
            name: Some("test".into()),
        });
        let json = match &updates[0] {
            ViewUpdate::RecipeSaved { json } => json.clone(),
            other => panic!("expected RecipeSaved, got {other:?}"),
        };

        // Reset everything
        state.dispatch(Command::ResetAll);
        state.geometry = crate::model::GeometryModel::default();
        state.export.hdr_mode = crate::model::export::HdrMode::Preserve;

        // Load recipe
        let updates = state.dispatch(Command::LoadRecipe { json });
        assert!(updates.iter().any(|u| matches!(u, ViewUpdate::RecipeLoaded)));

        // Verify state was restored
        assert!(state.geometry.flip_h);
        assert_eq!(state.adjustments.film_preset.as_deref(), Some("portra"));
        assert_eq!(state.export.hdr_mode, crate::model::export::HdrMode::Tonemap);
    }

    #[test]
    fn dispatch_geometry_commands() {
        let mut state = EditorState::new(100, 100);
        state.init_from_rgba(solid_rgba(100, 100, 128, 128, 128), 100, 100);

        let updates = state.dispatch(Command::SetCrop {
            crop: crate::model::geometry::CropMode::Percent { x: 0.1, y: 0.1, w: 0.8, h: 0.8 },
        });
        assert!(updates.iter().any(|u| matches!(u, ViewUpdate::GeometryChanged)));
        assert!(state.render_needed());

        let updates = state.dispatch(Command::SetFlip {
            horizontal: true,
            vertical: false,
        });
        assert!(updates.iter().any(|u| matches!(u, ViewUpdate::GeometryChanged)));
        assert!(state.geometry.flip_h);
        assert!(!state.geometry.flip_v);
    }
}
