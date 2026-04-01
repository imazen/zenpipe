/**
 * Pipeline worker: runs zenpipe Editor in a Web Worker.
 *
 * Handles image init, overview render, region render, and export.
 * All pixel work happens here — main thread only does UI and CSS previews.
 *
 * Protocol:
 *   Main → Worker:  { id, type, ...params }
 *   Worker → Main:  { id, type: 'result'|'error', ...data }
 *
 * In WASM mode, loads the zenpipe-demo WASM module.
 * In mock mode (no WASM), uses OffscreenCanvas for basic rendering.
 */

/// <reference lib="webworker" />

let editor = null;  // Will hold WASM Editor or mock editor
let wasmReady = false;

// ─── Mock editor (browser-native, no WASM) ───

class MockEditor {
  constructor(imageData) {
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
    ctx.drawImage(this.imageData, 0, 0, w, h);
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
    ctx.drawImage(this.imageData, sx, sy, sw, sh, 0, 0, dw, dh);
    return ctx.getImageData(0, 0, dw, dh);
  }
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

// ─── Message handler ───

self.addEventListener('message', async (e) => {
  const msg = e.data;
  const id = msg.id;

  try {
    switch (msg.type) {
      case 'init': {
        // Decode image bytes to ImageData via OffscreenCanvas
        const blob = new Blob([msg.data]);
        const bitmap = await createImageBitmap(blob);
        const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
        const ctx = canvas.getContext('2d');
        ctx.drawImage(bitmap, 0, 0);
        const imageData = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
        bitmap.close();

        editor = new MockEditor(imageData);

        self.postMessage({
          id, type: 'ready',
          width: editor.width,
          height: editor.height,
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
          { id, type: 'result', imageData: result, width: result.width, height: result.height },
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
          { id, type: 'result', imageData: result, width: result.width, height: result.height },
          { transfer: [result.data.buffer] },
        );
        break;
      }

      case 'export': {
        if (!editor) throw new Error('Editor not initialized');
        // Full-res render for export (mock: just re-render at source size)
        const result = editor.renderOverview(msg.adjustments || {}, Math.max(editor.width, editor.height));
        const canvas = new OffscreenCanvas(result.width, result.height);
        const ctx = canvas.getContext('2d');
        ctx.putImageData(result, 0, 0);
        const blob = await canvas.convertToBlob({
          type: msg.format === 'png' ? 'image/png' : 'image/jpeg',
          quality: (msg.quality || 85) / 100,
        });
        const data = new Uint8Array(await blob.arrayBuffer());
        self.postMessage(
          { id, type: 'result', data, format: msg.format || 'jpeg', size: data.length },
          { transfer: [data.buffer] },
        );
        break;
      }

      default:
        throw new Error(`Unknown message type: ${msg.type}`);
    }
  } catch (err) {
    self.postMessage({ id, type: 'error', message: err.message || String(err) });
  }
});
