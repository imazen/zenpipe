const MAX_ENTRIES = 500;

class AppLogger {
  constructor() {
    this._entries = [];
    this._nextId = 1;
    this._errors = 0;
    this._listeners = new Set();
    this._origError = console.error.bind(console);
    this._origWarn = console.warn.bind(console);
    this._hooksInstalled = false;
  }

  add(level, source, message, detail = null) {
    const entry = {
      id: this._nextId++,
      time: new Date().toISOString().slice(11, 23),
      level,
      source,
      message: String(message ?? ""),
      detail: detail == null ? null : (typeof detail === "string" ? detail : safeStringify(detail)),
    };
    this._entries.push(entry);
    if (this._entries.length > MAX_ENTRIES) {
      const removed = this._entries.shift();
      if (removed?.level === "error") this._errors = Math.max(0, this._errors - 1);
    }
    if (level === "error") this._errors++;
    this._notify();
    return entry;
  }

  info(source, message, detail = null) {
    return this.add("info", source, message, detail);
  }

  warn(source, message, detail = null) {
    return this.add("warn", source, message, detail);
  }

  error(source, message, detail = null) {
    return this.add("error", source, message, detail);
  }

  storage(message, detail = null) {
    return this.add("storage", "vault", message, detail);
  }

  codec(method, ms, summary = null, err = null) {
    if (err) {
      const msg = err instanceof Error ? err.message : String(err);
      return this.add("error", `codec:${method}`, `${msg} (${ms} ms)`, err instanceof Error ? err.stack : null);
    }
    const text = summary ? `${summary} (${ms} ms)` : `ok (${ms} ms)`;
    return this.add("codec", `codec:${method}`, text, null);
  }

  entries() {
    return this._entries;
  }

  errorCount() {
    return this._errors;
  }

  clear() {
    this._entries = [];
    this._errors = 0;
    this._notify();
  }

  formatText() {
    return this._entries.map(e => {
      const head = `[${e.time}] [${e.level.toUpperCase()}] [${e.source}] ${e.message}`;
      return e.detail ? `${head}\n  ${e.detail}` : head;
    }).join("\n");
  }

  subscribe(fn) {
    this._listeners.add(fn);
    return () => this._listeners.delete(fn);
  }

  _notify() {
    for (const fn of this._listeners) {
      try { fn(); } catch {}
    }
  }

  installGlobalHooks() {
    if (this._hooksInstalled || typeof window === "undefined") return;
    this._hooksInstalled = true;
    window.addEventListener("error", event => {
      this.error("window", event.message || "Uncaught error", event.error?.stack || `${event.filename}:${event.lineno}`);
    });
    window.addEventListener("unhandledrejection", event => {
      const reason = event.reason;
      const msg = reason instanceof Error ? reason.message : String(reason);
      this.error("promise", `Unhandled rejection: ${msg}`, reason instanceof Error ? reason.stack : null);
    });
    console.error = (...args) => {
      this._origError(...args);
      this.add("error", "console", args.map(formatArg).join(" "));
    };
    console.warn = (...args) => {
      this._origWarn(...args);
      this.add("warn", "console", args.map(formatArg).join(" "));
    };
  }
}

function formatArg(arg) {
  if (arg instanceof Error) return `${arg.message}${arg.stack ? `\n${arg.stack}` : ""}`;
  if (typeof arg === "string") return arg;
  return safeStringify(arg);
}

function safeStringify(val) {
  try {
    return JSON.stringify(val);
  } catch {
    return String(val);
  }
}

export const appLog = new AppLogger();

export function makeConsolePanel({ onOpenLog, download, status }) {
  const $ = id => document.getElementById(id);
  let filter = "all";
  let renderQueued = false;

  function updateBadge() {
    const toggle = $("log-toggle");
    const count = appLog.entries().length;
    const errs = appLog.errorCount();
    if (toggle) {
      toggle.textContent = errs > 0 ? `Log · ${errs} err` : `Log (${count})`;
      toggle.toggleAttribute("data-has-error", errs > 0);
    }
    const tab = $("log-tab");
    if (tab) {
      tab.textContent = errs > 0 ? `Log (${errs})` : "Log";
      tab.toggleAttribute("data-has-error", errs > 0);
    }
  }

  function render() {
    renderQueued = false;
    updateBadge();
    const list = $("log-list");
    if (!list) return;
    const items = appLog.entries().filter(e => {
      if (filter === "errors") return e.level === "error" || e.level === "warn";
      if (filter === "codec") return e.level === "codec" || e.source.startsWith("codec:");
      if (filter === "storage") return e.level === "storage" || e.source === "vault";
      return true;
    });
    const summary = $("log-summary");
    if (summary) {
      summary.textContent = `${items.length} shown · ${appLog.entries().length} total · ${appLog.errorCount()} errors`;
    }
    list.replaceChildren();
    if (!items.length) {
      const empty = document.createElement("li");
      empty.className = "log-empty muted-note";
      empty.textContent = "No log entries match this filter.";
      list.append(empty);
      return;
    }
    const frag = document.createDocumentFragment();
    for (let i = items.length - 1; i >= 0; i--) {
      const e = items[i];
      const li = document.createElement("li");
      li.className = `log-entry log-${e.level}`;
      const meta = document.createElement("div");
      meta.className = "log-meta";
      const time = document.createElement("span");
      time.className = "log-time";
      time.textContent = e.time;
      const src = document.createElement("span");
      src.className = "log-src";
      src.textContent = e.source;
      meta.append(time, src);
      const msg = document.createElement("div");
      msg.className = "log-msg";
      msg.textContent = e.message;
      li.append(meta, msg);
      if (e.detail) {
        const det = document.createElement("pre");
        det.className = "log-detail";
        det.textContent = e.detail;
        li.append(det);
      }
      frag.append(li);
    }
    list.append(frag);
  }

  function scheduleRender() {
    updateBadge();
    if (!renderQueued) {
      renderQueued = true;
      queueMicrotask(render);
    }
  }

  appLog.subscribe(scheduleRender);

  const filterRow = $("log-filters");
  if (filterRow) {
    for (const btn of filterRow.querySelectorAll("[data-log-filter]")) {
      btn.addEventListener("click", () => {
        filter = btn.dataset.logFilter;
        for (const other of filterRow.querySelectorAll("[data-log-filter]")) {
          other.setAttribute("aria-pressed", String(other === btn));
        }
        render();
      });
    }
  }

  $("log-toggle")?.addEventListener("click", () => {
    if (onOpenLog) onOpenLog();
    render();
  });

  $("log-copy")?.addEventListener("click", async () => {
    const text = appLog.formatText();
    try {
      await navigator.clipboard.writeText(text);
      status("Console log copied to clipboard");
    } catch {
      status("Clipboard copy unavailable — use Download log", true);
    }
  });

  $("log-download")?.addEventListener("click", () => {
    const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
    download(new Blob([appLog.formatText() + "\n"], { type: "text/plain" }), `jxl-studio-log-${stamp}.txt`);
    status("Console log downloaded");
  });

  $("log-clear")?.addEventListener("click", () => {
    appLog.clear();
    render();
  });

  render();
  return { render };
}
