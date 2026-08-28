// =====================================================================
// Export worker pool (zenpipe#22)
//
// The primary worker (worker-client.js) keeps serving interactive
// overview/detail renders. Export jobs — full-resolution encodes, srcset
// batches — run on this pool of extra `worker.js` instances, each with
// its own Editor initialised from the same source bytes, so the UI never
// waits behind an encode and several encodes run at once.
//
// Priority: interactive work never queues here at all (it owns the
// primary worker), so it is always ahead of exports. Cancel: a running
// job cannot be interrupted inside WASM, so cancelling terminates the
// worker (and its in-flight encode) and lets the pool respawn lazily.
// =====================================================================

import { state } from './state.js';

/** Export workers: at least 2 (so batches overlap), at most 4. */
export const POOL_SIZE = Math.max(2, Math.min(4, (navigator.hardwareConcurrency || 2) - 1));

class PoolWorker {
  constructor(epoch) {
    this.epoch = epoch;
    this.worker = new Worker('worker.js');
    this.pending = new Map();
    this.nextId = 0;
    this.busy = false;
    this.ready = null;
    this.worker.addEventListener('message', e => {
      const msg = e.data;
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.type === 'error') p.reject(new Error(msg.message));
      else p.resolve(msg);
    });
    this.worker.addEventListener('error', e => {
      for (const p of this.pending.values()) p.reject(new Error(e.message || 'worker error'));
      this.pending.clear();
    });
  }

  send(type, data, transfer = []) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.worker.postMessage({ id, type, ...data }, transfer);
    });
  }

  /** Decode the current source into this worker's own editor. */
  init() {
    if (!this.ready) {
      this.ready = (async () => {
        if (!state.sourceBytes) throw new Error('no source image loaded');
        const bytes = state.sourceBytes.slice(0);
        await this.send('init', { data: bytes }, [bytes]);
        if (state.nativeUpgraded) {
          // Match the primary worker's metadata-preserving native decode.
          try { await this.send('upgrade', {}); } catch { /* mock backend */ }
        }
      })();
    }
    return this.ready;
  }

  terminate() {
    this.worker.terminate();
    for (const p of this.pending.values()) p.reject(new Error('cancelled'));
    this.pending.clear();
  }
}

export class ExportPool {
  constructor(size = POOL_SIZE) {
    this.size = size;
    this.workers = [];
    this.spawned = 0;
    this.completed = 0;
    this.failed = 0;
    this.cancelled = 0;
    this.running = 0;
  }

  /** Drop workers whose editor holds a previous image. */
  _prune() {
    this.workers = this.workers.filter(w => {
      if (w.epoch !== state.imageEpoch) {
        w.terminate();
        return false;
      }
      return true;
    });
  }

  _acquire() {
    this._prune();
    let w = this.workers.find(w => !w.busy);
    if (!w && this.workers.length < this.size) {
      w = new PoolWorker(state.imageEpoch);
      this.workers.push(w);
      this.spawned++;
    }
    if (w) w.busy = true;
    return w || null;
  }

  /**
   * Run `jobs` (`[{ type, data, transfer? }]`) across the pool, at most
   * `size` at a time, in submission order. Resolves to results in job
   * order. `onProgress(done, total)` fires after each job. `signal`
   * (AbortSignal) cancels: running workers are terminated, the promise
   * rejects with `cancelled`.
   */
  run(jobs, { onProgress, signal } = {}) {
    if (signal?.aborted) return Promise.reject(new Error('cancelled'));
    return new Promise((resolve, reject) => {
      const results = new Array(jobs.length);
      let next = 0;
      let done = 0;
      let inFlight = 0;
      let finished = false;
      const fail = err => {
        if (finished) return;
        finished = true;
        reject(err);
      };
      const cancel = () => {
        if (finished) return;
        this.cancelled += inFlight;
        for (const w of this.workers) w.terminate();
        this.workers = [];
        this.running = 0;
        fail(new Error('cancelled'));
      };
      signal?.addEventListener('abort', cancel, { once: true });

      const pump = () => {
        if (finished) return;
        if (done === jobs.length) {
          finished = true;
          signal?.removeEventListener('abort', cancel);
          resolve(results);
          return;
        }
        while (next < jobs.length) {
          const w = this._acquire();
          if (!w) break;
          const i = next++;
          inFlight++;
          this.running++;
          (async () => {
            await w.init();
            return w.send(jobs[i].type, jobs[i].data, jobs[i].transfer || []);
          })().then(
            res => {
              if (finished) return;
              results[i] = res;
              w.busy = false;
              inFlight--;
              this.running--;
              done++;
              this.completed++;
              onProgress?.(done, jobs.length);
              pump();
            },
            err => {
              if (finished) return;
              this.failed++;
              // A worker that failed is not trusted again.
              w.terminate();
              this.workers = this.workers.filter(x => x !== w);
              inFlight--;
              this.running--;
              fail(err);
            },
          );
        }
      };
      pump();
    });
  }

  /** Counters for diagnostics and tests. */
  stats() {
    return {
      size: this.size,
      epoch: state.imageEpoch,
      alive: this.workers.length,
      running: this.running,
      spawned: this.spawned,
      completed: this.completed,
      failed: this.failed,
      cancelled: this.cancelled,
    };
  }

  /** Terminate every worker (new image, page teardown). */
  shutdown() {
    for (const w of this.workers) w.terminate();
    this.workers = [];
    this.running = 0;
  }
}

export const exportPool = new ExportPool();
// Diagnostics hook for the playwright suite.
window.__zenpipeExportPool = exportPool;
