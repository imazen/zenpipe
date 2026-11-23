const utf8 = new TextEncoder();

export const SVG_TEMPLATES = {
  "svg-title": {
    name: "Hero Title Banner.svg",
    title: "Hero Title Banner",
    category: "vectors",
    badge: "SVG",
    note: "Editable headline, subtitle, and accent pill",
    markup: `<svg xmlns="http://www.w3.org/2000/svg" width="420" height="120" viewBox="0 0 420 120">
  <rect x="4" y="4" width="412" height="112" rx="18" fill="#182338" fill-opacity="0.88" stroke="#a7c5ff" stroke-width="2.5"/>
  <rect x="24" y="26" width="8" height="68" rx="4" fill="#f5b841"/>
  <text x="48" y="62" fill="#f5f8ff" font-family="system-ui, sans-serif" font-size="34" font-weight="700">JPEG XL Studio</text>
  <text x="48" y="90" fill="#a7c5ff" font-family="system-ui, sans-serif" font-size="16" font-weight="500">Non-destructive vector title · click Edit SVG</text>
</svg>`,
  },
  "svg-badge": {
    name: "Studio Seal.svg",
    title: "Studio Seal Badge",
    category: "vectors",
    badge: "SVG",
    note: "Circular vector emblem with editable text",
    markup: `<svg xmlns="http://www.w3.org/2000/svg" width="180" height="180" viewBox="0 0 180 180">
  <circle cx="90" cy="90" r="82" fill="#1b2b48" stroke="#f5c344" stroke-width="5"/>
  <circle cx="90" cy="90" r="68" fill="none" stroke="#a7c5ff" stroke-width="2" stroke-dasharray="6 4"/>
  <polygon points="90,34 97,54 118,54 101,66 107,86 90,74 73,86 79,66 62,54 83,54" fill="#f5c344"/>
  <text x="90" y="118" text-anchor="middle" fill="#ffffff" font-family="system-ui, sans-serif" font-size="22" font-weight="700">JXL PRO</text>
  <text x="90" y="138" text-anchor="middle" fill="#a7c5ff" font-family="system-ui, sans-serif" font-size="12" font-weight="600">LOSSLESS EDITION</text>
</svg>`,
  },
  "svg-callout": {
    name: "Callout Arrow.svg",
    title: "Callout Arrow & Label",
    category: "vectors",
    badge: "SVG",
    note: "Annotation pointer with editable label",
    markup: `<svg xmlns="http://www.w3.org/2000/svg" width="260" height="110" viewBox="0 0 260 110">
  <rect x="78" y="12" width="172" height="52" rx="12" fill="#e94b6a" stroke="#ffffff" stroke-width="2.5"/>
  <line x1="78" y1="50" x2="18" y2="92" stroke="#ffffff" stroke-width="4" stroke-linecap="round"/>
  <circle cx="16" cy="94" r="8" fill="#f5dc3c" stroke="#182030" stroke-width="2.5"/>
  <text x="164" y="45" text-anchor="middle" fill="#ffffff" font-family="system-ui, sans-serif" font-size="20" font-weight="700">Key Detail</text>
</svg>`,
  },
  "svg-frame": {
    name: "Gallery Frame.svg",
    title: "Gallery Frame",
    category: "overlays",
    badge: "SVG",
    note: "Crisp architectural vector frame overlay",
    markup: `<svg xmlns="http://www.w3.org/2000/svg" width="560" height="380" viewBox="0 0 560 380">
  <rect x="12" y="12" width="536" height="356" rx="10" fill="none" stroke="#ffffff" stroke-width="6"/>
  <rect x="24" y="24" width="512" height="332" rx="6" fill="none" stroke="#a7c5ff" stroke-width="1.5" stroke-opacity="0.85"/>
  <rect x="180" y="334" width="200" height="28" rx="14" fill="#171d28" fill-opacity="0.9" stroke="#a7c5ff" stroke-width="1.5"/>
  <text x="280" y="353" text-anchor="middle" fill="#edf2fb" font-family="system-ui, sans-serif" font-size="13" font-weight="600">STUDIO ARCHIVE · JXL</text>
</svg>`,
  },
  "svg-geometric": {
    name: "Neon Geometric.svg",
    title: "Neon Geometric Accent",
    category: "vectors",
    badge: "SVG",
    note: "Multi-shape vector accent for compositing",
    markup: `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="240" viewBox="0 0 240 240">
  <circle cx="120" cy="120" r="96" fill="#2563eb" fill-opacity="0.28" stroke="#60a5fa" stroke-width="3"/>
  <polygon points="120,26 206,174 34,174" fill="#ec4899" fill-opacity="0.42" stroke="#f9a8d4" stroke-width="3"/>
  <rect x="74" y="74" width="92" height="92" rx="14" fill="#f59e0b" fill-opacity="0.45" stroke="#fde68a" stroke-width="3"/>
  <circle cx="120" cy="120" r="24" fill="#ffffff"/>
</svg>`,
  },
  "svg-watermark": {
    name: "Corner Watermark.svg",
    title: "Corner Watermark",
    category: "overlays",
    badge: "SVG",
    note: "Clean translucent vector mark",
    markup: `<svg xmlns="http://www.w3.org/2000/svg" width="240" height="56" viewBox="0 0 240 56">
  <rect x="2" y="2" width="236" height="52" rx="26" fill="#101622" fill-opacity="0.58" stroke="#ffffff" stroke-opacity="0.45" stroke-width="1.5"/>
  <circle cx="30" cy="28" r="12" fill="#a7c5ff"/>
  <text x="54" y="34" fill="#ffffff" fill-opacity="0.92" font-family="system-ui, sans-serif" font-size="18" font-weight="650">© STUDIO JXL</text>
</svg>`,
  },
};

function canvasToBlob(canvas, type = "image/png", quality = undefined) {
  return new Promise((resolve, reject) => {
    canvas.toBlob(blob => {
      if (blob) resolve(blob);
      else reject(new Error(`Failed to render ${type} gallery item.`));
    }, type, quality);
  });
}

function drawColorChart(width = 512, height = 340) {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  const sky = ctx.createLinearGradient(0, 0, 0, height * 0.56);
  sky.addColorStop(0, "#16264c");
  sky.addColorStop(0.52, "#4b5d9e");
  sky.addColorStop(0.84, "#e38c5a");
  sky.addColorStop(1, "#f7d47c");
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, width, height);

  // Sun + mountain silhouette
  ctx.fillStyle = "#fff1c2";
  ctx.beginPath();
  ctx.arc(width * 0.72, height * 0.34, 32, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = "#202b44";
  ctx.beginPath();
  ctx.moveTo(0, height * 0.58);
  ctx.lineTo(width * 0.22, height * 0.34);
  ctx.lineTo(width * 0.44, height * 0.52);
  ctx.lineTo(width * 0.68, height * 0.38);
  ctx.lineTo(width, height * 0.54);
  ctx.lineTo(width, height);
  ctx.lineTo(0, height);
  ctx.closePath();
  ctx.fill();

  // Calibrated color patches + gray ramp
  const patches = [
    "#735244", "#c29682", "#627a9d", "#576c43", "#8580b1", "#67bdaa",
    "#d67e2c", "#505ba6", "#c15a63", "#5e3c6c", "#9dbc40", "#e0a32e",
  ];
  const pad = 16;
  const cols = 6;
  const cellW = (width - pad * 2 - (cols - 1) * 8) / cols;
  const cellH = 34;
  const startY = Math.round(height * 0.62);
  patches.forEach((color, idx) => {
    const col = idx % cols;
    const row = Math.floor(idx / cols);
    ctx.fillStyle = color;
    ctx.fillRect(pad + col * (cellW + 8), startY + row * (cellH + 8), cellW, cellH);
  });
  const grayY = startY + 2 * (cellH + 8) + 4;
  for (let i = 0; i < 6; i++) {
    const v = Math.round(245 - i * 44);
    ctx.fillStyle = `rgb(${v},${v},${v})`;
    ctx.fillRect(pad + i * (cellW + 8), grayY, cellW, 20);
  }
  return canvas;
}

function drawLowQJpegScene(width = 384, height = 256) {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  const grad = ctx.createLinearGradient(0, 0, width, height);
  grad.addColorStop(0, "#1d3557");
  grad.addColorStop(0.55, "#457b9d");
  grad.addColorStop(1, "#e63946");
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, width, height);

  // Sharp architectural lines + high-contrast text that trigger strong JPEG 8x8 ringing
  ctx.fillStyle = "#f1faee";
  ctx.fillRect(22, 24, width - 44, 64);
  ctx.fillStyle = "#1d3557";
  ctx.font = "bold 28px system-ui, sans-serif";
  ctx.fillText("ZenSR De-JPEG Test", 38, 66);

  // Diagonal stripes + fine rings
  ctx.strokeStyle = "#f4d35e";
  ctx.lineWidth = 3;
  for (let i = 0; i < 7; i++) {
    ctx.beginPath();
    ctx.moveTo(28 + i * 24, 114);
    ctx.lineTo(74 + i * 24, 224);
    ctx.stroke();
  }
  ctx.strokeStyle = "#ffffff";
  ctx.lineWidth = 2;
  for (let r = 16; r <= 52; r += 12) {
    ctx.beginPath();
    ctx.arc(298, 170, r, 0, Math.PI * 2);
    ctx.stroke();
  }
  return canvas;
}

function drawBokehOverlay(width = 480, height = 320) {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  const circles = [
    [90, 84, 72, "rgba(255, 206, 120, 0.42)"],
    [220, 140, 94, "rgba(255, 130, 168, 0.34)"],
    [372, 92, 68, "rgba(130, 195, 255, 0.42)"],
    [336, 236, 86, "rgba(255, 226, 152, 0.36)"],
    [136, 244, 62, "rgba(176, 148, 255, 0.35)"],
    [420, 196, 46, "rgba(255, 255, 255, 0.32)"],
  ];
  for (const [x, y, r, color] of circles) {
    const g = ctx.createRadialGradient(x, y, r * 0.15, x, y, r);
    g.addColorStop(0, color);
    g.addColorStop(0.75, color);
    g.addColorStop(1, "rgba(255,255,255,0)");
    ctx.fillStyle = g;
    ctx.beginPath();
    ctx.arc(x, y, r, 0, Math.PI * 2);
    ctx.fill();
  }
  return canvas;
}

function drawSharpGrid(width = 256, height = 192) {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  ctx.fillStyle = "#181d27";
  ctx.fillRect(0, 0, width, height);
  ctx.strokeStyle = "#485874";
  ctx.lineWidth = 1;
  for (let x = 0; x < width; x += 16) {
    ctx.beginPath(); ctx.moveTo(x + 0.5, 0); ctx.lineTo(x + 0.5, height); ctx.stroke();
  }
  for (let y = 0; y < height; y += 16) {
    ctx.beginPath(); ctx.moveTo(0, y + 0.5); ctx.lineTo(width, y + 0.5); ctx.stroke();
  }
  // High-frequency bars & crisp diagonal lines to evaluate resampling kernels
  for (let x = 16; x < 112; x += 4) {
    ctx.fillStyle = (x / 4) % 2 ? "#f8fafc" : "#1e293b";
    ctx.fillRect(x, 20, 2, 64);
  }
  ctx.strokeStyle = "#60a5fa";
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.arc(184, 96, 54, 0, Math.PI * 2);
  ctx.stroke();
  ctx.strokeStyle = "#fbbf24";
  ctx.beginPath();
  ctx.moveTo(16, 170); ctx.lineTo(132, 102); ctx.lineTo(240, 174);
  ctx.stroke();
  return canvas;
}

export async function makeLowQJpegFile() {
  const blob = await canvasToBlob(drawLowQJpegScene(), "image/jpeg", 0.16);
  return new File([blob], "Low-Q JPEG Test.jpg", { type: "image/jpeg" });
}

export function makeSvgFileFromTemplate(templateId = "svg-title") {
  const tpl = SVG_TEMPLATES[templateId] ?? SVG_TEMPLATES["svg-title"];
  return new File([utf8.encode(tpl.markup)], tpl.name, { type: "image/svg+xml" });
}

const GALLERY_ITEMS = [
  {
    id: "sample-photo",
    title: "Grand Prismatic Spring",
    category: "photos",
    badge: "Photo",
    note: "Public-domain NPS sample photo (1296 × 864)",
    previewSrc: "./sample.jpg",
    async buildFile() {
      const res = await fetch("./sample.jpg");
      if (!res.ok) throw new Error("Sample photo is unavailable.");
      return new File([await res.blob()], "Grand Prismatic Spring.jpg", { type: "image/jpeg" });
    },
  },
  {
    id: "color-checker",
    title: "Sunset & Color Chart",
    category: "photos",
    badge: "PNG",
    note: "Gradient sky + 12 color patches + gray ramp for ZenFilters",
    previewCanvas: () => drawColorChart(512, 340),
    async buildFile() {
      const blob = await canvasToBlob(drawColorChart(512, 340), "image/png");
      return new File([blob], "Sunset Color Chart.png", { type: "image/png" });
    },
  },
  {
    id: "lowq-jpeg-demo",
    title: "Low-Q JPEG (ZenSR Test)",
    category: "photos",
    badge: "JPEG",
    note: "Compressed at Q16 to test ZenSR dejpeg_rt24g restoration",
    previewCanvas: () => drawLowQJpegScene(384, 256),
    buildFile: makeLowQJpegFile,
  },
  {
    id: "sharp-grid",
    title: "Resampling Target Grid",
    category: "photos",
    badge: "PNG",
    note: "High-frequency bars & arcs to test zenresize kernels",
    previewCanvas: () => drawSharpGrid(256, 192),
    async buildFile() {
      const blob = await canvasToBlob(drawSharpGrid(256, 192), "image/png");
      return new File([blob], "Resampling Target Grid.png", { type: "image/png" });
    },
  },
  {
    id: "bokeh-overlay",
    title: "Warm Studio Bokeh",
    category: "overlays",
    badge: "PNG α",
    note: "Translucent light circles for compositing & masks",
    previewCanvas: () => drawBokehOverlay(480, 320),
    async buildFile() {
      const blob = await canvasToBlob(drawBokehOverlay(480, 320), "image/png");
      return new File([blob], "Warm Studio Bokeh.png", { type: "image/png" });
    },
  },
  ...Object.entries(SVG_TEMPLATES).map(([id, tpl]) => ({
    id,
    title: tpl.title,
    category: tpl.category,
    badge: tpl.badge,
    note: tpl.note,
    svgMarkup: tpl.markup,
    async buildFile() {
      return makeSvgFileFromTemplate(id);
    },
  })),
];

export function makeGallery({ openFile, addLayerFiles, hasDocument, getBusy, run }) {
  const $ = id => document.getElementById(id);
  const dialog = $("gallery-dialog");
  const grid = $("gallery-grid");
  const filterBtns = [...dialog.querySelectorAll("[data-gallery-cat]")];
  let activeCategory = "all";
  const blobUrls = [];

  function revokeUrls() {
    while (blobUrls.length) URL.revokeObjectURL(blobUrls.pop());
  }

  function close() {
    dialog.hidden = true;
    revokeUrls();
  }

  function renderGrid() {
    revokeUrls();
    grid.replaceChildren();
    const docOpen = hasDocument();
    const items = activeCategory === "all"
      ? GALLERY_ITEMS
      : GALLERY_ITEMS.filter(item => item.category === activeCategory);
    for (const item of items) {
      const card = document.createElement("article");
      card.className = "gallery-card";
      card.dataset.galleryId = item.id;

      const thumbWrap = document.createElement("div");
      thumbWrap.className = "gallery-thumb";
      if (item.previewSrc) {
        const img = document.createElement("img");
        img.src = item.previewSrc;
        img.alt = item.title;
        thumbWrap.append(img);
      } else if (item.svgMarkup) {
        const url = URL.createObjectURL(new Blob([item.svgMarkup], { type: "image/svg+xml" }));
        blobUrls.push(url);
        const img = document.createElement("img");
        img.src = url;
        img.alt = item.title;
        thumbWrap.append(img);
      } else if (item.previewCanvas) {
        thumbWrap.append(item.previewCanvas());
      }

      const meta = document.createElement("div");
      meta.className = "gallery-meta";
      const header = document.createElement("div");
      header.className = "gallery-card-head";
      const titleEl = document.createElement("strong");
      titleEl.textContent = item.title;
      const badge = document.createElement("span");
      badge.className = "badge";
      badge.textContent = item.badge;
      header.append(titleEl, badge);
      const note = document.createElement("p");
      note.textContent = item.note;
      meta.append(header, note);

      const actions = document.createElement("div");
      actions.className = "gallery-card-actions";
      const addBtn = document.createElement("button");
      addBtn.type = "button";
      addBtn.className = "primary";
      addBtn.textContent = docOpen ? "Add as layer" : "Open";
      addBtn.dataset.action = "add";
      addBtn.onclick = () => {
        if (getBusy()) return;
        close();
        run(`Loading ${item.title}…`, async () => {
          const file = await item.buildFile();
          if (hasDocument()) await addLayerFiles([file]);
          else await openFile(file);
        });
      };
      actions.append(addBtn);

      if (docOpen) {
        const openBtn = document.createElement("button");
        openBtn.type = "button";
        openBtn.textContent = "Open new";
        openBtn.dataset.action = "open";
        openBtn.onclick = () => {
          if (getBusy()) return;
          close();
          run(`Opening ${item.title}…`, async () => {
            const file = await item.buildFile();
            await openFile(file);
          });
        };
        actions.append(openBtn);
      }

      card.append(thumbWrap, meta, actions);
      grid.append(card);
    }
  }

  for (const btn of filterBtns) {
    btn.addEventListener("click", () => {
      activeCategory = btn.dataset.galleryCat;
      for (const b of filterBtns) b.setAttribute("aria-pressed", String(b === btn));
      renderGrid();
    });
  }

  $("gallery-close").onclick = close;
  dialog.addEventListener("click", event => { if (event.target === dialog) close(); });
  window.addEventListener("keydown", event => {
    if (event.key === "Escape" && !dialog.hidden) close();
  });

  return {
    open(category = "all") {
      if (getBusy()) return;
      activeCategory = category;
      for (const b of filterBtns) b.setAttribute("aria-pressed", String(b.dataset.galleryCat === activeCategory));
      renderGrid();
      dialog.hidden = false;
      $("gallery-close").focus();
    },
    close,
  };
}
