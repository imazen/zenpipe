let worker;
let nextId = 0;
const pending = new Map();

export function codec(method, fields = {}, transfer = []) {
  if (!worker) {
    worker = new Worker(new URL("./codec-worker.mjs", import.meta.url), { type: "module" });
    worker.onmessage = ({ data: { id, value, error } }) => {
      const request = pending.get(id);
      if (!request) return;
      pending.delete(id);
      if (error) request.reject(new Error(error));
      else request.resolve(value);
    };
    worker.onerror = event => {
      for (const request of pending.values()) request.reject(new Error(event.message || "Codec worker stopped."));
      pending.clear();
      worker.terminate();
      worker = null;
    };
  }
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    try {
      worker.postMessage({ id, method, ...fields }, transfer);
    } catch (error) {
      pending.delete(id);
      reject(error);
    }
  });
}
