import { codec } from "./codec.mjs";
import { DEFAULT_MASK } from "./filters.mjs";

export async function encodeMaskAlphaToJxl(maskAlpha, width, height) {
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    const v = maskAlpha[i];
    rgba[i * 4] = v;
    rgba[i * 4 + 1] = v;
    rgba[i * 4 + 2] = v;
    rgba[i * 4 + 3] = 255;
  }
  return codec("encode", { width, height, stride: width * 4, pixels: rgba }, [rgba.buffer]);
}

export async function decodeMaskJxlToAlpha(jxlBytes, expectedW, expectedH) {
  const decoded = await codec("decode", { bytes: jxlBytes.slice() });
  if (decoded.width !== expectedW || decoded.height !== expectedH) {
    throw new Error("Stored layer mask dimensions do not match its layer.");
  }
  const out = new Uint8Array(expectedW * expectedH);
  const px = decoded.pixels;
  for (let i = 0; i < out.length; i++) out[i] = px[i * 4];
  return out;
}

function stampBrush(alpha, width, height, cx, cy, radius, hardness, opacity, value) {
  const r = Math.max(1, radius);
  const x0 = Math.max(0, Math.floor(cx - r));
  const y0 = Math.max(0, Math.floor(cy - r));
  const x1 = Math.min(width - 1, Math.ceil(cx + r));
  const y1 = Math.min(height - 1, Math.ceil(cy + r));
  const inner = r * Math.min(0.98, Math.max(0, hardness));
  const span = Math.max(0.5, r - inner);
  for (let y = y0; y <= y1; y++) {
    const dy = y - cy;
    for (let x = x0; x <= x1; x++) {
      const dx = x - cx;
      const dist = Math.hypot(dx, dy);
      if (dist > r) continue;
      let falloff = 1;
      if (dist > inner) {
        const t = (dist - inner) / span;
        falloff = 1 - t * t * (3 - 2 * t);
      }
      const w = falloff * opacity;
      const idx = y * width + x;
      alpha[idx] = Math.round(alpha[idx] + (value - alpha[idx]) * w);
    }
  }
}

export function makeMasksPanel({
  getState,
  getLayer,
  getBusy,
  storeState,
  patchState,
  requestPaint,
  run,
  status,
  onBrushModeChange,
}) {
  const $ = id => document.getElementById(id);
  let rubyOverlay = false;
  let brushMode = null; // null | "reveal" | "erase"
  let brushSize = 24;
  let brushHardness = 0.6;
  let brushOpacity = 1.0;
  let stroke = null;

  function currentMask(layer) {
    return { ...DEFAULT_MASK, ...(layer?.mask ?? {}) };
  }

  function mutateMask(patch, label) {
    const state = getState();
    const layer = getLayer();
    if (!state || !layer || getBusy()) return;
    const next = { ...currentMask(layer), ...patch };
    const nextState = patchState(state, layer.id, { mask: next });
    storeState(nextState, label);
  }

  function commitRasterMask(newAlpha, label, enableRaster = true) {
    const state = getState();
    const layer = getLayer();
    if (!state || !layer || getBusy()) return;
    const prevMask = currentMask(layer);
    const nextMode = !enableRaster ? prevMask.mode
      : prevMask.shape && prevMask.shape !== "none" ? "combined" : "raster";
    const nextMask = {
      ...prevMask,
      enabled: true,
      mode: nextMode,
      hasRaster: Boolean(newAlpha),
    };
    const nextState = patchState(state, layer.id, { mask: nextMask });
    const assets = new Map(nextState.assets);
    const prevAsset = assets.get(layer.id);
    assets.set(layer.id, {
      ...prevAsset,
      maskAlpha: newAlpha,
      maskJxl: null,
      maskRev: (prevAsset.maskRev ?? 0) + 1,
    });
    storeState({ ...nextState, assets }, label);
  }

  function setBrushMode(mode) {
    brushMode = mode;
    $("mask-brush-reveal").setAttribute("aria-pressed", String(mode === "reveal"));
    $("mask-brush-erase").setAttribute("aria-pressed", String(mode === "erase"));
    $("mask-brush-stop").disabled = !mode;
    if (onBrushModeChange) onBrushModeChange(mode);
    requestPaint();
  }

  function toLayerCoords(layer, pt) {
    const theta = layer.angle * Math.PI / 180;
    const dx = pt.x - layer.x;
    const dy = pt.y - layer.y;
    const u = (Math.cos(theta) * dx + Math.sin(theta) * dy) / layer.scale;
    const v = (-Math.sin(theta) * dx + Math.cos(theta) * dy) / layer.scale;
    return { x: u, y: v };
  }

  function startStroke(layer, canvasPt) {
    if (!brushMode || !layer) return false;
    const state = getState();
    const asset = state.assets.get(layer.id);
    const working = asset.maskAlpha
      ? asset.maskAlpha.slice()
      : new Uint8Array(layer.width * layer.height).fill(255);
    const lp = toLayerCoords(layer, canvasPt);
    const val = brushMode === "reveal" ? 255 : 0;
    stampBrush(working, layer.width, layer.height, lp.x, lp.y, brushSize / 2, brushHardness, brushOpacity, val);

    const prevMask = currentMask(layer);
    const nextMode = prevMask.shape && prevMask.shape !== "none" ? "combined" : "raster";
    const liveMask = { ...prevMask, enabled: true, mode: nextMode, hasRaster: true };
    const liveState = patchState(state, layer.id, { mask: liveMask });
    const liveAssets = new Map(liveState.assets);
    liveAssets.set(layer.id, {
      ...asset,
      maskAlpha: working,
      maskJxl: null,
      maskRev: (asset.maskRev ?? 0) + 1,
    });
    stroke = {
      layerId: layer.id,
      working,
      last: lp,
      val,
      state: { ...liveState, assets: liveAssets },
    };
    return stroke.state;
  }

  function moveStroke(layer, canvasPt) {
    if (!stroke || !layer) return null;
    const lp = toLayerCoords(layer, canvasPt);
    const dist = Math.hypot(lp.x - stroke.last.x, lp.y - stroke.last.y);
    const steps = Math.max(1, Math.ceil(dist / Math.max(1, brushSize * 0.25)));
    for (let i = 1; i <= steps; i++) {
      const t = i / steps;
      const x = stroke.last.x + (lp.x - stroke.last.x) * t;
      const y = stroke.last.y + (lp.y - stroke.last.y) * t;
      stampBrush(stroke.working, layer.width, layer.height, x, y, brushSize / 2, brushHardness, brushOpacity, stroke.val);
    }
    stroke.last = lp;
    const asset = stroke.state.assets.get(layer.id);
    asset.maskRev = (asset.maskRev ?? 0) + 1;
    return stroke.state;
  }

  function endStroke() {
    if (!stroke) return;
    const finished = stroke;
    stroke = null;
    storeState(finished.state, finished.val === 0 ? "Painted hide mask" : "Painted reveal mask");
  }

  // Bind UI events
  $("mask-enabled").addEventListener("change", e => {
    const on = e.target.checked;
    const layer = getLayer();
    const cur = currentMask(layer);
    const shape = on && cur.mode === "parametric" && cur.shape === "none" ? "radial" : cur.shape;
    mutateMask({ enabled: on, shape }, on ? "Enabled layer mask" : "Disabled layer mask");
  });
  $("mask-mode").addEventListener("change", e => {
    mutateMask({ enabled: true, mode: e.target.value }, "Changed mask mode");
  });
  $("mask-shape").addEventListener("change", e => {
    const shape = e.target.value;
    const cur = currentMask(getLayer());
    const mode = shape === "none" ? (cur.hasRaster ? "raster" : "parametric")
      : cur.hasRaster ? "combined" : "parametric";
    mutateMask({ enabled: shape !== "none" || cur.hasRaster, mode, shape }, `Mask shape: ${shape}`);
  });
  $("mask-invert").addEventListener("change", e => mutateMask({ invert: e.target.checked }, "Inverted layer mask"));
  $("mask-ruby").addEventListener("change", e => {
    rubyOverlay = e.target.checked;
    requestPaint();
  });

  const bindMaskRange = (id, key, label, fmt) => {
    const el = $(id);
    const out = $(`${id}-val`);
    el.addEventListener("input", () => { if (out) out.value = fmt(el.valueAsNumber); });
    el.addEventListener("change", () => mutateMask({ enabled: true, [key]: el.valueAsNumber }, label));
  };
  bindMaskRange("mask-density", "density", "Mask density", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-feather", "feather", "Mask feather", v => `${v.toFixed(1)} px`);
  bindMaskRange("mask-angle", "angle", "Mask gradient angle", v => `${Math.round(v)}°`);
  bindMaskRange("mask-position", "position", "Mask gradient position", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-spread", "spread", "Mask gradient spread", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-cx", "cx", "Mask center X", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-cy", "cy", "Mask center Y", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-rx", "rx", "Mask radius X", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-ry", "ry", "Mask radius Y", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-softness", "softness", "Mask edge softness", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-inset", "inset", "Mask rect inset", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-radius", "radius", "Mask corner radius", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-luma-low", "low", "Mask luminance low", v => `${Math.round(v * 100)}%`);
  bindMaskRange("mask-luma-high", "high", "Mask luminance high", v => `${Math.round(v * 100)}%`);

  $("mask-brush-reveal").onclick = () => setBrushMode(brushMode === "reveal" ? null : "reveal");
  $("mask-brush-erase").onclick = () => setBrushMode(brushMode === "erase" ? null : "erase");
  $("mask-brush-stop").onclick = () => setBrushMode(null);
  $("mask-brush-size").addEventListener("input", e => {
    brushSize = e.target.valueAsNumber;
    $("mask-brush-size-val").value = `${Math.round(brushSize)} px`;
  });
  $("mask-brush-hardness").addEventListener("input", e => {
    brushHardness = e.target.valueAsNumber;
    $("mask-brush-hardness-val").value = `${Math.round(brushHardness * 100)}%`;
  });
  $("mask-brush-opacity").addEventListener("input", e => {
    brushOpacity = e.target.valueAsNumber;
    $("mask-brush-opacity-val").value = `${Math.round(brushOpacity * 100)}%`;
  });

  $("mask-fill-white").onclick = () => {
    const layer = getLayer();
    if (!layer) return;
    commitRasterMask(new Uint8Array(layer.width * layer.height).fill(255), "Filled mask white (reveal)");
  };
  $("mask-fill-black").onclick = () => {
    const layer = getLayer();
    if (!layer) return;
    commitRasterMask(new Uint8Array(layer.width * layer.height).fill(0), "Filled mask black (hide)");
  };
  $("mask-invert-paint").onclick = () => {
    const layer = getLayer();
    const state = getState();
    if (!layer || !state) return;
    const prev = state.assets.get(layer.id)?.maskAlpha;
    const next = prev ? prev.map(v => 255 - v) : new Uint8Array(layer.width * layer.height).fill(0);
    commitRasterMask(next, "Inverted painted mask");
  };
  $("mask-clear-paint").onclick = () => {
    const layer = getLayer();
    if (!layer) return;
    const cur = currentMask(layer);
    const nextState = patchState(getState(), layer.id, {
      mask: { ...cur, hasRaster: false, mode: cur.shape !== "none" ? "parametric" : "parametric", enabled: cur.shape !== "none" && cur.enabled },
    });
    const assets = new Map(nextState.assets);
    const prev = assets.get(layer.id);
    assets.set(layer.id, { ...prev, maskAlpha: null, maskJxl: null, maskRev: (prev.maskRev ?? 0) + 1 });
    storeState({ ...nextState, assets }, "Cleared painted mask");
  };

  $("mask-import-btn").onclick = () => $("mask-import-file").click();
  $("mask-import-file").addEventListener("change", event => {
    const file = event.target.files[0];
    event.target.value = "";
    if (!file) return;
    run("Importing mask image…", async () => {
      const layer = getLayer();
      if (!layer) return;
      const url = URL.createObjectURL(file);
      try {
        const img = new Image();
        img.src = url;
        await img.decode();
        const canvas = document.createElement("canvas");
        canvas.width = layer.width;
        canvas.height = layer.height;
        const cctx = canvas.getContext("2d");
        cctx.drawImage(img, 0, 0, layer.width, layer.height);
        const { data } = cctx.getImageData(0, 0, layer.width, layer.height);
        const alpha = new Uint8Array(layer.width * layer.height);
        for (let i = 0; i < alpha.length; i++) {
          const luma = Math.round(0.2126 * data[i * 4] + 0.7152 * data[i * 4 + 1] + 0.0722 * data[i * 4 + 2]);
          alpha[i] = Math.round(luma * (data[i * 4 + 3] / 255));
        }
        commitRasterMask(alpha, "Imported raster mask");
        status("Mask image imported");
      } finally {
        URL.revokeObjectURL(url);
      }
    });
  });

  function updateUI() {
    const state = getState();
    const layer = getLayer();
    const busy = getBusy();
    $("mask-fields").disabled = !layer || busy;
    if (!layer) return;
    const m = currentMask(layer);
    const asset = state.assets.get(layer.id);
    $("mask-enabled").checked = Boolean(m.enabled);
    $("mask-mode").value = m.mode;
    $("mask-shape").value = m.shape;
    $("mask-invert").checked = Boolean(m.invert);
    $("mask-ruby").checked = rubyOverlay;
    $("mask-density").value = String(m.density);
    $("mask-density-val").value = `${Math.round(m.density * 100)}%`;
    $("mask-feather").value = String(m.feather);
    $("mask-feather-val").value = `${Number(m.feather).toFixed(1)} px`;

    $("mask-linear-opts").hidden = m.shape !== "linear";
    $("mask-radial-opts").hidden = m.shape !== "radial";
    $("mask-rect-opts").hidden = m.shape !== "rounded_rect";
    $("mask-luma-opts").hidden = m.shape !== "luminance";

    $("mask-angle").value = String(m.angle);
    $("mask-angle-val").value = `${Math.round(m.angle)}°`;
    $("mask-position").value = String(m.position);
    $("mask-position-val").value = `${Math.round(m.position * 100)}%`;
    $("mask-spread").value = String(m.spread);
    $("mask-spread-val").value = `${Math.round(m.spread * 100)}%`;
    $("mask-cx").value = String(m.cx);
    $("mask-cx-val").value = `${Math.round(m.cx * 100)}%`;
    $("mask-cy").value = String(m.cy);
    $("mask-cy-val").value = `${Math.round(m.cy * 100)}%`;
    $("mask-rx").value = String(m.rx);
    $("mask-rx-val").value = `${Math.round(m.rx * 100)}%`;
    $("mask-ry").value = String(m.ry);
    $("mask-ry-val").value = `${Math.round(m.ry * 100)}%`;
    $("mask-softness").value = String(m.softness);
    $("mask-softness-val").value = `${Math.round(m.softness * 100)}%`;
    $("mask-inset").value = String(m.inset);
    $("mask-inset-val").value = `${Math.round(m.inset * 100)}%`;
    $("mask-radius").value = String(m.radius);
    $("mask-radius-val").value = `${Math.round(m.radius * 100)}%`;
    $("mask-luma-low").value = String(m.low);
    $("mask-luma-low-val").value = `${Math.round(m.low * 100)}%`;
    $("mask-luma-high").value = String(m.high);
    $("mask-luma-high-val").value = `${Math.round(m.high * 100)}%`;

    $("mask-raster-status").textContent = asset?.maskAlpha
      ? `Painted mask active (${layer.width} × ${layer.height}, saved losslessly in znmk box)`
      : "No painted mask yet — click Paint or Erase to brush directly on canvas.";
  }

  return {
    updateUI,
    isRubyOverlay: () => rubyOverlay,
    getBrushMode: () => brushMode,
    stopBrush: () => setBrushMode(null),
    startStroke,
    moveStroke,
    endStroke,
  };
}
