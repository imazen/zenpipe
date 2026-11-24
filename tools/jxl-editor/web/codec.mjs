import { appLog } from "./console-log.mjs";

let worker;
let nextId = 0;
const pending = new Map();

function summarizeResult(method, value) {
  if (!value) return null;
  if (method === "decode" || method === "zensrRun") {
    return `${value.width}×${value.height} (${value.pixels?.byteLength ?? 0} B)`;
  }
  if (method === "nativeInfo") {
    const f = value.frames?.length ?? 0;
    const c = value.canvas ? `${value.canvas[0]}×${value.canvas[1]}` : "";
    return `${c} · ${f} frame${f === 1 ? "" : "s"}${value.profile?.cmyk ? " · CMYK" : ""}`;
  }
  if (method === "nativeTimeline") {
    return `${value.width}×${value.height} · ${value.info?.frames?.length ?? 0} frames`;
  }
  if (value instanceof Uint8Array) {
    return `${value.byteLength} B`;
  }
  if (typeof value === "string") return value;
  return null;
}

export function codec(method, fields = {}, transfer = []) {
  if (!worker) {
    worker = new Worker(new URL("./codec-worker.mjs", import.meta.url), { type: "module" });
    worker.onmessage = ({ data: { id, value, error } }) => {
      const request = pending.get(id);
      if (!request) return;
      pending.delete(id);
      const ms = Math.round(performance.now() - request.t0);
      if (error) {
        const err = new Error(error);
        appLog.codec(request.method, ms, null, err);
        request.reject(err);
      } else {
        appLog.codec(request.method, ms, summarizeResult(request.method, value), null);
        request.resolve(value);
      }
    };
    worker.onerror = event => {
      const err = new Error(event.message || "Codec worker stopped.");
      appLog.error("codec:worker", err.message);
      for (const request of pending.values()) request.reject(err);
      pending.clear();
      worker.terminate();
      worker = null;
    };
  }
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject, method, t0: performance.now() });
    try {
      worker.postMessage({ id, method, ...fields }, transfer);
    } catch (error) {
      pending.delete(id);
      appLog.codec(method, 0, null, error);
      reject(error);
    }
  });
}
