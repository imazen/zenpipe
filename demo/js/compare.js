// =====================================================================
// Original vs Edited Compare
// =====================================================================
//
// Hold backslash (\) or tap-hold on the detail canvas to see the
// unfiltered original. Release to snap back to edited.
//
// Implementation: renders with empty adjustments (geometry prefix
// cache hit, no filter suffix → fast). Stores the original ImageData
// and swaps canvas content on hold/release.

import { $, state, OVERVIEW_MAX, DETAIL_MAX } from './state.js';
import { sendToWorker } from './worker-client.js';

let comparing = false;
let originalOverviewData = null; // { pixels, width, height }
let originalDetailData = null;
let savedOverviewData = null;   // current edited data to restore
let savedDetailData = null;
let holdTimer = null;
const HOLD_DELAY_MS = 200;

/** Render the original (unfiltered) at both overview and detail sizes. */
async function renderOriginal() {
  try {
    const [overview, detail] = await Promise.all([
      sendToWorker('overview', {
        adjustments: {},
        maxDim: OVERVIEW_MAX,
        film_preset: null,
      }),
      sendToWorker('region', {
        adjustments: {},
        rect: state.region,
        maxDim: DETAIL_MAX,
        film_preset: null,
      }),
    ]);
    originalOverviewData = overview;
    originalDetailData = detail;
  } catch {
    // Failed to render original — compare won't work
    originalOverviewData = null;
    originalDetailData = null;
  }
}

function showOriginal() {
  if (comparing || !originalDetailData) return;
  comparing = true;

  // Save current canvas content
  const detailCanvas = $('detail-canvas');
  const overviewCanvas = $('overview-canvas');
  savedDetailData = saveCanvas(detailCanvas);
  savedOverviewData = saveCanvas(overviewCanvas);

  // Draw original
  drawToCanvas(detailCanvas, originalDetailData);
  drawToCanvas(overviewCanvas, originalOverviewData);

  // Show badge
  showBadge(true);
}

function showEdited() {
  if (!comparing) return;
  comparing = false;

  // Restore edited canvas
  const detailCanvas = $('detail-canvas');
  const overviewCanvas = $('overview-canvas');
  if (savedDetailData) restoreCanvas(detailCanvas, savedDetailData);
  if (savedOverviewData) restoreCanvas(overviewCanvas, savedOverviewData);
  savedDetailData = null;
  savedOverviewData = null;

  showBadge(false);
}

function saveCanvas(canvas) {
  if (!canvas.width || !canvas.height) return null;
  const ctx = canvas.getContext('2d');
  return ctx.getImageData(0, 0, canvas.width, canvas.height);
}

function restoreCanvas(canvas, imageData) {
  if (!imageData) return;
  canvas.width = imageData.width;
  canvas.height = imageData.height;
  canvas.getContext('2d').putImageData(imageData, 0, 0);
}

function drawToCanvas(canvas, result) {
  if (!result?.pixels) return;
  const w = result.width;
  const h = result.height;
  canvas.width = w;
  canvas.height = h;
  const expected = w * h * 4;
  const px = result.pixels;
  const clamped = new Uint8ClampedArray(px.buffer || px, px.byteOffset || 0, expected);
  canvas.getContext('2d').putImageData(new ImageData(clamped, w, h), 0, 0);
}

function showBadge(show) {
  let badge = document.getElementById('compare-badge');
  if (show && !badge) {
    badge = document.createElement('div');
    badge.id = 'compare-badge';
    badge.textContent = 'ORIGINAL';
    $('detail-wrap').appendChild(badge);
  }
  if (badge) badge.style.display = show ? 'flex' : 'none';
}

/** Pre-render original when adjustments change. */
export function invalidateOriginal() {
  originalOverviewData = null;
  originalDetailData = null;
}

export function initCompare() {
  const detailCanvas = $('detail-canvas');

  // Keyboard: hold backslash
  document.addEventListener('keydown', (e) => {
    if (e.key === '\\' && !e.repeat && state.sourceImage) {
      e.preventDefault();
      if (!originalDetailData) {
        renderOriginal().then(() => { if (e.key === '\\') showOriginal(); });
      } else {
        showOriginal();
      }
    }
  });
  document.addEventListener('keyup', (e) => {
    if (e.key === '\\') showEdited();
  });

  // Touch: tap-hold on detail canvas (200ms threshold)
  detailCanvas.addEventListener('touchstart', (e) => {
    if (e.touches.length !== 1 || !state.sourceImage) return;
    const startX = e.touches[0].clientX;
    const startY = e.touches[0].clientY;

    holdTimer = setTimeout(() => {
      holdTimer = null;
      if (!originalDetailData) {
        renderOriginal().then(() => showOriginal());
      } else {
        showOriginal();
      }
    }, HOLD_DELAY_MS);

    // Cancel if finger moves (it's a drag, not a hold)
    const moveHandler = (me) => {
      const dx = me.touches[0].clientX - startX;
      const dy = me.touches[0].clientY - startY;
      if (Math.abs(dx) > 10 || Math.abs(dy) > 10) {
        if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }
        detailCanvas.removeEventListener('touchmove', moveHandler);
      }
    };
    detailCanvas.addEventListener('touchmove', moveHandler, { passive: true });
  }, { passive: true });

  detailCanvas.addEventListener('touchend', () => {
    if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }
    showEdited();
  });

  // Mouse: hold click on detail canvas (same 200ms threshold)
  detailCanvas.addEventListener('mousedown', (e) => {
    if (e.button !== 0 || !state.sourceImage) return;
    holdTimer = setTimeout(() => {
      holdTimer = null;
      if (!originalDetailData) {
        renderOriginal().then(() => showOriginal());
      } else {
        showOriginal();
      }
    }, HOLD_DELAY_MS);
  });
  detailCanvas.addEventListener('mouseup', () => {
    if (holdTimer) { clearTimeout(holdTimer); holdTimer = null; }
    showEdited();
  });
}
