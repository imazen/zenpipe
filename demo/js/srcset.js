// =====================================================================
// Srcset generator (zenpipe#22): widths × formats → encode jobs on the
// export worker pool → one zip (or individual downloads) + an
// `<img srcset>` snippet, with a progress bar and cancel.
// =====================================================================

import { $, state, getFilterAdjustments } from './state.js';
import { exportPool } from './worker-pool.js';
import { buildZip } from './zip.js';

export const SRCSET_PRESETS = {
  thumbnail: [160, 320],
  mobile: [480, 640, 960],
  desktop: [1280, 1600, 1920],
  retina: [1920, 2560, 3840],
  all: [320, 480, 640, 960, 1280, 1600, 1920, 2560],
};

const FORMAT_INFO = {
  jpeg: { ext: 'jpg', mime: 'image/jpeg' },
  webp: { ext: 'webp', mime: 'image/webp' },
  png: { ext: 'png', mime: 'image/png' },
  avif: { ext: 'avif', mime: 'image/avif' },
  jxl: { ext: 'jxl', mime: 'image/jxl' },
  gif: { ext: 'gif', mime: 'image/gif' },
};

let abortController = null;
/** Per-format encoder settings, supplied by export-modal.js. */
let settingsFor = () => ({});

/** Parse the widths field: comma/space separated integers, deduplicated,
 *  ascending, capped at the source width (no upscaling in a srcset). */
export function parseWidths(text, sourceWidth) {
  const seen = new Set();
  const out = [];
  for (const tok of String(text).split(/[\s,;]+/)) {
    if (!tok) continue;
    const w = parseInt(tok, 10);
    if (!Number.isFinite(w) || w <= 0) continue;
    const capped = sourceWidth > 0 ? Math.min(w, sourceWidth) : w;
    if (!seen.has(capped)) {
      seen.add(capped);
      out.push(capped);
    }
  }
  return out.sort((a, b) => a - b);
}

/** Build the jobs for the pool: one export per width × format. */
export function buildJobs(widths, formats, adjustments, filmPreset) {
  const jobs = [];
  for (const format of formats) {
    for (const width of widths) {
      const height = Math.max(1, Math.round(width * state.sourceHeight / state.sourceWidth));
      jobs.push({
        type: 'export',
        data: {
          adjustments,
          format,
          width,
          height,
          options: { ...settingsFor(format) },
          film_preset: filmPreset,
        },
        width,
        format,
      });
    }
  }
  return jobs;
}

/** `image-400w.webp 400w, image-800w.webp 800w` per format + a full tag. */
export function srcsetSnippet(entries, baseName) {
  const byFormat = new Map();
  for (const e of entries) {
    if (!byFormat.has(e.format)) byFormat.set(e.format, []);
    byFormat.get(e.format).push(e);
  }
  const lines = ['<picture>'];
  const formats = [...byFormat.keys()];
  const fallback = formats.includes('jpeg') ? 'jpeg' : formats[formats.length - 1];
  for (const format of formats) {
    const list = byFormat.get(format)
      .sort((a, b) => a.width - b.width)
      .map(e => `${e.name} ${e.width}w`)
      .join(', ');
    if (format === fallback) {
      const largest = byFormat.get(format).reduce((a, b) => (a.width > b.width ? a : b));
      lines.push(`  <img src="${largest.name}" srcset="${list}" sizes="100vw" alt="${baseName}">`);
    } else {
      lines.push(`  <source type="${FORMAT_INFO[format].mime}" srcset="${list}" sizes="100vw">`);
    }
  }
  lines.push('</picture>');
  return lines.join('\n');
}

function setProgress(done, total, label) {
  const bar = $('srcset-progress');
  bar.max = Math.max(1, total);
  bar.value = done;
  $('srcset-progress-text').textContent = label ?? `${done} / ${total}`;
}

function selectedFormats() {
  return [...$('srcset-formats').querySelectorAll('input[type="checkbox"]:checked')]
    .map(el => el.value);
}

function download(name, data, mime) {
  const blob = new Blob([data], { type: mime });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

async function generate() {
  if (!state.sourceImage || abortController) return;
  const widths = parseWidths($('srcset-widths').value, state.sourceWidth);
  const formats = selectedFormats();
  if (widths.length === 0 || formats.length === 0) {
    setProgress(0, 1, 'Pick at least one width and one format');
    return;
  }
  const baseName = ($('srcset-basename').value || 'image').trim() || 'image';
  const jobs = buildJobs(widths, formats, getFilterAdjustments(), state.filmPreset);
  abortController = new AbortController();
  $('srcset-generate').disabled = true;
  $('srcset-cancel').disabled = false;
  $('srcset-snippet').value = '';
  setProgress(0, jobs.length);
  const started = performance.now();
  try {
    const results = await exportPool.run(jobs, {
      onProgress: (done, total) => setProgress(done, total),
      signal: abortController.signal,
    });
    const entries = results.map((r, i) => {
      const fmt = r.format || jobs[i].format;
      const ext = FORMAT_INFO[fmt]?.ext || fmt;
      return {
        name: `${baseName}-${jobs[i].width}w.${ext}`,
        data: r.data,
        width: jobs[i].width,
        format: fmt,
        mime: FORMAT_INFO[fmt]?.mime || 'application/octet-stream',
      };
    });
    $('srcset-snippet').value = srcsetSnippet(entries, baseName);
    const total = entries.reduce((n, e) => n + e.data.length, 0);
    if ($('srcset-individual').checked) {
      for (const e of entries) download(e.name, e.data, e.mime);
    } else {
      download(`${baseName}-srcset.zip`, buildZip(entries), 'application/zip');
    }
    const secs = ((performance.now() - started) / 1000).toFixed(1);
    setProgress(jobs.length, jobs.length,
      `Done: ${entries.length} files, ${(total / 1024).toFixed(0)} KB in ${secs}s`);
    $('status').textContent = `Srcset: ${entries.length} files (${widths.join('/')}w × ${formats.join('/')})`;
  } catch (e) {
    const cancelled = /cancelled/.test(e.message);
    setProgress(0, jobs.length, cancelled ? 'Cancelled' : `Error: ${e.message}`);
    if (!cancelled) $('status').textContent = `Srcset error: ${e.message}`;
  } finally {
    abortController = null;
    $('srcset-generate').disabled = false;
    $('srcset-cancel').disabled = true;
  }
}

export function cancelSrcset() {
  abortController?.abort();
}

/** Wire the srcset section of the export modal. `getSettings(format)`
 *  returns the per-format encoder options the modal holds. */
export function initSrcset(getSettings) {
  settingsFor = getSettings;
  const presets = $('srcset-preset');
  presets.addEventListener('change', () => {
    const p = SRCSET_PRESETS[presets.value];
    if (p) $('srcset-widths').value = p.join(', ');
  });
  $('srcset-generate').addEventListener('click', generate);
  $('srcset-cancel').addEventListener('click', cancelSrcset);
  $('srcset-cancel').disabled = true;
}

/** Called when the export modal opens: reflect the current image. */
export function refreshSrcset() {
  const src = state.sourceWidth;
  $('srcset-source-note').textContent = src
    ? `widths above ${src}px are capped to the source (no upscaling)`
    : '';
  if (!$('srcset-widths').value) {
    $('srcset-widths').value = SRCSET_PRESETS.mobile.join(', ');
  }
  setProgress(0, 1, '');
}
