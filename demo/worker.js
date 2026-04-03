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
let originalBytes = null; // Original file bytes for native decode upgrade

// ─── WASM loading ───

let wasmModule = null;

let threadsAvailable = false;

async function tryLoadWasm() {
  try {
    const mod = await import('./pkg/zenpipe_demo.js');
    await mod.default(); // init WASM

    // Initialize rayon thread pool if available (requires SharedArrayBuffer + COOP/COEP)
    if (typeof mod.initThreadPool === 'function' && typeof SharedArrayBuffer !== 'undefined') {
      try {
        const cores = navigator.hardwareConcurrency || 4;
        await mod.initThreadPool(Math.min(cores, 8));
        threadsAvailable = true;
        console.log(`WASM thread pool: ${Math.min(cores, 8)} threads`);
      } catch (e) {
        console.warn('Thread pool init failed (single-threaded mode):', e.message || e);
      }
    }

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

  renderOverview(adjustments, maxDim, filmPreset) {
    const json = JSON.stringify(adjustments);
    const result = filmPreset
      ? this.inner.render_overview(json, filmPreset)
      : this.inner.render_overview(json);
    const w = result.width;
    const h = result.height;
    const data = new Uint8ClampedArray(result.data.slice().buffer);
    result.free();
    return new ImageData(data, w, h);
  }

  renderRegion(adjustments, region, maxDim, filmPreset) {
    const json = JSON.stringify(adjustments);
    const result = filmPreset
      ? this.inner.render_region(json, region.x, region.y, region.w, region.h, filmPreset)
      : this.inner.render_region(json, region.x, region.y, region.w, region.h);
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

  renderOverview(adjustments, maxDim, _filmPreset) {
    const scale = Math.min(maxDim / this.width, maxDim / this.height, 1);
    const w = Math.round(this.width * scale);
    const h = Math.round(this.height * scale);
    const canvas = new OffscreenCanvas(w, h);
    const ctx = canvas.getContext('2d');
    ctx.filter = toCssFilter(adjustments);
    ctx.drawImage(this.bitmap, 0, 0, w, h);
    return ctx.getImageData(0, 0, w, h);
  }

  renderRegion(adjustments, region, maxDim, _filmPreset) {
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
  const bytes = new Uint8Array(buffer);

  // Try WASM decode first for formats browsers may not support (JXL, AVIF)
  if (backend === 'wasm' && wasmModule?.wasm_can_decode(bytes)) {
    const result = wasmModule.wasm_decode_image(bytes);
    if (result) {
      const w = result.width;
      const h = result.height;
      const rgba = new Uint8ClampedArray(result.data.slice().buffer);
      result.free();
      const imageData = new ImageData(rgba, w, h);
      const bitmap = await createImageBitmap(imageData);
      return { imageData, bitmap, decoder: 'wasm' };
    }
  }

  // Browser-native decode (JPEG, PNG, WebP, GIF, and browser-supported AVIF)
  try {
    const blob = new Blob([buffer]);
    const bitmap = await createImageBitmap(blob);
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const ctx = canvas.getContext('2d');
    ctx.drawImage(bitmap, 0, 0);
    const imageData = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
    return { imageData, bitmap, decoder: 'browser' };
  } catch (browserErr) {
    // Browser can't decode — try WASM for any format as last resort
    if (backend === 'wasm') {
      const result = wasmModule.wasm_decode_image(bytes);
      if (result) {
        const w = result.width;
        const h = result.height;
        const rgba = new Uint8ClampedArray(result.data.slice().buffer);
        result.free();
        const imageData = new ImageData(rgba, w, h);
        const bitmap = await createImageBitmap(imageData);
        return { imageData, bitmap, decoder: 'wasm-fallback' };
      }
    }
    throw new Error(`Cannot decode image: ${browserErr.message}`);
  }
}

// ─── Message handler ───

// Serialize all message processing to prevent concurrent &mut self borrows
// on WasmEditor. Without this, two async handlers (e.g., encode_preview +
// export) can both suspend at `await wasmReady`, both resume, and both
// try to borrow &mut self — causing wasm-bindgen's aliasing error.
let messageQueue = Promise.resolve();

self.addEventListener('message', (e) => {
  const msg = e.data;
  // Chain sequentially; always resolve so the chain never breaks.
  // handleMessage has its own try/catch that sends error responses.
  messageQueue = messageQueue.then(
    () => handleMessage(msg),
    () => handleMessage(msg),  // If prior handler rejected, still run next
  );
});

async function handleMessage(msg) {
  const id = msg.id;

  // Wait for WASM init to complete (or fail)
  await wasmReady;

  try {
    switch (msg.type) {
      case 'init': {
        // Store original bytes for native decode upgrade
        originalBytes = new Uint8Array(msg.data);

        const { imageData, bitmap, decoder } = await decodeImage(msg.data);
        editor = backend === 'wasm'
          ? new WasmEditorWrapper(imageData)
          : new MockEditor(imageData, bitmap);

        self.postMessage({
          id, type: 'ready',
          width: editor.width,
          height: editor.height,
          backend,
          decoder,
          threads: threadsAvailable,
        });
        break;
      }

      case 'upgrade': {
        // Phase 2: native decode via zencodecs with metadata preservation
        if (!editor) throw new Error('Editor not initialized');
        if (backend !== 'wasm') throw new Error('Native decode requires WASM backend');
        if (!originalBytes) throw new Error('No original bytes available');

        const result = editor.inner.upgrade_from_bytes(originalBytes);
        // Free the stored bytes — no longer needed
        originalBytes = null;

        self.postMessage({
          id, type: 'result',
          format: result.format,
          width: result.width,
          height: result.height,
          has_icc: result.has_icc,
          has_exif: result.has_exif,
          has_xmp: result.has_xmp,
          has_gain_map: result.has_gain_map,
          backend,
        });
        result.free();
        break;
      }

      case 'overview': {
        if (!editor) throw new Error('Editor not initialized');
        const result = editor.renderOverview(
          msg.adjustments || {},
          msg.maxDim || 512,
          msg.film_preset || null,
        );
        // Send raw RGBA bytes + dimensions (not ImageData — its buffer
        // gets detached on transfer, breaking putImageData on the main thread).
        const pixels = new Uint8Array(result.data.buffer, result.data.byteOffset, result.data.byteLength);
        self.postMessage(
          { id, type: 'result', pixels, width: result.width, height: result.height, backend },
          { transfer: [pixels.buffer] },
        );
        break;
      }

      case 'region': {
        if (!editor) throw new Error('Editor not initialized');
        const result = editor.renderRegion(
          msg.adjustments || {},
          msg.rect || { x: 0.25, y: 0.25, w: 0.5, h: 0.5 },
          msg.maxDim || 800,
          msg.film_preset || null,
        );
        const pixels = new Uint8Array(result.data.buffer, result.data.byteOffset, result.data.byteLength);
        self.postMessage(
          { id, type: 'result', pixels, width: result.width, height: result.height, backend },
          { transfer: [pixels.buffer] },
        );
        break;
      }

      case 'encode_preview': {
        // Encode at overview size for inline preview (cache hit, near-instant)
        if (!editor) throw new Error('Editor not initialized');
        if (backend !== 'wasm') throw new Error('Encode preview requires WASM backend');

        const adjustmentsJson = JSON.stringify(msg.adjustments || {});
        const optionsJson = JSON.stringify(msg.options || {});
        const filmPreset = msg.film_preset || undefined;

        const encResult = editor.inner.encode_preview(
          adjustmentsJson, msg.format || 'jpeg', optionsJson, filmPreset,
        );

        const data = new Uint8Array(encResult.data.slice().buffer);
        const w = encResult.width;
        const h = encResult.height;
        const size = encResult.size;
        const mime = encResult.mime;
        encResult.free();

        self.postMessage(
          { id, type: 'result', data, format: msg.format, mime,
            size, width: w, height: h,
            bpp: (size * 8 / (w * h)).toFixed(2), backend },
          { transfer: [data.buffer] },
        );
        break;
      }

      case 'export': {
        if (!editor) throw new Error('Editor not initialized');

        const format = msg.format || 'jpeg';
        const exportWidth = msg.width || editor.width;
        const exportHeight = msg.height || editor.height;

        if (backend === 'wasm') {
          const adjustmentsJson = JSON.stringify(msg.adjustments || {});
          const optionsJson = JSON.stringify(msg.options || {});
          const filmPreset = msg.film_preset || undefined;

          const encResult = editor.inner.encode_full(
            adjustmentsJson, exportWidth, exportHeight,
            format, optionsJson, filmPreset,
          );

          const data = new Uint8Array(encResult.data.slice().buffer);
          const resultMime = encResult.mime;
          const resultWidth = encResult.width;
          const resultHeight = encResult.height;
          const resultSize = encResult.size;
          encResult.free();

          self.postMessage(
            { id, type: 'result', data, format, mime: resultMime,
              size: resultSize, width: resultWidth, height: resultHeight, backend },
            { transfer: [data.buffer] },
          );
          break;
        }

        // Fallback: browser-native encoding (mock backend).
        const exportMaxDim = Math.max(exportWidth, exportHeight);
        const result = editor.renderOverview(msg.adjustments || {}, exportMaxDim, msg.film_preset || null);

        const canvas = new OffscreenCanvas(result.width, result.height);
        const ctx = canvas.getContext('2d');
        ctx.putImageData(result, 0, 0);

        const MIME_MAP = {
          jpeg: 'image/jpeg', webp: 'image/webp', png: 'image/png',
          avif: 'image/avif', jxl: 'image/jxl', gif: 'image/gif',
        };
        const mime = MIME_MAP[format] || 'image/jpeg';
        const quality = (format === 'png') ? undefined : (msg.quality || 85) / 100;

        let blob;
        try {
          blob = await canvas.convertToBlob({ type: mime, quality });
        } catch {
          blob = await canvas.convertToBlob({ type: 'image/jpeg', quality: quality || 0.85 });
        }

        const data = new Uint8Array(await blob.arrayBuffer());
        self.postMessage(
          { id, type: 'result', data, format, size: data.length,
            width: result.width, height: result.height, backend },
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
}
