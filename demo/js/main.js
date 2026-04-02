// =====================================================================
// Entry point — wires everything together
// =====================================================================

import { $, state } from './state.js';
import { initWorker } from './worker-client.js';
import { loadSchemaAndBuildUI, formatVal } from './sidebar.js';
import { loadImage, buildPhotoPicker } from './file-load.js';
import { initRegionDrag, initPinchZoom, initScrollZoom } from './region.js';
import { scheduleRender } from './render.js';
import { buildPresetStrip, setActivePreset } from './presets.js';
import { initExportModal } from './export-modal.js';

// File input and open button
$('file-input').addEventListener('change', e => {
  if (e.target.files[0]) loadImage(e.target.files[0]);
});
$('open-btn').addEventListener('click', () => $('file-input').click());

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
$('reset-btn').addEventListener('click', () => {
  state.touchedSliders.clear();
  state.lastChangedSliderKey = null;
  for (const row of document.querySelectorAll('.slider-row')) {
    const slider = row.querySelector('input[type="range"]');
    const display = row.querySelector('.val');
    const resetBtn = row.querySelector('.slider-reset');
    const identity = parseFloat(slider.dataset.identity);
    slider.value = identity;
    state.adjustments[slider.dataset.key] = identity;
    display.textContent = formatVal(identity);
    if (resetBtn) resetBtn.classList.remove('visible');
    row.classList.remove('slider-disabled');
  }
  setActivePreset(null);
  // Reset intensity
  state.filmPresetIntensity = 1.0;
  $('preset-intensity').value = 1;
  $('preset-intensity-val').textContent = '1.00';
  scheduleRender();
});

// Initialize region interactions
initRegionDrag();
initScrollZoom();
initPinchZoom();

// Initialize export modal
initExportModal();

// Boot
(async function init() {
  initWorker();
  buildPhotoPicker();
  buildPresetStrip();
  await loadSchemaAndBuildUI();
  $('status').textContent = `${state.sliderNodes.length} filters loaded \u2014 drop an image to start`;
})();
