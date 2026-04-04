//! Schema model — parsed node registry for filter parameter metadata.
//!
//! Parses the zennode registry into typed descriptors so the editor knows
//! identity values, ranges, groups, and slider types for each parameter.
//! Also builds the pipeline adjustment format from flat key→value maps.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::adjustment::ParamValue;

/// Descriptor for a single filter parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamDescriptor {
    /// Full adjust key: `"zenfilters.exposure.stops"`.
    pub adjust_key: String,
    /// Node ID: `"zenfilters.exposure"`.
    pub node_id: String,
    /// Parameter name within the node: `"stops"`.
    pub param_name: String,
    /// Parameter kind.
    pub kind: ParamKind,
    /// Minimum value.
    pub min: f64,
    /// Maximum value.
    pub max: f64,
    /// Default value.
    pub default: f64,
    /// Identity value (no-op value — filter has no effect).
    pub identity: f64,
    /// Step size for sliders.
    pub step: f64,
    /// Slider mapping type.
    pub slider: SliderType,
    /// Display unit (e.g. "EV", "x").
    pub unit: Option<String>,
    /// Section within the node (e.g. "Main", "Advanced").
    pub section: Option<String>,
    /// Group name (e.g. "tone", "color", "detail").
    pub group: Option<String>,
    /// For array parameters: the parent array param name.
    pub array_param: Option<String>,
    /// For array parameters: index within the array.
    pub array_index: Option<usize>,
    /// For array parameters: total array size.
    pub array_size: Option<usize>,
    /// Display label for array elements.
    pub label: Option<String>,
}

/// The kind of parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Number,
    Boolean,
    ArrayElement,
}

/// Slider mapping type — how the slider position maps to the parameter value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SliderType {
    #[default]
    Linear,
    SquareFromSlider,
    FactorCentered,
}

/// Descriptor for a filter node (group of parameters).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDescriptor {
    /// Node ID: `"zenfilters.exposure"`.
    pub id: String,
    /// Display title: `"Exposure"`.
    pub title: String,
    /// Group: `"tone"`, `"color"`, `"detail"`, etc.
    pub group: String,
    /// Node role: `"filter"`.
    pub role: String,
    /// Parameters in display order.
    pub params: Vec<ParamDescriptor>,
}

/// Parsed schema from the zennode registry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchemaModel {
    /// All filter nodes in display order.
    pub nodes: Vec<NodeDescriptor>,
    /// Raw JSON schema (for sending to the view layer).
    #[serde(skip)]
    pub raw_json: String,
}

impl SchemaModel {
    /// Parse the schema from the zenpipe node registry.
    #[cfg(feature = "std")]
    pub fn from_registry() -> Self {
        let registry = zenpipe::full_registry();
        let full = zennode::json_schema::registry_to_json_schema(&registry);
        let raw_json =
            serde_json::to_string_pretty(&Self::filter_schema(&full)).unwrap_or_default();
        let nodes = Self::parse_nodes(&full);
        Self { nodes, raw_json }
    }

    /// Filter the full schema down to zenfilters filter nodes only.
    #[cfg(feature = "std")]
    fn filter_schema(full: &serde_json::Value) -> serde_json::Value {
        let defs = match full.get("$defs") {
            Some(serde_json::Value::Object(map)) => map,
            _ => return serde_json::json!({}),
        };

        let mut filtered = serde_json::Map::new();
        for (key, val) in defs {
            if !key.starts_with("zenfilters.") {
                continue;
            }
            if val.get("x-zennode-role").and_then(|v| v.as_str()) != Some("filter") {
                continue;
            }
            filtered.insert(key.clone(), val.clone());
        }

        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": filtered,
        })
    }

    /// Parse node descriptors from the full JSON schema.
    #[cfg(feature = "std")]
    fn parse_nodes(full: &serde_json::Value) -> Vec<NodeDescriptor> {
        let defs = match full.get("$defs") {
            Some(serde_json::Value::Object(map)) => map,
            _ => return Vec::new(),
        };

        let mut nodes = Vec::new();
        for (node_id, node_def) in defs {
            if !node_id.starts_with("zenfilters.") {
                continue;
            }
            if node_def.get("x-zennode-role").and_then(|v| v.as_str()) != Some("filter") {
                continue;
            }

            let title = node_def
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(node_id)
                .to_string();
            let group = node_def
                .get("x-zennode-group")
                .and_then(|v| v.as_str())
                .unwrap_or("other")
                .to_string();
            let role = node_def
                .get("x-zennode-role")
                .and_then(|v| v.as_str())
                .unwrap_or("filter")
                .to_string();

            let props = match node_def.get("properties").and_then(|p| p.as_object()) {
                Some(p) => p,
                None => continue,
            };

            let mut params = Vec::new();
            for (param_name, param_schema) in props {
                let param_type = param_schema.get("type").and_then(|t| t.as_str());

                match param_type {
                    Some("number") | Some("integer") => {
                        let desc = parse_numeric_param(node_id, param_name, param_schema, &group);
                        params.push(desc);
                    }
                    Some("boolean") => {
                        params.push(ParamDescriptor {
                            adjust_key: format!("{node_id}.{param_name}"),
                            node_id: node_id.clone(),
                            param_name: param_name.clone(),
                            kind: ParamKind::Boolean,
                            min: 0.0,
                            max: 1.0,
                            default: 0.0,
                            identity: param_schema
                                .get("x-zennode-identity")
                                .and_then(|v| {
                                    if v.as_bool() == Some(true) {
                                        Some(1.0)
                                    } else {
                                        Some(0.0)
                                    }
                                })
                                .unwrap_or(0.0),
                            step: 1.0,
                            slider: SliderType::Linear,
                            unit: None,
                            section: param_schema
                                .get("x-zennode-section")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                            group: Some(group.clone()),
                            array_param: None,
                            array_index: None,
                            array_size: None,
                            label: None,
                        });
                    }
                    Some("array") => {
                        // Parse array parameters into individual elements
                        if let Some(items) = param_schema.get("items") {
                            let min_items = param_schema
                                .get("minItems")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as usize;
                            let max_items = param_schema
                                .get("maxItems")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(min_items as u64)
                                as usize;
                            let size = min_items.max(max_items);

                            let labels: Vec<String> = param_schema
                                .get("x-zennode-labels")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(String::from))
                                        .collect()
                                })
                                .unwrap_or_default();

                            for i in 0..size {
                                let label = labels.get(i).cloned();
                                let desc = parse_numeric_param(
                                    node_id,
                                    &format!("{param_name}[{i}]"),
                                    items,
                                    &group,
                                );
                                params.push(ParamDescriptor {
                                    adjust_key: format!("{node_id}.{param_name}[{i}]"),
                                    param_name: param_name.clone(),
                                    kind: ParamKind::ArrayElement,
                                    array_param: Some(param_name.clone()),
                                    array_index: Some(i),
                                    array_size: Some(size),
                                    label,
                                    ..desc
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }

            if !params.is_empty() {
                nodes.push(NodeDescriptor {
                    id: node_id.clone(),
                    title,
                    group,
                    role,
                    params,
                });
            }
        }

        nodes
    }

    /// Get the identity value for a parameter key.
    pub fn identity_for(&self, key: &str) -> f64 {
        for node in &self.nodes {
            for p in &node.params {
                if p.adjust_key == key {
                    return p.identity;
                }
            }
        }
        0.0
    }

    /// Get all parameter descriptors (flattened from all nodes).
    pub fn all_params(&self) -> impl Iterator<Item = &ParamDescriptor> {
        self.nodes.iter().flat_map(|n| n.params.iter())
    }

    /// Build the nested pipeline adjustment format from a flat key→value map.
    ///
    /// Input:  `{"zenfilters.exposure.stops": 1.5, ...}`
    /// Output: `{"zenfilters.exposure": {"stops": 1.5}, ...}`
    ///
    /// Only includes nodes where at least one param differs from identity.
    pub fn build_pipeline_adjustments(
        &self,
        values: &BTreeMap<String, ParamValue>,
    ) -> BTreeMap<String, serde_json::Value> {
        let mut adj: BTreeMap<String, serde_json::Value> = BTreeMap::new();

        for node in &self.nodes {
            let mut node_params = serde_json::Map::new();
            let mut arrays: BTreeMap<String, ArrayAccumulator> = BTreeMap::new();
            let mut any_changed = false;

            for p in &node.params {
                let val = values
                    .get(&p.adjust_key)
                    .copied()
                    .unwrap_or(ParamValue::Number(p.identity));

                match p.kind {
                    ParamKind::ArrayElement => {
                        let arr_name = p.array_param.as_deref().unwrap_or(&p.param_name);
                        let acc = arrays.entry(arr_name.to_string()).or_insert_with(|| {
                            ArrayAccumulator {
                                values: vec![0.0; p.array_size.unwrap_or(0)],
                                identities: vec![0.0; p.array_size.unwrap_or(0)],
                                any_changed: false,
                            }
                        });
                        if let Some(idx) = p.array_index {
                            if idx < acc.values.len() {
                                acc.values[idx] = val.as_f64();
                                acc.identities[idx] = p.identity;
                                if (val.as_f64() - p.identity).abs() > 1e-6 {
                                    acc.any_changed = true;
                                }
                            }
                        }
                    }
                    ParamKind::Boolean => {
                        let b = val.as_bool();
                        node_params.insert(p.param_name.clone(), serde_json::Value::Bool(b));
                        if b != (p.identity != 0.0) {
                            any_changed = true;
                        }
                    }
                    ParamKind::Number => {
                        let n = val.as_f64();
                        node_params.insert(
                            p.param_name.clone(),
                            serde_json::Value::from(n),
                        );
                        if (n - p.identity).abs() > 1e-6 {
                            any_changed = true;
                        }
                    }
                }
            }

            // Assemble arrays
            for (arr_name, acc) in &arrays {
                let arr_val: Vec<serde_json::Value> =
                    acc.values.iter().map(|v| serde_json::Value::from(*v)).collect();
                node_params.insert(arr_name.clone(), serde_json::Value::Array(arr_val));
                if acc.any_changed {
                    any_changed = true;
                }
            }

            if any_changed {
                adj.insert(node.id.clone(), serde_json::Value::Object(node_params));
            }
        }

        adj
    }

    /// Export the raw filter schema JSON for the view layer.
    pub fn schema_json(&self) -> &str {
        &self.raw_json
    }
}

struct ArrayAccumulator {
    values: Vec<f64>,
    identities: Vec<f64>,
    any_changed: bool,
}

/// Parse a numeric param descriptor from JSON schema.
fn parse_numeric_param(
    node_id: &str,
    param_name: &str,
    schema: &serde_json::Value,
    group: &str,
) -> ParamDescriptor {
    let min = schema.get("minimum").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let max = schema
        .get("maximum")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    let default = schema
        .get("default")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let identity = schema
        .get("x-zennode-identity")
        .and_then(|v| v.as_f64())
        .unwrap_or(default);
    let step = schema
        .get("x-zennode-step")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.01);

    let slider = match schema
        .get("x-zennode-slider")
        .and_then(|v| v.as_str())
    {
        Some("square_from_slider") => SliderType::SquareFromSlider,
        Some("factor_centered") => SliderType::FactorCentered,
        _ => SliderType::Linear,
    };

    ParamDescriptor {
        adjust_key: format!("{node_id}.{param_name}"),
        node_id: node_id.to_string(),
        param_name: param_name.to_string(),
        kind: ParamKind::Number,
        min,
        max,
        default,
        identity,
        step,
        slider,
        unit: schema
            .get("x-zennode-unit")
            .and_then(|v| v.as_str())
            .map(String::from),
        section: schema
            .get("x-zennode-section")
            .and_then(|v| v.as_str())
            .map(String::from),
        group: Some(group.to_string()),
        array_param: None,
        array_index: None,
        array_size: None,
        label: None,
    }
}
