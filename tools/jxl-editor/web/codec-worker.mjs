// A function containing i8x16.relaxed_swizzle. Validation probes the opcode
// before loading a larger module compiled with relaxed SIMD enabled.
const relaxedProbe = Uint8Array.of(0, 97, 115, 109, 1, 0, 0, 0,
  1, 5, 1, 96, 1, 123, 0, 3, 2, 1, 0,
  10, 12, 1, 10, 0, 32, 0, 32, 0, 253, 128, 2, 26, 11);

let ready;
let filters;
let native;
let zensr;
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

async function sha256Hex(bytes) {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest), b => b.toString(16).padStart(2, "0")).join("");
}

async function fetchModelWeights(entry) {
  if (zensrWeightsCache.has(entry.id)) return zensrWeightsCache.get(entry.id);
  const res = await fetch(entry.weightsUrl);
  if (!res.ok) throw new Error(`Failed to fetch ZenSR weights (${res.status}): ${entry.weightsUrl}`);
  const bytes = new Uint8Array(await res.arrayBuffer());
  if (entry.weightsSha256 && crypto?.subtle) {
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
