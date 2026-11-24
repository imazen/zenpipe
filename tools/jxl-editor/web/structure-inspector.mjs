import {
  DEFAULT_FILTERS, DEFAULT_MASK, DEFAULT_ZENSR,
  effectiveLayerSize, hasActiveMask, hasActiveZensr, hasAdjustments,
} from "./filters.mjs";
import { defaultCoding } from "./author-panels.mjs";
import { jxlBoxes, MAX_PIXELS, MAX_SIDE, validateManifest } from "./project.mjs";

const $ = id => document.getElementById(id);
const utf8 = new TextEncoder();

const KIND_META = {
  native: { badge: "NATIVE JXL", cls: "prov-native", plain: true, desc: "Standard ISO/IEC 18181 JPEG XL feature decoded natively by all JXL viewers." },
  prerendered: { badge: "PRERENDERED", cls: "prov-prerendered", plain: true, desc: "Baked into the standard JXL presentation pixels so plain JXL viewers see the exact result." },
  editor: { badge: "EDITOR DATA", cls: "prov-editor", plain: false, desc: "Private non-destructive container box stored only in .project.jxl (stripped by plain Export JXL)." },
  preserved: { badge: "PRESERVED", cls: "prov-preserved", plain: true, desc: "Unknown third-party container box carried through verbatim in both Plain JXL and .project.jxl." },
};

const BOX_DESC = {
  "JXL ": { title: "JPEG XL Signature", kind: "native", desc: "12-byte ISO BMFF JPEG XL container signature." },
  ftyp: { title: "File Type Box", kind: "native", desc: "Declares the `jxl ` brand for standard JXL container readers." },
  jxlc: { title: "JPEG XL Codestream", kind: "native", desc: "Primary image/animation codestream decoded by every JXL viewer." },
  jxlp: { title: "Partial Codestream", kind: "native", desc: "Chunked JXL codestream box." },
  jxll: { title: "Codestream Level", kind: "native", desc: "Signals Level 10 features (e.g. CMYK, >4 extra channels)." },
  jbrd: { title: "JPEG Bitstream Reconstruction", kind: "native", desc: "Bit-exact original JPEG reconstruction data." },
  Exif: { title: "EXIF Metadata", kind: "native", desc: "Camera/photo EXIF metadata box." },
  "xml ": { title: "XMP Metadata", kind: "native", desc: "Adobe XMP XML metadata box." },
  jhgm: { title: "HDR Gain Map", kind: "native", desc: "ISO 21496-1 / JXL HDR gain map bundle." },
  colr: { title: "Color Profile Box", kind: "native", desc: "Container color profile box." },
  brob: { title: "Brotli-Compressed Box", kind: "native", desc: "Brotli-compressed metadata or application box." },
  zned: { title: "Editor Manifest JSON", kind: "editor", desc: "Non-destructive layer tree, transforms, ZenFilters, masks, ZenSR, timeline & spline definitions." },
  znas: { title: "Original Layer Asset", kind: "editor", desc: "Untouched original layer source bytes (JPEG/PNG/JXL/SVG) for lossless re-editing." },
  znrv: { title: "SVG Raster Rendition", kind: "editor", desc: "Lossless JXL fallback rendition of an SVG layer." },
  znvs: { title: "Edited SVG Markup", kind: "editor", desc: "UTF-8 edited SVG vector markup saved non-destructively alongside the original SVG." },
  znmk: { title: "Painted Raster Mask", kind: "editor", desc: "Lossless JXL grayscale alpha channel storing brush-painted layer masks." },
  zncm: { title: "CMYK Ink Samples", kind: "editor", desc: "Exact 8-bit or 16-bit inverted CMYK ink samples for non-destructive CMYK editing." },
  znic: { title: "CMYK ICC Profile", kind: "editor", desc: "Embedded CMYK ICC profile used for proofing and Level 10 CMYK export." },
};

function fmtBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}

function makeBadge(kind) {
  const meta = KIND_META[kind] || KIND_META.native;
  const span = document.createElement("span");
  span.className = `prov-badge ${meta.cls}`;
  span.textContent = meta.badge;
  span.title = meta.desc;
  return span;
}

function makePropRow(label, inputEl) {
  const row = document.createElement("label");
  row.className = "struct-prop-row";
  const cap = document.createElement("span");
  cap.textContent = label;
  row.append(cap, inputEl);
  return row;
}

function makeKV(key, val, badgeKind = null) {
  const div = document.createElement("div");
  div.className = "struct-kv";
  const k = document.createElement("span");
  k.className = "struct-k";
  k.textContent = key;
  const v = document.createElement("span");
  v.className = "struct-v";
  v.textContent = val;
  if (badgeKind) v.append(" ", makeBadge(badgeKind));
  div.append(k, v);
  return div;
}

export function makeStructureInspector({
  getState,
  getSelectedLayerId,
  selectLayer,
  getTitle,
  isPassthrough,
  updateLayer,
  mutateManifest,
  openSvgEditor,
  downloadSource,
  download,
  status,
  report,
}) {
  let selectedNodeId = "doc";
  let provFilter = "all"; // all | plain | native | prerendered | editor

  function plannedBoxes(state) {
    const m = state.manifest;
    const passthrough = isPassthrough(state);
    const hasNativeFeatures = Boolean(
      passthrough || m.timeline?.enabled || m.cmyk?.enabled || m.splines?.length,
    );
    const boxes = [
      { id: "box:sig", type: "JXL ", title: "JPEG XL Signature", kind: "native", bytes: 12, plain: true },
      { id: "box:ftyp", type: "ftyp", title: "File Type (jxl )", kind: "native", bytes: 20, plain: true },
      {
        id: "box:jxlc",
        type: "jxlc",
        title: passthrough
          ? "Original JXL Codestream (Zero-Loss Passthrough)"
          : m.cmyk?.enabled
            ? `Level 10 CMYK ${m.cmyk.bitDepth}-bit Codestream`
            : m.timeline?.enabled
              ? `Native Animation Codestream (${m.timeline.frames.length} frames)`
              : m.splines?.length
                ? `VarDCT Codestream + ${m.splines.length} Native Spline(s)`
                : "Prerendered Presentation Codestream",
        kind: hasNativeFeatures ? "native" : "prerendered",
        bytes: passthrough ? (state.assets.get(m.originalId)?.source?.byteLength ?? null) : null,
        plain: true,
      },
    ];
    for (let i = 0; i < (state.preserved?.length ?? 0); i++) {
      const p = state.preserved[i];
      boxes.push({
        id: `box:preserved:${i}`,
        type: p.type,
        title: `Foreign box "${p.type}"`,
        kind: "preserved",
        bytes: p.data.byteLength,
        plain: true,
        rawBox: p,
      });
    }
    const manifestBytes = utf8.encode(JSON.stringify(m)).byteLength;
    boxes.push({
      id: "box:zned",
      type: "zned",
      title: "Editor Manifest JSON",
      kind: "editor",
      bytes: manifestBytes,
      plain: false,
    });
    for (const layer of m.layers) {
      const asset = state.assets.get(layer.id);
      boxes.push({
        id: `box:znas:${layer.id}`,
        type: "znas",
        title: `Source: ${layer.name}`,
        kind: "editor",
        bytes: asset?.source?.byteLength ?? 0,
        plain: false,
        layerId: layer.id,
      });
      if (layer.mime === "image/svg+xml") {
        boxes.push({
          id: `box:znrv:${layer.id}`,
          type: "znrv",
          title: `SVG Rendition: ${layer.name}`,
          kind: "editor",
          bytes: asset?.rendition?.byteLength ?? null,
          plain: false,
          layerId: layer.id,
        });
        if (asset?.svgEdit) {
          boxes.push({
            id: `box:znvs:${layer.id}`,
            type: "znvs",
            title: `Edited SVG Markup: ${layer.name}`,
            kind: "editor",
            bytes: asset.svgEdit.byteLength,
            plain: false,
            layerId: layer.id,
          });
        }
      }
      if (layer.mask?.hasRaster || asset?.maskAlpha || asset?.maskJxl) {
        boxes.push({
          id: `box:znmk:${layer.id}`,
          type: "znmk",
          title: `Painted Mask JXL: ${layer.name}`,
          kind: "editor",
          bytes: asset?.maskJxl?.byteLength ?? null,
          plain: false,
          layerId: layer.id,
        });
      }
    }
    if (m.cmyk?.enabled && state.cmykData) {
      boxes.push({
        id: "box:zncm",
        type: "zncm",
        title: `CMYK Raw Ink Samples (${m.cmyk.bitDepth}-bit)`,
        kind: "editor",
        bytes: state.cmykData.samples?.byteLength ?? 0,
        plain: false,
      });
      boxes.push({
        id: "box:znic",
        type: "znic",
        title: `CMYK ICC Profile (${m.cmyk.iccName || "ICC"})`,
        kind: "editor",
        bytes: state.cmykData.icc?.byteLength ?? 0,
        plain: false,
      });
    }
    return boxes;
  }

  function buildTree(state) {
    const m = state.manifest;
    const passthrough = isPassthrough(state);
    const coding = { ...defaultCoding(), ...(m.coding ?? {}) };
    const nodes = [];

    nodes.push({
      id: "doc",
      depth: 0,
      label: `${getTitle()} (${m.width}×${m.height})`,
      sub: `${m.cmyk?.enabled ? `CMYK ${m.cmyk.bitDepth}-bit` : "RGBA sRGB"} · ${coding.mode.toUpperCase()}`,
      kind: "native",
    });

    const codestreamKind = passthrough || m.timeline?.enabled || m.cmyk?.enabled || m.splines?.length
      ? "native"
      : (m.layers.length === 1 && !hasAdjustments(m.layers[0]) && !hasActiveMask(m.layers[0], state.assets.get(m.layers[0].id)) && !hasActiveZensr(m.layers[0]) && m.layers[0].scale === 1 && m.layers[0].angle === 0 && m.layers[0].mime === "image/jxl")
        ? "native"
        : "prerendered";

    nodes.push({
      id: "codestream",
      depth: 1,
      label: "Presentation Codestream (jxlc)",
      sub: passthrough
        ? "Zero-loss original JXL passthrough"
        : m.cmyk?.enabled
          ? `Level 10 CMYK (${m.cmyk.mode})`
          : m.timeline?.enabled
            ? `Native JXL animation · ${m.timeline.frames.length} frames`
            : m.splines?.length
              ? `VarDCT + ${m.splines.length} native spline(s)`
              : m.layers.length >= 2 && coding.mode === "modular"
                ? "Auto native multi-frame layers / composite"
                : `${coding.mode === "vardct" ? `VarDCT d=${coding.distance}` : "Modular lossless"}`,
      kind: codestreamKind,
    });

    if (m.timeline?.enabled && m.timeline.frames?.length) {
      m.timeline.frames.forEach((f, idx) => {
        nodes.push({
          id: `frame:${idx}`,
          depth: 2,
          label: `Frame ${idx + 1}: ${f.name || "Untitled"}`,
          sub: `${f.duration} ticks · ${f.blendMode || "auto"}${f.referenceOnly ? " · ref-only" : ""}`,
          kind: "native",
          frameIdx: idx,
        });
      });
    }

    if (m.splines?.length) {
      for (const s of m.splines) {
        nodes.push({
          id: `spline:${s.id}`,
          depth: 2,
          label: `Spline: ${s.name}`,
          sub: `${s.controlPoints.length} pts · σ=${s.sigma}`,
          kind: "native",
          splineId: s.id,
        });
      }
    }

    if (m.cmyk?.enabled) {
      nodes.push({
        id: "cmyk",
        depth: 2,
        label: `CMYK + Black Channel (${m.cmyk.bitDepth}-bit)`,
        sub: `${m.cmyk.mode} · ${state.cmykData?.icc?.byteLength ?? 0} B ICC`,
        kind: "native",
      });
    }

    nodes.push({
      id: "layers-root",
      depth: 1,
      label: `Layers & Composition (${m.layers.length})`,
      sub: m.layers.length > 1 ? "Composited in browser canvas → JXL presentation" : "Single layer",
      kind: passthrough ? "native" : "prerendered",
    });

    for (const layer of [...m.layers].reverse()) {
      const asset = state.assets.get(layer.id);
      const eff = effectiveLayerSize(layer);
      const isPlainNative = passthrough && layer.id === m.originalId;
      nodes.push({
        id: `layer:${layer.id}`,
        depth: 2,
        label: `${layer.visible ? "" : "[Hidden] "}${layer.name}`,
        sub: `${eff.width}×${eff.height} · ${Math.round(layer.scale * 100)}% · ${layer.angle}°`,
        kind: isPlainNative ? "native" : "prerendered",
        layerId: layer.id,
      });
      nodes.push({
        id: `layer-source:${layer.id}`,
        depth: 3,
        label: `znas · Original ${layer.mime.split("/")[1].toUpperCase()}`,
        sub: `${layer.width}×${layer.height} · ${fmtBytes(asset?.source?.byteLength ?? 0)}`,
        kind: "editor",
        layerId: layer.id,
      });
      if (layer.mime === "image/svg+xml") {
        nodes.push({
          id: `layer-svg:${layer.id}`,
          depth: 3,
          label: asset?.svgEdit ? "znvs · Edited Vector SVG → Raster" : "SVG Vector → Raster",
          sub: asset?.svgEdit ? `${fmtBytes(asset.svgEdit.byteLength)} markup` : "Prerendered at target scale",
          kind: asset?.svgEdit ? "editor" : "prerendered",
          layerId: layer.id,
        });
      }
      if (hasActiveZensr(layer)) {
        nodes.push({
          id: `layer-zensr:${layer.id}`,
          depth: 3,
          label: `ZenSR Neural (${layer.zensr.model} · ${layer.zensr.scale}×)`,
          sub: `${layer.width}×${layer.height} → ${eff.width}×${eff.height}`,
          kind: "prerendered",
          layerId: layer.id,
        });
      }
      if (hasAdjustments(layer)) {
        const fCount = Object.keys(layer.filters ?? {}).length +
          Number(layer.exposure !== 0) + Number(layer.contrast !== 1) + Number(layer.saturation !== 1);
        nodes.push({
          id: `layer-filters:${layer.id}`,
          depth: 3,
          label: `ZenFilters Oklab (${fCount} active)`,
          sub: layer.filters?.preset || layer.filters?.filmLook || "Non-destructive color & tone",
          kind: "prerendered",
          layerId: layer.id,
        });
      }
      if (hasActiveMask(layer, asset)) {
        nodes.push({
          id: `layer-mask:${layer.id}`,
          depth: 3,
          label: `Layer Mask (${layer.mask.mode}/${layer.mask.shape})`,
          sub: layer.mask.hasRaster ? "Includes znmk lossless JXL alpha" : "Parametric non-destructive",
          kind: "editor",
          layerId: layer.id,
        });
      }
    }

    const boxes = plannedBoxes(state);
    const editorBytes = boxes.filter(b => b.kind === "editor").reduce((s, b) => s + (b.bytes ?? 0), 0);
    nodes.push({
      id: "boxes-root",
      depth: 1,
      label: `Container Boxes (${boxes.length} in .project.jxl)`,
      sub: `${boxes.filter(b => b.plain).length} in Plain JXL · ${fmtBytes(editorBytes)} editor data`,
      kind: "native",
    });
    for (const b of boxes) {
      nodes.push({
        id: b.id,
        depth: 2,
        label: `${b.type} · ${b.title}`,
        sub: b.bytes != null ? fmtBytes(b.bytes) : "encoded on save",
        kind: b.kind,
        boxInfo: b,
      });
    }

    // Also inspect native source boxes if the selected layer is a JXL container
    const activeLayer = m.layers.find(l => l.id === getSelectedLayerId()) ?? m.layers[0];
    if (activeLayer?.mime === "image/jxl") {
      try {
        const srcBoxes = jxlBoxes(state.assets.get(activeLayer.id).source);
        if (srcBoxes.length) {
          nodes.push({
            id: "src-boxes-root",
            depth: 1,
            label: `Source JXL Boxes (${srcBoxes.length})`,
            sub: activeLayer.name,
            kind: "native",
          });
          srcBoxes.forEach((sb, idx) => {
            const meta = BOX_DESC[sb.type];
            nodes.push({
              id: `srcbox:${idx}`,
              depth: 2,
              label: `${sb.type} · ${meta?.title ?? "Application box"}`,
              sub: fmtBytes(sb.data.byteLength),
              kind: meta?.kind ?? "preserved",
              sourceBox: sb,
            });
          });
        }
      } catch {}
    }

    return nodes;
  }

  function matchesFilter(node) {
    if (provFilter === "all") return true;
    if (provFilter === "plain") return KIND_META[node.kind]?.plain === true;
    return node.kind === provFilter;
  }

  function renderProperties(state, node) {
    const panel = $("struct-props");
    if (!panel) return;
    panel.replaceChildren();
    if (!node) {
      panel.textContent = "Select a node in the document structure tree to view and edit its properties.";
      return;
    }

    const head = document.createElement("div");
    head.className = "struct-prop-head";
    const h3 = document.createElement("h3");
    h3.textContent = node.label;
    head.append(h3, makeBadge(node.kind));
    panel.append(head);

    const desc = document.createElement("p");
    desc.className = "muted-note";
    desc.textContent = KIND_META[node.kind]?.desc ?? "";
    panel.append(desc);

    const m = state.manifest;
    const coding = { ...defaultCoding(), ...(m.coding ?? {}) };

    if (node.id === "doc" || node.id === "codestream") {
      const grid = document.createElement("div");
      grid.className = "struct-prop-grid";
      const wIn = document.createElement("input");
      wIn.type = "number"; wIn.min = "1"; wIn.max = String(MAX_SIDE); wIn.value = String(m.width);
      const hIn = document.createElement("input");
      hIn.type = "number"; hIn.min = "1"; hIn.max = String(MAX_SIDE); hIn.value = String(m.height);
      const applyCanvas = () => {
        const w = Math.round(wIn.valueAsNumber);
        const h = Math.round(hIn.valueAsNumber);
        if (!Number.isInteger(w) || !Number.isInteger(h) || w < 1 || h < 1 || w > MAX_SIDE || h > MAX_SIDE || w * h > MAX_PIXELS) {
          report(new Error("Canvas dimensions must be within 16,384 px / 64 MP."));
          return;
        }
        if (w === m.width && h === m.height) return;
        mutateManifest(next => { next.width = w; next.height = h; }, `Resized canvas to ${w}×${h}`);
      };
      wIn.onchange = applyCanvas;
      hIn.onchange = applyCanvas;

      const modeSel = document.createElement("select");
      modeSel.add(new Option("Modular (Lossless)", "modular"));
      modeSel.add(new Option("VarDCT (Lossy)", "vardct"));
      modeSel.value = coding.mode;
      modeSel.onchange = () => mutateManifest(next => {
        next.coding = { ...defaultCoding(), ...(next.coding ?? {}), mode: modeSel.value };
      }, `Set coding mode to ${modeSel.value}`);

      const effortIn = document.createElement("input");
      effortIn.type = "number"; effortIn.min = "1"; effortIn.max = "10"; effortIn.value = String(coding.effort);
      effortIn.onchange = () => {
        const v = Math.max(1, Math.min(10, Math.round(effortIn.valueAsNumber || 3)));
        mutateManifest(next => {
          next.coding = { ...defaultCoding(), ...(next.coding ?? {}), effort: v };
        }, `Set coding effort to ${v}`);
      };

      const distIn = document.createElement("input");
      distIn.type = "number"; distIn.min = "0.05"; distIn.max = "15"; distIn.step = "0.05";
      distIn.value = String(coding.distance);
      distIn.disabled = coding.mode !== "vardct";
      distIn.onchange = () => {
        const v = Math.max(0.05, Math.min(15, Number(distIn.valueAsNumber || 1)));
        mutateManifest(next => {
          next.coding = { ...defaultCoding(), ...(next.coding ?? {}), distance: v };
        }, `Set VarDCT distance to ${v}`);
      };

      const contSel = document.createElement("select");
      contSel.add(new Option("Auto", "auto"));
      contSel.add(new Option("Always (ISOBMFF)", "always"));
      contSel.add(new Option("Never (Bare codestream)", "never"));
      contSel.value = coding.containerMode;
      contSel.onchange = () => mutateManifest(next => {
        next.coding = { ...defaultCoding(), ...(next.coding ?? {}), containerMode: contSel.value };
      }, `Set container wrapper to ${contSel.value}`);

      grid.append(
        makePropRow("Canvas width (px)", wIn),
        makePropRow("Canvas height (px)", hIn),
        makePropRow("Coding mode", modeSel),
        makePropRow("Effort (1–10)", effortIn),
        makePropRow("VarDCT distance", distIn),
        makePropRow("Container mode", contSel),
      );
      panel.append(grid);
      panel.append(
        makeKV("Plain JXL passthrough", isPassthrough(state) ? "Yes (bit-exact original bytes)" : "No (re-encoded on export)", isPassthrough(state) ? "native" : "prerendered"),
        makeKV("Native features active", [
          m.timeline?.enabled ? `Animation (${m.timeline.frames.length}f)` : null,
          m.splines?.length ? `Splines (${m.splines.length})` : null,
          m.cmyk?.enabled ? `CMYK ${m.cmyk.bitDepth}b` : null,
          state.preserved?.length ? `${state.preserved.length} foreign box(es)` : null,
        ].filter(Boolean).join(", ") || "Standard still frame", "native"),
      );
      return;
    }

    if (node.id.startsWith("layer:")) {
      const layer = m.layers.find(l => l.id === node.layerId);
      if (!layer) return;
      selectLayer(layer.id);
      const grid = document.createElement("div");
      grid.className = "struct-prop-grid";

      const nameIn = document.createElement("input");
      nameIn.type = "text"; nameIn.value = layer.name;
      nameIn.onchange = () => updateLayer({ name: nameIn.value.trim() || layer.name }, "Renamed layer", layer.id);

      const visIn = document.createElement("input");
      visIn.type = "checkbox"; visIn.checked = layer.visible;
      visIn.onchange = () => updateLayer({ visible: visIn.checked }, visIn.checked ? "Showed layer" : "Hid layer", layer.id);

      const numField = (key, min, max, step, label) => {
        const inp = document.createElement("input");
        inp.type = "number"; inp.min = String(min); inp.max = String(max); inp.step = String(step);
        inp.value = String(layer[key]);
        inp.onchange = () => {
          const v = inp.valueAsNumber;
          if (Number.isFinite(v)) updateLayer({ [key]: Math.max(min, Math.min(max, v)) }, `Edited ${label}`, layer.id);
        };
        return makePropRow(label, inp);
      };

      grid.append(
        makePropRow("Name", nameIn),
        makePropRow("Visible", visIn),
        numField("x", -32768, 32768, 1, "Position X"),
        numField("y", -32768, 32768, 1, "Position Y"),
        numField("scale", 0.05, 8, 0.05, "Scale"),
        numField("angle", -180, 180, 1, "Rotation (°)"),
        numField("opacity", 0, 1, 0.05, "Opacity"),
        numField("exposure", -2, 2, 0.05, "Exposure (EV)"),
        numField("contrast", 0, 2, 0.05, "Contrast"),
        numField("saturation", 0, 2, 0.05, "Saturation"),
      );
      panel.append(grid);
      const eff = effectiveLayerSize(layer);
      panel.append(
        makeKV("Native asset size", `${layer.width} × ${layer.height} px (${layer.mime})`, "editor"),
        makeKV("Effective rendered size", `${Math.round(eff.width * layer.scale)} × ${Math.round(eff.height * layer.scale)} px`, "prerendered"),
      );
      return;
    }

    if (node.id.startsWith("layer-source:") || node.id.startsWith("layer-svg:")) {
      const layer = m.layers.find(l => l.id === node.layerId);
      const asset = state.assets.get(node.layerId);
      if (!layer || !asset) return;
      selectLayer(layer.id);
      panel.append(
        makeKV("Box type", node.id.startsWith("layer-svg:") ? "znvs (UTF-8 SVG markup)" : "znas (Original source bytes)", "editor"),
        makeKV("Layer ID (UUID)", layer.id),
        makeKV("MIME type", layer.mime),
        makeKV("Source byte length", `${asset.source.byteLength.toLocaleString()} B`),
        makeKV("Included in Plain JXL?", "No — stripped on Export JXL; kept verbatim in .project.jxl", "editor"),
      );
      const actions = document.createElement("div");
      actions.className = "asset-actions";
      const dl = document.createElement("button");
      dl.type = "button"; dl.textContent = "Download original source";
      dl.onclick = () => downloadSource();
      actions.append(dl);
      if (layer.mime === "image/svg+xml") {
        const editSvg = document.createElement("button");
        editSvg.type = "button"; editSvg.className = "primary"; editSvg.textContent = "Open SVG Vector Studio";
        editSvg.onclick = () => openSvgEditor();
        actions.append(editSvg);
      }
      panel.append(actions);
      return;
    }

    if (node.id.startsWith("layer-filters:")) {
      const layer = m.layers.find(l => l.id === node.layerId);
      if (!layer) return;
      selectLayer(layer.id);
      const active = {
        ...(layer.exposure !== 0 ? { exposure: layer.exposure } : {}),
        ...(layer.contrast !== 1 ? { contrast: layer.contrast } : {}),
        ...(layer.saturation !== 1 ? { saturation: layer.saturation } : {}),
        ...(layer.filters ?? {}),
      };
      for (const [k, v] of Object.entries(active)) {
        panel.append(makeKV(k, typeof v === "object" ? JSON.stringify(v) : String(v), "prerendered"));
      }
      const btn = document.createElement("button");
      btn.type = "button"; btn.textContent = "Reset ZenFilters on this layer";
      btn.onclick = () => updateLayer({ exposure: 0, contrast: 1, saturation: 1, filters: undefined }, "Reset layer filters", layer.id);
      panel.append(btn);
      return;
    }

    if (node.id.startsWith("layer-zensr:")) {
      const layer = m.layers.find(l => l.id === node.layerId);
      if (!layer) return;
      selectLayer(layer.id);
      const z = { ...DEFAULT_ZENSR, ...(layer.zensr ?? {}) };
      panel.append(
        makeKV("Model", `${z.model} (${z.arch}, nf=${z.nf}, nc=${z.nc})`),
        makeKV("Scale factor", `${z.scale}× (${layer.width}×${layer.height} → ${layer.width * z.scale}×${layer.height * z.scale})`, "prerendered"),
        makeKV("Strength", `${Math.round((z.strength ?? 1) * 100)}%`),
        makeKV("Stored in .project.jxl", "Config in zned; original source stays unscaled in znas", "editor"),
      );
      return;
    }

    if (node.id.startsWith("layer-mask:")) {
      const layer = m.layers.find(l => l.id === node.layerId);
      if (!layer) return;
      selectLayer(layer.id);
      const mk = { ...DEFAULT_MASK, ...(layer.mask ?? {}) };
      const grid = document.createElement("div");
      grid.className = "struct-prop-grid";
      const en = document.createElement("input");
      en.type = "checkbox"; en.checked = Boolean(mk.enabled);
      en.onchange = () => updateLayer({ mask: { ...mk, enabled: en.checked } }, "Toggled mask", layer.id);
      const inv = document.createElement("input");
      inv.type = "checkbox"; inv.checked = Boolean(mk.invert);
      inv.onchange = () => updateLayer({ mask: { ...mk, invert: inv.checked } }, "Inverted mask", layer.id);
      const dens = document.createElement("input");
      dens.type = "number"; dens.min = "0"; dens.max = "1"; dens.step = "0.05"; dens.value = String(mk.density);
      dens.onchange = () => updateLayer({ mask: { ...mk, density: Math.max(0, Math.min(1, dens.valueAsNumber || 0)) } }, "Edited mask density", layer.id);
      const feath = document.createElement("input");
      feath.type = "number"; feath.min = "0"; feath.max = "40"; feath.step = "0.5"; feath.value = String(mk.feather);
      feath.onchange = () => updateLayer({ mask: { ...mk, feather: Math.max(0, Math.min(40, feath.valueAsNumber || 0)) } }, "Edited mask feather", layer.id);
      grid.append(
        makePropRow("Enabled", en),
        makePropRow("Invert", inv),
        makePropRow("Density (0–1)", dens),
        makePropRow("Feather (px)", feath),
      );
      panel.append(grid, makeKV("Raster paint box (znmk)", mk.hasRaster ? "Present (lossless JXL grayscale)" : "None (parametric only)", "editor"));
      return;
    }

    if (node.id.startsWith("frame:")) {
      const f = m.timeline?.frames?.[node.frameIdx];
      if (!f) return;
      const grid = document.createElement("div");
      grid.className = "struct-prop-grid";
      const nameIn = document.createElement("input");
      nameIn.type = "text"; nameIn.value = f.name || "";
      nameIn.onchange = () => mutateManifest(next => { next.timeline.frames[node.frameIdx].name = nameIn.value.slice(0, 1019); }, "Renamed frame");
      const durIn = document.createElement("input");
      durIn.type = "number"; durIn.min = "0"; durIn.value = String(f.duration);
      durIn.onchange = () => mutateManifest(next => { next.timeline.frames[node.frameIdx].duration = Math.max(0, Math.round(durIn.valueAsNumber || 0)); }, "Edited frame duration");
      const blendSel = document.createElement("select");
      for (const b of ["auto", "replace", "blend", "add", "alpha_weighted_add", "mul"]) blendSel.add(new Option(b, b));
      blendSel.value = f.blendMode || "auto";
      blendSel.onchange = () => mutateManifest(next => { next.timeline.frames[node.frameIdx].blendMode = blendSel.value; }, "Edited frame blend");
      const refIn = document.createElement("input");
      refIn.type = "checkbox"; refIn.checked = Boolean(f.referenceOnly);
      refIn.onchange = () => mutateManifest(next => { next.timeline.frames[node.frameIdx].referenceOnly = refIn.checked; }, "Toggled reference-only frame");
      grid.append(
        makePropRow("Frame name", nameIn),
        makePropRow("Duration (ticks)", durIn),
        makePropRow("Blend mode", blendSel),
        makePropRow("Reference only", refIn),
      );
      panel.append(grid);
      return;
    }

    if (node.id.startsWith("spline:")) {
      const s = m.splines?.find(x => x.id === node.splineId);
      if (!s) return;
      const grid = document.createElement("div");
      grid.className = "struct-prop-grid";
      const nameIn = document.createElement("input");
      nameIn.type = "text"; nameIn.value = s.name;
      nameIn.onchange = () => mutateManifest(next => {
        const t = next.splines.find(x => x.id === s.id);
        if (t) t.name = nameIn.value.trim() || t.name;
      }, "Renamed spline");
      const bakeIn = document.createElement("input");
      bakeIn.type = "checkbox"; bakeIn.checked = s.bakeIntoSource !== false;
      bakeIn.onchange = () => mutateManifest(next => {
        const t = next.splines.find(x => x.id === s.id);
        if (t) t.bakeIntoSource = bakeIn.checked;
      }, "Toggled spline bake");
      grid.append(makePropRow("Spline name", nameIn), makePropRow("Bake into VarDCT target", bakeIn));
      panel.append(
        grid,
        makeKV("Control points", `${s.controlPoints.length} points`, "native"),
        makeKV("Sigma width", String(s.sigma)),
      );
      return;
    }

    if (node.id === "cmyk" && m.cmyk?.enabled) {
      panel.append(
        makeKV("Bit depth", `${m.cmyk.bitDepth}-bit CMYK + Black extra channel`, "native"),
        makeKV("Coding mode", m.cmyk.mode, "native"),
        makeKV("Embedded ICC", `${m.cmyk.iccName || "CMYK ICC"} (${state.cmykData?.icc?.byteLength ?? 0} B)`, "native"),
        makeKV("Ink offsets (C,M,Y,K)", (m.cmyk.channelOffsets ?? [0, 0, 0, 0]).map(v => `${Math.round(v * 100)}%`).join(", "), "editor"),
      );
      return;
    }

    if (node.id === "box:zned") {
      panel.append(
        makeKV("Container 4CC", "zned (Editor non-destructive manifest JSON)", "editor"),
        makeKV("Included in Plain JXL?", "No — Export JXL omits zned", "editor"),
      );
      const ta = document.createElement("textarea");
      ta.id = "struct-manifest-json";
      ta.rows = 12;
      ta.spellcheck = false;
      ta.value = JSON.stringify(m, null, 2);
      const errEl = document.createElement("p");
      errEl.className = "error-text";
      const actions = document.createElement("div");
      actions.className = "asset-actions";
      const applyBtn = document.createElement("button");
      applyBtn.type = "button";
      applyBtn.className = "primary";
      applyBtn.textContent = "Validate & Apply JSON";
      applyBtn.onclick = () => {
        try {
          const parsed = JSON.parse(ta.value);
          validateManifest(parsed);
          // Ensure every referenced layer ID still exists in current assets
          for (const l of parsed.layers) {
            if (!state.assets.has(l.id)) throw new Error(`Unknown layer id "${l.id}" has no loaded asset.`);
          }
          mutateManifest(next => {
            for (const k of Object.keys(next)) delete next[k];
            Object.assign(next, parsed);
          }, "Edited zned manifest JSON");
          errEl.textContent = "";
          status("Manifest JSON applied");
        } catch (err) {
          errEl.textContent = err?.message ?? String(err);
        }
      };
      const dlJson = document.createElement("button");
      dlJson.type = "button";
      dlJson.textContent = "Download manifest .json";
      dlJson.onclick = () => {
        download(new Blob([JSON.stringify(m, null, 2) + "\n"], { type: "application/json" }), `${getTitle()}.zned.json`);
        status("Manifest JSON downloaded");
      };
      actions.append(applyBtn, dlJson);
      panel.append(ta, errEl, actions);
      return;
    }

    if (node.boxInfo || node.sourceBox) {
      const b = node.boxInfo;
      const sb = node.sourceBox;
      const type = b ? b.type : sb.type;
      const meta = BOX_DESC[type];
      panel.append(
        makeKV("Box 4CC", type, node.kind),
        makeKV("Payload size", b ? (b.bytes != null ? fmtBytes(b.bytes) : "computed on save") : fmtBytes(sb.data.byteLength)),
        makeKV("In Plain JXL export?", b ? (b.plain ? "Yes" : "No (.project.jxl only)") : "From source JXL container"),
        makeKV("Role", meta?.desc ?? "Preserved opaque application container box"),
      );
      if (sb || b?.rawBox) {
        const raw = sb ? sb.raw : b.rawBox.raw;
        const dl = document.createElement("button");
        dl.type = "button"; dl.textContent = `Download ${type} box (.bin)`;
        dl.onclick = () => download(new Blob([raw], { type: "application/octet-stream" }), `box-${type.trim()}.bin`);
        panel.append(dl);
      }
      return;
    }

    panel.append(makeKV("Node", node.sub || node.label, node.kind));
  }

  function render() {
    const state = getState();
    const summaryEl = $("struct-summary");
    const treeEl = $("struct-tree");
    const propsEl = $("struct-props");
    if (!treeEl || !propsEl) return;

    if (!state) {
      if (summaryEl) summaryEl.textContent = "Open an image or project to inspect its live JXL structure (Native JXL vs Prerendered vs Editor-only data).";
      treeEl.replaceChildren();
      propsEl.replaceChildren();
      return;
    }

    const boxes = plannedBoxes(state);
    const plainCount = boxes.filter(b => b.plain).length;
    const editorBoxes = boxes.filter(b => !b.plain);
    const editorTotal = editorBoxes.reduce((sum, b) => sum + (b.bytes ?? 0), 0);

    if (summaryEl) {
      summaryEl.replaceChildren();
      const plainBox = document.createElement("div");
      plainBox.className = "struct-pill-card";
      plainBox.innerHTML = `<strong>Plain JXL (.jxl)</strong><small>${plainCount} boxes · ${state.manifest.width}×${state.manifest.height} · ${isPassthrough(state) ? "Zero-loss native passthrough" : "Standard JXL presentation"}</small>`;
      const projBox = document.createElement("div");
      projBox.className = "struct-pill-card";
      projBox.innerHTML = `<strong>Project (.project.jxl)</strong><small>+${editorBoxes.length} editor boxes (${fmtBytes(editorTotal)} non-destructive assets &amp; edits)</small>`;
      summaryEl.append(plainBox, projBox);
    }

    const allNodes = buildTree(state);
    const visibleNodes = allNodes.filter(matchesFilter);
    if (!visibleNodes.some(n => n.id === selectedNodeId)) {
      selectedNodeId = visibleNodes[0]?.id ?? "doc";
    }

    treeEl.replaceChildren();
    const frag = document.createDocumentFragment();
    for (const node of visibleNodes) {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "struct-node";
      btn.style.paddingLeft = `${8 + node.depth * 14}px`;
      btn.setAttribute("aria-pressed", String(node.id === selectedNodeId));
      btn.dataset.nodeId = node.id;

      const textWrap = document.createElement("span");
      textWrap.className = "struct-node-text";
      const main = document.createElement("strong");
      main.textContent = node.label;
      textWrap.append(main);
      if (node.sub) {
        const sub = document.createElement("small");
        sub.textContent = node.sub;
        textWrap.append(sub);
      }
      btn.append(textWrap, makeBadge(node.kind));
      btn.onclick = () => {
        selectedNodeId = node.id;
        render();
      };
      li.append(btn);
      frag.append(li);
    }
    treeEl.append(frag);

    const activeNode = visibleNodes.find(n => n.id === selectedNodeId) ?? visibleNodes[0];
    renderProperties(state, activeNode);
  }

  const filterBar = $("struct-filters");
  if (filterBar) {
    for (const btn of filterBar.querySelectorAll("[data-struct-filter]")) {
      btn.addEventListener("click", () => {
        provFilter = btn.dataset.structFilter;
        for (const other of filterBar.querySelectorAll("[data-struct-filter]")) {
          other.setAttribute("aria-pressed", String(other === btn));
        }
        render();
      });
    }
  }

  return { render };
}
