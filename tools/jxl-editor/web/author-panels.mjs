import { codec } from "./codec.mjs";
import { MAX_TIMELINE_FRAMES, MAX_SPLINES } from "./project.mjs";

const $ = id => document.getElementById(id);
const LAYER_PROP_KEYS = ["visible", "opacity", "x", "y", "scale", "angle", "exposure", "contrast", "saturation"];

export function defaultCoding() {
  return {
    mode: "modular",
    effort: 3,
    distance: 1.0,
    containerMode: "auto",
    fasterDecoding: 0,
    keepInvisible: true,
    autoDeltaFrames: false,
    lossyPalette: false,
    paletteColors: null,
    squeeze: false,
    forceRct: "auto",
    groupSizeShift: null,
    predictor: null,
    patches: null,
    progressive: "single",
    progressiveDc: 0,
    centerFirst: false,
    resampling: 1,
    gaborish: null,
    epfLevel: -1,
    noiseMode: "off",
    photonNoiseIso: 1600,
    manualNoiseLut: [0.02, 0.025, 0.025, 0.02, 0.015, 0.01, 0.008, 0.004],
    denoise: false,
    autoSplines: false,
    alphaDistance: 0,
    alphaSqueeze: false,
    strategy: "zenjxl",
  };
}

export function isDefaultModularCoding(coding) {
  if (!coding) return true;
  const d = defaultCoding();
  return coding.mode === "modular" &&
    (coding.effort ?? 3) === 3 &&
    (coding.containerMode ?? "auto") === "auto" &&
    (coding.fasterDecoding ?? 0) === 0 &&
    (coding.keepInvisible ?? true) === true &&
    !coding.lossyPalette &&
    (coding.paletteColors ?? null) === null &&
    !coding.squeeze &&
    (coding.forceRct ?? "auto") === "auto" &&
    (coding.groupSizeShift ?? null) === null &&
    (coding.predictor ?? null) === null &&
    (coding.patches ?? null) === null &&
    !coding.autoDeltaFrames &&
    d.mode === "modular";
}

export function snapshotLayerOverrides(layers) {
  const map = {};
  for (const layer of layers) {
    const props = {};
    for (const key of LAYER_PROP_KEYS) props[key] = layer[key];
    map[layer.id] = props;
  }
  return map;
}

export function resolveLayersForFrame(manifest, frameIndex) {
  const tl = manifest.timeline;
  if (!tl?.enabled || !tl.frames?.length) return manifest.layers;
  const frame = tl.frames[Math.max(0, Math.min(frameIndex, tl.frames.length - 1))];
  const overrides = frame?.layerOverrides;
  if (!overrides) return manifest.layers;
  return manifest.layers.map(layer => overrides[layer.id] ? { ...layer, ...overrides[layer.id] } : layer);
}

export function createDefaultTimeline(manifest, newId) {
  return {
    enabled: false,
    tpsNumerator: 1000,
    tpsDenominator: 1,
    loops: 0,
    frames: [
      {
        id: newId(),
        name: "Frame 1",
        duration: 250,
        timecode: null,
        blendMode: "auto",
        blendSource: null,
        saveAsReference: null,
        referenceOnly: false,
        layerOverrides: snapshotLayerOverrides(manifest.layers),
      },
    ],
  };
}

export function applyCmykOffsets(rawSamples, bitDepth, offsets) {
  if (!offsets || offsets.every(v => v === 0)) return rawSamples;
  const out = new Uint8Array(rawSamples.length);
  if (bitDepth === 8) {
    for (let i = 0; i < rawSamples.length; i += 4) {
      for (let c = 0; c < 4; c++) {
        // JXL ink convention: 0 = full ink, 255 = no ink.
        // Positive user offset adds ink -> decreases sample code.
        const v = Math.round(rawSamples[i + c] - offsets[c] * 255);
        out[i + c] = Math.max(0, Math.min(255, v));
      }
    }
    return out;
  }
  const srcView = new DataView(rawSamples.buffer, rawSamples.byteOffset, rawSamples.byteLength);
  const dstView = new DataView(out.buffer);
  for (let i = 0; i < rawSamples.length; i += 8) {
    for (let c = 0; c < 4; c++) {
      const orig = srcView.getUint16(i + c * 2, true);
      const v = Math.round(orig - offsets[c] * 65535);
      dstView.setUint16(i + c * 2, Math.max(0, Math.min(65535, v)), true);
    }
  }
  return out;
}

export function parseHexRgb(hex) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex?.trim() ?? "");
  if (!m) return [245, 220, 60];
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function parseControlPointsText(text) {
  const lines = text.split(/[\n;]+/).map(s => s.trim()).filter(Boolean);
  if (!lines.length) throw new Error("Enter at least one x,y control point.");
  if (lines.length > 256) throw new Error("At most 256 control points per spline.");
  const pts = [];
  let prev = null;
  for (const line of lines) {
    const parts = line.split(/[,\s]+/).filter(Boolean);
    if (parts.length !== 2) throw new Error(`Invalid control point "${line}". Use x,y.`);
    const x = Number(parts[0]);
    const y = Number(parts[1]);
    if (!Number.isFinite(x) || !Number.isFinite(y) || Math.abs(x) > 32768 || Math.abs(y) > 32768) {
      throw new Error(`Control point "${line}" is out of range.`);
    }
    const rx = Math.round(x);
    const ry = Math.round(y);
    if (prev && prev[0] === rx && prev[1] === ry) {
      throw new Error("Consecutive control points must differ after integer rounding.");
    }
    prev = [rx, ry];
    pts.push([x, y]);
  }
  return pts;
}

export function formatControlPointsText(points) {
  return points.map(([x, y]) => `${Math.round(x * 10) / 10}, ${Math.round(y * 10) / 10}`).join("\n");
}

export function buildCodingPayload(manifest) {
  const c = { ...defaultCoding(), ...(manifest.coding ?? {}) };
  const payload = {
    mode: c.mode,
    effort: c.effort,
    containerMode: c.containerMode,
    fasterDecoding: c.fasterDecoding,
    keepInvisible: c.keepInvisible,
    autoDeltaFrames: Boolean(c.autoDeltaFrames),
  };
  if (c.mode === "modular") {
    payload.lossyPalette = Boolean(c.lossyPalette);
    if (c.paletteColors !== null && c.paletteColors !== undefined) payload.paletteColors = c.paletteColors;
    payload.squeeze = Boolean(c.squeeze);
    if (c.forceRct && c.forceRct !== "auto") payload.forceRct = c.forceRct;
    if (c.groupSizeShift !== null && c.groupSizeShift !== undefined) payload.groupSizeShift = c.groupSizeShift;
    if (c.predictor !== null && c.predictor !== undefined) payload.predictor = c.predictor;
    if (c.patches !== null && c.patches !== undefined) payload.patches = c.patches;
  } else {
    payload.distance = c.distance;
    payload.progressive = c.progressive;
    payload.progressiveDc = c.progressiveDc;
    payload.centerFirst = Boolean(c.centerFirst);
    payload.resampling = c.resampling;
    if (c.patches !== null && c.patches !== undefined) payload.patches = c.patches;
    if (c.gaborish !== null && c.gaborish !== undefined) payload.gaborish = c.gaborish;
    payload.epfLevel = c.epfLevel;
    payload.noiseMode = c.noiseMode;
    if (c.noiseMode === "photon_iso") payload.photonNoiseIso = c.photonNoiseIso;
    if (c.noiseMode === "manual_lut") payload.manualNoiseLut = c.manualNoiseLut;
    payload.denoise = Boolean(c.denoise);
    payload.autoSplines = Boolean(c.autoSplines);
    if (c.alphaDistance > 0) {
      payload.alphaDistance = c.alphaDistance;
      payload.alphaSqueeze = Boolean(c.alphaSqueeze);
    }
    payload.strategy = c.strategy;
    if (!manifest.timeline?.enabled && manifest.splines?.length) {
      payload.splines = manifest.splines.map(s => ({
        controlPoints: s.controlPoints,
        colorDct: s.colorDct,
        sigmaDct: s.sigmaDct,
      }));
      payload.bakeSplines = manifest.splines.every(s => s.bakeIntoSource !== false);
    }
  }
  return payload;
}

export function splineCompatibilityError(manifest) {
  if (manifest.cmyk?.enabled) return "Manual splines are unavailable in CMYK output mode.";
  if (manifest.timeline?.enabled) return "Manual splines are not supported in multi-frame animations.";
  const coding = manifest.coding ?? defaultCoding();
  if (coding.mode !== "vardct") return "Manual JPEG XL splines require VarDCT lossy coding.";
  return null;
}

export function makeAuthorPanels({
  getState, getBusy, getFrameIndex, setFrameIndex, getSelectedSpline, setSelectedSpline,
  getSplinePickMode, setSplinePickMode, getTimelineOpen, setTimelineOpen,
  newId, storeState, run, report, status, stopPlayback, togglePlayback, isPlaying,
  importFrameFile, enableCmykFromCanvas, refreshCmykPreview, sampleCanvasRgb, renderFrameThumb,
}) {
  function mutateManifest(mutator, label, allowBusy = false) {
    const state = getState();
    if (!state || (!allowBusy && getBusy())) return;
    const nextManifest = structuredClone(state.manifest);
    const changed = mutator(nextManifest, state);
    if (changed === false) return;
    storeState({ ...state, manifest: nextManifest }, label);
  }

  function ensureTimeline(manifest) {
    if (!manifest.timeline) manifest.timeline = createDefaultTimeline(manifest, newId);
    return manifest.timeline;
  }

  function ensureCoding(manifest) {
    if (!manifest.coding) manifest.coding = defaultCoding();
    return manifest.coding;
  }

  // ── Timeline controls ──
  $("timeline-toggle").onclick = () => {
    if (!getState()) return;
    setTimelineOpen(!getTimelineOpen());
  };
  $("timeline-enabled").onchange = event => {
    stopPlayback();
    mutateManifest(m => {
      const tl = ensureTimeline(m);
      tl.enabled = event.target.checked;
      if (tl.enabled && tl.frames.length === 1) {
        // Keep single frame valid, user can add more immediately
        tl.frames[0].layerOverrides = snapshotLayerOverrides(m.layers);
      }
      if (tl.enabled && m.cmyk?.enabled) {
        m.cmyk.enabled = false;
      }
    }, event.target.checked ? "Enabled native timeline" : "Disabled native timeline");
  };
  $("timeline-play").onclick = togglePlayback;
  $("timeline-prev").onclick = () => {
    stopPlayback();
    const tl = getState()?.manifest.timeline;
    if (!tl?.frames.length) return;
    setFrameIndex((getFrameIndex() - 1 + tl.frames.length) % tl.frames.length);
  };
  $("timeline-next").onclick = () => {
    stopPlayback();
    const tl = getState()?.manifest.timeline;
    if (!tl?.frames.length) return;
    setFrameIndex((getFrameIndex() + 1) % tl.frames.length);
  };
  $("timeline-tps-num").onchange = event => {
    const v = Math.round(event.target.valueAsNumber);
    if (!Number.isInteger(v) || v < 1 || v > (1 << 30)) return;
    mutateManifest(m => {
      const tl = ensureTimeline(m);
      if (tl.tpsNumerator === v) return false;
      tl.tpsNumerator = v;
    }, "Changed animation TPS");
  };
  $("timeline-tps-den").onchange = event => {
    const v = Math.round(event.target.valueAsNumber);
    if (!Number.isInteger(v) || v < 1 || v > 1024) return;
    mutateManifest(m => {
      const tl = ensureTimeline(m);
      if (tl.tpsDenominator === v) return false;
      tl.tpsDenominator = v;
    }, "Changed animation TPS denominator");
  };
  $("timeline-loops").onchange = event => {
    const v = Math.round(event.target.valueAsNumber);
    if (!Number.isInteger(v) || v < 0 || v > 65535) return;
    mutateManifest(m => {
      const tl = ensureTimeline(m);
      if (tl.loops === v) return false;
      tl.loops = v;
    }, "Changed animation loop count");
  };
  $("timeline-auto-delta").onchange = event => {
    mutateManifest(m => { ensureCoding(m).autoDeltaFrames = event.target.checked; }, "Toggled auto delta frames");
  };
  $("timeline-add").onclick = () => {
    stopPlayback();
    const state = getState();
    if (!state || getBusy()) return;
    const currentTl = state.manifest.timeline;
    if (currentTl && currentTl.frames.length >= MAX_TIMELINE_FRAMES) {
      report(new Error(`Timeline supports up to ${MAX_TIMELINE_FRAMES} frames.`));
      return;
    }
    let nextIdx = 0;
    mutateManifest(m => {
      const tl = ensureTimeline(m);
      tl.enabled = true;
      if (m.cmyk?.enabled) m.cmyk.enabled = false;
      const srcIdx = Math.min(getFrameIndex(), tl.frames.length - 1);
      const base = tl.frames[srcIdx];
      const frame = {
        id: newId(),
        name: `Frame ${tl.frames.length + 1}`,
        duration: base?.duration || 250,
        timecode: null,
        blendMode: "auto",
        blendSource: null,
        saveAsReference: null,
        referenceOnly: false,
        layerOverrides: structuredClone(base?.layerOverrides ?? snapshotLayerOverrides(m.layers)),
      };
      tl.frames.push(frame);
      nextIdx = tl.frames.length - 1;
    }, "Added timeline frame");
    setFrameIndex(nextIdx);
    status("Timeline frame added");
  };
  $("timeline-dup").onclick = () => {
    stopPlayback();
    const state = getState();
    if (!state || getBusy()) return;
    const currentTl = state.manifest.timeline;
    if (currentTl && currentTl.frames.length >= MAX_TIMELINE_FRAMES) {
      report(new Error(`Timeline supports up to ${MAX_TIMELINE_FRAMES} frames.`));
      return;
    }
    let nextIdx = 0;
    mutateManifest(m => {
      const tl = ensureTimeline(m);
      tl.enabled = true;
      if (m.cmyk?.enabled) m.cmyk.enabled = false;
      const srcIdx = Math.min(getFrameIndex(), tl.frames.length - 1);
      const copy = structuredClone(tl.frames[srcIdx]);
      copy.id = newId();
      copy.name = `${copy.name} copy`.slice(0, 1019);
      copy.timecode = null;
      tl.frames.splice(srcIdx + 1, 0, copy);
      nextIdx = srcIdx + 1;
    }, "Duplicated timeline frame");
    setFrameIndex(nextIdx);
  };
  $("timeline-import").onclick = () => $("timeline-frame-file").click();
  $("timeline-frame-file").onchange = event => {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (file) run("Adding image frame to timeline…", () => importFrameFile(file));
  };
  function reorderFrame(delta) {
    stopPlayback();
    const state = getState();
    const tl = state?.manifest.timeline;
    if (!tl || tl.frames.length < 2) return;
    const from = getFrameIndex();
    const to = from + delta;
    if (to < 0 || to >= tl.frames.length) return;
    mutateManifest(m => {
      const [moved] = m.timeline.frames.splice(from, 1);
      m.timeline.frames.splice(to, 0, moved);
    }, "Reordered timeline frames");
    setFrameIndex(to);
  }
  $("timeline-move-left").onclick = () => reorderFrame(-1);
  $("timeline-move-right").onclick = () => reorderFrame(1);
  $("timeline-delete").onclick = () => {
    stopPlayback();
    const state = getState();
    const tl = state?.manifest.timeline;
    if (!tl || tl.frames.length < 2) return;
    const idx = getFrameIndex();
    mutateManifest(m => {
      m.timeline.frames.splice(idx, 1);
    }, "Deleted timeline frame");
    setFrameIndex(Math.min(idx, tl.frames.length - 2));
  };
  function updateCurrentFrame(patch, label) {
    mutateManifest(m => {
      const tl = ensureTimeline(m);
      const frame = tl.frames[Math.min(getFrameIndex(), tl.frames.length - 1)];
      if (!frame || Object.entries(patch).every(([k, v]) => frame[k] === v)) return false;
      Object.assign(frame, patch);
    }, label);
  }
  $("frame-name").onchange = event => updateCurrentFrame({ name: event.target.value.slice(0, 1019) }, "Renamed frame");
  $("frame-duration").onchange = event => {
    const v = Math.round(event.target.valueAsNumber);
    if (!Number.isInteger(v) || v < 0 || v > 0x7fffffff) return;
    updateCurrentFrame({ duration: v }, "Changed frame duration");
  };
  $("frame-timecode").onchange = event => {
    const raw = event.target.value.trim();
    if (!raw) {
      updateCurrentFrame({ timecode: null }, "Cleared frame timecode");
      return;
    }
    const v = Number(raw);
    if (!Number.isInteger(v) || v < 0 || v > 0xffffffff) return;
    updateCurrentFrame({ timecode: v }, "Changed frame timecode");
  };
  $("frame-blend").onchange = event => updateCurrentFrame({ blendMode: event.target.value }, "Changed frame blend mode");
  $("frame-blend-source").onchange = event => {
    const v = event.target.value === "" ? null : Number(event.target.value);
    updateCurrentFrame({ blendSource: v }, "Changed frame blend source");
  };
  $("frame-save-ref").onchange = event => {
    const v = event.target.value === "" ? null : Number(event.target.value);
    updateCurrentFrame({ saveAsReference: v }, "Changed frame reference slot");
  };
  $("frame-ref-only").onchange = event => updateCurrentFrame({ referenceOnly: event.target.checked }, "Toggled reference-only frame");

  // ── Coding & CMYK controls ──
  $("coding-color-model").onchange = event => {
    const model = event.target.value;
    if (model === "cmyk") {
      run("Preparing CMYK samples and ICC profile…", () => enableCmykFromCanvas(8));
    } else {
      mutateManifest(m => {
        if (m.cmyk) m.cmyk.enabled = false;
      }, "Switched to RGBA output");
    }
  };
  $("cmyk-bits").onchange = event => {
    const bits = Number(event.target.value);
    run(`Converting CMYK buffer to ${bits}-bit…`, () => enableCmykFromCanvas(bits));
  };
  $("cmyk-mode").onchange = event => {
    mutateManifest(m => {
      if (m.cmyk) m.cmyk.mode = event.target.value;
    }, "Changed CMYK coding mode");
  };
  $("cmyk-distance").onchange = event => {
    const v = event.target.valueAsNumber;
    if (Number.isFinite(v) && v >= 0.01 && v <= 15) {
      mutateManifest(m => { if (m.cmyk) m.cmyk.distance = v; }, "Changed CMYK distance");
    }
  };
  $("cmyk-effort").onchange = event => {
    const v = Math.round(event.target.valueAsNumber);
    if (Number.isInteger(v) && v >= 1 && v <= 10) {
      mutateManifest(m => { if (m.cmyk) m.cmyk.effort = v; }, "Changed CMYK effort");
    }
  };
  $("cmyk-icc-load").onclick = () => $("cmyk-icc-file").click();
  $("cmyk-icc-file").onchange = event => {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    run("Loading CMYK ICC profile…", async () => {
      const bytes = new Uint8Array(await file.arrayBuffer());
      if (bytes.length < 128 || String.fromCharCode(...bytes.subarray(16, 20)) !== "CMYK") {
        throw new Error("The selected ICC file does not have a 'CMYK' color space header.");
      }
      const state = getState();
      const nextManifest = structuredClone(state.manifest);
      if (nextManifest.cmyk) nextManifest.cmyk.iccName = file.name.slice(0, 128);
      const nextCmyk = { ...state.cmykData, icc: bytes };
      storeState({ ...state, manifest: nextManifest, cmykData: nextCmyk }, "Attached CMYK ICC profile");
      await refreshCmykPreview();
      status(`Attached CMYK ICC (${bytes.length} bytes)`);
    });
  };
  $("cmyk-icc-builtin").onclick = () => run("Generating built-in subtractive CMYK ICC…", async () => {
    const icc = await codec("minimalCmykIcc");
    const state = getState();
    const nextManifest = structuredClone(state.manifest);
    if (nextManifest.cmyk) nextManifest.cmyk.iccName = "Simple Subtractive CMYK (built-in)";
    const nextCmyk = { ...state.cmykData, icc };
    storeState({ ...state, manifest: nextManifest, cmykData: nextCmyk }, "Applied built-in CMYK ICC");
    await refreshCmykPreview();
    status("Applied built-in subtractive CMYK ICC");
  });
  for (let c = 0; c < 4; c++) {
    const input = $(`cmyk-offset-${c}`);
    input.oninput = () => {
      $(`cmyk-val-${c}`).textContent = `${Math.round(input.valueAsNumber * 100)}%`;
    };
    input.onchange = () => {
      const v = input.valueAsNumber;
      mutateManifest(m => {
        if (!m.cmyk) return false;
        if (!m.cmyk.channelOffsets) m.cmyk.channelOffsets = [0, 0, 0, 0];
        m.cmyk.channelOffsets[c] = v;
      }, "Adjusted CMYK ink offset");
      refreshCmykPreview();
    };
  }
  $("cmyk-reset-offsets").onclick = () => {
    mutateManifest(m => {
      if (m.cmyk) m.cmyk.channelOffsets = [0, 0, 0, 0];
    }, "Reset CMYK ink offsets");
    refreshCmykPreview();
  };

  function setCoding(patch, label) {
    mutateManifest(m => {
      const c = ensureCoding(m);
      if (Object.entries(patch).every(([k, v]) => JSON.stringify(c[k]) === JSON.stringify(v))) return false;
      Object.assign(c, patch);
    }, label);
  }
  $("coding-mode").onchange = event => {
    const mode = event.target.value;
    setCoding({ mode, effort: mode === "vardct" ? 4 : 3 }, `Switched coding mode to ${mode}`);
  };
  $("coding-effort").onchange = event => {
    const v = Math.round(event.target.valueAsNumber);
    if (Number.isInteger(v) && v >= 1 && v <= 10) setCoding({ effort: v }, "Changed coding effort");
  };
  $("coding-faster-decoding").onchange = event => {
    const v = Math.round(event.target.valueAsNumber);
    if (Number.isInteger(v) && v >= 0 && v <= 4) setCoding({ fasterDecoding: v }, "Changed faster decoding");
  };
  $("coding-lossy-palette").onchange = event => setCoding({ lossyPalette: event.target.checked }, "Toggled lossy palette");
  $("coding-squeeze").onchange = event => setCoding({ squeeze: event.target.checked }, "Toggled Haar squeeze");
  $("coding-palette-colors").onchange = event => {
    const raw = event.target.value.trim();
    const v = raw === "" ? null : Math.round(Number(raw));
    if (v !== null && (!Number.isInteger(v) || v < 0 || v > 65536)) return;
    setCoding({ paletteColors: v }, "Changed palette cap");
  };
  $("coding-rct").onchange = event => setCoding({ forceRct: event.target.value }, "Changed RCT transform");
  $("coding-group-size").onchange = event => {
    const v = event.target.value === "" ? null : Number(event.target.value);
    setCoding({ groupSizeShift: v }, "Changed modular group size");
  };
  $("coding-predictor").onchange = event => {
    const v = event.target.value === "" ? null : Number(event.target.value);
    setCoding({ predictor: v }, "Changed modular predictor");
  };
  function parseTri(v) {
    return v === "on" ? true : v === "off" ? false : null;
  }
  $("coding-modular-patches").onchange = event => setCoding({ patches: parseTri(event.target.value) }, "Changed modular patches");
  $("coding-distance").oninput = event => {
    $("coding-distance-val").textContent = event.target.valueAsNumber.toFixed(2);
  };
  $("coding-distance").onchange = event => setCoding({ distance: event.target.valueAsNumber }, "Changed VarDCT distance");
  $("coding-progressive").onchange = event => setCoding({ progressive: event.target.value }, "Changed progressive AC mode");
  $("coding-progressive-dc").onchange = event => setCoding({ progressiveDc: Number(event.target.value) }, "Changed progressive DC LF frame");
  $("coding-noise-mode").onchange = event => setCoding({ noiseMode: event.target.value }, "Changed noise synthesis mode");
  $("coding-photon-iso").onchange = event => {
    const v = event.target.valueAsNumber;
    if (Number.isFinite(v) && v >= 1 && v <= 50000) setCoding({ photonNoiseIso: v }, "Changed photon noise ISO");
  };
  $("coding-manual-lut").onchange = event => {
    const vals = event.target.value.split(",").map(s => Number(s.trim()));
    if (vals.length === 8 && vals.every(v => Number.isFinite(v) && v >= 0 && v <= 1) && vals.some(v => v > 0)) {
      setCoding({ manualNoiseLut: vals }, "Changed manual noise LUT");
    } else {
      report(new Error("Manual noise LUT needs 8 numbers in 0..1 with at least one positive entry."));
    }
  };
  $("coding-denoise").onchange = event => setCoding({ denoise: event.target.checked }, "Toggled Wiener denoise");
  $("coding-vardct-patches").onchange = event => setCoding({ patches: parseTri(event.target.value) }, "Changed VarDCT patches");
  $("coding-gaborish").onchange = event => setCoding({ gaborish: parseTri(event.target.value) }, "Changed gaborish");
  $("coding-epf").onchange = event => setCoding({ epfLevel: Number(event.target.value) }, "Changed EPF level");
  $("coding-resampling").onchange = event => setCoding({ resampling: Number(event.target.value) }, "Changed spatial resampling");
  $("coding-alpha-distance").onchange = event => {
    const v = event.target.valueAsNumber;
    if (Number.isFinite(v) && v >= 0 && v <= 15) setCoding({ alphaDistance: v }, "Changed alpha distance");
  };
  $("coding-alpha-squeeze").onchange = event => setCoding({ alphaSqueeze: event.target.checked }, "Toggled alpha squeeze");
  $("coding-center-first").onchange = event => setCoding({ centerFirst: event.target.checked }, "Toggled center-first groups");
  $("coding-auto-splines").onchange = event => setCoding({ autoSplines: event.target.checked }, "Toggled auto splines");
  $("coding-strategy").onchange = event => setCoding({ strategy: event.target.value }, "Changed encoder strategy");
  $("coding-keep-invisible").onchange = event => setCoding({ keepInvisible: event.target.checked }, "Toggled keep invisible");
  $("coding-container").onchange = event => setCoding({ containerMode: event.target.value }, "Changed container wrapper");

  // ── Splines controls ──
  $("splines-enable-vardct").onclick = () => {
    stopPlayback();
    mutateManifest(m => {
      ensureCoding(m).mode = "vardct";
      if (m.timeline) m.timeline.enabled = false;
      if (m.cmyk) m.cmyk.enabled = false;
    }, "Switched to VarDCT still for splines");
    status("Switched to VarDCT still coding for manual splines");
  };

  async function recomputeSplineDct(hex, sigma, points) {
    const [fgR, fgG, fgB] = parseHexRgb(hex);
    const mid = points[Math.floor(points.length / 2)] ?? [0, 0];
    const [bgR, bgG, bgB] = sampleCanvasRgb(mid[0], mid[1]);
    const dct = await codec("splineColorDct", { fgR, fgG, fgB, bgR, bgG, bgB, sigma });
    const z31 = Array(31).fill(0);
    return {
      colorDct: [[dct[0], ...z31], [dct[1], ...z31], [dct[2], ...z31]],
      sigmaDct: [dct[3], ...z31],
    };
  }

  $("spline-add").onclick = () => run("Adding JPEG XL spline…", async () => {
    const state = getState();
    if (!state) return;
    if ((state.manifest.splines?.length ?? 0) >= MAX_SPLINES) {
      throw new Error(`At most ${MAX_SPLINES} manual splines are supported.`);
    }
    const { width, height } = state.manifest;
    const pts = [
      [Math.max(1, Math.round(width * 0.15)), Math.round(height * 0.55)],
      [Math.round(width * 0.5), Math.max(1, Math.round(height * 0.3))],
      [Math.min(width - 1, Math.round(width * 0.85)), Math.round(height * 0.55)],
    ];
    if (pts[0][0] === pts[1][0] && pts[0][1] === pts[1][1]) pts[1] = [pts[0][0] + 2, pts[0][1]];
    if (pts[1][0] === pts[2][0] && pts[1][1] === pts[2][1]) pts[2] = [pts[1][0] + 2, pts[1][1] + 2];
    const colorHex = "#f5dc3c";
    const sigma = 1.8;
    const { colorDct, sigmaDct } = await recomputeSplineDct(colorHex, sigma, pts);
    const id = newId();
    mutateManifest(m => {
      if (!m.splines) m.splines = [];
      m.splines.push({
        id,
        name: `Spline ${m.splines.length + 1}`,
        colorHex,
        sigma,
        controlPoints: pts,
        colorDct,
        sigmaDct,
        bakeIntoSource: $("spline-bake").checked,
      });
    }, "Added manual spline", true);
    setSelectedSpline(id);
    status("Manual JPEG XL spline added");
  });

  $("spline-delete").onclick = () => {
    const selId = getSelectedSpline();
    if (!selId) return;
    setSplinePickMode(false);
    mutateManifest(m => {
      if (!m.splines) return false;
      m.splines = m.splines.filter(s => s.id !== selId);
    }, "Deleted manual spline");
    const next = getState()?.manifest.splines?.at(-1)?.id ?? null;
    setSelectedSpline(next);
  };

  $("spline-pick-mode").onclick = () => {
    if (!getSelectedSpline()) return;
    setSplinePickMode(!getSplinePickMode());
  };

  $("spline-bake").onchange = event => {
    mutateManifest(m => {
      if (!m.splines) return false;
      for (const s of m.splines) s.bakeIntoSource = event.target.checked;
    }, "Toggled spline baking");
  };

  $("spline-name").onchange = event => {
    const selId = getSelectedSpline();
    if (!selId) return;
    mutateManifest(m => {
      const s = m.splines?.find(x => x.id === selId);
      if (s) s.name = event.target.value.trim() || s.name;
    }, "Renamed spline");
  };

  async function updateSplineStyle() {
    const selId = getSelectedSpline();
    const state = getState();
    const spline = state?.manifest.splines?.find(x => x.id === selId);
    if (!spline) return;
    const colorHex = $("spline-color").value;
    const sigma = $("spline-sigma").valueAsNumber;
    if (!Number.isFinite(sigma) || sigma < 0.5 || sigma > 16) return;
    const { colorDct, sigmaDct } = await recomputeSplineDct(colorHex, sigma, spline.controlPoints);
    mutateManifest(m => {
      const s = m.splines?.find(x => x.id === selId);
      if (!s) return false;
      s.colorHex = colorHex;
      s.sigma = sigma;
      s.colorDct = colorDct;
      s.sigmaDct = sigmaDct;
    }, "Updated spline stroke", true);
  }
  $("spline-color").onchange = () => run("Updating spline color…", updateSplineStyle);
  $("spline-sigma").onchange = () => run("Updating spline width…", updateSplineStyle);

  $("spline-points").onchange = event => {
    const selId = getSelectedSpline();
    if (!selId) return;
    try {
      const pts = parseControlPointsText(event.target.value);
      $("spline-error").textContent = "";
      mutateManifest(m => {
        const s = m.splines?.find(x => x.id === selId);
        if (s) s.controlPoints = pts;
      }, "Edited spline control points");
    } catch (error) {
      $("spline-error").textContent = error.message;
    }
  };

  for (const [id, kind] of [
    ["spline-dct-x0", 0], ["spline-dct-y0", 1], ["spline-dct-b0", 2], ["spline-dct-s0", 3],
  ]) {
    $(id).onchange = event => {
      const selId = getSelectedSpline();
      const v = event.target.valueAsNumber;
      if (!selId || !Number.isFinite(v)) return;
      mutateManifest(m => {
        const s = m.splines?.find(x => x.id === selId);
        if (!s) return false;
        if (kind < 3) s.colorDct[kind][0] = v;
        else s.sigmaDct[0] = v;
      }, "Edited spline DCT coefficient");
    };
  }

  function updateUI() {
    const state = getState();
    const busy = getBusy();
    const hasDoc = Boolean(state);
    const tlOpen = hasDoc && getTimelineOpen();
    $("timeline-toggle").disabled = !hasDoc || busy;
    $("timeline-toggle").setAttribute("aria-pressed", String(tlOpen));
    $("timeline-dock").hidden = !tlOpen;
    $("coding-tab").disabled = !hasDoc;
    $("splines-tab").disabled = !hasDoc;
    $("coding-fields").disabled = !hasDoc || busy;
    if (!state) return;

    const { manifest } = state;
    const coding = { ...defaultCoding(), ...(manifest.coding ?? {}) };

    // ── Timeline dock refresh ──
    const tl = manifest.timeline ?? createDefaultTimeline(manifest, newId);
    const fIdx = Math.max(0, Math.min(getFrameIndex(), tl.frames.length - 1));
    const curFrame = tl.frames[fIdx];
    $("timeline-enabled").checked = Boolean(tl.enabled);
    $("timeline-enabled").disabled = busy;
    $("timeline-play").disabled = busy || tl.frames.length < 2;
    $("timeline-play").textContent = isPlaying() ? "Pause" : "Play";
    $("timeline-prev").disabled = busy || tl.frames.length < 2;
    $("timeline-next").disabled = busy || tl.frames.length < 2;
    $("timeline-counter").textContent = `Frame ${fIdx + 1} / ${tl.frames.length}`;
    $("timeline-tps-num").value = tl.tpsNumerator;
    $("timeline-tps-den").value = tl.tpsDenominator;
    $("timeline-loops").value = tl.loops;
    $("timeline-auto-delta").checked = Boolean(coding.autoDeltaFrames);
    $("timeline-move-left").disabled = busy || fIdx === 0;
    $("timeline-move-right").disabled = busy || fIdx >= tl.frames.length - 1;
    $("timeline-delete").disabled = busy || tl.frames.length < 2;
    for (const id of ["timeline-add", "timeline-dup", "timeline-import"]) $(id).disabled = busy;

    if (tlOpen) {
      const strip = $("timeline-frames");
      strip.replaceChildren();
      tl.frames.forEach((frame, i) => {
        const li = document.createElement("li");
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "timeline-frame";
        btn.disabled = busy;
        if (i === fIdx) btn.setAttribute("aria-current", "true");
        const thumb = renderFrameThumb(state, i);
        const info = document.createElement("span");
        info.textContent = frame.name || `Frame ${i + 1}`;
        const sub = document.createElement("small");
        const ms = Math.round(frame.duration * 1000 * tl.tpsDenominator / tl.tpsNumerator);
        sub.textContent = frame.referenceOnly ? "Ref only" : `${frame.duration}t (${ms} ms)`;
        info.append(sub);
        btn.append(thumb, info);
        btn.onclick = () => { stopPlayback(); setFrameIndex(i); };
        li.append(btn);
        strip.append(li);
      });
      if (curFrame) {
        $("frame-name").value = curFrame.name ?? "";
        $("frame-duration").value = curFrame.duration ?? 0;
        const ms = curFrame.duration * 1000 * tl.tpsDenominator / tl.tpsNumerator;
        $("frame-duration-ms").textContent = curFrame.referenceOnly
          ? "Reference-only (not displayed)"
          : `= ${ms.toFixed(1)} ms${ms > 0 && ms < 8.34 ? " (Level 10)" : ""}`;
        $("frame-timecode").value = curFrame.timecode ?? "";
        $("frame-blend").value = curFrame.blendMode ?? "auto";
        $("frame-blend-source").value = curFrame.blendSource ?? "";
        $("frame-save-ref").value = curFrame.saveAsReference ?? "";
        $("frame-ref-only").checked = Boolean(curFrame.referenceOnly);
      }
    }

    // ── Coding & CMYK panel refresh ──
    const cmykOn = Boolean(manifest.cmyk?.enabled);
    $("coding-color-model").value = cmykOn ? "cmyk" : "rgba";
    $("cmyk-controls").hidden = !cmykOn;
    $("rgba-coding-controls").hidden = cmykOn;
    if (cmykOn) {
      const cm = manifest.cmyk;
      $("cmyk-bits").value = String(cm.bitDepth);
      $("cmyk-mode").value = cm.mode;
      $("cmyk-vardct-opts").hidden = cm.mode !== "vardct";
      $("cmyk-distance").value = cm.distance ?? 1.0;
      $("cmyk-effort").value = cm.effort ?? (cm.mode === "vardct" ? 4 : 3);
      $("cmyk-semantics-note").textContent = cm.mode === "modular"
        ? `Lossless ${cm.bitDepth}-bit CMYK (Level 10): exact C, M, Y, K codes (K in Black extra channel 0) and embedded CMYK ICC survive byte for byte.`
        : `Lossy ${cm.bitDepth}-bit CMYK (Level 10): converts CMY/K via the embedded ICC to linear sRGB XYB while retaining K in Black extra channel 0 and embedding the CMYK ICC.`;
      const iccLen = state.cmykData?.icc?.byteLength ?? 0;
      $("cmyk-icc-status").textContent = `ICC profile: ${cm.iccName || "CMYK ICC"} (${iccLen} bytes, origin: ${cm.origin === "source" ? "exact JXL source" : "separated from RGBA"})`;
      const offsets = cm.channelOffsets ?? [0, 0, 0, 0];
      for (let c = 0; c < 4; c++) {
        $(`cmyk-offset-${c}`).value = offsets[c];
        $(`cmyk-val-${c}`).textContent = `${Math.round(offsets[c] * 100)}%`;
      }
      if (state.cmykData?.samples?.length) {
        const eff = applyCmykOffsets(state.cmykData.samples, cm.bitDepth, offsets);
        if (cm.bitDepth === 8) {
          $("cmyk-sample-info").textContent = `Pixel (0,0) JXL ink codes (0=full, 255=none): C=${eff[0]} M=${eff[1]} Y=${eff[2]} K=${eff[3]}`;
        } else {
          const dv = new DataView(eff.buffer, eff.byteOffset, eff.byteLength);
          $("cmyk-sample-info").textContent = `Pixel (0,0) 16-bit JXL ink codes (0=full, 65535=none): C=${dv.getUint16(0, true)} M=${dv.getUint16(2, true)} Y=${dv.getUint16(4, true)} K=${dv.getUint16(6, true)}`;
        }
      }
    } else {
      $("coding-mode").value = coding.mode;
      $("coding-effort").value = coding.effort;
      $("coding-faster-decoding").value = coding.fasterDecoding;
      $("coding-modular-group").hidden = coding.mode !== "modular";
      $("coding-vardct-group").hidden = coding.mode !== "vardct";
      $("coding-lossy-palette").checked = Boolean(coding.lossyPalette);
      $("coding-squeeze").checked = Boolean(coding.squeeze);
      $("coding-palette-colors").value = coding.paletteColors ?? "";
      $("coding-rct").value = coding.forceRct ?? "auto";
      $("coding-group-size").value = coding.groupSizeShift ?? "";
      $("coding-predictor").value = coding.predictor ?? "";
      const triStr = v => v === true ? "on" : v === false ? "off" : "auto";
      $("coding-modular-patches").value = triStr(coding.patches);
      $("coding-distance").value = coding.distance;
      $("coding-distance-val").textContent = Number(coding.distance).toFixed(2);
      $("coding-progressive").value = coding.progressive;
      $("coding-progressive-dc").value = String(coding.progressiveDc ?? 0);
      $("coding-noise-mode").value = coding.noiseMode;
      $("coding-photon-iso-row").hidden = coding.noiseMode !== "photon_iso";
      $("coding-photon-iso").value = coding.photonNoiseIso;
      $("coding-manual-lut-row").hidden = coding.noiseMode !== "manual_lut";
      $("coding-manual-lut").value = (coding.manualNoiseLut ?? []).join(",");
      $("coding-denoise").checked = Boolean(coding.denoise);
      $("coding-vardct-patches").value = triStr(coding.patches);
      $("coding-gaborish").value = triStr(coding.gaborish);
      $("coding-epf").value = String(coding.epfLevel ?? -1);
      $("coding-resampling").value = String(coding.resampling ?? 1);
      $("coding-alpha-distance").value = coding.alphaDistance ?? 0;
      $("coding-alpha-squeeze").checked = Boolean(coding.alphaSqueeze);
      $("coding-center-first").checked = Boolean(coding.centerFirst);
      $("coding-auto-splines").checked = Boolean(coding.autoSplines);
      $("coding-strategy").value = coding.strategy ?? "zenjxl";
      $("coding-keep-invisible").checked = Boolean(coding.keepInvisible);
      $("coding-container").value = coding.containerMode ?? "auto";
    }

    // ── Splines panel refresh ──
    const compatErr = splineCompatibilityError(manifest);
    $("splines-compat").hidden = !compatErr;
    $("splines-compat-text").textContent = compatErr ?? "";
    const splines = manifest.splines ?? [];
    let selId = getSelectedSpline();
    if (selId && !splines.some(s => s.id === selId)) {
      selId = splines.at(-1)?.id ?? null;
      setSelectedSpline(selId);
    }
    $("spline-add").disabled = busy;
    $("spline-delete").disabled = busy || !selId;
    $("spline-pick-mode").disabled = busy || !selId;
    $("spline-pick-mode").setAttribute("aria-pressed", String(Boolean(selId && getSplinePickMode())));
    if (splines.length) $("spline-bake").checked = splines.every(s => s.bakeIntoSource !== false);

    const list = $("spline-list");
    list.replaceChildren();
    for (const s of splines) {
      const li = document.createElement("li");
      const btn = document.createElement("button");
      btn.type = "button";
      btn.disabled = busy;
      btn.setAttribute("aria-pressed", String(s.id === selId));
      const label = document.createElement("span");
      label.textContent = s.name;
      const meta = document.createElement("small");
      meta.textContent = `${s.controlPoints.length} pts · σ=${Number(s.sigma).toFixed(1)}`;
      btn.append(label, meta);
      btn.onclick = () => setSelectedSpline(s.id);
      li.append(btn);
      list.append(li);
    }
    const activeSpline = splines.find(s => s.id === selId);
    $("spline-editor").hidden = !activeSpline;
    if (activeSpline) {
      $("spline-name").value = activeSpline.name;
      $("spline-color").value = activeSpline.colorHex ?? "#f5dc3c";
      $("spline-sigma").value = activeSpline.sigma ?? 1.8;
      $("spline-points").value = formatControlPointsText(activeSpline.controlPoints);
      $("spline-dct-x0").value = Number(activeSpline.colorDct[0][0]).toFixed(4);
      $("spline-dct-y0").value = Number(activeSpline.colorDct[1][0]).toFixed(4);
      $("spline-dct-b0").value = Number(activeSpline.colorDct[2][0]).toFixed(4);
      $("spline-dct-s0").value = Number(activeSpline.sigmaDct[0]).toFixed(4);
    }
  }

  return { updateUI };
}
