// =====================================================================
// Mechanical oracle for the contract-shell controller (zenpipe#113)
//
// Node built-in tests (`node --test`) driving a scripted fake backend —
// plus the real ModelBackend under a manual scheduler. Every assertion
// targets state, backend effects and reservations, not happy-path values.
// Run: node --test demo/tests/media-suite-controller.node.mjs
// =====================================================================

import test from 'node:test';
import assert from 'node:assert/strict';
import { Controller, TaskState, SubmitRefusal } from '../lab/controller.mjs';
import { ModelBackend } from '../lab/backend-model.mjs';
import {
  FIXTURE_BYTES,
  fixturePixel,
  generateFixture,
  applyModelOp,
} from '../lab/fixture.mjs';

// Fully scripted backend: records effects, emits events only when the
// test drives them. `disposeEmits` controls whether dispose() reports
// exits for live ops (false = terminated worker that says nothing).
class ScriptedBackend {
  constructor({ identity = 'model', disposeEmits = false } = {}) {
    this.identity = identity;
    this.disposeEmits = disposeEmits;
    this.submitted = []; // { opId, generation, kind, payload }
    this.cancelSignals = []; // opIds signalled
    this.disposed = 0;
    this.unsupported = new Set();
    this.onSubmit = null; // optional hook run inside submit()
    this._emit = () => {};
  }
  attach(emit) {
    this._emit = emit;
  }
  supports(kind) {
    return !this.unsupported.has(kind);
  }
  submit(spec) {
    this.onSubmit?.(spec, this);
    this.submitted.push(spec);
  }
  cancel(opId) {
    this.cancelSignals.push(opId);
  }
  dispose() {
    this.disposed++;
    if (this.disposeEmits) {
      for (const spec of this.submitted.splice(0)) {
        this._emit({
          type: 'cancelled',
          opId: spec.opId,
          generation: spec.generation,
          backend: this.identity,
        });
      }
    }
  }
  // --- test drivers ---
  send(ev) {
    this._emit({ backend: this.identity, ...ev });
  }
  started(opId, generation = 0) {
    this.send({ type: 'started', opId, generation });
  }
  complete(opId, generation = 0, output = null) {
    this.send({ type: 'completed', opId, generation, output });
  }
  fail(opId, generation = 0, error = 'boom') {
    this.send({ type: 'failed', opId, generation, error });
  }
  cancelled(opId, generation = 0) {
    this.send({ type: 'cancelled', opId, generation });
  }
}

const SPEC = { kind: 'invert', cpu: 1, estimateBytes: 100, reserveBytes: 1000 };

function makeCtl(overrides = {}) {
  const backend = new ScriptedBackend(overrides.backendOpts ?? {});
  const outputs = [];
  const ctl = new Controller({
    backend,
    cpuTokens: 2,
    byteBudget: 8000,
    strict: true,
    onOutput: (v, o) => outputs.push({ view: v, output: o }),
    ...overrides.ctl,
  });
  return { ctl, backend, outputs };
}

function task(ctl, opId) {
  return ctl.listTasks().find(t => t.opId === opId);
}

/** Whitebox handle for payload-retention assertions (internals only). */
function internal(ctl, opId) {
  return ctl._tasks.get(opId);
}

// ----------------------------------------------------------------------

test('fixture is deterministic and ops are exact', () => {
  const a = generateFixture(0);
  const b = generateFixture(0);
  assert.deepEqual(a.data, b.data, 'same seed must give identical bytes');
  const c = generateFixture(1);
  assert.notDeepEqual(c.data, a.data, 'a new generation must change pixels');

  const at = (f, x, y) => [...f.data.slice((y * f.width + x) * 4, (y * f.width + x) * 4 + 4)];
  assert.deepEqual(at(a, 0, 0), fixturePixel(0, 0, 0));
  assert.deepEqual(at(a, 95, 63), fixturePixel(95, 63, 0));

  const inv = applyModelOp('invert', a);
  assert.deepEqual(at(inv, 0, 0), fixturePixel(0, 0, 0).map((v, i) => (i === 3 ? v : 255 - v)));
  const swap = applyModelOp('swap-rb', a);
  const [r, g, bl, al] = at(a, 7, 3);
  assert.deepEqual(at(swap, 7, 3), [bl, g, r, al]);
  assert.equal(at(applyModelOp('fade', a), 0, 0)[3], 127);
});

test('constructor rejects non-integer budgets', () => {
  for (const cpuTokens of [0, -1, 1.5, NaN, Infinity]) {
    assert.throws(
      () => new Controller({ backend: new ScriptedBackend(), cpuTokens }),
      RangeError,
      `cpuTokens=${cpuTokens}`,
    );
  }
  for (const byteBudget of [-1, 0.5, NaN, Infinity]) {
    assert.throws(
      () => new Controller({ backend: new ScriptedBackend(), byteBudget }),
      RangeError,
      `byteBudget=${byteBudget}`,
    );
  }
  assert.doesNotThrow(
    () => new Controller({ backend: new ScriptedBackend(), cpuTokens: 1, byteBudget: 0 }),
  );
});

test('invalid spec values refuse before any backend effect', () => {
  const bad = [
    [{ ...SPEC, cpu: 0 }, 'cpu'],
    [{ ...SPEC, cpu: -2 }, 'cpu'],
    [{ ...SPEC, cpu: 1.5 }, 'cpu'],
    [{ ...SPEC, cpu: NaN }, 'cpu'],
    [{ ...SPEC, reserveBytes: -5 }, 'reserveBytes'],
    [{ ...SPEC, reserveBytes: 2.5 }, 'reserveBytes'],
    [{ ...SPEC, reserveBytes: NaN }, 'reserveBytes'],
    [{ ...SPEC, estimateBytes: -1 }, 'estimateBytes'],
    [{ ...SPEC, estimateBytes: 1.2 }, 'estimateBytes'],
    [{ ...SPEC, kind: '' }, 'kind'],
    [{ ...SPEC, kind: null }, 'kind'],
  ];
  const { ctl, backend } = makeCtl();
  for (const [spec, field] of bad) {
    const v = ctl.submit(spec);
    assert.equal(v.state, TaskState.FAILED, JSON.stringify(spec));
    assert.equal(v.refusal, 'invalid-spec');
  }
  assert.deepEqual(backend.submitted, [], 'no invalid spec reaches the backend');
  assert.equal(ctl.usage().byteUsed, 0);
  const fields = ctl.receipts().filter(r => r.type === 'refused').map(r => r.field);
  assert.deepEqual(fields, bad.map(([, f]) => f));
});

test('queued cancellation has no backend effects and reserves nothing', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1 } });
  const a = ctl.submit({ ...SPEC, payload: { frame: generateFixture(0) } });
  const b = ctl.submit({ ...SPEC, payload: { frame: generateFixture(0) } });
  assert.equal(task(ctl, a.opId).state, TaskState.ADMITTED);
  assert.equal(task(ctl, b.opId).state, TaskState.QUEUED);

  assert.equal(ctl.cancel(b.opId), true);
  assert.equal(task(ctl, b.opId).state, TaskState.CANCELLED);
  assert.deepEqual(backend.submitted.map(s => s.opId), [a.opId], 'queued task must never reach the backend');
  assert.deepEqual(backend.cancelSignals, [], 'no cancel signal for an unsubmitted task');
  assert.equal(internal(ctl, b.opId).payload, null, 'queued-cancel drops the caller payload');
  assert.equal(internal(ctl, a.opId).payload, null, 'admission hands the payload to the backend');
  assert.equal(task(ctl, b.opId).payloadSummary?.kind, 'frame', 'bounded summary is kept');
  assert.equal(ctl.usage().cpuUsed, 1, 'only the admitted task holds a token');
  assert.equal(ctl.usage().byteUsed, 1000);
});

test('queued-head cancellation admits the next fitting task', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 2, byteBudget: 2000 } });
  const a = ctl.submit({ ...SPEC, reserveBytes: 1500 }); // admitted, 500 free
  const big = ctl.submit({ ...SPEC, reserveBytes: 900 }); // queued: needs 900 > 500
  const small = ctl.submit({ ...SPEC, reserveBytes: 400 }); // queued behind big
  assert.equal(task(ctl, big.opId).state, TaskState.QUEUED);

  ctl.cancel(big.opId); // head leaves; small now fits the free 500
  assert.equal(task(ctl, small.opId).state, TaskState.ADMITTED);
  assert.deepEqual(
    backend.submitted.map(s => s.opId),
    [a.opId, small.opId],
    'cancelled head is never submitted; the next fitting task is',
  );
});

test('running cancellation holds the reservation until the terminal receipt', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1, byteBudget: 2000 } });
  const a = ctl.submit({ ...SPEC, reserveBytes: 1500 });
  backend.started(a.opId);
  assert.equal(task(ctl, a.opId).state, TaskState.RUNNING);

  ctl.cancel(a.opId);
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLING);
  assert.deepEqual(backend.cancelSignals, [a.opId], 'signal reaches the backend immediately');
  assert.equal(ctl.usage().cpuUsed, 1, 'reservation is held while cancelling');
  assert.equal(ctl.usage().byteUsed, 1500);

  // A cancel acknowledgement is not exit.
  backend.send({ type: 'cancel-ack', opId: a.opId, generation: 0 });
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLING);
  assert.equal(ctl.usage().byteUsed, 1500, 'ack must not release the reservation');

  backend.cancelled(a.opId);
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLED);
  assert.equal(ctl.usage().cpuUsed, 0);
  assert.equal(ctl.usage().byteUsed, 0);
});

test('every terminal kind releases its reservation exactly once', () => {
  for (const [emit, endState] of [
    ['complete', TaskState.COMPLETED],
    ['fail', TaskState.FAILED],
    ['cancelled', TaskState.CANCELLED],
  ]) {
    const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1 } });
    const a = ctl.submit(SPEC);
    backend.started(a.opId);
    ctl.cancel(a.opId); // even mid-cancel, the reported terminal applies
    backend[emit](a.opId, 0, { ok: true });
    assert.equal(task(ctl, a.opId).state, endState);
    assert.equal(ctl.usage().cpuUsed, 0, `${endState} releases cpu`);
    assert.equal(ctl.usage().byteUsed, 0, `${endState} releases bytes`);
  }
});

test('duplicate and stale terminal events never double-release or change state', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1 } });
  const a = ctl.submit(SPEC);
  backend.started(a.opId);
  backend.complete(a.opId, 0, { out: 1 });
  assert.equal(ctl.usage().cpuUsed, 0);
  const receiptsBefore = ctl.receipts().length;

  backend.complete(a.opId, 0, { out: 2 }); // duplicate terminal
  backend.fail(a.opId, 0); // contradicting second terminal
  assert.equal(task(ctl, a.opId).state, TaskState.COMPLETED);
  assert.equal(ctl.usage().cpuUsed, 0, 'no double-release');
  assert.equal(ctl.receipts().length, receiptsBefore, 'duplicates produce no state receipts');
  assert.equal(
    ctl.diagnostics().filter(d => d.kind === 'duplicate-terminal').length,
    2,
    'each duplicate is retained as diagnostic evidence',
  );

  // A terminal for an op the controller never issued is ignored entirely.
  backend.complete('op-999', 0, {});
  assert.equal(ctl.diagnostics().at(-1).kind, 'unknown-op');
});

test('stale-generation completion is rejected from preview but still releases', () => {
  const { ctl, backend, outputs } = makeCtl({ ctl: { cpuTokens: 1 } });
  const a = ctl.submit(SPEC);
  backend.started(a.opId);
  ctl.advanceGeneration(); // source moved on; the running task is superseded

  const frame = generateFixture(0);
  backend.complete(a.opId, 0, frame);
  assert.equal(task(ctl, a.opId).state, TaskState.COMPLETED);
  assert.equal(outputs.length, 0, 'superseded output must never preview');
  // Bounded summary retained for diagnostics; raw pixels are not.
  assert.deepEqual(task(ctl, a.opId).outputSummary, {
    kind: 'frame',
    width: frame.width,
    height: frame.height,
    bytes: frame.data.byteLength,
  });
  assert.equal(ctl.usage().cpuUsed, 0, 'stale completion still releases');
  const diag = ctl.diagnostics().find(d => d.kind === 'superseded-output' && d.opId === a.opId);
  assert.equal(diag.detail.bytes, frame.data.byteLength, 'diagnostic carries the bounded summary');
});

test('a newer requested op supersedes an older same-generation completion', () => {
  const { ctl, backend, outputs } = makeCtl();
  const slow = ctl.submit(SPEC); // preview op, then…
  const fast = ctl.submit(SPEC); // …a newer request takes preview ownership
  backend.started(slow.opId);
  backend.started(fast.opId);

  backend.complete(fast.opId, 0, { mine: 'fast' });
  assert.equal(outputs.length, 1);
  assert.equal(outputs[0].view.opId, fast.opId);

  backend.complete(slow.opId, 0, { mine: 'slow' });
  assert.equal(outputs.length, 1, 'the older same-generation output cannot displace the newer preview');
  assert.ok(ctl.diagnostics().some(d => d.kind === 'superseded-output' && d.opId === slow.opId));
});

test('delivery is rechecked at completion time, not before settlement', () => {
  // An observer that advances the generation during settle must not
  // receive the output computed against the pre-settle generation.
  const backend = new ScriptedBackend();
  const outputs = [];
  const ctl = new Controller({
    backend,
    cpuTokens: 1,
    byteBudget: 8000,
    strict: true,
    onReceipt: r => {
      if (r.type === 'completed') ctl.advanceGeneration();
    },
    onOutput: v => outputs.push(v),
  });
  const a = ctl.submit(SPEC);
  backend.started(a.opId);
  backend.complete(a.opId, 0, { out: 1 });
  assert.equal(outputs.length, 0, 'generation advanced mid-settle: no delivery');
  assert.ok(ctl.diagnostics().some(d => d.kind === 'superseded-output'));
});

test('current-generation completion previews; cancelling is isolated per task', () => {
  const { ctl, backend, outputs } = makeCtl();
  const a = ctl.submit(SPEC);
  const b = ctl.submit(SPEC);
  backend.started(a.opId);
  backend.started(b.opId);

  ctl.cancel(a.opId);
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLING);
  assert.equal(task(ctl, b.opId).state, TaskState.RUNNING, 'unrelated task stays live');
  assert.equal(ctl.usage().cpuUsed, 2, 'both reservations still held');

  backend.cancelled(a.opId);
  backend.complete(b.opId, 0, { img: 1 });
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLED);
  assert.equal(task(ctl, b.opId).state, TaskState.COMPLETED);
  assert.equal(outputs.length, 1);
  assert.equal(outputs[0].view.opId, b.opId);
  assert.equal(ctl.usage().cpuUsed, 0);
});

test('strict admission refuses an undeclared reservation, not an unknown estimate', () => {
  const { ctl, backend } = makeCtl();
  // Strict + declared reservation + unknown estimate: admitted. An
  // unknown empirical prediction is not a missing bound.
  const ok = ctl.submit({ kind: 'invert', cpu: 1, estimateBytes: null, reserveBytes: 500 });
  assert.equal(ok.state, TaskState.ADMITTED);
  assert.equal(task(ctl, ok.opId).estimateBytes, null, 'unknown stays unknown');
  assert.equal(ctl.usage().byteUsed, 500);

  // Strict + undeclared reservation: refused, no effects.
  const no = ctl.submit({ kind: 'invert', cpu: 1, estimateBytes: 100, reserveBytes: null });
  assert.equal(no.state, TaskState.FAILED);
  assert.equal(no.refusal, 'unbounded-reservation');
  assert.equal(backend.submitted.length, 1, 'only the admitted task reached the backend');
  assert.ok(ctl.receipts().some(r => r.type === 'refused' && r.reason === 'unbounded-reservation'));
});

test('non-strict admission books the whole budget for an undeclared reservation', () => {
  const { ctl, backend } = makeCtl({ ctl: { strict: false, byteBudget: 5000 } });
  const v = ctl.submit({ kind: 'invert', estimateBytes: null, reserveBytes: null });
  assert.equal(v.state, TaskState.ADMITTED);
  assert.equal(task(ctl, v.opId).reserveBytes, 5000, 'compatible alternative: whole-budget reservation');
  assert.equal(task(ctl, v.opId).unboundedReserve, true);
  assert.equal(ctl.usage().byteUsed, 5000);
  assert.equal(backend.submitted.length, 1);
});

test('impossible reservations refuse instead of queueing forever', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1, byteBudget: 4000 } });
  const fat = ctl.submit({ ...SPEC, reserveBytes: 4001 });
  assert.equal(fat.state, TaskState.FAILED);
  assert.equal(fat.refusal, 'impossible');
  const greedy = ctl.submit({ ...SPEC, cpu: 2 });
  assert.equal(greedy.refusal, 'impossible');
  assert.equal(ctl.listTasks().filter(t => t.state === TaskState.QUEUED).length, 0);
  assert.deepEqual(backend.submitted, []);
});

test('admission is FIFO and a freed reservation admits the head', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 2 } });
  const a = ctl.submit(SPEC);
  const b = ctl.submit(SPEC);
  const c = ctl.submit(SPEC);
  assert.deepEqual(
    backend.submitted.map(s => s.opId),
    [a.opId, b.opId],
    'only tasks that fit are submitted',
  );
  assert.equal(task(ctl, c.opId).state, TaskState.QUEUED);

  backend.complete(a.opId, 0, {});
  assert.equal(task(ctl, c.opId).state, TaskState.ADMITTED, 'release re-admits the queue head');
  assert.deepEqual(backend.submitted.map(s => s.opId), [a.opId, b.opId, c.opId]);

  // Head-of-line blocking: an oversized-but-possible head blocks the queue.
  const { ctl: c2, backend: b2 } = makeCtl({ ctl: { cpuTokens: 2, byteBudget: 3000 } });
  const big = c2.submit({ ...SPEC, reserveBytes: 2500 });
  const small = c2.submit({ ...SPEC, cpu: 1, reserveBytes: 400 });
  assert.equal(task(c2, big.opId).state, TaskState.ADMITTED);
  const big2 = c2.submit({ ...SPEC, reserveBytes: 2500 });
  const small2 = c2.submit({ ...SPEC, reserveBytes: 400 });
  assert.equal(task(c2, big2.opId).state, TaskState.QUEUED, 'head does not fit');
  assert.equal(task(c2, small2.opId).state, TaskState.QUEUED, 'strict FIFO: no skipping');
  assert.equal(b2.submitted.length, 2);
});

test('a backend error settles every owned task and refuses new submissions', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 2 } });
  const a = ctl.submit(SPEC);
  const b = ctl.submit(SPEC);
  const c = ctl.submit({ ...SPEC, cpu: 1 }); // queued behind a+b
  backend.started(a.opId);
  assert.equal(task(ctl, c.opId).state, TaskState.QUEUED);

  backend.send({ type: 'backend-error', error: 'worker exploded' });
  for (const opId of [a.opId, b.opId, c.opId]) {
    assert.equal(task(ctl, opId).state, TaskState.FAILED, `${opId} settled failed`);
  }
  assert.equal(ctl.usage().cpuUsed, 0);
  assert.equal(ctl.usage().byteUsed, 0);
  // Events from the dead backend afterwards are diagnostics, not releases.
  backend.complete(a.opId, 0, {});
  assert.equal(ctl.diagnostics().at(-1).kind, 'duplicate-terminal');
  // A dead backend admits nothing new — no forever-queue.
  const late = ctl.submit(SPEC);
  assert.equal(late.refusal, 'backend-dead');
  assert.equal(backend.submitted.length, 2);
});

test('a foreign-identity backend-error is untrusted evidence', () => {
  const { ctl, backend } = makeCtl();
  const a = ctl.submit(SPEC);
  backend.started(a.opId);
  backend.send({ type: 'backend-error', backend: 'someone-else', error: 'lie' });
  assert.equal(task(ctl, a.opId).state, TaskState.RUNNING);
  assert.equal(ctl.usage().cpuUsed, 1);
  assert.equal(ctl.diagnostics().at(-1).kind, 'foreign-backend-error');
});

test('an event for a queued task is unsolicited evidence, not a state change', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1 } });
  const a = ctl.submit(SPEC);
  const b = ctl.submit(SPEC); // queued behind a
  backend.complete(b.opId, 0, {}); // bogus: b was never submitted
  assert.equal(task(ctl, b.opId).state, TaskState.QUEUED);
  assert.equal(ctl.usage().cpuUsed, 1);
  assert.equal(ctl.diagnostics().at(-1).kind, 'event-for-queued-task');
  assert.deepEqual(backend.submitted.map(s => s.opId), [a.opId]);
});

test('SubmitRefusal is a clean no-effects refusal', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1 } });
  backend.onSubmit = () => {
    throw new SubmitRefusal('busy');
  };
  const a = ctl.submit(SPEC);
  assert.equal(task(ctl, a.opId).state, TaskState.FAILED);
  assert.equal(task(ctl, a.opId).error, 'busy');
  assert.equal(ctl.usage().cpuUsed, 0, 'no-effects refusal releases the reservation');
  assert.equal(ctl.usage().byteUsed, 0);
});

test('an uncertain submit throw retains ownership and requests cleanup', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1, byteBudget: 2000 } });
  backend.onSubmit = () => {
    throw new Error('harness blew up after maybe starting');
  };
  const a = ctl.submit({ ...SPEC, reserveBytes: 1500 });
  // Absence of 'started' does not prove absence of effects: the task is
  // held in cancelling with its reservation until a terminal receipt.
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLING);
  assert.equal(task(ctl, a.opId).uncertain, true);
  assert.deepEqual(backend.cancelSignals, [a.opId], 'cleanup was requested');
  assert.equal(ctl.usage().byteUsed, 1500, 'reservation is not released on uncertainty');
  assert.ok(ctl.diagnostics().some(d => d.kind === 'submit-threw-uncertain'));

  backend.cancelled(a.opId); // worker eventually reports exit
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLED);
  assert.equal(ctl.usage().byteUsed, 0);
});

test('a backend cannot throw SubmitRefusal after started to evade ownership', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1 } });
  backend.onSubmit = spec => {
    backend.started(spec.opId); // effects already happened
    throw new SubmitRefusal('too late');
  };
  const a = ctl.submit(SPEC);
  assert.equal(task(ctl, a.opId).state, TaskState.RUNNING, 'started worker is retained, not fake-refused');
  assert.equal(ctl.usage().cpuUsed, 1);
  assert.ok(ctl.diagnostics().some(d => d.kind === 'submit-threw-after-effects'));

  backend.cancelled(a.opId); // worker exits after its signal-less failure
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLED);
  assert.equal(ctl.usage().cpuUsed, 0);
});

test('a generic submit throw after started keeps the running owner', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 1 } });
  backend.onSubmit = spec => {
    backend.started(spec.opId);
    throw new Error('post-start exception');
  };
  const a = ctl.submit(SPEC);
  assert.equal(task(ctl, a.opId).state, TaskState.RUNNING);
  assert.equal(task(ctl, a.opId).error, 'Error: post-start exception');
  backend.complete(a.opId, 0, {});
  assert.equal(task(ctl, a.opId).state, TaskState.COMPLETED);
  assert.equal(ctl.usage().cpuUsed, 0);
});

test('observer faults never interrupt state or reservation transitions', () => {
  const backend = new ScriptedBackend();
  const ctl = new Controller({
    backend,
    cpuTokens: 1,
    byteBudget: 8000,
    onReceipt: () => {
      throw new Error('receipt observer broke');
    },
    onOutput: () => {
      throw new Error('output observer broke');
    },
    onDiagnostic: () => {
      throw new Error('diagnostic observer broke');
    },
  });
  const a = ctl.submit(SPEC);
  backend.started(a.opId);
  backend.complete(a.opId, 0, { out: 1 });
  assert.equal(task(ctl, a.opId).state, TaskState.COMPLETED, 'task settles despite observer throws');
  assert.equal(ctl.usage().cpuUsed, 0, 'reservation released despite observer throws');
  assert.ok(ctl.observerFaults().length >= 3, 'each fault is recorded');

  // A throwing diagnostic observer must not recurse or lose the record.
  backend.complete('op-999', 0, {});
  assert.ok(ctl.diagnostics().some(d => d.kind === 'unknown-op'));
});

test('teardown during an admitted receipt starts nothing afterwards', () => {
  const backend = new ScriptedBackend();
  let armed = false;
  const ctl = new Controller({
    backend,
    cpuTokens: 1,
    byteBudget: 2000,
    onReceipt: r => {
      if (armed && r.type === 'admitted') ctl.teardown();
    },
  });
  const a = ctl.submit({ ...SPEC, reserveBytes: 1500 }); // admitted
  const b = ctl.submit({ ...SPEC, reserveBytes: 400 }); // queued (400 > 500 free? no: cpu blocks)
  const c = ctl.submit({ ...SPEC, reserveBytes: 400 }); // queued
  assert.equal(task(ctl, b.opId).state, TaskState.QUEUED);

  armed = true;
  backend.complete(a.opId, 0, {}); // frees -> pump admits b -> receipt -> teardown
  // The just-admitted task was never submitted: signalled, unresolved.
  assert.equal(task(ctl, b.opId).state, TaskState.CANCELLING);
  assert.equal(task(ctl, c.opId).state, TaskState.CANCELLED, 'queued task cancelled, no effects');
  assert.deepEqual(backend.submitted.map(s => s.opId), [a.opId], 'admitted-during-teardown task never started');
  assert.equal(backend.disposed, 1);
});

test('a fatal backend event mid-submit admits nothing further', () => {
  const { ctl, backend } = makeCtl({ ctl: { cpuTokens: 2 } });
  backend.onSubmit = spec => {
    backend.send({ type: 'backend-error', error: 'died inside submit' });
  };
  const a = ctl.submit(SPEC);
  const b = ctl.submit(SPEC);
  assert.equal(task(ctl, a.opId).state, TaskState.FAILED);
  assert.equal(task(ctl, b.opId).state, TaskState.FAILED, 'queued task settled by the fatal error');
  assert.equal(backend.submitted.length, 1, 'second task never submitted');
  assert.equal(ctl.usage().cpuUsed, 0);
});

test('a cancel inside the admitted receipt settles pre-submit with no fake terminal wait', () => {
  const backend = new ScriptedBackend();
  const ctl = new Controller({ backend, cpuTokens: 2, byteBudget: 8000 });
  let opToCancel = null;
  // Cancel the op the moment it is admitted — before backend.submit.
  const origReceipt = ctl._receipt.bind(ctl);
  ctl._receipt = (task, type, detail) => {
    const r = origReceipt(task, type, detail);
    if (type === 'admitted' && task) ctl.cancel(task.opId);
    return r;
  };
  const a = ctl.submit(SPEC);
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLED);
  assert.equal(backend.submitted.length, 0, 'never submitted after pre-submit cancel');
  assert.equal(ctl.usage().cpuUsed, 0, 'the just-booked reservation was released');
});

test('teardown reports unresolved ownership and held reservations honestly', () => {
  // Silent worker: no exits on dispose -> honest unresolved report.
  const { ctl, backend } = makeCtl({ backendOpts: { disposeEmits: false }, ctl: { cpuTokens: 1 } });
  const a = ctl.submit({ ...SPEC, reserveBytes: 900 });
  const b = ctl.submit({ ...SPEC, payload: { frame: generateFixture(0) } }); // queued
  backend.started(a.opId);

  const report = ctl.teardown();
  assert.equal(backend.disposed, 1);
  assert.deepEqual(backend.cancelSignals, [a.opId], 'live task was signalled');
  assert.equal(task(ctl, b.opId).state, TaskState.CANCELLED, 'queued task cancelled with no effects');
  assert.equal(internal(ctl, b.opId).payload, null, 'teardown drops the queued payload');
  assert.deepEqual(backend.submitted.map(s => s.opId), [a.opId]);
  assert.equal(report.unresolved.length, 1);
  assert.equal(report.unresolved[0].opId, a.opId);
  assert.equal(report.unresolved[0].heldCpu, 1);
  assert.equal(report.unresolved[0].heldBytes, 900, 'held reservation is reported, not hidden');
  assert.ok(ctl.usage().byteUsed > 0 || ctl.usage().cpuUsed > 0, 'unresolved reservation is not faked as freed');

  // Submissions after teardown refuse.
  const late = ctl.submit(SPEC);
  assert.equal(late.refusal, 'torn-down');
});

test('teardown settles tasks when the worker reports exits', () => {
  const { ctl, backend } = makeCtl({ backendOpts: { disposeEmits: true }, ctl: { cpuTokens: 1 } });
  const a = ctl.submit(SPEC);
  backend.started(a.opId);
  const report = ctl.teardown();
  assert.equal(report.unresolved.length, 0);
  assert.equal(task(ctl, a.opId).state, TaskState.CANCELLED);
  assert.equal(ctl.usage().cpuUsed, 0);
});

test('receipts are immutable, ordered, and carry task identity', () => {
  const { ctl, backend } = makeCtl();
  const a = ctl.submit(SPEC);
  backend.started(a.opId);
  backend.complete(a.opId, 0, {});
  const receipts = ctl.receipts();
  assert.ok(receipts.length >= 3);
  for (const r of receipts) assert.ok(Object.isFrozen(r), 'receipt is frozen');
  assert.throws(() => {
    'use strict';
    receipts[0].type = 'mutated';
  }, TypeError);
  const seqs = receipts.map(r => r.seq);
  assert.deepEqual(seqs, [...seqs].sort((x, y) => x - y), 'seq is monotonic');
  assert.equal(receipts.every(r => r.backend === 'model'), true, 'backend identity on every receipt');
  const owned = receipts.filter(r => r.opId === a.opId);
  assert.ok(owned.every(r => r.generation === 0), 'generation on every task receipt');
});

test('receipts never retain or freeze caller buffers', () => {
  const { ctl, backend, outputs } = makeCtl();
  const frame = generateFixture(0);
  const a = ctl.submit({ ...SPEC, payload: { frame } });
  backend.started(a.opId);
  const outFrame = generateFixture(0);
  backend.complete(a.opId, 0, outFrame);
  // The delivered output object is the caller's — still mutable, never
  // frozen by the immutable-receipt machinery.
  outFrame.data[0] = 123;
  assert.equal(outFrame.data[0], 123);
  assert.equal(outputs.length, 1);
  assert.equal(outputs[0].output, outFrame, 'ownership transfers to the observer');
  assert.equal(internal(ctl, a.opId).output, undefined, 'controller retains no raw output');
});

test('foreign and malformed events are diagnostics, never effects', () => {
  const { ctl, backend } = makeCtl();
  const a = ctl.submit(SPEC);
  backend.started(a.opId);
  backend.send({ type: 'completed', opId: a.opId, generation: 9 }); // wrong gen
  backend.send({ type: 'completed', opId: a.opId, generation: 0, backend: 'other' }); // wrong backend
  assert.equal(task(ctl, a.opId).state, TaskState.RUNNING);
  assert.equal(ctl.usage().cpuUsed, 1, 'foreign events release nothing');
  const kinds = ctl.diagnostics().map(d => d.kind);
  assert.deepEqual(kinds, ['foreign-event', 'foreign-event']);
});

test('ModelBackend under a manual scheduler satisfies the whole contract end-to-end', () => {
  const queue = [];
  const backend = new ModelBackend({
    steps: 2,
    schedule: fn => queue.push(fn),
    unschedule: () => {},
  });
  const outputs = [];
  const ctl = new Controller({ backend, cpuTokens: 1, byteBudget: 1 << 20, onOutput: (v, o) => outputs.push(o) });

  const frame = generateFixture(0);
  const v = ctl.submit({
    kind: 'invert',
    payload: { frame },
    estimateBytes: 2 * FIXTURE_BYTES,
    reserveBytes: 3 * FIXTURE_BYTES,
  });
  assert.equal(task(ctl, v.opId).state, TaskState.RUNNING, 'model backend reports started synchronously');
  assert.equal(internal(ctl, v.opId).payload, null, 'payload handed to the backend');

  while (queue.length) queue.shift()();
  assert.equal(task(ctl, v.opId).state, TaskState.COMPLETED);
  assert.equal(outputs.length, 1, 'real model output delivered to the preview');
  const expected = applyModelOp('invert', frame);
  assert.deepEqual([...outputs[0].data], [...expected.data], 'exact transform pixels, end-to-end');
  assert.equal(ctl.usage().cpuUsed, 0);

  // Async cancel through the real model backend path: cancel-ack is not
  // exit; the terminal cancelled receipt is what releases.
  const w = ctl.submit({
    kind: 'fade',
    payload: { frame },
    estimateBytes: 2 * FIXTURE_BYTES,
    reserveBytes: 3 * FIXTURE_BYTES,
  });
  ctl.cancel(w.opId);
  assert.equal(task(ctl, w.opId).state, TaskState.CANCELLING);
  assert.equal(ctl.usage().cpuUsed, 1, 'reservation held past cancel-ack');
  while (queue.length) queue.shift()();
  assert.equal(task(ctl, w.opId).state, TaskState.CANCELLED);
  assert.equal(ctl.usage().cpuUsed, 0);
});

test('unsupported media kinds refuse as unqualified', () => {
  const { ctl, backend } = makeCtl();
  backend.unsupported = new Set(['mp4-clip']);
  const v = ctl.submit({ kind: 'mp4-clip', estimateBytes: 5, reserveBytes: 5 });
  assert.equal(v.state, TaskState.FAILED);
  assert.equal(v.refusal, 'unsupported');
  assert.deepEqual(backend.submitted, []);
});
