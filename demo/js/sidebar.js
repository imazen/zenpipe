// =====================================================================
// Schema-Driven Sidebar Generation
// =====================================================================

import { $, state } from './state.js';
import { scheduleRender } from './render.js';

/** Group display order and labels. */
const GROUP_ORDER = [
  'tone', 'tone_range', 'tone_map', 'color', 'detail', 'effects',
  'local', 'creative', 'lens', 'auto',
];
const GROUP_LABELS = {
  tone: 'Tone', tone_range: 'Tone Range', tone_map: 'Tone Map',
  color: 'Color', detail: 'Detail', effects: 'Effects',
  local: 'Local', creative: 'Creative', lens: 'Lens', auto: 'Auto',
};

export function formatVal(v) {
  return Math.abs(v) >= 10 ? v.toFixed(0) : v.toFixed(2);
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
  // Remove old slider groups but preserve the preset strip
  sidebar.querySelectorAll('.slider-group').forEach(el => el.remove());

  // Discover all filter nodes from schema
  const groups = new Map();
  for (const [nodeId, def] of Object.entries(defs)) {
    if (!nodeId.startsWith('zenfilters.')) continue;
    if (def['x-zennode-role'] !== 'filter') continue;

    const group = def['x-zennode-group'] || 'other';
    if (!groups.has(group)) groups.set(group, []);

    const params = [];
    for (const [paramName, paramSchema] of Object.entries(def.properties || {})) {
      if (paramSchema.type !== 'number' && paramSchema.type !== 'integer') continue;
      const min = paramSchema.minimum ?? 0;
      const max = paramSchema.maximum ?? 1;
      const step = paramSchema['x-zennode-step'] ?? 0.05;
      const defaultVal = paramSchema.default ?? 0;
      const identity = paramSchema['x-zennode-identity'] ?? defaultVal;
      const label = paramSchema.title || paramName;

      // adjustKey stores nodeId + paramName for internal tracking
      const adjustKey = nodeId + '.' + paramName;
      state.adjustments[adjustKey] = identity;
      params.push({ paramName, label, min, max, step, defaultVal, identity, adjustKey });
    }

    if (params.length > 0) {
      groups.get(group).push({
        id: nodeId,
        title: def.title || nodeId,
        params,
      });
    }
  }

  // Sort nodes within each group alphabetically by title
  for (const nodes of groups.values()) {
    nodes.sort((a, b) => a.title.localeCompare(b.title));
  }

  // Render groups in order
  for (const groupKey of GROUP_ORDER) {
    const nodes = groups.get(groupKey);
    if (!nodes || nodes.length === 0) continue;

    const groupEl = document.createElement('div');
    const isCollapsed = groupKey !== 'tone';
    groupEl.className = 'slider-group' + (isCollapsed ? ' collapsed' : '');
    groupEl.innerHTML = `<h3><span class="collapse-icon">&#x25B8;</span>${GROUP_LABELS[groupKey] || groupKey}</h3>`;

    // Collapsible header toggle
    groupEl.querySelector('h3').addEventListener('click', () => {
      groupEl.classList.toggle('collapsed');
    });

    for (const node of nodes) {
      state.sliderNodes.push(node);
      for (const param of node.params) {
        const row = document.createElement('div');
        row.className = 'slider-row';
        row.innerHTML = `
          <label title="${node.title}: ${param.paramName}">${param.label}</label>
          <input type="range" min="${param.min}" max="${param.max}"
                 step="${param.step}" value="${param.identity}"
                 data-key="${param.adjustKey}" data-identity="${param.identity}">
          <span class="val">${formatVal(param.identity)}</span>
          <button class="slider-reset" title="Reset to default">&#x21BA;</button>
        `;
        const slider = row.querySelector('input');
        const display = row.querySelector('.val');
        const resetBtn = row.querySelector('.slider-reset');

        function updateResetVisibility() {
          const isIdentity = parseFloat(slider.value) === param.identity;
          resetBtn.classList.toggle('visible', state.touchedSliders.has(param.adjustKey) && !isIdentity);
        }

        slider.addEventListener('input', () => {
          state.touchedSliders.add(param.adjustKey);
          state.lastChangedSliderKey = param.adjustKey;
          state.adjustments[param.adjustKey] = parseFloat(slider.value);
          display.textContent = formatVal(state.adjustments[param.adjustKey]);
          updateResetVisibility();
          scheduleRender();
        });

        // Double-click to reset
        slider.addEventListener('dblclick', () => {
          slider.value = param.identity;
          state.adjustments[param.adjustKey] = param.identity;
          display.textContent = formatVal(param.identity);
          state.touchedSliders.delete(param.adjustKey);
          updateResetVisibility();
          scheduleRender();
        });

        // Reset button click
        resetBtn.addEventListener('click', () => {
          slider.value = param.identity;
          state.adjustments[param.adjustKey] = param.identity;
          display.textContent = formatVal(param.identity);
          state.touchedSliders.delete(param.adjustKey);
          updateResetVisibility();
          scheduleRender();
        });

        updateResetVisibility();
        groupEl.appendChild(row);
      }
    }
    sidebar.appendChild(groupEl);
  }
}
