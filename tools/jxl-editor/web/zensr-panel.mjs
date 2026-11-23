import { DEFAULT_ZENSR, getZensrElapsed } from "./filters.mjs";
import { makeLowQJpegFile } from "./gallery.mjs";

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
    const next = { ...currentZensr(layer), ...patch };
    storeState(patchState(state, layer.id, { zensr: next }), label);
  }

  $("zensr-enabled").addEventListener("change", e => {
    const on = e.target.checked;
    mutateZensr({ enabled: on }, on ? "Enabled ZenSR restoration" : "Disabled ZenSR restoration");
  });

  $("zensr-model").addEventListener("change", e => {
    const model = e.target.value;
    if (model === "dejpeg_rt24g") {
      mutateZensr({ model, arch: "compact", nf: 24, nc: 8, scale: 1 }, "Selected ZenSR dejpeg_rt24g");
    } else {
      mutateZensr({ model: "custom" }, "Selected custom ZenSR checkpoint");
    }
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
      $("zensr-telemetry").textContent = "Open a raster image to run ZenSR in-browser via WebAssembly.";
      return;
    }
    const z = currentZensr(layer);
    const asset = state.assets.get(layer.id);
    $("zensr-enabled").checked = Boolean(z.enabled);
    $("zensr-model").value = z.model === "custom" ? "custom" : "dejpeg_rt24g";
    $("zensr-strength").value = String(z.strength ?? 1);
    $("zensr-strength-val").value = `${Math.round((z.strength ?? 1) * 100)}%`;
    $("zensr-strength-row").hidden = z.scale > 1;
    $("zensr-custom-opts").hidden = z.model !== "custom";
    $("zensr-custom-arch").value = z.arch;
    $("zensr-custom-nf").value = String(z.nf ?? 24);
    $("zensr-custom-nc").value = String(z.nc ?? 8);
    $("zensr-custom-scale").value = String(z.scale ?? 1);
    $("zensr-weights-note").textContent = asset?.customZensrWeights
      ? `Loaded: ${asset.customZensrName} (${asset.customZensrWeights.byteLength.toLocaleString()} B)`
      : "Load any zensr-micro f16 or f32 .raw checkpoint (Compact 1×–4× or SPAN-48 2×/4×).";

    const elapsed = getZensrElapsed(asset);
    if (isSvg) {
      $("zensr-telemetry").textContent = "Vector SVG layers re-rasterize directly at target scale.";
    } else if (!z.enabled) {
      $("zensr-telemetry").textContent = "Original source bytes preserved non-destructively in .project.jxl. Enable ZenSR to run zensr-micro neural restoration in WebAssembly.";
    } else if (elapsed !== null) {
      const mp = (layer.width * layer.height) / 1e6;
      const mps = elapsed > 0 ? (mp / (elapsed / 1000)).toFixed(2) : "∞";
      $("zensr-telemetry").textContent = `ZenSR active (${z.model}, ${z.scale}×) · ${elapsed} ms (${mps} MP/s) · original source untouched`;
    } else {
      $("zensr-telemetry").textContent = `ZenSR running (${z.model}, ${z.scale}×)…`;
    }
  }

  return { updateUI };
}
