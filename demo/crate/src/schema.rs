//! Schema export for UI generation.
//!
//! Filters the full zenpipe node registry down to filter-role nodes
//! and exports their JSON schema for slider autogeneration.

/// Export filter node schemas as a JSON string.
///
/// Returns a JSON object with `$defs` containing only filter-role nodes
/// from zenfilters. Each node has properties with full slider metadata:
/// `minimum`, `maximum`, `default`, `x-zennode-step`, `x-zennode-identity`,
/// `x-zennode-slider`, `x-zennode-section`, `x-zennode-unit`.
pub fn export_filter_schema() -> String {
    let registry = zenpipe::full_registry();
    let full = zennode::json_schema::registry_to_json_schema(&registry);

    // Filter down to zenfilters.* filter nodes only
    let defs = match full.get("$defs") {
        Some(serde_json::Value::Object(map)) => map,
        _ => return "{}".into(),
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

    let output = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": filtered,
    });

    serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_schema_has_exposure() {
        let json: serde_json::Value = serde_json::from_str(&export_filter_schema()).unwrap();
        let defs = json.get("$defs").unwrap().as_object().unwrap();
        assert!(defs.contains_key("zenfilters.exposure"));
    }

    #[test]
    fn filter_schema_excludes_non_filters() {
        let json: serde_json::Value = serde_json::from_str(&export_filter_schema()).unwrap();
        let defs = json.get("$defs").unwrap().as_object().unwrap();
        // Should not contain geometry or codec nodes
        assert!(!defs.contains_key("zenresize.constrain"));
        assert!(!defs.contains_key("zenjpeg.encode"));
    }

    #[test]
    fn exposure_has_slider_metadata() {
        let json: serde_json::Value = serde_json::from_str(&export_filter_schema()).unwrap();
        let exp = &json["$defs"]["zenfilters.exposure"]["properties"]["stops"];
        assert!(exp.get("minimum").is_some());
        assert!(exp.get("maximum").is_some());
        assert!(exp.get("default").is_some());
        assert!(exp.get("x-zennode-step").is_some());
        assert!(exp.get("x-zennode-slider").is_some());
    }

    #[test]
    fn filter_schema_has_groups() {
        let json: serde_json::Value = serde_json::from_str(&export_filter_schema()).unwrap();
        let exp = &json["$defs"]["zenfilters.exposure"];
        assert_eq!(exp["x-zennode-group"].as_str(), Some("tone"));
    }

    #[test]
    fn all_filter_nodes_have_properties() {
        let json: serde_json::Value = serde_json::from_str(&export_filter_schema()).unwrap();
        let defs = json["$defs"].as_object().unwrap();
        for (name, node) in defs {
            assert!(
                node.get("properties").is_some(),
                "{name} missing properties"
            );
        }
    }
}
