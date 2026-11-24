// A function containing i8x16.relaxed_swizzle. Validation probes the opcode
// before loading a larger module compiled with relaxed SIMD enabled.
const relaxedProbe = Uint8Array.of(0, 97, 115, 109, 1, 0, 0, 0,
  1, 5, 1, 96, 1, 123, 0, 3, 2, 1, 0,
  10, 12, 1, 10, 0, 32, 0, 32, 0, 253, 128, 2, 26, 11);

let ready;
let filters;
let native;
let zensr;
let decoders;
let libjxl;
const zensrWeightsCache = new Map();

function initialize() {
  return ready ??= (async () => {
    const optimized = WebAssembly.validate(relaxedProbe);
    const codec = optimized ? await import("./wasm/relaxed/jxl_editor_codecs.js")
      : await import("./wasm/baseline/jxl_editor_codecs.js");
    await codec.default();
    return { codec, optimized, mode: optimized ? "Relaxed SIMD" : "Wasm compatibility" };
  })();
}

async function loadFilters(optimized) {
  return filters ??= (async () => {
    const module = optimized ? await import("./wasm/relaxed/jxl_editor_filters.js")
      : await import("./wasm/baseline/jxl_editor_filters.js");
    await module.default();
    return module;
  })();
}

async function loadNative(optimized) {
  return native ??= (async () => {
    const module = optimized ? await import("./wasm/relaxed/jxl_editor_native.js")
      : await import("./wasm/baseline/jxl_editor_native.js");
    await module.default();
    return module;
  })();
}

async function loadZensr(optimized) {
  return zensr ??= (async () => {
    const module = optimized ? await import("./wasm/relaxed/jxl_editor_zensr.js")
      : await import("./wasm/baseline/jxl_editor_zensr.js");
    await module.default();
    return module;
  })();
}

async function loadDecoders(optimized) {
  return decoders ??= (async () => {
    const module = optimized ? await import("./wasm/relaxed/jxl_editor_decoders.js")
      : await import("./wasm/baseline/jxl_editor_decoders.js");
    await module.default();
    return module;
  })();
}

async function loadLibjxl() {
  return libjxl ??= (async () => {
    const mod = await import("./wasm/libjxl/libjxl_dec.mjs");
    const factory = mod.default;
    return factory({
      locateFile: p => new URL(`./wasm/libjxl/${p}`, import.meta.url).href,
    });
  })();
}

async function decodeWithLibjxl(bytes) {
  const t0 = performance.now();
  try {
    const mod = await loadLibjxl();
    const ptr = mod._malloc(bytes.byteLength);
    if (!ptr) throw new Error("libjxl WASM malloc failed.");
    try {
      mod.HEAPU8.set(bytes, ptr);
      mod._libjxl_decode_rgba(ptr, bytes.byteLength);
      const reportJson = mod.UTF8ToString(mod._libjxl_result_json());
      const report = JSON.parse(reportJson);
      const pLen = mod._libjxl_result_pixels_len();
      let pixels = new Uint8Array(0);
      if (report.ok && pLen > 0) {
        const pPtr = mod._libjxl_result_pixels();
        pixels = mod.HEAPU8.subarray(pPtr, pPtr + pLen).slice();
      }
      mod._libjxl_free_result();
      report.elapsedMs = Math.round(performance.now() - t0);
      return { report, pixels };
    } finally {
      mod._free(ptr);
    }
  } catch (error) {
    // Reset instance in case of WASM trap / OOM so next call starts fresh.
    libjxl = null;
    return {
      report: {
        decoder: "libjxl",
        version: "libjxl 0.12.0 (lcms2 wasm)",
        ok: false,
        error: error instanceof Error ? error.message : String(error),
        width: 0,
        height: 0,
        frameCount: 0,
        frameDurationsMs: [],
        loopCount: null,
        hasAlpha: false,
        hasIcc: false,
        isCmyk: false,
        usesOriginalProfile: false,
        extraChannels: 0,
        orientation: 1,
        cmsUsed: "lcms2",
        level5Accepted: null,
        warnings: [],
        elapsedMs: Math.round(performance.now() - t0),
      },
      pixels: new Uint8Array(0),
    };
  }
}

function compareDecoderBuffers(aId, aMeta, aPx, bId, bMeta, bPx) {
  const sizeMismatch = aMeta.width !== bMeta.width || aMeta.height !== bMeta.height || aPx.byteLength !== bPx.byteLength;
  const frameCountMismatch = aMeta.frameCount != null && bMeta.frameCount != null && aMeta.frameCount !== bMeta.frameCount;
  let durationMismatch = false;
  if (Array.isArray(aMeta.frameDurationsMs) && Array.isArray(bMeta.frameDurationsMs) &&
      aMeta.frameDurationsMs.length && bMeta.frameDurationsMs.length) {
    if (aMeta.frameDurationsMs.length !== bMeta.frameDurationsMs.length) {
      durationMismatch = true;
    } else {
      for (let i = 0; i < aMeta.frameDurationsMs.length; i++) {
        if (Math.abs(aMeta.frameDurationsMs[i] - bMeta.frameDurationsMs[i]) > 1) {
          durationMismatch = true;
          break;
        }
      }
    }
  }
  if (sizeMismatch) {
    return {
      a: aId,
      b: bId,
      exactMatch: false,
      sizeMismatch: true,
      frameCountMismatch,
      durationMismatch,
      maxAbsDelta: 255,
      maxRgbDelta: 255,
      maxAlphaDelta: 255,
      meanAbsDelta: 255,
      diffPixels: Math.max(aMeta.width * aMeta.height, bMeta.width * bMeta.height),
      diffOver1Pixels: Math.max(aMeta.width * aMeta.height, bMeta.width * bMeta.height),
      diffPercent: 100,
      psnrDb: 0,
    };
  }
  const nPixels = Math.floor(aPx.byteLength / 4);
  let maxRgbDelta = 0;
  let maxAlphaDelta = 0;
  let sumAbs = 0;
  let sumSq = 0;
  let diffPixels = 0;
  let diffOver1Pixels = 0;
  for (let i = 0; i < aPx.byteLength; i += 4) {
    const dr = Math.abs(aPx[i] - bPx[i]);
    const dg = Math.abs(aPx[i + 1] - bPx[i + 1]);
    const db = Math.abs(aPx[i + 2] - bPx[i + 2]);
    const da = Math.abs(aPx[i + 3] - bPx[i + 3]);
    const mRgb = dr > dg ? (dr > db ? dr : db) : (dg > db ? dg : db);
    if (mRgb > maxRgbDelta) maxRgbDelta = mRgb;
    if (da > maxAlphaDelta) maxAlphaDelta = da;
    const mAll = mRgb > da ? mRgb : da;
    if (mAll > 0) {
      diffPixels++;
      if (mAll > 1) diffOver1Pixels++;
    }
    sumAbs += dr + dg + db + da;
    sumSq += dr * dr + dg * dg + db * db + da * da;
  }
  const maxAbsDelta = Math.max(maxRgbDelta, maxAlphaDelta);
  const totalSamples = Math.max(1, aPx.byteLength);
  const meanAbsDelta = sumAbs / totalSamples;
  const mse = sumSq / totalSamples;
  const psnrDb = mse === 0 ? null : Number((10 * Math.log10((255 * 255) / mse)).toFixed(2));
  return {
    a: aId,
    b: bId,
    exactMatch: maxAbsDelta === 0 && !frameCountMismatch && !durationMismatch,
    sizeMismatch: false,
    frameCountMismatch,
    durationMismatch,
    maxAbsDelta,
    maxRgbDelta,
    maxAlphaDelta,
    meanAbsDelta: Number(meanAbsDelta.toFixed(4)),
    diffPixels,
    diffOver1Pixels,
    diffPercent: Number(((diffPixels * 100) / Math.max(1, nPixels)).toFixed(3)),
    psnrDb,
  };
}

const SHA256_K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

function sha256PureJs(bytes) {
  let h0 = 0x6a09e667, h1 = 0xbb67ae85, h2 = 0x3c6ef372, h3 = 0xa54ff53a;
  let h4 = 0x510e527f, h5 = 0x9b05688c, h6 = 0x1f83d9ab, h7 = 0x5be0cd19;
  const len = bytes.byteLength;
  const totalLen = ((len + 9 + 63) >> 6) << 6;
  const padded = new Uint8Array(totalLen);
  padded.set(bytes);
  padded[len] = 0x80;
  const dv = new DataView(padded.buffer);
  dv.setUint32(totalLen - 8, Math.floor(len / 0x20000000), false);
  dv.setUint32(totalLen - 4, (len << 3) >>> 0, false);
  const w = new Uint32Array(64);
  for (let off = 0; off < totalLen; off += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(off + i * 4, false);
    for (let i = 16; i < 64; i++) {
      const s0 = ((w[i - 15] >>> 7) | (w[i - 15] << 25)) ^ ((w[i - 15] >>> 18) | (w[i - 15] << 14)) ^ (w[i - 15] >>> 3);
      const s1 = ((w[i - 2] >>> 17) | (w[i - 2] << 15)) ^ ((w[i - 2] >>> 19) | (w[i - 2] << 13)) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) >>> 0;
    }
    let a = h0, b = h1, c = h2, d = h3, e = h4, f = h5, g = h6, h = h7;
    for (let i = 0; i < 64; i++) {
      const S1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7));
      const ch = (e & f) ^ (~e & g);
      const t1 = (h + S1 + ch + SHA256_K[i] + w[i]) >>> 0;
      const S0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10));
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + maj) >>> 0;
      h = g; g = f; f = e; e = (d + t1) >>> 0;
      d = c; c = b; b = a; a = (t1 + t2) >>> 0;
    }
    h0 = (h0 + a) >>> 0; h1 = (h1 + b) >>> 0; h2 = (h2 + c) >>> 0; h3 = (h3 + d) >>> 0;
    h4 = (h4 + e) >>> 0; h5 = (h5 + f) >>> 0; h6 = (h6 + g) >>> 0; h7 = (h7 + h) >>> 0;
  }
  return [h0, h1, h2, h3, h4, h5, h6, h7].map(v => v.toString(16).padStart(8, "0")).join("");
}

async function sha256Hex(bytes) {
  if (crypto?.subtle) {
    const digest = await crypto.subtle.digest("SHA-256", bytes);
    return Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, "0")).join("");
  }
  return sha256PureJs(bytes);
}

async function fetchModelWeights(entry) {
  if (zensrWeightsCache.has(entry.id)) return zensrWeightsCache.get(entry.id);
  const res = await fetch(entry.weightsUrl);
  if (!res.ok) throw new Error(`Failed to fetch ZenSR weights (${res.status}): ${entry.weightsUrl}`);
  const bytes = new Uint8Array(await res.arrayBuffer());
  if (entry.weightsSha256) {
    const actual = await sha256Hex(bytes);
    if (actual !== entry.weightsSha256) {
      throw new Error(`ZenSR weights SHA-256 mismatch for ${entry.id}.`);
    }
  }
  zensrWeightsCache.set(entry.id, bytes);
  return bytes;
}

self.onmessage = async ({ data }) => {
  const { id, method } = data;
  try {
    const { codec, mode, optimized } = await initialize();
    if (method === "init") {
      self.postMessage({ id, value: mode });
    } else if (["nativeInfo", "nativeFrame", "nativeIcc", "nativeJpeg", "nativeTimeline", "nativeCmyk"].includes(method)) {
      const nativeModule = await loadNative(optimized);
      if (method === "nativeInfo") {
        self.postMessage({ id, value: JSON.parse(nativeModule.native_jxl_info(data.bytes)) });
      } else if (method === "nativeIcc" || method === "nativeJpeg") {
        const value = method === "nativeIcc" ? nativeModule.native_embedded_icc(data.bytes)
          : nativeModule.native_reconstruct_jpeg(data.bytes);
        self.postMessage({ id, value }, [value.buffer]);
      } else if (method === "nativeTimeline") {
        const timeline = nativeModule.native_jxl_timeline(data.bytes);
        const info = JSON.parse(timeline.info());
        const width = timeline.width();
        const height = timeline.height();
        const pixels = timeline.into_pixels();
        self.postMessage({ id, value: { info, width, height, pixels } }, [pixels.buffer]);
      } else if (method === "nativeCmyk") {
        const cmyk = nativeModule.native_extract_cmyk(data.bytes);
        try {
          const width = cmyk.width();
          const height = cmyk.height();
          const bitDepth = cmyk.bit_depth();
          const usesOriginalProfile = cmyk.uses_original_profile();
          const samples = cmyk.take_samples();
          const icc = cmyk.take_icc();
          self.postMessage({
            id,
            value: { width, height, bitDepth, usesOriginalProfile, samples, icc },
          }, [samples.buffer, icc.buffer]);
        } finally {
          cmyk.free();
        }
      } else {
        const frame = nativeModule.native_jxl_frame(data.bytes, data.index);
        try {
          const info = JSON.parse(frame.info());
          const preview = frame.take_preview();
          const previewError = frame.preview_error();
          const planes = data.includePlanes
            ? Array.from({ length: frame.plane_count() }, (_, i) => frame.take_plane(i))
            : [];
          self.postMessage({ id, value: { info, preview, previewError, planes } },
            [preview.buffer, ...planes.map(plane => plane.buffer)]);
        } finally {
          frame.free();
        }
      }
    } else if (["filter", "filterCatalog", "applyZenFilters", "resizeKernels", "resizeRgba", "applyMask"].includes(method)) {
      const filterModule = await loadFilters(optimized);
      if (method === "filterCatalog") {
        self.postMessage({ id, value: JSON.parse(filterModule.filter_catalog_json()) });
      } else if (method === "resizeKernels") {
        self.postMessage({ id, value: JSON.parse(filterModule.resize_kernels_json()) });
      } else if (method === "filter") {
        const value = filterModule.filter_rgba(data.pixels, data.width, data.height,
          data.exposure, data.contrast, data.saturation);
        self.postMessage({ id, value }, [value.buffer]);
      } else if (method === "applyZenFilters") {
        const value = filterModule.apply_zenfilters_rgba(
          data.pixels, data.width, data.height, JSON.stringify(data.spec));
        self.postMessage({ id, value }, [value.buffer]);
      } else if (method === "resizeRgba") {
        const value = filterModule.resize_rgba(
          data.pixels, data.srcW, data.srcH, data.dstW, data.dstH,
          data.kernel ?? "robidoux", data.linear !== false);
        self.postMessage({ id, value }, [value.buffer]);
      } else if (method === "applyMask") {
        const value = filterModule.apply_layer_mask_rgba(
          data.pixels, data.width, data.height, JSON.stringify(data.spec),
          data.rasterMask ?? new Uint8Array(0), Boolean(data.overlayRuby));
        self.postMessage({ id, value }, [value.buffer]);
      }
    } else if (["zensrCatalog", "zensrRun"].includes(method)) {
      const zensrModule = await loadZensr(optimized);
      if (method === "zensrCatalog") {
        self.postMessage({ id, value: JSON.parse(zensrModule.zensr_catalog_json()) });
      } else if (method === "zensrRun") {
        let weights = data.weights;
        if (!weights) {
          const catalog = JSON.parse(zensrModule.zensr_catalog_json());
          const entry = catalog.find(m => m.id === data.model) ?? catalog[0];
          if (!entry) throw new Error(`Unknown ZenSR model '${data.model}'.`);
          weights = await fetchModelWeights(entry);
        }
        const t0 = performance.now();
        const result = zensrModule.zensr_run_rgba(
          data.pixels, data.width, data.height, weights, JSON.stringify(data.config));
        const width = result.width();
        const height = result.height();
        const pixels = result.into_pixels();
        const elapsedMs = Math.round(performance.now() - t0);
        self.postMessage({ id, value: { width, height, pixels, elapsedMs } }, [pixels.buffer]);
      }
    } else if (method === "encode") {
      const value = codec.encode_jxl(data.pixels, data.width, data.height, data.stride);
      self.postMessage({ id, value }, [value.buffer]);
    } else if (method === "encodeConfigured") {
      const value = codec.encode_jxl_configured(
        data.pixels, data.width, data.height, JSON.stringify(data.options));
      self.postMessage({ id, value }, [value.buffer]);
    } else if (method === "encodeTimeline") {
      const value = codec.encode_timeline_configured(
        data.pixels, data.width, data.height, JSON.stringify(data.options));
      self.postMessage({ id, value }, [value.buffer]);
    } else if (method === "encodeLayers") {
      const value = codec.encode_layers(data.pixels, data.expected, data.width, data.height, data.ids);
      self.postMessage({ id, value }, value ? [value.buffer] : []);
    } else if (method === "renderSplines") {
      const value = codec.render_splines_rgba(
        data.pixels, data.width, data.height, JSON.stringify(data.splines));
      self.postMessage({ id, value }, [value.buffer]);
    } else if (method === "splineColorDct") {
      const value = codec.spline_color_dct_from_srgb(
        data.fgR, data.fgG, data.fgB, data.bgR, data.bgG, data.bgB, data.sigma);
      self.postMessage({ id, value: Array.from(value) });
    } else if (method === "minimalCmykIcc") {
      const value = codec.generate_minimal_cmyk_icc();
      self.postMessage({ id, value }, [value.buffer]);
    } else if (method === "rgbaToCmyk") {
      const value = data.bitDepth === 16
        ? codec.rgba8_to_cmyk16_jxl(data.pixels)
        : codec.rgba8_to_cmyk8_jxl(data.pixels);
      self.postMessage({ id, value }, [value.buffer]);
    } else if (method === "encodeCmyk") {
      const value = codec.encode_cmyk_jxl(
        data.samples, data.width, data.height, data.bitDepth, data.icc, JSON.stringify(data.options));
      self.postMessage({ id, value }, [value.buffer]);
    } else if (method === "previewCmyk") {
      const value = codec.preview_cmyk_to_rgba8(
        data.samples, data.width, data.height, data.bitDepth, data.icc);
      self.postMessage({ id, value }, [value.buffer]);
    } else if (method === "decodersCatalog") {
      const decMod = await loadDecoders(optimized);
      const rustList = JSON.parse(decMod.decoders_catalog_json());
      let libjxlVersion = "libjxl 0.12.0 (lcms2 wasm)";
      let libjxlAvailable = true;
      try {
        const mod = await loadLibjxl();
        libjxlVersion = mod.UTF8ToString(mod._libjxl_version_str());
      } catch {
        libjxlAvailable = false;
      }
      self.postMessage({
        id,
        value: [
          ...rustList,
          {
            id: "libjxl",
            label: "libjxl",
            version: libjxlVersion,
            engine: "C++ Emscripten WASM (lcms2)",
            supportsLevel5Probe: false,
            available: libjxlAvailable,
          },
        ],
      });
    } else if (method === "checkDecoders") {
      const bytes = data.bytes instanceof Uint8Array ? data.bytes : new Uint8Array(data.bytes);
      const requested = Array.isArray(data.decoders) && data.decoders.length
        ? data.decoders
        : ["zenjxl-decoder", "jxl-rs", "jxl-oxide", "libjxl"];
      const optionsJson = JSON.stringify(data.options ?? {});
      const returnPixels = data.returnPixels !== false;
      const jxlSha256 = await sha256Hex(bytes);

      const needsRust = requested.some(d => d !== "libjxl");
      const decMod = needsRust ? await loadDecoders(optimized) : null;

      const results = [];
      const pixelMap = new Map();

      for (const decId of requested) {
        if (decId === "libjxl") {
          const { report, pixels } = await decodeWithLibjxl(bytes);
          if (report.ok && pixels.byteLength > 0) {
            report.pixelSha256 = await sha256Hex(pixels);
            pixelMap.set(decId, { width: report.width, height: report.height, pixels });
          } else {
            report.pixelSha256 = null;
          }
          results.push(report);
        } else {
          const t0 = performance.now();
          try {
            const out = decMod.decode_with_decoder(decId, bytes, optionsJson);
            const report = JSON.parse(out.json());
            const pixels = out.into_pixels();
            report.elapsedMs = Math.round(performance.now() - t0);
            if (report.ok && pixels.byteLength > 0) {
              report.pixelSha256 = await sha256Hex(pixels);
              pixelMap.set(decId, { width: report.width, height: report.height, pixels });
            } else {
              report.pixelSha256 = null;
            }
            results.push(report);
          } catch (error) {
            results.push({
              decoder: decId,
              version: decId,
              ok: false,
              error: error instanceof Error ? error.message : String(error),
              width: 0,
              height: 0,
              frameCount: 0,
              frameDurationsMs: [],
              loopCount: null,
              hasAlpha: false,
              hasIcc: false,
              isCmyk: false,
              usesOriginalProfile: false,
              extraChannels: 0,
              orientation: 1,
              cmsUsed: "none",
              level5Accepted: null,
              warnings: [],
              elapsedMs: Math.round(performance.now() - t0),
              pixelSha256: null,
            });
          }
        }
      }

      const okResults = results.filter(r => r.ok && pixelMap.has(r.decoder));
      const pairwise = [];
      for (let i = 0; i < okResults.length; i++) {
        for (let j = i + 1; j < okResults.length; j++) {
          const a = okResults[i];
          const b = okResults[j];
          pairwise.push(compareDecoderBuffers(
            a.decoder, a, pixelMap.get(a.decoder).pixels,
            b.decoder, b, pixelMap.get(b.decoder).pixels,
          ));
        }
      }
      if (data.referencePixels && data.referenceWidth && data.referenceHeight) {
        const refPx = data.referencePixels instanceof Uint8Array
          ? data.referencePixels : new Uint8Array(data.referencePixels);
        const refMeta = { width: data.referenceWidth, height: data.referenceHeight };
        for (const r of okResults) {
          pairwise.push(compareDecoderBuffers(
            "editor-canvas", refMeta, refPx,
            r.decoder, r, pixelMap.get(r.decoder).pixels,
          ));
        }
      }

      const pixelsByDecoder = {};
      const transfers = [];
      if (returnPixels) {
        for (const [k, v] of pixelMap.entries()) {
          pixelsByDecoder[k] = v;
          transfers.push(v.pixels.buffer);
        }
      }
      self.postMessage({
        id,
        value: {
          jxlSha256,
          jxlByteLength: bytes.byteLength,
          results,
          pairwise,
          pixelsByDecoder,
        },
      }, transfers);
    } else if (method === "decode") {
      const raster = codec.decode_jxl(data.bytes);
      const width = raster.width();
      const height = raster.height();
      const pixels = raster.into_pixels();
      self.postMessage({ id, value: { pixels, width, height } }, [pixels.buffer]);
    } else {
      throw new Error("Unknown codec request.");
    }
  } catch (error) {
    self.postMessage({ id, error: error instanceof Error ? error.message : String(error) });
  }
};
