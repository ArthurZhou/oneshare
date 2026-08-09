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

    // Download a single file as a Blob over the libfw `/file/{path}` endpoint
    // with the read token. This is the traditional-download path used by
    // browsers WITHOUT the File System Access API (no `showDirectoryPicker`),
    // where the SDK's `downloadFile`/`downloadFolder` cannot save files. XHR
    // (not fetch) gives us download progress and an abort handle for the
    // transfers panel. `path` is the display (virtual) path — the server
    // resolves it and libfw serves identity bytes.
    fetchBlob(path, token, onEvent) {
      return this._enqueue(() => new Promise((resolve, reject) => {
        // Per-segment encode so `/` stays a separator (matches the API client).
        const enc = String(path).split('/').map(encodeURIComponent).join('/');
        const xhr = new XMLHttpRequest();
        xhr.open('GET', `${base}/file/${enc}`, true);
        xhr.responseType = 'blob';
        xhr.setRequestHeader('Authorization', `Bearer ${token}`);
        xhr.onprogress = (e) => {
          if (e.lengthComputable && typeof onEvent === 'function') {
            onEvent({ type: 'progress', done: e.loaded, total: e.total });
          }
        };
        xhr.onload = () => {
          if (this._activeXhr === xhr) this._activeXhr = null;
          if (xhr.status >= 200 && xhr.status < 300) resolve(xhr.response);
          else reject(new Error(`Download failed (HTTP ${xhr.status})`));
        };
        xhr.onerror = () => {
          if (this._activeXhr === xhr) this._activeXhr = null;
          reject(new Error('Network error during download'));
        };
        xhr.onabort = () => {
          if (this._activeXhr === xhr) this._activeXhr = null;
          const err = new Error('Download cancelled');
          err.code = 'cancelled';
          reject(err);
        };
        this._activeXhr = xhr;
        xhr.send();
      }));
    },

    // Minimal ZIP writer (STORE / no compression) used by the folder-download
    // fallback for browsers without the File System Access API. `entries` is
    // an Array of `{ path, blob }` (`path` = entry name inside the archive).
    // Pure client-side, no dependencies. NOTE: the archive is assembled in
    // browser memory (≈ total folder size + overhead), so this is best suited
    // to small/medium folders; very large folders should prefer a server-side
    // zip endpoint.
    async buildZip(entries) {
      const enc = new TextEncoder();
      // CRC-32 table (standard polynomial 0xEDB88320).
      const crcTable = (() => {
        const t = new Uint32Array(256);
        for (let n = 0; n < 256; n++) {
          let c = n;
          for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
          t[n] = c >>> 0;
        }
        return t;
      })();
      const crc32 = (data) => {
        let c = 0xffffffff;
        for (let i = 0; i < data.length; i++) c = crcTable[(c ^ data[i]) & 0xff] ^ (c >>> 8);
        return (c ^ 0xffffffff) >>> 0;
      };
      const dosDateTime = () => {
        const d = new Date();
        const time = (d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1);
        const date = ((d.getFullYear() - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate();
        return { time, date };
      };

      const body = [];
      const central = [];
      let offset = 0;

      for (const entry of entries) {
        const name = String(entry.path);
        const nameBytes = enc.encode(name);
        const data = entry.blob
          ? new Uint8Array(await entry.blob.arrayBuffer())
          : new Uint8Array(0);
        const crc = crc32(data);
        const { time, date } = dosDateTime();

        // Local file header (signature, version, flags, method, dos time/date,
        // crc, sizes, name len, extra len, name).
        const lfh = new Uint8Array(30);
        const dv = new DataView(lfh.buffer);
        dv.setUint32(0, 0x04034b50, true);
        dv.setUint16(4, 20, true);
        dv.setUint16(6, 0x0800, true);   // general purpose bit 11 = UTF-8 names
        dv.setUint16(8, 0, true);        // method 0 = STORE
        dv.setUint16(10, time, true);
        dv.setUint16(12, date, true);
        dv.setUint32(14, crc, true);
        dv.setUint32(18, data.length, true); // compressed size
        dv.setUint32(22, data.length, true); // uncompressed size
        dv.setUint16(26, nameBytes.length, true);
        dv.setUint16(28, 0, true);
        body.push(lfh, nameBytes, data);

        // Central directory record.
        const cd = new Uint8Array(46);
        const cv = new DataView(cd.buffer);
        cv.setUint32(0, 0x02014b50, true);
        cv.setUint16(4, 20, true);        // version made by
        cv.setUint16(6, 20, true);        // version needed
        cv.setUint16(8, 0x0800, true);
        cv.setUint16(10, 0, true);        // method 0 = STORE
        cv.setUint16(12, time, true);
        cv.setUint16(14, date, true);
        cv.setUint32(16, crc, true);
        cv.setUint32(20, data.length, true);
        cv.setUint32(24, data.length, true);
        cv.setUint16(28, nameBytes.length, true);
        cv.setUint16(30, 0, true);        // extra len
        cv.setUint16(32, 0, true);        // comment len
        cv.setUint16(34, 0, true);        // disk number start
        cv.setUint16(36, 0, true);        // internal attrs
        cv.setUint32(38, 0, true);        // external attrs
        cv.setUint32(42, offset, true);   // local header offset
        central.push(cd, nameBytes);

        offset += 30 + nameBytes.length + data.length;
      }

      // End of central directory record.
      const centralSize = central.reduce((s, c) => s + c.length, 0);
      const eocd = new Uint8Array(22);
      const ev = new DataView(eocd.buffer);
      ev.setUint32(0, 0x06054b50, true);
      ev.setUint16(4, 0, true);
      ev.setUint16(6, 0, true);
      ev.setUint16(8, entries.length, true);
      ev.setUint16(10, entries.length, true);
      ev.setUint32(12, centralSize, true);
      ev.setUint32(16, offset, true);
      ev.setUint16(20, 0, true);

      return new Blob([...body, ...central, eocd], { type: 'application/zip' });
    },

    // Cancel the active transfer (SDK engine or in-flight fallback XHR).
    cancel() {
      if (this._activeXhr) { try { this._activeXhr.abort(); } catch (e) { /* noop */ } }
      if (this._client) { try { this._client.cancel(); } catch (e) { /* noop */ } }
    },
    pause() { if (this._client) { try { this._client.pause(); } catch (e) { /* noop */ } } },
    resume() { if (this._client) { try { this._client.resume(); } catch (e) { /* noop */ } } },
  };

  window.Libfw = Libfw;
})();
