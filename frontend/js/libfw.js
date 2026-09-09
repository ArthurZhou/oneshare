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
  // Release builds serve the embedded WASM with long-lived immutable caching,
  // keyed by the bundle version (`?v=…`, see statics.rs `static_version`).
  // Mirror it here so the SDK fetches the versioned URL and never reuses a
  // stale wasm after an upgrade. Debug builds serve no-cache, so this stays
  // empty there and the SDK uses its default script-relative resolution.
  const bundleVersion = (typeof window.ONESHARE_VERSION === 'string' && window.ONESHARE_VERSION) || '';
  const served = (typeof window.ONESHARE_LIBFW === 'object' && window.ONESHARE_LIBFW) || {};
  const opts = {
    compress: served.compress !== false,
    concurrency: typeof served.concurrency === 'number' ? served.concurrency : 4,
    chunkSize: typeof served.chunkSize === 'number' ? served.chunkSize : 2 * 1024 * 1024,
    downloadChunkSize: typeof served.downloadChunkSize === 'number'
      ? served.downloadChunkSize
      : (typeof served.chunkSize === 'number' ? served.chunkSize : 2 * 1024 * 1024),
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

  // Try to fetch the server's `/capabilities` advertisement and apply any
  // advertised `default` start values into `opts`. This is best-effort and
  // does not block SDK initialization; it helps the client start with the
  // same starting point the server advises (reduces race between SDK and
  // server when tuning is enabled).
  (async function fetchCapabilitiesAndMerge() {
    try {
      const url = (base || '') + '/capabilities';
      const res = await fetch(url, { cache: 'no-store', credentials: 'same-origin' });
      if (!res.ok) return;
      const j = await res.json();
      const limits = j && j.limits;
      if (!limits || typeof limits !== 'object') return;

      const getDefault = (k) => {
        const v = limits[k];
        if (!v) return undefined;
        if (typeof v === 'object' && v.default != null) return Number(v.default);
        return undefined;
      };

      const maybeSet = (field, val) => {
        if (typeof val === 'number' && !Number.isNaN(val)) opts[field] = val;
      };

      // Range knobs: use advertised `default` when present.
      maybeSet('concurrency', getDefault('concurrency') ?? opts.concurrency);
      maybeSet('uploadWindow', getDefault('uploadWindow') ?? opts.uploadWindow);
      maybeSet('downloadWindow', getDefault('downloadWindow') ?? opts.downloadWindow);
      // chunkSize may be the unified knob; use it first.
      maybeSet('chunkSize', getDefault('chunkSize') ?? opts.chunkSize);
      // downloadChunkSize fallback: prefer its default if present.
      maybeSet('downloadChunkSize', getDefault('downloadChunkSize') ?? opts.downloadChunkSize);

      // Scalar knobs
      if (limits.maxRetries != null && typeof limits.maxRetries === 'number') {
        opts.maxRetries = limits.maxRetries;
      }
      if (limits.timeoutMs != null && typeof limits.timeoutMs === 'number') {
        opts.timeoutMs = limits.timeoutMs;
      }
    } catch (e) {
      // best-effort: ignore network/parse errors
    }
  })();

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

  if (typeof LibfwClientClass !== 'function') {
    console.error('[oneshare] libfw-client SDK not loaded — file transfers will be unavailable');
  }

  // The upstream libfw 0.4.x SDK already covers the functionality we used to
  // patch ourselves: directory-handle reuse, display-name mapping, and upload
  // path resolution are all exposed as public options. Keep only the app-level
  // integration here and let the SDK own the heavy lifting.
  const displayNameCache = new Map();
  const displayNameResolves = new Map();

  async function resolveDisplayName(shadow) {
    const key = String(shadow || '');
    if (!key) return key;
    if (displayNameCache.has(key)) return displayNameCache.get(key);
    if (displayNameResolves.has(key)) return displayNameResolves.get(key);

    const p = fetch((base || '') + '/api/files/names?paths=' + encodeURIComponent(key), {
      credentials: 'same-origin',
    })
      .then((r) => {
        if (!r.ok) throw new Error('name resolution failed: HTTP ' + r.status);
        return r.json();
      })
      .then((m) => {
        const display = m[key] || key;
        displayNameCache.set(key, display);
        return display;
      })
      .finally(() => displayNameResolves.delete(key));

    displayNameResolves.set(key, p);
    return p;
  }

  // ── Libfw facade ──
  const Libfw = {
    base,
    opts,

    _client: null,
    _chain: Promise.resolve(),
    _activeOnEvent: null,
    _dirHandle: null,
    _batchReuse: false,

    async ensureDirectoryHandle() {
      if (this._dirHandle) return this._dirHandle;
      if (typeof window === 'undefined' || typeof window.showDirectoryPicker !== 'function') {
        throw new Error('当前浏览器不支持目录选择，无法按分片方式下载。');
      }
      try {
        const handle = await window.showDirectoryPicker();
        this._dirHandle = handle;
        return handle;
      } catch (e) {
        const msg = e && (e.name || e.code) ? String(e.name || e.code) : String(e);
        if (msg.includes('SecurityError') || msg.includes('User activation')) {
          throw new Error('下载需要在点击后立即选择目录，请重试一次。');
        }
        throw e;
      }
    },

    // Per-transfer identity: `_activeId` is the transfer currently running
    // in the engine; `_cancelledIds` holds ids of QUEUED transfers that were
    // cancelled before they started. The historical `cancel()` hit whatever
    // the engine happened to be doing, so pressing cancel on a queued task
    // killed the *active* one instead. Now `cancel(id)` only ever cancels
    // the matching transfer: the active one is aborted in the engine, a
    // queued one is flagged and skipped when its turn comes.
    _nextId: 1,
    _activeId: null,
    _cancelledIds: new Set(),

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
        this._client = new LibfwClientClass({
          baseUrl: base,
          concurrency: opts.concurrency,
          compress: opts.compress,
          chunkSize: opts.chunkSize,
          downloadChunkSize: opts.downloadChunkSize,
          uploadWindow: opts.uploadWindow,
          downloadWindow: opts.downloadWindow,
          maxRetries: opts.maxRetries,
          baseRetryDelayMs: opts.baseRetryDelayMs,
          maxRetryDelayMs: opts.maxRetryDelayMs,
          timeoutMs: opts.timeoutMs,
          autoTune: opts.autoTune,
          tuneTtlMs: opts.tuneTtlMs,
          wasmUrl: bundleVersion
            ? (base || '') + '/vendor/libfw_client_bg.wasm?v=' + encodeURIComponent(bundleVersion)
            : undefined,
          onEvent: (ev) => this._handleEvent(ev),
          directoryHandle: async () => {
            if (this._batchReuse && this._dirHandle) return this._dirHandle;
            return this.ensureDirectoryHandle();
          },
          resolveDisplayName: async (shadow) => {
            const value = await resolveDisplayName(shadow);
            return value || shadow;
          },
          resolveUploadPath: async (entry, defaultPath) => {
            const raw = (entry && typeof entry === 'object' && typeof entry.relPath === 'string' && entry.relPath)
              ? entry.relPath
              : defaultPath;
            const rel = String(raw || '').replace(/^\/+/, '');
            if (!rel) return defaultPath || '';
            return this._uploadDirShadow ? `${this._uploadDirShadow}/${rel}` : rel;
          },
        });
      }
      if (this._client) this._client._uploadDest = destPath || '';
      return this._client;
    },

    _cancelledError() {
      const e = new Error('Transfer cancelled');
      e.name = 'AbortError';
      e.code = 'cancelled';
      return e;
    },

    // Run SDK operations one at a time — the WASM engine drives a single
    // active transfer per client instance. Each transfer gets an id; a
    // queued transfer whose id was cancelled is skipped (throws) instead of
    // running.
    _enqueue(id, fn) {
      const run = this._chain.then(async () => {
        if (this._cancelledIds.has(id)) {
          this._cancelledIds.delete(id);
          throw this._cancelledError();
        }
        this._activeId = id;
        try {
          return await fn();
        } finally {
          if (this._activeId === id) this._activeId = null;
        }
      });
      this._chain = run.catch(() => {});
      return run;
    },

    // Upload `items` (Array<{ file, relPath }>) into `destPath` (display
    // path). `dirShadow` is the directory shadow from the token response —
    // every plan path is `{dirShadow}/{rel}` (see class comment).
    // `onId(id)` is invoked synchronously with this transfer's ENGINE id so
    // the UI can cancel exactly this transfer (the engine's id numbering is
    // independent of the caller's list ids).
    upload(destPath, token, dirShadow, items, onEvent, onId) {
      const id = this._nextId++;
      if (typeof onId === 'function') onId(id);
      return this._enqueue(id, async () => {
        const client = this._getClient(destPath);
        if (!client) throw new Error('libfw-client SDK not loaded');
        this._uploadDirShadow = dirShadow || this._uploadDirShadow || '';
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
    downloadFolder(token, dirPath, onEvent, onId) {
      const id = this._nextId++;
      if (typeof onId === 'function') onId(id);
      return this._enqueue(id, async () => {
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
    downloadFile(token, path, name, onEvent, onId) {
      const id = this._nextId++;
      if (typeof onId === 'function') onId(id);
      return this._enqueue(id, async () => {
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

    // Cancel a transfer by ENGINE id (the UI learns it via the ops' `onId`
    // callback). With no id, cancels the active transfer only. A queued
    // transfer is flagged and skipped when its turn comes; the active one is
    // aborted in the engine. Other queued transfers are untouched.
    cancel(id) {
      if (id == null || id === this._activeId) {
        this._cancelledIds.delete(id);
        if (this._client) { try { this._client.cancel(); } catch (e) { /* noop */ } }
      } else {
        this._cancelledIds.add(id);
      }
    },
    pause() { if (this._client) { try { this._client.pause(); } catch (e) { /* noop */ } } },
    resume() { if (this._client) { try { this._client.resume(); } catch (e) { /* noop */ } } },

    // Batch download mode: keep a single directory handle for the whole batch,
    // reusing it through the SDK's public `directoryHandle` option. Pass
    // `false` when the batch ends.
    setBatchReuse(on) {
      this._batchReuse = !!on;
      if (!this._batchReuse) this._dirHandle = null;
      if (on) this._getClient('');
    },
  };

  window.Libfw = Libfw;
})();
