// libfw.js — libfw-client SDK integration.
//
// All file transfers (uploads + single-file/folder downloads) go through the
// libfw WASM SDK (`libfw-client`, loaded from vendor/libfw-client.js),
// configured from the backend:
//   - `window.ONESHARE_BASE`  — where `/file` and `/dir` are mounted (URL prefix)
//   - `window.ONESHARE_LIBFW` — SDK options from `[libfw]` in config.toml,
//     served by the backend's `/config.js`.
//
// Since libfw-client 0.3.0 the SDK drives every transfer over plain HTTP
// (`/file`, `/dir`) with parallel Range GETs (download) and concurrent
// chunked POSTs (upload), and handles both download save paths itself:
//   - File System Access API (`showDirectoryPicker`) when available — the
//     download streams into a user-picked directory.
//   - a native in-browser fallback (`downloadMode: 'auto'`) when it is not —
//     single files are saved via a normal browser download, folders are
//     packed into a `.zip` and downloaded.
// The SDK sends the opaque shadow paths (`v1.…`) from `/api/files/token`
// and `/dir` listings; the embedded libfw server decrypts them back to the
// real paths it authorizes, so real filesystem paths never reach the
// browser. No hand-rolled fetch/XHR/ZIP code remains in the client.
(function () {
  'use strict';

  // ── Config served by the backend (/config.js) ──
  const base = ((typeof window.ONESHARE_BASE === 'string' && window.ONESHARE_BASE) || '').replace(/\/+$/, '');
  const served = (typeof window.ONESHARE_LIBFW === 'object' && window.ONESHARE_LIBFW) || {};
  const opts = {
    compress: served.compress !== false,
    concurrency: typeof served.concurrency === 'number' ? served.concurrency : 4,
    chunkSize: typeof served.chunkSize === 'number' ? served.chunkSize : 2 * 1024 * 1024,
    // Per-file scheduling window (parallel chunks in flight per file).
    // Total in-flight chunks ≈ concurrency × uploadWindow; defaults to
    // concurrency so uploads stay bounded by the configured knob.
    uploadWindow: typeof served.uploadWindow === 'number'
      ? served.uploadWindow
      : (typeof served.concurrency === 'number' ? served.concurrency : 4),
    // Download-side mirror of uploadWindow: parallel byte ranges (chunks)
    // fetched per file. Fallback matches the SDK's own downloadWindow
    // default, which is also the concurrency value.
    downloadWindow: typeof served.downloadWindow === 'number'
      ? served.downloadWindow
      : (typeof served.concurrency === 'number' ? served.concurrency : 4),
    maxRetries: typeof served.maxRetries === 'number' ? served.maxRetries : 3,
    baseRetryDelayMs: typeof served.baseRetryDelayMs === 'number' ? served.baseRetryDelayMs : 500,
    maxRetryDelayMs: typeof served.maxRetryDelayMs === 'number' ? served.maxRetryDelayMs : 30000,
    // The libfw engine applies timeoutMs as a PER-READ timeout on HTTP
    // transfers and aborts the whole transfer if any single read stalls
    // longer than it. Keep the fallback generous (10 min) so it never kills
    // active transfers.
    timeoutMs: typeof served.timeoutMs === 'number' ? served.timeoutMs : 600000,
    // Adaptive tuning (libfw-client >= 0.3.3): when enabled the engine probes
    // the server's public /capabilities advertisement and TCP-style ramps
    // concurrency / windows / chunk sizes from real transfer stats, persisting
    // a settled result per origin for tuneTtlMs. The static knobs above are
    // the starting/minimum values. Tuning updates arrive as
    // `{ type: 'tuning', phase, params, stats }` events.
    autoTune: served.autoTune === true,
    tuneTtlMs: typeof served.tuneTtlMs === 'number' ? served.tuneTtlMs : 3600000,
  };

  // Latest adaptive-tuning state (phase + params + last-window stats), kept
  // on the facade so the UI can render a live tuning readout without losing
  // events fired before the transfers panel subscribed. `onTuningChange` is a
  // hook the UI sets once on boot; every `{ type: 'tuning' }` event is both
  // recorded here and forwarded through it.
  const tuningState = {
    phase: null,
    params: null,
    stats: null,
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

  // ── Upload client: plan paths are per-file opaque shadows ──
  // The SDK POSTs to `/file/{path}` for each plan entry; libfw decrypts the
  // whole URL path, so every entry is a full shadow minted per file (via
  // `/api/files/token`) at planning time — display dest + rel never reaches
  // the server as a path, and the File is registered under its shadow key
  // for `readFile`.
  class OneshareLibfwClient extends LibfwClientClass {
    constructor(options) {
      super(options);
      this._uploadDest = '';
      // Display path → opaque shadow, kept for the lifetime of the page so a
      // retried/failed upload of the same file reuses its shadow (and token
      // scope) instead of minting a new one — which would lose the SDK's
      // chunk-level resume state (keys are the plan paths).
      this._shadowCache = new Map();
      // Shadow → display path, populated lazily while downloads run so the
      // SDK writes files/dirs under their real names instead of `v1.…`
      // shadows.
      this._displayMap = new Map();
      this._nameResolves = new Map();
      // Leaf name for single-file downloads (the explorer passes the display
      // name it already has — no extra resolve needed).
      this._leafName = null;
    }
    async _collectProvidedFiles(files) {
      // Every plan entry must be an opaque FULL shadow of its real path: the
      // embedded libfw server decrypts the entire URL path with the AES codec,
      // so it cannot append literal segments to a directory shadow. Mint one
      // per-file shadow via `/api/files/token` and use it as the plan path.
      // (The upload token itself — the directory token from the initial
      // getToken — still covers every child: libfw validates on the DECODED
      // real path with prefix semantics, `docs` ⊇ `docs/a.md`.)
      const items = Array.from(files || []);
      const resolved = await Promise.all(items.map(async (it) => {
        const file = it instanceof File ? it : (it && it.file);
        if (!(file instanceof File)) return null;
        const rel = (it && it.relPath) || file.webkitRelativePath || file.name;
        const display = this._uploadDest ? this._uploadDest + '/' + rel : rel;
        let shadow = this._shadowCache.get(display);
        if (!shadow) {
          const resp = await API.getToken(display, 'write');
          shadow = resp.path;
          this._shadowCache.set(display, shadow);
        }
        return { file, shadow };
      }));
      const plan = [];
      for (const r of resolved) {
        if (!r) continue;
        this._uploadFiles.set(r.shadow, r.file);
        plan.push({
          path: r.shadow,
          size: r.file.size,
          mtime: Math.floor(r.file.lastModified / 1e3),
        });
      }
      plan.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
      return plan;
    }

    // Single-file download: the SDK writes `filePath` preserving its path
    // structure under the picked directory, but in a file manager a single
    // file should land at the ROOT of the picked directory under its own
    // (leaf) name (`leafName` is the display name from the listing row; the
    // transfer path is an opaque shadow, unusable as a filename).
    async downloadFile(token, filePath, leafName) {
      this._downloadAsLeaf = true;
      this._leafName = leafName || null;
      try {
        return await super.downloadFile(token, filePath);
      } finally {
        this._downloadAsLeaf = false;
        this._leafName = null;
      }
    }

    // Folder download: shadows are minted per listing with fresh random
    // nonces, so no pre-walk can correlate names across the SDK's own `/dir`
    // requests — resolution must happen per path, lazily, from the paths the
    // SDK actually uses (fileStart/writeChunk callbacks).
    async downloadFolder(token, filePath) {
      return super.downloadFolder(token, filePath);
    }

    // Shadow → display path, resolved lazily from `/api/files/names` and
    // cached. Used so downloads write files/dirs under their real names
    // instead of opaque `v1.…` shadows. Concurrent callers share one
    // in-flight request per shadow.
    _ensureMapped(shadow) {
      if (this._displayMap.has(shadow)) return Promise.resolve(this._displayMap.get(shadow));
      if (this._nameResolves.has(shadow)) return this._nameResolves.get(shadow);
      const base = this._options.baseUrl || '';
      const p = fetch(base + '/api/files/names?paths=' + encodeURIComponent(shadow), {
        credentials: 'same-origin',
      })
        .then((r) => {
          if (!r.ok) throw new Error('name resolution failed: HTTP ' + r.status);
          return r.json();
        })
        .then((m) => {
          const display = m[shadow] || shadow;
          this._displayMap.set(shadow, display);
          return display;
        })
        .finally(() => this._nameResolves.delete(shadow));
      this._nameResolves.set(shadow, p);
      return p;
    }

    _flushNameResolves() {
      return Promise.allSettled([...this._nameResolves.values()]);
    }

    // The SDK emits fileStart before a file's data flows; kick the name
    // resolution off immediately so the (sync) zip build at the end never
    // waits on it.
    _emit(ev) {
      if (ev && ev.type === 'fileStart') this._ensureMapped(ev.path).catch(() => {});
      return super._emit(ev);
    }

    // While `_downloadAsLeaf` is set, ignore the path structure and write
    // just the leaf segment into the picked directory. libfw-client 0.1.3's
    // `_ensureFileHandle` returns `{ dir, name, handle }`, so the leaf
    // override must return the same shape. Mapped display names are used
    // everywhere so downloads never carry `v1.…` shadow names.
    async _ensureFileHandle(path) {
      if (!this._downloadAsLeaf) {
        const display = await this._ensureMapped(path);
        return super._ensureFileHandle(display);
      }
      const segs = String(this._displayMap.get(path) || path).split('/').filter(Boolean);
      const leaf = this._leafName || (segs.length ? segs[segs.length - 1] : 'download');
      const handle = await this._dirHandle.getFileHandle(leaf, { create: true });
      return { dir: this._dirHandle, name: leaf, handle };
    }

    _safeEntryName(path) {
      return super._safeEntryName(this._displayMap.get(path) || path);
    }

    _downloadName(path) {
      if (this._downloadAsLeaf && this._leafName) return this._leafName;
      return super._downloadName(this._displayMap.get(path) || path);
    }

    // Copy of libfw-client's browser-download fallback with one addition:
    // pending shadow→display resolutions are flushed before the zip entries
    // are named (names are looked up synchronously there). Keep in sync with
    // the vendored SDK.
    async _downloadViaBrowser(engine, token, path, isFolder) {
      this._fallback = { isFolder, buffers: new Map(), order: [], sizes: new Map(), total: 0 };
      try {
        const r = isFolder
          ? await engine.download_folder(this._options.baseUrl, token, path)
          : await engine.download_file(this._options.baseUrl, token, path);
        await this._flushNameResolves();
        const { buffers: bufs, order, sizes } = this._fallback;
        if (isFolder) {
          const d = [];
          for (const w of order) {
            d.push({ name: this._safeEntryName(w), data: this._concatBuffers(bufs.get(w)?.chunks || []) });
          }
          for (const k of sizes.keys()) {
            if (!bufs.has(k)) d.push({ name: this._safeEntryName(k), data: new Uint8Array(0) });
          }
          this._triggerBrowserDownload(this._zipEntries(d), this._archiveName(path));
        } else {
          const data = this._concatBuffers(bufs.get(path)?.chunks || []);
          this._triggerBrowserDownload(new Blob([data], { type: 'application/octet-stream' }), this._downloadName(path));
        }
        return r;
      } finally {
        this._fallback = null;
      }
    }

    // Zip builder — same output as the vendored SDK's internal `V()`.
    _zipEntries(items) {
      let offset = 0;
      const crcTable = new Uint32Array(256);
      for (let n = 0; n < 256; n++) {
        let c = n;
        for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
        crcTable[n] = c >>> 0;
      }
      const crc32 = (bytes) => {
        let c = 0xffffffff;
        for (const b of bytes) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
        return (c ^ 0xffffffff) >>> 0;
      };
      const enc = new TextEncoder();
      const chunks = [];
      const entries = [];
      for (const it of items) {
        const nameBytes = enc.encode(it.name);
        const data = it.data;
        const crc = crc32(data);
        const head = new Uint8Array(30 + nameBytes.length);
        const dv = new DataView(head.buffer);
        dv.setUint32(0, 0x04034b50, true);
        dv.setUint16(4, 20, true);
        dv.setUint16(6, 0, true);
        dv.setUint16(8, 0, true);
        dv.setUint16(10, 0, true);
        dv.setUint16(12, 33, true);
        dv.setUint32(14, crc, true);
        dv.setUint32(18, data.length, true);
        dv.setUint32(22, data.length, true);
        dv.setUint16(26, nameBytes.length, true);
        dv.setUint16(28, 0, true);
        head.set(nameBytes, 30);
        chunks.push(head, data);
        entries.push({ nameBytes, crc, size: data.length, offset });
        offset += head.length + data.length;
      }
      const central = [];
      let cdOffset = offset;
      for (const e of entries) {
        const rec = new Uint8Array(46 + e.nameBytes.length);
        const dv = new DataView(rec.buffer);
        dv.setUint32(0, 0x02014b50, true);
        dv.setUint16(4, 20, true);
        dv.setUint16(6, 20, true);
        dv.setUint16(8, 0, true);
        dv.setUint16(10, 0, true);
        dv.setUint16(12, 33, true);
        dv.setUint32(16, e.crc, true);
        dv.setUint32(20, e.size, true);
        dv.setUint32(24, e.size, true);
        dv.setUint16(28, e.nameBytes.length, true);
        dv.setUint16(30, 0, true);
        dv.setUint32(32, 0, true);
        dv.setUint32(36, 0, true);
        dv.setUint32(42, e.offset, true);
        rec.set(e.nameBytes, 46);
        central.push(rec);
      }
      const end = new Uint8Array(22);
      const edv = new DataView(end.buffer);
      edv.setUint32(0, 0x06054b50, true);
      edv.setUint16(8, entries.length, true);
      edv.setUint16(10, entries.length, true);
      edv.setUint32(12, central.reduce((s, c) => s + c.length, 0), true);
      edv.setUint32(16, cdOffset, true);
      chunks.push(...central, end);
      return new Blob(chunks, { type: 'application/zip' });
    }
  }

  // ── Libfw facade ──
  const Libfw = {
    base,
    opts,

    _client: null,
    _chain: Promise.resolve(),
    _activeOnEvent: null,

    // Latest tuning state (see `tuningState` above) + UI hook.
    tuning: tuningState,
    onTuningChange: null,

    _handleEvent(ev) {
      if (ev && ev.type === 'tuning') {
        tuningState.phase = ev.phase;
        tuningState.params = ev.params || null;
        tuningState.stats = ev.stats || null;
        if (typeof this.onTuningChange === 'function') this.onTuningChange(tuningState);
      }
      if (typeof this._activeOnEvent === 'function') this._activeOnEvent(ev);
    },

    _getClient(destPath) {
      if (!this._client && LibfwClientClass) {
        this._client = new OneshareLibfwClient({
          baseUrl: base,
          concurrency: opts.concurrency,
          compress: opts.compress,
          // One knob controls every chunk size: upload chunks AND download
          // byte ranges both use `chunk_size` (otherwise the SDK falls back
          // to its own 256 KiB download default).
          chunkSize: opts.chunkSize,
          downloadChunkSize: opts.chunkSize,
          uploadWindow: opts.uploadWindow,
          downloadWindow: opts.downloadWindow,
          maxRetries: opts.maxRetries,
          baseRetryDelayMs: opts.baseRetryDelayMs,
          maxRetryDelayMs: opts.maxRetryDelayMs,
          timeoutMs: opts.timeoutMs,
          autoTune: opts.autoTune,
          tuneTtlMs: opts.tuneTtlMs,
          onEvent: (ev) => this._handleEvent(ev),
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

    // Download a folder. `dirPath` is an opaque shadow from the token
    // endpoint; the wrapper pre-walks it to map shadows back to display
    // names, then libfw-client 0.1.3 saves how it likes: streamed into a
    // user-picked directory (FS API) or packed into a `.zip` browser
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
    // opaque shadow bound to the token; `name` is the display leaf name used
    // for the saved file. As with the folder case, libfw-client 0.1.3 saves
    // via the FS API when available, else through a traditional browser
    // download (leaf filename).
    downloadFile(token, path, name, onEvent) {
      return this._enqueue(async () => {
        const client = this._getClient('');
        if (!client) throw new Error('libfw-client SDK not loaded');
        // Fresh full download (same rationale as downloadFolder).
        try { await client.clearResumeStore('download'); } catch (e) { /* best-effort */ }
        this._activeOnEvent = onEvent;
        try {
          return await client.downloadFile(token, path, name);
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
