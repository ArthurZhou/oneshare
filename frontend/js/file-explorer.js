// File Explorer state
//
// Paths in this file are DISPLAY paths: for non-admin users they are VIRTUAL
// (the web root is a Samba-style share root listing ACL-configured dirs, and
// real filesystem paths never reach the browser); for admins they are the real
// paths of the actual tree. In both cases `entry.path` is used uniformly for
// navigation AND file operations — the backend resolves it internally.
let currentPath = '';        // current directory path ('' = root)
let currentWritable = false; // whether the current directory allows writes
let selectedFile = null;

// ── Address-bar (hash) routing ──
// `#/public2/sub` drives navigation; the fragment is independent of the
// configured URL prefix, so back/forward and deep links work anywhere.
let suppressHash = false;

function getPathFromHash() {
  const h = window.location.hash || '';
  if (!h.startsWith('#/')) return '';
  const raw = h.slice(2);
  if (!raw) return '';
  try {
    return decodeURIComponent(raw).replace(/^\/+/, '');
  } catch {
    return '';
  }
}

function setHashForPath(path) {
  const target = path ? '#/' + encodeURIComponent(path) : '#/';
  if (window.location.hash !== target) {
    suppressHash = true;
    window.location.hash = target;
  }
}

// User-driven navigation: update the address bar and load.
function navigate(path) {
  setHashForPath(path);
  loadFiles(path);
}

window.addEventListener('hashchange', () => {
  if (suppressHash) {
    suppressHash = false;
    return;
  }
  loadFiles(getPathFromHash());
});

async function loadFiles(path) {
  currentPath = path || '';
  const el = document.getElementById('file-list');
  try {
    const data = await API.listFiles(currentPath);
    currentWritable = !!data.writable;
    renderFiles(data);
    updatePathNav(data);
    updateToolbar();
  } catch (e) {
    el.innerHTML = `<div class="empty">Error: ${escapeHtml(e.message)}</div>`;
    // Don't keep stale state (e.g. after a 404 on an unknown hash) that could
    // wrongly enable upload/mkdir buttons.
    currentWritable = false;
    updateToolbar();
  }
}

// Uploads/mkdir only make sense inside a writable real directory (the share
// root and read-only shares have no write target).
function updateToolbar() {
  // At the real root (admin/root ACL) currentWritable is true so uploads are
  // allowed; at the virtual share root it is false so they are disabled.
  const canWrite = currentWritable;
  ['btn-upload', 'btn-upload-folder', 'btn-mkdir'].forEach(id => {
    const btn = document.getElementById(id);
    if (btn) btn.disabled = !canWrite;
  });
}

function updatePathNav(data) {
  const nav = document.getElementById('path-nav');
  const parts = data.current_path.split('/').filter(Boolean);
  let html = '<a href="#" data-path="">' + iconSvg('home') + ' Home</a>';
  if (parts.length > 0) html += '<span class="sep">/</span>';
  let cumulative = '';
  parts.forEach((p, i) => {
    cumulative += (i > 0 ? '/' : '') + p;
    if (i === parts.length - 1) {
      html += `<span class="cur">${escapeHtml(p)}</span>`;
    } else {
      html += `<a href="#" data-path="${escapeHtml(cumulative)}">${escapeHtml(p)}</a><span class="sep">/</span>`;
    }
  });
  nav.innerHTML = html;

  // Wire up nav links
  nav.querySelectorAll('a').forEach(a => {
    a.addEventListener('click', (e) => {
      e.preventDefault();
      navigate(a.dataset.path);
    });
  });
}

function renderFiles(data) {
  const el = document.getElementById('file-list');
  if (data.entries.length === 0) {
    el.innerHTML = data.is_share_root
      ? '<div class="empty">No shared folders — ask an admin to grant you access</div>'
      : '<div class="empty">This folder is empty</div>';
    return;
  }

  let html = `
    <div class="file-row header">
      <div class="file-icon"></div>
      <div class="file-name">Name</div>
      <div class="file-size">Size</div>
      <div class="file-mtime">Modified</div>
      <div class="file-actions"></div>
    </div>`;

  data.entries.forEach(entry => {
    const icon = entry.is_dir ? iconSvg('folder') : fileIcon(entry.name);
    const sizeStr = entry.is_dir ? '' : formatSize(entry.size);
    const cls = entry.is_dir ? 'file-row dir' : 'file-row';
    html += `
      <div class="${cls}" data-path="${escapeHtml(entry.path)}" data-name="${escapeHtml(entry.name)}" data-is-dir="${entry.is_dir}">
        <div class="file-icon">${icon}</div>
        <div class="file-name" title="${escapeHtml(entry.name)}">${escapeHtml(entry.name)}</div>
        <div class="file-size">${sizeStr}</div>
        <div class="file-mtime">${escapeHtml(entry.modified)}</div>
        <div class="file-actions">
          <button class="btn btn-sm btn-icon" data-action="download" title="${entry.is_dir ? 'Download folder' : 'Download'}">${iconSvg('download')}</button>
        </div>
      </div>`;
  });

  el.innerHTML = html;

  // Wire up click events
  el.querySelectorAll('.file-row.dir').forEach(row => {
    row.addEventListener('click', () => navigate(row.dataset.path));
  });

  // Wire up download buttons (files download directly, folders download as a tree/ZIP)
  el.querySelectorAll('[data-action="download"]').forEach(btn => {
    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      const row = btn.closest('.file-row');
      if (row.dataset.isDir === 'true') {
        downloadFolder(row.dataset.path, row.dataset.name);
      } else {
        downloadFile(row.dataset.path, row.dataset.name);
      }
    });
  });

  // Wire up context menu: right-click on desktop, long-press on touch.
  el.querySelectorAll('.file-row').forEach(row => {
    if (row.classList.contains('header')) return;
    row.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      openContextMenuAt(e.clientX, e.clientY, row);
    });
    attachLongPress(row, (x, y) => openContextMenuAt(x, y, row));
  });
}

function openContextMenuAt(x, y, row) {
  selectedFile = { path: row.dataset.path, name: row.dataset.name, isDir: row.dataset.isDir === 'true' };
  showContextMenu(x, y, selectedFile);
}

function showContextMenu(x, y, file) {
  const menu = document.getElementById('ctx-menu');
  menu.style.display = 'block';
  // Keep the menu inside the viewport — important on touch, where the finger
  // is usually near the bottom/edge of the screen.
  const r = menu.getBoundingClientRect();
  const px = Math.min(x, Math.max(0, window.innerWidth - r.width - 8));
  const py = Math.min(y, Math.max(0, window.innerHeight - r.height - 8));
  menu.style.left = px + 'px';
  menu.style.top = py + 'px';

  // Download is available for both files and folders (folders download as a tree/ZIP)
  menu.querySelectorAll('.ctx-item').forEach(item => {
    item.onclick = () => {
      hideContextMenu();
      handleContextAction(item.dataset.action, file);
    };
  });
}

function hideContextMenu() {
  document.getElementById('ctx-menu').style.display = 'none';
}

// ── Touch long-press → context menu ──
// Touchscreens have no right-click, so holding a file/folder row for
// ~LONG_PRESS_MS (without scrolling) opens the context menu, exactly like a
// right click on desktop. The synthetic `click` that a long-press produces is
// suppressed so it can't navigate the row (dir rows) or immediately dismiss
// the menu.
const LONG_PRESS_MS = 500;
const LONG_PRESS_MOVE_TOLERANCE = 10; // px of finger travel that cancels
// Clicks on a ROW are swallowed for this long after a long-press, so the
// browser's synthetic click (fired right after the finger lifts) can't
// navigate the row or dismiss the just-opened menu. Clicks on the context
// menu itself are never affected, so a tap on a menu item always works even
// if the browser happened not to emit the synthetic row click.
let suppressRowClicksUntil = 0;

function attachLongPress(el, onLongPress) {
  let timer = null;
  let startX = 0;
  let startY = 0;

  const clear = () => {
    if (timer) { clearTimeout(timer); timer = null; }
  };

  el.addEventListener('touchstart', (e) => {
    // Don't hijack taps on the row's action buttons (they have their own
    // click handlers and are meant to be tapped, not long-pressed).
    if (e.target.closest('[data-action]')) return;
    const t = e.touches[0];
    if (!t) return;
    startX = t.clientX;
    startY = t.clientY;
    clear();
    timer = setTimeout(() => {
      timer = null;
      suppressRowClicksUntil = Date.now() + 800;
      if (navigator.vibrate) { try { navigator.vibrate(15); } catch (err) { /* noop */ } }
      onLongPress(startX, startY);
    }, LONG_PRESS_MS);
  }, { passive: true });

  // Scrolling / dragging cancels the long-press.
  el.addEventListener('touchmove', (e) => {
    const t = e.touches[0];
    if (!t) return;
    if (Math.abs(t.clientX - startX) > LONG_PRESS_MOVE_TOLERANCE ||
        Math.abs(t.clientY - startY) > LONG_PRESS_MOVE_TOLERANCE) {
      clear();
    }
  }, { passive: true });

  el.addEventListener('touchend', clear);
  el.addEventListener('touchcancel', clear);
}

// Capture-phase: right after a long-press, suppress the synthetic click when
// it lands on a row, so it doesn't navigate the row (`.file-row.dir` click
// handler) or close the menu (app.js's global click-to-dismiss runs in the
// bubble phase). Clicks elsewhere (context-menu items, toolbar, empty space)
// pass through untouched.
document.addEventListener('click', (e) => {
  if (Date.now() < suppressRowClicksUntil && e.target.closest('.file-row')) {
    suppressRowClicksUntil = 0;
    e.preventDefault();
    e.stopPropagation();
  }
}, true);

async function handleContextAction(action, file) {
  switch (action) {
    case 'download':
      if (file.isDir) await downloadFolder(file.path, file.name);
      else await downloadFile(file.path, file.name);
      break;
    case 'rename':
      showRenameModal(file);
      break;
    case 'move':
      showMoveModal(file);
      break;
    case 'delete':
      showDeleteConfirm(file);
      break;
  }
}

// ── Transfers (upload/download tasks with progress & resume) ──

let transfers = [];
let nextTransferId = 1;

function addTransfer(transfer) {
  transfer.id = nextTransferId++;
  transfers.push(transfer);
  renderTransfers();
}

function updateTransfer(id, patch) {
  const t = transfers.find(x => x.id === id);
  if (!t) return;
  Object.assign(t, patch);
  renderTransfers();
}

function removeTransfer(id) {
  transfers = transfers.filter(x => x.id !== id);
  renderTransfers();
}

function transferStatusLabel(t) {
  switch (t.status) {
    case 'active': return '...';
    case 'done': return `${iconSvg('check')} done`;
    case 'error': return `${iconSvg('alert-circle')} failed`;
    case 'cancelled': return `${iconSvg('x')} cancelled`;
    default: return '';
  }
}

function transferPct(t) {
  if (t.total > 0) {
    const pct = Math.min(100, Math.round(((t.done || 0) / t.total) * 100));
    // The libfw engine reports upload progress as bytes DISPATCHED into the
    // WebSocket (not bytes the server has confirmed), so it can hit 100%
    // before the transfer is actually complete. Never claim 100% while a
    // transfer is still active — only the post-completion path (status
    // 'done') shows 100%.
    return t.status === 'active' ? Math.min(99, pct) : pct;
  }
  return 0;
}

function renderTransfers() {
  const panel = document.getElementById('transfers-panel');
  const list = document.getElementById('transfers-list');
  if (!panel || !list) return;
  if (transfers.length === 0) {
    panel.style.display = 'none';
    return;
  }
  panel.style.display = 'block';

  list.innerHTML = transfers.map(t => {
    const done = t.status === 'done';
    const finished = done || t.status === 'error' || t.status === 'cancelled';
    const action = finished
      ? `<button class="btn btn-sm" data-xfer-remove="${t.id}" title="Remove">${iconSvg('x')}</button>`
      : `<button class="btn btn-sm" data-xfer-cancel="${t.id}" title="Cancel">${iconSvg('x')}</button>`;
    const arrow = iconSvg(t.kind === 'upload' ? 'upload' : 'download');
    const sub = t.kind === 'upload'
      ? `Upload · ${formatSize(t.total)}`
      : 'Download';
    return `
      <div class="transfer-row">
        <div class="transfer-top">
          <span class="transfer-name" title="${escapeHtml(t.name)}">${arrow} ${escapeHtml(t.name)}</span>
          <span class="transfer-right">
            <span class="transfer-pct">${done ? '100%' : transferPct(t) + '%'} ${transferStatusLabel(t)}</span>
            ${action}
          </span>
        </div>
        <div class="progress-bar"><div class="progress-fill" style="width:${done ? 100 : transferPct(t)}%"></div></div>
        <div class="transfer-sub">${sub}${t.error ? ` · <span class="err">${escapeHtml(t.error)}</span>` : ''}</div>
      </div>`;
  }).join('');

  list.querySelectorAll('[data-xfer-remove]').forEach(btn => {
    btn.addEventListener('click', () => removeTransfer(parseInt(btn.dataset.xferRemove)));
  });
  list.querySelectorAll('[data-xfer-cancel]').forEach(btn => {
    btn.addEventListener('click', () => {
      const t = transfers.find(x => x.id === parseInt(btn.dataset.xferCancel));
      if (t && typeof t.cancel === 'function') t.cancel();
    });
  });
}

// ── Upload (via libfw SDK) ──
//
// Uploads go through the libfw-client SDK, which handles chunking, zstd
// compression and x-libfw-offset resume itself. The SDK builds `/file/{path}`
// from each plan entry, so the wrapper prefixes every relative path with the
// current (display) directory and the server's token binds the real target.
// Resume state is persisted per path in IndexedDB by the SDK, so a re-run
// continues from where an interrupted upload stopped.
// Refresh the listing shortly after an upload finishes, debounced so a batch
// of concurrent uploads only triggers one reload.
let uploadRefreshTimer = null;
function scheduleUploadRefresh() {
  clearTimeout(uploadRefreshTimer);
  uploadRefreshTimer = setTimeout(() => loadFiles(currentPath), 500);
}

async function handleUpload(input) {
  // Accept both raw File objects (legacy callers, e.g. a stale cached app.js)
  // and { file, relPath } items, then drop anything without a File.
  const items = Array.from(input || [])
    .map((it) =>
      it instanceof File
        ? { file: it, relPath: it.webkitRelativePath || it.name }
        : it
    )
    .filter((it) => it && it.file);
  if (!items.length) return;

  const destPath = currentPath || '';
  if (!currentWritable) {
    alert('Upload is not allowed in this folder');
    return;
  }

  // One write token for the destination; the server binds it to the real
  // directory path, and the SDK uploads real child paths underneath it.
  // libfw 0.2.0 drives transfers over WebSocket, where the SDK sends the path
  // it is given — so we hand it the REAL destination path from the token.
  let token, realDest;
  try {
    const resp = await API.getToken(destPath || '/', 'write');
    token = resp.token;
    realDest = resp.real_path;
  } catch (e) {
    alert('Upload denied: ' + e.message);
    return;
  }

  const total = items.reduce((s, it) => s + (it.file.size || 0), 0);
  const name = items.length === 1 ? items[0].relPath : `${items.length} files`;
  const t = {
    kind: 'upload',
    name,
    total,
    done: 0,
    status: 'active',
    error: null,
  };
  addTransfer(t);
  t.cancel = () => Libfw.cancel();
  t.run = () => runUploadTask(t, realDest, token, items);
  t.run();
}

async function runUploadTask(t, destPath, token, items) {
  t.status = 'active';
  t.error = null;
  renderTransfers();
  try {
    await Libfw.upload(destPath, token, items, (ev) => {
      if (ev.type === 'progress') updateTransfer(t.id, { done: ev.done, total: ev.total });
    });
    updateTransfer(t.id, { status: 'done', done: t.total });
    scheduleUploadRefresh();
  } catch (e) {
    const cancelled = e && (e.code === 'cancelled' || e.code === 'abort' || e.name === 'AbortError');
    updateTransfer(t.id, {
      status: cancelled ? 'cancelled' : 'error',
      error: cancelled ? '' : (e && e.message) || String(e),
    });
  }
}

// ── Downloads (via libfw SDK) ──
//
// All downloads go through the libfw-client SDK (`downloadFile` /
// `downloadFolder`). Since 0.1.3 the SDK handles the save path itself: with
// the File System Access API it streams into a user-picked directory;
// without it (`downloadMode: 'auto'`) single files are saved via a normal
// browser download and folders are packed into a `.zip` and downloaded — no
// feature detection needed here.

function downloadFile(path, name) {
  (async () => {
    const tokenResp = await API.getToken(path, 'read');
    const t = { kind: 'download', name, total: 0, done: 0, status: 'active', error: null };
    addTransfer(t);
    t.cancel = () => Libfw.cancel();
    t.run = () => runFileDownloadTask(t, path, name, tokenResp.token, tokenResp.real_path);
    t.run();
  })().catch(e => alert('Download denied: ' + e.message));
}

async function runFileDownloadTask(t, path, name, token, realPath) {
  t.status = 'active';
  t.error = null;
  renderTransfers();
  try {
    const progress = (ev) => {
      if (ev.type === 'progress') updateTransfer(t.id, { done: ev.done, total: ev.total });
    };
    // libfw-client 0.2.0 saves the file itself (streamed into a user-picked
    // directory via FS API, or via a traditional browser download). The SDK
    // sends the path we give it over WebSocket, so pass the REAL path.
    const done = await Libfw.downloadFile(token, realPath, progress);
    updateTransfer(t.id, { status: 'done', done });
    setTimeout(() => removeTransfer(t.id), 3000);
  } catch (e) {
    const cancelled = e && (e.code === 'cancelled' || e.code === 'abort' || e.name === 'AbortError');
    updateTransfer(t.id, {
      status: cancelled ? 'cancelled' : 'error',
      error: cancelled ? '' : (e && e.message) || String(e),
    });
  }
}

function downloadFolder(path, name) {
  (async () => {
    const tokenResp = await API.getToken(path, 'read');
    const t = { kind: 'download', name, total: 0, done: 0, status: 'active', error: null };
    addTransfer(t);
    t.cancel = () => Libfw.cancel();
    t.run = () => runFolderDownloadTask(t, path, name, tokenResp.token, tokenResp.real_path);
    t.run();
  })().catch(e => alert('Download denied: ' + e.message));
}

async function runFolderDownloadTask(t, path, name, token, realPath) {
  t.status = 'active';
  t.error = null;
  renderTransfers();
  try {
    // libfw-client 0.2.0 downloads the whole tree itself over WebSocket
    // (streamed into a user-picked directory via FS API, or packed into a
    // `.zip` and saved via a normal browser download). It sends the path we
    // give it, so pass the REAL root path of the folder.
    const bytes = await Libfw.downloadFolder(token, realPath, (ev) => {
      if (ev.type === 'progress') updateTransfer(t.id, { done: ev.done, total: ev.total });
    });
    updateTransfer(t.id, { status: 'done', done: bytes });
    setTimeout(() => removeTransfer(t.id), 3000);
  } catch (e) {
    const cancelled = e && (e.code === 'cancelled' || e.code === 'abort' || e.name === 'AbortError');
    updateTransfer(t.id, {
      status: cancelled ? 'cancelled' : 'error',
      error: cancelled ? '' : (e && e.message) || String(e),
    });
  }
}

function showRenameModal(file) {
  showModal('Rename', `
    <label>New name for "${escapeHtml(file.name)}":</label>
    <input type="text" id="rename-input" value="${escapeHtml(file.name)}">
  `, async () => {
    const newName = document.getElementById('rename-input').value.trim();
    if (!newName || newName === file.name) return;
    try {
      await API.renameFile(file.path, newName);
      loadFiles(currentPath);
      hideModal();
    } catch (e) {
      alert('Rename failed: ' + e.message);
    }
  });
}

function showMoveModal(file) {
  // The destination is entered in the current path space (virtual for
  // non-admins, real for admins); the server resolves it to a real path.
  const placeholder = currentPath ? currentPath + '/' : '/';
  showModal('Move', `
    <label>Move "${escapeHtml(file.name)}" to:</label>
    <input type="text" id="move-input" placeholder="/path/to/destination" value="${escapeHtml(placeholder)}">
  `, async () => {
    const destDir = document.getElementById('move-input').value.trim().replace(/\/$/, '');
    if (!destDir) return;
    const destPath = destDir + '/' + file.name;
    try {
      await API.moveFile(file.path, destPath);
      loadFiles(currentPath);
      hideModal();
    } catch (e) {
      alert('Move failed: ' + e.message);
    }
  });
}

function showDeleteConfirm(file) {
  showModal('Delete', `
    <p>Are you sure you want to delete "${escapeHtml(file.name)}"?</p>
    ${file.isDir ? '<p style="color:var(--error);font-size:0.85em">This will delete the entire folder and all its contents!</p>' : ''}
  `, async () => {
    try {
      await API.deleteFile(file.path);
      loadFiles(currentPath);
      hideModal();
    } catch (e) {
      alert('Delete failed: ' + e.message);
    }
  });
}

// ── New folder ──

function showMkdirModal() {
  showModal('New Folder', `
    <label>Folder name:</label>
    <input type="text" id="mkdir-input" placeholder="New folder">
  `, async () => {
    const name = document.getElementById('mkdir-input').value.trim();
    if (!name) return;
    try {
      await API.mkdir(currentPath, name);
      loadFiles(currentPath);
      hideModal();
    } catch (e) {
      alert('Create folder failed: ' + e.message);
    }
  });
}

// ── Drag and drop ──

function setupDragDrop() {
  const overlay = document.getElementById('drag-overlay');

  document.addEventListener('dragenter', (e) => {
    e.preventDefault();
    if (e.dataTransfer.types.includes('Files')) {
      overlay.style.display = 'flex';
    }
  });

  document.addEventListener('dragover', (e) => {
    e.preventDefault();
  });

  document.addEventListener('dragleave', (e) => {
    if (e.target === document.documentElement || e.target === overlay) {
      overlay.style.display = 'none';
    }
  });

  document.addEventListener('drop', (e) => {
    e.preventDefault();
    overlay.style.display = 'none';
    const dt = e.dataTransfer;
    if (dt && (dt.files.length || (dt.items && dt.items.length))) {
      collectDropItems(dt).then(handleUpload);
    }
  });
}

// Flatten a DataTransfer into upload items, descending into dropped folders
// (via webkitGetAsEntry) so the directory structure is preserved.
async function collectDropItems(dt) {
  const items = [];
  if (dt.items && dt.items.length) {
    const entries = [];
    for (const item of dt.items) {
      if (item.kind !== 'file') continue;
      // Prefer the entry API (preserves folder structure); if it is unavailable
      // or returns null, fall back to getAsFile() so the file is never dropped.
      const entry = item.webkitGetAsEntry ? item.webkitGetAsEntry() : null;
      if (entry) {
        entries.push(entry);
      } else {
        const f = item.getAsFile();
        if (f) items.push({ file: f, relPath: f.name });
      }
    }
    for (const entry of entries) {
      await traverseEntry(entry, '', items);
    }
  } else {
    for (const f of dt.files) items.push({ file: f, relPath: f.name });
  }
  return items;
}

async function traverseEntry(entry, base, out) {
  if (!entry) return;
  if (entry.isFile) {
    await new Promise((resolve, reject) => {
      entry.file((file) => {
        out.push({ file, relPath: base ? base + '/' + file.name : file.name });
        resolve();
      }, reject);
    });
  } else if (entry.isDirectory) {
    const reader = entry.createReader();
    const children = await new Promise((resolve, reject) => {
      const all = [];
      const readBatch = () => {
        reader.readEntries((batch) => {
          if (!batch.length) resolve(all);
          else { all.push(...batch); readBatch(); }
        }, reject);
      };
      readBatch();
    });
    const childBase = base ? base + '/' + entry.name : entry.name;
    for (const child of children) {
      await traverseEntry(child, childBase, out);
    }
  }
}

// ── Modal helpers ──

function showModal(title, bodyHtml, onOk) {
  document.getElementById('modal-title').textContent = title;
  document.getElementById('modal-body').innerHTML = bodyHtml;
  document.getElementById('modal-overlay').style.display = 'flex';

  const okBtn = document.getElementById('modal-ok');
  const cancelBtn = document.getElementById('modal-cancel');
  const closeBtn = document.getElementById('modal-close');

  const cleanup = () => {
    okBtn.onclick = null;
    cancelBtn.onclick = null;
    closeBtn.onclick = null;
  };

  okBtn.onclick = () => {
    cleanup();
    if (onOk) onOk();
  };

  cancelBtn.onclick = () => {
    cleanup();
    hideModal();
  };

  closeBtn.onclick = () => {
    cleanup();
    hideModal();
  };

  // Focus input if present
  const input = document.getElementById('modal-body').querySelector('input');
  if (input) setTimeout(() => input.focus(), 100);
}

function hideModal() {
  document.getElementById('modal-overlay').style.display = 'none';
}

// ── Utilities ──

function formatSize(bytes) {
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return (bytes / Math.pow(1024, i)).toFixed(i > 0 ? 1 : 0) + ' ' + units[i];
}

function fileIcon(name) {
  const ext = name.split('.').pop().toLowerCase();
  const icons = {
    pdf: 'file-text', doc: 'file-text', docx: 'file-text',
    xls: 'file-text', xlsx: 'file-text', ppt: 'file-text', pptx: 'file-text',
    jpg: 'image', jpeg: 'image', png: 'image', gif: 'image', svg: 'image', webp: 'image',
    mp4: 'film', avi: 'film', mkv: 'film', mov: 'film',
    mp3: 'music', wav: 'music', flac: 'music', aac: 'music',
    zip: 'archive', rar: 'archive', tar: 'archive', gz: 'archive', '7z': 'archive',
    txt: 'file-text', md: 'file-text', json: 'file-text', xml: 'file-text',
    yaml: 'file-text', yml: 'file-text', toml: 'file-text',
    js: 'code', ts: 'code', jsx: 'code', tsx: 'code', py: 'code', rs: 'code',
    go: 'code', java: 'code', c: 'code', cpp: 'code',
    html: 'globe', css: 'pen-tool',
  };
  return iconSvg(icons[ext] || 'file');
}

function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

// Downloads go through the libfw SDK (`downloadFile`/`downloadFolder` in
// libfw.js); since libfw-client 0.1.3 the SDK handles both save paths itself
// (File System Access API when available, else a native browser download with
// folders packed into a `.zip`).
