// =====================================================================
// Export Modal
// =====================================================================

import { $, state, getFilterAdjustments } from './state.js';
import { sendToWorker } from './worker-client.js';

const EXPORT_FORMATS = [
  { id: 'jpeg', label: 'JPEG', ext: 'jpg', mime: 'image/jpeg', color: '#f59e0b' },
  { id: 'webp', label: 'WebP', ext: 'webp', mime: 'image/webp', color: '#10b981' },
  { id: 'png',  label: 'PNG',  ext: 'png',  mime: 'image/png',  color: '#6366f1' },
  { id: 'avif', label: 'AVIF', ext: 'avif', mime: 'image/avif', color: '#ef4444' },
  { id: 'jxl',  label: 'JXL',  ext: 'jxl',  mime: 'image/jxl',  color: '#8b5cf6' },
  { id: 'gif',  label: 'GIF',  ext: 'gif',  mime: 'image/gif',  color: '#ec4899' },
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

// bytes per pixel at default quality/settings
const BPP_ESTIMATES = {
  jpeg: 0.5, webp: 0.3, png: 1.5, avif: 0.2, jxl: 0.25, gif: 0.8,
};

// ms per 1000 pixels at default settings
const TIME_PER_KPIX = {
  jpeg: 0.5, webp: 0.5, png: 0.5, avif: 5, jxl: 3, gif: 2,
};

let exportFormat = 'jpeg';
let exportAspectLocked = true;
let exportWidth = 0, exportHeight = 0;
const exportSettings = {}; // format -> { key: value }

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
        updateExportEstimates();
      });
      container.appendChild(row);
    }
  }
}

function updateExportDims() {
  const mp = (exportWidth * exportHeight / 1e6).toFixed(1);
  $('export-dims').textContent = `${exportWidth} \u00d7 ${exportHeight} (${mp} MP)`;
  $('export-width').value = exportWidth;
  $('export-height').value = exportHeight;
  $('export-megapixels').textContent = `${mp} MP`;
}

function updateExportEstimates() {
  const pixels = exportWidth * exportHeight;
  const kpix = pixels / 1000;
  const settings = exportSettings[exportFormat];

  // Time estimate
  let baseTime = TIME_PER_KPIX[exportFormat] * kpix;
  // Apply effort/speed factor for relevant formats
  if (exportFormat === 'jxl' && settings.effort) {
    baseTime *= settings.effort / 7;
  }
  if (exportFormat === 'avif' && settings.speed) {
    // Lower speed = slower encoding
    baseTime *= (11 - settings.speed) / 5;
  }
  $('export-est-time').textContent = formatTime(baseTime);

  // Size estimate
  let bpp = BPP_ESTIMATES[exportFormat];
  // Adjust for quality
  if (settings.quality !== undefined) {
    bpp *= settings.quality / 75; // normalize around a mid-point
  }
  if (settings.lossless) {
    bpp *= 3; // lossless is much bigger
  }
  if (exportFormat === 'gif' && settings.colors !== undefined) {
    bpp *= settings.colors / 256;
  }
  const sizeBytes = pixels * bpp;
  $('export-est-size').textContent = formatSize(sizeBytes);
}

function formatTime(ms) {
  if (ms < 1000) return '< 1s';
  if (ms < 10000) return `~${Math.round(ms / 1000)}s`;
  if (ms < 60000) return `~${Math.round(ms / 1000)}s`;
  return `~${(ms / 60000).toFixed(1)}m`;
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
    tab.innerHTML = `<span class="export-format-dot" style="background:${fmt.color}"></span>${fmt.label}`;
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
}

export function initExportModal() {
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
        quality: settings.quality || 85,
        film_preset: state.filmPreset,
        film_preset_intensity: state.filmPresetIntensity,
      };
      // Pass resize dimensions if different from source
      if (exportWidth !== state.sourceWidth || exportHeight !== state.sourceHeight) {
        exportData.width = exportWidth;
        exportData.height = exportHeight;
      }
      // Pass format-specific options
      for (const [key, val] of Object.entries(settings)) {
        if (key !== 'quality') exportData[key] = val;
      }

      const result = await sendToWorker('export', exportData);

      // Use the actual format the worker encoded (may differ if browser fallback)
      const actualFmt = EXPORT_FORMATS.find(f => f.id === result.format) || fmt;
      const MIME_MAP = {
        jpeg: 'image/jpeg', webp: 'image/webp', png: 'image/png',
        avif: 'image/avif', jxl: 'image/jxl', gif: 'image/gif',
      };
      const mime = MIME_MAP[result.format] || 'image/jpeg';
      const ext = actualFmt.ext;

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
      $('status').textContent = `Export error: ${e.message}`;
    }
  });
}
