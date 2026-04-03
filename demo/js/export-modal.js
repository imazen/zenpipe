// =====================================================================
// Export Modal
// =====================================================================

import { $, state, getFilterAdjustments } from './state.js';
import { recordExport, initExportHistory } from './export-history.js';
import { sendToWorker } from './worker-client.js';

const EXPORT_FORMATS = [
  { id: 'jpeg', label: 'JPEG', ext: 'jpg', mime: 'image/jpeg', color: '#f59e0b', engine: 'zen' },
  { id: 'webp', label: 'WebP', ext: 'webp', mime: 'image/webp', color: '#10b981', engine: 'zen' },
  { id: 'png',  label: 'PNG',  ext: 'png',  mime: 'image/png',  color: '#6366f1', engine: 'zen' },
  { id: 'jxl',  label: 'JXL',  ext: 'jxl',  mime: 'image/jxl',  color: '#8b5cf6', engine: 'zen' },
  { id: 'gif',  label: 'GIF',  ext: 'gif',  mime: 'image/gif',  color: '#ec4899', engine: 'zen' },
  { id: 'avif', label: 'AVIF', ext: 'avif', mime: 'image/avif', color: '#ef4444', engine: 'zen' },
];

// Format controls from zencodecs audit (zencodecs/src/zennode_defs.rs)
const FORMAT_CONTROLS = {
  jpeg: [
    { type: 'range', key: 'quality', label: 'Quality', min: 1, max: 100, default: 85, unit: '' },
    { type: 'range', key: 'effort', label: 'Effort', min: 0, max: 2, default: 1, unit: '', hint: '0=fast 2=best' },
  ],
  webp: [
    { type: 'range', key: 'quality', label: 'Quality', min: 1, max: 100, default: 80, unit: '' },
    { type: 'range', key: 'effort', label: 'Effort', min: 0, max: 10, default: 5, unit: '' },
    { type: 'checkbox', key: 'lossless', label: 'Lossless', default: false },
    { type: 'range', key: 'near_lossless', label: 'Near-Lossless', min: 0, max: 100, default: 0, unit: '', visible_when: 'lossless' },
  ],
  png: [
    { type: 'range', key: 'effort', label: 'Effort', min: 0, max: 12, default: 5, unit: '', hint: '0=fast 12=smallest' },
  ],
  avif: [
    { type: 'range', key: 'quality', label: 'Quality', min: 1, max: 100, default: 75, unit: '' },
    { type: 'range', key: 'effort', label: 'Effort', min: 0, max: 10, default: 6, unit: '' },
    { type: 'checkbox', key: 'lossless', label: 'Lossless', default: false },
  ],
  jxl: [
    { type: 'range', key: 'quality', label: 'Quality', min: 1, max: 100, default: 75, unit: '' },
    { type: 'range', key: 'effort', label: 'Effort', min: 0, max: 10, default: 7, unit: '' },
    { type: 'checkbox', key: 'lossless', label: 'Lossless', default: false },
  ],
  gif: [
    { type: 'range', key: 'quality', label: 'Quality', min: 1, max: 100, default: 80, unit: '' },
    { type: 'range', key: 'dithering', label: 'Dithering', min: 0, max: 100, default: 50, unit: '%' },
  ],
};

let exportFormat = 'jpeg';
let exportAspectLocked = true;
let exportWidth = 0, exportHeight = 0;
const exportSettings = {}; // format -> { key: value }
let previewDebounceId = null;
let previewBlobUrl = null;

// Initialize default settings for each format
for (const fmt of EXPORT_FORMATS) {
  exportSettings[fmt.id] = {};
  for (const ctrl of FORMAT_CONTROLS[fmt.id]) {
    exportSettings[fmt.id][ctrl.key] = ctrl.default;
  }
}

function selectExportFormat(formatId) {
  exportFormat = formatId;

  // Update tab active state
  for (const tab of $('export-format-tabs').children) {
    tab.classList.toggle('active', tab.dataset.format === formatId);
  }

  // Update confirm button text
  const fmt = EXPORT_FORMATS.find(f => f.id === formatId);
  $('export-confirm').textContent = `Export ${fmt.label}`;

  renderFormatControls();
  updateExportEstimates();
}

function renderFormatControls() {
  const container = $('export-format-controls');
  container.innerHTML = '<div class="export-section-label">Format Settings</div>';

  const controls = FORMAT_CONTROLS[exportFormat];
  const settings = exportSettings[exportFormat];
  const conditionalRows = []; // { row, visibleWhen } for visibility updates

  for (const ctrl of controls) {
    if (ctrl.type === 'range') {
      const row = document.createElement('div');
      row.className = 'export-control';
      row.innerHTML = `
        <label>${ctrl.label}</label>
        <input type="range" min="${ctrl.min}" max="${ctrl.max}" step="1"
               value="${settings[ctrl.key]}" data-key="${ctrl.key}">
        <span class="export-val">${settings[ctrl.key]}</span>
      `;
      const slider = row.querySelector('input');
      const valSpan = row.querySelector('.export-val');
      slider.addEventListener('input', () => {
        settings[ctrl.key] = parseInt(slider.value);
        valSpan.textContent = slider.value;
        updateExportEstimates();
      });
      if (ctrl.visible_when) conditionalRows.push({ row, visibleWhen: ctrl.visible_when });
      container.appendChild(row);
    } else if (ctrl.type === 'checkbox') {
      const row = document.createElement('div');
      row.className = 'export-check';
      const id = `export-chk-${ctrl.key}`;
      row.innerHTML = `
        <input type="checkbox" id="${id}" ${settings[ctrl.key] ? 'checked' : ''} data-key="${ctrl.key}">
        <label for="${id}">${ctrl.label}</label>
      `;
      const chk = row.querySelector('input');
      chk.addEventListener('change', () => {
        settings[ctrl.key] = chk.checked;
        updateVisibility();
        updateExportEstimates();
      });
      container.appendChild(row);
    }
  }

  function updateVisibility() {
    for (const { row, visibleWhen } of conditionalRows) {
      // visibleWhen is a settings key — show row when that key is truthy
      row.style.display = settings[visibleWhen] ? '' : 'none';
    }
  }
  updateVisibility();
}

function updateExportDims() {
  const mp = (exportWidth * exportHeight / 1e6).toFixed(1);
  $('export-dims').textContent = `${exportWidth} \u00d7 ${exportHeight} (${mp} MP)`;
  $('export-width').value = exportWidth;
  $('export-height').value = exportHeight;
  $('export-megapixels').textContent = `${mp} MP`;
}

function updateExportEstimates() {
  // Trigger a debounced encode preview — real data replaces heuristics
  scheduleEncodePreview();
}

/** Request an encode preview from the worker (debounced 200ms). */
function scheduleEncodePreview() {
  if (previewDebounceId) clearTimeout(previewDebounceId);
  previewDebounceId = setTimeout(runEncodePreview, 200);
}

async function runEncodePreview() {
  previewDebounceId = null;
  if (!state.sourceImage) return;
  // Skip if modal was closed (e.g., user clicked Export while preview was debounced)
  if (!$('export-modal-backdrop').classList.contains('open')) return;

  const wrap = $('export-preview-wrap');
  wrap.classList.add('loading');

  try {
    const settings = exportSettings[exportFormat];
    const result = await sendToWorker('encode_preview', {
      adjustments: getFilterAdjustments(),
      format: exportFormat,
      options: { ...settings },
      film_preset: state.filmPreset,
    });

    // Update preview image
    if (previewBlobUrl) URL.revokeObjectURL(previewBlobUrl);
    const MIME_MAP = {
      jpeg: 'image/jpeg', webp: 'image/webp', png: 'image/png',
      avif: 'image/avif', jxl: 'image/jxl', gif: 'image/gif',
    };
    const mime = MIME_MAP[exportFormat] || 'image/jpeg';
    const blob = new Blob([result.data], { type: mime });
    previewBlobUrl = URL.createObjectURL(blob);
    const img = $('export-preview-img');
    img.src = previewBlobUrl;
    img.style.display = 'block';

    // Stats overlay on preview
    const bpp = (result.size * 8 / (result.width * result.height)).toFixed(2);
    $('export-preview-stats').textContent = `${formatSize(result.size)} · ${bpp} bpp`;

    // Estimates from real preview data
    const previewPixels = result.width * result.height;
    const fullPixels = exportWidth * exportHeight;
    const ratio = fullPixels / previewPixels;
    const estFullSize = result.size * ratio;

    $('export-est-size').textContent = formatSize(result.size);
    $('export-est-full').textContent = `~${formatSize(estFullSize)}`;
    $('export-est-bpp').textContent = bpp;
  } catch (e) {
    // Preview failed — show placeholder
    $('export-preview-img').style.display = 'none';
    $('export-preview-stats').textContent = '';
    $('export-est-size').textContent = '--';
    $('export-est-full').textContent = '--';
    $('export-est-bpp').textContent = '--';
  }
  wrap.classList.remove('loading');
}

function formatSize(bytes) {
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function openExportModal() {
  if (!state.sourceImage) return;

  exportWidth = state.sourceWidth;
  exportHeight = state.sourceHeight;

  // Populate format tabs
  const tabsEl = $('export-format-tabs');
  tabsEl.innerHTML = '';
  for (const fmt of EXPORT_FORMATS) {
    const tab = document.createElement('button');
    tab.className = 'export-format-tab' + (fmt.id === exportFormat ? ' active' : '');
    tab.dataset.format = fmt.id;
    const engineBadge = fmt.engine === 'browser'
      ? '<span class="export-engine browser" title="Browser-native encoding">browser</span>'
      : '<span class="export-engine zen" title="WASM zen codec encoding">zen</span>';
    tab.innerHTML = `<span class="export-format-dot" style="background:${fmt.color}"></span>${fmt.label} ${engineBadge}`;
    tab.addEventListener('click', () => selectExportFormat(fmt.id));
    tabsEl.appendChild(tab);
  }

  updateExportDims();
  renderFormatControls();
  updateExportEstimates();

  $('export-modal-backdrop').classList.add('open');
}

export function closeExportModal() {
  $('export-modal-backdrop').classList.remove('open');
  if (previewDebounceId) { clearTimeout(previewDebounceId); previewDebounceId = null; }
  if (previewBlobUrl) { URL.revokeObjectURL(previewBlobUrl); previewBlobUrl = null; }
}

export function initExportModal() {
  initExportHistory();

  // Aspect ratio lock
  $('export-aspect-lock').addEventListener('click', () => {
    exportAspectLocked = !exportAspectLocked;
    const btn = $('export-aspect-lock');
    btn.classList.toggle('locked', exportAspectLocked);
    btn.innerHTML = exportAspectLocked ? '&#x1f512;' : '&#x1f513;';
  });

  // Width/Height linked inputs
  $('export-width').addEventListener('input', () => {
    const w = parseInt($('export-width').value) || 1;
    exportWidth = Math.max(1, Math.min(32000, w));
    if (exportAspectLocked && state.sourceWidth > 0) {
      exportHeight = Math.round(exportWidth * state.sourceHeight / state.sourceWidth);
      $('export-height').value = exportHeight;
    }
    updateExportDims();
    updateExportEstimates();
  });

  $('export-height').addEventListener('input', () => {
    const h = parseInt($('export-height').value) || 1;
    exportHeight = Math.max(1, Math.min(32000, h));
    if (exportAspectLocked && state.sourceHeight > 0) {
      exportWidth = Math.round(exportHeight * state.sourceWidth / state.sourceHeight);
      $('export-width').value = exportWidth;
    }
    updateExportDims();
    updateExportEstimates();
  });

  // Close modal
  $('export-close').addEventListener('click', closeExportModal);
  $('export-cancel').addEventListener('click', closeExportModal);
  $('export-modal-backdrop').addEventListener('click', (e) => {
    if (e.target === $('export-modal-backdrop')) closeExportModal();
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && $('export-modal-backdrop').classList.contains('open')) {
      closeExportModal();
    }
  });

  // Open modal from header button
  $('export-btn').addEventListener('click', () => openExportModal());

  // Perform export
  $('export-confirm').addEventListener('click', async () => {
    if (!state.sourceImage) return;
    closeExportModal();

    const fmt = EXPORT_FORMATS.find(f => f.id === exportFormat);
    const settings = exportSettings[exportFormat];

    $('status').textContent = `Exporting ${fmt.label}...`;
    try {
      const exportData = {
        adjustments: getFilterAdjustments(),
        format: exportFormat,
        width: exportWidth,
        height: exportHeight,
        options: { ...settings },
        film_preset: state.filmPreset,
      };

      const result = await sendToWorker('export', exportData);

      // Use the actual format the worker encoded (may differ if browser fallback)
      const actualFmt = EXPORT_FORMATS.find(f => f.id === result.format) || fmt;
      const MIME_MAP = {
        jpeg: 'image/jpeg', webp: 'image/webp', png: 'image/png',
        avif: 'image/avif', jxl: 'image/jxl', gif: 'image/gif',
      };
      const mime = MIME_MAP[result.format] || 'image/jpeg';
      const ext = actualFmt.ext;

      // Record in export history
      recordExport({ ...result, mime });

      const blob = new Blob([result.data], { type: mime });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `export.${ext}`;
      a.click();
      URL.revokeObjectURL(url);

      const dims = result.width ? `${result.width}\u00d7${result.height} ` : '';
      const sizeKB = (result.size / 1024).toFixed(0);
      const fallback = result.format !== exportFormat ? ` (${actualFmt.label} fallback)` : '';
      $('status').textContent = `Exported ${dims}${actualFmt.label} ${sizeKB} KB${fallback}`;
    } catch (e) {
      // Show error in the export modal (re-open it) with retry
      openExportModal();
      const previewStats = $('export-preview-stats');
      if (previewStats) previewStats.textContent = `Error: ${e.message}`;
      $('status').textContent = `Export error: ${e.message}`;
    }
  });
}
