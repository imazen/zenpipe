// =====================================================================
// CSS Filter Approximation
// =====================================================================

import { $, getFilterAdjustments } from './state.js';

const CSS_FILTER_MAP = {
  'zenfilters.exposure': (params) => {
    const v = params.stops;
    return (v !== undefined && v !== 0) ? `brightness(${Math.pow(2, v).toFixed(3)})` : null;
  },
  'zenfilters.contrast': (params) => {
    const v = params.amount;
    return (v !== undefined && v !== 0) ? `contrast(${(1 + v).toFixed(3)})` : null;
  },
  'zenfilters.saturation': (params) => {
    const v = params.factor;
    return (v !== undefined && v !== 1) ? `saturate(${v.toFixed(3)})` : null;
  },
};

export function toCssFilter() {
  const parts = [];
  const adj = getFilterAdjustments();
  for (const [nodeId, params] of Object.entries(adj)) {
    const mapper = CSS_FILTER_MAP[nodeId];
    if (!mapper) continue;
    const css = mapper(params);
    if (css) parts.push(css);
  }
  return parts.length ? parts.join(' ') : 'none';
}

export function applyCssPreview() {
  const f = toCssFilter();
  $('overview-canvas').style.filter = f;
  $('detail-canvas').style.filter = f;
}

export function clearCssPreview() {
  $('overview-canvas').style.filter = 'none';
  $('detail-canvas').style.filter = 'none';
}
