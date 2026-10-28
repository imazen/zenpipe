// Source-only protocol control. Canvas encoding is stubbed; browser readback
// lives in tests/encode-preview.spec.js and is a separate qualification gate.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

async function preview(format) {
  const messages = [], encodes = [];
  const context = vm.createContext({
    Blob, Uint8Array, Uint8ClampedArray,
    console: { warn() {} },
    self: { addEventListener() {}, postMessage(message) { messages.push(message); } },
    OffscreenCanvas: class {
      constructor(width, height) { this.width = width; this.height = height; }
      getContext() { return { putImageData() {} }; }
      async convertToBlob(options) {
        encodes.push(options);
        return new Blob([new Uint8Array([1, 2, 3])], {
          type: options.type === 'image/jpeg' ? 'image/jpeg' : 'image/png',
        });
      }
    },
  });
  const source = process.env.WORKER_SOURCE || path.join(__dirname, '../worker.js');
  new vm.Script(fs.readFileSync(source, 'utf8'), {
    filename: source,
    importModuleDynamically: () => Promise.reject(new Error('explicit mock control')),
  }).runInContext(context);
  await vm.runInContext('wasmReady', context);
  vm.runInContext(`editor = {
    renderOverview(adjustments, maxDim) {
      if (adjustments.exposure !== 1 || maxDim !== 512) throw Error('preview settings lost');
      return { data: new Uint8ClampedArray(24), width: 3, height: 2 };
    }
  };`, context);
  context.request = { id: 7, type: 'encode_preview', format,
    adjustments: { exposure: 1 }, options: { quality: 65 } };
  await vm.runInContext('handleMessage(request)', context);
  return { result: messages[0], encodes };
}

test('mock encode_preview returns bytes, dimensions and actual MIME', async () => {
  const { result, encodes } = await preview('jpeg');
  assert.equal(result.type, 'result', result.message);
  assert.equal(result.backend, 'mock');
  assert.equal(result.mime, 'image/jpeg');
  assert.equal(result.width, 3);
  assert.equal(result.height, 2);
  assert.equal(result.size, result.data.byteLength);
  assert.equal(result.size, 3);
  assert.equal(encodes[0].quality, 0.65);
});

test('mock encode_preview reports the browser fallback MIME', async () => {
  const { result } = await preview('avif');
  assert.equal(result.type, 'result', result.message);
  assert.equal(result.mime, 'image/png');
});
