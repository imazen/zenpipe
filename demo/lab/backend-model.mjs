// =====================================================================
// Model backend — deterministic async job runner (zenpipe#113)
//
// Applies the trivial fixture transforms from fixture.mjs over several
// scheduled steps, emitting controller events tagged with operation ID,
// source generation and backend identity. It stands in for a worker:
// the same submit/cancel/dispose surface and event shapes a native WASM
// adapter will translate real operation receipts onto.
//
// This backend is explicitly a MODEL: it qualifies orchestration only,
// never codec correctness or memory performance. Cancellation is async
// and cooperative between scheduled steps — the model never claims a
// postMessage cancel can interrupt synchronous WASM; a real worker
// blocked inside WASM would not observe it.
//
// The scheduler is injectable so Node tests can drive every step
// deterministically (`schedule: fn => queue.push(fn)`).
// =====================================================================

import { MODEL_KINDS, applyModelOp } from './fixture.mjs';

export class ModelBackend {
  /**
   * @param {object} [opts]
   * @param {number} [opts.steps]    progress steps before completion
   * @param {number} [opts.stepMs]   delay between scheduled steps
   * @param {(fn:()=>void)=>any} [opts.schedule]   default setTimeout
   * @param {(h:any)=>void} [opts.unschedule]      default clearTimeout
   * @param {boolean} [opts.disposeReportsExits]   emit 'cancelled' for
   *        live ops on dispose (models a worker that reports exit;
   *        a terminated worker would report nothing)
   */
  constructor({
    steps = 3,
    stepMs = 40,
    schedule = null,
    unschedule = null,
    disposeReportsExits = true,
  } = {}) {
    this.identity = 'model';
    this.kinds = MODEL_KINDS;
    this.steps = steps;
    this.stepMs = stepMs;
    this.disposeReportsExits = disposeReportsExits;
    this._schedule = schedule ?? (fn => setTimeout(fn, this.stepMs));
    this._unschedule = unschedule ?? (h => clearTimeout(h));
    this._emit = () => {};
    this._live = new Map(); // opId -> { generation, timer, cancelled }
  }

  attach(emit) {
    this._emit = emit;
  }

  supports(kind) {
    return this.kinds.includes(kind);
  }

  /** Begin a model job. Emits started -> progress*steps -> completed. */
  submit({ opId, generation, kind, payload }) {
    const job = { generation, timer: null, cancelled: false, payload, kind };
    this._live.set(opId, job);
    this._emit({ type: 'started', opId, generation, backend: this.identity });
    let step = 0;
    const tick = () => {
      if (job.cancelled) return; // 'cancelled' already emitted by cancel()
      if (!this._live.has(opId)) return; // disposed
      step++;
      if (step <= this.steps) {
        this._emit({
          type: 'progress',
          opId,
          generation,
          backend: this.identity,
          detail: { step, of: this.steps },
        });
        job.timer = this._schedule(tick);
      } else {
        this._live.delete(opId);
        const output = applyModelOp(kind, payload.frame);
        this._emit({
          type: 'completed',
          opId,
          generation,
          backend: this.identity,
          output,
        });
      }
    };
    job.timer = this._schedule(tick);
  }

  /**
   * Signal cancellation. Async and cooperative: a 'cancel-ack' event is
   * emitted immediately (it is NOT an exit), then the terminal
   * 'cancelled' receipt follows on a later scheduled step — the shape a
   * real worker uses when it can observe control between steps.
   */
  cancel(opId) {
    const job = this._live.get(opId);
    if (!job) return;
    job.cancelled = true;
    this._emit({
      type: 'cancel-ack',
      opId,
      generation: job.generation,
      backend: this.identity,
    });
    if (job.timer != null) this._unschedule(job.timer);
    this._schedule(() => {
      this._live.delete(opId);
      this._emit({
        type: 'cancelled',
        opId,
        generation: job.generation,
        backend: this.identity,
      });
    });
  }

  /**
   * Teardown. With `disposeReportsExits` (default) every still-live op
   * emits its terminal 'cancelled' receipt — a worker reporting exit.
   * Set false to model a terminated worker that reports nothing, which
   * is how the shell exercises honest unresolved-ownership reporting.
   */
  dispose() {
    for (const [opId, job] of this._live) {
      if (job.timer != null) this._unschedule(job.timer);
      if (this.disposeReportsExits) {
        this._emit({
          type: 'cancelled',
          opId,
          generation: job.generation,
          backend: this.identity,
        });
      }
    }
    this._live.clear();
  }
}
