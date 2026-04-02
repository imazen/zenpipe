// =====================================================================
// Film Preset Strip
// =====================================================================

import { $, state } from './state.js';
import { scheduleRender } from './render.js';
import { pushState } from './history.js';

const FILM_PRESETS = [
  { id: null,              name: 'None',             cat: 'none' },
  // Creative
  { id: 'bleach_bypass',   name: 'Bleach Bypass',    cat: 'creative' },
  { id: 'cross_process',   name: 'Cross Process',    cat: 'creative' },
  { id: 'teal_orange',     name: 'Teal Orange',      cat: 'creative' },
  { id: 'faded_film',      name: 'Faded Film',       cat: 'creative' },
  { id: 'golden_hour',     name: 'Golden Hour',      cat: 'creative' },
  { id: 'noir',            name: 'Noir',             cat: 'creative' },
  { id: 'technicolor',     name: 'Technicolor',      cat: 'creative' },
  { id: 'matte',           name: 'Matte',            cat: 'creative' },
  // Classic Negative
  { id: 'portra',          name: 'Portra',           cat: 'classic_neg' },
  { id: 'kodak_gold',      name: 'Kodak Gold',       cat: 'classic_neg' },
  { id: 'ektar',           name: 'Ektar',            cat: 'classic_neg' },
  { id: 'superia',         name: 'Superia',          cat: 'classic_neg' },
  { id: 'pro_400h',        name: 'Pro 400H',         cat: 'classic_neg' },
  // Slide
  { id: 'velvia',          name: 'Velvia',           cat: 'slide' },
  { id: 'provia',          name: 'Provia',           cat: 'slide' },
  { id: 'kodachrome',      name: 'Kodachrome',       cat: 'slide' },
  { id: 'ektachrome',      name: 'Ektachrome',       cat: 'slide' },
  // Motion Picture
  { id: 'print_2383',      name: 'Print 2383',       cat: 'motion' },
  { id: 'tungsten_500t',   name: 'Tungsten 500T',    cat: 'motion' },
  // Digital
  { id: 'classic_chrome',  name: 'Classic Chrome',   cat: 'digital' },
  { id: 'classic_neg',     name: 'Classic Neg',      cat: 'digital' },
  { id: 'cool_chrome',     name: 'Cool Chrome',      cat: 'digital' },
  // Cinematic
  { id: 'cyberpunk_neon',  name: 'Cyberpunk Neon',   cat: 'cinematic' },
  { id: 'desert_crush',    name: 'Desert Crush',     cat: 'cinematic' },
  { id: 'green_code',      name: 'Green Code',       cat: 'cinematic' },
  { id: 'french_whimsy',   name: 'French Whimsy',    cat: 'cinematic' },
  { id: 'arctic_light',    name: 'Arctic Light',     cat: 'cinematic' },
  { id: 'neon_noir',       name: 'Neon Noir',        cat: 'cinematic' },
  { id: 'dusty_americana', name: 'Dusty Americana',  cat: 'cinematic' },
  { id: 'moonlit_blue',    name: 'Moonlit Blue',     cat: 'cinematic' },
  { id: 'cold_case',       name: 'Cold Case',        cat: 'cinematic' },
  { id: 'desert_spice',    name: 'Desert Spice',     cat: 'cinematic' },
  { id: 'candy_pop',       name: 'Candy Pop',        cat: 'cinematic' },
  { id: 'blockbuster',     name: 'Blockbuster',      cat: 'cinematic' },
];

export function setActivePreset(presetId) {
  state.filmPreset = presetId;
  const chips = document.querySelectorAll('.preset-chip');
  chips.forEach(c => {
    const cid = c.dataset.presetId || null;
    c.classList.toggle('active', cid === (presetId || ''));
  });
}

export function buildPresetStrip() {
  const container = $('preset-chips');
  container.innerHTML = '';
  for (const preset of FILM_PRESETS) {
    const chip = document.createElement('span');
    chip.className = 'preset-chip';
    chip.dataset.presetId = preset.id || '';
    chip.dataset.cat = preset.cat;
    chip.textContent = preset.name;
    if (preset.id === null) chip.classList.add('active');
    chip.addEventListener('click', () => {
      setActivePreset(preset.id);
      pushState();
      scheduleRender();
    });
    container.appendChild(chip);
  }

  // Collapsible header
  const strip = $('preset-strip');
  strip.querySelector('h3').addEventListener('click', () => {
    strip.classList.toggle('collapsed');
  });

  // Intensity slider
  const intensitySlider = $('preset-intensity');
  const intensityVal = $('preset-intensity-val');
  intensitySlider.addEventListener('input', () => {
    state.filmPresetIntensity = parseFloat(intensitySlider.value);
    intensityVal.textContent = state.filmPresetIntensity.toFixed(2);
    pushState();
    if (state.filmPreset) scheduleRender();
  });
  intensitySlider.addEventListener('dblclick', () => {
    intensitySlider.value = 1;
    state.filmPresetIntensity = 1.0;
    intensityVal.textContent = '1.00';
    if (state.filmPreset) scheduleRender();
  });
}
