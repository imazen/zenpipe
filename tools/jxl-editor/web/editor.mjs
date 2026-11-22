import { codec } from "./codec.mjs";
import {
  checkDimensions, isJxl, MAX_FILE, MAX_LAYERS, MAX_PIXELS, MAX_SVG, opaqueBoxes,
  RASTER_TYPES, readProject, writeImage, writeProject,
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
  // getRandomValues works on a LAN served over HTTP as well as on HTTPS.
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
  // Blob URLs rendered as an image document keep SVG scripts, interactive
  // elements and external resources disabled. Source markup only enters the textarea.
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
  for (const layer of layers) {
    pixels += layer.width * layer.height;
    bytes += assets.get(layer.id).source.byteLength;
  }
  if (pixels > MAX_ASSET_PIXELS || bytes > MAX_FILE) {
    throw new Error("The document exceeds the combined layer size limit.");
  }
}

function activeLayer() {
  return current?.manifest.layers.find(layer => layer.id === selected);
}

function svgSource(asset) {
  return asset.svgEdit ?? asset.source;
}

function originalLayer() {
  return current?.manifest.layers.find(layer => layer.id === current.manifest.originalId)
    ?? current?.manifest.layers[0];
}

function storeState(state, label) {
  history.splice(cursor + 1);
  history.push({ state, label });
  if (history.length > MAX_HISTORY) history.shift();
  goTo(history.length - 1);
}

function goTo(index) {
  if (index < 0 || index >= history.length) return;
  cursor = index;
  current = history[index].state;
  if (!activeLayer()) selected = current.manifest.layers.at(-1).id;
  update();
}

function install(state, label, newTitle, alreadySaved) {
  history = [];
  cursor = -1;
  zoom = null;
  compare = false;
  comparingKey = false;
  selected = state.manifest.layers.at(-1).id;
  title = newTitle.replace(/\.(?:project\.jxl|jxl|jpg|jpeg|png|webp|svg|avif)$/i, "");
  saved = alreadySaved ? state : null;
  storeState(state, label);
}

async function open(file) {
  if (current && current !== saved && !confirm("Replace the current document and discard unsaved edits?")) return;
  if (file.size > MAX_FILE) throw new Error("The source exceeds 128 MiB.");
  const bytes = new Uint8Array(await file.arrayBuffer());
  const project = isJxl(bytes) ? readProject(bytes) : null;
  if (project) {
    if (!hasCanvasFilters && project.manifest.layers.some(hasAdjustments)) {
      throw new Error("This browser cannot render the project’s exposure, contrast or saturation.");
    }
    checkAssets(project.manifest.layers, project.assets);
    // Validate that the ordinary JPEG XL view is usable before taking ownership.
    await codec("decode", { bytes: bytes.slice() });
    for (const layer of project.manifest.layers) {
      const asset = project.assets.get(layer.id);
      if (layer.mime === "image/svg+xml") {
        text.decode(asset.source);
        text.decode(svgSource(asset));
      }
      asset.canvas = await decodeSource(asset.rendition ?? asset.source, asset.rendition ? "image/jxl" : layer.mime);
      if (layer.width !== asset.canvas.width || layer.height !== asset.canvas.height) {
        throw new Error("A layer’s stored dimensions do not match its image.");
      }
    }
    install(project, "Reopened project", file.name, true);
    status("Project reopened · original assets restored");
    return;
  }
  const { layer, asset } = await makeLayer(file, bytes);
  const manifest = { version: 1, width: layer.width, height: layer.height, originalId: layer.id, layers: [layer] };
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
  const { manifest, preserved } = current;
  if (manifest.layers.length + files.length > MAX_LAYERS) throw new Error("A project can hold up to 32 layers.");
  const layers = [...manifest.layers];
  const assets = new Map(current.assets);
  for (const file of files) {
    const { layer, asset } = await makeLayer(file);
    layer.x = Math.round((manifest.width - layer.width) / 2);
    layer.y = Math.round((manifest.height - layer.height) / 2);
    layers.push(layer);
    assets.set(layer.id, asset);
    checkAssets(layers, assets);
  }
  selected = layers.at(-1).id;
  compare = false;
  storeState({ manifest: { ...manifest, layers }, assets, preserved }, files.length === 1 ? "Added layer" : "Added layers");
  status(files.length === 1 ? "Layer added" : "Layers added");
}

function patchState(state, id, patch) {
  const layers = state.manifest.layers.map(layer => layer.id === id ? { ...layer, ...patch } : layer);
  return { ...state, manifest: { ...state.manifest, layers } };
}

function updateLayer(patch, label, id = selected) {
  if (busy || !current) return;
  const layer = current.manifest.layers.find(entry => entry.id === id);
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
  const { manifest, preserved } = current;
  const assets = new Map(current.assets);
  assets.delete(selected);
  const layers = manifest.layers.filter(layer => layer.id !== selected);
  const originalId = manifest.originalId === selected ? null : manifest.originalId;
  compare = false;
  storeState({ manifest: { ...manifest, layers, originalId }, assets, preserved }, "Deleted layer");
}

function drawLayers(context, state, width, height, original = false) {
  const { manifest, assets } = state;
  context.setTransform(1, 0, 0, 1, 0, 0);
  context.clearRect(0, 0, width, height);
  context.scale(width / manifest.width, height / manifest.height);
  if (original) {
    const layer = originalLayer();
    context.drawImage(assets.get(layer.id).canvas, 0, 0);
    return;
  }
  context.imageSmoothingQuality = "high";
  for (const layer of manifest.layers) {
    if (layer.visible && layer.opacity) drawLayer(context, layer, assets.get(layer.id));
  }
}

function hasAdjustments(layer) {
  return layer.exposure !== 0 || layer.contrast !== 1 || layer.saturation !== 1;
}

function drawLayer(context, layer, asset) {
  context.save();
  context.translate(layer.x, layer.y);
  context.rotate(layer.angle * Math.PI / 180);
  context.scale(layer.scale, layer.scale);
  context.globalAlpha = layer.opacity;
  if (hasCanvasFilters && hasAdjustments(layer)) {
    context.filter = `brightness(${2 ** layer.exposure}) contrast(${layer.contrast}) saturate(${layer.saturation})`;
  }
  context.drawImage(asset.canvas, 0, 0);
  context.restore();
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
  $("fit").textContent = zoom === null ? `Fit · ${Math.round(scale * 100)}%` : `${Math.round(scale * 100)}%`;
  const state = gesture?.state ?? current;
  drawLayers(ctx, state, w, h, compare || comparingKey);
  const layer = state.manifest.layers.find(entry => entry.id === selected);
  const outline = $("outline");
  outline.hidden = !layer?.visible || compare || comparingKey || state.manifest.layers.length === 1;
  if (!outline.hidden) {
    outline.style.width = `${layer.width * layer.scale * scale}px`;
    outline.style.height = `${layer.height * layer.scale * scale}px`;
    outline.style.transform = `translate(${layer.x * scale}px, ${layer.y * scale}px) rotate(${layer.angle}deg)`;
  }
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

function update() {
  const hasDoc = Boolean(current);
  const layer = activeLayer();
  const layers = current?.manifest.layers ?? [];
  const index = layers.indexOf(layer);
  $("title").textContent = title;
  $("dirty").hidden = !hasDoc || current === saved;
  $("empty").hidden = hasDoc;
  $("canvas-wrap").hidden = !hasDoc;
  $("dimensions").textContent = hasDoc ? `${current.manifest.width} × ${current.manifest.height} px` : "";
  for (const id of ["save", "export", "add", "compare", "fit", "zoom-in", "zoom-out"]) $(id).disabled = !hasDoc || busy;
  for (const id of ["open", "empty-open", "sample"]) $(id).disabled = busy;
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
    input.disabled = !hasCanvasFilters && ["exposure", "contrast", "saturation"].includes(key);
    $(`value-${key}`).value = format(value);
  }
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
      moveLayer(dragged, current.manifest.layers.indexOf(entry));
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
    detail.textContent = entry.mime === "image/svg+xml" ? "SVG" : `${entry.width} × ${entry.height}`;
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
  const { originalId, layers, width, height } = state.manifest;
  const [layer] = layers;
  return layers.length === 1 && layer.id === originalId && layer.mime === "image/jxl" &&
    width === layer.width && height === layer.height &&
    Object.entries(defaults).every(([key, value]) => layer[key] === value);
}

async function encodePresentation(canvas, state, includeLayers) {
  const { width, height } = canvas;
  const context = canvas.getContext("2d");
  const { data: expected } = context.getImageData(0, 0, width, height);
  const visible = state.manifest.layers.filter(layer => layer.visible && layer.opacity > 0);
  if (includeLayers && visible.length >= 2 && visible.length <= 8 &&
      width * height * visible.length <= 16_000_000) {
    const pixels = new Uint8Array(expected.byteLength * visible.length);
    // Pass the cumulative canvas after each layer. The encoder can store only
    // the changed crop as a native Replace frame. This retains Canvas blend,
    // filter and alpha-rounding semantics at all points in the stack.
    context.clearRect(0, 0, width, height);
    visible.forEach((layer, index) => {
      drawLayer(context, layer, state.assets.get(layer.id));
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
  const state = current;
  let flattened;
  let preserved = state.preserved;
  if (originalPassthrough(state)) {
    flattened = state.assets.get(state.manifest.originalId).source;
    const sourceBoxes = opaqueBoxes(flattened);
    // The original already contains its own opaque boxes. Keep additional
    // envelope boxes a different tool may have added to the reopened project.
    preserved = preserved.filter(entry => {
      const index = sourceBoxes.findIndex(box =>
        box.type === entry.type && box.data.length === entry.data.length &&
        box.data.every((byte, offset) => byte === entry.data[offset]));
      if (index < 0) return true;
      sourceBoxes.splice(index, 1);
      return false;
    });
  } else {
    const canvas = document.createElement("canvas");
    canvas.width = state.manifest.width;
    canvas.height = state.manifest.height;
    drawLayers(canvas.getContext("2d"), state, canvas.width, canvas.height);
    flattened = await encodePresentation(canvas, state, project);
  }
  if (project) {
    const assets = new Map(state.assets);
    for (const layer of state.manifest.layers) {
      const asset = assets.get(layer.id);
      if (layer.mime === "image/svg+xml" && !asset.rendition) {
        assets.set(layer.id, { ...asset, rendition: await encodeCanvas(asset.canvas) });
      }
    }
    download(writeProject(flattened, state.manifest, assets, preserved), `${title}.project.jxl`);
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
$("raise").onclick = () => moveLayer(selected, current.manifest.layers.indexOf(activeLayer()) + 1);
$("lower").onclick = () => moveLayer(selected, current.manifest.layers.indexOf(activeLayer()) - 1);
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

function showHistory(show) {
  $("history").hidden = !show;
  $("properties").hidden = show;
  $("history-tab").setAttribute("aria-pressed", String(show));
  $("properties-tab").setAttribute("aria-pressed", String(!show));
}
$("history-tab").onclick = () => showHistory(true);
$("properties-tab").onclick = () => showHistory(false);

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
  const layer = current.manifest.layers.findLast(entry => hit(entry, point.x, point.y));
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
  if (files.length) run("Reading image…", () => files.length > 1 ? addLayers(files) : open(files[0]));
});

window.addEventListener("keydown", event => {
  if ($("svg-dialog").open || event.target instanceof Element && event.target.matches("input, textarea")) return;
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
