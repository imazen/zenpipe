import { codec } from "./codec.mjs";
import { appLog } from "./console-log.mjs";
import { jxlBoxes, writeImage, writeProject } from "./project.mjs";
import {
  clearDecodeRuns,
  deleteDecodeRun,
  getDecodeRun,
  listDecodeRuns,
  saveDecodeRun,
  updateDecodeRunNote,
} from "./storage.mjs";

const ALL_DECODERS = ["zenjxl-decoder", "jxl-rs", "jxl-oxide", "libjxl"];
const SHORT_LABELS = {
  "zenjxl-decoder": "zenjxl",
  "jxl-rs": "jxl-rs",
  "jxl-oxide": "oxide",
  "libjxl": "libjxl",
  "editor-canvas": "canvas",
};

function formatBytes(n) {
  if (!Number.isFinite(n) || n < 0) return "0 B";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}

function formatTime(ms) {
  if (!ms) return "";
  const d = new Date(ms);
  return `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })} ${d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" })}`;
}

function bytesToBase64(bytes) {
  let bin = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    bin += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(bin);
}

function sanitizeFileSlug(name) {
  return String(name || "check").replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 64) || "check";
}

function detectJxlFeatures(bytes, results, context = {}) {
  const feats = new Set();
  if (Array.isArray(context.extraFeatures)) {
    for (const f of context.extraFeatures) feats.add(f);
  }
  // Container box inspection
  const isContainer = bytes.length >= 12 &&
    bytes[0] === 0 && bytes[1] === 0 && bytes[2] === 0 && bytes[3] === 0x0c &&
    bytes[4] === 0x4a && bytes[5] === 0x58 && bytes[6] === 0x4c && bytes[7] === 0x20;
  feats.add(isContainer ? "container" : "bare-codestream");
  if (isContainer) {
    try {
      const boxes = jxlBoxes(bytes);
      for (const b of boxes) {
        if (b.type === "zned" || b.type === "znas" || b.type === "znmk" || b.type === "zncm") {
          feats.add("private-boxes");
        } else if (b.type === "jxll") {
          if (b.data.length >= 1 && b.data[0] >= 10) feats.add("level-10");
        } else if (b.type === "jbrd") {
          feats.add("jpeg-reconstruct");
        } else if (b.type === "Exif") {
          feats.add("exif");
        } else if (b.type === "xml ") {
          feats.add("xmp");
        } else if (b.type === "brob") {
          feats.add("brob-compressed-box");
        }
      }
    } catch {
      feats.add("nonstandard-container");
    }
  }

  const coding = context.coding ?? null;
  if (coding) {
    if (coding.mode === "vardct") {
      feats.add("vardct");
      if (coding.progressive && coding.progressive !== "single") feats.add("progressive");
      if (coding.progressiveDc > 0) feats.add("lf-frame");
      if (coding.noiseMode && coding.noiseMode !== "off") feats.add("noise");
      if ((coding.resampling ?? 1) > 1) feats.add(`resampling-${coding.resampling}x`);
      if (coding.patches) feats.add("patches");
      if (coding.autoSplines) feats.add("auto-splines");
      if ((coding.alphaDistance ?? 0) > 0) feats.add("lossy-alpha");
    } else {
      feats.add(coding.lossyPalette ? "modular-lossy-palette" : "modular-lossless");
      if (coding.squeeze) feats.add("squeeze");
      if (coding.forceRct && coding.forceRct !== "auto") feats.add(`rct-${coding.forceRct}`);
      if (coding.patches) feats.add("patches");
    }
  }
  if (context.hasSplines) feats.add("splines");
  if (context.hasTimeline) feats.add("animation");
  if (context.hasBlending) feats.add("frame-blending");
  if (context.hasRefOnly) feats.add("ref-only-frame");
  if (context.isCmyk) feats.add("cmyk");

  // Augment from decoder metadata
  for (const r of results) {
    if (!r.ok) continue;
    if (r.frameCount > 1) feats.add("animation");
    if (r.hasAlpha) feats.add("alpha");
    if (r.hasIcc) feats.add("icc-profile");
    if (r.isCmyk) feats.add("cmyk");
    if (r.orientation && r.orientation !== 1) feats.add(`orientation-${r.orientation}`);
    if ((r.width & 1) || (r.height & 1)) feats.add("odd-dimensions");
    if (r.level5Accepted === false) feats.add("level-10");
    if (r.extraChannels > 1) feats.add("multi-extra-channels");
  }
  return [...feats].sort();
}

function classifyCheckRun(results, pairwise) {
  const okResults = results.filter(r => r.ok);
  const failResults = results.filter(r => !r.ok);
  const decoderPairs = pairwise.filter(p => p.a !== "editor-canvas" && p.b !== "editor-canvas");
  const maxDecoderDelta = decoderPairs.reduce((m, p) => Math.max(m, p.maxAbsDelta ?? 0), 0);
  const anyMetaMismatch = decoderPairs.some(p => p.sizeMismatch || p.frameCountMismatch || p.durationMismatch);

  let verdict = "all-exact";
  if (okResults.length === 0 && failResults.length > 0) {
    verdict = "all-failed";
  } else if (failResults.length > 0) {
    verdict = "partial-failure";
  } else if (anyMetaMismatch || maxDecoderDelta > 2) {
    verdict = "pixel-disagreement";
  } else if (maxDecoderDelta > 0) {
    verdict = "all-agree";
  }

  const suspectedBugs = [];
  if (verdict === "all-failed") {
    suspectedBugs.push({
      target: "encoder/bitstream",
      severity: "error",
      summary: `All ${failResults.length} decoders rejected the bitstream`,
      details: failResults.map(r => `${r.decoder}: ${r.error}`).join(" | "),
    });
  } else if (failResults.length > 0) {
    const okNames = okResults.map(r => r.decoder).join(", ");
    for (const f of failResults) {
      suspectedBugs.push({
        target: f.decoder,
        severity: "error",
        summary: `${f.decoder} failed to decode while ${okNames} succeeded`,
        details: f.error || "Unknown decode error",
      });
    }
  }

  if (anyMetaMismatch || maxDecoderDelta > 2) {
    // Check if there is a clear outlier among >= 3 passing decoders using pixelSha256 / <=2 agreement clusters
    const clusterScore = new Map();
    for (const r of okResults) clusterScore.set(r.decoder, 0);
    for (const p of decoderPairs) {
      if (!p.sizeMismatch && !p.frameCountMismatch && !p.durationMismatch && p.maxAbsDelta <= 2) {
        clusterScore.set(p.a, (clusterScore.get(p.a) ?? 0) + 1);
        clusterScore.set(p.b, (clusterScore.get(p.b) ?? 0) + 1);
      }
    }
    const maxScore = Math.max(0, ...clusterScore.values());
    const outliers = okResults.filter(r => maxScore >= 1 && (clusterScore.get(r.decoder) ?? 0) === 0);
    for (const p of decoderPairs) {
      if (p.sizeMismatch || p.frameCountMismatch || p.durationMismatch || p.maxAbsDelta > 2) {
        const reasons = [];
        if (p.sizeMismatch) reasons.push("dimensions differ");
        if (p.frameCountMismatch) reasons.push("frame count differs");
        if (p.durationMismatch) reasons.push("frame durations differ");
        if (p.maxAbsDelta > 2) reasons.push(`max Δ=${p.maxAbsDelta} (${p.diffPercent}% px, PSNR=${p.psnrDb ?? "∞"} dB)`);
        const suspect = outliers.length === 1 && (outliers[0].decoder === p.a || outliers[0].decoder === p.b)
          ? outliers[0].decoder
          : `${p.a} vs ${p.b}`;
        suspectedBugs.push({
          target: suspect,
          severity: p.sizeMismatch || p.frameCountMismatch || p.maxAbsDelta >= 16 ? "error" : "warn",
          summary: `Disagreement (${p.a} vs ${p.b}): ${reasons.join(", ")}`,
          details: `maxRgb=${p.maxRgbDelta}, maxAlpha=${p.maxAlphaDelta}, meanAbs=${p.meanAbsDelta}, diffOver1=${p.diffOver1Pixels}`,
        });
      }
    }
  }

  return { verdict, maxDecoderDelta, anyMetaMismatch, suspectedBugs };
}

export function buildCompatibilitySurface(runs) {
  const byFeature = new Map();
  for (const run of runs) {
    const features = Array.isArray(run.features) && run.features.length ? run.features : ["(unclassified)"];
    const decoderPairs = (run.pairwise ?? []).filter(p => p.a !== "editor-canvas" && p.b !== "editor-canvas");
    for (const feat of features) {
      if (!byFeature.has(feat)) {
        const entry = { feature: feat, runs: 0, decoders: {} };
        for (const d of ALL_DECODERS) {
          entry.decoders[d] = { total: 0, ok: 0, exact: 0, rounding: 0, diverged: 0, failed: 0 };
        }
        byFeature.set(feat, entry);
      }
      const row = byFeature.get(feat);
      row.runs++;
      for (const r of (run.results ?? [])) {
        const cell = row.decoders[r.decoder];
        if (!cell) continue;
        cell.total++;
        if (!r.ok) {
          cell.failed++;
        } else {
          cell.ok++;
          const pairs = decoderPairs.filter(p => p.a === r.decoder || p.b === r.decoder);
          const worstDelta = pairs.reduce((m, p) => Math.max(m, p.maxAbsDelta ?? 0), 0);
          const metaDiff = pairs.some(p => p.sizeMismatch || p.frameCountMismatch || p.durationMismatch);
          if (metaDiff || worstDelta > 2) cell.diverged++;
          else if (worstDelta > 0) cell.rounding++;
          else cell.exact++;
        }
      }
    }
  }
  return [...byFeature.values()].sort((a, b) => a.feature.localeCompare(b.feature));
}

function verdictMeta(verdict) {
  switch (verdict) {
    case "all-exact": return { label: "ALL EXACT", cls: "dc-badge-ok" };
    case "all-agree": return { label: "AGREE (≤2 Δ)", cls: "dc-badge-agree" };
    case "pixel-disagreement": return { label: "DISAGREEMENT", cls: "dc-badge-warn" };
    case "partial-failure": return { label: "PARTIAL FAIL", cls: "dc-badge-err" };
    case "all-failed": return { label: "ALL FAILED", cls: "dc-badge-err" };
    default: return { label: String(verdict || "UNKNOWN"), cls: "dc-badge-muted" };
  }
}

function makeSweepTestPatch(state, renderFlattenedCanvas) {
  return (async () => {
    const w = 96;
    const h = 64;
    const c = document.createElement("canvas");
    c.width = w;
    c.height = h;
    const ctx = c.getContext("2d");
    if (state && renderFlattenedCanvas) {
      const { canvas } = await renderFlattenedCanvas(state, 0);
      ctx.drawImage(canvas, 0, 0, w, h);
    } else {
      const grad = ctx.createLinearGradient(0, 0, w, h);
      grad.addColorStop(0, "#1c2d4a");
      grad.addColorStop(0.5, "#d87d3a");
      grad.addColorStop(1, "#3eb8a0");
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, w, h);
      ctx.fillStyle = "rgba(255,255,255,0.85)";
      ctx.fillRect(10, 10, 28, 18);
      ctx.fillStyle = "#e03e52";
      ctx.beginPath();
      ctx.arc(68, 36, 16, 0, Math.PI * 2);
      ctx.fill();
    }
    const baseData = new Uint8Array(ctx.getImageData(0, 0, w, h).data);
    // Create a semi-transparent variant to exercise alpha coding too
    const alphaData = baseData.slice();
    for (let y = 0; y < 14; y++) {
      for (let x = 0; x < 14; x++) {
        alphaData[(y * w + x) * 4 + 3] = Math.round(((x + y) / 26) * 255);
      }
    }
    return { width: w, height: h, opaque: baseData, alpha: alphaData };
  })();
}

export function makeDecodeLab({
  getState,
  getTitle,
  getSelectedLayer,
  getBusy,
  buildPlainJxlBytes,
  buildProjectJxlBytes,
  renderFlattenedCanvas,
  download,
  newId,
  status,
  showDecodeTab,
}) {
  const $ = id => document.getElementById(id);

  let latestRun = null;
  let latestPixels = {};
  let latestRef = null;
  let savedRuns = [];
  let historyFilter = "all";
  let running = false;
  let catalogCache = null;

  function selectedDecoders() {
    const out = [];
    for (const id of ALL_DECODERS) {
      const cb = $(`dc-use-${id}`);
      if (!cb || cb.checked) out.push(id);
    }
    return out.length ? out : [...ALL_DECODERS];
  }

  function setBusyUi(isBusy, msg = "") {
    running = isBusy;
    for (const id of ["dc-run-btn", "dc-sweep-btn", "dc-upload-btn", "check-decoding"]) {
      const el = $(id);
      if (el) el.disabled = isBusy || (id === "check-decoding" && !getState());
    }
    const prog = $("dc-progress");
    if (prog) {
      prog.hidden = !msg;
      prog.textContent = msg;
    }
  }

  function updateButtons(hasDoc, editorBusy) {
    const checkBtn = $("check-decoding");
    if (checkBtn) checkBtn.disabled = !hasDoc || editorBusy || running;
    const runBtn = $("dc-run-btn");
    if (runBtn) runBtn.disabled = editorBusy || running;
    const sweepBtn = $("dc-sweep-btn");
    if (sweepBtn) sweepBtn.disabled = editorBusy || running;
    const uploadBtn = $("dc-upload-btn");
    if (uploadBtn) uploadBtn.disabled = editorBusy || running;
    const rawOpt = $("dc-target")?.querySelector('option[value="raw-source"]');
    if (rawOpt) {
      const layer = getSelectedLayer?.();
      const state = getState?.();
      const hasRawJxl = Boolean(state && layer?.mime === "image/jxl" && state.assets?.get(layer.id)?.source);
      rawOpt.disabled = !hasRawJxl;
    }
  }

  function updateTabBadge() {
    const bugCount = savedRuns.filter(r =>
      r.verdict === "pixel-disagreement" || r.verdict === "partial-failure" || r.verdict === "all-failed").length;
    const tab = $("decode-tab");
    if (tab) {
      tab.textContent = bugCount > 0 ? `Decode Lab (${bugCount}!)` : savedRuns.length ? `Decode Lab (${savedRuns.length})` : "Decode Lab";
      tab.classList.toggle("has-errors", bugCount > 0);
    }
  }

  function renderDiffCanvas() {
    const canvas = $("dc-diff-canvas");
    const info = $("dc-diff-info");
    if (!canvas) return;
    const aKey = $("dc-diff-a")?.value;
    const bKey = $("dc-diff-b")?.value;
    const mode = $("dc-diff-mode")?.value || "x16";

    const getBuf = key => {
      if (key === "editor-canvas") return latestRef;
      return latestPixels[key] ?? null;
    };
    const bufA = getBuf(aKey);
    const bufB = getBuf(bKey);

    if (!bufA || !bufB) {
      canvas.hidden = true;
      if (info) info.textContent = "Run a check with ≥ 2 passing decoders to compare decoded pixels.";
      return;
    }
    if (bufA.width !== bufB.width || bufA.height !== bufB.height) {
      canvas.hidden = true;
      if (info) info.textContent = `Dimension mismatch: ${aKey} is ${bufA.width}×${bufA.height} vs ${bKey} is ${bufB.width}×${bufB.height}.`;
      return;
    }
    const { width, height } = bufA;
    canvas.width = width;
    canvas.height = height;
    canvas.hidden = false;
    const ctx = canvas.getContext("2d");
    const img = ctx.createImageData(width, height);
    const dst = img.data;
    const pa = bufA.pixels;
    const pb = bufB.pixels;

    let maxD = 0;
    let diffPx = 0;
    for (let i = 0; i < pa.byteLength; i += 4) {
      if (mode === "view-a") {
        dst[i] = pa[i]; dst[i + 1] = pa[i + 1]; dst[i + 2] = pa[i + 2]; dst[i + 3] = pa[i + 3];
        continue;
      }
      if (mode === "view-b") {
        dst[i] = pb[i]; dst[i + 1] = pb[i + 1]; dst[i + 2] = pb[i + 2]; dst[i + 3] = pb[i + 3];
        continue;
      }
      const dr = Math.abs(pa[i] - pb[i]);
      const dg = Math.abs(pa[i + 1] - pb[i + 1]);
      const db = Math.abs(pa[i + 2] - pb[i + 2]);
      const da = Math.abs(pa[i + 3] - pb[i + 3]);
      const m = Math.max(dr, dg, db, da);
      if (m > maxD) maxD = m;
      if (m > 0) diffPx++;
      if (mode === "mask0") {
        const hit = m > 0;
        dst[i] = hit ? 255 : 16;
        dst[i + 1] = hit ? 68 : 22;
        dst[i + 2] = hit ? 96 : 32;
        dst[i + 3] = 255;
      } else if (mode === "mask2") {
        const hit = m > 2;
        dst[i] = hit ? 255 : (m > 0 ? 110 : 16);
        dst[i + 1] = hit ? 50 : (m > 0 ? 140 : 22);
        dst[i + 2] = hit ? 50 : (m > 0 ? 60 : 32);
        dst[i + 3] = 255;
      } else {
        const gain = mode === "x64" ? 64 : mode === "x16" ? 16 : 1;
        dst[i] = Math.min(255, dr * gain + (da > 0 ? Math.min(180, da * gain) : 0));
        dst[i + 1] = Math.min(255, dg * gain);
        dst[i + 2] = Math.min(255, db * gain + (da > 0 ? Math.min(255, da * gain) : 0));
        dst[i + 3] = 255;
      }
    }
    ctx.putImageData(img, 0, 0);
    if (info) {
      info.textContent = mode.startsWith("view-")
        ? `Showing decoded output of ${mode === "view-a" ? aKey : bKey} (${width}×${height}).`
        : `${SHORT_LABELS[aKey] ?? aKey} vs ${SHORT_LABELS[bKey] ?? bKey}: max Δ=${maxD}, differing pixels=${diffPx.toLocaleString()} / ${(width * height).toLocaleString()}.`;
    }
  }

  function populateDiffSelectors() {
    const selA = $("dc-diff-a");
    const selB = $("dc-diff-b");
    if (!selA || !selB) return;
    const keys = Object.keys(latestPixels);
    if (latestRef) keys.push("editor-canvas");
    const prevA = selA.value;
    const prevB = selB.value;
    selA.replaceChildren();
    selB.replaceChildren();
    for (const k of keys) {
      const o1 = document.createElement("option");
      o1.value = k;
      o1.textContent = SHORT_LABELS[k] ?? k;
      selA.append(o1);
      const o2 = document.createElement("option");
      o2.value = k;
      o2.textContent = SHORT_LABELS[k] ?? k;
      selB.append(o2);
    }
    if (keys.includes(prevA)) selA.value = prevA;
    else if (keys.includes("libjxl")) selA.value = "libjxl";
    else if (keys[0]) selA.value = keys[0];

    if (keys.includes(prevB) && prevB !== selA.value) selB.value = prevB;
    else if (keys.includes("zenjxl-decoder") && selA.value !== "zenjxl-decoder") selB.value = "zenjxl-decoder";
    else if (keys[1]) selB.value = keys[1];
    renderDiffCanvas();
  }

  function renderLatestResult() {
    const box = $("dc-latest");
    if (!box) return;
    box.replaceChildren();
    if (!latestRun) {
      const empty = document.createElement("p");
      empty.className = "muted-note";
      empty.textContent = "Click “Check decoding” to run zenjxl-decoder, jxl-rs, jxl-oxide, and libjxl in WebAssembly and compare decoded pixels & metadata.";
      box.append(empty);
      populateDiffSelectors();
      return;
    }
    const vm = verdictMeta(latestRun.verdict);
    const banner = document.createElement("div");
    banner.className = `dc-verdict ${vm.cls}`;
    const vTitle = document.createElement("strong");
    vTitle.textContent = `${vm.label} · ${latestRun.label}`;
    const vSub = document.createElement("span");
    vSub.textContent = `${formatBytes(latestRun.jxlByteLength)} · SHA-256 ${latestRun.jxlSha256?.slice(0, 12) || "—"} · max decoder Δ=${latestRun.maxDecoderDelta}`;
    banner.append(vTitle, vSub);
    box.append(banner);

    // Feature chips
    if (latestRun.features?.length) {
      const chips = document.createElement("div");
      chips.className = "dc-chip-row";
      for (const f of latestRun.features) {
        const chip = document.createElement("span");
        chip.className = "dc-chip";
        chip.textContent = f;
        chips.append(chip);
      }
      box.append(chips);
    }

    // Suspected bug callouts
    if (latestRun.suspectedBugs?.length) {
      const bugsBox = document.createElement("div");
      bugsBox.className = "dc-bugs-box";
      const h = document.createElement("strong");
      h.textContent = `Suspected Bug Findings (${latestRun.suspectedBugs.length})`;
      bugsBox.append(h);
      for (const b of latestRun.suspectedBugs) {
        const item = document.createElement("div");
        item.className = `dc-bug-item dc-sev-${b.severity}`;
        const top = document.createElement("div");
        top.textContent = `[${b.target}] ${b.summary}`;
        const det = document.createElement("code");
        det.textContent = b.details;
        item.append(top, det);
        bugsBox.append(item);
      }
      box.append(bugsBox);
    }

    // 4-decoder cards
    const grid = document.createElement("div");
    grid.className = "dc-cards";
    for (const r of latestRun.results) {
      const card = document.createElement("div");
      card.className = `dc-card ${r.ok ? "dc-card-ok" : "dc-card-err"}`;
      card.dataset.decoder = r.decoder;
      const head = document.createElement("div");
      head.className = "dc-card-head";
      const name = document.createElement("strong");
      name.textContent = r.decoder;
      const pill = document.createElement("span");
      pill.className = `dc-pill ${r.ok ? "dc-pill-ok" : "dc-pill-err"}`;
      pill.textContent = r.ok ? `OK · ${r.elapsedMs} ms` : `FAIL · ${r.elapsedMs} ms`;
      head.append(name, pill);

      const ver = document.createElement("div");
      ver.className = "dc-card-ver";
      ver.textContent = r.version;

      card.append(head, ver);
      if (r.ok) {
        const meta = document.createElement("div");
        meta.className = "dc-card-meta";
        const frameStr = r.frameCount > 1
          ? `${r.frameCount}f [${r.frameDurationsMs?.join(",") ?? ""}ms${r.loopCount != null ? ` loop=${r.loopCount}` : ""}]`
          : "1 frame";
        const l5 = r.level5Accepted === true ? "L5 OK" : r.level5Accepted === false ? "L10 only" : "";
        meta.textContent = `${r.width}×${r.height} · ${frameStr} · CMS: ${r.cmsUsed}${l5 ? ` · ${l5}` : ""}`;
        const hash = document.createElement("div");
        hash.className = "dc-card-hash";
        hash.textContent = `px sha256: ${r.pixelSha256 ? r.pixelSha256.slice(0, 16) : "—"}`;
        card.append(meta, hash);
      } else {
        const err = document.createElement("pre");
        err.className = "dc-card-error";
        err.textContent = r.error || "Unknown decode error";
        card.append(err);
      }
      if (r.warnings?.length) {
        const warn = document.createElement("div");
        warn.className = "dc-card-warn";
        warn.textContent = `⚠ ${r.warnings.join(" · ")}`;
        card.append(warn);
      }
      grid.append(card);
    }
    box.append(grid);

    // Pairwise matrix table
    if (latestRun.pairwise?.length) {
      const tblWrap = document.createElement("div");
      tblWrap.className = "dc-table-wrap";
      const tbl = document.createElement("table");
      tbl.className = "dc-table";
      const thead = document.createElement("thead");
      const hr = document.createElement("tr");
      for (const col of ["Pair", "Max Δ", "Diff %", "PSNR", "Status"]) {
        const th = document.createElement("th");
        th.textContent = col;
        hr.append(th);
      }
      thead.append(hr);
      const tbody = document.createElement("tbody");
      for (const p of latestRun.pairwise) {
        const tr = document.createElement("tr");
        const pairTd = document.createElement("td");
        pairTd.textContent = `${SHORT_LABELS[p.a] ?? p.a} ↔ ${SHORT_LABELS[p.b] ?? p.b}`;
        const maxTd = document.createElement("td");
        maxTd.textContent = p.sizeMismatch ? "SIZE!" : String(p.maxAbsDelta);
        const pctTd = document.createElement("td");
        pctTd.textContent = `${p.diffPercent}%`;
        const psnrTd = document.createElement("td");
        psnrTd.textContent = p.psnrDb == null ? "∞" : `${p.psnrDb}`;
        const stTd = document.createElement("td");
        stTd.className = p.exactMatch ? "dc-cell-ok"
          : (!p.sizeMismatch && !p.frameCountMismatch && !p.durationMismatch && p.maxAbsDelta <= 2) ? "dc-cell-agree"
          : "dc-cell-err";
        stTd.textContent = p.exactMatch ? "EXACT"
          : p.sizeMismatch ? "SIZE"
          : p.frameCountMismatch ? "FRAMES"
          : p.durationMismatch ? "TIMING"
          : p.maxAbsDelta <= 2 ? "≤2 Δ"
          : "DIFF!";
        tr.append(pairTd, maxTd, pctTd, psnrTd, stTd);
        tbody.append(tr);
      }
      tbl.append(thead, tbody);
      tblWrap.append(tbl);
      box.append(tblWrap);
    }

    populateDiffSelectors();
  }

  function renderSurfaceMatrix() {
    const mount = $("dc-surface");
    if (!mount) return;
    mount.replaceChildren();
    const surface = buildCompatibilitySurface(savedRuns);
    if (!surface.length) {
      const p = document.createElement("p");
      p.className = "muted-note";
      p.textContent = "Run checks or a compatibility sweep to build the live Feature × Decoder compatibility surface.";
      mount.append(p);
      return;
    }
    const wrap = document.createElement("div");
    wrap.className = "dc-table-wrap";
    const tbl = document.createElement("table");
    tbl.className = "dc-table dc-surface-table";
    const thead = document.createElement("thead");
    const hr = document.createElement("tr");
    for (const label of ["Feature", "zenjxl", "jxl-rs", "oxide", "libjxl"]) {
      const th = document.createElement("th");
      th.textContent = label;
      hr.append(th);
    }
    thead.append(hr);
    const tbody = document.createElement("tbody");
    for (const row of surface) {
      const tr = document.createElement("tr");
      const ftd = document.createElement("td");
      ftd.textContent = row.feature;
      tr.append(ftd);
      for (const d of ALL_DECODERS) {
        const c = row.decoders[d];
        const td = document.createElement("td");
        if (!c || c.total === 0) {
          td.textContent = "—";
          td.className = "dc-cell-muted";
        } else if (c.failed > 0) {
          td.textContent = `${c.ok}/${c.total} ✕`;
          td.className = "dc-cell-err";
          td.title = `${c.failed} failed, ${c.diverged} diverged, ${c.exact} exact`;
        } else if (c.diverged > 0) {
          td.textContent = `${c.ok}/${c.total} Δ`;
          td.className = "dc-cell-warn";
          td.title = `${c.diverged} pixel/metadata disagreements, ${c.exact} exact`;
        } else if (c.rounding > 0) {
          td.textContent = `${c.ok}/${c.total} ≈`;
          td.className = "dc-cell-agree";
          td.title = `${c.rounding} within ±2 rounding, ${c.exact} exact`;
        } else {
          td.textContent = `${c.ok}/${c.total} ✓`;
          td.className = "dc-cell-ok";
          td.title = `All ${c.exact} runs bit-exact`;
        }
        tr.append(td);
      }
      tbody.append(tr);
    }
    tbl.append(thead, tbody);
    wrap.append(tbl);
    mount.append(wrap);
  }

  function renderHistoryList() {
    const list = $("dc-history-list");
    const summary = $("dc-history-summary");
    if (!list) return;
    list.replaceChildren();
    const filtered = savedRuns.filter(r => {
      if (historyFilter === "bugs") return r.verdict !== "all-exact" && r.verdict !== "all-agree";
      if (historyFilter === "fails") return r.verdict === "partial-failure" || r.verdict === "all-failed";
      if (historyFilter === "exact") return r.verdict === "all-exact";
      return true;
    });
    const bugCount = savedRuns.filter(r => r.verdict !== "all-exact" && r.verdict !== "all-agree").length;
    if (summary) {
      summary.textContent = `${filtered.length} shown · ${savedRuns.length} saved in IndexedDB · ${bugCount} bug/diff finding${bugCount === 1 ? "" : "s"}`;
    }
    if (!filtered.length) {
      const li = document.createElement("li");
      li.className = "muted-note";
      li.textContent = savedRuns.length ? "No runs match this filter." : "No saved decode runs yet.";
      list.append(li);
      return;
    }
    for (const run of filtered) {
      const li = document.createElement("li");
      li.className = "dc-run-item";
      li.dataset.runId = run.runId;
      li.dataset.verdict = run.verdict;
      const vm = verdictMeta(run.verdict);

      const top = document.createElement("div");
      top.className = "dc-run-top";
      const badge = document.createElement("span");
      badge.className = `dc-pill ${vm.cls}`;
      badge.textContent = vm.label;
      const label = document.createElement("strong");
      label.textContent = run.label;
      const ts = document.createElement("span");
      ts.className = "dc-run-ts";
      ts.textContent = formatTime(run.createdAt);
      top.append(badge, label, ts);

      const mini = document.createElement("div");
      mini.className = "dc-run-mini";
      for (const r of (run.results ?? [])) {
        const s = document.createElement("span");
        s.className = `dc-mini ${r.ok ? "dc-mini-ok" : "dc-mini-err"}`;
        s.textContent = `${SHORT_LABELS[r.decoder] ?? r.decoder}:${r.ok ? `${r.elapsedMs}ms` : "FAIL"}`;
        if (!r.ok && r.error) s.title = r.error;
        mini.append(s);
      }
      const meta = document.createElement("span");
      meta.className = "dc-run-meta";
      meta.textContent = `${formatBytes(run.jxlByteLength)} · Δ=${run.maxDecoderDelta ?? 0}`;
      mini.append(meta);

      const noteInput = document.createElement("input");
      noteInput.type = "text";
      noteInput.className = "dc-note-input";
      noteInput.placeholder = "Add bug note or issue ID…";
      noteInput.value = run.note ?? "";
      noteInput.addEventListener("change", async () => {
        await updateDecodeRunNote(run.runId, noteInput.value.trim());
        run.note = noteInput.value.trim();
        appLog.storage(`Updated note on decode run "${run.label}"`);
      });

      const actions = document.createElement("div");
      actions.className = "dc-run-actions";
      const viewBtn = document.createElement("button");
      viewBtn.type = "button";
      viewBtn.className = "pill-btn";
      viewBtn.textContent = "Inspect";
      viewBtn.onclick = () => {
        latestRun = run;
        latestPixels = {};
        latestRef = null;
        renderLatestResult();
      };
      actions.append(viewBtn);

      if (run.hasStoredBytes) {
        const dlBtn = document.createElement("button");
        dlBtn.type = "button";
        dlBtn.className = "pill-btn";
        dlBtn.textContent = "Download .jxl";
        dlBtn.onclick = async () => {
          const full = await getDecodeRun(run.runId);
          if (!full?.jxlBytes) return;
          const slug = sanitizeFileSlug(run.label);
          download(new Blob([full.jxlBytes], { type: "image/jxl" }), `${slug}-${run.jxlSha256?.slice(0, 8) || "repro"}.jxl`);
        };
        actions.append(dlBtn);
      }

      const delBtn = document.createElement("button");
      delBtn.type = "button";
      delBtn.className = "pill-btn";
      delBtn.textContent = "Delete";
      delBtn.onclick = async () => {
        await deleteDecodeRun(run.runId);
        await refreshSavedRuns();
      };
      actions.append(delBtn);

      li.append(top, mini, noteInput, actions);
      list.append(li);
    }
  }

  async function refreshSavedRuns() {
    try {
      const all = await listDecodeRuns();
      // Keep lightweight metadata in memory (jxlBytes loaded on demand for download/export)
      savedRuns = all.map(r => {
        const hasStoredBytes = Boolean(r.jxlBytes && r.jxlBytes.byteLength > 0);
        const { jxlBytes: _omit, ...rest } = r;
        return { ...rest, hasStoredBytes };
      });
    } catch (error) {
      appLog.warn("decode-lab", `Could not read decode runs from IndexedDB: ${error.message}`);
      savedRuns = [];
    }
    updateTabBadge();
    renderSurfaceMatrix();
    renderHistoryList();
  }

  async function checkBytes(jxlBytes, {
    label = "Uploaded JXL",
    sourceKind = "external",
    coding = null,
    extraFeatures = [],
    hasSplines = false,
    hasTimeline = false,
    hasBlending = false,
    hasRefOnly = false,
    isCmyk = false,
    referencePixels = null,
    referenceWidth = 0,
    referenceHeight = 0,
    decoders = selectedDecoders(),
    probeLevel5 = $("dc-probe-l5")?.checked ?? true,
    keepPixels = true,
    saveToStorage = true,
    storeBytesOverride = null,
  } = {}) {
    const bytes = jxlBytes instanceof Uint8Array ? jxlBytes : new Uint8Array(jxlBytes);
    const workerReq = {
      bytes: bytes.slice(),
      decoders,
      options: { probeLevel5, applyExternalCmsForJxlRs: true },
      returnPixels: keepPixels,
    };
    const transfers = [workerReq.bytes.buffer];
    if (referencePixels && referenceWidth && referenceHeight) {
      const refCopy = referencePixels.slice();
      workerReq.referencePixels = refCopy;
      workerReq.referenceWidth = referenceWidth;
      workerReq.referenceHeight = referenceHeight;
      transfers.push(refCopy.buffer);
    }
    const res = await codec("checkDecoders", workerReq, transfers);
    const features = detectJxlFeatures(bytes, res.results, {
      coding, extraFeatures, hasSplines, hasTimeline, hasBlending, hasRefOnly, isCmyk,
    });
    const { verdict, maxDecoderDelta, anyMetaMismatch, suspectedBugs } = classifyCheckRun(res.results, res.pairwise);

    const runId = newId();
    const record = {
      runId,
      createdAt: Date.now(),
      label,
      sourceKind,
      jxlSha256: res.jxlSha256,
      jxlByteLength: res.jxlByteLength,
      features,
      coding,
      results: res.results,
      pairwise: res.pairwise,
      verdict,
      maxDecoderDelta,
      anyMetaMismatch,
      suspectedBugs,
      note: "",
    };

    const keepAll = $("dc-store-bytes")?.checked ?? false;
    const isInteresting = verdict !== "all-exact" && verdict !== "all-agree";
    const shouldStoreBytes = storeBytesOverride !== null ? storeBytesOverride : (keepAll || isInteresting);
    if (saveToStorage) {
      await saveDecodeRun({
        ...record,
        jxlBytes: shouldStoreBytes ? bytes.slice() : null,
      });
    }

    if (keepPixels) {
      latestRun = { ...record, hasStoredBytes: shouldStoreBytes };
      latestPixels = res.pixelsByDecoder ?? {};
      latestRef = referencePixels && referenceWidth && referenceHeight
        ? { width: referenceWidth, height: referenceHeight, pixels: referencePixels }
        : null;
      renderLatestResult();
    }

    const okCount = res.results.filter(r => r.ok).length;
    const logMsg = `Decode check "${label}": ${verdict} (${okCount}/${res.results.length} OK, max Δ=${maxDecoderDelta}, ${formatBytes(bytes.byteLength)})`;
    if (verdict === "all-exact" || verdict === "all-agree") {
      appLog.info("decode-lab", logMsg);
    } else if (verdict === "pixel-disagreement") {
      appLog.warn("decode-lab", logMsg, suspectedBugs.map(b => b.summary).join("\n"));
    } else {
      appLog.error("decode-lab", logMsg, suspectedBugs.map(b => `${b.summary}: ${b.details}`).join("\n"));
    }
    if (saveToStorage) await refreshSavedRuns();
    return record;
  }

  async function checkCurrent(targetOverride = null) {
    const state = getState();
    if (!state) {
      status("Open an image first, or upload a .jxl directly in Decode Lab.");
      return null;
    }
    const target = targetOverride || $("dc-target")?.value || "plain-jxl";
    setBusyUi(true, `Encoding & checking ${target} across ${selectedDecoders().length} decoders…`);
    try {
      let bytes;
      let label = `${getTitle()} (${target})`;
      let refPixels = null;
      let refW = 0;
      let refH = 0;
      const manifest = state.manifest;

      if (target === "raw-source") {
        const layer = getSelectedLayer();
        const asset = layer && state.assets.get(layer.id);
        if (!asset?.source || layer.mime !== "image/jxl") {
          throw new Error("Selected layer is not a native JPEG XL source.");
        }
        bytes = asset.source;
        label = `${layer.name} (raw source .jxl)`;
      } else if (target === "project-jxl") {
        bytes = await buildProjectJxlBytes(state);
        label = `${getTitle()}.project.jxl`;
      } else {
        bytes = await buildPlainJxlBytes(state);
        label = `${getTitle()}.jxl`;
      }

      if (target !== "raw-source" && !manifest.cmyk?.enabled) {
        const { canvas } = await renderFlattenedCanvas(state, 0);
        refW = canvas.width;
        refH = canvas.height;
        refPixels = new Uint8Array(canvas.getContext("2d").getImageData(0, 0, refW, refH).data);
      }

      const rec = await checkBytes(bytes, {
        label,
        sourceKind: target,
        coding: manifest.coding ?? null,
        hasSplines: Boolean(manifest.splines?.length),
        hasTimeline: Boolean(manifest.timeline?.enabled && manifest.timeline.frames?.length > 1),
        hasBlending: Boolean(manifest.timeline?.frames?.some(f => f.blendMode && f.blendMode !== "auto" && f.blendMode !== "replace")),
        hasRefOnly: Boolean(manifest.timeline?.frames?.some(f => f.referenceOnly)),
        isCmyk: Boolean(manifest.cmyk?.enabled),
        referencePixels: refPixels,
        referenceWidth: refW,
        referenceHeight: refH,
      });
      status(`Decode Lab: ${verdictMeta(rec.verdict).label} (${rec.results.filter(r => r.ok).length}/${rec.results.length} OK)`);
      return rec;
    } catch (error) {
      appLog.error("decode-lab", error instanceof Error ? error.message : String(error));
      status(error instanceof Error ? error.message : String(error));
      throw error;
    } finally {
      setBusyUi(false);
    }
  }

  async function runCompatibilitySweep() {
    const decs = selectedDecoders();
    setBusyUi(true, "Preparing compatibility sweep test patch…");
    const results = [];
    try {
      const patch = await makeSweepTestPatch(getState(), renderFlattenedCanvas);
      const { width: w, height: h, opaque, alpha } = patch;
      const icc = await codec("minimalCmykIcc");
      const cmyk8 = await codec("rgbaToCmyk", { pixels: opaque.slice(), bitDepth: 8 });
      const splineDct = await codec("splineColorDct", { fgR: 245, fgG: 220, fgB: 60, bgR: 24, bgG: 32, bgB: 48, sigma: 1.8 });
      const z31 = Array(31).fill(0);

      const f1 = opaque.slice();
      const f2 = opaque.slice();
      for (let i = 0; i < f1.length; i += 4) {
        f1[i] = Math.min(255, f1[i] + 35);
        f2[i + 2] = Math.min(255, f2[i + 2] + 45);
      }
      const animPixels = new Uint8Array(opaque.byteLength * 3);
      animPixels.set(opaque, 0);
      animPixels.set(f1, opaque.byteLength);
      animPixels.set(f2, opaque.byteLength * 2);

      const cases = [
        {
          label: "Sweep: Modular Lossless (RGBA)",
          coding: { mode: "modular", effort: 3, containerMode: "always" },
          build: () => codec("encodeConfigured", {
            pixels: alpha.slice(), width: w, height: h,
            options: { mode: "modular", effort: 3, containerMode: "always", keepInvisible: true },
          }),
          ref: alpha,
        },
        {
          label: "Sweep: Modular Squeeze + YCoCg",
          coding: { mode: "modular", effort: 3, squeeze: true, forceRct: "ycocg", groupSizeShift: 0, containerMode: "always" },
          build: () => codec("encodeConfigured", {
            pixels: opaque.slice(), width: w, height: h,
            options: { mode: "modular", effort: 3, squeeze: true, forceRct: "ycocg", groupSizeShift: 0, containerMode: "always" },
          }),
          ref: opaque,
        },
        {
          label: "Sweep: VarDCT Lossy (d=1.0)",
          coding: { mode: "vardct", distance: 1.0, effort: 3, containerMode: "always" },
          build: () => codec("encodeConfigured", {
            pixels: opaque.slice(), width: w, height: h,
            options: { mode: "vardct", distance: 1.0, effort: 3, containerMode: "always" },
          }),
          ref: opaque,
        },
        {
          label: "Sweep: VarDCT Progressive + LF + Noise",
          coding: { mode: "vardct", distance: 1.2, effort: 5, progressive: "dc_vlf_lf_ac", progressiveDc: 1, noiseMode: "content", containerMode: "always" },
          build: () => codec("encodeConfigured", {
            pixels: opaque.slice(), width: w, height: h,
            options: { mode: "vardct", distance: 1.2, effort: 5, progressive: "dc_vlf_lf_ac", progressiveDc: 1, noiseMode: "content", containerMode: "always" },
          }),
          ref: opaque,
        },
        {
          label: "Sweep: VarDCT 2× Resampling",
          coding: { mode: "vardct", distance: 1.5, effort: 3, resampling: 2, containerMode: "always" },
          build: () => codec("encodeConfigured", {
            pixels: opaque.slice(), width: w, height: h,
            options: { mode: "vardct", distance: 1.5, effort: 3, resampling: 2, containerMode: "always" },
          }),
          ref: opaque,
        },
        {
          label: "Sweep: VarDCT + Manual Splines",
          coding: { mode: "vardct", distance: 1.0, effort: 3, containerMode: "always" },
          hasSplines: true,
          build: () => codec("encodeConfigured", {
            pixels: opaque.slice(), width: w, height: h,
            options: {
              mode: "vardct", distance: 1.0, effort: 3, containerMode: "always", bakeSplines: true,
              splines: [{
                controlPoints: [[8, 48], [48, 12], [88, 48]],
                colorDct: [[splineDct[0], ...z31], [splineDct[1], ...z31], [splineDct[2], ...z31]],
                sigmaDct: [splineDct[3], ...z31],
              }],
            },
          }),
          ref: null,
        },
        {
          label: "Sweep: Native Animation (3f, 150/250/350ms)",
          coding: { mode: "modular", effort: 3, containerMode: "always" },
          hasTimeline: true,
          build: () => codec("encodeTimeline", {
            pixels: animPixels.slice(), width: w, height: h,
            options: {
              tpsNumerator: 1000, tpsDenominator: 1, loops: 2,
              frames: [
                { name: "f0", duration: 150, blendMode: "replace" },
                { name: "f1", duration: 250, blendMode: "replace" },
                { name: "f2", duration: 350, blendMode: "replace" },
              ],
              coding: { mode: "modular", effort: 3, containerMode: "always" },
            },
          }),
          ref: opaque,
        },
        {
          label: "Sweep: CMYK Lossless (Level 10 + ICC)",
          coding: { mode: "modular", effort: 3 },
          isCmyk: true,
          extraFeatures: ["level-10"],
          build: () => codec("encodeCmyk", {
            samples: cmyk8.slice(), width: w, height: h, bitDepth: 8, icc: icc.slice(),
            options: { mode: "modular", distance: 0, effort: 3 },
          }),
          ref: null,
        },
        {
          label: "Sweep: CMYK Lossy (ICC + Black EC)",
          coding: { mode: "vardct", distance: 1.5, effort: 3 },
          isCmyk: true,
          build: () => codec("encodeCmyk", {
            samples: cmyk8.slice(), width: w, height: h, bitDepth: 8, icc: icc.slice(),
            options: { mode: "vardct", distance: 1.5, effort: 3 },
          }),
          ref: null,
        },
        {
          label: "Sweep: Project Container (zned + znas)",
          coding: { mode: "modular", effort: 3, containerMode: "always" },
          extraFeatures: ["private-boxes"],
          build: async () => {
            const inner = await codec("encode", { pixels: alpha.slice(), width: w, height: h, stride: w * 4 });
            const sweepId = newId();
            const fakeManifest = {
              version: 1, width: w, height: h, originalId: sweepId,
              layers: [{ id: sweepId, name: "patch.jxl", mime: "image/jxl", width: w, height: h, x: 0, y: 0, visible: true, opacity: 1, exposure: 0, contrast: 1, saturation: 1, scale: 1, angle: 0 }],
            };
            const fakeAssets = new Map([[sweepId, { source: inner }]]);
            const blob = writeProject(inner, fakeManifest, fakeAssets, []);
            return new Uint8Array(await blob.arrayBuffer());
          },
          ref: alpha,
        },
      ];

      for (let i = 0; i < cases.length; i++) {
        const c = cases[i];
        setBusyUi(true, `Sweep ${i + 1}/${cases.length}: ${c.label}…`);
        const bytes = await c.build();
        const isLast = i === cases.length - 1;
        const rec = await checkBytes(bytes, {
          label: c.label,
          sourceKind: "sweep",
          coding: c.coding,
          extraFeatures: c.extraFeatures ?? [],
          hasSplines: Boolean(c.hasSplines),
          hasTimeline: Boolean(c.hasTimeline),
          isCmyk: Boolean(c.isCmyk),
          referencePixels: c.ref,
          referenceWidth: c.ref ? w : 0,
          referenceHeight: c.ref ? h : 0,
          decoders: decs,
          keepPixels: isLast,
          saveToStorage: true,
        });
        results.push(rec);
      }

      const bugs = results.filter(r => r.verdict !== "all-exact" && r.verdict !== "all-agree").length;
      appLog.info("decode-lab", `Compatibility sweep complete: ${results.length} cases run, ${bugs} bug/disagreement finding${bugs === 1 ? "" : "s"}.`);
      status(`Sweep complete: ${results.length} cases · ${bugs} disagreement/failure${bugs === 1 ? "" : "s"}`);
      return results;
    } catch (error) {
      appLog.error("decode-lab", `Compatibility sweep failed: ${error instanceof Error ? error.message : String(error)}`);
      status(error instanceof Error ? error.message : String(error));
      throw error;
    } finally {
      setBusyUi(false);
    }
  }

  async function buildExportPayload({ includeBytes = true } = {}) {
    const allFull = await listDecodeRuns();
    const runs = allFull.map(r => {
      const { jxlBytes, ...rest } = r;
      const out = { ...rest, hasStoredBytes: Boolean(jxlBytes && jxlBytes.byteLength > 0) };
      if (includeBytes && jxlBytes && jxlBytes.byteLength > 0) {
        out.jxlBase64 = bytesToBase64(jxlBytes);
      }
      return out;
    });
    const surface = buildCompatibilitySurface(runs);
    const suspectedBugs = runs.flatMap(r =>
      (r.suspectedBugs ?? []).map(b => ({
        runId: r.runId,
        createdAt: r.createdAt,
        label: r.label,
        jxlSha256: r.jxlSha256,
        features: r.features,
        verdict: r.verdict,
        ...b,
      })),
    );
    return {
      schemaVersion: 1,
      tool: "JPEG XL Studio Decode Compatibility Lab",
      exportedAt: new Date().toISOString(),
      decoders: catalogCache ?? ALL_DECODERS,
      summary: {
        totalRuns: runs.length,
        exactRuns: runs.filter(r => r.verdict === "all-exact").length,
        agreeRuns: runs.filter(r => r.verdict === "all-agree").length,
        disagreementRuns: runs.filter(r => r.verdict === "pixel-disagreement").length,
        partialFailureRuns: runs.filter(r => r.verdict === "partial-failure").length,
        allFailedRuns: runs.filter(r => r.verdict === "all-failed").length,
        suspectedBugCount: suspectedBugs.length,
      },
      compatibilitySurface: surface,
      suspectedBugs,
      runs,
    };
  }

  async function exportJson({ includeBytes = true } = {}) {
    const payload = await buildExportPayload({ includeBytes });
    const text = JSON.stringify(payload, null, 2);
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    download(new Blob([text], { type: "application/json" }), `jxl-decode-lab-${stamp}.json`);
    appLog.info("decode-lab", `Exported JSON report (${payload.runs.length} runs, ${payload.suspectedBugs.length} findings).`);
    status(`Exported ${payload.runs.length} decode runs (JSON)`);
    return payload;
  }

  async function exportNdjson() {
    const allFull = await listDecodeRuns();
    const lines = allFull.map(r => {
      const { jxlBytes: _omit, ...rest } = r;
      return JSON.stringify(rest);
    });
    const text = lines.join("\n") + (lines.length ? "\n" : "");
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    download(new Blob([text], { type: "application/x-ndjson" }), `jxl-decode-lab-${stamp}.ndjson`);
    appLog.info("decode-lab", `Exported NDJSON (${lines.length} runs).`);
    status(`Exported ${lines.length} decode runs (NDJSON)`);
    return text;
  }

  async function exportMarkdown() {
    const payload = await buildExportPayload({ includeBytes: false });
    const lines = [
      "# JPEG XL Multi-Decoder Compatibility & Bug Report",
      "",
      `- Exported: ${payload.exportedAt}`,
      `- Total runs: ${payload.summary.totalRuns} (exact: ${payload.summary.exactRuns}, ≤2 Δ: ${payload.summary.agreeRuns}, disagreements: ${payload.summary.disagreementRuns}, partial fails: ${payload.summary.partialFailureRuns}, all failed: ${payload.summary.allFailedRuns})`,
      "",
      "## Feature Compatibility Surface",
      "",
      "| Feature | zenjxl-decoder | jxl-rs | jxl-oxide | libjxl |",
      "|---|---:|---:|---:|---:|",
    ];
    for (const row of payload.compatibilitySurface) {
      const cols = ALL_DECODERS.map(d => {
        const c = row.decoders[d];
        if (!c || c.total === 0) return "—";
        if (c.failed > 0) return `${c.ok}/${c.total} FAIL`;
        if (c.diverged > 0) return `${c.ok}/${c.total} DIFF`;
        if (c.rounding > 0) return `${c.ok}/${c.total} ≈`;
        return `${c.ok}/${c.total} EXACT`;
      });
      lines.push(`| \`${row.feature}\` | ${cols.join(" | ")} |`);
    }
    lines.push("", "## Suspected Findings", "");
    if (!payload.suspectedBugs.length) {
      lines.push("_No decoder failures or pixel disagreements (> 2) recorded._");
    } else {
      for (const b of payload.suspectedBugs) {
        lines.push(`- **[${b.target}]** \`${b.label}\` (sha256 \`${b.jxlSha256?.slice(0, 12) || "—"}\`, features: ${b.features.join(", ")}): ${b.summary} — \`${b.details}\``);
      }
    }
    const text = lines.join("\n") + "\n";
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    download(new Blob([text], { type: "text/markdown" }), `jxl-decode-lab-${stamp}.md`);
    appLog.info("decode-lab", `Exported Markdown bug summary (${payload.suspectedBugs.length} findings).`);
    status("Exported Markdown compatibility summary");
    return text;
  }

  // Wire DOM controls
  $("dc-run-btn")?.addEventListener("click", () => {
    checkCurrent().catch(() => {});
  });
  $("check-decoding")?.addEventListener("click", () => {
    showDecodeTab?.();
    checkCurrent().catch(() => {});
  });
  $("dc-sweep-btn")?.addEventListener("click", () => {
    runCompatibilitySweep().catch(() => {});
  });
  $("dc-upload-btn")?.addEventListener("click", () => $("dc-upload-file")?.click());
  $("dc-upload-file")?.addEventListener("change", async event => {
    const files = [...event.target.files];
    event.target.value = "";
    if (!files.length) return;
    setBusyUi(true, `Checking ${files.length} external .jxl file${files.length === 1 ? "" : "s"}…`);
    try {
      for (const f of files) {
        const bytes = new Uint8Array(await f.arrayBuffer());
        await checkBytes(bytes, { label: f.name, sourceKind: "external-upload" });
      }
      status(`Checked ${files.length} external JXL file${files.length === 1 ? "" : "s"}`);
    } catch (error) {
      status(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyUi(false);
    }
  });
  $("dc-diff-a")?.addEventListener("change", renderDiffCanvas);
  $("dc-diff-b")?.addEventListener("change", renderDiffCanvas);
  $("dc-diff-mode")?.addEventListener("change", renderDiffCanvas);
  $("dc-export-json")?.addEventListener("click", () => exportJson({ includeBytes: true }));
  $("dc-export-ndjson")?.addEventListener("click", () => exportNdjson());
  $("dc-export-md")?.addEventListener("click", () => exportMarkdown());
  $("dc-clear-runs")?.addEventListener("click", async () => {
    if (!savedRuns.length) return;
    await clearDecodeRuns();
    latestRun = null;
    latestPixels = {};
    latestRef = null;
    await refreshSavedRuns();
    renderLatestResult();
    appLog.storage("Cleared saved Decode Lab runs");
    status("Cleared saved decode runs");
  });
  for (const btn of document.querySelectorAll("[data-dc-filter]")) {
    btn.addEventListener("click", () => {
      historyFilter = btn.dataset.dcFilter;
      for (const b of document.querySelectorAll("[data-dc-filter]")) {
        b.setAttribute("aria-pressed", String(b === btn));
      }
      renderHistoryList();
    });
  }

  // Expose clean automation API for headless/scripted decoder testing loops
  const api = {
    checkBytes,
    checkCurrent,
    runSweep: runCompatibilitySweep,
    listRuns: () => listDecodeRuns(),
    getCompatibilitySurface: () => buildCompatibilitySurface(savedRuns),
    buildExportPayload,
    exportJson,
    exportNdjson,
    exportMarkdown,
    clearRuns: async () => {
      await clearDecodeRuns();
      await refreshSavedRuns();
    },
    getLatestRun: () => latestRun,
    refresh: refreshSavedRuns,
    updateButtons,
  };
  if (typeof window !== "undefined") {
    window.jxlStudioDecodeLab = api;
  }

  refreshSavedRuns();
  renderLatestResult();
  return api;
}
