#!/usr/bin/env python3
"""Generate documentation site from zennode schema JSON files.

Reads the exported node schemas and querystring key registry,
produces a GitHub Pages-compatible site with:
- Per-node reference pages with parameter tables and RIAPI keys
- Mermaid pipeline diagrams
- Querystring key reference
- Format/codec capability table

Usage:
    # Export schemas from Rust first:
    # cargo test --features json-schema -- schema_export::tests::dump_to_files

    # Then generate docs:
    python3 tools/docgen/generate_docs.py schemas/ docs/
"""

import json
import os
import sys
from pathlib import Path


def load_json(path: str) -> dict:
    with open(path) as f:
        return json.load(f)


def pascal_to_title(s: str) -> str:
    """zenresize.constrain -> Constrain"""
    return s.split(".")[-1].replace("_", " ").title()


def node_id_to_slug(node_id: str) -> str:
    """zenresize.constrain -> zenresize-constrain"""
    return node_id.replace(".", "-")


def role_emoji(role: str) -> str:
    return {
        "Decode": "📥",
        "Encode": "📤",
        "Filter": "🎨",
        "Geometry": "📐",
        "Orient": "🔄",
        "Resize": "↔️",
        "Composite": "🔲",
        "Canvas": "🖼️",
        "Analysis": "🔍",
        "Meta": "ℹ️",
    }.get(role, "⚙️")


def group_emoji(group: str) -> str:
    return {
        "Tone": "☀️",
        "Color": "🎨",
        "Detail": "🔍",
        "Effects": "✨",
        "Geometry": "📐",
        "Layout": "📏",
        "Encode": "📤",
        "Decode": "📥",
        "Composite": "🔲",
        "Canvas": "🖼️",
        "Quantize": "🎯",
        "Analysis": "📊",
        "Hdr": "🌅",
    }.get(group, "⚙️")


def type_label(prop: dict) -> str:
    """Extract human-readable type from JSON Schema property."""
    t = prop.get("type", "string")
    if isinstance(t, list):
        t = [x for x in t if x != "null"][0] if len(t) > 1 else t[0]

    if t == "number":
        parts = []
        if "minimum" in prop:
            parts.append(f"{prop['minimum']}")
        if "maximum" in prop:
            parts.append(f"{prop['maximum']}")
        if parts:
            return f"float ({' – '.join(parts)})"
        return "float"
    elif t == "integer":
        parts = []
        if "minimum" in prop:
            parts.append(f"{prop['minimum']}")
        if "maximum" in prop:
            parts.append(f"{prop['maximum']}")
        if parts:
            return f"int ({' – '.join(parts)})"
        return "int"
    elif t == "boolean":
        return "bool"
    elif t == "string":
        if "enum" in prop:
            return "enum"
        return "string"
    elif t == "array":
        return "array"
    return str(t)


def generate_node_page(node_id: str, schema: dict) -> str:
    """Generate Markdown documentation for a single node."""
    label = schema.get("title", pascal_to_title(node_id))
    desc = schema.get("description", "")
    role = schema.get("x-zennode-role", "")
    group = schema.get("x-zennode-group", "")
    tags = schema.get("x-zennode-tags", [])
    inputs = schema.get("x-zennode-inputs", [])
    props = schema.get("properties", {})

    lines = []
    lines.append(f"# {role_emoji(role)} {label}")
    lines.append("")
    lines.append(f"> **ID:** `{node_id}` · **Role:** {role} · **Group:** {group}")
    if tags:
        lines.append(f"> **Tags:** {', '.join(f'`{t}`' for t in tags)}")
    lines.append("")

    if desc:
        lines.append(desc)
        lines.append("")

    # Input ports
    if inputs:
        lines.append("## Input Ports")
        lines.append("")
        lines.append("| Port | Label | Edge Kind | Required |")
        lines.append("|------|-------|-----------|----------|")
        for inp in inputs:
            lines.append(
                f"| `{inp['name']}` | {inp['label']} | {inp['edge_kind']} | "
                f"{'Yes' if inp.get('required') else 'No'} |"
            )
        lines.append("")

    # Parameters table
    if props:
        # Group by section
        sections = {}
        for name, prop in props.items():
            section = prop.get("x-zennode-section", "Main")
            sections.setdefault(section, []).append((name, prop))

        lines.append("## Parameters")
        lines.append("")

        for section, params in sections.items():
            if len(sections) > 1:
                lines.append(f"### {section}")
                lines.append("")

            lines.append("| Parameter | Type | Default | Description |")
            lines.append("|-----------|------|---------|-------------|")

            for name, prop in params:
                title = prop.get("title", name)
                default = prop.get("default", "")
                unit = prop.get("x-zennode-unit", "")
                optional = prop.get("x-zennode-optional", False)
                tl = type_label(prop)

                desc_text = prop.get("description", title)
                if unit:
                    desc_text += f" ({unit})"
                if optional:
                    desc_text += " *(optional)*"

                default_str = str(default) if default != "" else "—"

                lines.append(f"| `{name}` | {tl} | {default_str} | {desc_text} |")

            lines.append("")

        # Enum details
        for name, prop in props.items():
            if "enum" in prop:
                labels = prop.get("x-zennode-enum-labels", [])
                lines.append(f"### `{name}` values")
                lines.append("")
                lines.append("| Value | Label | Description |")
                lines.append("|-------|-------|-------------|")
                for i, val in enumerate(prop["enum"]):
                    label_info = labels[i] if i < len(labels) else {}
                    lbl = label_info.get("label", val)
                    edesc = label_info.get("description", "")
                    lines.append(f"| `{val}` | {lbl} | {edesc} |")
                lines.append("")

    # RIAPI querystring keys
    kv_params = [(name, prop) for name, prop in props.items() if prop.get("x-zennode-kv-keys")]
    if kv_params:
        lines.append("## RIAPI Querystring Keys")
        lines.append("")
        lines.append("| Key | Aliases | Parameter |")
        lines.append("|-----|---------|-----------|")
        for name, prop in kv_params:
            keys = prop["x-zennode-kv-keys"]
            primary = keys[0]
            aliases = ", ".join(f"`{k}`" for k in keys[1:]) if len(keys) > 1 else "—"
            lines.append(f"| `{primary}` | {aliases} | `{name}` |")
        lines.append("")

        # Example querystring
        example_parts = []
        for name, prop in kv_params[:3]:
            key = prop["x-zennode-kv-keys"][0]
            default = prop.get("default", "")
            if default and default != "" and default != "within":
                example_parts.append(f"{key}={default}")
            elif "integer" in str(prop.get("type", "")):
                example_parts.append(f"{key}=400")
            else:
                example_parts.append(f"{key}=value")
        if example_parts:
            lines.append(f"**Example:** `?{'&'.join(example_parts)}`")
            lines.append("")

    return "\n".join(lines)


def generate_index(nodes: dict) -> str:
    """Generate the main index page."""
    lines = []
    lines.append("# Zen Pipeline Node Reference")
    lines.append("")
    lines.append("Auto-generated from [zennode](https://github.com/imazen/zennode) schemas.")
    lines.append("")

    # Pipeline diagram
    lines.append("## Pipeline Flow")
    lines.append("")
    lines.append("```mermaid")
    lines.append("graph LR")
    lines.append("    A[📥 Decode] --> B[🔄 Orient]")
    lines.append("    B --> C[📐 Crop/Region]")
    lines.append("    C --> D[↔️ Resize/Constrain]")
    lines.append("    D --> E[🎨 Filters]")
    lines.append("    E --> F[🔲 Composite/Watermark]")
    lines.append("    F --> G[📤 Encode]")
    lines.append("    style A fill:#e1f5fe")
    lines.append("    style G fill:#e8f5e9")
    lines.append("```")
    lines.append("")

    # Group by role
    by_group = {}
    for node_id, schema in sorted(nodes.items()):
        group = schema.get("x-zennode-group", "Other")
        by_group.setdefault(group, []).append((node_id, schema))

    for group in sorted(by_group.keys()):
        emoji = group_emoji(group)
        lines.append(f"## {emoji} {group}")
        lines.append("")
        lines.append("| Node | Description | RIAPI Keys |")
        lines.append("|------|-------------|------------|")

        for node_id, schema in by_group[group]:
            label = schema.get("title", pascal_to_title(node_id))
            desc = schema.get("description", "")[:80]
            slug = node_id_to_slug(node_id)

            # Collect RIAPI keys
            kv_keys = []
            for prop in schema.get("properties", {}).values():
                for k in prop.get("x-zennode-kv-keys", []):
                    kv_keys.append(k)
            kv_str = ", ".join(f"`{k}`" for k in kv_keys[:4])
            if len(kv_keys) > 4:
                kv_str += f" +{len(kv_keys)-4}"

            lines.append(f"| [{label}](nodes/{slug}.md) | {desc} | {kv_str} |")

        lines.append("")

    return "\n".join(lines)


def generate_querystring_reference(qs_keys: dict) -> str:
    """Generate the RIAPI querystring key reference page."""
    lines = []
    lines.append("# RIAPI Querystring Reference")
    lines.append("")
    lines.append("All supported querystring keys for image processing URLs.")
    lines.append("")
    lines.append("## Quick Reference")
    lines.append("")
    lines.append("```")
    lines.append("?w=800&h=600&mode=crop&format=webp&qp=high&accept.webp=true")
    lines.append("```")
    lines.append("")

    nodes = qs_keys.get("nodes", {})
    for node_id in sorted(nodes.keys()):
        node_info = nodes[node_id]
        label = node_info.get("label", node_id)
        keys = node_info.get("keys", [])
        if not keys:
            continue

        slug = node_id_to_slug(node_id)
        lines.append(f"### [{label}](nodes/{slug}.md)")
        lines.append("")
        lines.append("| Key | Aliases | Type | Description |")
        lines.append("|-----|---------|------|-------------|")

        for key_info in keys:
            key = key_info.get("key", "")
            aliases = key_info.get("aliases", [])
            alias_str = ", ".join(f"`{a}`" for a in aliases) if aliases else "—"
            desc = key_info.get("description", key_info.get("label", ""))

            value_schema = key_info.get("value_schema", {})
            vtype = value_schema.get("type", "string")
            if isinstance(vtype, list):
                vtype = [x for x in vtype if x != "null"][0]

            lines.append(f"| `{key}` | {alias_str} | {vtype} | {desc} |")

        lines.append("")

    return "\n".join(lines)


def generate_codec_reference(codecs: dict) -> str:
    """Generate the format/codec reference page."""
    lines = []
    lines.append("# Supported Image Formats")
    lines.append("")
    lines.append("| Format | MIME Type | Extensions | Alpha | Lossless | Animation | Decode | Encode |")
    lines.append("|--------|----------|------------|-------|----------|-----------|--------|--------|")

    for codec in codecs.get("codecs", []):
        name = codec["name"]
        mime = codec["mime_type"]
        exts = ", ".join(codec.get("extensions", []))
        alpha = "✅" if codec.get("supports_alpha") else "—"
        lossless = "✅" if codec.get("supports_lossless") else "—"
        anim = "✅" if codec.get("supports_animation") else "—"
        dec = "✅" if codec.get("can_decode") else "—"
        enc = "✅" if codec.get("can_encode") else "—"
        lines.append(f"| **{name.upper()}** | `{mime}` | {exts} | {alpha} | {lossless} | {anim} | {dec} | {enc} |")

    lines.append("")
    return "\n".join(lines)


def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <schema_dir> <output_dir>")
        print("  schema_dir: directory with v3_nodes.json, v3_qs_keys.json, v3_codecs.json")
        print("  output_dir: directory to write Markdown files")
        sys.exit(1)

    schema_dir = Path(sys.argv[1])
    output_dir = Path(sys.argv[2])

    # Load schemas
    nodes_schema = load_json(schema_dir / "v3_nodes.json")
    qs_keys = load_json(schema_dir / "v3_qs_keys.json")

    codecs = {}
    codecs_path = schema_dir / "v3_codecs.json"
    if codecs_path.exists():
        codecs = load_json(codecs_path)

    nodes = nodes_schema.get("$defs", {})
    print(f"Loaded {len(nodes)} node schemas, {sum(len(n.get('keys', [])) for n in qs_keys.get('nodes', {}).values())} QS keys")

    # Create output structure
    nodes_dir = output_dir / "nodes"
    nodes_dir.mkdir(parents=True, exist_ok=True)

    # Generate index
    index_md = generate_index(nodes)
    (output_dir / "index.md").write_text(index_md)
    print(f"  wrote index.md")

    # Generate per-node pages
    for node_id, schema in sorted(nodes.items()):
        slug = node_id_to_slug(node_id)
        page_md = generate_node_page(node_id, schema)
        (nodes_dir / f"{slug}.md").write_text(page_md)
    print(f"  wrote {len(nodes)} node pages")

    # Generate QS reference
    qs_md = generate_querystring_reference(qs_keys)
    (output_dir / "querystring.md").write_text(qs_md)
    print(f"  wrote querystring.md")

    # Generate codec reference
    if codecs:
        codec_md = generate_codec_reference(codecs)
        (output_dir / "formats.md").write_text(codec_md)
        print(f"  wrote formats.md")

    # Generate _config.yml for GitHub Pages (Jekyll)
    config = """title: Zen Pipeline Nodes
description: Auto-generated reference for zen image processing nodes
theme: jekyll-theme-minimal
"""
    (output_dir / "_config.yml").write_text(config)

    print(f"\nDone! {len(nodes)} node pages + index + QS reference + formats")
    print(f"Push {output_dir}/ to a gh-pages branch for GitHub Pages.")


if __name__ == "__main__":
    main()
