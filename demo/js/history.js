// =====================================================================
// Undo / Redo History
// =====================================================================
//
// State snapshots are JSON blobs of { adjustments, filmPreset, filmPresetIntensity }.
// Push on every meaningful change (debounced 300ms to coalesce rapid slider drags).
// Ctrl+Z = undo, Ctrl+Shift+Z / Ctrl+Y = redo.

import { state } from './state.js';

const MAX_HISTORY = 100;
const DEBOUNCE_MS = 300;

let undoStack = [];
let redoStack = [];
let debounceId = null;
let restoreCallback = null; // set by caller to apply snapshot

/** Take a snapshot of the current edit state. */
function snapshot() {
  return {
    adjustments: { ...state.adjustments },
    filmPreset: state.filmPreset,
    filmPresetIntensity: state.filmPresetIntensity,
  };
}

/** Apply a snapshot to the current state. */
function applySnapshot(snap) {
  // Restore adjustments
  for (const key of Object.keys(state.adjustments)) {
    delete state.adjustments[key];
  }
  Object.assign(state.adjustments, snap.adjustments);
  state.filmPreset = snap.filmPreset;
  state.filmPresetIntensity = snap.filmPresetIntensity;
  if (restoreCallback) restoreCallback();
}

/**
 * Initialize the history system.
 * @param {Function} onRestore — called after undo/redo applies a snapshot.
 *   Should sync DOM sliders and re-render.
 */
export function initHistory(onRestore) {
  restoreCallback = onRestore;
  // Push initial state
  undoStack = [snapshot()];
  redoStack = [];

  document.addEventListener('keydown', (e) => {
    // Ctrl+Z (undo) or Cmd+Z on Mac
    if ((e.ctrlKey || e.metaKey) && e.key === 'z' && !e.shiftKey) {
      e.preventDefault();
      undo();
    }
    // Ctrl+Shift+Z or Ctrl+Y (redo)
    if ((e.ctrlKey || e.metaKey) && (e.key === 'Z' || e.key === 'y') && (e.shiftKey || e.key === 'y')) {
      e.preventDefault();
      redo();
    }
  });
}

/**
 * Record a state change (debounced).
 * Call after any slider/preset/reset interaction.
 */
export function pushState() {
  if (debounceId) clearTimeout(debounceId);
  debounceId = setTimeout(() => {
    debounceId = null;
    const snap = snapshot();
    // Skip if identical to top of undo stack
    const top = undoStack[undoStack.length - 1];
    if (top && JSON.stringify(top) === JSON.stringify(snap)) return;
    undoStack.push(snap);
    if (undoStack.length > MAX_HISTORY) undoStack.shift();
    // New change clears redo stack
    redoStack.length = 0;
  }, DEBOUNCE_MS);
}

/** Undo to previous state. */
export function undo() {
  if (undoStack.length <= 1) return; // nothing to undo (first entry is initial state)
  // Flush any pending debounced push
  if (debounceId) { clearTimeout(debounceId); debounceId = null; }
  // Save current as redo point
  redoStack.push(snapshot());
  // Pop the current, apply previous
  undoStack.pop();
  applySnapshot(undoStack[undoStack.length - 1]);
}

/** Redo previously undone state. */
export function redo() {
  if (redoStack.length === 0) return;
  if (debounceId) { clearTimeout(debounceId); debounceId = null; }
  const snap = redoStack.pop();
  undoStack.push(snap);
  applySnapshot(snap);
}

/** Reset history (e.g., when loading a new image). */
export function resetHistory() {
  undoStack = [snapshot()];
  redoStack = [];
  if (debounceId) { clearTimeout(debounceId); debounceId = null; }
}

/** Current undo/redo stack depths (for UI indicators). */
export function historyDepth() {
  return { undo: undoStack.length - 1, redo: redoStack.length };
}
