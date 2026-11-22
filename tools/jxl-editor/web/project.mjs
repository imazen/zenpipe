const SIGNATURE = Uint8Array.of(0, 0, 0, 12, 74, 88, 76, 32, 13, 10, 135, 10);
const FILE_TYPE = Uint8Array.of(0, 0, 0, 20, 102, 116, 121, 112, 106, 120, 108, 32,
  0, 0, 0, 0, 106, 120, 108, 32);
const MANIFEST = "zned";
const SOURCE = "znas";
const RENDITION = "znrv";
const VECTOR_EDIT = "znvs";
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const idPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
export const MAX_PIXELS = 24_000_000;
export const MAX_FILE = 128 * 1024 * 1024;
export const MAX_LAYERS = 32;
export const MAX_SVG = 2 * 1024 * 1024;
export const RASTER_TYPES = ["image/jxl", "image/jpeg", "image/png", "image/webp", "image/avif"];

export function checkDimensions(width, height) {
  if (!Number.isInteger(width) || !Number.isInteger(height) ||
      width < 1 || height < 1 || width > 8192 || height > 8192 || width * height > MAX_PIXELS) {
    throw new Error("The image exceeds the editor’s 8,192 px / 24 MP canvas limit.");
  }
}

export function isJxl(bytes) {
  return (bytes[0] === 255 && bytes[1] === 10) ||
    SIGNATURE.every((byte, index) => bytes[index] === byte);
}

export function box(type, ...parts) {
  const header = new Uint8Array(8);
  const size = parts.reduce((total, part) => total + part.byteLength, 8);
  if (type.length !== 4 || size > MAX_FILE) throw new Error("Invalid JPEG XL box.");
  new DataView(header.buffer).setUint32(0, size);
  for (let i = 0; i < 4; i++) header[4 + i] = type.charCodeAt(i);
  return [header, ...parts];
}

export function jxlBoxes(bytes) {
  if (bytes[0] === 255 && bytes[1] === 10) return [];
  if (!isJxl(bytes) || bytes.length > MAX_FILE) throw new Error("Invalid JPEG XL container.");
  const data = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const result = [];
  for (let offset = SIGNATURE.length; offset < bytes.length;) {
    if (bytes.length - offset < 8 || result.length === 16384) {
      throw new Error("Invalid JPEG XL box table.");
    }
    let length = data.getUint32(offset);
    const type = String.fromCharCode(...bytes.subarray(offset + 4, offset + 8));
    let header = 8;
    const extendsToEnd = length === 0;
    if (length === 1) {
      header = 16;
      if (bytes.length - offset < header) throw new Error("Truncated JPEG XL box.");
      length = Number(data.getBigUint64(offset + 8));
    }
    if (extendsToEnd) length = bytes.length - offset;
    if (length < header || length > bytes.length - offset) throw new Error("Truncated JPEG XL box.");
    result.push({
      type,
      data: bytes.subarray(offset + header, offset + length),
      raw: bytes.subarray(offset, offset + length),
      extendsToEnd,
    });
    offset += length;
  }
  if (result[0]?.type !== "ftyp") throw new Error("JPEG XL file type box is missing.");
  return result;
}

function copyBox(entry) {
  return entry.extendsToEnd ? box(entry.type, entry.data) : [entry.raw];
}

const structural = new Set(["ftyp", "JXL ", "jxlc", "jxlp", "jxli", "jxll", "jbrd",
  "Exif", "xml ", "brob", "jhgm", "hdr ", "hCdR", "colr", "jumb", MANIFEST, SOURCE, RENDITION, VECTOR_EDIT]);

export function opaqueBoxes(bytes) {
  return jxlBoxes(bytes).filter(({ type, data }) => {
    // Compressed application boxes are opaque too. Exclude compressed standard
    // metadata and this editor's reserved types from an edited image's envelope.
    if (type === "brob" && data.length >= 4) {
      return !structural.has(String.fromCharCode(...data.subarray(0, 4)));
    }
    return !structural.has(type);
  });
}

function publicImageParts(flattened) {
  const boxes = jxlBoxes(flattened);
  if (boxes.length === 0) return [SIGNATURE, FILE_TYPE, ...box("jxlc", flattened)];
  return [SIGNATURE, ...boxes.flatMap(copyBox)];
}

function fileBlob(parts) {
  if (parts.reduce((total, part) => total + part.byteLength, 0) > MAX_FILE) {
    throw new Error("The exported document exceeds 128 MiB.");
  }
  return new Blob(parts, { type: "image/jxl" });
}

function presentationParts(flattened, preserved) {
  const parts = publicImageParts(flattened);
  for (const entry of preserved) parts.push(...copyBox(entry));
  return parts;
}

export function writeImage(flattened, preserved) {
  return fileBlob(preserved.length ? presentationParts(flattened, preserved) : [flattened]);
}

export function writeProject(flattened, manifest, assets, preserved = []) {
  validateManifest(manifest);
  const parts = presentationParts(flattened, preserved);
  parts.push(...box(MANIFEST, encoder.encode(JSON.stringify(manifest))));
  for (const layer of manifest.layers) {
    const asset = assets.get(layer.id);
    if (!asset?.source?.length) throw new Error("A layer’s original source is missing.");
    const id = encoder.encode(layer.id);
    parts.push(...box(SOURCE, id, asset.source));
    if (layer.mime === "image/svg+xml") {
      if (!asset.rendition?.length) throw new Error("A vector layer’s raster rendition is missing.");
      parts.push(...box(RENDITION, id, asset.rendition));
      if (asset.svgEdit) parts.push(...box(VECTOR_EDIT, id, asset.svgEdit));
    }
  }
  return fileBlob(parts);
}

function inRange(value, min, max) {
  return Number.isFinite(value) && value >= min && value <= max;
}

export function validateManifest(manifest) {
  if (manifest?.version !== 1) throw new Error("This editor project version is unsupported.");
  if (manifest.filterEngine !== undefined && manifest.filterEngine !== "zenfilters-v1") {
    throw new Error("This editor project uses an unsupported filter engine.");
  }
  checkDimensions(manifest.width, manifest.height);
  const { layers } = manifest;
  if (!Array.isArray(layers) || layers.length === 0 || layers.length > MAX_LAYERS) {
    throw new Error("Invalid editor layer count.");
  }
  if (manifest.originalId !== null && !idPattern.test(manifest.originalId)) {
    throw new Error("Invalid original image reference.");
  }
  const seen = new Set();
  for (const layer of layers) {
    if (!layer || !idPattern.test(layer.id) || seen.has(layer.id) ||
        typeof layer.name !== "string" || layer.name.length > 255 ||
        ![...RASTER_TYPES, "image/svg+xml"].includes(layer.mime) ||
        typeof layer.visible !== "boolean" ||
        !inRange(layer.opacity, 0, 1) || !inRange(layer.x, -32768, 32768) ||
        !inRange(layer.y, -32768, 32768) || !inRange(layer.scale, 0.05, 8) ||
        !inRange(layer.angle, -180, 180) || !inRange(layer.exposure, -2, 2) ||
        !inRange(layer.contrast, 0, 2) || !inRange(layer.saturation, 0, 2)) {
      throw new Error("Invalid editor layer settings.");
    }
    checkDimensions(layer.width, layer.height);
    seen.add(layer.id);
  }
  if (manifest.originalId !== null &&
      !layers.some(layer => layer.id === manifest.originalId)) {
    throw new Error("The original layer is missing.");
  }
}

export function readProject(bytes) {
  const boxes = jxlBoxes(bytes);
  const manifestBoxes = boxes.filter(entry => entry.type === MANIFEST);
  if (manifestBoxes.length === 0) return null;
  if (manifestBoxes.length !== 1 || manifestBoxes[0].data.length > 64 * 1024) {
    throw new Error("Invalid editor manifest box.");
  }
  const manifest = JSON.parse(decoder.decode(manifestBoxes[0].data));
  validateManifest(manifest);
  const assets = new Map(manifest.layers.map(layer => [layer.id, {}]));
  for (const { type, data } of boxes) {
    if (type !== SOURCE && type !== RENDITION && type !== VECTOR_EDIT) continue;
    const id = decoder.decode(data.subarray(0, 36));
    const asset = assets.get(id);
    const field = type === SOURCE ? "source" : type === RENDITION ? "rendition" : "svgEdit";
    if (!asset || asset[field] || data.length <= 36) throw new Error("Invalid editor asset box.");
    asset[field] = data.subarray(36);
  }
  for (const layer of manifest.layers) {
    const { source, rendition, svgEdit } = assets.get(layer.id);
    const vector = layer.mime === "image/svg+xml";
    if (!source || (vector && (source.length > MAX_SVG || !rendition || !isJxl(rendition) || svgEdit?.length > MAX_SVG)) ||
        (!vector && (rendition || svgEdit)) || (layer.mime === "image/jxl" && !isJxl(source))) {
      throw new Error("The editor project has missing or invalid assets.");
    }
  }
  return { manifest, assets, preserved: opaqueBoxes(bytes) };
}
