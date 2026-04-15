//! Generate a single-file Markdown reference for the full RIAPI pipeline.
//!
//! Produces `/tmp/zen-riapi-reference.md` (or path given as first arg) containing
//! every registered zennode, every RIAPI querystring key, parameters, defaults,
//! and aliases — one source of truth for the RIAPI surface.
//!
//! Run: `cargo run --example riapi_reference --features "json-schema,zennode,nodes-filters" -- /tmp/out.md`
//!
//! The generator reads from the live node registry so the document stays in
//! sync with whatever features are enabled. For full coverage, enable the
//! broadest feature set that compiles (typically all `nodes-*` features).

use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/zen-riapi-reference.md".to_string());

    let schemas = zenpipe::schema_export::export_all();
    let mut out = Vec::new();

    emit_header(&mut out)?;
    emit_toc(&mut out, &schemas.querystring_keys)?;
    emit_all_keys_index(&mut out, &schemas.querystring_keys)?;
    emit_node_sections(&mut out, &schemas.querystring_keys, &schemas.node_schemas)?;
    emit_footer(&mut out, &schemas)?;

    fs::write(&output_path, &out)?;
    eprintln!(
        "Wrote {} bytes to {}",
        out.len(),
        output_path
    );
    Ok(())
}

fn emit_header(out: &mut Vec<u8>) -> std::io::Result<()> {
    writeln!(out, "# zenpipe RIAPI Pipeline Reference\n")?;
    writeln!(
        out,
        "Generated from the live zennode registry. \
         Every RIAPI querystring key, parameter, default, and alias \
         registered in the build is documented below.\n"
    )?;
    writeln!(
        out,
        "**Regenerate with:** \
         `cargo run --example riapi_reference --features \"json-schema,zennode,nodes-all\"`\n"
    )?;
    writeln!(out, "---\n")?;
    Ok(())
}

fn emit_toc(out: &mut Vec<u8>, qs_keys: &Value) -> std::io::Result<()> {
    writeln!(out, "## Contents\n")?;
    writeln!(out, "- [Querystring keys index](#querystring-keys-index) — flat list of every key")?;
    writeln!(out, "- [Nodes](#nodes) — detailed reference per node")?;

    let nodes = qs_keys.get("nodes").and_then(|n| n.as_object());
    if let Some(nodes) = nodes {
        for (node_id, node) in nodes {
            let label = node
                .get("label")
                .and_then(|l| l.as_str())
                .unwrap_or(node_id);
            let anchor = anchor_for(node_id);
            writeln!(out, "  - [`{}` — {}](#{})", node_id, label, anchor)?;
        }
    }
    writeln!(out)?;
    writeln!(out, "---\n")?;
    Ok(())
}

fn emit_all_keys_index(out: &mut Vec<u8>, qs_keys: &Value) -> std::io::Result<()> {
    writeln!(out, "## Querystring keys index\n")?;
    writeln!(
        out,
        "| Key | Aliases | Node | Param | Type |"
    )?;
    writeln!(out, "|---|---|---|---|---|")?;

    // Flatten: one row per (key, alias) pair, sorted.
    let mut rows: BTreeMap<String, String> = BTreeMap::new();

    if let Some(nodes) = qs_keys.get("nodes").and_then(|n| n.as_object()) {
        for (node_id, node) in nodes {
            let keys = match node.get("keys").and_then(|k| k.as_array()) {
                Some(k) => k,
                None => continue,
            };
            for key_entry in keys {
                let key = key_entry
                    .get("key")
                    .and_then(|k| k.as_str())
                    .unwrap_or("?");
                let param = key_entry
                    .get("param")
                    .and_then(|p| p.as_str())
                    .unwrap_or("");
                let ty = key_entry
                    .get("value_schema")
                    .and_then(|s| s.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let aliases = key_entry
                    .get("aliases")
                    .and_then(|a| a.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .map(|s| format!("`{}`", s))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();

                let row = format!(
                    "| `{}` | {} | [`{}`](#{}) | `{}` | {} |",
                    key,
                    if aliases.is_empty() { "—".into() } else { aliases },
                    node_id,
                    anchor_for(node_id),
                    param,
                    ty
                );
                rows.insert(key.to_string(), row);
            }
        }
    }

    for (_, row) in rows {
        writeln!(out, "{}", row)?;
    }
    writeln!(out)?;
    writeln!(out, "---\n")?;
    Ok(())
}

fn emit_node_sections(
    out: &mut Vec<u8>,
    qs_keys: &Value,
    node_schemas: &Value,
) -> std::io::Result<()> {
    writeln!(out, "## Nodes\n")?;

    let nodes = match qs_keys.get("nodes").and_then(|n| n.as_object()) {
        Some(n) => n,
        None => return Ok(()),
    };

    let defs = node_schemas.get("$defs").and_then(|d| d.as_object());

    for (node_id, node) in nodes {
        let label = node
            .get("label")
            .and_then(|l| l.as_str())
            .unwrap_or(node_id);
        writeln!(out, "### `{}` — {}\n", node_id, label)?;

        // Pull description from the JSON schema if available.
        if let Some(defs) = defs {
            if let Some(schema) = defs.get(node_id) {
                if let Some(desc) = schema.get("description").and_then(|d| d.as_str()) {
                    writeln!(out, "{}\n", desc)?;
                }
            }
        }

        let keys = match node.get("keys").and_then(|k| k.as_array()) {
            Some(k) if !k.is_empty() => k,
            _ => {
                writeln!(out, "_No RIAPI keys registered for this node._\n")?;
                continue;
            }
        };

        writeln!(out, "| Key | Aliases | Param | Type | Default | Description |")?;
        writeln!(out, "|---|---|---|---|---|---|")?;
        for key_entry in keys {
            let key = key_entry.get("key").and_then(|k| k.as_str()).unwrap_or("?");
            let param = key_entry
                .get("param")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let aliases = key_entry
                .get("aliases")
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| format!("`{}`", s))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let schema = key_entry.get("value_schema");
            let ty = schema
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let default = schema
                .and_then(|s| s.get("default"))
                .map(|d| format!("`{}`", serde_json::to_string(d).unwrap_or_default()))
                .unwrap_or_else(|| "—".into());
            let desc = key_entry
                .get("label")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let extra_desc = key_entry
                .get("description")
                .and_then(|d| d.as_str())
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();

            writeln!(
                out,
                "| `{}` | {} | `{}` | {} | {} | {}{} |",
                key,
                if aliases.is_empty() {
                    "—".into()
                } else {
                    aliases
                },
                param,
                ty,
                default,
                desc,
                extra_desc
            )?;
        }
        writeln!(out)?;

        // Emit enum value lists if present in the schema.
        if let Some(defs) = defs {
            if let Some(schema) = defs.get(node_id) {
                emit_enum_values(out, schema)?;
            }
        }

        writeln!(out, "---\n")?;
    }
    Ok(())
}

fn emit_enum_values(out: &mut Vec<u8>, schema: &Value) -> std::io::Result<()> {
    // Walk properties looking for enum fields; emit a bullet list per field.
    let props = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Ok(()),
    };

    let mut emitted_header = false;
    for (prop_name, prop_schema) in props {
        let enum_values = prop_schema.get("enum").and_then(|e| e.as_array());
        if let Some(values) = enum_values {
            if !emitted_header {
                writeln!(out, "**Enum values:**\n")?;
                emitted_header = true;
            }
            let values: Vec<String> = values
                .iter()
                .filter_map(|v| v.as_str().map(|s| format!("`{}`", s)))
                .collect();
            writeln!(out, "- `{}`: {}", prop_name, values.join(", "))?;
        }
    }
    if emitted_header {
        writeln!(out)?;
    }
    Ok(())
}

fn emit_footer(out: &mut Vec<u8>, schemas: &zenpipe::schema_export::ExportedSchemas) -> std::io::Result<()> {
    let num_nodes = schemas
        .querystring_keys
        .get("nodes")
        .and_then(|n| n.as_object())
        .map(|o| o.len())
        .unwrap_or(0);
    let num_keys = schemas
        .querystring_keys
        .get("nodes")
        .and_then(|n| n.as_object())
        .map(|o| {
            o.values()
                .filter_map(|v| v.get("keys").and_then(|k| k.as_array()))
                .map(|arr| arr.len())
                .sum::<usize>()
        })
        .unwrap_or(0);

    writeln!(out, "## Statistics\n")?;
    writeln!(out, "- **Registered nodes:** {}", num_nodes)?;
    writeln!(out, "- **Registered RIAPI keys:** {}", num_keys)?;
    writeln!(
        out,
        "- **Generated at:** build time ({})",
        env!("CARGO_PKG_NAME")
    )?;
    Ok(())
}

fn anchor_for(node_id: &str) -> String {
    // GitHub-flavored markdown anchor: drop backticks, lowercase, dots removed.
    node_id.replace('.', "").to_lowercase()
}
