import { codec } from "./codec.mjs";
import { appLog, makeConsolePanel } from "./console-log.mjs";
import { makeVaultPanel } from "./storage.mjs";
import { makeStructureInspector } from "./structure-inspector.mjs";
import { makeDecodeLab } from "./decode-check.mjs";
import { makeNativeInspector } from "./native-panel.mjs";
import {
  ZEN_FILTERS, effectiveLayerSize, hasActiveMask, hasActiveZensr, hasAdjustments,
  needsRasterPipeline, previewFilter, renderFilter,
} from "./filters.mjs";
import { makeZenFiltersPanel } from "./zenfilters-panel.mjs";
import { makeGallery, makeSvgFileFromTemplate } from "./gallery.mjs";
import { makeSvgEditor } from "./svg-editor.mjs";
import { decodeMaskJxlToAlpha, encodeMaskAlphaToJxl, makeMasksPanel } from "./masks.mjs";
import { makeZensrPanel } from "./zensr-panel.mjs";
import {
  applyCmykOffsets, buildCodingPayload, createDefaultTimeline, defaultCoding,
  isDefaultModularCoding, makeAuthorPanels, resolveLayersForFrame,
  snapshotLayerOverrides, splineCompatibilityError,
} from "./author-panels.mjs";
import {
  checkDimensions, hasManifestBox, isJxl, jxlBoxes, MAX_FILE, MAX_LAYERS, MAX_PIXELS, MAX_SIDE, MAX_SVG,
  MAX_TIMELINE_FRAMES, opaqueBoxes, RASTER_TYPES, readProject, writeImage, writeProject,
} from "./project.mjs";

appLog.installGlobalHooks();
appLog.info("app", "JPEG XL Photo Studio initialized");

const $ = id => document.getElementById(id);
const preview = $("canvas");
const stage = $("stage");
const defaults = { visible: true, opacity: 1, x: 0, y: 0, scale: 1, angle: 0,
  exposure: 0, contrast: 1, saturation: 1 };
const types = { jpg: "image/jpeg", jpeg: "image/jpeg", png: "image/png", webp: "image/webp",
  avif: "image/avif", svg: "image/svg+xml" };
const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const utf8 = new TextEncoder();
const clampPosition = value => Math.max(-32768, Math.min(32768, Math.round(value)));
const clampScale = value => Math.max(0.05, Math.min(8, Math.round(value * 100) / 100));
const normalizeAngle = deg => {
  let a = Math.round(deg);
  while (a > 180) a -= 360;
  while (a < -180) a += 360;
  return a === 0 ? 0 : a;
};
const MAX_ASSET_PIXELS = 2 * MAX_PIXELS;
const MAX_HISTORY = 32;

let history = [];
let cursor = -1;
let current;
let saved;
let selected;
let activeDocId = null;
let title = "Untitled";
let vaultPanel = null;
let structureInspector = null;
let decodeLab = null;
let zoom = null;
let lastPaintScale = 1;
let compare = false;
let comparingKey = false;
let busy = false;
let animation;
let gesture;
let sliderKey;
let dragged;
const activeTouches = new Map();
let pinchCooldown = false;
let inspectorView = "properties";
let activeFrameIndex = 0;
let timelineOpen = false;
let playbackTimer = null;
let playbackLoopCount = 0;
let selectedSplineId = null;
let splinePickMode = false;
let splinePreviewCache = { key: null, canvas: null, pending: false, token: 0 };
let cmykPreviewCache = { key: null, canvas: null, token: 0 };
const ctx = preview.getContext("2d");
const hasCanvasFilters = "filter" in ctx;

function status(message, error = false) {
  $("status").textContent = message;
  $("status").toggleAttribute("data-error", error);
}

function report(error) {
  const msg = error instanceof Error ? error.message : String(error);
  appLog.error("editor", msg, error instanceof Error ? error.stack : null);
  status(msg, true);
}

function sniffMime(bytes) {
  if (isJxl(bytes)) return "image/jxl";
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) return "image/jpeg";
  if (bytes.length >= 8 && bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47) return "image/png";
  if (bytes.length >= 12 && bytes[0] === 0x52 && bytes[1] === 0x49 && bytes[2] === 0x46 && bytes[3] === 0x46 &&
      bytes[8] === 0x57 && bytes[9] === 0x45 && bytes[10] === 0x42 && bytes[11] === 0x50) return "image/webp";
  if (bytes.length >= 12 && bytes[4] === 0x66 && bytes[5] === 0x74 && bytes[6] === 0x79 && bytes[7] === 0x70) {
    const brand = String.fromCharCode(bytes[8], bytes[9], bytes[10], bytes[11]);
    if (brand === "avif" || brand === "avis" || brand === "mif1") return "image/avif";
  }
  const head = new TextDecoder("utf-8", { fatal: false }).decode(bytes.subarray(0, 256)).trimStart();
  if (head.startsWith("<svg") || (head.startsWith("<?xml") && head.includes("<svg"))) return "image/svg+xml";
  return null;
}

function safeOpaqueBoxes(bytes) {
  try {
    return opaqueBoxes(bytes);
  } catch (err) {
    appLog.warn("jxl", `Skipped container box preservation on non-standard JXL: ${err?.message ?? err}`);
    return [];
  }
}

function newId() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 15) | 64;
  bytes[8] = (bytes[8] & 63) | 128;
  const hex = Array.from(bytes, n => n.toString(16).padStart(2, "0")).join("");
  return [hex.slice(0, 8), hex.slice(8, 12), hex.slice(12, 16), hex.slice(16, 20), hex.slice(20)].join("-");
}

function canvasFrom({ width, height, pixels }) {
  checkDimensions(width, height);
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  canvas.getContext("2d").putImageData(
    new ImageData(new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, pixels.byteLength), width, height), 0, 0);
  return canvas;
}

async function decodeSource(source, mime) {
  if (mime === "image/jxl") return canvasFrom(await codec("decode", { bytes: source.slice() }));
  if (mime === "image/svg+xml") {
    if (source.length > MAX_SVG) throw new Error("The SVG source exceeds 2 MiB.");
    text.decode(source);
  }
  const url = URL.createObjectURL(new Blob([source], { type: mime }));
  try {
    const image = new Image();
    image.src = url;
    await image.decode();
    const { naturalWidth: width, naturalHeight: height } = image;
    checkDimensions(width, height);
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    canvas.getContext("2d").drawImage(image, 0, 0);
    return canvas;
  } finally {
    URL.revokeObjectURL(url);
  }
}

async function encodeCanvas(canvas) {
  const { width, height } = canvas;
  const { data } = canvas.getContext("2d").getImageData(0, 0, width, height);
  return codec("encode", { width, height, stride: width * 4, pixels: data }, [data.buffer]);
}

async function makeLayer(file, source) {
  if (file.size > MAX_FILE) throw new Error("The source exceeds 128 MiB.");
  if (!source) {
    source = new Uint8Array(await file.arrayBuffer());
    if (isJxl(source) && readProject(source)) {
      throw new Error("Use Open image to reopen a layered JPEG XL project.");
    }
  }
  const extension = file.name.split(".").at(-1)?.toLowerCase();
  const mime = sniffMime(source) ?? types[extension] ?? file.type;
  if (![...RASTER_TYPES, "image/svg+xml"].includes(mime)) {
    throw new Error(`Unsupported image format (${file.name || "unnamed"}, type=${file.type || "unknown"}).`);
  }
  const canvas = await decodeSource(source, mime);
  const layer = { id: newId(), name: file.name.slice(0, 255), mime,
    width: canvas.width, height: canvas.height, ...defaults };
  return { layer, asset: { source, canvas, rendition: null } };
}

function checkAssets(layers, assets) {
  let pixels = 0;
  let bytes = 0;
  const seenSources = new Set();
  for (const layer of layers) {
    pixels += layer.width * layer.height;
    const src = assets.get(layer.id).source;
    if (!seenSources.has(src)) {
      bytes += src.byteLength;
      seenSources.add(src);
    }
  }
  if (pixels > MAX_ASSET_PIXELS || bytes > MAX_FILE) {
    throw new Error("The document exceeds the combined layer size limit.");
  }
}

function currentLayers(state = current, frameIdx = activeFrameIndex) {
  if (!state) return [];
  return resolveLayersForFrame(state.manifest, frameIdx);
}

function activeLayer() {
  return currentLayers().find(layer => layer.id === selected);
}

function svgSource(asset) {
  return asset.svgEdit ?? asset.source;
}

function originalLayer() {
  return current?.manifest.layers.find(layer => layer.id === current.manifest.originalId)
    ?? current?.manifest.layers[0];
}

function stopPlayback() {
  if (playbackTimer !== null) {
    clearTimeout(playbackTimer);
    playbackTimer = null;
    update();
  }
}

function togglePlayback() {
  if (playbackTimer !== null) {
    stopPlayback();
    return;
  }
  const tl = current?.manifest.timeline;
  if (!tl || tl.frames.length < 2) return;
  const playable = tl.frames.map((f, i) => ({ f, i })).filter(({ f }) => !f.referenceOnly);
  if (playable.length < 2) return;
  playbackLoopCount = 0;
  let pos = Math.max(0, playable.findIndex(({ i }) => i === activeFrameIndex));
  const step = () => {
    const curTl = current?.manifest.timeline;
    if (!curTl || curTl.frames.length < 2) { stopPlayback(); return; }
    const frames = curTl.frames.map((f, i) => ({ f, i })).filter(({ f }) => !f.referenceOnly);
    if (frames.length < 2) { stopPlayback(); return; }
    pos = pos % frames.length;
    const { f, i } = frames[pos];
    activeFrameIndex = i;
    update();
    const ms = Math.max(16, Math.round(f.duration * 1000 * curTl.tpsDenominator / curTl.tpsNumerator));
    playbackTimer = setTimeout(() => {
      pos++;
      if (pos >= frames.length) {
        playbackLoopCount++;
        if (curTl.loops > 0 && playbackLoopCount >= curTl.loops) {
          playbackTimer = null;
          update();
          return;
        }
        pos = 0;
      }
      step();
    }, ms);
  };
  step();
}

function storeState(state, label) {
  history.splice(cursor + 1);
  history.push({ state, label });
  if (history.length > MAX_HISTORY) history.shift();
  goTo(history.length - 1);
  if (history.length > 1) vaultPanel?.scheduleAutoSave(label);
}

function goTo(index) {
  if (index < 0 || index >= history.length) return;
  stopPlayback();
  cursor = index;
  current = history[index].state;
  const tl = current.manifest.timeline;
  if (tl?.frames?.length) {
    activeFrameIndex = Math.max(0, Math.min(activeFrameIndex, tl.frames.length - 1));
  } else {
    activeFrameIndex = 0;
  }
  if (!activeLayer()) selected = current.manifest.layers.at(-1).id;
  if (current.manifest.cmyk?.enabled) refreshCmykPreview();
  update();
}

function install(state, label, newTitle, alreadySaved, docIdOverride = null) {
  stopPlayback();
  history = [];
  cursor = -1;
  zoom = null;
  lastPaintScale = 1;
  gesture = null;
  sliderKey = null;
  activeTouches.clear();
  pinchCooldown = false;
  compare = false;
  comparingKey = false;
  activeFrameIndex = 0;
  timelineOpen = Boolean(state.manifest.timeline?.enabled);
  selectedSplineId = state.manifest.splines?.[0]?.id ?? null;
  splinePickMode = false;
  masksPanel.stopBrush();
  splinePreviewCache = { key: null, canvas: null, pending: false, token: 0 };
  cmykPreviewCache = { key: null, canvas: null, token: 0 };
  activeDocId = docIdOverride || newId();
  showInspector("properties");
  selected = state.manifest.layers.at(-1).id;
  title = newTitle.replace(/\.(?:project\.jxl|jxl|jpg|jpeg|png|webp|svg|avif)$/i, "");
  saved = alreadySaved ? state : null;
  storeState(state, label);
  zenFiltersPanel.loadCatalog();
  vaultPanel?.refreshUI();
}

async function importNativeTimeline(asset, sourceName) {
  const decoded = await codec("nativeTimeline", { bytes: asset.source.slice() });
  const { info, width, height, pixels } = decoded;
  const frames = info.frames;
  if (!frames?.length) throw new Error("No animation frames found in JPEG XL source.");
  if (frames.length > MAX_LAYERS) {
    throw new Error(`Importing animation directly into layers supports up to ${MAX_LAYERS} frames.`);
  }
  const frameBytes = width * height * 4;
  const layers = [];
  const assets = new Map();
  const tlFrames = [];
  for (let i = 0; i < frames.length; i++) {
    const f = frames[i];
    const id = newId();
    const slice = pixels.subarray(i * frameBytes, (i + 1) * frameBytes);
    const canvas = canvasFrom({ width, height, pixels: slice });
    const frameSource = await encodeCanvas(canvas);
    const layer = {
      id,
      name: (f.name || `Frame ${i + 1}`).slice(0, 255),
      mime: "image/jxl",
      width,
      height,
      ...defaults,
      visible: i === 0,
    };
    layers.push(layer);
    assets.set(id, { source: frameSource, canvas, rendition: null });
    tlFrames.push({
      id: newId(),
      name: (f.name || `Frame ${i + 1}`).slice(0, 1019),
      duration: f.durationTicks || 100,
      timecode: f.timecode ?? null,
      blendMode: "auto",
      blendSource: null,
      saveAsReference: null,
      referenceOnly: false,
      layerOverrides: {},
    });
  }
  for (let i = 0; i < tlFrames.length; i++) {
    const overrides = {};
    for (let j = 0; j < layers.length; j++) {
      overrides[layers[j].id] = { ...defaults, visible: j === i };
    }
    tlFrames[i].layerOverrides = overrides;
  }
  const anim = info.animation ?? { tpsNumerator: 1000, tpsDenominator: 1, loops: 0 };
  const manifest = {
    version: 1,
    filterEngine: ZEN_FILTERS,
    width,
    height,
    originalId: layers[0].id,
    layers,
    timeline: {
      enabled: true,
      tpsNumerator: anim.tpsNumerator,
      tpsDenominator: anim.tpsDenominator,
      loops: anim.loops,
      frames: tlFrames,
    },
    coding: defaultCoding(),
  };
  install({ manifest, assets, preserved: safeOpaqueBoxes(asset.source) }, "Imported native animation", sourceName, false);
  timelineOpen = true;
  update();
  status(`Imported ${frames.length} animation frames to timeline`);
}

async function loadNativeCmyk(asset, sourceName) {
  const extracted = await codec("nativeCmyk", { bytes: asset.source.slice() });
  const state = current;
  const nextManifest = structuredClone(state.manifest);
  if (nextManifest.timeline) nextManifest.timeline.enabled = false;
  nextManifest.cmyk = {
    enabled: true,
    bitDepth: extracted.bitDepth,
    mode: extracted.usesOriginalProfile ? "modular" : "vardct",
    distance: 1.0,
    effort: extracted.usesOriginalProfile ? 3 : 4,
    iccName: `${(sourceName || "source").replace(/\.jxl$/i, "")}.icc`,
    origin: "source",
    channelOffsets: [0, 0, 0, 0],
  };
  const cmykData = { samples: extracted.samples, icc: extracted.icc };
  storeState({ ...state, manifest: nextManifest, cmykData }, "Loaded native CMYK samples");
  await refreshCmykPreview();
  showInspector("coding");
  status(`Loaded exact ${extracted.bitDepth}-bit CMYK samples (${extracted.icc.byteLength} B ICC)`);
}

async function renderFlattenedCanvas(state, frameIndex = activeFrameIndex) {
  const canvas = document.createElement("canvas");
  canvas.width = state.manifest.width;
  canvas.height = state.manifest.height;
  const layers = resolveLayersForFrame(state.manifest, frameIndex);
  const rasters = new Map();
  if (state.manifest.filterEngine === ZEN_FILTERS) {
    for (const layer of layers) {
      const asset = state.assets.get(layer.id);
      if (layer.visible && layer.opacity && needsRasterPipeline(layer, asset)) {
        rasters.set(layer.id, await renderFilter(asset, layer, { rubyOverlay: false }));
      }
    }
  }
  drawLayers(canvas.getContext("2d"), state, canvas.width, canvas.height, false, rasters, frameIndex);
  return { canvas, rasters };
}

async function enableCmykFromCanvas(bitDepth) {
  const state = current;
  if (!state) return;
  let samples = state.cmykData?.samples;
  let icc = state.cmykData?.icc;
  let iccName = state.manifest.cmyk?.iccName;
  let origin = state.manifest.cmyk?.origin ?? "separated";

  if (!samples || state.manifest.cmyk?.bitDepth !== bitDepth) {
    const { canvas } = await renderFlattenedCanvas(state);
    const { data } = canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height);
    samples = await codec("rgbaToCmyk", { pixels: data, bitDepth }, [data.buffer]);
    origin = "separated";
  }
  if (!icc) {
    const orig = originalLayer();
    if (orig?.mime === "image/jxl") {
      try {
        const srcIcc = await codec("nativeIcc", { bytes: state.assets.get(orig.id).source.slice() });
        if (srcIcc.length >= 128 && String.fromCharCode(...srcIcc.subarray(16, 20)) === "CMYK") {
          icc = srcIcc;
          iccName = "Source embedded CMYK ICC";
        }
      } catch {
        // Source has no CMYK ICC; fall back to built-in subtractive ICC.
      }
    }
    if (!icc) {
      icc = await codec("minimalCmykIcc");
      iccName = "Simple Subtractive CMYK (built-in)";
    }
  }
  const nextManifest = structuredClone(state.manifest);
  if (nextManifest.timeline) nextManifest.timeline.enabled = false;
  nextManifest.cmyk = {
    enabled: true,
    bitDepth,
    mode: nextManifest.cmyk?.mode ?? "modular",
    distance: nextManifest.cmyk?.distance ?? 1.0,
    effort: nextManifest.cmyk?.effort ?? 3,
    iccName: iccName || "CMYK ICC",
    origin,
    channelOffsets: nextManifest.cmyk?.channelOffsets ?? [0, 0, 0, 0],
  };
  storeState({ ...state, manifest: nextManifest, cmykData: { samples, icc } }, `Enabled ${bitDepth}-bit CMYK output`);
  await refreshCmykPreview();
  status(`CMYK ${bitDepth}-bit ready (${icc.byteLength} B ICC)`);
}

async function refreshCmykPreview() {
  const state = current;
  if (!state?.manifest.cmyk?.enabled || !state.cmykData?.samples?.length) return;
  const { width, height, cmyk } = state.manifest;
  const key = `${cmyk.bitDepth}:${state.cmykData.samples.byteLength}:${state.cmykData.icc.byteLength}:${(cmyk.channelOffsets ?? []).join(",")}`;
  const token = ++cmykPreviewCache.token;
  try {
    const effective = applyCmykOffsets(state.cmykData.samples, cmyk.bitDepth, cmyk.channelOffsets);
    const rgba = await codec("previewCmyk", {
      samples: effective.slice(),
      width,
      height,
      bitDepth: cmyk.bitDepth,
      icc: state.cmykData.icc.slice(),
    });
    if (token !== cmykPreviewCache.token) return;
    cmykPreviewCache = { key, canvas: canvasFrom({ width, height, pixels: rgba }), token };
    requestPaint();
  } catch (error) {
    report(error);
  }
}

async function openBytes(bytes, filename, { existingDocId = null, skipConfirm = false, labelOverride = null } = {}) {
  if (!skipConfirm && current && current !== saved && !confirm("Replace the current document and discard unsaved edits?")) return;
  if (bytes.byteLength > MAX_FILE) throw new Error("The source exceeds 128 MiB.");
  const headHex = Array.from(bytes.subarray(0, 12), b => b.toString(16).padStart(2, "0")).join(" ");
  appLog.info("open", `Opening "${filename}" (${bytes.byteLength.toLocaleString()} B, head=${headHex})`);

  let project = null;
  if (isJxl(bytes)) {
    const isProj = hasManifestBox(bytes);
    if (isProj) {
      project = readProject(bytes);
    } else {
      try {
        project = readProject(bytes);
      } catch (err) {
        appLog.warn("jxl", `Non-standard container box table on "${filename}" — decoding codestream directly: ${err?.message ?? err}`);
        project = null;
      }
    }
  }

  if (project) {
    if (project.manifest.filterEngine !== ZEN_FILTERS && !hasCanvasFilters && project.manifest.layers.some(hasAdjustments)) {
      throw new Error("This browser cannot render the project’s exposure, contrast or saturation.");
    }
    checkAssets(project.manifest.layers, project.assets);
    try {
      await codec("decode", { bytes: bytes.slice() });
    } catch (err) {
      appLog.warn("open", `Presentation codestream decode warning on "${filename}": ${err?.message ?? err}`);
      if (!project.manifest.cmyk?.enabled) throw err;
    }
    for (const layer of project.manifest.layers) {
      const asset = project.assets.get(layer.id);
      if (layer.mime === "image/svg+xml") {
        text.decode(asset.source);
        text.decode(svgSource(asset));
      }
      if (layer.nativeFrame !== undefined) {
        const frame = await codec("nativeFrame", { bytes: asset.source, index: layer.nativeFrame });
        if (!frame.preview.length) throw new Error(frame.previewError);
        const [width, height] = frame.info.displaySize;
        asset.canvas = canvasFrom({ width, height, pixels: frame.preview });
      } else {
        asset.canvas = await decodeSource(asset.rendition ?? asset.source, asset.rendition ? "image/jxl" : layer.mime);
      }
      if (layer.width !== asset.canvas.width || layer.height !== asset.canvas.height) {
        throw new Error(`Layer "${layer.name}" stored dimensions (${layer.width}×${layer.height}) do not match decoded image (${asset.canvas.width}×${asset.canvas.height}).`);
      }
      if (asset.maskJxl) {
        asset.maskAlpha = await decodeMaskJxlToAlpha(asset.maskJxl, layer.width, layer.height);
        asset.maskRev = 1;
      }
    }
    install(project, labelOverride || "Reopened project", filename, true, existingDocId);
    if (project.manifest.cmyk?.enabled) await refreshCmykPreview();
    appLog.info("open", `Reopened project "${filename}" (${project.manifest.width}×${project.manifest.height}, ${project.manifest.layers.length} layers)`);
    status("Project reopened · original assets restored");
    return;
  }

  const file = new File([bytes], filename, { type: sniffMime(bytes) || "application/octet-stream" });

  if (isJxl(bytes)) {
    let info = null;
    try {
      info = await codec("nativeInfo", { bytes: bytes.slice() });
    } catch (err) {
      appLog.warn("jxl", `nativeInfo skipped for "${filename}": ${err?.message ?? err}`);
      info = null;
    }
    const visibleAnimated = info?.animation &&
      info.frames.filter(f => f.durationTicks > 0).length >= 2 &&
      info.frames.length <= MAX_LAYERS;
    if (visibleAnimated) {
      try {
        await importNativeTimeline({ source: bytes }, filename);
        saved = current;
        appLog.info("open", `Opened animated JXL "${filename}" (${info.frames.length} frames)`);
        status("Image opened");
        return;
      } catch (err) {
        appLog.warn("jxl", `Timeline import failed on "${filename}", opening frame 1 instead: ${err?.message ?? err}`);
      }
    }
    if (info?.profile?.cmyk) {
      try {
        const extracted = await codec("nativeCmyk", { bytes: bytes.slice() });
        let canvas;
        try {
          const decoded = await decodeSource(bytes, "image/jxl");
          canvas = decoded;
        } catch {
          const rgba = await codec("previewCmyk", {
            samples: extracted.samples.slice(),
            width: extracted.width,
            height: extracted.height,
            bitDepth: extracted.bitDepth,
            icc: extracted.icc.slice(),
          });
          canvas = canvasFrom({ width: extracted.width, height: extracted.height, pixels: rgba });
        }
        const layer = {
          id: newId(),
          name: filename.slice(0, 255),
          mime: "image/jxl",
          width: canvas.width,
          height: canvas.height,
          ...defaults,
        };
        const asset = { source: bytes, canvas, rendition: null };
        const manifest = {
          version: 1,
          filterEngine: ZEN_FILTERS,
          width: layer.width,
          height: layer.height,
          originalId: layer.id,
          layers: [layer],
          cmyk: {
            enabled: true,
            bitDepth: extracted.bitDepth,
            mode: extracted.usesOriginalProfile ? "modular" : "vardct",
            distance: 1.0,
            effort: extracted.usesOriginalProfile ? 3 : 4,
            iccName: `${filename.replace(/\.jxl$/i, "")}.icc`,
            origin: "source",
            channelOffsets: [0, 0, 0, 0],
          },
        };
        const assets = new Map([[layer.id, asset]]);
        install(
          { manifest, assets, preserved: safeOpaqueBoxes(bytes), cmykData: { samples: extracted.samples, icc: extracted.icc } },
          "Opened CMYK image",
          filename,
          true,
          existingDocId,
        );
        await refreshCmykPreview();
        appLog.info("open", `Opened CMYK JXL "${filename}" (${layer.width}×${layer.height}, ${extracted.bitDepth}-bit)`);
        status("Image opened");
        return;
      } catch (err) {
        appLog.warn("jxl", `CMYK native extraction failed on "${filename}", falling back to RGBA decode: ${err?.message ?? err}`);
      }
    }
  }

  const { layer, asset } = await makeLayer(file, bytes);
  const manifest = { version: 1, filterEngine: ZEN_FILTERS, width: layer.width, height: layer.height, originalId: layer.id, layers: [layer] };
  const assets = new Map([[layer.id, asset]]);
  install({ manifest, assets, preserved: layer.mime === "image/jxl" ? safeOpaqueBoxes(bytes) : [] },
    "Opened image", filename, true, existingDocId);
  appLog.info("open", `Opened "${filename}" (${layer.width}×${layer.height}, ${layer.mime})`);
  status("Image opened");
}

async function open(file) {
  if (file.size > MAX_FILE) throw new Error("The source exceeds 128 MiB.");
  const bytes = new Uint8Array(await file.arrayBuffer());
  return openBytes(bytes, file.name);
}

async function addLayers(files) {
  if (!current) {
    const [first, ...rest] = files;
    await open(first);
    if (!rest.length) return;
    files = rest;
  }
  const { manifest, preserved, cmykData } = current;
  if (manifest.layers.length + files.length > MAX_LAYERS) throw new Error("A project can hold up to 32 layers.");
  const nextManifest = structuredClone(manifest);
  const assets = new Map(current.assets);
  for (const file of files) {
    const { layer, asset } = await makeLayer(file);
    layer.x = Math.round((nextManifest.width - layer.width) / 2);
    layer.y = Math.round((nextManifest.height - layer.height) / 2);
    nextManifest.layers.push(layer);
    assets.set(layer.id, asset);
    if (nextManifest.timeline?.frames) {
      for (const frame of nextManifest.timeline.frames) {
        if (!frame.layerOverrides) frame.layerOverrides = {};
        frame.layerOverrides[layer.id] = { ...defaults, x: layer.x, y: layer.y };
      }
    }
    checkAssets(nextManifest.layers, assets);
  }
  selected = nextManifest.layers.at(-1).id;
  compare = false;
  storeState({ manifest: nextManifest, assets, preserved, cmykData }, files.length === 1 ? "Added layer" : "Added layers");
  status(files.length === 1 ? "Layer added" : "Layers added");
}

async function importFrameFile(file) {
  const state = current;
  if (!state) return;
  if (state.manifest.layers.length >= MAX_LAYERS) throw new Error("A project can hold up to 32 layers.");
  if ((state.manifest.timeline?.frames?.length ?? 0) >= MAX_TIMELINE_FRAMES) {
    throw new Error(`Timeline supports up to ${MAX_TIMELINE_FRAMES} frames.`);
  }
  const { layer, asset } = await makeLayer(file);
  layer.x = Math.round((state.manifest.width - layer.width) / 2);
  layer.y = Math.round((state.manifest.height - layer.height) / 2);
  const nextManifest = structuredClone(state.manifest);
  if (!nextManifest.timeline) nextManifest.timeline = createDefaultTimeline(nextManifest, newId);
  nextManifest.timeline.enabled = true;
  if (nextManifest.cmyk?.enabled) nextManifest.cmyk.enabled = false;
  for (const frame of nextManifest.timeline.frames) {
    if (!frame.layerOverrides) frame.layerOverrides = snapshotLayerOverrides(nextManifest.layers);
    frame.layerOverrides[layer.id] = { ...defaults, x: layer.x, y: layer.y, visible: false };
  }
  nextManifest.layers.push(layer);
  const assets = new Map(state.assets);
  assets.set(layer.id, asset);
  checkAssets(nextManifest.layers, assets);
  const overrides = {};
  for (const l of nextManifest.layers) {
    overrides[l.id] = { ...defaults, x: l.x, y: l.y, visible: l.id === layer.id };
  }
  nextManifest.timeline.frames.push({
    id: newId(),
    name: file.name.slice(0, 255),
    duration: 250,
    timecode: null,
    blendMode: "auto",
    blendSource: null,
    saveAsReference: null,
    referenceOnly: false,
    layerOverrides: overrides,
  });
  activeFrameIndex = nextManifest.timeline.frames.length - 1;
  selected = layer.id;
  storeState({ ...state, manifest: nextManifest, assets }, "Imported image as timeline frame");
  status("Image added as new timeline frame");
}

function patchState(state, id, patch) {
  if (state.manifest.timeline?.enabled && state.manifest.timeline.frames.length) {
    const nextManifest = structuredClone(state.manifest);
    const frame = nextManifest.timeline.frames[Math.min(activeFrameIndex, nextManifest.timeline.frames.length - 1)];
    if (!frame.layerOverrides) frame.layerOverrides = snapshotLayerOverrides(nextManifest.layers);
    const base = frame.layerOverrides[id] ?? nextManifest.layers.find(l => l.id === id);
    const propPatch = {};
    const metaPatch = {};
    for (const [k, v] of Object.entries(patch)) {
      if (k in defaults) propPatch[k] = v;
      else metaPatch[k] = v;
    }
    if (Object.keys(propPatch).length && base) {
      frame.layerOverrides[id] = { ...base, ...propPatch };
    }
    if (Object.keys(metaPatch).length || activeFrameIndex === 0) {
      nextManifest.layers = nextManifest.layers.map(l => {
        if (l.id !== id) return l;
        const merged = { ...l, ...metaPatch, ...(activeFrameIndex === 0 ? propPatch : {}) };
        for (const [k, v] of Object.entries(metaPatch)) if (v === undefined) delete merged[k];
        return merged;
      });
    }
    return { ...state, manifest: nextManifest };
  }
  const layers = state.manifest.layers.map(layer => {
    if (layer.id !== id) return layer;
    const merged = { ...layer, ...patch };
    for (const [k, v] of Object.entries(patch)) if (v === undefined) delete merged[k];
    return merged;
  });
  return { ...state, manifest: { ...state.manifest, layers } };
}

function updateLayer(patch, label, id = selected) {
  if (busy || !current) return;
  const layer = currentLayers().find(entry => entry.id === id);
  if (!layer || Object.entries(patch).every(([key, value]) => JSON.stringify(layer[key]) === JSON.stringify(value))) return;
  compare = false;
  storeState(patchState(current, id, patch), label);
}

function moveLayer(id, target) {
  if (busy || !current) return;
  const layers = [...current.manifest.layers];
  const from = layers.findIndex(layer => layer.id === id);
  if (from < 0 || target < 0 || target >= layers.length || target === from) return;
  const [layer] = layers.splice(from, 1);
  layers.splice(target, 0, layer);
  selected = id;
  compare = false;
  storeState({ ...current, manifest: { ...current.manifest, layers } }, "Reordered layers");
}

function removeLayer() {
  if (busy || current.manifest.layers.length < 2) return;
  const nextManifest = structuredClone(current.manifest);
  const assets = new Map(current.assets);
  assets.delete(selected);
  nextManifest.layers = nextManifest.layers.filter(layer => layer.id !== selected);
  if (nextManifest.originalId === selected) nextManifest.originalId = null;
  if (nextManifest.timeline?.frames) {
    for (const frame of nextManifest.timeline.frames) {
      if (frame.layerOverrides) delete frame.layerOverrides[selected];
    }
  }
  compare = false;
  storeState({ ...current, manifest: nextManifest, assets }, "Deleted layer");
}

function drawLayers(context, state, width, height, original = false, rasters = new Map(), frameIdx = activeFrameIndex) {
  const { manifest, assets } = state;
  context.setTransform(1, 0, 0, 1, 0, 0);
  context.clearRect(0, 0, width, height);
  context.scale(width / manifest.width, height / manifest.height);
  if (original) {
    const layer = originalLayer();
    context.drawImage(assets.get(layer.id).canvas, 0, 0);
    return;
  }
  if (manifest.cmyk?.enabled && cmykPreviewCache.canvas) {
    context.imageSmoothingQuality = "high";
    context.drawImage(cmykPreviewCache.canvas, 0, 0, manifest.width, manifest.height);
    return;
  }
  context.imageSmoothingQuality = "high";
  const layers = resolveLayersForFrame(manifest, frameIdx);
  for (const layer of layers) {
    if (layer.visible && layer.opacity) {
      drawLayer(context, layer, rasters.get(layer.id) ?? assets.get(layer.id).canvas, manifest.filterEngine);
    }
  }
}

function drawLayer(context, layer, source, filterEngine) {
  const eff = effectiveLayerSize(layer);
  context.save();
  context.translate(layer.x, layer.y);
  context.rotate(layer.angle * Math.PI / 180);
  context.globalAlpha = layer.opacity;
  if (filterEngine !== ZEN_FILTERS && hasCanvasFilters && hasAdjustments(layer)) {
    context.filter = `brightness(${2 ** layer.exposure}) contrast(${layer.contrast}) saturate(${layer.saturation})`;
  }
  context.drawImage(source, 0, 0, eff.width * layer.scale, eff.height * layer.scale);
  context.restore();
}

function renderFrameThumb(state, frameIdx) {
  const thumb = document.createElement("canvas");
  thumb.width = thumb.height = 32;
  const { width, height } = state.manifest;
  const s = 32 / Math.max(width, height);
  const tw = Math.max(1, Math.round(width * s));
  const th = Math.max(1, Math.round(height * s));
  const off = document.createElement("canvas");
  off.width = tw;
  off.height = th;
  drawLayers(off.getContext("2d"), state, tw, th, false, new Map(), frameIdx);
  thumb.getContext("2d").drawImage(off, (32 - tw) / 2, (32 - th) / 2);
  return thumb;
}

function sampleCanvasRgb(x, y) {
  if (!current) return [24, 30, 44];
  const px = Math.max(0, Math.min(preview.width - 1, Math.round(x * preview.width / current.manifest.width)));
  const py = Math.max(0, Math.min(preview.height - 1, Math.round(y * preview.height / current.manifest.height)));
  const d = ctx.getImageData(px, py, 1, 1).data;
  return [d[0], d[1], d[2]];
}

function renderSplineOverlay(state, scale) {
  const svg = $("spline-overlay");
  const splines = state.manifest.splines ?? [];
  const show = inspectorView === "splines" && splines.length > 0 && !compare && !comparingKey;
  svg.toggleAttribute("hidden", !show);
  if (!show) return;
  svg.setAttribute("viewBox", `0 0 ${state.manifest.width} ${state.manifest.height}`);
  svg.replaceChildren();
  const ns = "http://www.w3.org/2000/svg";
  const r = Math.max(1.5, 4.5 / scale);
  const sw = Math.max(0.6, 1.4 / scale);
  for (const s of splines) {
    const isSel = s.id === selectedSplineId;
    if (s.controlPoints.length >= 2) {
      const poly = document.createElementNS(ns, "polyline");
      poly.setAttribute("fill", "none");
      poly.setAttribute("stroke", isSel ? "#a7c5ff" : "#ffffff88");
      poly.setAttribute("stroke-width", String(sw));
      poly.setAttribute("stroke-dasharray", `${sw * 3} ${sw * 2}`);
      poly.setAttribute("points", s.controlPoints.map(([x, y]) => `${x},${y}`).join(" "));
      svg.append(poly);
    }
    for (const [x, y] of s.controlPoints) {
      const c = document.createElementNS(ns, "circle");
      c.setAttribute("cx", String(x));
      c.setAttribute("cy", String(y));
      c.setAttribute("r", String(r));
      c.setAttribute("fill", isSel ? (s.colorHex || "#f5dc3c") : "#aeb7c6");
      c.setAttribute("stroke", "#12151a");
      c.setAttribute("stroke-width", String(sw));
      svg.append(c);
    }
  }
}

function paint() {
  animation = null;
  if (!current) return;
  const { width, height } = current.manifest;
  const interactiveTransform = Boolean(gesture?.handle || gesture?.pinch);
  const interactiveFilter = interactiveTransform || sliderKey === "scale" || sliderKey === "angle";
  const fit = Math.min((stage.clientWidth - 56) / width, (stage.clientHeight - 72) / height, 2);
  const scale = interactiveTransform && lastPaintScale ? lastPaintScale : Math.max(0.02, zoom ?? fit);
  lastPaintScale = scale;
  const rasterScale = Math.min(scale * window.devicePixelRatio, 1, 1800 / Math.max(width, height));
  const w = Math.max(1, Math.round(width * rasterScale));
  const h = Math.max(1, Math.round(height * rasterScale));
  if (preview.width !== w || preview.height !== h) {
    preview.width = w;
    preview.height = h;
  }
  preview.style.width = `${width * scale}px`;
  preview.style.height = `${height * scale}px`;
  preview.toggleAttribute("data-spline-pick", Boolean(splinePickMode && selectedSplineId));
  preview.toggleAttribute("data-mask-brush", Boolean(masksPanel.getBrushMode()));
  $("fit").textContent = zoom === null ? `Fit · ${Math.round(scale * 100)}%` : `${Math.round(scale * 100)}%`;
  const state = gesture?.state ?? current;
  const layers = currentLayers(state);
  const rasters = new Map();
  let pending = false;
  if (!compare && !comparingKey && !state.manifest.cmyk?.enabled && state.manifest.filterEngine === ZEN_FILTERS) {
    for (const layer of layers) {
      const asset = state.assets.get(layer.id);
      const rubyOverlay = inspectorView === "mask" && masksPanel.isRubyOverlay() && layer.id === selected;
      const filterOpts = { rubyOverlay, interactive: interactiveFilter && layer.id === selected };
      if (!layer.visible || !layer.opacity || !needsRasterPipeline(layer, asset, filterOpts)) continue;
      const filtered = previewFilter(asset, layer, requestPaint, report, filterOpts, () => {
        if (inspectorView === "zensr") zensrPanel.updateUI();
      });
      rasters.set(layer.id, filtered.canvas);
      pending ||= filtered.pending;
    }
  }
  drawLayers(ctx, state, w, h, compare || comparingKey, rasters);

  const splines = state.manifest.splines ?? [];
  const splinesActive = !compare && !comparingKey && !state.manifest.cmyk?.enabled &&
    !state.manifest.timeline?.enabled && splines.length > 0;
  if (splinesActive && !pending) {
    const scaledSplines = splines.map(s => ({
      controlPoints: s.controlPoints.map(([x, y]) => [x * w / width, y * h / height]),
      colorDct: s.colorDct,
      sigmaDct: s.sigmaDct.map((v, idx) => idx === 0 ? v * (w / width) : v),
    }));
    const key = `${w}x${h}:${activeFrameIndex}:${JSON.stringify(layers)}:${JSON.stringify(scaledSplines)}`;
    if (splinePreviewCache.key === key && splinePreviewCache.canvas) {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.drawImage(splinePreviewCache.canvas, 0, 0);
    } else if (!splinePreviewCache.pending || splinePreviewCache.key !== key) {
      pending = true;
      const token = ++splinePreviewCache.token;
      splinePreviewCache.key = key;
      splinePreviewCache.pending = true;
      const baseData = ctx.getImageData(0, 0, w, h).data;
      codec("renderSplines", { pixels: baseData, width: w, height: h, splines: scaledSplines }, [baseData.buffer])
        .then(rendered => {
          if (token !== splinePreviewCache.token) return;
          splinePreviewCache = { key, canvas: canvasFrom({ width: w, height: h, pixels: rendered }), pending: false, token };
          requestPaint();
        })
        .catch(err => {
          if (token === splinePreviewCache.token) splinePreviewCache.pending = false;
          report(err);
        });
    } else {
      pending = true;
    }
  }
  preview.toggleAttribute("data-filter-pending", pending);

  const layer = layers.find(entry => entry.id === selected);
  const outline = $("outline");
  const showOutline = Boolean(layer?.visible) && !compare && !comparingKey && !state.manifest.cmyk?.enabled &&
    !masksPanel.getBrushMode() && !splinePickMode;
  outline.hidden = !showOutline;
  const badge = $("transform-badge");
  if (showOutline) {
    const eff = effectiveLayerSize(layer);
    outline.style.width = `${eff.width * layer.scale * scale}px`;
    outline.style.height = `${eff.height * layer.scale * scale}px`;
    outline.style.transform = `translate(${layer.x * scale}px, ${layer.y * scale}px) rotate(${layer.angle}deg)`;
    if (interactiveTransform && gesture?.patch) {
      badge.hidden = false;
      const parts = [];
      if (gesture.pinch || gesture.kind !== "rotate") parts.push(`${Math.round(layer.scale * 100)}%`);
      if (gesture.pinch || gesture.kind === "rotate") parts.push(`${layer.angle}°`);
      badge.textContent = parts.join(" · ");
      badge.style.transform = `translate(-50%, -50%) rotate(${-layer.angle}deg)`;
    } else {
      badge.hidden = true;
    }
  } else {
    badge.hidden = true;
  }
  renderSplineOverlay(state, scale);
  $("compare").setAttribute("aria-pressed", String(compare || comparingKey));
}

function requestPaint() {
  if (!animation) animation = requestAnimationFrame(paint);
}

const adjustments = [
  ["opacity", "Opacity", 0, 1, 0.01, n => `${Math.round(n * 100)}%`],
  ["exposure", "Exposure", -2, 2, 0.01, n => `${n.toFixed(2)} EV`],
  ["contrast", "Contrast", 0, 2, 0.01, n => `${Math.round(n * 100)}%`],
  ["saturation", "Saturation", 0, 2, 0.01, n => `${Math.round(n * 100)}%`],
  ["scale", "Scale", 0.05, 8, 0.01, n => `${Math.round(n * 100)}%`],
  ["angle", "Rotation", -180, 180, 1, n => `${n}°`],
];
for (const [key, label, min, max, step, format] of adjustments) {
  const element = document.createElement("label");
  element.className = "adjustment";
  const heading = document.createElement("span");
  const output = document.createElement("output");
  output.id = `value-${key}`;
  heading.append(label, output);
  const input = document.createElement("input");
  input.type = "range";
  input.id = `adjust-${key}`;
  input.min = min;
  input.max = max;
  input.step = step;
  input.setAttribute("aria-label", label);
  input.addEventListener("input", () => {
    if (!current || busy) return;
    compare = false;
    sliderKey = key;
    gesture = { state: patchState(current, selected, { [key]: input.valueAsNumber }) };
    output.value = format(input.valueAsNumber);
    requestPaint();
  });
  input.addEventListener("change", () => {
    if (!current || busy) return;
    gesture = null;
    sliderKey = null;
    output.value = format(input.valueAsNumber);
    updateLayer({ [key]: input.valueAsNumber }, label);
    requestPaint();
  });
  element.append(heading, input);
  $(key === "scale" || key === "angle" ? "transforms" : "sliders").append(element);
}

function visibilityIcon(visible) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.7");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(svg.namespaceURI, "path");
  path.setAttribute("d", visible ? "M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12Z M15 12a3 3 0 1 0-6 0 3 3 0 0 0 6 0"
    : "M3 3 21 21 M2 12s3.5-6 10-6 10 6 10 6-1.3 2.3-4 4 M13.5 18H12c-6.5 0-10-6-10-6");
  svg.append(path);
  return svg;
}

const zenFiltersPanel = makeZenFiltersPanel({
  getLayer: activeLayer,
  getBusy: () => busy,
  previewPatch: patch => {
    if (!current || busy) return;
    compare = false;
    gesture = { state: patchState(current, selected, patch) };
    requestPaint();
  },
  commitPatch: (patch, label) => updateLayer(patch, label),
  clearPreview: () => { gesture = null; },
});

const masksPanel = makeMasksPanel({
  getState: () => current,
  getLayer: activeLayer,
  getBusy: () => busy,
  storeState,
  patchState,
  requestPaint,
  run,
  status,
  onBrushModeChange: mode => {
    if (mode) splinePickMode = false;
    requestPaint();
  },
});

const zensrPanel = makeZensrPanel({
  getState: () => current,
  getLayer: activeLayer,
  getBusy: () => busy,
  storeState,
  patchState,
  addLayerFiles: addLayers,
  openFile: open,
  run,
  status,
});

const gallery = makeGallery({
  openFile: open,
  addLayerFiles: addLayers,
  hasDocument: () => Boolean(current),
  getBusy: () => busy,
  run,
});

const svgEditor = makeSvgEditor({
  getActiveSvgMarkup: () => text.decode(svgSource(current.assets.get(selected))),
  getBusy: () => busy,
  async applySvgMarkup(markup, errorEl, dialog) {
    const asset = current.assets.get(selected);
    if (markup === text.decode(svgSource(asset)).replace(/\r\n?/g, "\n")) {
      dialog.close();
      return;
    }
    const source = utf8.encode(markup);
    busy = true;
    $("svg-apply").disabled = $("svg-cancel").disabled = true;
    try {
      const canvas = await decodeSource(source, "image/svg+xml");
      const prevLayer = activeLayer();
      const state = patchState(current, selected, { width: canvas.width, height: canvas.height });
      state.assets = new Map(state.assets);
      let nextMaskAlpha = asset.maskAlpha;
      let nextMaskJxl = asset.maskJxl;
      if (nextMaskAlpha && (prevLayer.width !== canvas.width || prevLayer.height !== canvas.height)) {
        const resampled = new Uint8Array(canvas.width * canvas.height);
        for (let y = 0; y < canvas.height; y++) {
          const sy = Math.min(prevLayer.height - 1, Math.floor((y + 0.5) * prevLayer.height / canvas.height));
          for (let x = 0; x < canvas.width; x++) {
            const sx = Math.min(prevLayer.width - 1, Math.floor((x + 0.5) * prevLayer.width / canvas.width));
            resampled[y * canvas.width + x] = nextMaskAlpha[sy * prevLayer.width + sx];
          }
        }
        nextMaskAlpha = resampled;
        nextMaskJxl = null;
      }
      state.assets.set(selected, {
        ...asset,
        svgEdit: source,
        canvas,
        rendition: null,
        maskAlpha: nextMaskAlpha,
        maskJxl: nextMaskJxl,
        maskRev: (asset.maskRev ?? 0) + 1,
      });
      checkAssets(state.manifest.layers, state.assets);
      storeState(state, "Edited SVG source");
      dialog.close();
      status("SVG rendered");
    } catch (error) {
      errorEl.textContent = error.message;
    } finally {
      busy = false;
      $("svg-apply").disabled = $("svg-cancel").disabled = false;
      update();
    }
  },
});

const authorPanels = makeAuthorPanels({
  getState: () => current,
  getBusy: () => busy,
  getFrameIndex: () => activeFrameIndex,
  setFrameIndex: idx => { activeFrameIndex = idx; update(); },
  getSelectedSpline: () => selectedSplineId,
  setSelectedSpline: id => { selectedSplineId = id; update(); },
  getSplinePickMode: () => splinePickMode,
  setSplinePickMode: v => { splinePickMode = v; if (v) masksPanel.stopBrush(); update(); },
  getTimelineOpen: () => timelineOpen,
  setTimelineOpen: v => { timelineOpen = v; update(); },
  newId,
  storeState,
  run,
  report,
  status,
  stopPlayback,
  togglePlayback,
  isPlaying: () => playbackTimer !== null,
  importFrameFile,
  enableCmykFromCanvas,
  refreshCmykPreview,
  sampleCanvasRgb,
  renderFrameThumb,
});

function update() {
  const hasDoc = Boolean(current);
  const layers = currentLayers();
  const layer = activeLayer();
  const index = layers.indexOf(layer);
  $("title").textContent = title;
  $("dirty").hidden = !hasDoc || current === saved;
  $("empty").hidden = hasDoc;
  $("canvas-wrap").hidden = !hasDoc;
  $("dimensions").textContent = hasDoc ? `${current.manifest.width} × ${current.manifest.height} px` : "";
  for (const id of ["save", "export", "compare", "fit", "zoom-in", "zoom-out", "mask-tab", "zensr-tab", "vault-save-snapshot", "vault-save-copy"]) {
    $(id).disabled = !hasDoc || busy;
  }
  for (const id of ["open", "add", "add-panel", "add-gallery", "new-svg", "open-gallery", "empty-open", "sample", "empty-gallery"]) {
    $(id).disabled = busy;
  }
  $("undo").disabled = cursor < 1 || busy;
  $("redo").disabled = cursor >= history.length - 1 || busy;
  $("raise").disabled = !layer || index === layers.length - 1 || busy;
  $("lower").disabled = index < 1 || busy;
  if ($("duplicate-layer")) $("duplicate-layer").disabled = !layer || layers.length >= MAX_LAYERS || busy;
  $("remove").disabled = layers.length < 2 || busy;
  $("edit-svg").hidden = layer?.mime !== "image/svg+xml";

  // Canva contextual bar above canvas
  const ctxBar = $("context-bar");
  if (ctxBar) {
    ctxBar.hidden = !hasDoc || !layer;
    if (hasDoc && layer) {
      $("ctx-layer-name").textContent = layer.name;
      $("ctx-edit-svg").hidden = layer.mime !== "image/svg+xml";
      $("ctx-upscale").hidden = layer.mime === "image/svg+xml";
      $("ctx-dup").disabled = layers.length >= MAX_LAYERS || busy;
      const provBtn = $("ctx-prov");
      const asset = current.assets.get(layer.id);
      const isNativeLayer = originalPassthrough(current) || current.manifest.cmyk?.enabled || current.manifest.timeline?.enabled || Boolean(current.manifest.splines?.length);
      const isSr = hasActiveZensr(layer);
      const isFiltered = hasAdjustments(layer) || hasActiveMask(layer, asset);
      if (isNativeLayer && !isFiltered && !isSr && layer.mime === "image/jxl") {
        provBtn.textContent = current.manifest.cmyk?.enabled ? "NATIVE CMYK" : current.manifest.timeline?.enabled ? "NATIVE ANIM" : "NATIVE JXL";
        provBtn.className = "prov-badge prov-native ctx-prov-btn";
      } else if (layer.mime === "image/svg+xml") {
        provBtn.textContent = "SVG → PRERENDERED";
        provBtn.className = "prov-badge prov-prerendered ctx-prov-btn";
      } else if (isSr) {
        provBtn.textContent = `ZENSR ${layer.zensr.scale}× · PRERENDERED`;
        provBtn.className = "prov-badge prov-prerendered ctx-prov-btn";
      } else if (isFiltered || layer.scale !== 1 || layer.angle !== 0 || layers.length > 1) {
        provBtn.textContent = "PRERENDERED + EDITOR";
        provBtn.className = "prov-badge prov-prerendered ctx-prov-btn";
      } else {
        provBtn.textContent = "SOURCE + JXL";
        provBtn.className = "prov-badge prov-native ctx-prov-btn";
      }
    }
  }
  if ($("rail-upscale")) $("rail-upscale").disabled = !hasDoc || busy;
  if ($("rail-mask")) $("rail-mask").disabled = !hasDoc || busy;
  if ($("rail-animate")) $("rail-animate").disabled = !hasDoc || busy;
  if ($("rail-animate")) $("rail-animate").setAttribute("aria-pressed", String(Boolean(timelineOpen)));
  $("fields").disabled = !layer || busy;
  $("layer-name").value = layer?.name ?? "";
  $("position-x").value = layer?.x ?? "";
  $("position-y").value = layer?.y ?? "";
  for (const [key, , , , , format] of adjustments) {
    const input = $(`adjust-${key}`);
    const value = layer?.[key] ?? defaults[key];
    input.value = value;
    input.disabled = current?.manifest.filterEngine !== ZEN_FILTERS && !hasCanvasFilters && ["exposure", "contrast", "saturation"].includes(key);
    $(`value-${key}`).value = format(value);
  }
  zenFiltersPanel.sync(layer);
  masksPanel.updateUI();
  zensrPanel.updateUI();
  structureInspector?.render();
  decodeLab?.updateButtons(hasDoc, busy);

  const nativeSource = layer?.mime === "image/jxl" ? layer : originalLayer();
  const nativeAsset = nativeSource?.mime === "image/jxl" && current.assets.get(nativeSource.id);
  $("native-tab").disabled = !nativeAsset;
  const sourceChanged = nativeInspector.setSource(nativeAsset || null, nativeSource?.name);
  nativeInspector.setBusy(busy);
  if (!nativeAsset && inspectorView === "native") showInspector("properties");
  if (!hasDoc && ["mask", "zensr", "coding", "splines"].includes(inspectorView)) showInspector("properties");
  if (nativeAsset && sourceChanged && inspectorView === "native") queueMicrotask(nativeInspector.inspect);

  authorPanels.updateUI();

  const layerList = $("layer-list");
  const focused = layerList.contains(document.activeElement);
  layerList.replaceChildren();
  for (const entry of [...layers].reverse()) {
    const row = document.createElement("li");
    row.className = "layer";
    row.dataset.id = entry.id;
    row.toggleAttribute("data-selected", entry.id === selected);
    row.toggleAttribute("data-layer-hidden", !entry.visible);
    row.draggable = !busy;
    row.addEventListener("dragstart", event => {
      dragged = entry.id;
      event.dataTransfer.setData("text/plain", entry.id);
      event.dataTransfer.effectAllowed = "move";
    });
    row.addEventListener("dragend", () => { dragged = null; });
    row.addEventListener("dragover", event => { if (dragged) event.preventDefault(); });
    row.addEventListener("drop", event => {
      if (!dragged) return;
      event.preventDefault();
      moveLayer(dragged, current.manifest.layers.findIndex(l => l.id === entry.id));
      dragged = null;
    });
    const visible = document.createElement("button");
    visible.type = "button";
    visible.className = "visibility";
    visible.setAttribute("aria-label", `${entry.visible ? "Hide" : "Show"} ${entry.name}`);
    visible.disabled = busy;
    visible.append(visibilityIcon(entry.visible));
    visible.addEventListener("click", () => updateLayer({ visible: !entry.visible }, entry.visible ? "Hid layer" : "Showed layer", entry.id));
    const pick = document.createElement("button");
    pick.type = "button";
    pick.className = "layer-pick";
    pick.setAttribute("aria-pressed", String(entry.id === selected));
    pick.disabled = busy;
    const thumbnail = document.createElement("canvas");
    thumbnail.width = thumbnail.height = 32;
    const s = 32 / Math.max(entry.width, entry.height);
    const thumbWidth = entry.width * s;
    const thumbHeight = entry.height * s;
    thumbnail.getContext("2d").drawImage(current.assets.get(entry.id).canvas,
      (32 - thumbWidth) / 2, (32 - thumbHeight) / 2, thumbWidth, thumbHeight);
    const name = document.createElement("span");
    name.textContent = entry.name;
    const detail = document.createElement("small");
    const tags = [];
    if (entry.mime === "image/svg+xml") tags.push("SVG");
    else if (entry.nativeFrame !== undefined) tags.push(`Native JXL frame ${entry.nativeFrame + 1}`);
    else tags.push(`${entry.width} × ${entry.height}`);
    if (hasActiveMask(entry, current.assets.get(entry.id))) tags.push("Mask");
    if (hasActiveZensr(entry)) tags.push("ZenSR");
    detail.textContent = tags.join(" · ");
    name.append(detail);
    pick.append(thumbnail, name);
    pick.addEventListener("click", () => { selected = entry.id; compare = false; update(); });
    row.append(visible, pick);
    layerList.append(row);
    if (focused && entry.id === selected) pick.focus({ preventScroll: true });
  }
  $("history").replaceChildren();
  for (let i = history.length - 1; i >= 0; i--) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    button.type = "button";
    button.disabled = busy;
    button.textContent = history[i].label;
    if (i === cursor) button.setAttribute("aria-current", "step");
    button.addEventListener("click", () => goTo(i));
    item.append(button);
    $("history").append(item);
  }
  requestPaint();
}

async function run(message, operation) {
  if (busy) return;
  busy = true;
  update();
  status(message);
  try {
    await operation();
  } catch (error) {
    report(error);
  } finally {
    busy = false;
    update();
  }
}

function download(blob, filename) {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function originalPassthrough(state) {
  const { originalId, layers, width, height, timeline, splines, coding, cmyk } = state.manifest;
  if (timeline?.enabled || splines?.length || cmyk?.enabled || !isDefaultModularCoding(coding)) return false;
  const [layer] = layers;
  const asset = state.assets.get(layer.id);
  if (hasAdjustments(layer) || hasActiveMask(layer, asset) || hasActiveZensr(layer)) return false;
  return layers.length === 1 && layer.id === originalId && layer.mime === "image/jxl" &&
    width === layer.width && height === layer.height &&
    Object.entries(defaults).every(([key, value]) => layer[key] === value);
}

async function encodePresentation(state, includeLayers) {
  const { width, height, timeline, splines, coding, cmyk } = state.manifest;

  if (cmyk?.enabled) {
    const effective = applyCmykOffsets(state.cmykData.samples, cmyk.bitDepth, cmyk.channelOffsets);
    return codec("encodeCmyk", {
      samples: effective.slice(),
      width,
      height,
      bitDepth: cmyk.bitDepth,
      icc: state.cmykData.icc.slice(),
      options: { mode: cmyk.mode, distance: cmyk.distance, effort: cmyk.effort },
    });
  }

  if (timeline?.enabled && timeline.frames?.length) {
    const frameBytes = width * height * 4;
    const total = frameBytes * timeline.frames.length;
    if (total > 64_000_000) throw new Error("Timeline exceeds the 64 MB uncompressed frame budget.");
    const pixels = new Uint8Array(total);
    for (let i = 0; i < timeline.frames.length; i++) {
      const { canvas } = await renderFlattenedCanvas(state, i);
      const { data } = canvas.getContext("2d").getImageData(0, 0, width, height);
      pixels.set(data, i * frameBytes);
    }
    const codingPayload = buildCodingPayload(state.manifest);
    delete codingPayload.splines;
    delete codingPayload.bakeSplines;
    const options = {
      tpsNumerator: timeline.tpsNumerator,
      tpsDenominator: timeline.tpsDenominator,
      loops: timeline.loops,
      frames: timeline.frames.map(f => ({
        name: f.name || undefined,
        duration: f.duration,
        timecode: f.timecode ?? undefined,
        blendMode: f.blendMode && f.blendMode !== "auto" ? f.blendMode : undefined,
        blendSource: f.blendSource ?? undefined,
        saveAsReference: f.saveAsReference ?? undefined,
        referenceOnly: Boolean(f.referenceOnly),
      })),
      coding: codingPayload,
    };
    return codec("encodeTimeline", { pixels, width, height, options }, [pixels.buffer]);
  }

  const { canvas, rasters } = await renderFlattenedCanvas(state, 0);
  const context = canvas.getContext("2d");
  const { data: expected } = context.getImageData(0, 0, width, height);

  if (splines?.length) {
    const compat = splineCompatibilityError(state.manifest);
    if (compat) throw new Error(compat);
  }
  const useConfigured = !isDefaultModularCoding(coding) || Boolean(splines?.length);
  if (useConfigured) {
    const options = buildCodingPayload(state.manifest);
    return codec("encodeConfigured", { pixels: expected, width, height, options }, [expected.buffer]);
  }

  const visible = state.manifest.layers.filter(layer => layer.visible && layer.opacity > 0);
  if (includeLayers && visible.length >= 2 && visible.length <= 8 &&
      width * height * visible.length <= 16_000_000) {
    const pixels = new Uint8Array(expected.byteLength * visible.length);
    context.clearRect(0, 0, width, height);
    visible.forEach((layer, index) => {
      drawLayer(context, layer, rasters.get(layer.id) ?? state.assets.get(layer.id).canvas, state.manifest.filterEngine);
      const cumulative = index + 1 === visible.length ? expected : context.getImageData(0, 0, width, height).data;
      pixels.set(cumulative, index * expected.byteLength);
    });
    const ids = visible.map(layer => layer.id).join(",");
    const layered = await codec("encodeLayers", { pixels, expected, width, height, ids }, [pixels.buffer]);
    if (layered) return layered;
  }
  return codec("encode", { width, height, stride: width * 4, pixels: expected }, [expected.buffer]);
}

async function buildProjectBlobAndBytes(state, includeLayers = true) {
  let flattened;
  let preserved = state.preserved;
  if (originalPassthrough(state)) {
    flattened = state.assets.get(state.manifest.originalId).source;
    const sourceBoxes = safeOpaqueBoxes(flattened);
    preserved = preserved.filter(entry => {
      const index = sourceBoxes.findIndex(box =>
        box.type === entry.type && box.data.length === entry.data.length &&
        box.data.every((byte, offset) => byte === entry.data[offset]));
      if (index < 0) return true;
      sourceBoxes.splice(index, 1);
      return false;
    });
  } else {
    flattened = await encodePresentation(state, includeLayers);
  }
  const assets = new Map(state.assets);
  for (const layer of state.manifest.layers) {
    let asset = assets.get(layer.id);
    let updated = null;
    if (layer.mime === "image/svg+xml" && !asset.rendition) {
      updated = { ...(updated ?? asset), rendition: await encodeCanvas(asset.canvas) };
    }
    if (asset.maskAlpha && !asset.maskJxl) {
      updated = { ...(updated ?? asset), maskJxl: await encodeMaskAlphaToJxl(asset.maskAlpha, layer.width, layer.height) };
    }
    if (updated) assets.set(layer.id, updated);
  }
  const cmykPayload = state.manifest.cmyk?.enabled ? state.cmykData : null;
  const blob = writeProject(flattened, state.manifest, assets, preserved, cmykPayload);
  return { blob, flattened, preserved };
}

async function buildPlainJxlBlob(state) {
  let flattened;
  let preserved = state.preserved;
  if (originalPassthrough(state)) {
    flattened = state.assets.get(state.manifest.originalId).source;
    const sourceBoxes = safeOpaqueBoxes(flattened);
    preserved = preserved.filter(entry => {
      const index = sourceBoxes.findIndex(box =>
        box.type === entry.type && box.data.length === entry.data.length &&
        box.data.every((byte, offset) => byte === entry.data[offset]));
      if (index < 0) return true;
      sourceBoxes.splice(index, 1);
      return false;
    });
  } else {
    flattened = await encodePresentation(state, false);
  }
  return writeImage(flattened, preserved);
}

async function saveProject(project) {
  stopPlayback();
  const state = current;
  if (project) {
    const { blob } = await buildProjectBlobAndBytes(state, true);
    const projectBytes = new Uint8Array(await blob.arrayBuffer());
    download(blob, `${title}.project.jxl`);
    saved = state;
    appLog.info("save", `Saved "${title}.project.jxl" (${projectBytes.byteLength.toLocaleString()} B)`);
    vaultPanel?.recordExplicitVersion("save", `Saved ${title}.project.jxl`, projectBytes);
    status("Project saved · original assets embedded");
  } else {
    const blob = await buildPlainJxlBlob(state);
    download(blob, `${title}.jxl`);
    appLog.info("export", `Exported "${title}.jxl" (${blob.size.toLocaleString()} B)`);
    status("JPEG XL exported");
  }
}

function bindFile(button, input, callback) {
  $(button).addEventListener("click", () => $(input).click());
  $(input).addEventListener("change", event => {
    const files = [...event.target.files];
    event.target.value = "";
    if (files.length) run("Reading image…", () => callback(files));
  });
}
bindFile("open", "open-file", files => open(files[0]));
bindFile("add", "layer-file", addLayers);
$("add-panel").onclick = () => $("layer-file").click();
$("empty-open").onclick = () => $("open-file").click();
$("open-gallery").onclick = () => gallery.open("all");
$("empty-gallery").onclick = () => gallery.open("all");
$("add-gallery").onclick = () => gallery.open("all");
$("new-svg").onclick = () => run("Creating SVG layer…", async () => {
  const file = makeSvgFileFromTemplate("svg-title");
  if (current) await addLayers([file]);
  else await open(file);
  svgEditor.open();
});
$("sample").onclick = () => run("Opening sample…", async () => {
  const response = await fetch("./sample.jpg");
  if (!response.ok) throw new Error("The sample photo is unavailable.");
  await open(new File([await response.blob()], "Grand Prismatic Spring.jpg", { type: "image/jpeg" }));
});
$("save").onclick = () => run("Saving project…", () => saveProject(true));
$("export").onclick = () => run("Exporting JPEG XL…", () => saveProject(false));
$("source-download").onclick = () => {
  const layer = activeLayer();
  const asset = current.assets.get(layer.id);
  const suffix = layer.mime === "image/svg+xml" ? "svg" : layer.mime.split("/")[1];
  const name = /\.(jxl|jpg|jpeg|png|webp|svg|avif)$/i.test(layer.name) ? layer.name : `${layer.name}.${suffix}`;
  download(new Blob([asset.source], { type: layer.mime }), name);
  status("Original layer source downloaded");
};
$("undo").onclick = () => goTo(cursor - 1);
$("redo").onclick = () => goTo(cursor + 1);
function duplicateActiveLayer() {
  const layer = activeLayer();
  if (!layer || !current || busy) return;
  if (current.manifest.layers.length >= MAX_LAYERS) {
    report(new Error(`A project can hold up to ${MAX_LAYERS} layers.`));
    return;
  }
  const nextManifest = structuredClone(current.manifest);
  const copy = structuredClone(layer);
  copy.id = newId();
  copy.name = `${layer.name} copy`.slice(0, 255);
  copy.x = clampPosition(layer.x + 16);
  copy.y = clampPosition(layer.y + 16);
  const idx = nextManifest.layers.findIndex(l => l.id === layer.id);
  nextManifest.layers.splice(idx + 1, 0, copy);
  if (nextManifest.timeline?.frames) {
    for (const f of nextManifest.timeline.frames) {
      if (!f.layerOverrides) f.layerOverrides = {};
      f.layerOverrides[copy.id] = { ...defaults, x: copy.x, y: copy.y, scale: copy.scale, angle: copy.angle };
    }
  }
  const srcAsset = current.assets.get(layer.id);
  const assets = new Map(current.assets);
  assets.set(copy.id, {
    ...srcAsset,
    maskAlpha: srcAsset.maskAlpha ? srcAsset.maskAlpha.slice() : null,
  });
  checkAssets(nextManifest.layers, assets);
  selected = copy.id;
  compare = false;
  storeState({ ...current, manifest: nextManifest, assets }, "Duplicated layer");
  status("Layer duplicated");
}
$("duplicate-layer").onclick = duplicateActiveLayer;
$("ctx-dup").onclick = duplicateActiveLayer;
$("ctx-edit-svg").onclick = () => svgEditor.open();
$("ctx-rot-ccw").onclick = () => rotateActiveBy(-90);
$("ctx-rot-cw").onclick = () => rotateActiveBy(90);
$("ctx-fit").onclick = () => $("scale-fit").click();
$("ctx-upscale").onclick = () => {
  showInspector("zensr");
  $("zensr-quick-2x-fast").click();
};
$("ctx-mask").onclick = () => showInspector("mask");
$("ctx-prov").onclick = () => showInspector("structure");
$("mode-studio").onclick = () => showInspector(inspectorView === "structure" ? "properties" : inspectorView);
$("mode-structure").onclick = () => showInspector("structure");
$("rail-gallery").onclick = () => gallery.open("all");
$("rail-svg").onclick = () => {
  if (activeLayer()?.mime === "image/svg+xml") svgEditor.open();
  else $("new-svg").click();
};
$("rail-adjust").onclick = () => showInspector("properties");
$("rail-upscale").onclick = () => showInspector("zensr");
$("rail-mask").onclick = () => showInspector("mask");
$("rail-animate").onclick = () => $("timeline-toggle").click();
$("rail-structure").onclick = () => showInspector("structure");
$("rail-decode").onclick = () => showInspector("decode");
$("rail-vault").onclick = () => showInspector("vault");

$("raise").onclick = () => moveLayer(selected, current.manifest.layers.findIndex(l => l.id === selected) + 1);
$("lower").onclick = () => moveLayer(selected, current.manifest.layers.findIndex(l => l.id === selected) - 1);
$("remove").onclick = removeLayer;
$("compare").onclick = () => { compare = !compare; requestPaint(); };
$("fit").onclick = () => { zoom = null; requestPaint(); };
function changeZoom(ratio) {
  zoom = Math.min(8, Math.max(0.02, (zoom ?? preview.clientWidth / current.manifest.width) * ratio));
  requestPaint();
}
$("zoom-in").onclick = () => changeZoom(1.25);
$("zoom-out").onclick = () => changeZoom(0.8);
$("reset").onclick = () => updateLayer({
  ...defaults,
  visible: activeLayer().visible,
  filters: undefined,
  resizeKernel: undefined,
  resizeLinear: undefined,
}, "Reset adjustments");
function rotateActiveBy(delta) {
  const layer = activeLayer();
  if (!layer || busy || !current) return;
  const eff = effectiveLayerSize(layer);
  const center = layerPointToCanvas(layer, eff.width / 2, eff.height / 2);
  const nextAngle = normalizeAngle(layer.angle + delta);
  const rad = nextAngle * Math.PI / 180;
  const hw = (eff.width * layer.scale) / 2;
  const hh = (eff.height * layer.scale) / 2;
  const nx = clampPosition(center.x - (hw * Math.cos(rad) - hh * Math.sin(rad)));
  const ny = clampPosition(center.y - (hw * Math.sin(rad) + hh * Math.cos(rad)));
  updateLayer({ angle: nextAngle, x: nx, y: ny }, `Rotated ${delta > 0 ? "+" : ""}${delta}°`);
}
$("rotate-ccw").onclick = () => rotateActiveBy(-90);
$("rotate-cw").onclick = () => rotateActiveBy(90);
$("scale-fit").onclick = () => {
  const layer = activeLayer();
  if (!layer || busy || !current) return;
  const eff = effectiveLayerSize(layer);
  const { width: cw, height: ch } = current.manifest;
  const nextScale = clampScale(Math.min(cw / eff.width, ch / eff.height));
  const nx = clampPosition((cw - eff.width * nextScale) / 2);
  const ny = clampPosition((ch - eff.height * nextScale) / 2);
  updateLayer({ scale: nextScale, angle: 0, x: nx, y: ny }, "Fit layer to canvas");
};
$("scale-reset").onclick = () => {
  const layer = activeLayer();
  if (!layer || busy || !current) return;
  const eff = effectiveLayerSize(layer);
  const { width: cw, height: ch } = current.manifest;
  const nx = clampPosition((cw - eff.width) / 2);
  const ny = clampPosition((ch - eff.height) / 2);
  updateLayer({ scale: 1, angle: 0, x: nx, y: ny }, "Reset layer transform");
};
$("canvas-upscale-2x").onclick = () => {
  if (!current || busy) return;
  const nextW = current.manifest.width * 2;
  const nextH = current.manifest.height * 2;
  if (nextW > MAX_SIDE || nextH > MAX_SIDE || nextW * nextH > MAX_PIXELS) {
    report(new Error(`Upscaled canvas ${nextW}×${nextH} exceeds the 16,384 px / 64 MP limit.`));
    return;
  }
  const nextManifest = structuredClone(current.manifest);
  nextManifest.width = nextW;
  nextManifest.height = nextH;
  for (const l of nextManifest.layers) {
    l.scale = clampScale(l.scale * 2);
    l.x = clampPosition(l.x * 2);
    l.y = clampPosition(l.y * 2);
  }
  if (nextManifest.timeline?.frames) {
    for (const f of nextManifest.timeline.frames) {
      if (!f.layerOverrides) continue;
      for (const props of Object.values(f.layerOverrides)) {
        props.scale = clampScale(props.scale * 2);
        props.x = clampPosition(props.x * 2);
        props.y = clampPosition(props.y * 2);
      }
    }
  }
  storeState({ ...current, manifest: nextManifest }, `Upscaled canvas 2× (${nextW}×${nextH})`);
  appLog.info("transform", `Upscaled canvas 2× → ${nextW}×${nextH}`);
  status(`Canvas upscaled 2× (${nextW} × ${nextH} px)`);
};
$("layer-name").onchange = event => updateLayer({ name: event.target.value.trim() || activeLayer().name }, "Renamed layer");
for (const axis of ["x", "y"]) {
  $(`position-${axis}`).onchange = event => {
    const value = event.target.valueAsNumber;
    if (Number.isFinite(value)) updateLayer({ [axis]: clampPosition(value) }, "Moved layer");
    $(`position-${axis}`).value = activeLayer()[axis];
  };
}

function showInspector(view) {
  inspectorView = view;
  if (view !== "splines") splinePickMode = false;
  if (view !== "mask") masksPanel.stopBrush();
  $("history").hidden = view !== "history";
  $("structure-panel").hidden = view !== "structure";
  $("decode-panel").hidden = view !== "decode";
  $("vault-panel").hidden = view !== "vault";
  $("log-panel").hidden = view !== "log";
  $("properties").hidden = view !== "properties";
  $("mask-panel").hidden = view !== "mask";
  $("zensr-panel").hidden = view !== "zensr";
  $("coding-panel").hidden = view !== "coding";
  $("splines-panel").hidden = view !== "splines";
  $("native-panel").hidden = view !== "native";
  for (const id of ["history", "structure", "decode", "vault", "log", "properties", "mask", "zensr", "coding", "splines", "native"]) {
    $(`${id}-tab`)?.setAttribute("aria-pressed", String(view === id));
  }
  const isStruct = view === "structure";
  document.body.classList.toggle("mode-structure", isStruct);
  $("mode-studio")?.setAttribute("aria-pressed", String(!isStruct));
  $("mode-structure")?.setAttribute("aria-pressed", String(isStruct));
  $("rail-adjust")?.setAttribute("aria-pressed", String(view === "properties"));
  $("rail-upscale")?.setAttribute("aria-pressed", String(view === "zensr"));
  $("rail-mask")?.setAttribute("aria-pressed", String(view === "mask"));
  $("rail-structure")?.setAttribute("aria-pressed", String(isStruct));
  $("rail-decode")?.setAttribute("aria-pressed", String(view === "decode"));
  $("rail-vault")?.setAttribute("aria-pressed", String(view === "vault"));
  if (view === "vault") vaultPanel?.refreshUI();
  if (view === "decode") decodeLab?.refresh();
  if (isStruct) structureInspector?.render();
  requestPaint();
}

function addNativeLayer(asset, frame, index) {
  if (current.manifest.layers.length === MAX_LAYERS) throw new Error("A project can hold up to 32 layers.");
  const { nativeFrame } = activeLayer();
  const { displaySize: [width, height], displayOrigin: [x, y], name } = frame.info;
  if (Math.abs(x) > 32768 || Math.abs(y) > 32768) {
    throw new Error("This native frame’s origin exceeds the editor position range.");
  }
  const canvas = canvasFrom({ width, height, pixels: frame.preview });
  const layer = { id: newId(), name: (nativeFrame === index || !name ? `Native frame ${index + 1}` : name).slice(0, 255),
    mime: "image/jxl", width, height, ...defaults, x, y, nativeFrame: index };
  const nextManifest = structuredClone(current.manifest);
  nextManifest.layers.push(layer);
  if (nextManifest.timeline?.frames) {
    for (const f of nextManifest.timeline.frames) {
      if (!f.layerOverrides) f.layerOverrides = {};
      f.layerOverrides[layer.id] = { ...defaults, x, y };
    }
  }
  const assets = new Map(current.assets);
  assets.set(layer.id, { source: asset.source, canvas, rendition: null });
  checkAssets(nextManifest.layers, assets);
  selected = layer.id;
  compare = false;
  storeState({ ...current, manifest: nextManifest, assets }, "Added native JPEG XL frame");
  showInspector("properties");
  status("Source-local frame added to Layers");
}

const nativeInspector = makeNativeInspector({
  run, download, canvasFrom, addNativeLayer, importNativeTimeline, loadNativeCmyk, report, status,
});
makeConsolePanel({
  onOpenLog: () => showInspector("log"),
  download,
  status,
});
vaultPanel = makeVaultPanel({
  getState: () => current,
  getTitle: () => title,
  getDocId: () => activeDocId,
  setDocId: id => { activeDocId = id; },
  getBusy: () => busy,
  buildProjectBytes: async state => {
    const { blob } = await buildProjectBlobAndBytes(state, false);
    return new Uint8Array(await blob.arrayBuffer());
  },
  renderThumbDataUrl: state => {
    try {
      return renderFrameThumb(state, activeFrameIndex).toDataURL("image/png");
    } catch {
      return null;
    }
  },
  openStoredBytes: (bytes, filename, docId) => openBytes(bytes, filename, { existingDocId: docId, skipConfirm: true }),
  downloadBytes: (bytes, filename) => download(new Blob([bytes], { type: "image/jxl" }), filename),
  newId,
  run,
  status,
  showVaultTab: () => showInspector("vault"),
});
structureInspector = makeStructureInspector({
  getState: () => current,
  getSelectedLayerId: () => selected,
  selectLayer: id => {
    if (selected === id) return;
    selected = id;
    compare = false;
    update();
  },
  getTitle: () => title,
  isPassthrough: originalPassthrough,
  updateLayer,
  mutateManifest: (mutator, label) => {
    if (!current || busy) return;
    const nextManifest = structuredClone(current.manifest);
    mutator(nextManifest);
    storeState({ ...current, manifest: nextManifest }, label);
  },
  openSvgEditor: () => svgEditor.open(),
  downloadSource: () => $("source-download").click(),
  download,
  status,
  report,
});
decodeLab = makeDecodeLab({
  getState: () => current,
  getTitle: () => title,
  getSelectedLayer: () => activeLayer(),
  getBusy: () => busy,
  buildPlainJxlBytes: async state => {
    const blob = await buildPlainJxlBlob(state);
    return new Uint8Array(await blob.arrayBuffer());
  },
  buildProjectJxlBytes: async state => {
    const { blob } = await buildProjectBlobAndBytes(state, true);
    return new Uint8Array(await blob.arrayBuffer());
  },
  renderFlattenedCanvas,
  download,
  newId,
  status,
  showDecodeTab: () => showInspector("decode"),
});
$("history-tab").onclick = () => showInspector("history");
$("structure-tab").onclick = () => showInspector("structure");
$("decode-tab").onclick = () => showInspector("decode");
$("vault-tab").onclick = () => showInspector("vault");
$("log-tab").onclick = () => showInspector("log");
$("properties-tab").onclick = () => showInspector("properties");
$("mask-tab").onclick = () => showInspector("mask");
$("zensr-tab").onclick = () => showInspector("zensr");
$("coding-tab").onclick = () => showInspector("coding");
$("splines-tab").onclick = () => showInspector("splines");
$("native-tab").onclick = () => {
  showInspector("native");
  nativeInspector.inspect();
};
$("edit-svg").onclick = () => svgEditor.open();

function clientToCanvas(clientX, clientY) {
  const bounds = preview.getBoundingClientRect();
  return { x: (clientX - bounds.left) * current.manifest.width / bounds.width,
    y: (clientY - bounds.top) * current.manifest.height / bounds.height };
}

function canvasPoint(event) {
  return clientToCanvas(event.clientX, event.clientY);
}

function canvasPointToLayer(layer, cx, cy) {
  const theta = layer.angle * Math.PI / 180;
  const dx = cx - layer.x;
  const dy = cy - layer.y;
  return {
    x: (Math.cos(theta) * dx + Math.sin(theta) * dy) / layer.scale,
    y: (-Math.sin(theta) * dx + Math.cos(theta) * dy) / layer.scale,
  };
}

function hit(layer, x, y) {
  if (!layer.visible || layer.opacity <= 0) return false;
  const eff = effectiveLayerSize(layer);
  const { x: u, y: v } = canvasPointToLayer(layer, x, y);
  return u >= 0 && v >= 0 && u < eff.width && v < eff.height;
}

// Direct on-canvas corner resize & rotation handles + 2-finger touch pinch/rotate
function layerPointToCanvas(layer, lx, ly) {
  const rad = layer.angle * Math.PI / 180;
  const cos = Math.cos(rad);
  const sin = Math.sin(rad);
  const sx = lx * layer.scale;
  const sy = ly * layer.scale;
  return { x: layer.x + sx * cos - sy * sin, y: layer.y + sx * sin + sy * cos };
}

function finishHandleGesture(commit) {
  if (!gesture?.handle) return;
  const { handle, patch, kind } = gesture;
  handle.classList.remove("is-active");
  gesture = null;
  if (commit && patch) updateLayer(patch, kind === "rotate" ? "Rotated layer" : "Resized layer");
  else update();
  requestPaint();
}

function startPinch() {
  const layer = activeLayer();
  if (!layer || !layer.visible || busy || !current || compare || comparingKey ||
      current.manifest.cmyk?.enabled || masksPanel.getBrushMode() || splinePickMode) return;
  if (gesture?.handle) gesture.handle.classList.remove("is-active");
  const [p1, p2] = [...activeTouches.values()];
  const c1 = clientToCanvas(p1.clientX, p1.clientY);
  const c2 = clientToCanvas(p2.clientX, p2.clientY);
  const startMid = { x: (c1.x + c2.x) / 2, y: (c1.y + c2.y) / 2 };
  const startDist = Math.max(4, Math.hypot(c2.x - c1.x, c2.y - c1.y));
  const startAngle = Math.atan2(c2.y - c1.y, c2.x - c1.x) * 180 / Math.PI;
  const baseLayer = gesture?.patch ? { ...gesture.layer, ...gesture.patch } : layer;
  const midLocal = canvasPointToLayer(baseLayer, startMid.x, startMid.y);
  gesture = {
    pinch: true,
    layer: baseLayer,
    midLocal,
    startDist,
    startAngle,
    state: gesture?.state ?? current,
    patch: gesture?.patch,
  };
  requestPaint();
}

// Capture all touch contacts over canvas + handles so a 2nd finger brushing a corner handle
// still smoothly transitions into 2-finger pinch-to-resize & twist-to-rotate.
$("canvas-wrap").addEventListener("pointerdown", event => {
  if (event.pointerType !== "touch" || busy || !current || compare || comparingKey) return;
  activeTouches.set(event.pointerId, { clientX: event.clientX, clientY: event.clientY });
  if (activeTouches.size === 2 && !gesture?.maskBrush) {
    event.preventDefault();
    startPinch();
  }
}, { capture: true });

for (const handle of $("outline").querySelectorAll(".handle")) {
  handle.addEventListener("touchstart", event => { event.preventDefault(); }, { passive: false });
  handle.addEventListener("pointerdown", event => {
    if (event.pointerType === "touch" && activeTouches.size >= 2) return;
    if ((event.pointerType === "mouse" && event.button !== 0) || !event.isPrimary) return;
    if (busy || !current || compare || comparingKey || gesture) return;
    event.preventDefault();
    event.stopPropagation();
    const layer = activeLayer();
    if (!layer) return;
    const eff = effectiveLayerSize(layer);
    const kind = handle.dataset.handle;
    const pt = canvasPoint(event);
    handle.classList.add("is-active");
    try { handle.setPointerCapture(event.pointerId); } catch {}
    if (kind === "rotate") {
      const center = layerPointToCanvas(layer, eff.width / 2, eff.height / 2);
      const startAngle = Math.atan2(pt.y - center.y, pt.x - center.x) * 180 / Math.PI;
      gesture = { handle, pointer: event.pointerId, kind, layer, eff, center, startAngle, state: current };
    } else {
      const anchorLocal = {
        nw: [eff.width, eff.height],
        ne: [0, eff.height],
        se: [0, 0],
        sw: [eff.width, 0],
      }[kind];
      const anchorWorld = layerPointToCanvas(layer, anchorLocal[0], anchorLocal[1]);
      const startVec = { x: pt.x - anchorWorld.x, y: pt.y - anchorWorld.y };
      const startDist = Math.max(4, Math.hypot(startVec.x, startVec.y));
      gesture = { handle, pointer: event.pointerId, kind, layer, eff, anchorLocal, anchorWorld, startVec, startDist, state: current };
    }
  });
  handle.addEventListener("lostpointercapture", event => {
    if (gesture?.handle === handle && gesture?.pointer === event.pointerId) {
      finishHandleGesture(true);
    }
  });
}

preview.addEventListener("pointerdown", event => {
  if (event.pointerType === "mouse" && event.button !== 0) return;
  if (busy || !current || compare || comparingKey) return;
  if (event.pointerType === "touch") {
    event.preventDefault();
    if (gesture?.pinch || activeTouches.size >= 2 || pinchCooldown) return;
  }
  if (gesture) return;
  const point = canvasPoint(event);
  if (masksPanel.getBrushMode()) {
    const layer = activeLayer();
    if (!layer) return;
    const liveState = masksPanel.startStroke(layer, point);
    if (liveState) {
      gesture = { pointer: event.pointerId, maskBrush: true, layer, state: liveState };
      try { preview.setPointerCapture(event.pointerId); } catch {}
      requestPaint();
    }
    return;
  }
  if (splinePickMode && selectedSplineId) {
    const rx = Math.round(point.x);
    const ry = Math.round(point.y);
    const nextManifest = structuredClone(current.manifest);
    const s = nextManifest.splines?.find(x => x.id === selectedSplineId);
    if (s) {
      const last = s.controlPoints.at(-1);
      if (!last || Math.round(last[0]) !== rx || Math.round(last[1]) !== ry) {
        s.controlPoints.push([rx, ry]);
        storeState({ ...current, manifest: nextManifest }, "Added spline control point");
      }
    }
    return;
  }
  const layers = currentLayers();
  const layer = layers.findLast(entry => hit(entry, point.x, point.y));
  if (!layer) return;
  selected = layer.id;
  gesture = {
    pointer: event.pointerId,
    pointerType: event.pointerType,
    startClient: { x: event.clientX, y: event.clientY },
    moved: event.pointerType !== "touch",
    point,
    layer,
    state: current,
  };
  try { preview.setPointerCapture(event.pointerId); } catch {}
  update();
});

window.addEventListener("pointermove", event => {
  if (!current) return;
  if (gesture?.handle && gesture.pointer === event.pointerId) {
    const pt = canvasPoint(event);
    const { layer, eff, kind } = gesture;
    if (kind === "rotate") {
      const curAngle = Math.atan2(pt.y - gesture.center.y, pt.x - gesture.center.x) * 180 / Math.PI;
      const rawAngle = layer.angle + (curAngle - gesture.startAngle);
      const nextAngle = normalizeAngle(event.shiftKey ? Math.round(rawAngle / 15) * 15 : rawAngle);
      const rad = nextAngle * Math.PI / 180;
      const hw = (eff.width * layer.scale) / 2;
      const hh = (eff.height * layer.scale) / 2;
      const nx = clampPosition(gesture.center.x - (hw * Math.cos(rad) - hh * Math.sin(rad)));
      const ny = clampPosition(gesture.center.y - (hw * Math.sin(rad) + hh * Math.cos(rad)));
      gesture.patch = { angle: nextAngle, x: nx, y: ny };
      $("adjust-angle").value = nextAngle;
      $("value-angle").value = `${nextAngle}°`;
    } else {
      const vx = pt.x - gesture.anchorWorld.x;
      const vy = pt.y - gesture.anchorWorld.y;
      const dot = vx * gesture.startVec.x + vy * gesture.startVec.y;
      const nextScale = dot <= 0 ? 0.05 : clampScale(layer.scale * (Math.hypot(vx, vy) / gesture.startDist));
      const rad = layer.angle * Math.PI / 180;
      const ax = gesture.anchorLocal[0] * nextScale;
      const ay = gesture.anchorLocal[1] * nextScale;
      const nx = clampPosition(gesture.anchorWorld.x - (ax * Math.cos(rad) - ay * Math.sin(rad)));
      const ny = clampPosition(gesture.anchorWorld.y - (ax * Math.sin(rad) + ay * Math.cos(rad)));
      gesture.patch = { scale: nextScale, x: nx, y: ny };
      $("adjust-scale").value = nextScale;
      $("value-scale").value = `${Math.round(nextScale * 100)}%`;
    }
    $("position-x").value = gesture.patch.x;
    $("position-y").value = gesture.patch.y;
    gesture.state = patchState(current, selected, gesture.patch);
    requestPaint();
    return;
  }
  if (event.pointerType === "touch" && activeTouches.has(event.pointerId)) {
    activeTouches.set(event.pointerId, { clientX: event.clientX, clientY: event.clientY });
    if (gesture?.pinch && activeTouches.size >= 2) {
      const [p1, p2] = [...activeTouches.values()];
      const c1 = clientToCanvas(p1.clientX, p1.clientY);
      const c2 = clientToCanvas(p2.clientX, p2.clientY);
      const curMid = { x: (c1.x + c2.x) / 2, y: (c1.y + c2.y) / 2 };
      const curDist = Math.max(4, Math.hypot(c2.x - c1.x, c2.y - c1.y));
      const curAngle = Math.atan2(c2.y - c1.y, c2.x - c1.x) * 180 / Math.PI;
      const nextScale = clampScale(gesture.layer.scale * (curDist / gesture.startDist));
      const nextAngle = normalizeAngle(gesture.layer.angle + (curAngle - gesture.startAngle));
      const rad = nextAngle * Math.PI / 180;
      const sx = gesture.midLocal.x * nextScale;
      const sy = gesture.midLocal.y * nextScale;
      const nx = clampPosition(curMid.x - (sx * Math.cos(rad) - sy * Math.sin(rad)));
      const ny = clampPosition(curMid.y - (sx * Math.sin(rad) + sy * Math.cos(rad)));
      gesture.patch = { scale: nextScale, angle: nextAngle, x: nx, y: ny };
      $("adjust-scale").value = nextScale;
      $("value-scale").value = `${Math.round(nextScale * 100)}%`;
      $("adjust-angle").value = nextAngle;
      $("value-angle").value = `${nextAngle}°`;
      $("position-x").value = nx;
      $("position-y").value = ny;
      gesture.state = patchState(current, selected, gesture.patch);
      requestPaint();
      return;
    }
  }
  if (gesture?.pointer !== event.pointerId || gesture.handle || gesture.pinch) return;
  const point = canvasPoint(event);
  if (gesture.maskBrush) {
    const liveState = masksPanel.moveStroke(gesture.layer, point);
    if (liveState) {
      gesture.state = liveState;
      requestPaint();
    }
    return;
  }
  if (!gesture.moved) {
    if (Math.hypot(event.clientX - gesture.startClient.x, event.clientY - gesture.startClient.y) < 4) return;
    gesture.moved = true;
  }
  const { layer } = gesture;
  gesture.patch = { x: clampPosition(layer.x + point.x - gesture.point.x),
    y: clampPosition(layer.y + point.y - gesture.point.y) };
  gesture.state = patchState(current, selected, gesture.patch);
  $("position-x").value = gesture.patch.x;
  $("position-y").value = gesture.patch.y;
  requestPaint();
});

function endPointer(event) {
  if (event.pointerType === "touch") {
    activeTouches.delete(event.pointerId);
    if (activeTouches.size === 0) pinchCooldown = false;
  }
  if (gesture?.handle && gesture.pointer === event.pointerId) {
    finishHandleGesture(true);
    return;
  }
  if (gesture?.pinch) {
    if (activeTouches.size < 2) {
      const { patch } = gesture;
      gesture = null;
      pinchCooldown = activeTouches.size > 0;
      if (patch) updateLayer(patch, "Transformed layer");
      else update();
      requestPaint();
    }
    return;
  }
  if (gesture?.pointer === event.pointerId && !gesture.handle) {
    if (gesture.maskBrush) {
      gesture = null;
      masksPanel.endStroke();
      requestPaint();
      return;
    }
    const { patch } = gesture;
    gesture = null;
    if (patch) updateLayer(patch, "Moved layer");
    requestPaint();
  }
}
window.addEventListener("pointerup", endPointer);
window.addEventListener("pointercancel", endPointer);

preview.addEventListener("lostpointercapture", event => {
  if (gesture?.pointer === event.pointerId && !gesture.handle && !gesture.pinch) {
    if (gesture.maskBrush) masksPanel.endStroke();
    const patch = gesture.maskBrush ? null : gesture.patch;
    gesture = null;
    if (patch) updateLayer(patch, "Moved layer");
    else update();
  }
});

stage.addEventListener("dragover", event => {
  if (!event.dataTransfer.types.includes("Files")) return;
  event.preventDefault();
  stage.setAttribute("data-drop", "");
});
stage.addEventListener("dragleave", event => { if (!stage.contains(event.relatedTarget)) stage.removeAttribute("data-drop"); });
stage.addEventListener("drop", event => {
  if (!event.dataTransfer.types.includes("Files")) return;
  event.preventDefault();
  stage.removeAttribute("data-drop");
  const files = [...event.dataTransfer.files];
  if (files.length) run("Reading image…", () => {
    if (!current && files.length === 1) return open(files[0]);
    return addLayers(files);
  });
});

window.addEventListener("keydown", event => {
  if ($("svg-dialog").open || !$("gallery-dialog").hidden ||
      (event.target instanceof Element && event.target.matches("input, textarea, select"))) return;
  const command = event.ctrlKey || event.metaKey;
  const key = event.key.toLowerCase();
  if (command && key === "o") {
    event.preventDefault();
    if (!busy) $("open-file").click();
    return;
  }
  if (!current || busy) return;
  if (command && key === "s") {
    event.preventDefault();
    run("Saving project…", () => saveProject(true));
  } else if (command && key === "d") {
    event.preventDefault();
    duplicateActiveLayer();
  } else if (command && (key === "z" || key === "y")) {
    event.preventDefault();
    goTo(cursor + (key === "y" || event.shiftKey ? 1 : -1));
  } else if (key === "0") {
    zoom = null;
    requestPaint();
  } else if (key === " " && event.target === preview) {
    event.preventDefault();
    comparingKey = true;
    requestPaint();
  } else if (event.target === preview && /^arrow(left|right|up|down)$/.test(key)) {
    event.preventDefault();
    const layer = activeLayer();
    const step = event.shiftKey ? 10 : 1;
    const dx = Number(key === "arrowright") - Number(key === "arrowleft");
    const dy = Number(key === "arrowdown") - Number(key === "arrowup");
    updateLayer({ x: clampPosition(layer.x + dx * step), y: clampPosition(layer.y + dy * step) }, "Moved layer");
  }
});
function stopCompare() {
  if (!comparingKey) return;
  comparingKey = false;
  requestPaint();
}
window.addEventListener("keyup", event => { if (event.key === " ") stopCompare(); });
window.addEventListener("blur", stopCompare);
new ResizeObserver(requestPaint).observe(stage);

codec("init").then(mode => {
  $("engine").textContent = mode;
  appLog.info("codec", `WASM worker ready (${mode})`);
  zenFiltersPanel.loadCatalog();
  if (!current && !busy) status("Ready");
}, report);
update();
