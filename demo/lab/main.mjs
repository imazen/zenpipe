// =====================================================================
// Lab page wiring — contract shell UI over the controller (zenpipe#113)
// =====================================================================

import { Controller } from './controller.mjs';
import { ModelBackend } from './backend-model.mjs';
import {
  FIXTURE_BYTES,
  MODEL_OPS,
  generateFixture,
} from './fixture.mjs';

const $ = id => document.getElementById(id);

// Illustrative model estimate and declared reservation; neither is measured
// peak memory nor a proven bound.
const ESTIMATE_BYTES = 2 * FIXTURE_BYTES;
const RESERVE_BYTES = 3 * FIXTURE_BYTES;

// Media kinds offered in the picker but unsupported by every backend
// here. They must refuse as unqualified — never fake a success.
const UNQUALIFIED = [
  ['avif-sequence', 'AVIF sequence'],
  ['mp4-clip', 'MP4 clip'],
  ['vp8-clip', 'VP8 clip'],
  ['tiff-collection', 'TIFF collection'],
  ['jxl-layers', 'JXL layered frame'],
];

const backend = new ModelBackend();
const controller = new Controller({
  backend,
  cpuTokens: 2,
  byteBudget: 8 * RESERVE_BYTES,
  strict: true,
  onReceipt: renderReceipt,
  onOutput: showOutput,
  onDiagnostic: renderDiagnostic,
});

let fixture = generateFixture(0);
let tornDown = false;

// --------------------------- fixture ---------------------------------

function drawFrame(canvas, frame) {
  canvas.width = frame.width;
  canvas.height = frame.height;
  canvas
    .getContext('2d')
    .putImageData(new ImageData(frame.data, frame.width, frame.height), 0, 0);
}

function refreshFixture() {
  drawFrame($('fixture-canvas'), fixture);
  $('fixture-caption').textContent = `generation ${controller.generation}`;
}

$('new-gen-btn').addEventListener('click', () => {
  controller.advanceGeneration();
  fixture = generateFixture(controller.generation);
  refreshFixture();
  setStatus(`source generation ${controller.generation}`);
});

// --------------------------- controls --------------------------------

const opSelect = $('op-select');
{
  const model = document.createElement('optgroup');
  model.label = 'Model operations';
  for (const [kind, op] of Object.entries(MODEL_OPS)) {
    const o = document.createElement('option');
    o.value = kind;
    o.textContent = op.label;
    model.append(o);
  }
  const other = document.createElement('optgroup');
  other.label = 'Unqualified (no backend)';
  for (const [kind, label] of UNQUALIFIED) {
    const o = document.createElement('option');
    o.value = kind;
    o.textContent = label;
    other.append(o);
  }
  opSelect.append(model, other);
}

function nextEstimate() {
  return $('estimate-select').value === 'known' ? ESTIMATE_BYTES : null;
}

function renderEstimatePreview() {
  const est = nextEstimate();
  $('mem-estimate').innerHTML =
    est == null
      ? '<span class="unknown">unknown</span> — not zero; shown honestly, never a bound'
      : `model estimate: ${est.toLocaleString()} B (illustrative; not measured or a bound)`;
  $('reservation-preview').textContent =
    `declared reservation on submit: ${RESERVE_BYTES.toLocaleString()} B + 1 CPU token`;
}
$('estimate-select').addEventListener('change', renderEstimatePreview);
renderEstimatePreview();

$('submit-btn').addEventListener('click', () => {
  if (tornDown) return setStatus('torn down — reload to restart');
  controller.strict = $('strict-check').checked;
  const view = controller.submit({
    kind: opSelect.value,
    payload: { frame: fixture },
    cpu: 1,
    estimateBytes: nextEstimate(),
    reserveBytes: RESERVE_BYTES,
  });
  setStatus(
    view.refusal
      ? `${view.opId} refused: ${view.refusal} — no effects`
      : `${view.opId} ${view.state}`,
  );
  renderTasks();
  renderUsage();
});

$('teardown-btn').addEventListener('click', () => {
  if (tornDown) return;
  tornDown = true;
  const report = controller.teardown();
  setStatus(
    report.unresolved.length
      ? `teardown: ${report.unresolved.length} task(s) unresolved — reservations still held`
      : 'teardown: all owned tasks settled',
  );
  renderTasks();
  renderUsage();
});

// --------------------------- rendering -------------------------------

function setStatus(text) {
  $('status').textContent = text;
}

function renderReceipt(r) {
  const li = document.createElement('li');
  li.textContent = `#${r.seq} ${r.opId ?? '—'} ${r.type}` +
    (r.reason ? ` (${r.reason})` : '');
  $('receipts').append(li);
  renderTasks();
  renderUsage();
}

function renderDiagnostic(d) {
  const li = document.createElement('li');
  li.textContent = `#${d.seq} ${d.opId ?? '—'} diag:${d.kind}`;
  $('receipts').append(li);
}

function showOutput(view, output) {
  drawFrame($('preview-canvas'), output);
  $('preview-caption').textContent =
    `${view.opId} · gen ${view.generation} · ${view.kind}`;
}

function renderTasks() {
  const tbody = $('task-body');
  tbody.textContent = '';
  const tasks = controller.listTasks();
  if (!tasks.length) {
    tbody.innerHTML = '<tr><td colspan="8" class="notes">none</td></tr>';
    return;
  }
  for (const t of tasks) {
    const tr = document.createElement('tr');
    const est = t.estimateBytes == null
      ? '<span class="unknown">unknown</span>'
      : `${t.estimateBytes.toLocaleString()} B`;
    const stateText = t.refusal
      ? `${t.state}: ${t.refusal}`
      : t.state + (t.cancelRequested && t.state === 'completed' ? ' (cancel requested)' : '');
    tr.innerHTML =
      `<td>${t.opId}</td><td>${t.kind}</td><td>${t.generation}</td>` +
      `<td class="st st-${t.state}">${stateText}</td>` +
      `<td>${est}</td>` +
      `<td>${t.reserveBytes == null ? '—' : t.reserveBytes.toLocaleString() + ' B'}</td>` +
      `<td>${t.cpu}</td>`;
    const td = document.createElement('td');
    if (['queued', 'admitted', 'running'].includes(t.state)) {
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'btn';
      b.textContent = 'Cancel';
      b.setAttribute('aria-label', `Cancel ${t.opId}`);
      b.addEventListener('click', () => {
        controller.cancel(t.opId);
        renderTasks();
        renderUsage();
      });
      td.append(b);
    }
    tr.append(td);
    tbody.append(tr);
  }
}

function renderUsage() {
  const u = controller.usage();
  $('mem-reserved').textContent =
    `${u.byteUsed.toLocaleString()} B / ${u.byteTotal.toLocaleString()} B`;
  $('mem-cpu').textContent = `${u.cpuUsed} / ${u.cpuTotal}`;
}

// Diagnostics hook for the Playwright suite and manual inspection.
window.__lab = { controller, backend, generateFixture };

refreshFixture();
renderUsage();
