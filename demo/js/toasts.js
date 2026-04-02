// =====================================================================
// Error / Info Toast
// =====================================================================

import { $ } from './state.js';

let errorToastTimer = null;
let resetToLastSafeCallback = null;

/**
 * Register the callback that resets adjustments to the last safe state.
 * Called from render.js during initialization.
 */
export function setResetToLastSafeCallback(cb) {
  resetToLastSafeCallback = cb;
}

export function showError(msg) {
  const wrap = $('detail-wrap');
  // Remove any existing error toast
  const existing = wrap.querySelector('.error-toast');
  if (existing) existing.remove();
  if (errorToastTimer) { clearTimeout(errorToastTimer); errorToastTimer = null; }

  const el = document.createElement('div');
  el.className = 'error-toast';
  el.innerHTML = `<div class="error-toast-msg">${msg}</div><div class="error-toast-hint">Tap to reset</div>`;

  function dismiss() {
    el.classList.add('fade-out');
    setTimeout(() => el.remove(), 500);
    errorToastTimer = null;
    if (resetToLastSafeCallback) resetToLastSafeCallback();
  }

  el.addEventListener('click', dismiss);
  wrap.appendChild(el);

  errorToastTimer = setTimeout(dismiss, 3000);
}

export function showInfo(msg) {
  const wrap = $('detail-wrap');
  // Remove any existing info toast
  const existing = wrap.querySelector('.info-toast');
  if (existing) existing.remove();

  const el = document.createElement('div');
  el.className = 'info-toast';
  el.textContent = msg;
  wrap.appendChild(el);

  // Auto-dismiss after 3 seconds
  setTimeout(() => {
    el.classList.add('fade-out');
    setTimeout(() => el.remove(), 500);
  }, 3000);
}
