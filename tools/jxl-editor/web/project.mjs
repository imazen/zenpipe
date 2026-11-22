const SIGNATURE = Uint8Array.of(0, 0, 0, 12, 74, 88, 76, 32, 13, 10, 135, 10);
const FILE_TYPE = Uint8Array.of(0, 0, 0, 20, 102, 116, 121, 112, 106, 120, 108, 32,
  0, 0, 0, 0, 106, 120, 108, 32);
const MANIFEST = "zned";
const SOURCE = "znas";
const RENDITION = "znrv";
const VECTOR_EDIT = "znvs";
const CMYK_SAMPLES = "zncm";
const CMYK_ICC = "znic";
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const idPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
export const MAX_PIXELS = 24_000_000;
export const MAX_FILE = 128 * 1024 * 1024;
export const MAX_LAYERS = 32;
export const MAX_TIMELINE_FRAMES = 64;
export const MAX_SPLINES = 64;
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
  "Exif", "xml ", "brob", "jhgm", "hdr ", "hCdR", "colr", "jumb",
  MANIFEST, SOURCE, RENDITION, VECTOR_EDIT, CMYK_SAMPLES, CMYK_ICC]);

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

export function writeProject(flattened, manifest, assets, preserved = [], cmykData = null) {
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
  if (manifest.cmyk?.enabled) {
    if (!cmykData?.samples?.length || !cmykData?.icc?.length) {
      throw new Error("CMYK project is missing its raw ink samples or ICC profile.");
    }
    const header = Uint8Array.of(manifest.cmyk.bitDepth);
    parts.push(...box(CMYK_SAMPLES, header, cmykData.samples));
    parts.push(...box(CMYK_ICC, cmykData.icc));
  }
  return fileBlob(parts);
}

function inRange(value, min, max) {
  return Number.isFinite(value) && value >= min && value <= max;
}

function validateLayerProps(props) {
  return props &&
    typeof props.visible === "boolean" &&
    inRange(props.opacity, 0, 1) && inRange(props.x, -32768, 32768) &&
    inRange(props.y, -32768, 32768) && inRange(props.scale, 0.05, 8) &&
    inRange(props.angle, -180, 180) && inRange(props.exposure, -2, 2) &&
    inRange(props.contrast, 0, 2) && inRange(props.saturation, 0, 2);
}

const VALID_BLENDS = new Set(["auto", "replace", "add", "blend", "alpha_weighted_add", "mul"]);

function validateTimeline(timeline, layerIds) {
  if (typeof timeline !== "object" || timeline === null || typeof timeline.enabled !== "boolean") {
    throw new Error("Invalid timeline settings.");
  }
  if (!Number.isInteger(timeline.tpsNumerator) || timeline.tpsNumerator < 1 || timeline.tpsNumerator > (1 << 30) ||
      !Number.isInteger(timeline.tpsDenominator) || timeline.tpsDenominator < 1 || timeline.tpsDenominator > 1024 ||
      !Number.isInteger(timeline.loops) || timeline.loops < 0 || timeline.loops > 65535) {
    throw new Error("Invalid timeline clock or loop count.");
  }
  if (!Array.isArray(timeline.frames) || timeline.frames.length === 0 || timeline.frames.length > MAX_TIMELINE_FRAMES) {
    throw new Error("Invalid timeline frame count.");
  }
  const seenFrames = new Set();
  for (const frame of timeline.frames) {
    if (!frame || !idPattern.test(frame.id) || seenFrames.has(frame.id) ||
        typeof frame.name !== "string" || frame.name.length > 1019 ||
        !Number.isInteger(frame.duration) || frame.duration < 0 || frame.duration > 0x7fffffff ||
        (frame.timecode !== null && frame.timecode !== undefined &&
          (!Number.isInteger(frame.timecode) || frame.timecode < 0 || frame.timecode > 0xffffffff)) ||
        (frame.blendMode !== undefined && !VALID_BLENDS.has(frame.blendMode)) ||
        (frame.blendSource !== null && frame.blendSource !== undefined &&
          (!Number.isInteger(frame.blendSource) || frame.blendSource < 0 || frame.blendSource > 3)) ||
        (frame.saveAsReference !== null && frame.saveAsReference !== undefined &&
          (!Number.isInteger(frame.saveAsReference) || frame.saveAsReference < 0 || frame.saveAsReference > 3)) ||
        typeof frame.referenceOnly !== "boolean") {
      throw new Error("Invalid timeline frame settings.");
    }
    if (frame.layerOverrides !== undefined) {
      if (typeof frame.layerOverrides !== "object" || frame.layerOverrides === null) {
        throw new Error("Invalid timeline frame layer overrides.");
      }
      for (const [id, props] of Object.entries(frame.layerOverrides)) {
        if (!layerIds.has(id) || !validateLayerProps(props)) {
          throw new Error("Invalid timeline layer override entry.");
        }
      }
    }
    seenFrames.add(frame.id);
  }
}

function validateSplines(splines) {
  if (!Array.isArray(splines) || splines.length > MAX_SPLINES) {
    throw new Error("Invalid spline list.");
  }
  const seen = new Set();
  for (const spline of splines) {
    if (!spline || !idPattern.test(spline.id) || seen.has(spline.id) ||
        typeof spline.name !== "string" || spline.name.length > 255 ||
        !Array.isArray(spline.controlPoints) || spline.controlPoints.length < 1 || spline.controlPoints.length > 256 ||
        !Array.isArray(spline.colorDct) || spline.colorDct.length !== 3 ||
        !Array.isArray(spline.sigmaDct) || spline.sigmaDct.length !== 32) {
      throw new Error("Invalid spline definition.");
    }
    for (const pt of spline.controlPoints) {
      if (!Array.isArray(pt) || pt.length !== 2 || !inRange(pt[0], -32768, 32768) || !inRange(pt[1], -32768, 32768)) {
        throw new Error("Invalid spline control point.");
      }
    }
    for (const ch of spline.colorDct) {
      if (!Array.isArray(ch) || ch.length !== 32 || !ch.every(Number.isFinite)) {
        throw new Error("Invalid spline color DCT.");
      }
    }
    if (!spline.sigmaDct.every(Number.isFinite)) {
      throw new Error("Invalid spline sigma DCT.");
    }
    seen.add(spline.id);
  }
}

function validateCoding(coding) {
  if (typeof coding !== "object" || coding === null) throw new Error("Invalid coding settings.");
  if (coding.mode !== undefined && !["modular", "vardct"].includes(coding.mode)) {
    throw new Error("Invalid coding mode.");
  }
  if (coding.distance !== undefined && !inRange(coding.distance, 0.01, 15)) {
    throw new Error("Invalid VarDCT distance.");
  }
  if (coding.effort !== undefined && (!Number.isInteger(coding.effort) || coding.effort < 1 || coding.effort > 10)) {
    throw new Error("Invalid coding effort.");
  }
}

function validateCmyk(cmyk) {
  if (typeof cmyk !== "object" || cmyk === null || typeof cmyk.enabled !== "boolean") {
    throw new Error("Invalid CMYK settings.");
  }
  if (![8, 16].includes(cmyk.bitDepth) || !["modular", "vardct"].includes(cmyk.mode)) {
    throw new Error("Invalid CMYK bit depth or coding mode.");
  }
  if (cmyk.channelOffsets !== undefined &&
      (!Array.isArray(cmyk.channelOffsets) || cmyk.channelOffsets.length !== 4 ||
        !cmyk.channelOffsets.every(v => inRange(v, -1, 1)))) {
    throw new Error("Invalid CMYK channel offsets.");
  }
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
        !validateLayerProps(layer)) {
      throw new Error("Invalid editor layer settings.");
    }
    if (layer.nativeFrame !== undefined &&
        (layer.mime !== "image/jxl" || !Number.isInteger(layer.nativeFrame) ||
          layer.nativeFrame < 0 || layer.nativeFrame >= 128)) {
      throw new Error("Invalid native JPEG XL frame reference.");
    }
    checkDimensions(layer.width, layer.height);
    seen.add(layer.id);
  }
  if (manifest.originalId !== null &&
      !layers.some(layer => layer.id === manifest.originalId)) {
    throw new Error("The original layer is missing.");
  }
  if (manifest.timeline !== undefined) validateTimeline(manifest.timeline, seen);
  if (manifest.splines !== undefined) validateSplines(manifest.splines);
  if (manifest.coding !== undefined) validateCoding(manifest.coding);
  if (manifest.cmyk !== undefined) validateCmyk(manifest.cmyk);
}

export function readProject(bytes) {
  const boxes = jxlBoxes(bytes);
  const manifestBoxes = boxes.filter(entry => entry.type === MANIFEST);
  if (manifestBoxes.length === 0) return null;
  if (manifestBoxes.length !== 1 || manifestBoxes[0].data.length > 512 * 1024) {
    throw new Error("Invalid editor manifest box.");
  }
  const manifest = JSON.parse(decoder.decode(manifestBoxes[0].data));
  validateManifest(manifest);
  const assets = new Map(manifest.layers.map(layer => [layer.id, {}]));
  let cmykSamples = null;
  let cmykIcc = null;
  for (const { type, data } of boxes) {
    if (type === CMYK_SAMPLES) {
      if (cmykSamples || data.length < 2) throw new Error("Invalid CMYK sample box.");
      cmykSamples = { bitDepth: data[0], samples: data.subarray(1) };
      continue;
    }
    if (type === CMYK_ICC) {
      if (cmykIcc || data.length < 128) throw new Error("Invalid CMYK ICC box.");
      cmykIcc = data;
      continue;
    }
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
  let cmykData = null;
  if (manifest.cmyk?.enabled) {
    if (!cmykSamples || !cmykIcc || cmykSamples.bitDepth !== manifest.cmyk.bitDepth) {
      throw new Error("The editor project is missing valid CMYK data.");
    }
    const expected = manifest.width * manifest.height * (manifest.cmyk.bitDepth === 16 ? 8 : 4);
    if (cmykSamples.samples.byteLength !== expected) {
      throw new Error("Stored CMYK sample dimensions do not match the canvas.");
    }
    cmykData = { samples: cmykSamples.samples, icc: cmykIcc };
  }
  return { manifest, assets, preserved: opaqueBoxes(bytes), cmykData };
}
