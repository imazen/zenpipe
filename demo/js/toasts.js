// =====================================================================
// Error / Info Toast
// =====================================================================

import { $ } from './state.js';

let errorToastTimer = null;

export function showError(msg) {
  const wrap = $('detail-wrap');
  // Remove any existing error toast
  const existing = wrap.querySelector('.error-toast');
  if (existing) existing.remove();
  if (errorToastTimer) { clearTimeout(errorToastTimer); errorToastTimer = null; }

  const el = document.createElement('div');
  el.className = 'error-toast';
  el.textContent = msg;
  wrap.appendChild(el);

  errorToastTimer = setTimeout(() => {
    el.classList.add('fade-out');
    setTimeout(() => el.remove(), 500);
    errorToastTimer = null;
  }, 5000);
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
