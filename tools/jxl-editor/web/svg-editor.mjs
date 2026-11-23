const SVG_NS = "http://www.w3.org/2000/svg";
const EDITABLE_TAGS = new Set(["text", "rect", "circle", "ellipse", "line", "polygon", "path"]);
const parser = new DOMParser();
const serializer = new XMLSerializer();

function normalizeColorHex(value, fallback = "#a7c5ff") {
  if (!value || value === "none" || value === "transparent") return fallback;
  const trimmed = value.trim();
  if (/^#[0-9a-f]{6}$/i.test(trimmed)) return trimmed.toLowerCase();
  if (/^#[0-9a-f]{3}$/i.test(trimmed)) {
    return `#${trimmed[1]}${trimmed[1]}${trimmed[2]}${trimmed[2]}${trimmed[3]}${trimmed[3]}`.toLowerCase();
  }
  return fallback;
}

function parseSvgDoc(markup) {
  const clean = markup.replace(/^\uFEFF/, "");
  const doc = parser.parseFromString(clean, "image/svg+xml");
  const parseErr = doc.querySelector("parsererror");
  if (parseErr) return { doc: null, error: "SVG XML syntax error." };
  const root = doc.documentElement;
  if (!root || root.localName !== "svg") return { doc: null, error: "Root element must be <svg>." };
  return { doc, root, error: null };
}

function listEditableElements(root) {
  if (!root) return [];
  return [...root.querySelectorAll("*")].filter(el => EDITABLE_TAGS.has(el.localName));
}

function describeElement(el, idx) {
  const tag = el.localName;
  if (tag === "text") {
    const label = (el.textContent || "").trim().slice(0, 22) || "(empty)";
    return `${idx + 1}. <text> “${label}”`;
  }
  const fill = el.getAttribute("fill");
  return `${idx + 1}. <${tag}>${fill ? ` (${fill})` : ""}`;
}

export function makeSvgEditor({ getActiveSvgMarkup, applySvgMarkup, getBusy }) {
  const $ = id => document.getElementById(id);
  const dialog = $("svg-dialog");
  const sourceArea = $("svg-source");
  const errorEl = $("svg-error");
  const previewImg = $("svg-preview");
  const elSelect = $("svg-el-select");
  const widthInput = $("svg-width");
  const heightInput = $("svg-height");
  const elFields = $("svg-el-fields");
  const textRow = $("svg-el-text-row");
  const textInput = $("svg-el-text");
  const fontSizeInput = $("svg-el-fontsize");
  const fillInput = $("svg-el-fill");
  const fillNoneInput = $("svg-el-fill-none");
  const strokeInput = $("svg-el-stroke");
  const strokeNoneInput = $("svg-el-stroke-none");
  const strokeWidthInput = $("svg-el-stroke-width");
  const opacityInput = $("svg-el-opacity");
  const geomX = $("svg-el-x");
  const geomY = $("svg-el-y");
  const geomW = $("svg-el-w");
  const geomH = $("svg-el-h");

  let previewUrl = null;
  let parsedDoc = null;
  let elements = [];
  let selectedIndex = -1;

  function updatePreview(markup) {
    if (previewUrl) URL.revokeObjectURL(previewUrl);
    previewUrl = URL.createObjectURL(new Blob([markup], { type: "image/svg+xml" }));
    previewImg.src = previewUrl;
  }

  function syncInspectorFromMarkup(keepIndex = true) {
    const markup = sourceArea.value;
    updatePreview(markup);
    const { doc, root, error } = parseSvgDoc(markup);
    if (error) {
      parsedDoc = null;
      elements = [];
      selectedIndex = -1;
      errorEl.textContent = error;
      elSelect.replaceChildren();
      elFields.disabled = true;
      return;
    }
    errorEl.textContent = "";
    parsedDoc = doc;
    widthInput.value = parseFloat(root.getAttribute("width")) || "";
    heightInput.value = parseFloat(root.getAttribute("height")) || "";
    elements = listEditableElements(root);
    if (!keepIndex || selectedIndex < 0 || selectedIndex >= elements.length) {
      selectedIndex = elements.length ? 0 : -1;
    }
    elSelect.replaceChildren();
    elements.forEach((el, idx) => {
      const opt = document.createElement("option");
      opt.value = String(idx);
      opt.textContent = describeElement(el, idx);
      elSelect.append(opt);
    });
    if (selectedIndex >= 0) elSelect.value = String(selectedIndex);
    populateSelectedElement();
  }

  function populateSelectedElement() {
    const el = elements[selectedIndex];
    elFields.disabled = !el;
    $("svg-el-delete").disabled = !el;
    if (!el) return;
    const tag = el.localName;
    const isText = tag === "text";
    textRow.hidden = !isText;
    if (isText) {
      textInput.value = el.textContent ?? "";
      fontSizeInput.value = parseFloat(el.getAttribute("font-size")) || 24;
    }
    const fill = el.getAttribute("fill");
    const noFill = fill === "none";
    fillNoneInput.checked = noFill;
    fillInput.disabled = noFill;
    fillInput.value = normalizeColorHex(fill, "#a7c5ff");

    const stroke = el.getAttribute("stroke");
    const noStroke = !stroke || stroke === "none";
    strokeNoneInput.checked = noStroke;
    strokeInput.disabled = noStroke;
    strokeInput.value = normalizeColorHex(stroke, "#ffffff");
    strokeWidthInput.value = parseFloat(el.getAttribute("stroke-width")) || 2;
    opacityInput.value = parseFloat(el.getAttribute("opacity") ?? el.getAttribute("fill-opacity")) || 1;

    if (tag === "circle") {
      geomX.value = el.getAttribute("cx") ?? "0";
      geomY.value = el.getAttribute("cy") ?? "0";
      geomW.value = el.getAttribute("r") ?? "24";
      geomH.value = "";
      geomH.disabled = true;
    } else if (tag === "ellipse") {
      geomX.value = el.getAttribute("cx") ?? "0";
      geomY.value = el.getAttribute("cy") ?? "0";
      geomW.value = el.getAttribute("rx") ?? "30";
      geomH.value = el.getAttribute("ry") ?? "20";
      geomH.disabled = false;
    } else if (tag === "line") {
      geomX.value = el.getAttribute("x1") ?? "0";
      geomY.value = el.getAttribute("y1") ?? "0";
      geomW.value = el.getAttribute("x2") ?? "80";
      geomH.value = el.getAttribute("y2") ?? "80";
      geomH.disabled = false;
    } else {
      geomX.value = el.getAttribute("x") ?? "";
      geomY.value = el.getAttribute("y") ?? "";
      geomW.value = el.getAttribute("width") ?? "";
      geomH.value = el.getAttribute("height") ?? "";
      geomH.disabled = tag === "text" || tag === "path" || tag === "polygon";
    }
  }

  function commitDomToMarkup() {
    if (!parsedDoc) return;
    let serialized = serializer.serializeToString(parsedDoc);
    // Keep clean indentation readability for newly added elements
    serialized = serialized.replace(/></g, ">\n  <").replace(/\n  <\/svg>/, "\n</svg>");
    sourceArea.value = serialized;
    syncInspectorFromMarkup(true);
  }

  function mutateSelected(fn) {
    const el = elements[selectedIndex];
    if (!el || !parsedDoc) return;
    fn(el);
    commitDomToMarkup();
  }

  function addShape(kind) {
    if (!parsedDoc) return;
    const root = parsedDoc.documentElement;
    const w = parseFloat(root.getAttribute("width")) || 320;
    const h = parseFloat(root.getAttribute("height")) || 180;
    const cx = Math.round(w / 2);
    const cy = Math.round(h / 2);
    let el;
    if (kind === "text") {
      el = parsedDoc.createElementNS(SVG_NS, "text");
      el.setAttribute("x", String(cx));
      el.setAttribute("y", String(cy));
      el.setAttribute("text-anchor", "middle");
      el.setAttribute("fill", "#ffffff");
      el.setAttribute("font-family", "system-ui, sans-serif");
      el.setAttribute("font-size", "26");
      el.setAttribute("font-weight", "700");
      el.textContent = "New Label";
    } else if (kind === "rect") {
      el = parsedDoc.createElementNS(SVG_NS, "rect");
      el.setAttribute("x", String(Math.max(4, cx - 50)));
      el.setAttribute("y", String(Math.max(4, cy - 30)));
      el.setAttribute("width", "100");
      el.setAttribute("height", "60");
      el.setAttribute("rx", "10");
      el.setAttribute("fill", "#3b82f6");
      el.setAttribute("stroke", "#ffffff");
      el.setAttribute("stroke-width", "2");
    } else if (kind === "circle") {
      el = parsedDoc.createElementNS(SVG_NS, "circle");
      el.setAttribute("cx", String(cx));
      el.setAttribute("cy", String(cy));
      el.setAttribute("r", String(Math.max(12, Math.round(Math.min(w, h) * 0.22))));
      el.setAttribute("fill", "#ec4899");
      el.setAttribute("stroke", "#ffffff");
      el.setAttribute("stroke-width", "2");
    } else if (kind === "line") {
      el = parsedDoc.createElementNS(SVG_NS, "line");
      el.setAttribute("x1", String(Math.round(w * 0.15)));
      el.setAttribute("y1", String(cy));
      el.setAttribute("x2", String(Math.round(w * 0.85)));
      el.setAttribute("y2", String(cy));
      el.setAttribute("stroke", "#f5dc3c");
      el.setAttribute("stroke-width", "4");
      el.setAttribute("stroke-linecap", "round");
    }
    if (!el) return;
    root.appendChild(el);
    elements = listEditableElements(root);
    selectedIndex = elements.length - 1;
    commitDomToMarkup();
  }

  sourceArea.addEventListener("input", () => syncInspectorFromMarkup(true));
  elSelect.addEventListener("change", () => {
    selectedIndex = Number(elSelect.value);
    populateSelectedElement();
  });

  widthInput.addEventListener("change", () => {
    const val = Math.round(widthInput.valueAsNumber);
    if (!parsedDoc || !Number.isFinite(val) || val < 1 || val > 8192) return;
    const root = parsedDoc.documentElement;
    root.setAttribute("width", String(val));
    const h = parseFloat(root.getAttribute("height")) || val;
    root.setAttribute("viewBox", `0 0 ${val} ${h}`);
    commitDomToMarkup();
  });
  heightInput.addEventListener("change", () => {
    const val = Math.round(heightInput.valueAsNumber);
    if (!parsedDoc || !Number.isFinite(val) || val < 1 || val > 8192) return;
    const root = parsedDoc.documentElement;
    root.setAttribute("height", String(val));
    const w = parseFloat(root.getAttribute("width")) || val;
    root.setAttribute("viewBox", `0 0 ${w} ${val}`);
    commitDomToMarkup();
  });

  textInput.addEventListener("input", () => mutateSelected(el => { el.textContent = textInput.value; }));
  fontSizeInput.addEventListener("change", () => mutateSelected(el => {
    const n = fontSizeInput.valueAsNumber;
    if (Number.isFinite(n) && n >= 4 && n <= 400) el.setAttribute("font-size", String(n));
  }));
  fillInput.addEventListener("input", () => mutateSelected(el => {
    fillNoneInput.checked = false;
    el.setAttribute("fill", fillInput.value);
  }));
  fillNoneInput.addEventListener("change", () => mutateSelected(el => {
    el.setAttribute("fill", fillNoneInput.checked ? "none" : fillInput.value);
  }));
  strokeInput.addEventListener("input", () => mutateSelected(el => {
    strokeNoneInput.checked = false;
    el.setAttribute("stroke", strokeInput.value);
    if (!el.getAttribute("stroke-width")) el.setAttribute("stroke-width", strokeWidthInput.value || "2");
  }));
  strokeNoneInput.addEventListener("change", () => mutateSelected(el => {
    if (strokeNoneInput.checked) el.setAttribute("stroke", "none");
    else {
      el.setAttribute("stroke", strokeInput.value);
      if (!el.getAttribute("stroke-width")) el.setAttribute("stroke-width", strokeWidthInput.value || "2");
    }
  }));
  strokeWidthInput.addEventListener("change", () => mutateSelected(el => {
    const n = strokeWidthInput.valueAsNumber;
    if (Number.isFinite(n) && n >= 0 && n <= 100) el.setAttribute("stroke-width", String(n));
  }));
  opacityInput.addEventListener("input", () => mutateSelected(el => {
    const n = opacityInput.valueAsNumber;
    if (Number.isFinite(n) && n >= 0 && n <= 1) el.setAttribute("opacity", String(n));
  }));

  function bindGeom() {
    mutateSelected(el => {
      const tag = el.localName;
      if (tag === "circle") {
        if (geomX.value !== "") el.setAttribute("cx", geomX.value);
        if (geomY.value !== "") el.setAttribute("cy", geomY.value);
        if (geomW.value !== "") el.setAttribute("r", geomW.value);
      } else if (tag === "ellipse") {
        if (geomX.value !== "") el.setAttribute("cx", geomX.value);
        if (geomY.value !== "") el.setAttribute("cy", geomY.value);
        if (geomW.value !== "") el.setAttribute("rx", geomW.value);
        if (geomH.value !== "") el.setAttribute("ry", geomH.value);
      } else if (tag === "line") {
        if (geomX.value !== "") el.setAttribute("x1", geomX.value);
        if (geomY.value !== "") el.setAttribute("y1", geomY.value);
        if (geomW.value !== "") el.setAttribute("x2", geomW.value);
        if (geomH.value !== "") el.setAttribute("y2", geomH.value);
      } else {
        if (geomX.value !== "") el.setAttribute("x", geomX.value);
        if (geomY.value !== "") el.setAttribute("y", geomY.value);
        if (geomW.value !== "" && !geomW.disabled) el.setAttribute("width", geomW.value);
        if (geomH.value !== "" && !geomH.disabled) el.setAttribute("height", geomH.value);
      }
    });
  }
  for (const inp of [geomX, geomY, geomW, geomH]) inp.addEventListener("change", bindGeom);

  $("svg-add-text").onclick = () => addShape("text");
  $("svg-add-rect").onclick = () => addShape("rect");
  $("svg-add-circle").onclick = () => addShape("circle");
  $("svg-add-line").onclick = () => addShape("line");
  $("svg-el-delete").onclick = () => {
    const el = elements[selectedIndex];
    if (!el || !parsedDoc) return;
    el.remove();
    selectedIndex = Math.max(0, selectedIndex - 1);
    commitDomToMarkup();
  };

  $("svg-cancel").onclick = () => dialog.close();
  dialog.addEventListener("cancel", event => { if (getBusy()) event.preventDefault(); });
  dialog.addEventListener("close", () => {
    if (previewUrl) { URL.revokeObjectURL(previewUrl); previewUrl = null; }
  });

  $("svg-form").onsubmit = async event => {
    event.preventDefault();
    if (getBusy()) return;
    await applySvgMarkup(sourceArea.value, errorEl, dialog);
  };

  return {
    open() {
      errorEl.textContent = "";
      sourceArea.value = getActiveSvgMarkup();
      syncInspectorFromMarkup(false);
      dialog.showModal();
    },
  };
}
