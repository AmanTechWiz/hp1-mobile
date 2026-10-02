// OpenHP1 browser host.
//
// Imports the player's own original installation into IndexedDB, serves it to
// the wasm build synchronously, persists OpenHP1 settings and saves, and starts
// the game. Game files never leave the device.

const DB_NAME = "openhp1";
const STORE = "files";
// Keep these in sync with openhp1_package::fs::{WEB_GAME_ROOT, WEB_SETTINGS_DIR}.
const GAME_ROOT = "/game";
const SETTINGS_DIR = "/settings";
// Package discovery only needs the four-byte magic; keep a little more.
const PREFIX_BYTES = 16;
// Small text files are read often, so they stay resident.
const RESIDENT_BYTES = 256 * 1024;
// How long a reload waits for pending IndexedDB writes before giving up.
const FLUSH_TIMEOUT_MS = 2000;

const element = (id) => document.getElementById(id);

// ---------------------------------------------------------------------------
// IndexedDB

function settle(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

function openDatabase() {
  const request = indexedDB.open(DB_NAME, 1);
  request.onupgradeneeded = () => request.result.createObjectStore(STORE);
  return settle(request);
}

function transaction(db, mode, run) {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE, mode);
    run(tx.objectStore(STORE));
    tx.oncomplete = () => resolve();
    tx.onerror = (event) => reject(storageError(event.target.error ?? tx.error));
    tx.onabort = () => reject(storageError(tx.error));
  });
}

async function loadStoredFiles(db) {
  const files = new Map();
  await transaction(db, "readonly", (store) => {
    store.openCursor().onsuccess = (event) => {
      const cursor = event.target.result;
      if (cursor) {
        files.set(cursor.key, cursor.value);
        cursor.continue();
      }
    };
  });
  return files;
}

function storageError(error) {
  const reason = error?.message || error?.name || "the transaction was aborted";
  return new Error(
    `Browser storage refused the game files (${reason}). Private Browsing cannot store ` +
      "them, and the device needs enough free space for the whole installation.",
  );
}

function directoryRange(directory) {
  // "0" sorts directly after "/", so this covers every key below the directory.
  return IDBKeyRange.bound(`${directory}/`, `${directory}0`, false, true);
}

// ---------------------------------------------------------------------------
// Importing an installation

// Matches Default.ini in System or in a language subdirectory such as
// System/0, which some releases use instead. Group 1 is the installation root.
const DEFAULT_INI = /^(|.*\/)system\/(?:[^/]+\/)?default\.ini$/i;

function installationEntries(entries) {
  const roots = entries
    .map((entry) => DEFAULT_INI.exec(entry.path)?.[1])
    .filter((root) => root !== undefined)
    .sort((left, right) => left.length - right.length);
  if (roots.length === 0) {
    throw new Error(
      "Could not find System/Default.ini. Choose the game folder that contains " +
        "the System, Maps, and Textures folders, or a ZIP archive of it.",
    );
  }
  const prefix = roots[0];
  return entries
    .filter((entry) => entry.path.startsWith(prefix))
    .map((entry) => ({ ...entry, path: `${GAME_ROOT}/${entry.path.slice(prefix.length)}` }));
}

function ignoredArchivePath(path) {
  return path.endsWith("/") || path.startsWith("__MACOSX/") || /(^|\/)\._/.test(path);
}

async function zipEntries(file) {
  const tailSize = Math.min(file.size, 22 + 0xffff);
  const tail = new DataView(await file.slice(file.size - tailSize).arrayBuffer());
  let end = -1;
  for (let at = tail.byteLength - 22; at >= 0; at -= 1) {
    if (tail.getUint32(at, true) === 0x06054b50) {
      end = at;
      break;
    }
  }
  if (end < 0) {
    throw new Error(`${file.name} is not a ZIP archive.`);
  }
  let count = tail.getUint16(end + 10, true);
  let size = tail.getUint32(end + 12, true);
  let offset = tail.getUint32(end + 16, true);
  if (count === 0xffff || size === 0xffffffff || offset === 0xffffffff) {
    const locator = end - 20;
    if (locator < 0 || tail.getUint32(locator, true) !== 0x07064b50) {
      throw new Error(`${file.name} has an unreadable ZIP64 directory.`);
    }
    const recordOffset = Number(tail.getBigUint64(locator + 8, true));
    const record = new DataView(await file.slice(recordOffset, recordOffset + 56).arrayBuffer());
    count = Number(record.getBigUint64(32, true));
    size = Number(record.getBigUint64(40, true));
    offset = Number(record.getBigUint64(48, true));
  }

  const directory = new DataView(await file.slice(offset, offset + size).arrayBuffer());
  const decoder = new TextDecoder();
  const entries = [];
  let at = 0;
  for (let index = 0; index < count; index += 1) {
    if (directory.getUint32(at, true) !== 0x02014b50) {
      throw new Error(`${file.name} has a damaged ZIP directory.`);
    }
    const flags = directory.getUint16(at + 8, true);
    const method = directory.getUint16(at + 10, true);
    let compressed = directory.getUint32(at + 20, true);
    let uncompressed = directory.getUint32(at + 24, true);
    const nameLength = directory.getUint16(at + 28, true);
    const extraLength = directory.getUint16(at + 30, true);
    const commentLength = directory.getUint16(at + 32, true);
    let local = directory.getUint32(at + 42, true);
    const name = decoder
      .decode(new Uint8Array(directory.buffer, at + 46, nameLength))
      .replaceAll("\\", "/");

    let extra = at + 46 + nameLength;
    const extraEnd = extra + extraLength;
    while (extra + 4 <= extraEnd) {
      const id = directory.getUint16(extra, true);
      const length = directory.getUint16(extra + 2, true);
      if (id === 0x0001) {
        let field = extra + 4;
        const next = () => {
          const value = Number(directory.getBigUint64(field, true));
          field += 8;
          return value;
        };
        if (uncompressed === 0xffffffff) uncompressed = next();
        if (compressed === 0xffffffff) compressed = next();
        if (local === 0xffffffff) local = next();
      }
      extra += 4 + length;
    }

    if (!ignoredArchivePath(name)) {
      entries.push({
        path: name,
        size: uncompressed,
        blob: () => zipEntryBlob(file, { name, flags, method, compressed, local }),
      });
    }
    at += 46 + nameLength + extraLength + commentLength;
  }
  return entries;
}

async function zipEntryBlob(file, entry) {
  const header = new DataView(await file.slice(entry.local, entry.local + 30).arrayBuffer());
  if (header.getUint32(0, true) !== 0x04034b50) {
    throw new Error(`${entry.name} has a damaged ZIP header.`);
  }
  if (entry.flags & 1) {
    throw new Error(`${entry.name} is encrypted.`);
  }
  const start = entry.local + 30 + header.getUint16(26, true) + header.getUint16(28, true);
  const data = file.slice(start, start + entry.compressed);
  if (entry.method === 0) {
    return data;
  }
  if (entry.method === 8) {
    const stream = data.stream().pipeThrough(new DecompressionStream("deflate-raw"));
    return new Response(stream).blob();
  }
  throw new Error(`${entry.name} uses unsupported ZIP compression method ${entry.method}.`);
}

function folderEntries(fileList) {
  return Array.from(fileList, (file) => ({
    path: file.webkitRelativePath || file.name,
    size: file.size,
    blob: () => file,
  }));
}

async function importInstallation(db, entries, progress) {
  const files = installationEntries(entries.filter((entry) => !ignoredArchivePath(entry.path)));
  const total = files.reduce((sum, file) => sum + file.size, 0);
  await transaction(db, "readwrite", (store) => store.delete(directoryRange(GAME_ROOT)));
  let done = 0;
  for (const file of files) {
    progress(`Importing ${file.path.slice(GAME_ROOT.length + 1)}`, done / Math.max(total, 1));
    const blob = await file.blob();
    await transaction(db, "readwrite", (store) => store.put(blob, file.path));
    done += file.size;
  }
  progress("Import complete", 1);
  await navigator.storage?.persist?.().catch(() => false);
  return files.length;
}

// ---------------------------------------------------------------------------
// Synchronous reads for the game

// Browsers only read Blobs synchronously through a synchronous XHR. A
// window-scoped XHR cannot return an ArrayBuffer, but x-user-defined maps every
// byte to one UTF-16 code unit whose low byte is the original byte.
function readBlobSync(blob) {
  const url = URL.createObjectURL(blob);
  try {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", url, false);
    xhr.overrideMimeType("text/plain; charset=x-user-defined");
    xhr.send();
    if (xhr.status !== 200 && xhr.status !== 0) {
      throw new Error(`blob read failed with status ${xhr.status}`);
    }
    const text = xhr.responseText;
    const bytes = new Uint8Array(text.length);
    for (let index = 0; index < text.length; index += 1) {
      bytes[index] = text.charCodeAt(index);
    }
    return bytes;
  } finally {
    URL.revokeObjectURL(url);
  }
}

// Text decoding honors byte order marks, so files starting with one cannot
// use readBlobSync and are kept resident instead.
function startsWithByteOrderMark(bytes) {
  return (
    (bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) ||
    (bytes[0] === 0xfe && bytes[1] === 0xff) ||
    (bytes[0] === 0xff && bytes[1] === 0xfe)
  );
}

function syncReadsWork() {
  try {
    const expected = Uint8Array.of(0xc1, 0x83, 0x2a, 0x9e, 0x00, 0x7f, 0x80, 0xff);
    const actual = readBlobSync(new Blob([expected]));
    return actual.length === expected.length && actual.every((byte, index) => byte === expected[index]);
  } catch {
    return false;
  }
}

class Host {
  constructor(db, stored) {
    this.db = db;
    this.stored = stored;
    this.resident = new Map();
    this.prefixes = new Map();
    // IndexedDB transactions that are dispatched but not settled yet. The
    // browser runs overlapping readwrite transactions on one store in creation
    // order, so dispatching each write when it arrives preserves write order
    // without a queue that could strand a save behind the tab closing.
    this.writes = new Set();
    // Writes the browser refused; their bytes exist only in wasm memory, so
    // they are lost as soon as the page reloads.
    this.failed = new Map();
    // One counter per path: a write that fails after a newer write for the
    // same path was dispatched must not report over the newer write's state.
    this.generation = new Map();
  }

  async prepare(progress) {
    const keepAll = !syncReadsWork();
    if (keepAll) {
      console.warn("Synchronous Blob reads are unavailable; keeping game files in memory.");
    }
    const entries = Array.from(this.stored);
    let done = 0;
    const next = async () => {
      while (entries.length > 0) {
        const [path, blob] = entries.pop();
        const prefix = new Uint8Array(await blob.slice(0, PREFIX_BYTES).arrayBuffer());
        if (keepAll || blob.size <= RESIDENT_BYTES || startsWithByteOrderMark(prefix)) {
          this.resident.set(path, new Uint8Array(await blob.arrayBuffer()));
        } else {
          this.prefixes.set(path, prefix);
        }
        done += 1;
        if (done % 64 === 0) {
          progress(`Preparing game files (${done}/${this.stored.size})`, done / this.stored.size);
        }
      }
    };
    await Promise.all(Array.from({ length: 8 }, next));
  }

  // The functions below are called by the wasm build.

  files() {
    return Array.from(this.stored, ([path, blob]) => [path, blob.size]);
  }

  read(path, limit) {
    const resident = this.resident.get(path);
    if (resident) {
      return limit === undefined ? resident : resident.subarray(0, limit);
    }
    if (limit !== undefined && limit <= PREFIX_BYTES && this.prefixes.has(path)) {
      return this.prefixes.get(path).subarray(0, limit);
    }
    const blob = this.stored.get(path);
    if (!blob) {
      throw new Error(`${path} is not stored`);
    }
    return readBlobSync(limit === undefined ? blob : blob.slice(0, limit));
  }

  persist(path, contents) {
    if (contents) {
      const bytes = contents.slice();
      const blob = new Blob([bytes]);
      this.stored.set(path, blob);
      this.resident.set(path, bytes);
    } else {
      this.stored.delete(path);
      this.resident.delete(path);
      this.prefixes.delete(path);
    }
    this.generation.set(path, (this.generation.get(path) ?? 0) + 1);
    this.failed.delete(path);
    this.dispatch(path);
    updateStorageWarning(this);
  }

  // Starts one IndexedDB transaction. The put/delete is issued before this
  // returns, so an unload that follows immediately cannot outrun it: with the
  // old promise chain the transaction only began after every earlier write had
  // committed, and a reload or tab close in that window silently dropped it.
  dispatch(path) {
    const generation = this.generation.get(path) ?? 0;
    const blob = this.stored.get(path);
    let committed;
    try {
      committed = transaction(this.db, "readwrite", (store) =>
        blob ? store.put(blob, path) : store.delete(path),
      );
    } catch (error) {
      this.recordFailure(path, generation, error);
      return;
    }
    const tracked = committed.then(
      () => this.writes.delete(tracked),
      (error) => {
        this.writes.delete(tracked);
        this.recordFailure(path, generation, error);
      },
    );
    this.writes.add(tracked);
  }

  recordFailure(path, generation, error) {
    if ((this.generation.get(path) ?? 0) !== generation) {
      // A newer write for this path decides whether the file is stored.
      return;
    }
    console.error(`Could not persist ${path}`, error);
    this.failed.set(path, error);
    updateStorageWarning(this);
  }

  // Re-dispatches refused writes. pagehide can start a transaction even
  // though it cannot wait for one, so this is a last chance for writes that
  // failed for a transient reason.
  retryFailed() {
    for (const path of [...this.failed.keys()]) {
      this.failed.delete(path);
      this.dispatch(path);
    }
    updateStorageWarning(this);
  }

  // Waits until every dispatched write has committed or failed. Failures
  // settle the promise too, so this always resolves.
  async flush() {
    while (this.writes.size > 0) {
      await Promise.all([...this.writes]);
    }
  }

  hasPendingWrites() {
    return this.writes.size > 0;
  }

  // The persistence state the wasm build could query through
  // `openhp1Host.storageStatus()`. Nothing in Rust reads it yet; the page uses
  // it for the on-screen storage warning.
  storageStatus() {
    return { pending: this.writes.size, failed: [...this.failed.keys()] };
  }

  status(message) {
    showProgress(message);
  }

  nextFrame() {
    return new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  }

  // The screen's `[top, right, bottom, left]` safe-area insets in CSS pixels:
  // the notch, rounded corners, and home indicator. The canvas covers the whole
  // screen (`viewport-fit=cover`), so on-screen controls must stay clear of
  // them.
  safeArea() {
    if (!this.safeAreaProbe) {
      const probe = document.createElement("div");
      probe.style.cssText =
        "position:fixed;visibility:hidden;pointer-events:none;left:0;top:0;width:0;height:0;" +
        "padding:env(safe-area-inset-top) env(safe-area-inset-right) " +
        "env(safe-area-inset-bottom) env(safe-area-inset-left)";
      document.body.appendChild(probe);
      this.safeAreaProbe = probe;
    }
    const style = getComputedStyle(this.safeAreaProbe);
    return [
      style.paddingTop,
      style.paddingRight,
      style.paddingBottom,
      style.paddingLeft,
    ].map((value) => parseFloat(value) || 0);
  }

  ready() {
    document.body.classList.add("playing");
    element("openhp1-canvas").focus();
  }

  fail(message) {
    document.body.classList.remove("playing");
    showError(message);
  }

  async exited() {
    // The wasm build ignores the return value, so waiting here only delays
    // the reload. Give in-flight writes a bounded window to commit; a stuck
    // transaction must not hang the exit forever.
    await Promise.race([
      this.flush(),
      new Promise((resolve) => setTimeout(resolve, FLUSH_TIMEOUT_MS)),
    ]);
    location.reload();
  }
}

// ---------------------------------------------------------------------------
// Audio

// Browsers start audio suspended until a user gesture, and iOS only resumes it
// inside one. The game creates its AudioContext after loading, so resume every
// context on each gesture.
function installAudioUnlock() {
  const contexts = new Set();
  const Native = window.AudioContext ?? window.webkitAudioContext;
  if (!Native) {
    return;
  }
  window.AudioContext = class extends Native {
    constructor(...args) {
      super(...args);
      contexts.add(this);
    }
  };
  const resume = () => {
    for (const context of contexts) {
      if (context.state !== "running") {
        context.resume().catch(() => {});
      }
    }
  };
  for (const type of ["pointerdown", "touchend", "keydown"]) {
    window.addEventListener(type, resume, { capture: true });
  }
  if (navigator.audioSession) {
    // Play through the ringer switch like other games.
    navigator.audioSession.type = "playback";
  }
}

// ---------------------------------------------------------------------------
// Page

function showProgress(message, fraction) {
  element("error").hidden = true;
  element("progress").hidden = false;
  element("progress-text").textContent = message;
  const bar = element("progress-bar");
  if (fraction === undefined) {
    bar.removeAttribute("value");
  } else {
    bar.value = fraction;
  }
}

function showError(message) {
  element("progress").hidden = true;
  element("error").hidden = false;
  element("error-text").textContent = message;
}

// IndexedDB refusing a write never reaches the wasm build: `persist` returns
// nothing (web.rs declares `host_persist` without `catch`), and an exception
// thrown here would unwind through wasm instead of reaching Rust's error
// handling. So a refused write is shown on the page itself, where the player
// can see that their save is only in memory.
let storageWarning = null;

function updateStorageWarning(host) {
  const failed = [...host.failed.keys()];
  if (failed.length === 0) {
    storageWarning?.remove();
    storageWarning = null;
    return;
  }
  if (!storageWarning) {
    storageWarning = document.createElement("p");
    storageWarning.id = "storage-warning";
    storageWarning.style.cssText =
      "position:fixed; z-index:1; top:0; left:0; right:0; margin:0; " +
      "padding:0.5rem 0.75rem calc(0.5rem + env(safe-area-inset-top)); " +
      "text-align:center; font-size:0.8rem; line-height:1.4; " +
      "background:#5b1d1d; color:#ffd9d9; cursor:default;";
    document.body.appendChild(storageWarning);
  }
  const shown = failed.slice(0, 3).join(", ");
  const more = failed.length > 3 ? ` and ${failed.length - 3} more` : "";
  storageWarning.textContent =
    `Browser storage refused ${shown}${more}. These files are only in memory: ` +
    "they will be lost when the page closes, and saving again may fail too.";
}

function formatBytes(bytes) {
  const units = ["B", "KB", "MB", "GB"];
  let unit = 0;
  while (bytes >= 1024 && unit < units.length - 1) {
    bytes /= 1024;
    unit += 1;
  }
  return `${bytes.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}

async function describeInstallation(db) {
  const files = await loadStoredFiles(db);
  const game = Array.from(files).filter(([path]) => path.startsWith(`${GAME_ROOT}/`));
  const installed = game.length > 0;
  const bytes = game.reduce((sum, [, blob]) => sum + blob.size, 0);
  element("installation").textContent = installed
    ? `${game.length} game files (${formatBytes(bytes)}) are stored in this browser.`
    : "No game files have been imported yet.";
  element("play").disabled = !installed;
  element("remove").disabled = !installed;
  return files;
}

async function start(db) {
  element("setup").hidden = true;
  try {
    document.documentElement.requestFullscreen?.().catch(() => {});
    navigator.wakeLock?.request("screen").catch(() => {});
    showProgress("Reading game files");
    const host = new Host(db, await loadStoredFiles(db));
    await host.prepare(showProgress);
    window.openhp1Host = host;
    showProgress("Starting OpenHP1");
    const game = await import("./openhp1.js");
    await game.default();
  } catch (error) {
    showError(error?.message ?? String(error));
  }
}

async function main() {
  installAudioUnlock();
  document.addEventListener("gesturestart", (event) => event.preventDefault());

  // Closing or reloading the page cannot await queued IndexedDB writes, but
  // the browser keeps the page (and its in-flight transactions) alive while a
  // beforeunload dialog is open, so hold the unload only while a write is
  // actually pending. pagehide gets one last synchronous dispatch for writes
  // that were refused earlier.
  window.addEventListener("beforeunload", (event) => {
    if (window.openhp1Host?.hasPendingWrites()) {
      event.preventDefault();
    }
  });
  window.addEventListener("pagehide", () => {
    window.openhp1Host?.retryFailed();
    if (window.openhp1Host?.hasPendingWrites()) {
      console.warn("Leaving the page with writes pending", window.openhp1Host.storageStatus());
    }
  });
  if (!navigator.gpu) {
    element("webgpu").hidden = false;
  }

  // Listen before the database opens so an early selection is not lost.
  const database = openDatabase();

  const runImport = async (entries) => {
    element("setup").hidden = true;
    try {
      const db = await database;
      const count = await importInstallation(db, await entries, showProgress);
      element("progress").hidden = true;
      element("setup").hidden = false;
      await describeInstallation(db);
      element("installation").textContent += ` Imported ${count} files.`;
    } catch (error) {
      showError(error?.message ?? String(error));
      element("setup").hidden = false;
    }
  };

  element("folder-input").addEventListener("change", (event) => {
    if (event.target.files.length > 0) {
      runImport(folderEntries(event.target.files));
    }
    event.target.value = "";
  });
  element("zip-input").addEventListener("change", (event) => {
    const file = event.target.files[0];
    if (file) {
      runImport(zipEntries(file));
    }
    event.target.value = "";
  });
  element("remove").addEventListener("click", async () => {
    if (!confirm("Remove the imported game files from this browser? Saves and settings are kept.")) {
      return;
    }
    const db = await database;
    await transaction(db, "readwrite", (store) => store.delete(directoryRange(GAME_ROOT)));
    await describeInstallation(db);
  });
  element("reset").addEventListener("click", async () => {
    if (!confirm("Delete OpenHP1 settings and saved games from this browser?")) {
      return;
    }
    const db = await database;
    await transaction(db, "readwrite", (store) => store.delete(directoryRange(SETTINGS_DIR)));
    await describeInstallation(db);
  });
  element("play").addEventListener("click", async () => start(await database));
  element("reload").addEventListener("click", async () => {
    // A reload from this button is user-initiated, so let pending writes
    // commit first instead of racing them against the page teardown.
    const host = window.openhp1Host;
    if (host) {
      await Promise.race([
        host.flush(),
        new Promise((resolve) => setTimeout(resolve, FLUSH_TIMEOUT_MS)),
      ]);
    }
    location.reload();
  });
  await describeInstallation(await database);
}

main().catch((error) => showError(error?.message ?? String(error)));
