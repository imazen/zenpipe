// =====================================================================
// Rendering
// =====================================================================

import { $, state, OVERVIEW_MAX, DETAIL_MAX, RENDER_DEBOUNCE_MS, getFilterAdjustments } from './state.js';
import { sendToWorker } from './worker-client.js';
import { showError, setResetToLastSafeCallback } from './toasts.js';
import { applyCssPreview, clearCssPreview } from './css-preview.js';
import { updateRegionSelector } from './region.js';
import { formatVal } from './sidebar.js';

let renderDebounceId = null;

// Register the reset-to-last-safe callback for toasts
setResetToLastSafeCallback(resetToLastSafe);

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

    // Snapshot safe adjustments after successful render
    state.lastSafeAdjustments = JSON.parse(JSON.stringify(getFilterAdjustments()));
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

    // Force CSS to upscale small canvases to fill the viewport.
    // max-width:100% only constrains — it won't enlarge a 50px canvas
    // to 800px. We set explicit CSS dimensions, maintaining aspect ratio.
    const wrap = $('detail-wrap');
    const wrapW = wrap.clientWidth;
    const wrapH = wrap.clientHeight - 30; // leave room for pixel-info bar
    const canvasAspect = result.width / result.height;
    const fitW = Math.min(wrapW, wrapH * canvasAspect);
    const fitH = fitW / canvasAspect;
    canvas.style.width = Math.round(fitW) + 'px';
    canvas.style.height = Math.round(fitH) + 'px';

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

  // Source pixels covered by the region
  const srcPixelsW = Math.round(state.region.w * state.sourceWidth);
  const srcPixelsH = Math.round(state.region.h * state.sourceHeight);
  // CSS display size: use getBoundingClientRect for the actual rendered size
  // (accounts for max-width, object-fit, etc.)
  const canvasRect = canvas.getBoundingClientRect();
  const cssDisplayW = canvasRect.width || wrap.clientWidth;
  // Ratio: source pixels per CSS pixel
  const ratio = srcPixelsW / cssDisplayW;

  const info = $('pixel-info');
  if (!info) return;

  let ratioText, ratioClass;
  canvas.style.imageRendering = 'auto'; // reset; upscale path overrides below
  if (ratio >= 0.95 && ratio <= 1.05) {
    ratioText = '1:1';
    ratioClass = 'ratio-exact';
  } else if (ratio > 1) {
    // Downscaled: more source pixels than display pixels
    const r = ratio < 10 ? ratio.toFixed(1) : Math.round(ratio);
    ratioText = `1:${r}`;
    ratioClass = 'ratio-down';
  } else {
    // Upscaled: fewer source pixels than display pixels (zoomed in past 1:1)
    const upscale = 1 / ratio;
    const r = upscale < 10 ? upscale.toFixed(1) : Math.round(upscale);
    ratioText = `${r}:1 upscaled`;
    ratioClass = upscale > 4 ? 'ratio-up-warn' : 'ratio-up';

    // Switch to pixelated rendering past 6x so users see actual pixels
    canvas.style.imageRendering = upscale > 6 ? 'pixelated' : 'auto';
  }

  const renderedW = canvas.width;
  const renderedH = canvas.height;
  info.innerHTML = `<span class="ratio-text ${ratioClass}">${ratioText}</span> `
    + `<span class="ratio-dims">${srcPixelsW}\u00d7${srcPixelsH} of ${state.sourceWidth}\u00d7${state.sourceHeight}</span>`;
  info.style.display = 'flex';
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

/**
 * Reset all adjustments to the last safe state and update all slider DOM values.
 */
export function resetToLastSafe() {
  const safe = state.lastSafeAdjustments;
  if (!safe || Object.keys(safe).length === 0) return;

  // Flatten safe adjustments back into state.adjustments
  // safe is in the format { "zenfilters.exposure": { "stops": 1.5 }, ... }
  // state.adjustments uses keys like "zenfilters.exposure.stops"
  for (const node of state.sliderNodes) {
    for (const p of node.params) {
      const safeNode = safe[node.id];
      if (safeNode && safeNode[p.paramName] !== undefined) {
        state.adjustments[p.adjustKey] = safeNode[p.paramName];
      } else {
        // Node wasn't in safe adjustments = all params at identity
        state.adjustments[p.adjustKey] = p.identity;
      }
    }
  }

  // Update all slider DOM elements to match
  for (const row of document.querySelectorAll('.slider-row')) {
    const slider = row.querySelector('input[type="range"]');
    const display = row.querySelector('.val');
    const resetBtn = row.querySelector('.slider-reset');
    if (!slider) continue;
    const key = slider.dataset.key;
    const val = state.adjustments[key];
    if (val !== undefined) {
      slider.value = val;
      if (display) display.textContent = formatVal(val);
      const identity = parseFloat(slider.dataset.identity);
      if (resetBtn) {
        resetBtn.classList.toggle('visible', state.touchedSliders.has(key) && val !== identity);
      }
    }
    row.classList.remove('slider-disabled');
  }

  state.lastChangedSliderKey = null;
  scheduleRender();
}

// Auto-reset to last safe state on render error
export function handleRenderError() {
  // resetToLastSafe is called by the toast on click or auto-timeout
  // No additional action needed here — the toast handles it
}
