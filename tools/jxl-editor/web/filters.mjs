import { codec } from "./codec.mjs";

export const ZEN_FILTERS = "zenfilters-v1";
const cache = new WeakMap();

export function hasAdjustments({ exposure, contrast, saturation }) {
  return exposure !== 0 || contrast !== 1 || saturation !== 1;
}

function filterKey({ exposure, contrast, saturation }) {
  return `${exposure}/${contrast}/${saturation}`;
}

function record(asset) {
  if (!cache.has(asset.canvas)) cache.set(asset.canvas, {});
  return cache.get(asset.canvas);
}

function start(asset, layer, key, entry) {
  const { width, height } = asset.canvas;
  const { data } = asset.canvas.getContext("2d").getImageData(0, 0, width, height);
  const { exposure, contrast, saturation } = layer;
  entry.pending = codec("filter", { pixels: data, width, height, exposure, contrast, saturation }, [data.buffer])
    .then(pixels => {
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      canvas.getContext("2d").putImageData(new ImageData(
        new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, pixels.byteLength), width, height), 0, 0);
      entry.canvas = canvas;
      entry.key = key;
      entry.failure = null;
      return canvas;
    })
    .catch(error => {
      entry.failure = { key, error };
      throw error;
    })
    .finally(() => { entry.pending = null; });
  return entry.pending;
}

export function previewFilter(asset, layer, repaint, report) {
  const entry = record(asset);
  const key = filterKey(layer);
  if (entry.key === key) return { canvas: entry.canvas, pending: false };
  if (entry.failure?.key === key) return { canvas: asset.canvas, pending: false };
  if (!entry.pending) start(asset, layer, key, entry).then(repaint, error => {
    report(error);
    repaint();
  });
  return { canvas: entry.canvas ?? asset.canvas, pending: true };
}

export async function renderFilter(asset, layer) {
  const entry = record(asset);
  const key = filterKey(layer);
  if (entry.pending) {
    // An obsolete preview can fail independently of the selected settings.
    await entry.pending.catch(() => {});
  }
  if (entry.key === key) return entry.canvas;
  if (entry.failure?.key === key) throw entry.failure.error;
  return start(asset, layer, key, entry);
}
