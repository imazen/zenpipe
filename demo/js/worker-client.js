// =====================================================================
// Worker Communication
// =====================================================================

let worker = null;
let nextMsgId = 0;
const pending = new Map(); // id -> { resolve, reject }

export function initWorker() {
  worker = new Worker('worker.js');
  worker.addEventListener('message', e => {
    const msg = e.data;
    const p = pending.get(msg.id);
    if (!p) return;
    pending.delete(msg.id);
    if (msg.type === 'error') p.reject(new Error(msg.message));
    else p.resolve(msg);
  });
}

export function sendToWorker(type, data) {
  const id = nextMsgId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    const msg = { id, type, ...data };
    // Transfer ArrayBuffer if present
    const transfer = [];
    if (data?.data instanceof ArrayBuffer) transfer.push(data.data);
    if (data?.data instanceof Uint8Array) transfer.push(data.data.buffer);
    worker.postMessage(msg, transfer);
  });
}
