// =====================================================================
// File Loading
// =====================================================================

import { $, state } from './state.js';
import { sendToWorker } from './worker-client.js';
import { renderOverview, renderDetail } from './render.js';
import { showError } from './toasts.js';

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
  const be = result.backend === 'wasm' ? 'WASM' : 'mock';

  // Smart region initialization: match detail viewport aspect ratio, clamped to 4:3/3:4
  const detailWrap = $('detail-wrap');
  const vpW = detailWrap.clientWidth || 800;
  const vpH = detailWrap.clientHeight || 600;
  const vpAspect = vpW / vpH;
  const clampedAspect = Math.max(3/4, Math.min(4/3, vpAspect));
  // Calculate region size for ~1:1 device pixels = source pixels
  const dpr = window.devicePixelRatio || 1;
  let regionW = Math.min(1, (vpW * dpr) / state.sourceWidth);
  let regionH = Math.min(1, ((vpW * dpr) / clampedAspect) / state.sourceHeight);
  // Clamp if region would exceed max real-time rendering (1920*1080 pixels)
  const maxPixels = 1920 * 1080;
  const regionPixels = (regionW * state.sourceWidth) * (regionH * state.sourceHeight);
  let scale = 1;
  if (regionPixels > maxPixels) {
    scale = Math.sqrt(maxPixels / regionPixels);
  }
  // Also cap at 80% of image so the region selector is always draggable
  const maxFraction = 0.8;
  regionW = Math.min(maxFraction, regionW * scale);
  regionH = Math.min(maxFraction, regionH * scale);
  state.region = { x: (1 - regionW) / 2, y: (1 - regionH) / 2, w: regionW, h: regionH };

  $('loading-message').classList.remove('active');
  $('status').textContent = `${state.sourceWidth}\u00d7${state.sourceHeight} \u2014 ${file.name} [${be}]`;

  renderOverview();
  renderDetail();
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
    const be = result.backend === 'wasm' ? 'WASM' : 'mock';

    // Smart region initialization
    const detailWrap = $('detail-wrap');
    const vpW = detailWrap.clientWidth || 800;
    const vpH = detailWrap.clientHeight || 600;
    const vpAspect = vpW / vpH;
    const clampedAspect = Math.max(3/4, Math.min(4/3, vpAspect));
    let regionW = Math.min(1, vpW / state.sourceWidth);
    let regionH = Math.min(1, (vpW / clampedAspect) / state.sourceHeight);
    const maxPixels = 1920 * 1080;
    const regionPixels = (regionW * state.sourceWidth) * (regionH * state.sourceHeight);
    let scale = 1;
    if (regionPixels > maxPixels) {
      scale = Math.sqrt(maxPixels / regionPixels);
    }
    const maxFraction = 0.8;
    regionW = Math.min(maxFraction, regionW * scale);
    regionH = Math.min(maxFraction, regionH * scale);
    state.region = { x: (1 - regionW) / 2, y: (1 - regionH) / 2, w: regionW, h: regionH };

    $('loading-message').classList.remove('active');
    $('status').textContent = `${state.sourceWidth}\u00d7${state.sourceHeight} \u2014 picsum/${pid} [${be}]`;

    renderOverview();
    renderDetail();
  } catch (e) {
    $('loading-message').classList.remove('active');
    $('status').textContent = `Failed to load photo: ${e.message}`;
    showError(`Failed to load photo: ${e.message}`);
    console.error('Picsum load failed:', e);
  }
  thumbEl.classList.remove('loading-active');
}
