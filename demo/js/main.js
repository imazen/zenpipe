// =====================================================================
// Entry point -- wires everything together
// =====================================================================

import { $, state } from './state.js';
import { initWorker } from './worker-client.js';
import { loadSchemaAndBuildUI, syncDOMToState } from './sidebar.js';
import { loadImage, buildPhotoPicker } from './file-load.js';
import { initRegionDrag, initPinchZoom, initScrollZoom, updateRegionSelector } from './region.js';
import { scheduleRender, scheduleDetailOnly } from './render.js';
import { buildPresetStrip, setActivePreset } from './presets.js';
import { initExportModal } from './export-modal.js';
import { initHistory, pushState } from './history.js';
import { initCompare, invalidateOriginal } from './compare.js';
import { initUserPresets } from './user-presets.js';

// File input and open button
$('file-input').addEventListener('change', e => {
  if (e.target.files[0]) loadImage(e.target.files[0]);
});
$('open-btn').addEventListener('click', () => $('file-input').click());

// Pick button: scroll to photo picker or toggle popover
$('pick-btn').addEventListener('click', () => {
  const dropzone = $('dropzone');
  if (!dropzone.classList.contains('hidden')) {
    // Dropzone is visible, scroll to sample photos
    $('sample-photos').scrollIntoView({ behavior: 'smooth', block: 'center' });
  } else {
    // Editor is showing, toggle the photo picker popover
    const popover = $('photo-picker-popover');
    popover.classList.toggle('open');
  }
});

// Close popover when clicking outside
document.addEventListener('click', e => {
  const popover = $('photo-picker-popover');
  if (!popover) return;
  if (!popover.classList.contains('open')) return;
  if (e.target.closest('#photo-picker-popover') || e.target.closest('#pick-btn')) return;
  popover.classList.remove('open');
});

// Drag-and-drop
document.addEventListener('dragover', e => { e.preventDefault(); $('dropzone').classList.add('dragover'); });
document.addEventListener('dragleave', () => $('dropzone').classList.remove('dragover'));
document.addEventListener('drop', e => {
  e.preventDefault();
  $('dropzone').classList.remove('dragover');
  const file = e.dataTransfer?.files[0];
  if (file?.type.startsWith('image/')) loadImage(file);
});

// Reset all sliders to identity and clear film preset
function resetAllSliders() {
  state.touchedSliders.clear();
  state.lastChangedSliderKey = null;
  // Reset all params to identity
  for (const node of state.sliderNodes) {
    for (const p of node.params) {
      state.adjustments[p.adjustKey] = p.identity;
    }
  }
  // Sync DOM to match
  syncDOMToState();
  setActivePreset(null);
  // Reset intensity
  state.filmPresetIntensity = 1.0;
  $('preset-intensity').value = 1;
  $('preset-intensity-val').textContent = '1.00';
  pushState();
  scheduleRender();
}

$('reset-btn').addEventListener('click', resetAllSliders);

// Initialize region interactions
initRegionDrag();
initScrollZoom();
initPinchZoom();

// Initialize export modal
initExportModal();

// Pixel info click: reset to 1:1 pixel ratio
$('pixel-info').addEventListener('click', () => {
  if (!state.sourceImage) return;
  const detailWrap = $('detail-wrap');
  const vpW = detailWrap.clientWidth || 800;
  const vpH = detailWrap.clientHeight || 600;
  // Set region so 1 source pixel = 1 device pixel (accounts for DPR)
  const dpr = window.devicePixelRatio || 1;
  const regionW = Math.min(1, (vpW * dpr) / state.sourceWidth);
  const regionH = Math.min(1, (vpH * dpr) / state.sourceHeight);
  // Recenter around current region center
  const cx = state.region.x + state.region.w / 2;
  const cy = state.region.y + state.region.h / 2;
  state.region.w = regionW;
  state.region.h = regionH;
  state.region.x = Math.max(0, Math.min(1 - regionW, cx - regionW / 2));
  state.region.y = Math.max(0, Math.min(1 - regionH, cy - regionH / 2));
  updateRegionSelector();
  scheduleDetailOnly();
});

// Dock position toggle: right → left → bottom → right
const DOCK_POSITIONS = ['right', 'left', 'bottom'];
const DOCK_LABELS = { right: 'R', left: 'L', bottom: 'B' };
let dockIndex = 0;
// Restore from localStorage
try {
  const saved = localStorage.getItem('zenpipe-dock');
  if (saved && DOCK_POSITIONS.includes(saved)) {
    dockIndex = DOCK_POSITIONS.indexOf(saved);
    if (dockIndex > 0) document.body.classList.add('dock-' + DOCK_POSITIONS[dockIndex]);
    $('dock-toggle').textContent = DOCK_LABELS[DOCK_POSITIONS[dockIndex]];
  }
} catch {}
$('dock-toggle').addEventListener('click', () => {
  document.body.classList.remove('dock-' + DOCK_POSITIONS[dockIndex]);
  dockIndex = (dockIndex + 1) % DOCK_POSITIONS.length;
  const pos = DOCK_POSITIONS[dockIndex];
  if (pos !== 'right') document.body.classList.add('dock-' + pos);
  $('dock-toggle').textContent = DOCK_LABELS[pos];
  try { localStorage.setItem('zenpipe-dock', pos); } catch {}
});

// Minimap overlay toggle
$('minimap-toggle').addEventListener('click', () => {
  const wrap = $('overview-wrap');
  wrap.classList.toggle('collapsed');
  $('minimap-toggle').textContent = wrap.classList.contains('collapsed') ? 'minimap' : 'minimap ✓';
});

// Crop region toggle
$('crop-toggle').addEventListener('click', () => {
  const sel = $('region-selector');
  sel.classList.toggle('hidden');
  $('crop-toggle').textContent = sel.classList.contains('hidden') ? 'crop' : 'crop ✓';
});

// Boot
(async function init() {
  initWorker();
  buildPhotoPicker();
  buildPresetStrip();
  await loadSchemaAndBuildUI();
  initCompare();
  initUserPresets();
  initHistory(() => {
    // On undo/redo: sync slider DOM, update preset chip, re-render
    invalidateOriginal();
    syncDOMToState();
    setActivePreset(state.filmPreset);
    $('preset-intensity').value = state.filmPresetIntensity;
    $('preset-intensity-val').textContent = state.filmPresetIntensity.toFixed(2);
    scheduleRender();
  });
  $('status').textContent = `${state.sliderNodes.length} filters loaded \u2014 drop an image to start`;
})();
