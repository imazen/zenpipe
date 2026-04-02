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
};

export const $ = id => document.getElementById(id);

/**
 * Build nested adjustment format for the worker:
 * { "zenfilters.exposure": { "stops": 1.5 }, ... }
 * Only includes a node if at least one param differs from its identity.
 */
export function getFilterAdjustments() {
  const adj = {};
  for (const node of state.sliderNodes) {
    const nodeParams = {};
    let anyChanged = false;
    for (const p of node.params) {
      const val = state.adjustments[p.adjustKey];
      nodeParams[p.paramName] = val;
      if (val !== p.identity) anyChanged = true;
    }
    if (anyChanged) adj[node.id] = nodeParams;
  }
  return adj;
}
