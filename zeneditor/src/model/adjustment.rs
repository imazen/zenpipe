//! Adjustment model — all filter parameter values and film preset state.
//!
//! Mirrors the flat `state.adjustments` map from the JS side. Keys are
//! dot-separated paths like `"zenfilters.exposure.stops"`. Values are f64
//! (numbers) or bool (toggles).
//!
//! The model builds the nested format needed by the pipeline:
//! `{"zenfilters.exposure": {"stops": 1.5}, ...}` via [`Self::to_pipeline_format()`].

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::schema::SchemaModel;

/// All filter adjustment values and film preset state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdjustmentModel {
    /// Flat key → value map. Keys are `"node_id.param_name"`.
    values: BTreeMap<String, ParamValue>,
    /// Which keys the user has touched (differs from identity).
    touched: BTreeSet<String>,
    /// Active film look preset ID (e.g. "portra"), or None.
    pub film_preset: Option<String>,
    /// Film preset intensity (0.0 to 1.0).
    pub film_preset_intensity: f32,
}

/// A parameter value — either a number or a boolean.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Number(f64),
    Bool(bool),
}

impl ParamValue {
    pub fn as_f64(self) -> f64 {
        match self {
            Self::Number(n) => n,
            Self::Bool(b) => {
                if b {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    pub fn as_bool(self) -> bool {
        match self {
            Self::Bool(b) => b,
            Self::Number(n) => n != 0.0,
        }
    }
}

impl Default for AdjustmentModel {
    fn default() -> Self {
        Self {
            values: BTreeMap::new(),
            touched: BTreeSet::new(),
            film_preset: None,
            film_preset_intensity: 1.0,
        }
    }
}

impl AdjustmentModel {
    /// Set a numeric parameter. Returns true if the value changed.
    pub fn set(&mut self, key: &str, value: f64) -> bool {
        let old = self.values.get(key).map(|v| v.as_f64());
        if old == Some(value) {
            return false;
        }
        self.values.insert(key.to_string(), ParamValue::Number(value));
        self.touched.insert(key.to_string());
        true
    }

    /// Set a boolean parameter. Returns true if the value changed.
    pub fn set_bool(&mut self, key: &str, value: bool) -> bool {
        let old = self.values.get(key).map(|v| v.as_bool());
        if old == Some(value) {
            return false;
        }
        self.values.insert(key.to_string(), ParamValue::Bool(value));
        self.touched.insert(key.to_string());
        true
    }

    /// Get a numeric parameter value, or the identity from the schema.
    pub fn get(&self, key: &str, schema: &SchemaModel) -> f64 {
        self.values
            .get(key)
            .map(|v| v.as_f64())
            .unwrap_or_else(|| schema.identity_for(key))
    }

    /// Get a boolean parameter value.
    pub fn get_bool(&self, key: &str) -> bool {
        self.values.get(key).is_some_and(|v| v.as_bool())
    }

    /// Reset a single parameter to its identity value.
    pub fn reset(&mut self, key: &str, schema: &SchemaModel) {
        let identity = schema.identity_for(key);
        self.values
            .insert(key.to_string(), ParamValue::Number(identity));
        self.touched.remove(key);
    }

    /// Reset all parameters to identity.
    pub fn reset_all(&mut self, schema: &SchemaModel) {
        for desc in schema.all_params() {
            self.values.insert(
                desc.adjust_key.clone(),
                ParamValue::Number(desc.identity),
            );
        }
        self.touched.clear();
        self.film_preset = None;
        self.film_preset_intensity = 1.0;
    }

    /// Initialize all values from the schema's identity values.
    pub fn init_from_schema(&mut self, schema: &SchemaModel) {
        for desc in schema.all_params() {
            self.values
                .entry(desc.adjust_key.clone())
                .or_insert(ParamValue::Number(desc.identity));
        }
    }

    /// Whether any parameter differs from identity.
    pub fn has_changes(&self, schema: &SchemaModel) -> bool {
        for desc in schema.all_params() {
            let val = self.values.get(&desc.adjust_key).map(|v| v.as_f64());
            if let Some(v) = val {
                if (v - desc.identity).abs() > 1e-6 {
                    return true;
                }
            }
        }
        self.film_preset.is_some()
    }

    /// Build the nested adjustment format needed by the zenpipe pipeline.
    ///
    /// Output: `{"zenfilters.exposure": {"stops": 1.5}, ...}`
    /// Only includes nodes where at least one param differs from identity.
    pub fn to_pipeline_format(
        &self,
        schema: &SchemaModel,
    ) -> BTreeMap<String, serde_json::Value> {
        schema.build_pipeline_adjustments(&self.values)
    }

    /// Access the raw flat key→value map (for recipe serialization).
    pub fn raw_values(&self) -> &BTreeMap<String, ParamValue> {
        &self.values
    }

    /// Take a snapshot of current state for undo/redo.
    pub fn snapshot(&self) -> AdjustmentSnapshot {
        AdjustmentSnapshot {
            values: self.values.clone(),
            touched: self.touched.clone(),
            film_preset: self.film_preset.clone(),
            film_preset_intensity: self.film_preset_intensity,
        }
    }

    /// Restore from a snapshot.
    pub fn restore(&mut self, snapshot: &AdjustmentSnapshot) {
        self.values = snapshot.values.clone();
        self.touched = snapshot.touched.clone();
        self.film_preset = snapshot.film_preset.clone();
        self.film_preset_intensity = snapshot.film_preset_intensity;
    }
}

/// Serializable snapshot for undo/redo history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdjustmentSnapshot {
    values: BTreeMap<String, ParamValue>,
    touched: BTreeSet<String>,
    film_preset: Option<String>,
    film_preset_intensity: f32,
}
