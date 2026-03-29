//! Schema export for downstream consumers (imageflow-dotnet, etc.).
//!
//! Provides functions to generate all schema artifacts from the full node registry:
//! - Node schemas (JSON Schema / OpenAPI)
//! - Querystring key registry (structured metadata)
//! - Querystring JSON Schema (for validation)
//!
//! These are the single source of truth for client SDK code generation.

extern crate std;
use serde_json::Value;

/// All schema artifacts exported from the full node registry.
#[derive(Debug)]
pub struct ExportedSchemas {
    /// JSON Schema 2020-12 with `$defs` for every registered node.
    pub node_schemas: Value,
    /// OpenAPI 3.1 `components/schemas` section.
    pub openapi_schemas: Value,
    /// Querystring JSON Schema for validating RIAPI querystrings.
    pub querystring_schema: Value,
    /// Structured querystring key registry grouped by node.
    pub querystring_keys: Value,
}

/// Export all schema artifacts from the full node registry.
///
/// This is the main entry point for schema generation. Call this once
/// and serialize the results to files for downstream consumption.
pub fn export_all() -> ExportedSchemas {
    let registry = crate::full_registry();

    ExportedSchemas {
        node_schemas: zennode::json_schema::registry_to_json_schema(&registry),
        openapi_schemas: zennode::json_schema::registry_to_openapi_schemas(&registry),
        querystring_schema: zennode::json_schema::querystring_to_json_schema(&registry),
        querystring_keys: zennode::json_schema::querystring_key_registry(&registry),
    }
}

/// Export node schemas as JSON Schema 2020-12.
pub fn export_node_schemas() -> Value {
    let registry = crate::full_registry();
    zennode::json_schema::registry_to_json_schema(&registry)
}

/// Export node schemas as OpenAPI 3.1 components/schemas.
pub fn export_openapi_schemas() -> Value {
    let registry = crate::full_registry();
    zennode::json_schema::registry_to_openapi_schemas(&registry)
}

/// Export querystring JSON Schema for RIAPI validation.
pub fn export_querystring_schema() -> Value {
    let registry = crate::full_registry();
    zennode::json_schema::querystring_to_json_schema(&registry)
}

/// Export structured querystring key registry.
pub fn export_querystring_keys() -> Value {
    let registry = crate::full_registry();
    zennode::json_schema::querystring_key_registry(&registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_all_produces_non_empty() {
        let schemas = export_all();
        assert!(schemas.node_schemas.get("$defs").is_some());
        assert!(schemas.openapi_schemas.get("schemas").is_some());
        assert!(schemas.querystring_schema.get("properties").is_some());
        assert!(schemas.querystring_keys.get("nodes").is_some());
    }

    #[test]
    fn node_schemas_include_constrain() {
        let schemas = export_node_schemas();
        let defs = schemas.get("$defs").unwrap();
        assert!(
            defs.get("zenresize.constrain").is_some(),
            "should include zenresize.constrain"
        );
    }

    #[test]
    fn openapi_schemas_normalize_dots() {
        let schemas = export_openapi_schemas();
        let s = schemas.get("schemas").unwrap();
        // Dots replaced with underscores for OpenAPI
        assert!(s.get("zenresize_constrain").is_some());
    }

    fn str_val(v: &Value, key: &str) -> Option<String> {
        v.get(key)?.as_str().map(|s| s.to_string())
    }

    #[test]
    fn querystring_schema_has_w_key() {
        let qs = export_querystring_schema();
        let props = qs.get("properties").unwrap();
        let w = props.get("w").expect("should have 'w' key");
        assert_eq!(str_val(w, "type").as_deref(), Some("integer"));
        assert_eq!(
            str_val(w, "x-zennode-node").as_deref(),
            Some("zenresize.constrain")
        );
    }

    #[test]
    fn querystring_schema_has_format_key() {
        let qs = export_querystring_schema();
        let props = qs.get("properties").unwrap();
        let f = props.get("format").expect("should have 'format' key");
        assert_eq!(
            str_val(f, "x-zennode-node").as_deref(),
            Some("zencodecs.quality_intent")
        );
    }

    #[test]
    fn querystring_schema_thumbnail_is_alias() {
        let qs = export_querystring_schema();
        let props = qs.get("properties").unwrap();
        let t = props.get("thumbnail").expect("should have 'thumbnail' key");
        assert_eq!(str_val(t, "x-zennode-alias-of").as_deref(), Some("format"));
    }

    #[test]
    fn querystring_keys_grouped_by_node() {
        let keys = export_querystring_keys();
        let nodes = keys.get("nodes").unwrap().as_object().unwrap();
        assert!(nodes.contains_key("zenresize.constrain"));
        assert!(nodes.contains_key("zencodecs.quality_intent"));

        let constrain = nodes.get("zenresize.constrain").unwrap();
        assert!(constrain.get("label").is_some());
        let ks = constrain.get("keys").unwrap().as_array().unwrap();
        assert!(!ks.is_empty());
    }

    #[test]
    fn querystring_keys_include_aliases() {
        let keys = export_querystring_keys();
        let nodes = keys.get("nodes").unwrap();
        let constrain = nodes.get("zenresize.constrain").unwrap();
        let ks = constrain.get("keys").unwrap().as_array().unwrap();

        // Find the 'w' key entry
        let w_entry = ks
            .iter()
            .find(|k| str_val(k, "key").as_deref() == Some("w"))
            .expect("should have 'w' key");
        let aliases = w_entry.get("aliases").map(|a| a.as_array()).flatten();
        assert!(aliases.is_some(), "w should have aliases (width, maxwidth)");
    }

    #[test]
    fn export_all_schemas_are_valid_json() {
        let schemas = export_all();
        // Verify round-trip through serde
        let node_json = serde_json::to_string(&schemas.node_schemas).unwrap();
        assert!(node_json.len() > 100);
        let qs_json = serde_json::to_string(&schemas.querystring_schema).unwrap();
        assert!(qs_json.len() > 100);
    }
}
