// =====================================================================
// Schema-Driven Sidebar Generation
// =====================================================================

import { $, state } from './state.js';
import { scheduleRender } from './render.js';

/** Group display order and labels. */
const GROUP_ORDER = [
  'favorites', 'tone', 'tone_range', 'tone_map', 'color', 'detail', 'effects',
  'local', 'creative', 'lens', 'auto',
];
const GROUP_LABELS = {
  favorites: 'Favorites', tone: 'Tone', tone_range: 'Tone Range', tone_map: 'Tone Map',
  color: 'Color', detail: 'Detail', effects: 'Effects',
  local: 'Local', creative: 'Creative', lens: 'Lens', auto: 'Auto',
};

/** Favorite filter nodes — shown in a pinned expanded group at the top. */
const FAVORITE_NODE_IDS = new Set([
  'zenfilters.exposure',
  'zenfilters.contrast',
  'zenfilters.highlights_shadows',
  'zenfilters.clarity',
  'zenfilters.brilliance',
  'zenfilters.saturation',
  'zenfilters.vibrance',
  'zenfilters.sharpen',
  'zenfilters.temperature',
  'zenfilters.dehaze',
  'zenfilters.vignette',
]);

/** Nodes hidden from the UI — internal, require custom editors, or handled elsewhere. */
const HIDDEN_NODE_IDS = new Set([
  'zenfilters.fused_adjust',       // Internal optimization (coalesced version of individual filters)
  'zenfilters.color_matrix',       // 5×5 matrix — needs custom matrix editor
  'zenfilters.asc_cdl',            // 10 technical params for DI colorists
  'zenfilters.cube_lut',           // Needs file upload for LUT data
  'zenfilters.hue_curves',         // No slider params (curves set programmatically)
  'zenfilters.film_look',          // Handled by the preset strip
  'zenfilters.tone_curve',         // Needs curve editor (string param)
  'zenfilters.channel_curves',     // Needs curve editor (string params)
  'zenfilters.basecurve_tonemap',  // Needs preset selector (string param)
  'zenfilters.grayscale',          // Needs dropdown for algorithm
]);

/** Sections that are collapsed by default. "Main" is always visible. */
const COLLAPSED_SECTIONS = new Set([
  'Advanced', 'Masking', 'Splits', 'Shape', 'advanced',
]);

export function formatVal(v, unit) {
  const num = Math.abs(v) >= 10 ? v.toFixed(0)
    : Math.abs(v) >= 1 ? v.toFixed(2)
    : v.toFixed(2);
  if (!unit) return num;
  // Units that attach directly (no space)
  if (unit === '°' || unit === '×' || unit === '%') return num + unit;
  return num + '\u2009' + unit; // thin space before unit
}

// ─── Slider type mappings ───

/**
 * Convert slider position → filter value.
 * For square_from_slider: finer control near identity, expanding quadratically.
 */
function posToValue(pos, sliderType, min, max, identity) {
  if (sliderType === 'square_from_slider') {
    if (pos >= identity) {
      const range = max - identity;
      if (range <= 0) return identity;
      const t = (pos - identity) / range; // 0..1
      return identity + t * t * range;
    } else {
      const range = identity - min;
      if (range <= 0) return identity;
      const t = (identity - pos) / range; // 0..1
      return identity - t * t * range;
    }
  }
  return pos; // linear, factor_centered, logarithmic — use position directly
}

/**
 * Convert filter value → slider position.
 * Inverse of posToValue.
 */
function valueToPos(value, sliderType, min, max, identity) {
  if (sliderType === 'square_from_slider') {
    if (value >= identity) {
      const range = max - identity;
      if (range <= 0) return identity;
      const t = Math.sqrt((value - identity) / range);
      return identity + t * range;
    } else {
      const range = identity - min;
      if (range <= 0) return identity;
      const t = Math.sqrt((identity - value) / range);
      return identity - t * range;
    }
  }
  return value;
}

// ─── Array param helpers ───

/** Compute a reasonable step for array element sliders from the item range. */
function computeArrayStep(min, max) {
  const range = max - min;
  if (range <= 0) return 0.01;
  const raw = range / 50;
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const norm = raw / mag;
  if (norm < 2) return mag;
  if (norm < 5) return 2 * mag;
  return 5 * mag;
}

// ─── Schema parsing ───

/**
 * Parse a param definition from JSON schema into a normalized param descriptor.
 * Returns null for unsupported param types (strings without labels, etc.)
 */
function parseParam(paramName, paramSchema, nodeId) {
  const type = paramSchema.type;

  // Number / integer → slider
  if (type === 'number' || type === 'integer') {
    const min = paramSchema.minimum ?? 0;
    const max = paramSchema.maximum ?? 1;
    const step = paramSchema['x-zennode-step'] ?? 0.05;
    const defaultVal = paramSchema.default ?? 0;
    const identity = paramSchema['x-zennode-identity'] ?? defaultVal;
    const label = paramSchema.title || paramName;
    const unit = paramSchema['x-zennode-unit'] || '';
    const section = paramSchema['x-zennode-section'] || 'Main';
    const slider = paramSchema['x-zennode-slider'] || 'linear';

    if (slider === 'not_slider') return null; // skip non-slider numeric params

    const visibleWhen = paramSchema['x-zennode-visible-when'] || null;
    const adjustKey = nodeId + '.' + paramName;
    return {
      kind: 'number', paramName, label, min, max, step, defaultVal,
      identity, adjustKey, unit, section, slider, visibleWhen,
    };
  }

  // Boolean → checkbox
  if (type === 'boolean') {
    const defaultVal = paramSchema.default ?? false;
    const label = paramSchema.title || paramName;
    const section = paramSchema['x-zennode-section'] || 'Main';
    const adjustKey = nodeId + '.' + paramName;
    return {
      kind: 'boolean', paramName, label, defaultVal,
      identity: defaultVal, adjustKey, unit: '', section, slider: 'linear',
    };
  }

  // Array with labels → expand to individual sliders
  if (type === 'array' && paramSchema['x-zennode-labels']?.length > 0) {
    const labels = paramSchema['x-zennode-labels'];
    const defaults = paramSchema.default || [];
    const items = paramSchema.items || {};
    const min = items.minimum ?? 0;
    const max = items.maximum ?? 1;
    const step = computeArrayStep(min, max);
    const unit = paramSchema['x-zennode-unit'] || '';
    const section = paramSchema['x-zennode-section'] || 'Main';
    const slider = 'linear'; // Array elements always use linear sliders

    const elements = [];
    for (let i = 0; i < labels.length; i++) {
      const identity = defaults[i] ?? 0;
      const adjustKey = nodeId + '.' + paramName + '[' + i + ']';
      elements.push({
        kind: 'array_element', paramName, label: labels[i],
        min, max, step, defaultVal: identity, identity,
        adjustKey, unit, section, slider,
        arrayParam: paramName, arrayIndex: i, arraySize: labels.length,
      });
    }
    return elements; // Return array of params
  }

  return null; // Unsupported type (string, unlabeled array, etc.)
}

/**
 * Sync all DOM slider/checkbox elements to match current state.adjustments.
 * Called after bulk state changes (reset, error recovery).
 */
export function syncDOMToState() {
  for (const node of state.sliderNodes) {
    for (const p of node.params) {
      const val = state.adjustments[p.adjustKey];
      if (val === undefined) continue;

      if (p.kind === 'boolean') {
        const cb = document.querySelector(`input[type="checkbox"][data-key="${p.adjustKey}"]`);
        if (cb) cb.checked = !!val;
      } else {
        const slider = document.querySelector(`input[type="range"][data-key="${p.adjustKey}"]`);
        if (!slider) continue;
        const row = slider.closest('.slider-row');
        const display = row?.querySelector('.val');
        const resetBtn = row?.querySelector('.slider-reset');

        const pos = valueToPos(val, p.slider, p.min, p.max, p.identity);
        slider.value = pos;
        if (display) display.textContent = formatVal(val, p.unit);
        if (resetBtn) {
          const isIdentity = Math.abs(val - p.identity) < 1e-6;
          resetBtn.classList.toggle('visible', state.touchedSliders.has(p.adjustKey) && !isIdentity);
        }
        if (row) row.classList.remove('slider-disabled');
      }
    }
  }
}

export async function loadSchemaAndBuildUI() {
  let schema;
  try {
    const resp = await fetch('schema.json');
    schema = await resp.json();
  } catch {
    const errP = document.createElement('p');
    errP.style.cssText = 'color:var(--text-dim);padding:8px';
    errP.textContent = 'Failed to load schema';
    $('sidebar').appendChild(errP);
    return;
  }

  const defs = schema.$defs || {};
  const sidebar = $('sidebar');
  // Remove old slider groups but preserve the preset strip and reset button
  sidebar.querySelectorAll('.slider-group').forEach(el => el.remove());

  // Discover all filter nodes from schema
  const groups = new Map();
  for (const [nodeId, def] of Object.entries(defs)) {
    if (!nodeId.startsWith('zenfilters.')) continue;
    if (def['x-zennode-role'] !== 'filter') continue;
    if (HIDDEN_NODE_IDS.has(nodeId)) continue;

    // Parse all params for this node
    const params = [];
    for (const [paramName, paramSchema] of Object.entries(def.properties || {})) {
      const parsed = parseParam(paramName, paramSchema, nodeId);
      if (!parsed) continue;
      // parseParam returns either a single param or an array (for array params)
      if (Array.isArray(parsed)) {
        params.push(...parsed);
      } else {
        params.push(parsed);
      }
    }

    if (params.length === 0) continue;

    // Initialize state for all params
    for (const p of params) {
      if (p.kind === 'boolean') {
        state.adjustments[p.adjustKey] = p.identity;
      } else {
        state.adjustments[p.adjustKey] = p.identity;
      }
    }

    // Favorites go to the pinned group; others go to their schema group.
    const isFavorite = FAVORITE_NODE_IDS.has(nodeId);
    const group = isFavorite ? 'favorites' : (def['x-zennode-group'] || 'other');
    if (!groups.has(group)) groups.set(group, []);

    groups.get(group).push({
      id: nodeId,
      title: def.title || nodeId,
      params,
    });
  }

  // Sort nodes within each group
  const FAVORITE_ORDER = [...FAVORITE_NODE_IDS];
  for (const [groupKey, nodes] of groups.entries()) {
    if (groupKey === 'favorites') {
      nodes.sort((a, b) => FAVORITE_ORDER.indexOf(a.id) - FAVORITE_ORDER.indexOf(b.id));
    } else {
      nodes.sort((a, b) => a.title.localeCompare(b.title));
    }
  }

  // Render groups in order
  for (const groupKey of GROUP_ORDER) {
    const nodes = groups.get(groupKey);
    if (!nodes || nodes.length === 0) continue;

    const groupEl = document.createElement('div');
    const isCollapsed = groupKey !== 'favorites';
    groupEl.className = 'slider-group' + (isCollapsed ? ' collapsed' : '');
    groupEl.innerHTML = `<h3><span class="collapse-icon">&#x25B8;</span>${GROUP_LABELS[groupKey] || groupKey}</h3>`;

    groupEl.querySelector('h3').addEventListener('click', () => {
      groupEl.classList.toggle('collapsed');
    });

    for (const node of nodes) {
      state.sliderNodes.push(node);
      renderNode(node, groupEl);
    }
    sidebar.appendChild(groupEl);
  }
}

// ─── Node rendering ───

function renderNode(node, container) {
  const isFavorite = FAVORITE_NODE_IDS.has(node.id);

  // Group params by section
  const sectionMap = new Map();
  for (const param of node.params) {
    const sec = param.section || 'Main';
    if (!sectionMap.has(sec)) sectionMap.set(sec, []);
    sectionMap.get(sec).push(param);
  }

  // For favorites with only "Main" section and 1-2 params, keep it flat
  const sections = [...sectionMap.keys()];
  const onlyMain = sections.length === 1 && sections[0] === 'Main';
  const mainParams = sectionMap.get('Main') || [];
  const isSingleParam = node.params.length === 1 && onlyMain;

  // Node header for multi-param nodes
  if (!isSingleParam) {
    const header = document.createElement('div');
    header.className = 'node-header';
    header.textContent = node.title;
    container.appendChild(header);
  }

  // Render sections
  for (const [sectionName, params] of sectionMap) {
    const isMainSection = sectionName === 'Main';
    const shouldCollapse = !isMainSection && (COLLAPSED_SECTIONS.has(sectionName) || !isFavorite);

    // Section wrapper
    let sectionEl;
    if (!onlyMain) {
      sectionEl = document.createElement('div');
      sectionEl.className = 'param-section' + (shouldCollapse ? ' section-collapsed' : '');

      // Non-Main sections get a toggle header
      if (!isMainSection) {
        const secHeader = document.createElement('div');
        secHeader.className = 'section-toggle';
        secHeader.innerHTML = `<span class="section-icon">&#x25B8;</span>${sectionName}`;
        secHeader.addEventListener('click', () => {
          sectionEl.classList.toggle('section-collapsed');
        });
        sectionEl.appendChild(secHeader);
      }
      container.appendChild(sectionEl);
    } else {
      sectionEl = container;
    }

    for (const param of params) {
      if (param.kind === 'boolean') {
        renderCheckbox(param, sectionEl, node, isSingleParam);
      } else {
        renderSlider(param, sectionEl, node, isSingleParam);
      }
    }
  }
}

function renderSlider(param, container, node, isSingleParam) {
  const displayLabel = isSingleParam ? node.title : param.label;
  const identityPos = valueToPos(param.identity, param.slider, param.min, param.max, param.identity);

  const row = document.createElement('div');
  row.className = 'slider-row';
  row.innerHTML = `
    <div class="slider-label-line">
      <label title="${node.title}: ${param.paramName}">${displayLabel}</label>
      <span class="val-reset">
        <span class="val">${formatVal(param.identity, param.unit)}</span>
        <button class="slider-reset" title="Reset to default">&#x21BA;</button>
      </span>
    </div>
    <input type="range" min="${param.min}" max="${param.max}"
           step="${param.step}" value="${identityPos}"
           data-key="${param.adjustKey}" data-identity="${param.identity}">
  `;
  const slider = row.querySelector('input[type="range"]');
  const display = row.querySelector('.val');
  const resetBtn = row.querySelector('.slider-reset');

  function updateValue() {
    const pos = parseFloat(slider.value);
    const value = posToValue(pos, param.slider, param.min, param.max, param.identity);
    state.adjustments[param.adjustKey] = value;
    display.textContent = formatVal(value, param.unit);
  }

  function updateResetVisibility() {
    const pos = parseFloat(slider.value);
    const value = posToValue(pos, param.slider, param.min, param.max, param.identity);
    const isIdentity = Math.abs(value - param.identity) < 1e-6;
    resetBtn.classList.toggle('visible', state.touchedSliders.has(param.adjustKey) && !isIdentity);
  }

  slider.addEventListener('input', () => {
    state.touchedSliders.add(param.adjustKey);
    state.lastChangedSliderKey = param.adjustKey;
    updateValue();
    updateResetVisibility();
    scheduleRender();
  });

  slider.addEventListener('dblclick', () => {
    resetSlider(slider, param, display, resetBtn);
  });

  resetBtn.addEventListener('click', () => {
    resetSlider(slider, param, display, resetBtn);
  });

  updateResetVisibility();
  container.appendChild(row);
}

function resetSlider(slider, param, display, resetBtn) {
  const identityPos = valueToPos(param.identity, param.slider, param.min, param.max, param.identity);
  slider.value = identityPos;
  state.adjustments[param.adjustKey] = param.identity;
  display.textContent = formatVal(param.identity, param.unit);
  state.touchedSliders.delete(param.adjustKey);
  resetBtn.classList.toggle('visible', false);
  scheduleRender();
}

function renderCheckbox(param, container, node, isSingleParam) {
  const displayLabel = isSingleParam ? node.title : param.label;

  const row = document.createElement('div');
  row.className = 'checkbox-row';
  row.innerHTML = `
    <label class="checkbox-label">
      <input type="checkbox" ${param.identity ? 'checked' : ''}
             data-key="${param.adjustKey}" data-identity="${param.identity}">
      <span>${displayLabel}</span>
    </label>
  `;
  const checkbox = row.querySelector('input[type="checkbox"]');

  checkbox.addEventListener('change', () => {
    state.touchedSliders.add(param.adjustKey);
    state.lastChangedSliderKey = param.adjustKey;
    state.adjustments[param.adjustKey] = checkbox.checked;
    scheduleRender();
  });

  container.appendChild(row);
}
