import { DEFAULT_ZENSR, effectiveLayerSize, getZensrElapsed } from "./filters.mjs";
import { makeLowQJpegFile } from "./gallery.mjs";
import { MAX_PIXELS, MAX_SIDE } from "./project.mjs";
import { appLog } from "./console-log.mjs";

const BUILTIN_MODELS = {
  dejpeg_rt24g: { arch: "compact", nf: 24, nc: 8, scale: 1, label: "ZenSR De-JPEG Realtime (1×)" },
  rtc_distill_2x: { arch: "compact", nf: 24, nc: 8, scale: 2, label: "ZenSR Realtime 2× Upscale" },
  nomosuni_span_2x: { arch: "span48", nf: 48, nc: 6, scale: 2, label: "ZenSR Quality 2× (SPAN-48)" },
  nomosuni_span_4x: { arch: "span48", nf: 48, nc: 6, scale: 4, label: "ZenSR Quality 4× (SPAN-48)" },
};

export function makeZensrPanel({
  getState,
  getLayer,
  getBusy,
  storeState,
  patchState,
  addLayerFiles,
  openFile,
  run,
  status,
}) {
  const $ = id => document.getElementById(id);

  function currentZensr(layer) {
    return { ...DEFAULT_ZENSR, ...(layer?.zensr ?? {}) };
  }

  function mutateZensr(patch, label) {
    const state = getState();
    const layer = getLayer();
    if (!state || !layer || getBusy()) return;
    if (layer.mime === "image/svg+xml") return;
    const prevEff = effectiveLayerSize(layer);
    const next = { ...currentZensr(layer), ...patch };
    const nextSr = next.enabled ? Math.max(1, next.scale ?? 1) : 1;
    const nextEffW = layer.width * nextSr;
    const nextEffH = layer.height * nextSr;
    if (nextEffW > MAX_SIDE || nextEffH > MAX_SIDE || nextEffW * nextEffH > MAX_PIXELS) {
      status(`Upscaled size ${nextEffW}×${nextEffH} exceeds the 16,384 px / 64 MP canvas limit.`, true);
      appLog.warn("zensr", `Blocked ${nextSr}× upscale on ${layer.width}×${layer.height}: exceeds 64 MP limit`);
      updateUI();
      return;
    }

    let nextState = patchState(state, layer.id, { zensr: next });
    // Automatically expand or restore the document canvas when it currently matches
    // the layer's effective pixel dimensions (standard single-image / matching canvas workflow).
    const canvasMatchedPrev = state.manifest.width === prevEff.width && state.manifest.height === prevEff.height;
    const autoExpand = $("zensr-auto-canvas")?.checked !== false;
    if (autoExpand && (canvasMatchedPrev || state.manifest.layers.length === 1) &&
        (nextEffW !== state.manifest.width || nextEffH !== state.manifest.height)) {
      const m = structuredClone(nextState.manifest);
      m.width = nextEffW;
      m.height = nextEffH;
      nextState = { ...nextState, manifest: m };
      appLog.info("zensr", `Resized canvas ${state.manifest.width}×${state.manifest.height} → ${nextEffW}×${nextEffH} for ${next.model} (${nextSr}×)`);
    }
    storeState(nextState, label);
  }

  function applyModelPreset(modelId, enable = true) {
    const spec = BUILTIN_MODELS[modelId];
    if (!spec) return;
    mutateZensr(
      { enabled: enable, model: modelId, arch: spec.arch, nf: spec.nf, nc: spec.nc, scale: spec.scale },
      enable ? `Enabled ${spec.label}` : "Disabled ZenSR",
    );
  }

  $("zensr-enabled").addEventListener("change", e => {
    const on = e.target.checked;
    mutateZensr({ enabled: on }, on ? "Enabled ZenSR" : "Disabled ZenSR");
  });

  $("zensr-model").addEventListener("change", e => {
    const model = e.target.value;
    const spec = BUILTIN_MODELS[model];
    if (spec) {
      const layer = getLayer();
      const wasEnabled = currentZensr(layer).enabled;
      mutateZensr(
        { enabled: wasEnabled || spec.scale > 1, model, arch: spec.arch, nf: spec.nf, nc: spec.nc, scale: spec.scale },
        `Selected ${spec.label}`,
      );
    } else {
      mutateZensr({ model: "custom" }, "Selected custom ZenSR checkpoint");
    }
  });

  $("zensr-quick-1x")?.addEventListener("click", () => applyModelPreset("dejpeg_rt24g", true));
  $("zensr-quick-2x-fast")?.addEventListener("click", () => applyModelPreset("rtc_distill_2x", true));
  $("zensr-quick-2x-hq")?.addEventListener("click", () => applyModelPreset("nomosuni_span_2x", true));
  $("zensr-quick-4x-hq")?.addEventListener("click", () => applyModelPreset("nomosuni_span_4x", true));
  $("zensr-quick-off")?.addEventListener("click", () => mutateZensr({ enabled: false }, "Disabled ZenSR"));

  $("zensr-fit-canvas")?.addEventListener("click", () => {
    const state = getState();
    const layer = getLayer();
    if (!state || !layer || getBusy()) return;
    const eff = effectiveLayerSize(layer);
    const w = Math.max(1, Math.round(eff.width * layer.scale));
    const h = Math.max(1, Math.round(eff.height * layer.scale));
    if (w > MAX_SIDE || h > MAX_SIDE || w * h > MAX_PIXELS) {
      status(`Target canvas ${w}×${h} exceeds 16,384 px / 64 MP limit.`, true);
      return;
    }
    const nextManifest = structuredClone(state.manifest);
    nextManifest.width = w;
    nextManifest.height = h;
    nextManifest.layers = nextManifest.layers.map(l => l.id === layer.id ? { ...l, x: 0, y: 0 } : l);
    storeState({ ...state, manifest: nextManifest }, `Matched canvas to ${w}×${h}`);
    status(`Canvas matched to ${w} × ${h} px`);
  });

  $("zensr-strength").addEventListener("input", e => {
    $("zensr-strength-val").value = `${Math.round(e.target.valueAsNumber * 100)}%`;
  });
  $("zensr-strength").addEventListener("change", e => {
    mutateZensr({ enabled: true, strength: e.target.valueAsNumber }, "Adjusted ZenSR strength");
  });

  $("zensr-custom-arch").addEventListener("change", e => {
    const arch = e.target.value;
    mutateZensr({ arch, scale: arch === "span48" ? 4 : Number($("zensr-custom-scale").value) }, "Changed custom ZenSR arch");
  });
  $("zensr-custom-nf").addEventListener("change", e => {
    const nf = Math.max(4, Math.min(64, Math.round(e.target.valueAsNumber / 4) * 4));
    mutateZensr({ nf }, "Changed custom ZenSR nf");
  });
  $("zensr-custom-nc").addEventListener("change", e => {
    const nc = Math.max(1, Math.min(32, Math.round(e.target.valueAsNumber)));
    mutateZensr({ nc }, "Changed custom ZenSR nc");
  });
  $("zensr-custom-scale").addEventListener("change", e => {
    mutateZensr({ scale: Number(e.target.value) }, "Changed custom ZenSR scale");
  });

  $("zensr-weights-btn").onclick = () => $("zensr-weights-file").click();
  $("zensr-weights-file").addEventListener("change", event => {
    const file = event.target.files[0];
    event.target.value = "";
    if (!file) return;
    run("Loading custom ZenSR weights…", async () => {
      const state = getState();
      const layer = getLayer();
      if (!state || !layer) return;
      const bytes = new Uint8Array(await file.arrayBuffer());
      const z = {
        ...currentZensr(layer),
        enabled: true,
        model: "custom",
        arch: $("zensr-custom-arch").value,
        nf: Number($("zensr-custom-nf").value) || 24,
        nc: Number($("zensr-custom-nc").value) || 8,
        scale: Number($("zensr-custom-scale").value) || 1,
      };
      const nextState = patchState(state, layer.id, { zensr: z });
      const assets = new Map(nextState.assets);
      assets.set(layer.id, { ...assets.get(layer.id), customZensrWeights: bytes, customZensrName: file.name });
      storeState({ ...nextState, assets }, `Loaded custom ZenSR weights (${file.name})`);
      status(`Loaded ${file.name} (${Math.round(bytes.byteLength / 1024)} KB)`);
    });
  });

  $("zensr-demo-jpeg").onclick = () => {
    if (getBusy()) return;
    run("Creating Low-Q JPEG test layer…", async () => {
      const file = await makeLowQJpegFile();
      if (getState()) await addLayerFiles([file]);
      else await openFile(file);
    });
  };

  function updateUI() {
    const state = getState();
    const layer = getLayer();
    const busy = getBusy();
    const isSvg = layer?.mime === "image/svg+xml";
    $("zensr-fields").disabled = !layer || isSvg || busy;
    $("zensr-svg-note").hidden = !isSvg;
    if (!layer) {
      $("zensr-telemetry").textContent = "Open a raster image to run ZenSR restoration or 2×/4× neural upscaling in WebAssembly.";
      return;
    }
    const z = currentZensr(layer);
    const asset = state.assets.get(layer.id);
    const eff = effectiveLayerSize(layer);
    $("zensr-enabled").checked = Boolean(z.enabled);
    $("zensr-model").value = BUILTIN_MODELS[z.model] ? z.model : "custom";
    $("zensr-strength").value = String(z.strength ?? 1);
    $("zensr-strength-val").value = `${Math.round((z.strength ?? 1) * 100)}%`;
    $("zensr-strength-row").hidden = z.scale > 1;
    $("zensr-custom-opts").hidden = Boolean(BUILTIN_MODELS[z.model]);
    $("zensr-custom-arch").value = z.arch;
    $("zensr-custom-nf").value = String(z.nf ?? 24);
    $("zensr-custom-nc").value = String(z.nc ?? 8);
    $("zensr-custom-scale").value = String(z.scale ?? 1);

    $("zensr-quick-1x")?.setAttribute("aria-pressed", String(Boolean(z.enabled && z.model === "dejpeg_rt24g")));
    $("zensr-quick-2x-fast")?.setAttribute("aria-pressed", String(Boolean(z.enabled && z.model === "rtc_distill_2x")));
    $("zensr-quick-2x-hq")?.setAttribute("aria-pressed", String(Boolean(z.enabled && z.model === "nomosuni_span_2x")));
    $("zensr-quick-4x-hq")?.setAttribute("aria-pressed", String(Boolean(z.enabled && z.model === "nomosuni_span_4x")));
    $("zensr-quick-off")?.setAttribute("aria-pressed", String(!z.enabled));

    const fitBtn = $("zensr-fit-canvas");
    if (fitBtn) {
      const targetW = Math.round(eff.width * layer.scale);
      const targetH = Math.round(eff.height * layer.scale);
      fitBtn.hidden = !z.enabled || (state.manifest.width === targetW && state.manifest.height === targetH);
      fitBtn.textContent = `Match canvas to ${targetW}×${targetH}`;
    }

    $("zensr-weights-note").textContent = asset?.customZensrWeights
      ? `Loaded: ${asset.customZensrName} (${asset.customZensrWeights.byteLength.toLocaleString()} B)`
      : "Load any zensr-micro f16 or f32 .raw checkpoint (Compact 1×–4× or SPAN-48 2×/4×).";

    const elapsed = getZensrElapsed(asset);
    if (isSvg) {
      $("zensr-telemetry").textContent = "Vector SVG layers re-rasterize crisply at any scale without neural upscaling.";
    } else if (!z.enabled) {
      $("zensr-telemetry").textContent = `Original ${layer.width}×${layer.height} source bytes preserved non-destructively in .project.jxl. Pick 1× Restore, 2× Upscale, or 4× Upscale above.`;
    } else if (elapsed !== null) {
      const mp = (eff.width * eff.height) / 1e6;
      const mps = elapsed > 0 ? (mp / (elapsed / 1000)).toFixed(2) : "∞";
      const dimNote = z.scale > 1 ? ` · ${layer.width}×${layer.height} → ${eff.width}×${eff.height}` : "";
      $("zensr-telemetry").textContent = `ZenSR active (${z.model}, ${z.scale}×${dimNote}) · ${elapsed} ms (${mps} MP/s) · original source untouched`;
    } else {
      $("zensr-telemetry").textContent = `ZenSR running (${z.model}, ${z.scale}× → ${eff.width}×${eff.height})…`;
    }
  }

  return { updateUI };
}
