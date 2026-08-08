// libfw.js — libfw-client SDK integration.
//
// All file transfers (uploads, folder downloads) go through the libfw WASM SDK
// (`libfw-client`, loaded from vendor/libfw-client.js), configured from the
// backend:
//   - `window.ONESHARE_BASE`  — where `/file` and `/dir` are mounted (URL prefix)
//   - `window.ONESHARE_LIBFW` — SDK options from `[libfw]` in config.toml,
//     served by the backend's `/config.js`.
//
// The SDK's only download API is `downloadFolder` (File System Access API);
// single files are downloaded with a plain fetch to the libfw `/file/{path}`
// endpoint, since the SDK exposes no single-file download.
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

  // ── Chinese filenames: keep `x-libfw-file-meta` a valid Latin-1 header ──
  // The SDK serializes `{ path, size, ... }` (raw UTF-8) into that header;
  // browsers reject non ISO-8859-1 header values. JSON-escape non-ASCII so the
  // header stays valid — serde_json on the server decodes `\uXXXX` back.
  (function patchHeaders() {
    if (typeof Headers === 'undefined' || typeof Headers.prototype.set !== 'function') return;
    const jsonEscape = (s) => s.replace(/[^\u0000-\u00ff]/g, (ch) => {
      const cp = ch.codePointAt(0);
      if (cp <= 0xffff) return '\\u' + cp.toString(16).padStart(4, '0');
      const hi = 0xd800 + ((cp - 0x10000) >> 10);
      const lo = 0xdc00 + ((cp - 0x10000) & 0x3ff);
      return '\\u' + hi.toString(16).padStart(4, '0') + '\\u' + lo.toString(16).padStart(4, '0');
    });
    const sanitize = (name, value) =>
      name === 'x-libfw-file-meta' && typeof value === 'string' && /[^\u0000-\u00ff]/.test(value)
        ? jsonEscape(value)
        : value;
    const origSet = Headers.prototype.set;
    Headers.prototype.set = function (name, value) {
      return origSet.call(this, name, sanitize(name, value));
    };
    const origAppend = Headers.prototype.append;
    Headers.prototype.append = function (name, value) {
      return origAppend.call(this, name, sanitize(name, value));
    };
  })();

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
  }

  // ── Helpers ──
  const encodePath = (path) => String(path).split('/').map(encodeURIComponent).join('/');

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

  function triggerDownload(blob, filename) {
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  // ── Libfw facade ──
  const Libfw = {
    base,
    opts,

    _client: null,
    _chain: Promise.resolve(),
    _activeOnEvent: null,
    _fileController: null,

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
        this._activeOnEvent = onEvent || null;
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
        this._activeOnEvent = onEvent || null;
        try {
          return await client.downloadFolder(token, dirPath || '');
        } finally {
          this._activeOnEvent = null;
        }
      });
    },

    // Download a single file via the libfw `/file/{path}` endpoint. The SDK
    // has no single-file API, so this is a plain fetch (no zrip negotiation →
    // the server sends raw bytes, no decompression needed).
    downloadFile(token, path, name, onEvent) {
      const controller = new AbortController();
      this._fileController = controller;
      const url = base + '/file/' + encodePath(path);
      return (async () => {
        const res = await fetch(url, {
          headers: { Authorization: 'Bearer ' + token },
          signal: controller.signal,
        });
        if (!res.ok) throw new LibfwError(`HTTP ${res.status} for \`${path}\``, 'http');
        const total = parseInt(res.headers.get('content-length') || '0', 10) || 0;
        if (typeof onEvent === 'function') onEvent({ type: 'progress', done: 0, total });
        let received = 0;
        const chunks = [];
        const reader = res.body ? res.body.getReader() : null;
        if (reader) {
          for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            chunks.push(value);
            received += value.byteLength;
            if (typeof onEvent === 'function') onEvent({ type: 'progress', done: received, total });
          }
        } else {
          const buf = await res.arrayBuffer();
          chunks.push(new Uint8Array(buf));
          received = buf.byteLength;
          if (typeof onEvent === 'function') onEvent({ type: 'progress', done: received, total });
        }
        triggerDownload(new Blob(chunks), name);
        return received;
      })().finally(() => {
        if (this._fileController === controller) this._fileController = null;
      });
    },

    // Cancel the active transfer (SDK engine + any single-file fetch).
    cancel() {
      if (this._fileController) { try { this._fileController.abort(); } catch (e) { /* noop */ } }
      if (this._client) { try { this._client.cancel(); } catch (e) { /* noop */ } }
    },
    pause() { if (this._client) { try { this._client.pause(); } catch (e) { /* noop */ } } },
    resume() { if (this._client) { try { this._client.resume(); } catch (e) { /* noop */ } } },
  };

  window.Libfw = Libfw;
})();
