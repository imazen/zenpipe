/**
 * Pipeline worker: runs zenpipe Editor in a Web Worker.
 *
 * Tries to load the WASM module (pkg/zenpipe_demo.js). If unavailable,
 * falls back to a mock editor using OffscreenCanvas.
 *
 * Protocol:
 *   Main → Worker:  { id, type, ...params }
 *   Worker → Main:  { id, type: 'result'|'ready'|'error', ...data }
 */

/// <reference lib="webworker" />

let editor = null;       // WasmEditor or MockEditor
let backend = 'pending'; // 'wasm' | 'mock' | 'pending'

// ─── WASM loading ───

let wasmModule = null;

async function tryLoadWasm() {
  try {
    const mod = await import('./pkg/zenpipe_demo.js');
    await mod.default(); // init WASM
    wasmModule = mod;
    backend = 'wasm';
    return true;
  } catch (e) {
    console.warn('WASM not available, using mock:', e.message || e);
    backend = 'mock';
    return false;
  }
}

const wasmReady = tryLoadWasm();

// ─── WASM editor wrapper ───

class WasmEditorWrapper {
  constructor(imageData) {
    const rgba = imageData.data;
    const w = imageData.width;
    const h = imageData.height;
    // Flatten ImageData to Uint8Array for WASM
    const bytes = new Uint8Array(rgba.buffer, rgba.byteOffset, rgba.byteLength);
    this.inner = new wasmModule.WasmEditor(bytes, w, h, 512, 800);
    this.width = w;
    this.height = h;
  }

  renderOverview(adjustments, maxDim) {
    const json = JSON.stringify(adjustments);
    const result = this.inner.render_overview(json);
    const w = result.width;
    const h = result.height;
    const data = new Uint8ClampedArray(result.data.slice().buffer);
    result.free();
    return new ImageData(data, w, h);
  }

  renderRegion(adjustments, region, maxDim) {
    const json = JSON.stringify(adjustments);
    const result = this.inner.render_region(json, region.x, region.y, region.w, region.h);
    const w = result.width;
    const h = result.height;
    const data = new Uint8ClampedArray(result.data.slice().buffer);
    result.free();
    return new ImageData(data, w, h);
  }

  getSchema() {
    return wasmModule.WasmEditor.get_filter_schema();
  }

  get overviewCacheLen() { return this.inner.overview_cache_len; }
  get detailCacheLen() { return this.inner.detail_cache_len; }
}

// ─── Mock editor (OffscreenCanvas fallback) ───

class MockEditor {
  constructor(imageData, bitmap) {
    this.bitmap = bitmap; // ImageBitmap for canvas drawImage
    this.imageData = imageData;
    this.width = imageData.width;
    this.height = imageData.height;
  }

  renderOverview(adjustments, maxDim) {
    const scale = Math.min(maxDim / this.width, maxDim / this.height, 1);
    const w = Math.round(this.width * scale);
    const h = Math.round(this.height * scale);
    const canvas = new OffscreenCanvas(w, h);
    const ctx = canvas.getContext('2d');
    ctx.filter = toCssFilter(adjustments);
    ctx.drawImage(this.bitmap, 0, 0, w, h);
    return ctx.getImageData(0, 0, w, h);
  }

  renderRegion(adjustments, region, maxDim) {
    const sx = Math.round(region.x * this.width);
    const sy = Math.round(region.y * this.height);
    const sw = Math.max(1, Math.round(region.w * this.width));
    const sh = Math.max(1, Math.round(region.h * this.height));
    const scale = Math.min(maxDim / sw, maxDim / sh, 2);
    const dw = Math.round(sw * scale);
    const dh = Math.round(sh * scale);
    const canvas = new OffscreenCanvas(dw, dh);
    const ctx = canvas.getContext('2d');
    ctx.filter = toCssFilter(adjustments);
    ctx.drawImage(this.bitmap, sx, sy, sw, sh, 0, 0, dw, dh);
    return ctx.getImageData(0, 0, dw, dh);
  }

  get overviewCacheLen() { return 0; }
  get detailCacheLen() { return 0; }
}

function toCssFilter(adj) {
  const parts = [];
  const exp = adj.exposure || 0;
  if (Math.abs(exp) > 0.001) parts.push(`brightness(${Math.pow(2, exp).toFixed(3)})`);
  const con = adj.contrast || 0;
  if (Math.abs(con) > 0.001) parts.push(`contrast(${(1 + con).toFixed(3)})`);
  const sat = (adj.saturation || 0) + (adj.vibrance || 0) * 0.5;
  if (Math.abs(sat) > 0.001) parts.push(`saturate(${(1 + sat).toFixed(3)})`);
  return parts.length ? parts.join(' ') : 'none';
}

// ─── Decode image bytes to ImageData ───

/**
 * Decode image bytes. Returns { imageData, bitmap }.
 * imageData has the RGBA pixels for WASM; bitmap is kept for mock canvas drawing.
 */
async function decodeImage(buffer) {
  const blob = new Blob([buffer]);
  const bitmap = await createImageBitmap(blob);
  const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
  const ctx = canvas.getContext('2d');
  ctx.drawImage(bitmap, 0, 0);
  const imageData = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
  return { imageData, bitmap };
}

// ─── Message handler ───

self.addEventListener('message', async (e) => {
  const msg = e.data;
  const id = msg.id;

  // Wait for WASM init to complete (or fail)
  await wasmReady;

  try {
    switch (msg.type) {
      case 'init': {
        const { imageData, bitmap } = await decodeImage(msg.data);
        editor = backend === 'wasm'
          ? new WasmEditorWrapper(imageData)
          : new MockEditor(imageData, bitmap);

        self.postMessage({
          id, type: 'ready',
          width: editor.width,
          height: editor.height,
          backend,
        });
        break;
      }

      case 'overview': {
        if (!editor) throw new Error('Editor not initialized');
        const result = editor.renderOverview(
          msg.adjustments || {},
          msg.maxDim || 512,
        );
        self.postMessage(
          { id, type: 'result', imageData: result, width: result.width, height: result.height, backend },
          { transfer: [result.data.buffer] },
        );
        break;
      }

      case 'region': {
        if (!editor) throw new Error('Editor not initialized');
        const result = editor.renderRegion(
          msg.adjustments || {},
          msg.rect || { x: 0.25, y: 0.25, w: 0.5, h: 0.5 },
          msg.maxDim || 800,
        );
        self.postMessage(
          { id, type: 'result', imageData: result, width: result.width, height: result.height, backend },
          { transfer: [result.data.buffer] },
        );
        break;
      }

      case 'export': {
        if (!editor) throw new Error('Editor not initialized');
        // Full-res render then encode via canvas
        const result = editor.renderOverview(
          msg.adjustments || {},
          Math.max(editor.width, editor.height),
        );
        const canvas = new OffscreenCanvas(result.width, result.height);
        const ctx = canvas.getContext('2d');
        ctx.putImageData(result, 0, 0);
        const blob = await canvas.convertToBlob({
          type: msg.format === 'png' ? 'image/png' : 'image/jpeg',
          quality: (msg.quality || 85) / 100,
        });
        const data = new Uint8Array(await blob.arrayBuffer());
        self.postMessage(
          { id, type: 'result', data, format: msg.format || 'jpeg', size: data.length, backend },
          { transfer: [data.buffer] },
        );
        break;
      }

      case 'schema': {
        // Return schema from WASM if available, otherwise null (main thread uses static file)
        let schema = null;
        if (backend === 'wasm' && wasmModule) {
          schema = wasmModule.WasmEditor.get_filter_schema();
        }
        self.postMessage({ id, type: 'result', schema, backend });
        break;
      }

      default:
        throw new Error(`Unknown message type: ${msg.type}`);
    }
  } catch (err) {
    self.postMessage({ id, type: 'error', message: err.message || String(err) });
  }
});
