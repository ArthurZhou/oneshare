// libfw.js — libfw-client SDK integration.
//
// All file transfers (uploads, folder downloads) go through the libfw WASM SDK
// (`libfw-client`, loaded from vendor/libfw-client.js), configured from the
// backend:
//   - `window.ONESHARE_BASE`  — where `/file` and `/dir` are mounted (URL prefix)
//   - `window.ONESHARE_LIBFW` — SDK options from `[libfw]` in config.toml,
//     served by the backend's `/config.js`.
//
// The SDK exposes `downloadFolder` (folder downloads) and, since libfw-client
// 0.1.2, `downloadFile` (single-file downloads) — both via the File System
// Access API. Every transfer operation (upload, folder download, single-file
// download) therefore goes through the SDK; no hand-rolled fetch/chunk code
// remains in the client.
(function () {
  'use strict';

  // ── Config served by the backend (/config.js) ──
  const base = ((typeof window.ONESHARE_BASE === 'string' && window.ONESHARE_BASE) || '').replace(/\/+$/, '');
  const served = (typeof window.ONESHARE_LIBFW === 'object' && window.ONESHARE_LIBFW) || {};
  const opts = {
    compress: served.compress !== false,
    concurrency: typeof served.concurrency === 'number' ? served.concurrency : 4,
    chunkSize: typeof served.chunkSize === 'number' ? served.chunkSize : 2 * 1024 * 1024,
    maxRetries: typeof served.maxRetries === 'number' ? served.maxRetries : 3,
    baseRetryDelayMs: typeof served.baseRetryDelayMs === 'number' ? served.baseRetryDelayMs : 500,
    maxRetryDelayMs: typeof served.maxRetryDelayMs === 'number' ? served.maxRetryDelayMs : 30000,
    timeoutMs: typeof served.timeoutMs === 'number' ? served.timeoutMs : 60000,
  };

  // ── SDK classes (the UMD bundle exports them on window.LibfwClient) ──
  const Sdk = window.LibfwClient || {};
  const LibfwClientClass = Sdk.LibfwClient || Sdk.default;
  const LibfwError = Sdk.LibfwError ||
    class LibfwError extends Error {
      constructor(message, code) {
        super(message);
        this.name = 'LibfwError';
        this.code = code || 'unknown';
      }
    };

  if (typeof LibfwClientClass !== 'function') {
    console.error('[oneshare] libfw-client SDK not loaded — file transfers will be unavailable');
  }

  // ── `x-libfw-file-meta` is base64 since libfw 0.1.2 ──
  // libfw-core 0.1.2's `encode_file_meta_header` (compiled into the WASM
  // engine) base64-encodes the meta JSON on the wire, so CJK filenames
  // survive HTTP headers with no JS-side header patching.

  // ── Upload client: plan paths are display paths under the current dir ──
  // The SDK POSTs to `/file/{path}` for each plan entry, so we prefix every
  // file's relative path with the upload destination (the current virtual
  // directory) and register the File under that same key for `readFile`.
  class OneshareLibfwClient extends LibfwClientClass {
    constructor(options) {
      super(options);
      this._uploadDest = '';
    }
    async _collectProvidedFiles(files) {
      const plan = [];
      for (const it of Array.from(files || [])) {
        const file = it instanceof File ? it : (it && it.file);
        if (!(file instanceof File)) continue;
        const rel = (it && it.relPath) || file.webkitRelativePath || file.name;
        const path = this._uploadDest ? this._uploadDest + '/' + rel : rel;
        this._uploadFiles.set(path, file);
        plan.push({ path, size: file.size, mtime: Math.floor(file.lastModified / 1e3) });
      }
      plan.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
      return plan;
    }

    // Single-file download: the SDK writes `filePath` preserving its path
    // structure under the picked directory, but in a file manager a single
    // file should land at the ROOT of the picked directory under its own
    // (leaf) name. Flag leaf-only writes for the duration of this call.
    async downloadFile(token, filePath) {
      this._downloadAsLeaf = true;
      try {
        return await super.downloadFile(token, filePath);
      } finally {
        this._downloadAsLeaf = false;
      }
    }

    // While `_downloadAsLeaf` is set, ignore the path structure and write
    // just the leaf segment into the picked directory.
    async _ensureFileHandle(path) {
      if (this._downloadAsLeaf) {
        const segs = String(path).split('/').filter(Boolean);
        const leaf = segs.length ? segs[segs.length - 1] : 'download';
        return this._dirHandle.getFileHandle(leaf, { create: true });
      }
      return super._ensureFileHandle(path);
    }
  }

  // ── Resume-state helper ──

  // The libfw SDK keeps upload AND download resume state in ONE IndexedDB
  // store (`libfw` / `resume`, keyed by path). That cross-contamination bites
  // hard: a completed DOWNLOAD of `x` leaves `{ offset: size }`, so a later
  // UPLOAD of the same path skips every chunk and reports 100% while writing
  // nothing (and vice-versa a stale offset can corrupt a resume). Since we
  // always want fresh, complete transfers in a file manager, wipe the resume
  // store before starting an upload or a folder download.
  function clearResumeStore() {
    return new Promise((resolve) => {
      if (typeof indexedDB === 'undefined') { resolve(); return; }
      let req;
      try { req = indexedDB.open('libfw', 1); } catch (e) { resolve(); return; }
      req.onupgradeneeded = () => {
        try { if (!req.result.objectStoreNames.contains('resume')) req.result.createObjectStore('resume'); } catch (e) { /* noop */ }
      };
      req.onsuccess = () => {
        try {
          const db = req.result;
          const tx = db.transaction('resume', 'readwrite');
          tx.objectStore('resume').clear();
          tx.oncomplete = () => { try { db.close(); } catch (e) { /* noop */ } resolve(); };
          tx.onerror = () => { try { db.close(); } catch (e) { /* noop */ } resolve(); };
        } catch (e) { resolve(); }
      };
      req.onerror = () => resolve();
    });
  }

  // ── Libfw facade ──
  const Libfw = {
    base,
    opts,

    _client: null,
    _chain: Promise.resolve(),
    _activeOnEvent: null,

    _getClient(destPath) {
      if (!this._client && LibfwClientClass) {
        this._client = new OneshareLibfwClient({
          baseUrl: base,
          concurrency: opts.concurrency,
          compress: opts.compress,
          chunkSize: opts.chunkSize,
          maxRetries: opts.maxRetries,
          baseRetryDelayMs: opts.baseRetryDelayMs,
          maxRetryDelayMs: opts.maxRetryDelayMs,
          timeoutMs: opts.timeoutMs,
          onEvent: (ev) => {
            if (typeof this._activeOnEvent === 'function') this._activeOnEvent(ev);
          },
        });
      }
      if (this._client) this._client._uploadDest = destPath || '';
      return this._client;
    },

    // Run SDK operations one at a time — the WASM engine drives a single
    // active transfer per client instance.
    _enqueue(fn) {
      const run = this._chain.then(() => fn());
      this._chain = run.catch(() => {});
      return run;
    },

    // Upload `items` (Array<{ file, relPath }>) into `destPath` (display path).
    upload(destPath, token, items, onEvent) {
      return this._enqueue(async () => {
        const client = this._getClient(destPath);
        if (!client) throw new Error('libfw-client SDK not loaded');
        // Wipe stale resume state so a previous download/upload of the same
        // path can't make the SDK skip (or corrupt) this upload.
        await clearResumeStore();
        // libfw 0.1.2 reports byte-progress as done:0 (TaskControl clone
        // issue, see downloadFolder) — compute file-level progress from the
        // fileStart/fileCompleted events instead, and swallow the raw ones.
        const sizes = new Map();
        let done = 0, total = 0;
        const wrapped = (ev) => {
          if (ev.type === 'fileStart') {
            sizes.set(ev.path, ev.total || 0);
            total += ev.total || 0;
            if (typeof onEvent === 'function') onEvent({ type: 'progress', done, total });
          } else if (ev.type === 'fileCompleted') {
            done += sizes.get(ev.path) || 0;
            if (typeof onEvent === 'function') onEvent({ type: 'progress', done, total });
          } else if (ev.type === 'progress') {
            // Swallow the engine's raw byte-progress (always done:0 in 0.1.2).
          } else if (typeof onEvent === 'function') onEvent(ev);
        };
        this._activeOnEvent = wrapped;
        try {
          return await client.upload(token, items);
        } finally {
          this._activeOnEvent = null;
        }
      });
    },

    // Download a folder into a user-picked directory (File System Access API).
    // `dirPath` is a display (virtual) path; the backend resolves it.
    downloadFolder(token, dirPath, onEvent) {
      return this._enqueue(async () => {
        const client = this._getClient('');
        if (!client) throw new Error('libfw-client SDK not loaded');
        // Always start a folder download fresh: stale resume offsets are the
        // main cause of truncated files and leftover `.crswap` temps.
        await clearResumeStore();
        // libfw 0.1.2's WASM engine reports download byte-progress from a
        // CLONED TaskControl (`#[derive(Clone)]` over `Cell`s copies the
        // counter instead of sharing it), so `done` stays 0 and the returned
        // byte count is 0 too — even though the files download correctly.
        // Compute file-level progress from the SDK's fileStart/fileCompleted
        // events instead, and return the summed total.
        const sizes = new Map();
        let done = 0, total = 0;
        const wrapped = (ev) => {
          if (ev.type === 'fileStart') {
            sizes.set(ev.path, ev.total || 0);
            total += ev.total || 0;
            if (typeof onEvent === 'function') onEvent({ type: 'progress', done, total });
          } else if (ev.type === 'fileCompleted') {
            done += sizes.get(ev.path) || 0;
            if (typeof onEvent === 'function') onEvent({ type: 'progress', done, total });
          } else if (ev.type === 'progress') {
            // Swallow the engine's raw byte-progress (always done:0 in 0.1.2)
            // — the synthetic events above carry the correct values.
          } else if (typeof onEvent === 'function') onEvent(ev);
        };
        this._activeOnEvent = wrapped;
        try {
          const bytes = await client.downloadFolder(token, dirPath || '');
          return total || bytes;
        } finally {
          this._activeOnEvent = null;
        }
      });
    },

    // Download a single file via the SDK's `downloadFile` (File System Access
    // API → the user picks a destination directory; the file is saved with
    // its own leaf name). `path` is the display (virtual) path — the backend
    // resolves it and libfw streams identity bytes (the SDK only negotiates
    // zrip when the server actually serves it).
    downloadFile(token, path, onEvent) {
      return this._enqueue(async () => {
        const client = this._getClient('');
        if (!client) throw new Error('libfw-client SDK not loaded');
        // Start fresh: stale resume offsets are the main cause of truncated
        // files (see downloadFolder).
        await clearResumeStore();
        // Same 0.1.2 done:0 progress issue — emit file-level progress. Note
        // 0.1.2's `download_single` reports fileStart total as 0 (size is
        // unknown up front), so only emit when the size is actually known.
        let size = 0;
        const wrapped = (ev) => {
          if (ev.type === 'fileStart') {
            size = ev.total || 0;
            if (size > 0 && typeof onEvent === 'function') onEvent({ type: 'progress', done: 0, total: size });
          } else if (ev.type === 'fileCompleted') {
            if (size > 0 && typeof onEvent === 'function') onEvent({ type: 'progress', done: size, total: size });
          } else if (ev.type === 'progress') {
            // Swallow the engine's raw byte-progress (always done:0 in 0.1.2).
          } else if (typeof onEvent === 'function') onEvent(ev);
        };
        this._activeOnEvent = wrapped;
        try {
          return await client.downloadFile(token, path);
        } finally {
          this._activeOnEvent = null;
        }
      });
    },

    // Cancel the active transfer (SDK engine).
    cancel() {
      if (this._client) { try { this._client.cancel(); } catch (e) { /* noop */ } }
    },
    pause() { if (this._client) { try { this._client.pause(); } catch (e) { /* noop */ } } },
    resume() { if (this._client) { try { this._client.resume(); } catch (e) { /* noop */ } } },
  };

  window.Libfw = Libfw;
})();
