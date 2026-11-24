import { codec } from "./codec.mjs";
import { compactFilters, DEFAULT_FILTERS, effectiveLayerSize, targetScaledDimensions } from "./filters.mjs";

const AUTO_MODES = [
  ["autoTone", "Auto Tone"],
  ["autoWhiteBalance", "Auto WB"],
  ["autoLevels", "Auto Levels"],
  ["autoContrast", "Auto Contrast"],
  ["autoExposure", "Auto Exposure"],
  ["autoVibrance", "Auto Vibrance"],
];

const FALLBACK_KERNELS = [
  { id: "robidoux", label: "Robidoux (Balanced)" },
  { id: "lanczos", label: "Lanczos 3" },
  { id: "lanczos_sharp", label: "Lanczos 3 Sharp" },
  { id: "lanczos2", label: "Lanczos 2" },
  { id: "ginseng", label: "Ginseng (Upscale)" },
  { id: "ginseng_sharp", label: "Ginseng Sharp" },
  { id: "robidoux_sharp", label: "Robidoux Sharp" },
  { id: "mitchell", label: "Mitchell-Netravali" },
  { id: "catmull_rom", label: "Catmull-Rom" },
  { id: "cubic_b_spline", label: "Cubic B-Spline" },
  { id: "hermite", label: "Hermite" },
  { id: "linear", label: "Bilinear (Triangle)" },
  { id: "box", label: "Box (Nearest/Area)" },
];

function formatControlValue(ctrl, val) {
  if (ctrl.id === "posterize") return val < 2 ? "Off" : `${Math.round(val)} lv`;
  if (ctrl.id === "solarize") return val < 0.05 ? "Off" : `${Math.round(val * 100)}%`;
  if (ctrl.unit === "°") return `${Math.round(val)}°`;
  if (ctrl.unit === "px") return `${val.toFixed(1)} px`;
  return `${Math.round(val * 100)}%`;
}

export function makeZenFiltersPanel({
  getLayer,
  getBusy,
  previewPatch,
  commitPatch,
  clearPreview,
}) {
  const $ = id => document.getElementById(id);
  const mount = $("zenfilters-mount");
  const kernelSelect = $("resize-kernel");
  const linearCheck = $("resize-linear");
  const scaledInfo = $("scaled-dimensions");

  const controlRefs = new Map();
  const autoButtons = new Map();
  let presetSelect = null;
  let presetStrengthInput = null;
  let presetStrengthOut = null;
  let filmSelect = null;
  let filmStrengthInput = null;
  let filmStrengthOut = null;
  let filterMaskKind = null;
  let filterMaskInvert = null;
  let activeSlider = null;

  function currentFilters(layer) {
    return { ...DEFAULT_FILTERS, ...(layer?.filters ?? {}) };
  }

  function setFilterKey(key, value, label, live = false) {
    const layer = getLayer();
    if (!layer || getBusy()) return;
    const next = { ...currentFilters(layer), [key]: value };
    const patch = { filters: compactFilters(next) };
    if (live) previewPatch(patch);
    else commitPatch(patch, label);
  }

  function populateKernels(kernels) {
    const cur = kernelSelect.value || "robidoux";
    kernelSelect.replaceChildren();
    for (const k of kernels) {
      const opt = document.createElement("option");
      opt.value = k.id;
      opt.textContent = k.label;
      if (k.note) opt.title = k.note;
      kernelSelect.append(opt);
    }
    kernelSelect.value = cur;
  }

  populateKernels(FALLBACK_KERNELS);

  kernelSelect.addEventListener("change", () => {
    const layer = getLayer();
    if (!layer || getBusy()) return;
    commitPatch({ resizeKernel: kernelSelect.value }, "Changed resampling kernel");
  });
  linearCheck.addEventListener("change", () => {
    const layer = getLayer();
    if (!layer || getBusy()) return;
    commitPatch({ resizeLinear: linearCheck.checked }, "Toggled linear-light resampling");
  });

  function buildCatalogUI(catalog) {
    mount.replaceChildren();
    controlRefs.clear();
    autoButtons.clear();

    // 1. One-Click Auto Enhance pills
    const autoHeading = document.createElement("h3");
    autoHeading.textContent = "ZenFilters Auto & Looks";
    mount.append(autoHeading);

    const autoRow = document.createElement("div");
    autoRow.className = "pill-row";
    for (const [key, label] of AUTO_MODES.slice(0, 4)) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "pill-btn";
      btn.id = `zf-${key}`;
      btn.textContent = label;
      btn.setAttribute("aria-pressed", "false");
      btn.addEventListener("click", () => {
        const layer = getLayer();
        if (!layer || getBusy()) return;
        const f = currentFilters(layer);
        const on = (f[key] ?? 0) > 0.01;
        setFilterKey(key, on ? 0 : 0.85, on ? `Disabled ${label}` : `Applied ${label}`);
      });
      autoButtons.set(key, btn);
      autoRow.append(btn);
    }
    mount.append(autoRow);

    // 2. Presets (19 built-in ZenFilters presets)
    const looksGrid = document.createElement("div");
    looksGrid.className = "zf-looks";

    const presetLabel = document.createElement("label");
    presetLabel.textContent = "ZenFilters Preset";
    presetSelect = document.createElement("select");
    presetSelect.id = "zf-preset";
    const noneP = document.createElement("option");
    noneP.value = "";
    noneP.textContent = "None (Custom)";
    presetSelect.append(noneP);
    const byCat = new Map();
    for (const p of catalog.presets) {
      if (!byCat.has(p.category)) byCat.set(p.category, []);
      byCat.get(p.category).push(p);
    }
    for (const [cat, items] of byCat) {
      const og = document.createElement("optgroup");
      og.label = cat;
      for (const p of items) {
        const opt = document.createElement("option");
        opt.value = p.name;
        opt.textContent = p.name;
        opt.title = p.description;
        og.append(opt);
      }
      presetSelect.append(og);
    }
    presetSelect.addEventListener("change", () => {
      setFilterKey("preset", presetSelect.value || null, presetSelect.value ? `Preset: ${presetSelect.value}` : "Cleared preset");
    });
    presetLabel.append(presetSelect);

    const pStrWrap = document.createElement("label");
    pStrWrap.className = "adjustment";
    const pStrHead = document.createElement("span");
    presetStrengthOut = document.createElement("output");
    presetStrengthOut.id = "zf-preset-strength-val";
    pStrHead.append("Preset intensity", presetStrengthOut);
    presetStrengthInput = document.createElement("input");
    presetStrengthInput.type = "range";
    presetStrengthInput.id = "zf-preset-strength";
    presetStrengthInput.min = "0";
    presetStrengthInput.max = "1";
    presetStrengthInput.step = "0.02";
    presetStrengthInput.addEventListener("input", () => {
      activeSlider = "presetStrength";
      presetStrengthOut.value = `${Math.round(presetStrengthInput.valueAsNumber * 100)}%`;
      setFilterKey("presetStrength", presetStrengthInput.valueAsNumber, "Preset intensity", true);
    });
    presetStrengthInput.addEventListener("change", () => {
      if (!getLayer() || getBusy()) return;
      activeSlider = null;
      clearPreview();
      presetStrengthOut.value = `${Math.round(presetStrengthInput.valueAsNumber * 100)}%`;
      setFilterKey("presetStrength", presetStrengthInput.valueAsNumber, "Preset intensity", false);
    });
    pStrWrap.append(pStrHead, presetStrengthInput);

    // 3. Film Looks (34 tensor-LUT Film Emulations)
    const filmLabel = document.createElement("label");
    filmLabel.textContent = "Film Emulation LUT (34 looks)";
    filmSelect = document.createElement("select");
    filmSelect.id = "zf-film-look";
    const noneF = document.createElement("option");
    noneF.value = "";
    noneF.textContent = "None";
    filmSelect.append(noneF);
    const filmByCat = new Map();
    for (const f of catalog.filmLooks) {
      if (!filmByCat.has(f.category)) filmByCat.set(f.category, []);
      filmByCat.get(f.category).push(f);
    }
    for (const [cat, items] of filmByCat) {
      const og = document.createElement("optgroup");
      og.label = cat;
      for (const f of items) {
        const opt = document.createElement("option");
        opt.value = f.id;
        opt.textContent = f.name;
        og.append(opt);
      }
      filmSelect.append(og);
    }
    filmSelect.addEventListener("change", () => {
      setFilterKey("filmLook", filmSelect.value || null, filmSelect.value ? `Film Look: ${filmSelect.selectedOptions[0]?.textContent}` : "Cleared film look");
    });
    filmLabel.append(filmSelect);

    const fStrWrap = document.createElement("label");
    fStrWrap.className = "adjustment";
    const fStrHead = document.createElement("span");
    filmStrengthOut = document.createElement("output");
    filmStrengthOut.id = "zf-film-strength-val";
    fStrHead.append("Film strength", filmStrengthOut);
    filmStrengthInput = document.createElement("input");
    filmStrengthInput.type = "range";
    filmStrengthInput.id = "zf-film-strength";
    filmStrengthInput.min = "0";
    filmStrengthInput.max = "1";
    filmStrengthInput.step = "0.02";
    filmStrengthInput.addEventListener("input", () => {
      activeSlider = "filmStrength";
      filmStrengthOut.value = `${Math.round(filmStrengthInput.valueAsNumber * 100)}%`;
      setFilterKey("filmStrength", filmStrengthInput.valueAsNumber, "Film strength", true);
    });
    filmStrengthInput.addEventListener("change", () => {
      if (!getLayer() || getBusy()) return;
      activeSlider = null;
      clearPreview();
      filmStrengthOut.value = `${Math.round(filmStrengthInput.valueAsNumber * 100)}%`;
      setFilterKey("filmStrength", filmStrengthInput.valueAsNumber, "Film strength", false);
    });
    fStrWrap.append(fStrHead, filmStrengthInput);

    looksGrid.append(presetLabel, pStrWrap, filmLabel, fStrWrap);
    mount.append(looksGrid);

    // 4. Grouped ZenFilters collapsible sections
    const groups = new Map();
    for (const ctrl of catalog.controls) {
      if (!groups.has(ctrl.group)) groups.set(ctrl.group, []);
      groups.get(ctrl.group).push(ctrl);
    }
    for (const [groupName, items] of groups) {
      const det = document.createElement("details");
      det.className = "zf-group";
      det.dataset.zfGroup = groupName.toLowerCase();
      if (groupName === "Tone" || groupName === "Color") det.open = false;
      const sum = document.createElement("summary");
      sum.textContent = `${groupName} (${items.length})`;
      det.append(sum);

      for (const ctrl of items) {
        if (ctrl.kind === "bool") {
          const lbl = document.createElement("label");
          lbl.className = "inline-check";
          const cb = document.createElement("input");
          cb.type = "checkbox";
          cb.id = `zf-${ctrl.id}`;
          cb.addEventListener("change", () => setFilterKey(ctrl.id, cb.checked, ctrl.label));
          lbl.append(cb, ` ${ctrl.label}`);
          controlRefs.set(ctrl.id, { ctrl, input: cb, output: null });
          det.append(lbl);
        } else {
          const lbl = document.createElement("label");
          lbl.className = "adjustment";
          const head = document.createElement("span");
          const out = document.createElement("output");
          out.id = `zf-val-${ctrl.id}`;
          head.append(ctrl.label, out);
          const input = document.createElement("input");
          input.type = "range";
          input.id = `zf-${ctrl.id}`;
          input.min = String(ctrl.min);
          input.max = String(ctrl.max);
          input.step = String(ctrl.step);
          input.setAttribute("aria-label", ctrl.label);
          const coerce = v => {
            if (ctrl.id === "posterize") return v < 2 ? 0 : Math.round(v);
            if (ctrl.id === "solarize") return v < 0.05 ? 0 : v;
            return v;
          };
          input.addEventListener("input", () => {
            activeSlider = ctrl.id;
            const v = coerce(input.valueAsNumber);
            out.value = formatControlValue(ctrl, v);
            setFilterKey(ctrl.id, v, ctrl.label, true);
          });
          input.addEventListener("change", () => {
            if (!getLayer() || getBusy()) return;
            activeSlider = null;
            clearPreview();
            const v = coerce(input.valueAsNumber);
            out.value = formatControlValue(ctrl, v);
            setFilterKey(ctrl.id, v, ctrl.label, false);
          });
          lbl.append(head, input);
          controlRefs.set(ctrl.id, { ctrl, input, output: out });
          det.append(lbl);
        }
      }
      mount.append(det);
    }

    // 5. Selective Filter Mask (apply adjustments only to gradient or luminance zone)
    const fmDet = document.createElement("details");
    fmDet.className = "zf-group";
    const fmSum = document.createElement("summary");
    fmSum.textContent = "Selective Filter Mask";
    fmDet.append(fmSum);
    const fmLabel = document.createElement("label");
    fmLabel.textContent = "Apply adjustments to";
    filterMaskKind = document.createElement("select");
    filterMaskKind.id = "zf-filter-mask";
    for (const [val, txt] of [
      ["none", "Entire layer"],
      ["linear", "Graduated linear (top → bottom)"],
      ["radial", "Radial spotlight (center)"],
      ["luminance", "Highlights only (luminance)"],
    ]) {
      const o = document.createElement("option");
      o.value = val;
      o.textContent = txt;
      filterMaskKind.append(o);
    }
    const buildFilterMask = () => {
      const k = filterMaskKind.value;
      if (k === "none") return null;
      const inv = filterMaskInvert.checked;
      if (k === "linear") return { kind: "linear", x0: 0.5, y0: 0, x1: 0.5, y1: 1, invert: inv };
      if (k === "radial") return { kind: "radial", cx: 0.5, cy: 0.5, innerRadius: 0.2, outerRadius: 0.55, invert: inv };
      return { kind: "luminance", low: 0.55, high: 1.0, feather: 0.15, invert: inv };
    };
    filterMaskKind.addEventListener("change", () => setFilterKey("filterMask", buildFilterMask(), "Updated selective filter mask"));
    fmLabel.append(filterMaskKind);

    const invLbl = document.createElement("label");
    invLbl.className = "inline-check";
    filterMaskInvert = document.createElement("input");
    filterMaskInvert.type = "checkbox";
    filterMaskInvert.id = "zf-filter-mask-invert";
    filterMaskInvert.addEventListener("change", () => setFilterKey("filterMask", buildFilterMask(), "Inverted selective filter mask"));
    invLbl.append(filterMaskInvert, " Invert selective zone");
    fmDet.append(fmLabel, invLbl);
    mount.append(fmDet);
  }

  let catalogLoaded = false;
  async function loadCatalog() {
    if (catalogLoaded) return;
    catalogLoaded = true;
    try {
      const [catalog, kernels] = await Promise.all([
        codec("filterCatalog"),
        codec("resizeKernels"),
      ]);
      populateKernels(kernels);
      buildCatalogUI(catalog);
      sync(getLayer());
    } catch {
      catalogLoaded = false;
    }
  }

  function sync(layer) {
    const f = currentFilters(layer);
    for (const [key, btn] of autoButtons) {
      btn.setAttribute("aria-pressed", String((f[key] ?? 0) > 0.01));
    }
    if (presetSelect) {
      presetSelect.value = f.preset ?? "";
      presetStrengthInput.value = String(f.presetStrength ?? 1);
      presetStrengthOut.value = `${Math.round((f.presetStrength ?? 1) * 100)}%`;
      presetStrengthInput.disabled = !f.preset;
    }
    if (filmSelect) {
      filmSelect.value = f.filmLook ?? "";
      filmStrengthInput.value = String(f.filmStrength ?? 1);
      filmStrengthOut.value = `${Math.round((f.filmStrength ?? 1) * 100)}%`;
      filmStrengthInput.disabled = !f.filmLook;
    }
    for (const [id, { ctrl, input, output }] of controlRefs) {
      const val = f[id] ?? ctrl.default;
      if (ctrl.kind === "bool") {
        input.checked = Boolean(val);
      } else {
        input.value = String(val);
        output.value = formatControlValue(ctrl, Number(val));
      }
    }
    if (filterMaskKind) {
      filterMaskKind.value = f.filterMask?.kind ?? "none";
      filterMaskInvert.checked = Boolean(f.filterMask?.invert);
      filterMaskInvert.disabled = filterMaskKind.value === "none";
    }
    if (layer) {
      kernelSelect.value = layer.resizeKernel ?? "robidoux";
      linearCheck.checked = layer.resizeLinear !== false;
      const isSvg = layer.mime === "image/svg+xml";
      kernelSelect.disabled = isSvg;
      linearCheck.disabled = isSvg;
      const eff = effectiveLayerSize(layer);
      const t = targetScaledDimensions(layer);
      const srTag = eff.sr > 1 ? ` · ZenSR ${eff.sr}× (${eff.width}×${eff.height})` : "";
      scaledInfo.textContent = isSvg
        ? (t.scaled ? `Vector SVG re-rasterized crisply at ${t.width} × ${t.height} px` : `Vector SVG at native ${layer.width} × ${layer.height} px`)
        : (t.scaled ? `zenresize: ${eff.width}×${eff.height} → ${t.width}×${t.height} px (${kernelSelect.value}${linearCheck.checked ? ", linear" : ""})${srTag}` : `Effective ${eff.width} × ${eff.height} px (100%)${srTag}`);
    } else {
      scaledInfo.textContent = "";
    }
  }

  return { loadCatalog, sync };
}
