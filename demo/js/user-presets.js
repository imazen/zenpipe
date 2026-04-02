// =====================================================================
// User Presets — Save / Load / Delete / Import / Export
// =====================================================================
//
// A preset captures: adjustments + film preset + film preset intensity.
// Stored in localStorage as JSON. Can be exported/imported as JSON files.

import { $, state } from './state.js';
import { syncDOMToState } from './sidebar.js';
import { scheduleRender } from './render.js';
import { setActivePreset } from './presets.js';
import { pushState } from './history.js';

const STORAGE_KEY = 'zenpipe-user-presets';

let presets = []; // { name, adjustments, filmPreset, filmPresetIntensity }

function loadFromStorage() {
  try {
    const json = localStorage.getItem(STORAGE_KEY);
    if (json) presets = JSON.parse(json);
  } catch { presets = []; }
}

function saveToStorage() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(presets));
  } catch { /* storage full or unavailable */ }
}

function createPresetSnapshot(name) {
  return {
    name,
    adjustments: { ...state.adjustments },
    filmPreset: state.filmPreset,
    filmPresetIntensity: state.filmPresetIntensity,
  };
}

function applyPreset(preset) {
  // Restore adjustments
  for (const key of Object.keys(state.adjustments)) {
    state.adjustments[key] = preset.adjustments[key] ?? state.adjustments[key];
  }
  state.filmPreset = preset.filmPreset;
  state.filmPresetIntensity = preset.filmPresetIntensity ?? 1.0;
  syncDOMToState();
  setActivePreset(state.filmPreset);
  $('preset-intensity').value = state.filmPresetIntensity;
  $('preset-intensity-val').textContent = state.filmPresetIntensity.toFixed(2);
  pushState();
  scheduleRender();
}

function renderPresetList() {
  const list = $('user-preset-list');
  if (!list) return;
  list.innerHTML = '';

  if (presets.length === 0) {
    list.innerHTML = '<div style="color:var(--text-dim);font-size:11px;padding:4px 0">No saved presets</div>';
    return;
  }

  for (let i = 0; i < presets.length; i++) {
    const p = presets[i];
    const row = document.createElement('div');
    row.className = 'user-preset-row';
    row.innerHTML = `
      <span class="user-preset-name" title="Click to apply">${p.name}</span>
      <button class="user-preset-del" title="Delete">&times;</button>
    `;
    row.querySelector('.user-preset-name').addEventListener('click', () => applyPreset(p));
    row.querySelector('.user-preset-del').addEventListener('click', () => {
      presets.splice(i, 1);
      saveToStorage();
      renderPresetList();
    });
    list.appendChild(row);
  }
}

export function initUserPresets() {
  loadFromStorage();

  // Build the UI section in the sidebar (after reset button, before preset strip)
  const sidebar = $('sidebar');
  const presetStrip = $('preset-strip');

  const section = document.createElement('div');
  section.id = 'user-presets-section';
  section.innerHTML = `
    <div class="user-presets-header">
      <span class="user-presets-title">My Presets</span>
      <div class="user-presets-actions">
        <button class="btn user-preset-btn" id="save-preset-btn" title="Save current as preset">Save</button>
        <button class="btn user-preset-btn" id="import-preset-btn" title="Import presets from JSON">Import</button>
        <button class="btn user-preset-btn" id="export-preset-btn" title="Export presets as JSON">Export</button>
      </div>
    </div>
    <div id="user-preset-list"></div>
    <input type="file" id="preset-file-input" accept=".json" hidden>
  `;
  sidebar.insertBefore(section, presetStrip);

  renderPresetList();

  // Save button
  $('save-preset-btn').addEventListener('click', () => {
    const name = prompt('Preset name:');
    if (!name?.trim()) return;
    presets.push(createPresetSnapshot(name.trim()));
    saveToStorage();
    renderPresetList();
  });

  // Export
  $('export-preset-btn').addEventListener('click', () => {
    if (presets.length === 0) return;
    const json = JSON.stringify(presets, null, 2);
    const blob = new Blob([json], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'zenpipe-presets.json';
    a.click();
    URL.revokeObjectURL(url);
  });

  // Import
  $('import-preset-btn').addEventListener('click', () => $('preset-file-input').click());
  $('preset-file-input').addEventListener('change', async (e) => {
    const file = e.target.files[0];
    if (!file) return;
    try {
      const text = await file.text();
      const imported = JSON.parse(text);
      if (!Array.isArray(imported)) throw new Error('Expected array');
      for (const p of imported) {
        if (p.name && p.adjustments) presets.push(p);
      }
      saveToStorage();
      renderPresetList();
    } catch (err) {
      alert('Invalid preset file: ' + err.message);
    }
    e.target.value = ''; // reset for re-import
  });
}
