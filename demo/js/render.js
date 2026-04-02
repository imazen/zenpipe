// =====================================================================
// Rendering
// =====================================================================

import { $, state, OVERVIEW_MAX, DETAIL_MAX, RENDER_DEBOUNCE_MS, getFilterAdjustments } from './state.js';
import { sendToWorker } from './worker-client.js';
import { showError } from './toasts.js';
import { applyCssPreview, clearCssPreview } from './css-preview.js';
import { updateRegionSelector } from './region.js';
import { formatVal } from './sidebar.js';

let renderDebounceId = null;

export async function renderOverview() {
  if (!state.sourceImage) return;
  const id = ++state.overviewRenderId;
  $('overview-spinner').classList.add('active');

  try {
    const result = await sendToWorker('overview', {
      adjustments: getFilterAdjustments(),
      maxDim: OVERVIEW_MAX,
      film_preset: state.filmPreset,
      film_preset_intensity: state.filmPresetIntensity,
    });
    if (id !== state.overviewRenderId) return; // superseded
    const canvas = $('overview-canvas');
    canvas.width = result.width;
    canvas.height = result.height;
    const ctx = canvas.getContext('2d');
    // result.pixels is a Uint8Array transferred from the worker.
    // Reconstruct as Uint8ClampedArray for ImageData.
    const px = result.pixels;
    const expected = result.width * result.height * 4;
    if (!px || px.byteLength !== expected) {
      console.error(`Overview pixel mismatch: got ${px?.byteLength}, expected ${expected}, type=${px?.constructor?.name}`);
    }
    const clamped = new Uint8ClampedArray(px.buffer, px.byteOffset, expected);
    const imgData = new ImageData(clamped, result.width, result.height);
    ctx.putImageData(imgData, 0, 0);
    canvas.classList.add('sharp');
  } catch (e) {
    console.error('Overview render failed:', e);
    showError(`Overview: ${e.message}`);
    handleRenderError();
  }
  $('overview-spinner').classList.remove('active');
  clearCssPreview();
  updateRegionSelector();
}

export async function renderDetail() {
  if (!state.sourceImage) return;
  const id = ++state.detailRenderId;
  $('detail-spinner').classList.add('active');

  try {
    const result = await sendToWorker('region', {
      adjustments: getFilterAdjustments(),
      rect: state.region,
      maxDim: DETAIL_MAX,
      film_preset: state.filmPreset,
      film_preset_intensity: state.filmPresetIntensity,
    });
    if (id !== state.detailRenderId) return;
    const canvas = $('detail-canvas');
    canvas.width = result.width;
    canvas.height = result.height;
    const ctx = canvas.getContext('2d');
    const imgData = new ImageData(new Uint8ClampedArray(result.pixels.buffer), result.width, result.height);
    ctx.putImageData(imgData, 0, 0);

    // Compute and display pixel ratio
    updatePixelRatioBadge();
  } catch (e) {
    console.error('Detail render failed:', e);
    showError(`Detail: ${e.message}`);
    handleRenderError();
  }
  $('detail-spinner').classList.remove('active');
  clearCssPreview();
}

export function updatePixelRatioBadge() {
  const canvas = $('detail-canvas');
  const wrap = $('detail-wrap');
  if (!canvas.width || !wrap.clientWidth) return;
  // The region covers region.w * sourceWidth pixels, rendered into canvas.width pixels
  const srcPixelsW = state.region.w * state.sourceWidth;
  const cssDisplayW = Math.min(wrap.clientWidth, canvas.width);
  const ratio = srcPixelsW / cssDisplayW;
  const badge = $('pixel-ratio-badge');
  if (ratio <= 1.05) {
    badge.textContent = '1:1';
    badge.style.display = '';
  } else {
    const r = Math.round(ratio);
    badge.textContent = `1:${r}`;
    badge.style.display = '';
  }
}

export function scheduleRender() {
  applyCssPreview();
  $('overview-canvas').classList.remove('sharp');
  // Debounce: wait for slider activity to pause before dispatching to worker.
  // CSS preview is instant; worker render waits for the slider to settle.
  if (renderDebounceId) clearTimeout(renderDebounceId);
  renderDebounceId = setTimeout(() => {
    renderDebounceId = null;
    // Bump version IDs -- any in-flight renders will be discarded on arrival.
    renderOverview();
    renderDetail();
  }, RENDER_DEBOUNCE_MS);
}

export function scheduleDetailOnly() {
  $('detail-spinner').classList.add('active');
  if (renderDebounceId) clearTimeout(renderDebounceId);
  renderDebounceId = setTimeout(() => {
    renderDebounceId = null;
    renderDetail();
  }, RENDER_DEBOUNCE_MS);
}

// Auto-reset the last changed slider on render error
export function handleRenderError() {
  if (!state.lastChangedSliderKey) return;
  const key = state.lastChangedSliderKey;
  state.lastChangedSliderKey = null; // prevent re-entry

  const slider = document.querySelector(`input[type="range"][data-key="${key}"]`);
  if (!slider) return;
  const identity = parseFloat(slider.dataset.identity);
  slider.value = identity;
  state.adjustments[key] = identity;

  const row = slider.closest('.slider-row');
  if (row) {
    const display = row.querySelector('.val');
    const resetBtn = row.querySelector('.slider-reset');
    if (display) display.textContent = formatVal(identity);
    if (resetBtn) resetBtn.classList.remove('visible');
    row.classList.add('slider-disabled');
    // Re-enable after 3 seconds
    setTimeout(() => row.classList.remove('slider-disabled'), 3000);
  }
  state.touchedSliders.delete(key);
}
