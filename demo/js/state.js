// =====================================================================
// Shared application state and constants
// =====================================================================

export const OVERVIEW_MAX = 512;
export const DETAIL_MAX = 800;
export const RENDER_DEBOUNCE_MS = 50;

export const state = {
  sourceImage: null,
  sourceWidth: 0,
  sourceHeight: 0,
  region: { x: 0.25, y: 0.25, w: 0.5, h: 0.5 },
  adjustments: {},
  filmPreset: null,
  filmPresetIntensity: 1.0,
  sliderNodes: [],
  overviewRenderId: 0,
  detailRenderId: 0,
  lastChangedSliderKey: null,
  touchedSliders: new Set(),
  lastSafeAdjustments: {},
};

export const $ = id => document.getElementById(id);

/**
 * Build nested adjustment format for the worker:
 * { "zenfilters.exposure": { "stops": 1.5 }, ... }
 * Only includes a node if at least one param differs from its identity.
 *
 * Handles three param kinds:
 * - number: value is a float
 * - boolean: value is true/false
 * - array_element: individual array indices are reassembled into arrays
 */
export function getFilterAdjustments() {
  const adj = {};
  for (const node of state.sliderNodes) {
    const nodeParams = {};
    const arrays = {}; // paramName → { values: [...], anyChanged: bool }
    let anyChanged = false;

    for (const p of node.params) {
      const val = state.adjustments[p.adjustKey];

      if (p.kind === 'array_element') {
        // Collect array elements; assemble after the loop
        if (!arrays[p.arrayParam]) {
          arrays[p.arrayParam] = {
            values: new Array(p.arraySize).fill(0),
            identities: new Array(p.arraySize).fill(0),
            anyChanged: false,
          };
        }
        const arr = arrays[p.arrayParam];
        arr.values[p.arrayIndex] = val;
        arr.identities[p.arrayIndex] = p.identity;
        if (Math.abs(val - p.identity) > 1e-6) arr.anyChanged = true;
      } else if (p.kind === 'boolean') {
        nodeParams[p.paramName] = val;
        if (val !== p.identity) anyChanged = true;
      } else {
        nodeParams[p.paramName] = val;
        if (Math.abs(val - p.identity) > 1e-6) anyChanged = true;
      }
    }

    // Assemble arrays into the node params
    for (const [paramName, arr] of Object.entries(arrays)) {
      nodeParams[paramName] = arr.values;
      if (arr.anyChanged) anyChanged = true;
    }

    if (anyChanged) adj[node.id] = nodeParams;
  }
  return adj;
}
