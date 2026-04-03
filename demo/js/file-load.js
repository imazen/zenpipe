// =====================================================================
// File Loading
// =====================================================================

import { $, state } from './state.js';
import { sendToWorker } from './worker-client.js';
import { renderOverview, renderDetail } from './render.js';
import { showError } from './toasts.js';
import { resetHistory } from './history.js';

/**
 * Compute initial detail region based on viewport size, DPR, and image dimensions.
 * Centers the region with aspect ratio matching the viewport, clamped 4:3/3:4.
 */
function initRegion() {
  const detailWrap = $('detail-wrap');
  const vpW = detailWrap.clientWidth || 800;
  const vpH = detailWrap.clientHeight || 600;
  const vpAspect = vpW / vpH;
  const clampedAspect = Math.max(3 / 4, Math.min(4 / 3, vpAspect));
  const dpr = window.devicePixelRatio || 1;
  let regionW = Math.min(1, (vpW * dpr) / state.sourceWidth);
  let regionH = Math.min(1, ((vpW * dpr) / clampedAspect) / state.sourceHeight);
  // Clamp to avoid excessive render cost
  const maxPixels = 1920 * 1080;
  const regionPixels = (regionW * state.sourceWidth) * (regionH * state.sourceHeight);
  if (regionPixels > maxPixels) {
    const scale = Math.sqrt(maxPixels / regionPixels);
    regionW *= scale;
    regionH *= scale;
  }
  // Cap at 80% so region selector is always draggable
  regionW = Math.min(0.8, regionW);
  regionH = Math.min(0.8, regionH);
  state.region = { x: (1 - regionW) / 2, y: (1 - regionH) / 2, w: regionW, h: regionH };
}

export async function loadImage(file) {
  $('status').textContent = 'Loading...';
  // Show large loading message in the viewport
  $('dropzone').classList.add('hidden');
  $('editor-ui').classList.remove('hidden');
  $('loading-message').classList.add('active');

  // Send raw bytes to worker for decoding
  const buffer = await file.arrayBuffer();
  let result;
  try {
    result = await sendToWorker('init', { data: buffer });
  } catch (e) {
    $('loading-message').classList.remove('active');
    $('status').textContent = `Load error: ${e.message}`;
    showError(`Image decode failed: ${e.message}`);
    return;
  }

  state.sourceWidth = result.width;
  state.sourceHeight = result.height;
  state.sourceImage = true;
  resetHistory();
  initRegion();
  const be = result.backend === 'wasm' ? 'WASM' : 'mock';

  $('loading-message').classList.remove('active');
  $('status').textContent = `${state.sourceWidth}\u00d7${state.sourceHeight} \u2014 ${file.name} [${be}]`;

  renderOverview();
  renderDetail();

  // Phase 2: native decode upgrade (background)
  triggerNativeUpgrade(file.name, be);
}

/**
 * Trigger background native decode upgrade via zencodecs.
 * Replaces browser-decoded pixels with natively-decoded pixels + metadata.
 * Re-renders overview + detail when complete.
 */
async function triggerNativeUpgrade(label, backendLabel) {
  try {
    const result = await sendToWorker('upgrade', {});
    // Build metadata badges for the status bar
    const badges = [];
    if (result.format) badges.push(result.format.toUpperCase());
    if (result.has_icc) badges.push('ICC');
    if (result.has_exif) badges.push('EXIF');
    if (result.has_xmp) badges.push('XMP');
    if (result.has_gain_map) badges.push('HDR');
    const meta = badges.length > 0 ? ` [${badges.join(' ')}]` : '';
    $('status').textContent = `${state.sourceWidth}\u00d7${state.sourceHeight} \u2014 ${label} [${backendLabel}]${meta}`;

    // Re-render with natively-decoded source
    renderOverview();
    renderDetail();
  } catch {
    // Upgrade failed silently — browser decode preview remains active
  }
}

// =====================================================================
// Picsum Photo Picker
// =====================================================================

const PICSUM_IDS = [10, 11, 15, 17, 24, 28, 29, 36, 37, 39, 40, 42];

export function buildPhotoPicker() {
  const container = $('sample-photos');
  const popover = $('popover-photos');
  for (const pid of PICSUM_IDS) {
    const img = document.createElement('img');
    img.className = 'photo-thumb';
    img.src = `https://picsum.photos/id/${pid}/200/150`;
    img.alt = `Sample photo ${pid}`;
    img.dataset.picsumId = pid;
    img.addEventListener('load', () => img.classList.add('loaded'));
    img.addEventListener('click', () => loadPicsumPhoto(pid, img));
    container.appendChild(img);

    // Also add to popover if it exists
    if (popover) {
      const img2 = document.createElement('img');
      img2.className = 'photo-thumb';
      img2.src = `https://picsum.photos/id/${pid}/200/150`;
      img2.alt = `Sample photo ${pid}`;
      img2.dataset.picsumId = pid;
      img2.addEventListener('load', () => img2.classList.add('loaded'));
      img2.addEventListener('click', () => {
        // Close popover when a photo is picked
        $('photo-picker-popover').classList.remove('open');
        loadPicsumPhoto(pid, img2);
      });
      popover.appendChild(img2);
    }
  }
}

export async function loadPicsumPhoto(pid, thumbEl) {
  // Mark the clicked thumb as loading
  const allThumbs = document.querySelectorAll('.photo-thumb');
  allThumbs.forEach(t => t.classList.remove('loading-active'));
  thumbEl.classList.add('loading-active');
  $('status').textContent = `Loading sample photo ${pid}...`;

  // Show large loading message
  $('dropzone').classList.add('hidden');
  $('editor-ui').classList.remove('hidden');
  $('loading-message').classList.add('active');

  try {
    const resp = await fetch(`https://picsum.photos/id/${pid}/4000/3000`);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const buffer = await resp.arrayBuffer();

    // Pass to worker the same way as file upload
    const result = await sendToWorker('init', { data: buffer });

    state.sourceWidth = result.width;
    state.sourceHeight = result.height;
    state.sourceImage = true;
    resetHistory();
    initRegion();
    const be = result.backend === 'wasm' ? 'WASM' : 'mock';

    $('loading-message').classList.remove('active');
    $('status').textContent = `${state.sourceWidth}\u00d7${state.sourceHeight} \u2014 picsum/${pid} [${be}]`;

    renderOverview();
    renderDetail();

    // Phase 2: native decode upgrade (background)
    triggerNativeUpgrade(`picsum/${pid}`, be);
  } catch (e) {
    $('loading-message').classList.remove('active');
    $('status').textContent = `Failed to load photo: ${e.message}`;
    showError(`Failed to load photo: ${e.message}`);
    console.error('Picsum load failed:', e);
  }
  thumbEl.classList.remove('loading-active');
}
