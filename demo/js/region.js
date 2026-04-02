// =====================================================================
// Region Selector & Detail Canvas Drag
// =====================================================================

import { $, state } from './state.js';
import { scheduleDetailOnly } from './render.js';

export function updateRegionSelector() {
  // Region selector is hidden, but keep the position math for overview click
  const canvas = $('overview-canvas');
  const rect = canvas.getBoundingClientRect();
  const wrap = $('overview-wrap').getBoundingClientRect();
  const cx = rect.left - wrap.left;
  const cy = rect.top - wrap.top;
  const sel = $('region-selector');
  sel.style.left = (cx + state.region.x * rect.width) + 'px';
  sel.style.top = (cy + state.region.y * rect.height) + 'px';
  sel.style.width = (state.region.w * rect.width) + 'px';
  sel.style.height = (state.region.h * rect.height) + 'px';
}

/**
 * Draw the overview canvas content upscaled into the detail canvas
 * as an instant preview during drag/zoom.
 */
export function showUpscaledPreview() {
  const overviewCanvas = $('overview-canvas');
  const detailCanvas = $('detail-canvas');
  if (!overviewCanvas.width || !detailCanvas.width) return;
  const ctx = detailCanvas.getContext('2d');
  const r = state.region;
  // Source rect on overview canvas
  const sx = r.x * overviewCanvas.width;
  const sy = r.y * overviewCanvas.height;
  const sw = r.w * overviewCanvas.width;
  const sh = r.h * overviewCanvas.height;
  // Draw overview crop upscaled into detail canvas
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = 'high';
  ctx.drawImage(overviewCanvas, sx, sy, sw, sh, 0, 0, detailCanvas.width, detailCanvas.height);
}

export function initRegionDrag() {
  // Detail canvas drag: pan the region by dragging on the detail view
  const detailCanvas = $('detail-canvas');
  let dragging = false, startX, startY, startRX, startRY;

  detailCanvas.addEventListener('pointerdown', e => {
    if (!state.sourceImage) return;
    dragging = true;
    startX = e.clientX; startY = e.clientY;
    startRX = state.region.x; startRY = state.region.y;
    detailCanvas.setPointerCapture(e.pointerId);
    detailCanvas.style.cursor = 'grabbing';
    e.preventDefault();
  });

  detailCanvas.addEventListener('pointermove', e => {
    if (!dragging) return;
    // Map mouse delta to normalized source image coordinates.
    // The detail canvas shows region.w × region.h of the source.
    // One CSS pixel of mouse movement = region.w/canvasDisplayW of source.
    const canvasRect = detailCanvas.getBoundingClientRect();
    const dxNorm = -(e.clientX - startX) / canvasRect.width * state.region.w;
    const dyNorm = -(e.clientY - startY) / canvasRect.height * state.region.h;
    state.region.x = Math.max(0, Math.min(1 - state.region.w, startRX + dxNorm));
    state.region.y = Math.max(0, Math.min(1 - state.region.h, startRY + dyNorm));
    updateRegionSelector();
    showUpscaledPreview();
  });

  detailCanvas.addEventListener('pointerup', () => {
    if (!dragging) return;
    dragging = false;
    detailCanvas.style.cursor = 'grab';
    scheduleDetailOnly();
  });

  detailCanvas.addEventListener('pointercancel', () => {
    dragging = false;
    detailCanvas.style.cursor = 'grab';
  });

  // Set default cursor style
  detailCanvas.style.cursor = 'grab';

  // Click on overview to reposition region (keep existing behavior)
  $('overview-wrap').addEventListener('click', e => {
    if (e.target === $('region-selector')) return;
    const rect = $('overview-canvas').getBoundingClientRect();
    const nx = (e.clientX - rect.left) / rect.width;
    const ny = (e.clientY - rect.top) / rect.height;
    state.region.x = Math.max(0, Math.min(1 - state.region.w, nx - state.region.w / 2));
    state.region.y = Math.max(0, Math.min(1 - state.region.h, ny - state.region.h / 2));
    updateRegionSelector();
    scheduleDetailOnly();
  });

  // Resize observer
  new ResizeObserver(() => updateRegionSelector()).observe($('overview-wrap'));
}

export function initScrollZoom() {
  $('detail-wrap').addEventListener('wheel', e => {
    if (!state.sourceImage) return;
    e.preventDefault();

    // Use proportional zoom: smooth trackpads send small deltaY,
    // mouse wheels send larger values. Clamp to ±5% per event for
    // smooth scrolling, up to ±20% for discrete wheel clicks.
    const rawDelta = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY; // lines → pixels
    const zoomPct = Math.max(-0.20, Math.min(0.20, rawDelta * 0.002));
    const scaleFactor = 1 + zoomPct;

    // Map mouse position in detail-wrap to normalized image coordinates
    const detailWrap = $('detail-wrap');
    const wrapRect = detailWrap.getBoundingClientRect();
    const mx = (e.clientX - wrapRect.left) / wrapRect.width;
    const my = (e.clientY - wrapRect.top) / wrapRect.height;
    // Convert to image coordinates
    const imgX = state.region.x + mx * state.region.w;
    const imgY = state.region.y + my * state.region.h;

    // Scale region dimensions
    const newW = Math.max(0.01, Math.min(1.0, state.region.w * scaleFactor));
    const newH = Math.max(0.01, Math.min(1.0, state.region.h * scaleFactor));

    // Recenter so the point under the mouse stays fixed
    state.region.w = newW;
    state.region.h = newH;
    state.region.x = Math.max(0, Math.min(1 - state.region.w, imgX - mx * state.region.w));
    state.region.y = Math.max(0, Math.min(1 - state.region.h, imgY - my * state.region.h));

    updateRegionSelector();
    showUpscaledPreview();
    scheduleDetailOnly();
  }, { passive: false });
}

export function initPinchZoom() {
  const wrap = $('detail-wrap');
  let initialDist = 0;
  let initialW = 0, initialH = 0;
  let pinchCenterX = 0, pinchCenterY = 0;

  function touchDist(t1, t2) {
    const dx = t1.clientX - t2.clientX;
    const dy = t1.clientY - t2.clientY;
    return Math.sqrt(dx * dx + dy * dy);
  }

  wrap.addEventListener('touchstart', e => {
    if (e.touches.length === 2) {
      e.preventDefault();
      initialDist = touchDist(e.touches[0], e.touches[1]);
      initialW = state.region.w;
      initialH = state.region.h;
      // Midpoint in normalized image coords
      const wrapRect = wrap.getBoundingClientRect();
      const midX = ((e.touches[0].clientX + e.touches[1].clientX) / 2 - wrapRect.left) / wrapRect.width;
      const midY = ((e.touches[0].clientY + e.touches[1].clientY) / 2 - wrapRect.top) / wrapRect.height;
      pinchCenterX = state.region.x + midX * state.region.w;
      pinchCenterY = state.region.y + midY * state.region.h;
    }
  }, { passive: false });

  wrap.addEventListener('touchmove', e => {
    if (e.touches.length === 2 && initialDist > 0) {
      e.preventDefault();
      const dist = touchDist(e.touches[0], e.touches[1]);
      // Pinch in (fingers closer) = smaller region = zoom in
      const scale = initialDist / dist;
      const newW = Math.max(0.01, Math.min(1.0, initialW * scale));
      const newH = Math.max(0.01, Math.min(1.0, initialH * scale));

      // Midpoint in normalized detail coords
      const wrapRect = wrap.getBoundingClientRect();
      const midX = ((e.touches[0].clientX + e.touches[1].clientX) / 2 - wrapRect.left) / wrapRect.width;
      const midY = ((e.touches[0].clientY + e.touches[1].clientY) / 2 - wrapRect.top) / wrapRect.height;

      state.region.w = newW;
      state.region.h = newH;
      state.region.x = Math.max(0, Math.min(1 - state.region.w, pinchCenterX - midX * state.region.w));
      state.region.y = Math.max(0, Math.min(1 - state.region.h, pinchCenterY - midY * state.region.h));

      updateRegionSelector();
      showUpscaledPreview();
    }
  }, { passive: false });

  wrap.addEventListener('touchend', e => {
    if (e.touches.length < 2 && initialDist > 0) {
      initialDist = 0;
      scheduleDetailOnly();
    }
  });
}
