import { codec } from "./codec.mjs";

export const ZEN_FILTERS = "zenfilters-v1";

export const DEFAULT_FILTERS = Object.freeze({
  highlights: 0, shadows: 0, whites: 0, blacks: 0, brilliance: 0,
  blackPoint: 0, whitePoint: 1, localToneMap: 0,
  vibrance: 0, temperature: 0, tint: 0, hue: 0, dehaze: 0,
  clarity: 0, texture: 0, sharpen: 0, blur: 0, denoiseLuma: 0, denoiseChroma: 0,
  vignette: 0, bloom: 0, grain: 0, sepia: 0,
  grayscale: false, invert: false, posterize: 0, solarize: 0,
  autoTone: 0, autoWhiteBalance: 0, autoExposure: 0,
  autoContrast: 0, autoLevels: 0, autoVibrance: 0,
  preset: null, presetStrength: 1,
  filmLook: null, filmStrength: 1,
  filterMask: null,
});

export const DEFAULT_MASK = Object.freeze({
  enabled: false,
  mode: "parametric",
  shape: "none",
  invert: false,
  density: 1,
  feather: 0,
  angle: 0,
  position: 0.5,
  spread: 0.35,
  cx: 0.5,
  cy: 0.5,
  rx: 0.38,
  ry: 0.38,
  softness: 0.3,
  inset: 0.08,
  radius: 0.12,
  low: 0.25,
  high: 1.0,
  lumaFeather: 0.12,
  hasRaster: false,
});

export const DEFAULT_ZENSR = Object.freeze({
  enabled: false,
  model: "dejpeg_rt24g",
  arch: "compact",
  nf: 24,
  nc: 8,
  scale: 1,
  strength: 1,
});

const cache = new WeakMap();
const MAX_SCALED_DIM = 16384;
const MAX_SCALED_PIXELS = 64_000_000;

export function mergedFilterSpec(layer) {
  return {
    ...DEFAULT_FILTERS,
    ...(layer.filters ?? {}),
    exposure: layer.exposure ?? 0,
    contrast: layer.contrast ?? 1,
    saturation: layer.saturation ?? 1,
  };
}

export function compactFilters(filters) {
  if (!filters) return undefined;
  const out = {};
  for (const [k, def] of Object.entries(DEFAULT_FILTERS)) {
    const val = filters[k];
    if (val === undefined) continue;
    if (k === "filterMask") {
      if (val && val.kind && val.kind !== "none") out[k] = val;
      continue;
    }
    if (val !== def) out[k] = val;
  }
  return Object.keys(out).length ? out : undefined;
}

export function hasAdjustments(layer) {
  if (!layer) return false;
  if (layer.exposure !== 0 || layer.contrast !== 1 || layer.saturation !== 1) return true;
  const f = layer.filters;
  if (!f) return false;
  if (f.preset && f.preset !== "none" && (f.presetStrength ?? 1) > 1e-4) return true;
  if (f.filmLook && f.filmLook !== "none" && (f.filmStrength ?? 1) > 1e-4) return true;
  for (const [k, def] of Object.entries(DEFAULT_FILTERS)) {
    if (k === "preset" || k === "presetStrength" || k === "filmLook" || k === "filmStrength" || k === "filterMask") continue;
    if (f[k] !== undefined && f[k] !== def) return true;
  }
  return false;
}

export function hasActiveMask(layer, asset = null) {
  if (!layer?.mask?.enabled) return false;
  const m = layer.mask;
  const param = (m.mode === "parametric" || m.mode === "combined") && m.shape && m.shape !== "none";
  const raster = (m.mode === "raster" || m.mode === "combined") && Boolean(m.hasRaster || asset?.maskAlpha);
  return param || raster;
}

export function hasActiveZensr(layer) {
  if (!layer?.zensr?.enabled) return false;
  return layer.zensr.scale > 1 || (layer.zensr.strength ?? 1) > 1e-4;
}

export function effectiveLayerSize(layer) {
  if (!layer) return { width: 0, height: 0, sr: 1 };
  const sr = hasActiveZensr(layer) ? Math.max(1, layer.zensr.scale ?? 1) : 1;
  return { width: layer.width * sr, height: layer.height * sr, sr };
}

export function targetScaledDimensions(layer) {
  const eff = effectiveLayerSize(layer);
  const s = layer.scale ?? 1;
  if (Math.abs(s - 1) < 1e-4) return { width: eff.width, height: eff.height, scaled: false };
  let dstW = Math.max(1, Math.round(eff.width * s));
  let dstH = Math.max(1, Math.round(eff.height * s));
  const cap = Math.max(dstW, dstH);
  if (cap > MAX_SCALED_DIM) {
    const r = MAX_SCALED_DIM / cap;
    dstW = Math.max(1, Math.round(dstW * r));
    dstH = Math.max(1, Math.round(dstH * r));
  }
  if (dstW * dstH > MAX_SCALED_PIXELS) {
    const r = Math.sqrt(MAX_SCALED_PIXELS / (dstW * dstH));
    dstW = Math.max(1, Math.round(dstW * r));
    dstH = Math.max(1, Math.round(dstH * r));
  }
  return { width: dstW, height: dstH, scaled: dstW !== eff.width || dstH !== eff.height };
}

export function needsRasterPipeline(layer, asset, opts = {}) {
  const { scaled } = opts.interactive ? { scaled: false } : targetScaledDimensions(layer);
  return hasAdjustments(layer) || hasActiveMask(layer, asset) || hasActiveZensr(layer) || scaled ||
    Boolean(opts.rubyOverlay && layer.mask?.enabled);
}

function canvasFromPixels(pixels, width, height) {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  canvas.getContext("2d").putImageData(
    new ImageData(new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, pixels.byteLength), width, height), 0, 0);
  return canvas;
}

async function rasterizeSvgAt(svgBytes, width, height) {
  const url = URL.createObjectURL(new Blob([svgBytes], { type: "image/svg+xml" }));
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    ctx.imageSmoothingEnabled = true;
    ctx.imageSmoothingQuality = "high";
    ctx.drawImage(img, 0, 0, width, height);
    return canvas;
  } finally {
    URL.revokeObjectURL(url);
  }
}

function resampleMaskAlpha(src, srcW, srcH, dstW, dstH) {
  if (srcW === dstW && srcH === dstH) return src;
  const out = new Uint8Array(dstW * dstH);
  for (let y = 0; y < dstH; y++) {
    const sy = Math.min(srcH - 1, Math.floor((y + 0.5) * srcH / dstH));
    for (let x = 0; x < dstW; x++) {
      const sx = Math.min(srcW - 1, Math.floor((x + 0.5) * srcW / dstW));
      out[y * dstW + x] = src[sy * srcW + sx];
    }
  }
  return out;
}

function record(asset) {
  if (!cache.has(asset.canvas)) {
    cache.set(asset.canvas, {
      svg: { key: null, canvas: null },
      zensr: { key: null, canvas: null, elapsedMs: null },
      filter: { key: null, canvas: null },
      mask: { key: null, canvas: null },
      final: { key: null, canvas: null },
      pending: null,
      failure: null,
    });
  }
  return cache.get(asset.canvas);
}

function pipelineKeys(asset, layer, opts) {
  const isSvg = layer.mime === "image/svg+xml";
  const eff = effectiveLayerSize(layer);
  const target = opts.interactive
    ? { width: eff.width, height: eff.height, scaled: false }
    : targetScaledDimensions(layer);
  const svgRev = asset.svgEdit ? asset.svgEdit.byteLength : asset.source?.byteLength ?? 0;
  const svgKey = isSvg && target.scaled ? `svg:${svgRev}:${target.width}x${target.height}` : "base";
  const zensrKey = hasActiveZensr(layer)
    ? `${svgKey}|sr:${JSON.stringify(layer.zensr)}:${asset.customZensrWeights?.byteLength ?? 0}`
    : svgKey;
  const filterKey = hasAdjustments(layer)
    ? `${zensrKey}|flt:${JSON.stringify(mergedFilterSpec(layer))}`
    : zensrKey;
  const ruby = Boolean(opts.rubyOverlay && layer.mask?.enabled);
  const maskRev = asset.maskRev ?? 0;
  const maskKey = (hasActiveMask(layer, asset) || ruby)
    ? `${filterKey}|msk:${JSON.stringify(layer.mask)}:r${maskRev}:rb${ruby ? 1 : 0}`
    : filterKey;
  const resizeKey = (!isSvg && target.scaled)
    ? `${maskKey}|rsz:${target.width}x${target.height}:${layer.resizeKernel ?? "robidoux"}:${layer.resizeLinear !== false ? 1 : 0}`
    : maskKey;
  return { isSvg, target, svgKey, zensrKey, filterKey, maskKey, resizeKey, ruby };
}

async function runPipeline(asset, layer, opts, entry, onTiming) {
  const keys = pipelineKeys(asset, layer, opts);

  // Stage 0: Base or crisp scale-aware SVG rasterization
  let curCanvas = asset.canvas;
  if (keys.isSvg && keys.target.scaled) {
    if (entry.svg.key === keys.svgKey && entry.svg.canvas) {
      curCanvas = entry.svg.canvas;
    } else {
      curCanvas = await rasterizeSvgAt(asset.svgEdit ?? asset.source, keys.target.width, keys.target.height);
      entry.svg = { key: keys.svgKey, canvas: curCanvas };
    }
  }

  // Stage 1: ZenSR neural restoration / super-resolution
  if (hasActiveZensr(layer)) {
    if (entry.zensr.key === keys.zensrKey && entry.zensr.canvas) {
      curCanvas = entry.zensr.canvas;
    } else {
      const { width, height } = curCanvas;
      const { data } = curCanvas.getContext("2d").getImageData(0, 0, width, height);
      const z = layer.zensr;
      const payload = {
        pixels: data,
        width,
        height,
        model: z.model,
        weights: z.model === "custom" ? asset.customZensrWeights : undefined,
        config: {
          arch: z.arch,
          nf: z.nf ?? 24,
          nc: z.nc ?? 8,
          scale: z.scale ?? 1,
          strength: z.strength ?? 1,
          tile: 128,
        },
      };
      const res = await codec("zensrRun", payload, [data.buffer]);
      curCanvas = canvasFromPixels(res.pixels, res.width, res.height);
      entry.zensr = { key: keys.zensrKey, canvas: curCanvas, elapsedMs: res.elapsedMs };
      if (onTiming) onTiming(res.elapsedMs, res.width, res.height);
    }
  }

  // Stage 2: Full ZenFilters Oklab pipeline
  if (hasAdjustments(layer)) {
    if (entry.filter.key === keys.filterKey && entry.filter.canvas) {
      curCanvas = entry.filter.canvas;
    } else {
      const { width, height } = curCanvas;
      const { data } = curCanvas.getContext("2d").getImageData(0, 0, width, height);
      const spec = mergedFilterSpec(layer);
      const pixels = await codec("applyZenFilters", { pixels: data, width, height, spec }, [data.buffer]);
      curCanvas = canvasFromPixels(pixels, width, height);
      entry.filter = { key: keys.filterKey, canvas: curCanvas };
    }
  }

  // Stage 3: Non-destructive Layer Mask (parametric + raster + feather + optional ruby overlay)
  if (hasActiveMask(layer, asset) || keys.ruby) {
    if (entry.mask.key === keys.maskKey && entry.mask.canvas) {
      curCanvas = entry.mask.canvas;
    } else {
      const { width, height } = curCanvas;
      const { data } = curCanvas.getContext("2d").getImageData(0, 0, width, height);
      let rasterMask = new Uint8Array(0);
      if (asset.maskAlpha && (layer.mask.mode === "raster" || layer.mask.mode === "combined")) {
        rasterMask = resampleMaskAlpha(asset.maskAlpha, layer.width, layer.height, width, height);
      }
      const pixels = await codec("applyMask", {
        pixels: data,
        width,
        height,
        spec: { ...DEFAULT_MASK, ...layer.mask },
        rasterMask,
        overlayRuby: keys.ruby,
      }, [data.buffer]);
      curCanvas = canvasFromPixels(pixels, width, height);
      entry.mask = { key: keys.maskKey, canvas: curCanvas };
    }
  }

  // Stage 4: High-quality zenresize resampling (skip for SVG already rendered crisply at target)
  if (!keys.isSvg && keys.target.scaled) {
    const { width: srcW, height: srcH } = curCanvas;
    const { width: dstW, height: dstH } = keys.target;
    if (srcW !== dstW || srcH !== dstH) {
      const { data } = curCanvas.getContext("2d").getImageData(0, 0, srcW, srcH);
      const pixels = await codec("resizeRgba", {
        pixels: data,
        srcW,
        srcH,
        dstW,
        dstH,
        kernel: layer.resizeKernel ?? "robidoux",
        linear: layer.resizeLinear !== false,
      }, [data.buffer]);
      curCanvas = canvasFromPixels(pixels, dstW, dstH);
    }
  }

  entry.final = { key: keys.resizeKey, canvas: curCanvas };
  entry.failure = null;
  return curCanvas;
}

export function previewFilter(asset, layer, repaint, report, opts = {}, onTiming = null) {
  const entry = record(asset);
  const keys = pipelineKeys(asset, layer, opts);
  const key = keys.resizeKey;
  if (entry.final.key === key && entry.final.canvas) {
    return { canvas: entry.final.canvas, pending: false };
  }
  if (opts.interactive) {
    if (entry.mask.key === keys.maskKey && entry.mask.canvas) {
      return { canvas: entry.mask.canvas, pending: false };
    }
    if (!hasActiveMask(layer, asset) && !keys.ruby && entry.filter.key === keys.filterKey && entry.filter.canvas) {
      return { canvas: entry.filter.canvas, pending: false };
    }
    if (!hasActiveMask(layer, asset) && !keys.ruby && !hasAdjustments(layer) &&
        entry.zensr.key === keys.zensrKey && entry.zensr.canvas) {
      return { canvas: entry.zensr.canvas, pending: false };
    }
    if (entry.final.canvas) {
      return { canvas: entry.final.canvas, pending: false };
    }
  }
  if (entry.failure?.key === key) {
    return { canvas: entry.final.canvas ?? asset.canvas, pending: false };
  }
  if (!entry.pending) {
    entry.pending = runPipeline(asset, layer, opts, entry, onTiming)
      .catch(error => {
        entry.failure = { key, error };
        throw error;
      })
      .finally(() => { entry.pending = null; });
    entry.pending.then(repaint, error => {
      report(error);
      repaint();
    });
  }
  return { canvas: entry.final.canvas ?? asset.canvas, pending: true };
}

export async function renderFilter(asset, layer, opts = {}) {
  const entry = record(asset);
  const { resizeKey: key } = pipelineKeys(asset, layer, opts);
  if (entry.pending) {
    await entry.pending.catch(() => {});
  }
  if (entry.final.key === key && entry.final.canvas) return entry.final.canvas;
  if (entry.failure?.key === key) throw entry.failure.error;
  return runPipeline(asset, layer, opts, entry, null);
}

export function getZensrElapsed(asset) {
  return cache.get(asset?.canvas)?.zensr?.elapsedMs ?? null;
}
