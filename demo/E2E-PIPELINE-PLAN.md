# End-to-End Pipeline Implementation Plan

## Phases (in dependency order)

### Phase 1: Rich Decode (decode.rs)
- `try_decode_rich(bytes)` → `RichDecodedImage` with metadata, ImageInfo, gain map info
- Uses `DecoderConfig::job().probe(bytes)` for metadata extraction
- Extends existing `try_decode()` as thin wrapper

### Phase 2: Editor::from_bytes (editor.rs)  
- New constructor that decodes internally, preserves metadata
- Content-based `source_hash` from bytes (not just dimensions)
- `make_source_info()` propagates real metadata to ProcessConfig
- Metadata fields on Editor struct

### Phase 3: WASM API (wasm_api.rs)
- `WasmEditor::from_bytes(bytes, overview_max, detail_max)`
- `has_metadata`, `source_format` getters

### Phase 4: Encode with Metadata (encode.rs)
- `encode_with_metadata(rgba, w, h, format, options, metadata)`
- ICC profile, EXIF, XMP passthrough per codec

### Phase 5: Two-Phase Worker Decode (worker.js)
- Browser decode → instant preview → send 'ready'
- WASM from_bytes() in background → send 'upgrade' when done
- Main thread re-renders on upgrade

### Phase 6: Edit Presets (preset.rs + presets-storage.js)
- `EditPreset` JSON schema (name, version, film_preset, adjustments)
- Querystring serialization for URL sharing
- localStorage persistence in browser
- Save/load/delete UI

## Deferred
- Gain map pixel decoding (depends on per-codec API)
- HEIC decode (needs heic crate integration)
- End-to-end streaming (decode → Session → encode without materialization)
