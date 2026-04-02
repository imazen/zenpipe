// =====================================================================
// Export History
// =====================================================================
//
// Tracks each export: thumbnail blob URL, format, file size, bpp,
// dimensions, timestamp. Collapsible section in export modal.
// In-memory (cleared on page refresh).

import { $ } from './state.js';

const history = [];

function formatSize(bytes) {
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * Record an export result.
 * @param {{ data: Uint8Array, format: string, mime: string, width: number, height: number, size: number }} result
 */
export function recordExport(result) {
  const blob = new Blob([result.data], { type: result.mime || 'application/octet-stream' });
  const blobUrl = URL.createObjectURL(blob);
  const bpp = (result.size * 8 / (result.width * result.height)).toFixed(2);

  history.unshift({
    blobUrl,
    blob,
    format: result.format,
    width: result.width,
    height: result.height,
    size: result.size,
    bpp,
    timestamp: Date.now(),
  });

  // Cap at 20 entries
  while (history.length > 20) {
    const old = history.pop();
    URL.revokeObjectURL(old.blobUrl);
  }

  renderHistory();
}

function renderHistory() {
  const container = $('export-history-list');
  if (!container) return;
  container.innerHTML = '';

  if (history.length === 0) {
    container.innerHTML = '<div style="color:var(--text-dim);font-size:11px;padding:4px">No exports yet</div>';
    return;
  }

  for (const entry of history) {
    const row = document.createElement('div');
    row.className = 'export-history-row';
    const time = new Date(entry.timestamp).toLocaleTimeString();
    row.innerHTML = `
      <img class="export-history-thumb" src="${entry.blobUrl}" alt="${entry.format}">
      <div class="export-history-info">
        <span class="export-history-format">${entry.format.toUpperCase()}</span>
        <span>${entry.width}×${entry.height}</span>
        <span>${formatSize(entry.size)}</span>
        <span>${entry.bpp} bpp</span>
      </div>
      <div class="export-history-actions">
        <a class="export-history-dl" href="${entry.blobUrl}" download="export.${entry.format}" title="Download">&#x2B73;</a>
        <a class="export-history-view" href="${entry.blobUrl}" target="_blank" title="View full size">&#x1F50D;</a>
      </div>
    `;
    container.appendChild(row);
  }
}

export function initExportHistory() {
  // Insert the history section into the export modal body
  const exportBody = document.querySelector('.export-body');
  if (!exportBody) return;

  const section = document.createElement('div');
  section.className = 'export-section export-history-section';
  section.innerHTML = `
    <div class="export-section-label" style="cursor:pointer;user-select:none" id="export-history-toggle">
      <span class="collapse-icon" style="font-size:9px;display:inline-block;transition:transform 0.15s">&#x25B8;</span>
      Export History
    </div>
    <div id="export-history-list" style="display:none"></div>
  `;
  exportBody.appendChild(section);

  // Toggle collapse
  $('export-history-toggle').addEventListener('click', () => {
    const list = $('export-history-list');
    const icon = section.querySelector('.collapse-icon');
    const visible = list.style.display !== 'none';
    list.style.display = visible ? 'none' : 'block';
    icon.style.transform = visible ? '' : 'rotate(90deg)';
  });

  renderHistory();
}
