import { codec } from "./codec.mjs";
import { makeNativeInspector } from "./native-panel.mjs";
import { ZEN_FILTERS, hasAdjustments, previewFilter, renderFilter } from "./filters.mjs";
import {
  applyCmykOffsets, buildCodingPayload, createDefaultTimeline, defaultCoding,
  isDefaultModularCoding, makeAuthorPanels, resolveLayersForFrame,
  snapshotLayerOverrides, splineCompatibilityError,
} from "./author-panels.mjs";
import {
  checkDimensions, isJxl, MAX_FILE, MAX_LAYERS, MAX_PIXELS, MAX_SVG,
  MAX_TIMELINE_FRAMES, opaqueBoxes, RASTER_TYPES, readProject, writeImage, writeProject,
} from "./project.mjs";

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
const MAX_ASSET_PIXELS = 2 * MAX_PIXELS;
const MAX_HISTORY = 32;

let history = [];
let cursor = -1;
let current;
let saved;
let selected;
let title = "Untitled";
let zoom = null;
let compare = false;
let comparingKey = false;
let busy = false;
let animation;
let gesture;
let sliderKey;
let dragged;
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
  status(error instanceof Error ? error.message : String(error), true);
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
  const mime = isJxl(source) ? "image/jxl" : types[extension] ?? file.type;
  if (![...RASTER_TYPES, "image/svg+xml"].includes(mime)) throw new Error("Unsupported image format.");
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

function install(state, label, newTitle, alreadySaved) {
  stopPlayback();
  history = [];
  cursor = -1;
  zoom = null;
  compare = false;
  comparingKey = false;
  activeFrameIndex = 0;
  timelineOpen = Boolean(state.manifest.timeline?.enabled);
  selectedSplineId = state.manifest.splines?.[0]?.id ?? null;
  splinePickMode = false;
  splinePreviewCache = { key: null, canvas: null, pending: false, token: 0 };
  cmykPreviewCache = { key: null, canvas: null, token: 0 };
  showInspector("properties");
  selected = state.manifest.layers.at(-1).id;
  title = newTitle.replace(/\.(?:project\.jxl|jxl|jpg|jpeg|png|webp|svg|avif)$/i, "");
  saved = alreadySaved ? state : null;
  storeState(state, label);
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
    assets.set(id, { source: asset.source, canvas, rendition: null });
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
  install({ manifest, assets, preserved: opaqueBoxes(asset.source) }, "Imported native animation", sourceName, false);
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
      if (layer.visible && layer.opacity && hasAdjustments(layer)) {
        rasters.set(layer.id, await renderFilter(state.assets.get(layer.id), layer));
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

async function open(file) {
  if (current && current !== saved && !confirm("Replace the current document and discard unsaved edits?")) return;
  if (file.size > MAX_FILE) throw new Error("The source exceeds 128 MiB.");
  const bytes = new Uint8Array(await file.arrayBuffer());
  const project = isJxl(bytes) ? readProject(bytes) : null;
  if (project) {
    if (project.manifest.filterEngine !== ZEN_FILTERS && !hasCanvasFilters && project.manifest.layers.some(hasAdjustments)) {
      throw new Error("This browser cannot render the project’s exposure, contrast or saturation.");
    }
    checkAssets(project.manifest.layers, project.assets);
    await codec("decode", { bytes: bytes.slice() });
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
        throw new Error("A layer’s stored dimensions do not match its image.");
      }
    }
    install(project, "Reopened project", file.name, true);
    if (project.manifest.cmyk?.enabled) await refreshCmykPreview();
    status("Project reopened · original assets restored");
    return;
  }

  // Check if a raw JXL is a multi-frame animation or CMYK image.
  if (isJxl(bytes)) {
    let info = null;
    try {
      info = await codec("nativeInfo", { bytes: bytes.slice() });
    } catch {
      info = null;
    }
    const visibleAnimated = info?.animation &&
      info.frames.filter(f => f.durationTicks > 0).length >= 2 &&
      info.frames.length <= MAX_LAYERS;
    if (visibleAnimated) {
      await importNativeTimeline({ source: bytes }, file.name);
      saved = current;
      status("Image opened");
      return;
    }
    if (info?.profile?.cmyk) {
      const { layer, asset } = await makeLayer(file, bytes);
      const extracted = await codec("nativeCmyk", { bytes: bytes.slice() });
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
          iccName: `${file.name.replace(/\.jxl$/i, "")}.icc`,
          origin: "source",
          channelOffsets: [0, 0, 0, 0],
        },
      };
      const assets = new Map([[layer.id, asset]]);
      install(
        { manifest, assets, preserved: opaqueBoxes(bytes), cmykData: { samples: extracted.samples, icc: extracted.icc } },
        "Opened CMYK image",
        file.name,
        true,
      );
      await refreshCmykPreview();
      status("Image opened");
      return;
    }
  }

  const { layer, asset } = await makeLayer(file, bytes);
  const manifest = { version: 1, filterEngine: ZEN_FILTERS, width: layer.width, height: layer.height, originalId: layer.id, layers: [layer] };
  const assets = new Map([[layer.id, asset]]);
  install({ manifest, assets, preserved: layer.mime === "image/jxl" ? opaqueBoxes(bytes) : [] },
    "Opened image", file.name, true);
  status("Image opened");
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
      nextManifest.layers = nextManifest.layers.map(l => l.id === id ? { ...l, ...metaPatch, ...(activeFrameIndex === 0 ? propPatch : {}) } : l);
    }
    return { ...state, manifest: nextManifest };
  }
  const layers = state.manifest.layers.map(layer => layer.id === id ? { ...layer, ...patch } : layer);
  return { ...state, manifest: { ...state.manifest, layers } };
}

function updateLayer(patch, label, id = selected) {
  if (busy || !current) return;
  const layer = currentLayers().find(entry => entry.id === id);
  if (!layer || Object.entries(patch).every(([key, value]) => layer[key] === value)) return;
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
  context.save();
  context.translate(layer.x, layer.y);
  context.rotate(layer.angle * Math.PI / 180);
  context.scale(layer.scale, layer.scale);
  context.globalAlpha = layer.opacity;
  if (filterEngine !== ZEN_FILTERS && hasCanvasFilters && hasAdjustments(layer)) {
    context.filter = `brightness(${2 ** layer.exposure}) contrast(${layer.contrast}) saturate(${layer.saturation})`;
  }
  context.drawImage(source, 0, 0);
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
  const padding = 40;
  const fit = Math.min((stage.clientWidth - padding) / width, (stage.clientHeight - padding) / height, 2);
  const scale = Math.max(0.02, zoom ?? fit);
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
  $("fit").textContent = zoom === null ? `Fit · ${Math.round(scale * 100)}%` : `${Math.round(scale * 100)}%`;
  const state = gesture?.state ?? current;
  const layers = currentLayers(state);
  const rasters = new Map();
  let pending = false;
  if (!compare && !comparingKey && !state.manifest.cmyk?.enabled && state.manifest.filterEngine === ZEN_FILTERS) {
    for (const layer of layers) {
      if (!layer.visible || !layer.opacity || !hasAdjustments(layer)) continue;
      const filtered = previewFilter(state.assets.get(layer.id), layer, requestPaint, report);
      rasters.set(layer.id, filtered.canvas);
      pending ||= filtered.pending;
    }
  }
  drawLayers(ctx, state, w, h, compare || comparingKey, rasters);

  // Render exact JXL spline preview onto canvas when splines are authored.
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
  outline.hidden = !layer?.visible || compare || comparingKey || state.manifest.layers.length === 1 || state.manifest.cmyk?.enabled;
  if (!outline.hidden) {
    outline.style.width = `${layer.width * layer.scale * scale}px`;
    outline.style.height = `${layer.height * layer.scale * scale}px`;
    outline.style.transform = `translate(${layer.x * scale}px, ${layer.y * scale}px) rotate(${layer.angle}deg)`;
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
    if (sliderKey !== key) return;
    gesture = null;
    sliderKey = null;
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

const authorPanels = makeAuthorPanels({
  getState: () => current,
  getBusy: () => busy,
  getFrameIndex: () => activeFrameIndex,
  setFrameIndex: idx => { activeFrameIndex = idx; update(); },
  getSelectedSpline: () => selectedSplineId,
  setSelectedSpline: id => { selectedSplineId = id; update(); },
  getSplinePickMode: () => splinePickMode,
  setSplinePickMode: v => { splinePickMode = v; update(); },
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
  for (const id of ["save", "export", "compare", "fit", "zoom-in", "zoom-out"]) $(id).disabled = !hasDoc || busy;
  for (const id of ["open", "add", "add-panel", "empty-open", "sample"]) $(id).disabled = busy;
  $("undo").disabled = cursor < 1 || busy;
  $("redo").disabled = cursor >= history.length - 1 || busy;
  $("raise").disabled = !layer || index === layers.length - 1 || busy;
  $("lower").disabled = index < 1 || busy;
  $("remove").disabled = layers.length < 2 || busy;
  $("edit-svg").hidden = layer?.mime !== "image/svg+xml";
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
  const nativeSource = layer?.mime === "image/jxl" ? layer : originalLayer();
  const nativeAsset = nativeSource?.mime === "image/jxl" && current.assets.get(nativeSource.id);
  $("native-tab").disabled = !nativeAsset;
  const sourceChanged = nativeInspector.setSource(nativeAsset || null, nativeSource?.name);
  nativeInspector.setBusy(busy);
  if (!nativeAsset && inspectorView === "native") showInspector("properties");
  if (!hasDoc && ["coding", "splines"].includes(inspectorView)) showInspector("properties");
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
    const scale = 32 / Math.max(entry.width, entry.height);
    const thumbWidth = entry.width * scale;
    const thumbHeight = entry.height * scale;
    thumbnail.getContext("2d").drawImage(current.assets.get(entry.id).canvas,
      (32 - thumbWidth) / 2, (32 - thumbHeight) / 2, thumbWidth, thumbHeight);
    const name = document.createElement("span");
    name.textContent = entry.name;
    const detail = document.createElement("small");
    detail.textContent = entry.mime === "image/svg+xml" ? "SVG"
      : entry.nativeFrame !== undefined ? `Native JXL frame ${entry.nativeFrame + 1}`
      : `${entry.width} × ${entry.height}`;
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
  return layers.length === 1 && layer.id === originalId && layer.mime === "image/jxl" &&
    width === layer.width && height === layer.height &&
    Object.entries(defaults).every(([key, value]) => layer[key] === value);
}

async function encodePresentation(state, includeLayers) {
  const { width, height, timeline, splines, coding, cmyk } = state.manifest;

  // 1. Native CMYK (Level 10 lossless Cmyk8/16 or lossy ICC->XYB + Black)
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

  // 2. Native multi-frame JXL animation timeline
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

  // 3. Still image (configured Modular/VarDCT/Splines OR default cumulative native layers)
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

async function saveProject(project) {
  stopPlayback();
  const state = current;
  let flattened;
  let preserved = state.preserved;
  if (originalPassthrough(state)) {
    flattened = state.assets.get(state.manifest.originalId).source;
    const sourceBoxes = opaqueBoxes(flattened);
    preserved = preserved.filter(entry => {
      const index = sourceBoxes.findIndex(box =>
        box.type === entry.type && box.data.length === entry.data.length &&
        box.data.every((byte, offset) => byte === entry.data[offset]));
      if (index < 0) return true;
      sourceBoxes.splice(index, 1);
      return false;
    });
  } else {
    flattened = await encodePresentation(state, project);
  }
  if (project) {
    const assets = new Map(state.assets);
    for (const layer of state.manifest.layers) {
      const asset = assets.get(layer.id);
      if (layer.mime === "image/svg+xml" && !asset.rendition) {
        assets.set(layer.id, { ...asset, rendition: await encodeCanvas(asset.canvas) });
      }
    }
    const cmykPayload = state.manifest.cmyk?.enabled ? state.cmykData : null;
    download(writeProject(flattened, state.manifest, assets, preserved, cmykPayload), `${title}.project.jxl`);
    saved = state;
    status("Project saved · original assets embedded");
  } else {
    download(writeImage(flattened, preserved), `${title}.jxl`);
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
$("reset").onclick = () => updateLayer({ ...defaults, visible: activeLayer().visible }, "Reset adjustments");
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
  $("history").hidden = view !== "history";
  $("properties").hidden = view !== "properties";
  $("coding-panel").hidden = view !== "coding";
  $("splines-panel").hidden = view !== "splines";
  $("native-panel").hidden = view !== "native";
  for (const id of ["history", "properties", "coding", "splines", "native"]) {
    $(`${id}-tab`).setAttribute("aria-pressed", String(view === id));
  }
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
$("history-tab").onclick = () => showInspector("history");
$("properties-tab").onclick = () => showInspector("properties");
$("coding-tab").onclick = () => showInspector("coding");
$("splines-tab").onclick = () => showInspector("splines");
$("native-tab").onclick = () => {
  showInspector("native");
  nativeInspector.inspect();
};

$("edit-svg").onclick = () => {
  $("svg-error").textContent = "";
  $("svg-source").value = text.decode(svgSource(current.assets.get(selected)));
  $("svg-dialog").showModal();
};
$("svg-cancel").onclick = () => $("svg-dialog").close();
$("svg-form").onsubmit = async event => {
  event.preventDefault();
  if (busy) return;
  const asset = current.assets.get(selected);
  if ($("svg-source").value === text.decode(svgSource(asset)).replace(/\r\n?/g, "\n")) {
    $("svg-dialog").close();
    return;
  }
  const source = utf8.encode($("svg-source").value);
  busy = true;
  $("svg-apply").disabled = $("svg-cancel").disabled = true;
  try {
    const canvas = await decodeSource(source, "image/svg+xml");
    const state = patchState(current, selected, { width: canvas.width, height: canvas.height });
    state.assets = new Map(state.assets);
    state.assets.set(selected, { ...asset, svgEdit: source, canvas, rendition: null });
    checkAssets(state.manifest.layers, state.assets);
    storeState(state, "Edited SVG source");
    $("svg-dialog").close();
    status("SVG rendered");
  } catch (error) {
    $("svg-error").textContent = error.message;
  } finally {
    busy = false;
    $("svg-apply").disabled = $("svg-cancel").disabled = false;
    update();
  }
};
$("svg-dialog").addEventListener("cancel", event => { if (busy) event.preventDefault(); });

function canvasPoint(event) {
  const bounds = preview.getBoundingClientRect();
  return { x: (event.clientX - bounds.left) * current.manifest.width / bounds.width,
    y: (event.clientY - bounds.top) * current.manifest.height / bounds.height };
}

function hit(layer, x, y) {
  const theta = layer.angle * Math.PI / 180;
  const dx = x - layer.x;
  const dy = y - layer.y;
  const u = (Math.cos(theta) * dx + Math.sin(theta) * dy) / layer.scale;
  const v = (-Math.sin(theta) * dx + Math.cos(theta) * dy) / layer.scale;
  return layer.visible && layer.opacity > 0 && u >= 0 && v >= 0 && u < layer.width && v < layer.height;
}

preview.addEventListener("pointerdown", event => {
  if (event.button !== 0 || busy || !current || compare || comparingKey || gesture) return;
  const point = canvasPoint(event);
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
  gesture = { pointer: event.pointerId, point, layer, state: current };
  preview.setPointerCapture(event.pointerId);
  update();
});
preview.addEventListener("pointermove", event => {
  if (gesture?.pointer !== event.pointerId) return;
  const point = canvasPoint(event);
  const { layer } = gesture;
  gesture.patch = { x: clampPosition(layer.x + point.x - gesture.point.x),
    y: clampPosition(layer.y + point.y - gesture.point.y) };
  gesture.state = patchState(current, selected, gesture.patch);
  $("position-x").value = gesture.patch.x;
  $("position-y").value = gesture.patch.y;
  requestPaint();
});
preview.addEventListener("pointerup", event => {
  if (gesture?.pointer !== event.pointerId) return;
  const { patch } = gesture;
  gesture = null;
  if (patch) updateLayer(patch, "Moved layer");
  requestPaint();
});
preview.addEventListener("lostpointercapture", () => {
  if (gesture?.pointer !== undefined) {
    gesture = null;
    update();
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
  if ($("svg-dialog").open || event.target instanceof Element && event.target.matches("input, textarea, select")) return;
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
  if (!current && !busy) status("Ready");
}, report);
update();
