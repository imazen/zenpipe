//! Recipe model — serializable pipeline state for save/load/batch.
//!
//! A recipe captures the complete edit: geometry + adjustments + film preset
//! + export settings. It can be applied to any image.
//!
//! CLI spec §6: `--save-recipe sunset.json`, `--recipe sunset.json`
//! Demo SPEC §12: procedural edit system, per-image persistence.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::adjustment::ParamValue;
use super::export::{ColorspaceTarget, ExportModel, HdrMode, MetadataPolicy};
use super::geometry::GeometryModel;

/// A serializable edit recipe — all operations needed to reproduce an edit.
///
/// Independent of the source image. Geometry uses normalized coordinates
/// where applicable so recipes work at any resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    /// Recipe format version (for forward compat).
    #[serde(default = "default_version")]
    pub version: u32,
    /// Optional user-given name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    // ─── Geometry ───
    /// Geometry edits (crop, rotate, flip, orient, pad).
    #[serde(default)]
    pub geometry: GeometryModel,

    // ─── Adjustments ───
    /// Flat key→value adjustment map (e.g. "zenfilters.exposure.stops" → 1.5).
    #[serde(default)]
    pub adjustments: BTreeMap<String, ParamValue>,

    /// Film look preset ID (e.g. "portra").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub film_preset: Option<String>,

    /// Film preset intensity (0.0..1.0).
    #[serde(default = "default_intensity")]
    pub film_preset_intensity: f32,

    // ─── Export (optional — recipes can omit export to inherit from context) ───
    /// Output format (e.g. "jpeg", "webp"). None = inherit from context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// Output quality (0-100). None = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<f32>,

    /// HDR handling mode. None = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr_mode: Option<HdrMode>,

    /// Metadata preservation policy. None = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_policy: Option<MetadataPolicy>,

    /// Output color space. None = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colorspace: Option<ColorspaceTarget>,
}

fn default_version() -> u32 {
    1
}

fn default_intensity() -> f32 {
    1.0
}

impl Default for Recipe {
    fn default() -> Self {
        Self {
            version: 1,
            name: None,
            geometry: GeometryModel::default(),
            adjustments: BTreeMap::new(),
            film_preset: None,
            film_preset_intensity: 1.0,
            format: None,
            quality: None,
            hdr_mode: None,
            metadata_policy: None,
            colorspace: None,
        }
    }
}

impl Recipe {
    /// Serialize to compact JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// Serialize to pretty JSON.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("invalid recipe JSON: {e}"))
    }

    /// Whether the recipe has any non-default adjustments.
    pub fn has_adjustments(&self) -> bool {
        !self.adjustments.is_empty() || self.film_preset.is_some()
    }

    /// Whether the recipe has any geometry edits.
    pub fn has_geometry(&self) -> bool {
        !self.geometry.is_identity()
    }

    /// Apply recipe's export settings onto an ExportModel, overriding
    /// only fields that are set in the recipe (non-None).
    pub fn apply_export(&self, export: &mut ExportModel) {
        if let Some(ref fmt) = self.format {
            export.format = fmt.clone();
        }
        if let Some(q) = self.quality {
            export.set_option("quality", serde_json::Value::from(q));
        }
        if let Some(hdr) = self.hdr_mode {
            export.hdr_mode = hdr;
        }
        if let Some(ref mp) = self.metadata_policy {
            export.metadata_policy = mp.clone();
        }
        if let Some(cs) = self.colorspace {
            export.colorspace = cs;
        }
    }
}

/// Build a Recipe from the current editor state.
///
/// This is the "snapshot" direction: editor state → recipe.
/// The reverse direction (recipe → editor state) is handled by
/// `EditorState::apply_recipe()`.
pub fn snapshot_recipe(
    geometry: &GeometryModel,
    adjustments: &super::AdjustmentModel,
    export: &ExportModel,
    name: Option<String>,
) -> Recipe {
    // Snapshot the raw flat values (not the pipeline format).
    let raw_values = adjustments.raw_values().clone();
    Recipe {
        version: 1,
        name,
        geometry: geometry.clone(),
        adjustments: raw_values,
        film_preset: adjustments.film_preset.clone(),
        film_preset_intensity: adjustments.film_preset_intensity,
        format: Some(export.format.clone()),
        quality: export.quality(),
        hdr_mode: Some(export.hdr_mode),
        metadata_policy: Some(export.metadata_policy.clone()),
        colorspace: Some(export.colorspace),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_json_round_trip() {
        let mut r = Recipe::default();
        r.name = Some("sunset".into());
        r.adjustments
            .insert("zenfilters.exposure.stops".into(), ParamValue::Number(0.5));
        r.film_preset = Some("portra".into());
        r.film_preset_intensity = 0.8;

        let json = r.to_json_pretty();
        let r2 = Recipe::from_json(&json).unwrap();
        assert_eq!(r2.name.as_deref(), Some("sunset"));
        assert_eq!(r2.film_preset.as_deref(), Some("portra"));
        assert!((r2.film_preset_intensity - 0.8).abs() < 1e-6);
    }

    #[test]
    fn empty_recipe_is_default() {
        let r = Recipe::from_json("{}").unwrap();
        assert_eq!(r.version, 1);
        assert!(!r.has_adjustments());
        assert!(!r.has_geometry());
    }

    #[test]
    fn apply_export_overrides() {
        let mut r = Recipe::default();
        r.format = Some("jxl".into());
        r.quality = Some(90.0);
        r.hdr_mode = Some(HdrMode::Tonemap);

        let mut export = ExportModel::default();
        assert_eq!(export.format, "jpeg");

        r.apply_export(&mut export);
        assert_eq!(export.format, "jxl");
        assert_eq!(export.hdr_mode, HdrMode::Tonemap);
    }
}
