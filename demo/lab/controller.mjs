// =====================================================================
// Internal browser task controller — contract shell (zenpipe#113)
//
// Deterministic orchestration for media tasks: operation IDs, source
// generations and backend identity on every task and event; a fixed
// global budget of CPU tokens and DECLARED byte reservations; explicit
// cancellation and teardown semantics. This is NOT a codec
// implementation and reproduces no Rust traits — a native adapter will
// later translate real operation receipts onto the same event shapes.
//
// Reservations are declared accounting, nothing more. `reserveBytes` is
// the amount the submitter books against the shell's budget; it is not a
// measured peak and not a proven bound. The empirical `estimateBytes`
// (null = unknown — unknown is never zero) is informational and does
// not, by itself, gate admission when a reservation is declared. A
// native strict gate must require a bound backed by an estimator, not an
// arbitrary numeric reservation — that stronger contract is out of scope
// for this shell.
//
// Backend interface (duck-typed):
//   backend.identity              string: 'model' | 'browser' | 'native-wasm'
//   backend.attach(emit)          controller -> backend event sink
//   backend.supports?(kind)       optional admission predicate
//   backend.submit(spec)          spec: { opId, generation, kind, payload }
//                                 returns normally = accepted; throwing
//                                 SubmitRefusal = guaranteed no effects;
//                                 any other throw = uncertain start
//   backend.cancel(opId)          signal only; terminal receipt comes later
//   backend.dispose()             teardown; may emit exits for owned tasks
//
// Backend -> controller events (all tagged opId + generation + backend):
//   { type:'started' }                      admitted -> running
//   { type:'progress', detail? }            non-state receipt only
//   { type:'cancel-ack' }                   acknowledged, NOT exit
//   { type:'completed', output? }           terminal, releases reservation
//   { type:'failed', error? }               terminal, releases reservation
//   { type:'cancelled' }                    terminal, releases reservation
//   { type:'backend-error', error? }        fatal: settles every owned task
//
// Contract highlights (docs/media-suite.md):
//   queued -> admitted -> running -> completed | failed | cancelled
//   running/admitted -> cancelling -> terminal only via backend receipt
//   queued cancel: no backend effects, nothing was reserved
//   cancelling holds the reservation until the terminal receipt arrives;
//   a cancel-ack is never counted as exit
//   superseded output is retained as a bounded summary for diagnostics —
//   the raw pixels are released — and can never replace the preview
//   duplicate/stale terminal events never double-release
//   strict admission refuses undeclared reservations before effects;
//   impossible reservations refuse instead of queueing forever
// =====================================================================

export const TaskState = Object.freeze({
  QUEUED: 'queued',
  ADMITTED: 'admitted',
  RUNNING: 'running',
  CANCELLING: 'cancelling',
  COMPLETED: 'completed',
  FAILED: 'failed',
  CANCELLED: 'cancelled',
});

const TERMINAL = new Set([TaskState.COMPLETED, TaskState.FAILED, TaskState.CANCELLED]);
const TERMINAL_EVENTS = new Set(['completed', 'failed', 'cancelled']);
// States that hold a reservation (admitted -> running -> cancelling).
const RESERVED = new Set([TaskState.ADMITTED, TaskState.RUNNING, TaskState.CANCELLING]);

/**
 * A backend throws this from submit() to refuse with guaranteed no
 * effects. Any other throw is treated as an uncertain start.
 */
export class SubmitRefusal extends Error {
  constructor(reason = 'refused') {
    super(reason);
    this.code = 'submit-refused';
  }
}

function isNonnegInt(v) {
  return Number.isSafeInteger(v) && v >= 0;
}

function deepFreeze(value) {
  if (value && typeof value === 'object' && !ArrayBuffer.isView(value) && !(value instanceof ArrayBuffer)) {
    for (const k of Object.keys(value)) deepFreeze(value[k]);
    Object.freeze(value);
  }
  return value;
}

/** Bounded summary of a task output: scalars copied, pixels not retained. */
function summarizeOutput(output) {
  if (output == null) return null;
  if (output.data && Number.isSafeInteger(output.width) && Number.isSafeInteger(output.height)) {
    return { kind: 'frame', width: output.width, height: output.height, bytes: output.data.byteLength };
  }
  if (output instanceof ArrayBuffer || ArrayBuffer.isView(output)) {
    return { kind: 'bytes', bytes: output.byteLength };
  }
  return { kind: 'object', bytes: null };
}

/** Bounded summary of a submit payload (looks one level into .frame). */
function summarizePayload(payload) {
  if (payload == null) return null;
  return summarizeOutput(payload.frame ?? payload) ?? { kind: 'object', bytes: null };
}

export class Controller {
  /**
   * @param {object} opts
   * @param {object} opts.backend          backend duck-type above
   * @param {number} [opts.cpuTokens]      fixed global CPU token pool (>=1)
   * @param {number} [opts.byteBudget]     fixed global reservation pool (>=0)
   * @param {boolean} [opts.strict]        refuse undeclared reservations
   * @param {(r:object)=>void} [opts.onReceipt]    every immutable receipt
   * @param {(view:object, output:*)=>void} [opts.onOutput]
   *        non-superseded completions only; ownership of `output`
   *        transfers to the observer — the controller retains only the
   *        bounded summary
   * @param {(d:object)=>void} [opts.onDiagnostic] ignored/superseded evidence
   */
  constructor({
    backend,
    cpuTokens = 2,
    byteBudget = 8 * 1024 * 1024,
    strict = true,
    onReceipt = () => {},
    onOutput = () => {},
    onDiagnostic = () => {},
  }) {
    if (!backend || typeof backend.attach !== 'function' ||
        typeof backend.submit !== 'function' || typeof backend.cancel !== 'function') {
      throw new TypeError('backend must implement attach/submit/cancel');
    }
    if (!Number.isSafeInteger(cpuTokens) || cpuTokens < 1) {
      throw new RangeError('cpuTokens must be a positive safe integer');
    }
    if (!isNonnegInt(byteBudget)) {
      throw new RangeError('byteBudget must be a nonnegative safe integer');
    }
    this.backend = backend;
    this.cpuTokens = cpuTokens;
    this.byteBudget = byteBudget;
    this.strict = strict;
    this.generation = 0;
    this._onReceipt = onReceipt;
    this._onOutput = onOutput;
    this._onDiagnostic = onDiagnostic;
    this._seq = 0;
    this._opSeq = 0;
    this._tasks = new Map(); // opId -> task record (mutable internals)
    this._queue = []; // queued task records, FIFO
    this._receipts = [];
    this._diagnostics = [];
    this._observerFaults = []; // observer exceptions, recorded not thrown
    this._pumping = false;
    this._tornDown = false;
    this._backendFailed = false;
    this._previewOpId = null; // latest requested op; older output is superseded
    backend.attach(ev => this._onEvent(ev));
  }

  // ------------------------- submission -----------------------------

  /**
   * Submit a task. `spec`: `{ kind, payload?, cpu?, estimateBytes?,
   * reserveBytes? }`.
   *
   * `cpu` must be a positive safe integer; `reserveBytes`, when given, a
   * nonnegative safe integer — invalid numbers are refused before any
   * effect. `reserveBytes` is the declared reservation booked at
   * admission; in strict mode it must be declared. Non-strict mode
   * admits an undeclared reservation by booking the entire byte budget
   * (the conservative alternative — it serializes the task). Unknown
   * `estimateBytes` alone never refuses: it is an empirical prediction,
   * not a bound, and is displayed as unknown either way.
   *
   * `payload` is caller memory retained only until it is queued-out:
   * handed to the backend at admission (which then owns it) or dropped
   * on refusal/queued cancellation. Queued payloads sit explicitly
   * OUTSIDE the declared compute reservation — this shell makes no
   * whole-memory bound claim over caller buffers.
   *
   * Returns a frozen task view — including refused submissions, which
   * are recorded but never queued or submitted.
   */
  submit(spec) {
    const task = {
      opId: `op-${++this._opSeq}`,
      generation: this.generation,
      backend: this.backend.identity,
      kind: spec?.kind,
      payload: spec?.payload ?? null,
      cpu: spec?.cpu ?? 1,
      estimateBytes: spec?.estimateBytes ?? null,
      reserveBytes: spec?.reserveBytes ?? null,
      unboundedReserve: false,
      state: TaskState.QUEUED,
      cancelRequested: false,
      released: true, // a queued task holds nothing; set false at admission
      eventsSeen: 0,
      refusal: null,
      error: null,
      outputSummary: null,
      payloadSummary: summarizePayload(spec?.payload),
    };
    this._tasks.set(task.opId, task);

    const invalid = this._invalidSpec(task);
    if (invalid) return this._refuse(task, 'invalid-spec', { field: invalid });
    if (this._tornDown) return this._refuse(task, 'torn-down');
    if (this._backendFailed) return this._refuse(task, 'backend-dead');
    if (this.backend.supports && !this.backend.supports(task.kind)) {
      return this._refuse(task, 'unsupported');
    }
    if (task.reserveBytes == null) {
      if (this.strict) return this._refuse(task, 'unbounded-reservation');
      // Non-strict compatible alternative: book the whole budget.
      task.reserveBytes = this.byteBudget;
      task.unboundedReserve = true;
    }
    if (task.cpu > this.cpuTokens || task.reserveBytes > this.byteBudget) {
      return this._refuse(task, 'impossible');
    }

    this._queue.push(task);
    this._previewOpId = task.opId; // newest requested op drives the preview
    this._receipt(task, 'submitted', { kind: task.kind });
    this._pump();
    return this._view(task);
  }

  /** First invalid field name, or null. */
  _invalidSpec(task) {
    if (typeof task.kind !== 'string' || task.kind.length === 0) return 'kind';
    if (!Number.isSafeInteger(task.cpu) || task.cpu < 1) return 'cpu';
    if (task.reserveBytes != null && !isNonnegInt(task.reserveBytes)) return 'reserveBytes';
    if (task.estimateBytes != null && !isNonnegInt(task.estimateBytes)) return 'estimateBytes';
    return null;
  }

  _refuse(task, reason, detail = {}) {
    task.state = TaskState.FAILED;
    task.refusal = reason;
    task.payload = null; // caller buffer: not retained
    this._receipt(task, 'refused', { reason, ...detail });
    return this._view(task);
  }

  _pump() {
    // Reentrancy-safe: backend.submit may synchronously emit events that
    // settle and release; the outer loop re-reads the queue head after
    // every iteration, so a nested _pump is simply skipped.
    if (this._pumping || this._tornDown || this._backendFailed) return;
    this._pumping = true;
    try {
      // Strict FIFO: the queue head must fit, or everyone behind it
      // waits. The loop re-checks the lifecycle flags each pass — a
      // receipt observer, a synchronous backend event or a throwing
      // submit may tear down or kill the backend mid-pump, and a queued
      // task must never start after that.
      while (this._queue.length && !this._tornDown && !this._backendFailed) {
        const head = this._queue[0];
        if (head.cpu > this._freeCpu() || head.reserveBytes > this._freeBytes()) break;
        this._queue.shift();
        head.state = TaskState.ADMITTED;
        head.released = false; // reservation now booked
        this._receipt(head, 'admitted', {
          cpu: head.cpu,
          reservedBytes: head.reserveBytes,
        });
        // The admitted receipt's observers may already have changed
        // this task: a cancel moves it to cancelling before the backend
        // ever saw it — that is a queued-equivalent cancel, so settle it
        // with no backend effects and release the just-booked
        // reservation. Torn-down tasks stay owned and honestly
        // unresolved. In every case the caller payload is dropped here.
        const payload = head.payload;
        head.payload = null;
        if (head.state !== TaskState.ADMITTED) {
          if (head.state === TaskState.CANCELLING && !this._tornDown) {
            this._settle(head, TaskState.CANCELLED, { while: 'pre-submit' });
          }
          continue;
        }
        try {
          this.backend.submit({
            opId: head.opId,
            generation: head.generation,
            kind: head.kind,
            payload,
          });
        } catch (err) {
          this._submitThrew(head, err);
        }
      }
    } finally {
      this._pumping = false;
    }
  }

  /**
   * submit() threw. Three honest outcomes, per the backend contract:
   * events seen first -> the worker signalled before the throw and may
   *                   still be live: retain ownership until its terminal
   *                   receipt. A backend cannot emit started and then
   *                   throw SubmitRefusal to evade ownership.
   * SubmitRefusal   -> guaranteed no effects: settle failed, release.
   * otherwise       -> uncertain start: the absence of a 'started' event
   *                   does NOT prove the absence of worker effects, so
   *                   ownership and the reservation are retained; the
   *                   task moves to cancelling and the backend is
   *                   signalled to request cleanup. It settles on the
   *                   terminal receipt — or is reported unresolved at
   *                   teardown.
   */
  _submitThrew(task, err) {
    if (task.eventsSeen > 0 || task.state !== TaskState.ADMITTED) {
      task.error = String(err);
      return this._diag('submit-threw-after-effects', task, String(err));
    }
    if (err && err.code === 'submit-refused') {
      task.error = String(err.message ?? err);
      return this._settle(task, TaskState.FAILED, { cause: 'submit-refused' });
    }
    task.error = String(err);
    task.uncertain = true;
    task.cancelRequested = true;
    task.state = TaskState.CANCELLING;
    this._receipt(task, 'cancel-requested', { while: 'submit-uncertain' });
    this._diag('submit-threw-uncertain', task, String(err));
    try {
      this.backend.cancel(task.opId);
    } catch (e) {
      this._diag('cancel-signal-error', task, String(e));
    }
  }

  // ------------------------- cancellation ---------------------------

  /**
   * Cancel a task. Queued: terminal cancelled, zero backend effects —
   * and removing the queue head may let a smaller queued task fit the
   * already-free capacity, so admission is re-evaluated. Admitted/
   * running: signals the backend and moves to cancelling; the
   * reservation is held until the worker's terminal receipt arrives.
   * Idempotent; terminal and unknown tasks are no-ops.
   */
  cancel(opId) {
    const task = this._tasks.get(opId);
    if (!task || TERMINAL.has(task.state) || task.state === TaskState.CANCELLING) {
      return false;
    }
    task.cancelRequested = true;
    if (task.state === TaskState.QUEUED) {
      this._queue = this._queue.filter(t => t !== task);
      task.state = TaskState.CANCELLED;
      task.payload = null; // never reached the backend: drop caller buffer
      this._receipt(task, 'cancelled', { while: 'queued' });
      this._pump();
      return true;
    }
    task.state = TaskState.CANCELLING;
    this._receipt(task, 'cancel-requested', { while: task.state });
    try {
      this.backend.cancel(opId);
    } catch (err) {
      this._diag('cancel-signal-error', task, String(err));
    }
    return true;
  }

  /** Advance the source generation. Older tasks become superseded: their
   * completions still release but can never replace the preview. */
  advanceGeneration() {
    this.generation++;
    this._receipt(null, 'generation', { generation: this.generation });
  }

  // ------------------------- backend events -------------------------

  _onEvent(ev) {
    if (!ev || typeof ev !== 'object') return;
    if (ev.type === 'backend-error') {
      if (ev.backend !== this.backend.identity) {
        return this._diag('foreign-backend-error', null, ev);
      }
      return this._backendError(ev);
    }

    const task = this._tasks.get(ev.opId);
    if (!task) return this._diag('unknown-op', null, ev);
    if (ev.backend !== task.backend || ev.generation !== task.generation) {
      return this._diag('foreign-event', task, ev);
    }
    task.eventsSeen++;

    if (task.state === TaskState.QUEUED) {
      // A queued task was never submitted: any event for it is foreign
      // to its lifecycle — evidence only, never a state change.
      return this._diag('event-for-queued-task', task, ev);
    }

    if (ev.type === 'started') {
      if (task.state === TaskState.ADMITTED) {
        task.state = TaskState.RUNNING;
        this._receipt(task, 'started');
      } else {
        this._diag('unexpected-start', task, ev);
      }
    } else if (ev.type === 'progress' || ev.type === 'cancel-ack') {
      // A cancel acknowledgement is not exit: no state change, no release.
      this._receipt(task, ev.type, ev.detail ? { detail: ev.detail } : {});
    } else if (TERMINAL_EVENTS.has(ev.type)) {
      this._terminal(task, ev);
    } else {
      this._diag('unknown-event-type', task, ev);
    }
  }

  _terminal(task, ev) {
    if (TERMINAL.has(task.state)) {
      // Duplicate/stale terminal: retained for diagnostics, never a
      // second release and never a state change.
      return this._diag('duplicate-terminal', task, ev);
    }
    const to = ev.type; // 'completed' | 'failed' | 'cancelled'
    if (ev.error != null) task.error = ev.error;
    // Bounded summary only: raw output is handed to the preview (which
    // takes ownership) or released, never retained on the task.
    task.outputSummary = summarizeOutput(ev.output);
    this._settle(task, to, { afterCancel: task.cancelRequested });

    // Delivery is decided HERE, after settlement — observers and the
    // admission pump may have advanced the generation or admitted a
    // newer operation while this one settled. The preview belongs to
    // the latest requested operation of the current generation only.
    const deliver =
      to === TaskState.COMPLETED &&
      !task.cancelRequested &&
      task.generation === this.generation &&
      task.opId === this._previewOpId;
    if (deliver) {
      this._observe(this._onOutput, this._view(task), ev.output);
    } else if (to === TaskState.COMPLETED && task.outputSummary) {
      this._diag(
        task.cancelRequested ? 'cancelled-race-output' : 'superseded-output',
        task,
        task.outputSummary,
      );
    }
  }

  /** Settle a task to a terminal state and release exactly once. */
  _settle(task, to, detail = {}) {
    task.state = to;
    const releasedNow = this._release(task);
    this._receipt(task, to, { ...detail, releasedNow });
    this._pump(); // a freed reservation admits the queue head
  }

  _release(task) {
    if (task.released) return false;
    task.released = true;
    return true;
  }

  /**
   * Fatal backend error: every task that backend owns is settled failed
   * and released exactly once. The backend is then dead — no further
   * admissions (a queued task would wait forever), and new submissions
   * refuse as 'backend-dead'.
   */
  _backendError(ev) {
    this._backendFailed = true;
    for (const task of this._tasks.values()) {
      if (task.backend !== ev.backend || TERMINAL.has(task.state)) continue;
      if (task.state === TaskState.QUEUED) {
        this._queue = this._queue.filter(t => t !== task);
        task.payload = null;
      }
      task.error = ev.error ?? 'backend-error';
      this._settle(task, TaskState.FAILED, { cause: 'backend-error' });
    }
    this._receipt(null, 'backend-error', { backend: ev.backend, error: ev.error ?? null });
  }

  // ------------------------- teardown -------------------------------

  /**
   * Cancel every owned task and dispose the backend. Queued tasks are
   * cancelled with no effects; live tasks are signalled and moved to
   * cancelling. Returns a frozen report that honestly lists tasks still
   * unresolved (no terminal receipt) and the reservations they hold.
   */
  teardown() {
    this._tornDown = true;
    for (const task of this._tasks.values()) {
      if (TERMINAL.has(task.state)) continue;
      if (task.state === TaskState.QUEUED) {
        this._queue = this._queue.filter(t => t !== task);
        task.state = TaskState.CANCELLED;
        task.payload = null;
        this._receipt(task, 'cancelled', { while: 'teardown-queued' });
      } else if (task.state !== TaskState.CANCELLING) {
        task.cancelRequested = true;
        task.state = TaskState.CANCELLING;
        this._receipt(task, 'cancel-requested', { while: 'teardown' });
        try {
          this.backend.cancel(task.opId);
        } catch (err) {
          this._diag('cancel-signal-error', task, String(err));
        }
      }
    }
    try {
      this.backend.dispose();
    } catch (err) {
      this._diag('dispose-error', null, String(err));
    }

    const unresolved = [];
    const settled = [];
    for (const task of this._tasks.values()) {
      if (TERMINAL.has(task.state)) settled.push(this._view(task));
      else {
        unresolved.push(
          deepFreeze({
            opId: task.opId,
            generation: task.generation,
            backend: task.backend,
            state: task.state,
            heldCpu: task.released ? 0 : task.cpu,
            heldBytes: task.released ? 0 : task.reserveBytes,
          }),
        );
      }
    }
    const report = deepFreeze({ settled, unresolved });
    this._receipt(null, 'teardown', { unresolved: unresolved.length });
    return report;
  }

  // ------------------------- views / accounting ---------------------

  _freeCpu() {
    let used = 0;
    for (const t of this._tasks.values()) {
      if (!t.released && RESERVED.has(t.state)) used += t.cpu;
    }
    return this.cpuTokens - used;
  }

  _freeBytes() {
    let used = 0;
    for (const t of this._tasks.values()) {
      if (!t.released && RESERVED.has(t.state)) used += t.reserveBytes;
    }
    return this.byteBudget - used;
  }

  /** Global accounting snapshot (frozen). */
  usage() {
    let cpuUsed = 0;
    let bytesUsed = 0;
    for (const t of this._tasks.values()) {
      if (!t.released && RESERVED.has(t.state)) {
        cpuUsed += t.cpu;
        bytesUsed += t.reserveBytes;
      }
    }
    return deepFreeze({
      cpuTotal: this.cpuTokens,
      cpuUsed,
      byteTotal: this.byteBudget,
      byteUsed: bytesUsed,
    });
  }

  /** Immutable public view of one task (metadata only — no raw output). */
  _view(task) {
    return deepFreeze({
      opId: task.opId,
      generation: task.generation,
      backend: task.backend,
      kind: task.kind,
      state: task.state,
      cpu: task.cpu,
      estimateBytes: task.estimateBytes, // null = unknown, not zero
      reserveBytes: task.reserveBytes,
      unboundedReserve: task.unboundedReserve,
      cancelRequested: task.cancelRequested,
      refusal: task.refusal,
      error: task.error,
      outputSummary: task.outputSummary,
      payloadSummary: task.payloadSummary,
      uncertain: task.uncertain === true,
    });
  }

  /** Frozen views of every task, in submission order. */
  listTasks() {
    return [...this._tasks.values()].map(t => this._view(t));
  }

  /** Append-only immutable receipt log (frozen entries). */
  receipts() {
    return this._receipts.slice();
  }

  /** Ignored/superseded/malformed evidence, retained for inspection. */
  diagnostics() {
    return this._diagnostics.slice();
  }

  /** Observer exceptions, recorded rather than thrown into the machine. */
  observerFaults() {
    return this._observerFaults.slice();
  }

  /**
   * Notify an observer without letting its failure interrupt state or
   * reservation transitions. Faults are recorded, never re-thrown.
   */
  _observe(fn, ...args) {
    try {
      fn(...args);
    } catch (err) {
      this._observerFaults.push(
        deepFreeze({ seq: this._seq++, error: String(err) }),
      );
    }
  }

  _receipt(task, type, detail = {}) {
    const r = deepFreeze({
      seq: this._seq++,
      opId: task ? task.opId : null,
      generation: task ? task.generation : this.generation,
      backend: task ? task.backend : this.backend.identity,
      type,
      ...detail,
    });
    this._receipts.push(r);
    this._observe(this._onReceipt, r);
    return r;
  }

  _diag(kind, task, detail) {
    const d = deepFreeze({
      seq: this._seq++,
      kind,
      opId: task ? task.opId : null,
      generation: task ? task.generation : null,
      backend: task ? task.backend : null,
      detail,
    });
    this._diagnostics.push(d);
    this._observe(this._onDiagnostic, d);
    return d;
  }
}
