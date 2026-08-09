// libfw.js — libfw-client SDK integration.
//
// All file transfers (uploads + single-file/folder downloads) go through the
// libfw WASM SDK (`libfw-client`, loaded from vendor/libfw-client.js),
// configured from the backend:
//   - `window.ONESHARE_BASE`  — where `/file` and `/dir` are mounted (URL prefix)
//   - `window.ONESHARE_LIBFW` — SDK options from `[libfw]` in config.toml,
//     served by the backend's `/config.js`.
//
// Since libfw-client 0.1.3 the SDK handles both download paths itself:
//   - File System Access API (`showDirectoryPicker`) when available — the
//     download streams into a user-picked directory.
//   - a native in-browser fallback (`downloadMode: 'auto'`) when it is not —
//     single files are saved via a normal browser download, folders are
//     packed into a `.zip` and downloaded.
// No hand-rolled fetch/XHR/ZIP code remains in the client.
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
    // just the leaf segment into the picked directory. libfw-client 0.1.3's
    // `_ensureFileHandle` returns `{ dir, name, handle }`, so the leaf
    // override must return the same shape.
    async _ensureFileHandle(path) {
      if (this._downloadAsLeaf) {
        const segs = String(path).split('/').filter(Boolean);
        const leaf = segs.length ? segs[segs.length - 1] : 'download';
        const handle = await this._dirHandle.getFileHandle(leaf, { create: true });
        return { dir: this._dirHandle, name: leaf, handle };
      }
      return super._ensureFileHandle(path);
    }
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
        // A file manager always wants a FRESH upload (a stale persisted
        // "upload complete" for the same path would otherwise make the SDK
        // skip a re-upload of an unchanged file). libfw-client 0.1.3 exposes
        // the targeted `clearResumeStore('upload')` for exactly this; it is
        // best-effort (transfers still work if IndexedDB is unavailable).
        try { await client.clearResumeStore('upload'); } catch (e) { /* best-effort */ }
        this._activeOnEvent = onEvent;
        try {
          return await client.upload(token, items);
        } finally {
          this._activeOnEvent = null;
        }
      });
    },

    // Download a folder. `dirPath` is a display (virtual) path; the backend
    // resolves it. libfw-client 0.1.3 chooses how to save it itself: streamed
    // into a user-picked directory (FS API) or packed into a `.zip` browser
    // download (`downloadMode: 'auto'`).
    downloadFolder(token, dirPath, onEvent) {
      return this._enqueue(async () => {
        const client = this._getClient('');
        if (!client) throw new Error('libfw-client SDK not loaded');
        // Always start a folder download fresh (stale resume offsets were the
        // historical cause of truncated files + leftover `.crswap` temps).
        try { await client.clearResumeStore('download'); } catch (e) { /* best-effort */ }
        this._activeOnEvent = onEvent;
        try {
          return await client.downloadFolder(token, dirPath || '');
        } finally {
          this._activeOnEvent = null;
        }
      });
    },

    // Download a single file via the SDK's `downloadFile`. `path` is the
    // display (virtual) path — the backend resolves it. As with the folder
    // case, libfw-client 0.1.3 saves via the FS API when available, else
    // through a traditional browser download (leaf filename).
    downloadFile(token, path, onEvent) {
      return this._enqueue(async () => {
        const client = this._getClient('');
        if (!client) throw new Error('libfw-client SDK not loaded');
        // Fresh full download (same rationale as downloadFolder).
        try { await client.clearResumeStore('download'); } catch (e) { /* best-effort */ }
        this._activeOnEvent = onEvent;
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
