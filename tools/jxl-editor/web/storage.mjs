import { appLog } from "./console-log.mjs";

const DB_NAME = "jxl-studio-vault-v1";
const DB_VERSION = 1;
const STORE_DOCS = "documents";
const STORE_VERSIONS = "versions";
const AUTOSAVE_PREF_KEY = "jxl-studio-autosave";
const MAX_VERSIONS_PER_DOC = 20;

let dbPromise = null;

function openDb() {
  if (typeof indexedDB === "undefined") {
    return Promise.reject(new Error("Browser IndexedDB storage is unavailable."));
  }
  return dbPromise ??= new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE_DOCS)) {
        const docs = db.createObjectStore(STORE_DOCS, { keyPath: "docId" });
        docs.createIndex("updatedAt", "updatedAt", { unique: false });
      }
      if (!db.objectStoreNames.contains(STORE_VERSIONS)) {
        const vers = db.createObjectStore(STORE_VERSIONS, { keyPath: "versionId" });
        vers.createIndex("byDoc", "docId", { unique: false });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => {
      dbPromise = null;
      reject(req.error || new Error("Failed to open IndexedDB."));
    };
  });
}

function txPromise(tx) {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error || new Error("IndexedDB transaction failed."));
    tx.onabort = () => reject(tx.error || new Error("IndexedDB transaction aborted."));
  });
}

function reqPromise(req) {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error || new Error("IndexedDB request failed."));
  });
}

function quickFingerprint(bytes) {
  let h = 2166136261 >>> 0;
  const step = Math.max(1, Math.floor(bytes.byteLength / 512));
  for (let i = 0; i < bytes.byteLength; i += step) {
    h ^= bytes[i];
    h = Math.imul(h, 16777619) >>> 0;
  }
  return `${bytes.byteLength}:${h.toString(16)}`;
}

export async function listStoredDocuments() {
  const db = await openDb();
  const tx = db.transaction(STORE_DOCS, "readonly");
  const all = await reqPromise(tx.objectStore(STORE_DOCS).getAll());
  return all.sort((a, b) => (b.updatedAt ?? 0) - (a.updatedAt ?? 0));
}

export async function listStoredVersions(docId) {
  if (!docId) return [];
  const db = await openDb();
  const tx = db.transaction(STORE_VERSIONS, "readonly");
  const all = await reqPromise(tx.objectStore(STORE_VERSIONS).index("byDoc").getAll(docId));
  return all.sort((a, b) => (b.createdAt ?? 0) - (a.createdAt ?? 0));
}

export async function getStoredDocument(docId) {
  const db = await openDb();
  const tx = db.transaction(STORE_DOCS, "readonly");
  return reqPromise(tx.objectStore(STORE_DOCS).get(docId));
}

export async function getStoredVersion(versionId) {
  const db = await openDb();
  const tx = db.transaction(STORE_VERSIONS, "readonly");
  return reqPromise(tx.objectStore(STORE_VERSIONS).get(versionId));
}

export async function saveStoredVersion({
  docId,
  versionId,
  title,
  label,
  kind = "snapshot",
  width,
  height,
  layerCount,
  thumbDataUrl = null,
  bytes,
}) {
  const db = await openDb();
  const payload = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  const fp = quickFingerprint(payload);
  const now = Date.now();

  const existing = await listStoredVersions(docId);
  if (kind === "autosave" && existing[0]?.kind === "autosave" && existing[0]?.fingerprint === fp) {
    return { skipped: true, versionCount: existing.length };
  }

  const tx = db.transaction([STORE_DOCS, STORE_VERSIONS], "readwrite");
  const docsStore = tx.objectStore(STORE_DOCS);
  const versStore = tx.objectStore(STORE_VERSIONS);

  const versionRecord = {
    versionId,
    docId,
    title,
    label,
    kind,
    createdAt: now,
    width,
    height,
    layerCount,
    byteLength: payload.byteLength,
    fingerprint: fp,
    thumbDataUrl,
    bytes: payload,
  };
  versStore.put(versionRecord);

  // Prune oldest versions beyond retention cap (autosaves pruned first)
  const combined = [versionRecord, ...existing];
  let keptCount = combined.length;
  if (combined.length > MAX_VERSIONS_PER_DOC) {
    const excess = combined.length - MAX_VERSIONS_PER_DOC;
    const candidates = [...existing]
      .sort((a, b) => {
        const aAuto = a.kind === "autosave" ? 0 : 1;
        const bAuto = b.kind === "autosave" ? 0 : 1;
        if (aAuto !== bAuto) return aAuto - bAuto;
        return (a.createdAt ?? 0) - (b.createdAt ?? 0);
      })
      .slice(0, excess);
    for (const old of candidates) {
      versStore.delete(old.versionId);
      keptCount--;
    }
  }

  docsStore.put({
    docId,
    title,
    updatedAt: now,
    width,
    height,
    layerCount,
    byteLength: payload.byteLength,
    versionCount: keptCount,
    thumbDataUrl,
    bytes: payload,
  });

  await txPromise(tx);
  return { skipped: false, versionId, versionCount: keptCount };
}

export async function deleteStoredVersion(versionId) {
  const db = await openDb();
  const tx = db.transaction(STORE_VERSIONS, "readwrite");
  tx.objectStore(STORE_VERSIONS).delete(versionId);
  await txPromise(tx);
}

export async function deleteStoredDocument(docId) {
  const db = await openDb();
  const versions = await listStoredVersions(docId);
  const tx = db.transaction([STORE_DOCS, STORE_VERSIONS], "readwrite");
  tx.objectStore(STORE_DOCS).delete(docId);
  const versStore = tx.objectStore(STORE_VERSIONS);
  for (const v of versions) versStore.delete(v.versionId);
  await txPromise(tx);
}

export async function clearAllStored() {
  const db = await openDb();
  const tx = db.transaction([STORE_DOCS, STORE_VERSIONS], "readwrite");
  tx.objectStore(STORE_DOCS).clear();
  tx.objectStore(STORE_VERSIONS).clear();
  await txPromise(tx);
}

function formatBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}

function formatStamp(ms) {
  if (!ms) return "";
  const d = new Date(ms);
  return `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })} ${d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" })}`;
}

export function makeVaultPanel({
  getState,
  getTitle,
  getDocId,
  setDocId,
  getBusy,
  buildProjectBytes,
  renderThumbDataUrl,
  openStoredBytes,
  downloadBytes,
  newId,
  run,
  status,
  showVaultTab,
}) {
  const $ = id => document.getElementById(id);
  let autoSaveTimer = null;
  let savingInFlight = false;
  let autoSaveEnabled = true;
  try {
    const pref = localStorage.getItem(AUTOSAVE_PREF_KEY);
    if (pref === "0") autoSaveEnabled = false;
  } catch {}

  const autoCheck = $("vault-autosave");
  if (autoCheck) {
    autoCheck.checked = autoSaveEnabled;
    autoCheck.addEventListener("change", () => {
      autoSaveEnabled = autoCheck.checked;
      try { localStorage.setItem(AUTOSAVE_PREF_KEY, autoSaveEnabled ? "1" : "0"); } catch {}
      appLog.storage(`Auto-save ${autoSaveEnabled ? "enabled" : "disabled"}`);
    });
  }

  async function saveSnapshotInternal(kind, label, prebuiltBytes = null, targetDocId = null, targetTitle = null) {
    const state = getState();
    if (!state) return null;
    const docId = targetDocId ?? getDocId();
    const docTitle = targetTitle ?? getTitle();
    const bytes = prebuiltBytes ?? await buildProjectBytes(state);
    const thumbDataUrl = renderThumbDataUrl(state);
    const res = await saveStoredVersion({
      docId,
      versionId: newId(),
      title: docTitle,
      label,
      kind,
      width: state.manifest.width,
      height: state.manifest.height,
      layerCount: state.manifest.layers.length,
      thumbDataUrl,
      bytes,
    });
    if (!res.skipped) {
      appLog.storage(`Saved ${kind} "${label}" for "${docTitle}" (${formatBytes(bytes.byteLength)}, ${res.versionCount} versions)`);
    }
    await refreshUI();
    return res;
  }

  function scheduleAutoSave(reason = "Auto-save") {
    if (!autoSaveEnabled) return;
    if (autoSaveTimer) clearTimeout(autoSaveTimer);
    autoSaveTimer = setTimeout(async () => {
      autoSaveTimer = null;
      if (!autoSaveEnabled || !getState() || getBusy() || savingInFlight) return;
      savingInFlight = true;
      try {
        await saveSnapshotInternal("autosave", reason);
      } catch (err) {
        appLog.warn("vault", `Auto-save skipped: ${err?.message ?? err}`);
      } finally {
        savingInFlight = false;
      }
    }, 2500);
  }

  async function recordExplicitVersion(kind, label, prebuiltBytes) {
    if (autoSaveTimer) {
      clearTimeout(autoSaveTimer);
      autoSaveTimer = null;
    }
    try {
      await saveSnapshotInternal(kind, label, prebuiltBytes);
    } catch (err) {
      appLog.warn("vault", `Could not archive ${kind} in browser storage: ${err?.message ?? err}`);
    }
  }

  async function refreshUI() {
    let docs = [];
    let versions = [];
    try {
      docs = await listStoredDocuments();
      const docId = getDocId();
      if (docId) versions = await listStoredVersions(docId);
    } catch (err) {
      const note = $("vault-usage");
      if (note) note.textContent = `Browser storage unavailable: ${err?.message ?? err}`;
      return;
    }

    // Update empty-state Resume button so users immediately see saved drafts after reload
    const resumeBtn = $("empty-resume");
    if (resumeBtn) {
      if (docs.length > 0) {
        const latest = docs[0];
        resumeBtn.hidden = false;
        resumeBtn.textContent = `Resume "${latest.title}" (${latest.versionCount || 1}v)`;
        resumeBtn.onclick = () => run("Restoring from browser storage…", async () => {
          const full = await getStoredDocument(latest.docId);
          if (!full) throw new Error("Stored document not found.");
          await openStoredBytes(full.bytes, `${full.title}.project.jxl`, full.docId);
        });
      } else {
        resumeBtn.hidden = true;
      }
    }

    const vaultTab = $("vault-tab");
    if (vaultTab) {
      vaultTab.textContent = docs.length ? `Versions (${docs.length})` : "Versions";
    }

    const hasDoc = Boolean(getState());
    if ($("vault-save-snapshot")) $("vault-save-snapshot").disabled = !hasDoc || getBusy();
    if ($("vault-save-copy")) $("vault-save-copy").disabled = !hasDoc || getBusy();
    if ($("vault-clear-all")) $("vault-clear-all").disabled = docs.length === 0 || getBusy();

    const usageEl = $("vault-usage");
    if (usageEl) {
      let estText = `${docs.length} saved project${docs.length === 1 ? "" : "s"}`;
      try {
        if (navigator.storage?.estimate) {
          const est = await navigator.storage.estimate();
          if (est.usage !== undefined && est.quota) {
            estText += ` · ${formatBytes(est.usage)} used of ${formatBytes(est.quota)}`;
          }
        }
      } catch {}
      usageEl.textContent = estText;
    }

    const vList = $("vault-versions-list");
    if (vList) {
      vList.replaceChildren();
      if (!versions.length) {
        const li = document.createElement("li");
        li.className = "muted-note";
        li.textContent = hasDoc
          ? "No snapshots saved yet for this document. Click Save version or Save project."
          : "Open a document to view its version snapshots.";
        vList.append(li);
      } else {
        for (const v of versions) {
          const li = document.createElement("li");
          li.className = "vault-card";
          const head = document.createElement("div");
          head.className = "vault-card-head";
          if (v.thumbDataUrl) {
            const img = document.createElement("img");
            img.src = v.thumbDataUrl;
            img.alt = "";
            img.className = "vault-thumb";
            head.append(img);
          }
          const info = document.createElement("div");
          info.className = "vault-meta";
          const titleRow = document.createElement("strong");
          titleRow.textContent = v.label || v.kind;
          const badge = document.createElement("span");
          badge.className = "badge";
          badge.textContent = v.kind;
          const sub = document.createElement("small");
          sub.textContent = `${formatStamp(v.createdAt)} · ${v.width}×${v.height} · ${formatBytes(v.byteLength)}`;
          info.append(titleRow, " ", badge, sub);
          head.append(info);

          const actions = document.createElement("div");
          actions.className = "vault-actions";
          const restore = document.createElement("button");
          restore.type = "button";
          restore.className = "pill-btn";
          restore.textContent = "Restore";
          restore.onclick = () => run("Restoring version…", async () => {
            const full = await getStoredVersion(v.versionId);
            if (!full) throw new Error("Version no longer exists in storage.");
            await openStoredBytes(full.bytes, `${full.title || getTitle()}.project.jxl`, full.docId);
            status(`Restored version: ${full.label}`);
          });
          const dl = document.createElement("button");
          dl.type = "button";
          dl.className = "pill-btn";
          dl.textContent = "Download";
          dl.onclick = async () => {
            const full = await getStoredVersion(v.versionId);
            if (!full) return;
            downloadBytes(full.bytes, `${full.title || getTitle()}-${v.kind}.project.jxl`);
            status("Downloaded stored version");
          };
          const del = document.createElement("button");
          del.type = "button";
          del.className = "pill-btn";
          del.textContent = "Delete";
          del.onclick = async () => {
            await deleteStoredVersion(v.versionId);
            appLog.storage(`Deleted version "${v.label}"`);
            await refreshUI();
          };
          actions.append(restore, dl, del);
          li.append(head, actions);
          vList.append(li);
        }
      }
    }

    const dList = $("vault-docs-list");
    if (dList) {
      dList.replaceChildren();
      if (!docs.length) {
        const li = document.createElement("li");
        li.className = "muted-note";
        li.textContent = "No copies saved in browser storage yet.";
        dList.append(li);
      } else {
        const activeDocId = getDocId();
        for (const doc of docs) {
          const li = document.createElement("li");
          li.className = "vault-card";
          li.toggleAttribute("data-current-doc", doc.docId === activeDocId);
          const head = document.createElement("div");
          head.className = "vault-card-head";
          if (doc.thumbDataUrl) {
            const img = document.createElement("img");
            img.src = doc.thumbDataUrl;
            img.alt = "";
            img.className = "vault-thumb";
            head.append(img);
          }
          const info = document.createElement("div");
          info.className = "vault-meta";
          const strong = document.createElement("strong");
          strong.textContent = doc.title;
          const badge = document.createElement("span");
          badge.className = "badge";
          badge.textContent = doc.docId === activeDocId ? "Current" : `${doc.versionCount || 1}v`;
          const sub = document.createElement("small");
          sub.textContent = `${formatStamp(doc.updatedAt)} · ${doc.width}×${doc.height} · ${doc.layerCount}L · ${formatBytes(doc.byteLength)}`;
          info.append(strong, " ", badge, sub);
          head.append(info);

          const actions = document.createElement("div");
          actions.className = "vault-actions";
          const openBtn = document.createElement("button");
          openBtn.type = "button";
          openBtn.className = "pill-btn";
          openBtn.textContent = "Open";
          openBtn.onclick = () => run("Opening stored project…", async () => {
            const full = await getStoredDocument(doc.docId);
            if (!full) throw new Error("Stored copy not found.");
            await openStoredBytes(full.bytes, `${full.title}.project.jxl`, full.docId);
          });
          const dlBtn = document.createElement("button");
          dlBtn.type = "button";
          dlBtn.className = "pill-btn";
          dlBtn.textContent = "Download";
          dlBtn.onclick = async () => {
            const full = await getStoredDocument(doc.docId);
            if (!full) return;
            downloadBytes(full.bytes, `${full.title}.project.jxl`);
            status("Downloaded stored project copy");
          };
          const delBtn = document.createElement("button");
          delBtn.type = "button";
          delBtn.className = "pill-btn";
          delBtn.textContent = "Delete";
          delBtn.onclick = async () => {
            await deleteStoredDocument(doc.docId);
            appLog.storage(`Deleted stored project "${doc.title}"`);
            await refreshUI();
          };
          actions.append(openBtn, dlBtn, delBtn);
          li.append(head, actions);
          dList.append(li);
        }
      }
    }
  }

  $("vault-save-snapshot")?.addEventListener("click", () => run("Saving version snapshot…", async () => {
    const labelInput = $("vault-label");
    const customLabel = labelInput?.value?.trim();
    const label = customLabel || `Snapshot ${new Date().toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}`;
    await saveSnapshotInternal("snapshot", label);
    if (labelInput) labelInput.value = "";
    status(`Version "${label}" saved to browser storage`);
  }));

  $("vault-save-copy")?.addEventListener("click", () => run("Saving independent copy…", async () => {
    const labelInput = $("vault-label");
    const customTitle = labelInput?.value?.trim();
    const copyTitle = customTitle || `${getTitle()} (copy)`;
    const copyId = newId();
    await saveSnapshotInternal("copy", `Copy of ${getTitle()}`, null, copyId, copyTitle);
    if (labelInput) labelInput.value = "";
    status(`Saved copy "${copyTitle}" in browser storage`);
  }));

  $("vault-clear-all")?.addEventListener("click", async () => {
    if (!confirm("Delete all saved copies and version snapshots from browser storage?")) return;
    await clearAllStored();
    appLog.storage("Cleared all browser storage copies & versions");
    await refreshUI();
    status("Browser storage cleared");
  });

  $("empty-vault")?.addEventListener("click", () => {
    if (showVaultTab) showVaultTab();
    refreshUI();
  });

  refreshUI();
  return { refreshUI, scheduleAutoSave, recordExplicitVersion };
}
