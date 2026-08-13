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

  // Wire up context menu: right-click on desktop; on touch, a long-press
  // either lifts the row to drag it or opens the menu (attachRowTouch).
  el.querySelectorAll('.file-row').forEach(row => {
    if (row.classList.contains('header')) return;
    row.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      openContextMenuAt(e.clientX, e.clientY, row);
    });
    attachRowTouch(row);
  });

  // Wire up drag-to-move: rows are only armed for dragging after a short
  // hold (armDragHold), so quick clicks and scroll-drags still work. Drop
  // targets are handled on the container + breadcrumb in setupMoveDragDrop.
  el.querySelectorAll('.file-row').forEach(row => {
    if (row.classList.contains('header')) return;
    row.addEventListener('pointerdown', (e) => {
      if (e.pointerType !== 'mouse') return;         // touch/pen use the long-press menu
      if (e.button !== 0) return;                    // left button only
      if (e.target.closest('[data-action]')) return; // not from action buttons
      armDragHold(row, e);
    });
    row.addEventListener('dragstart', (e) => {
      holdRow = null; // drag started; the mouseup cleanup must not un-arm us
      dragSource = { path: row.dataset.path, name: row.dataset.name, isDir: row.dataset.isDir === 'true' };
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = 'move';
        try {
          e.dataTransfer.setData('application/x-oneshare-move', row.dataset.path);
          e.dataTransfer.setData('text/plain', row.dataset.name);
        } catch (err) { /* some browsers restrict setData during dragstart */ }
      }
      row.classList.add('dragging');
    });
    row.addEventListener('dragend', () => {
      dragSource = null;
      row.classList.remove('dragging');
      row.removeAttribute('draggable');
      cancelDragHold();
      clearDropTargets();
      clearAutoEnter();
    });
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

// ── Touch: long-press lifts a row to drag, or opens the context menu ──
// Touchscreens have no right-click, so holding a row for ~TOUCH_LIFT_MS
// "lifts" it: if the finger then moves it becomes a drag-and-drop move (drop
// onto a folder row, a breadcrumb link, or the blank area of the current
// dir); if it lifts without moving, the context menu opens (old long-press
// behavior). Moving before the lift is treated as a scroll and cancels it.
const TOUCH_LIFT_MS = 400;
const TOUCH_LIFT_MOVE_TOLERANCE = 10; // px of travel before the lift that cancels
const TOUCH_DRAG_START_TOLERANCE = 8; // px of travel after the lift that starts a drag
// Clicks on a ROW are swallowed for this long after a touch gesture, so the
// browser's synthetic click (fired right after the finger lifts) can't
// navigate the row or dismiss the just-opened menu. Clicks on the context
// menu itself are never affected, so a tap on a menu item always works even
// if the browser happened not to emit the synthetic row click.
let suppressRowClicksUntil = 0;

function attachRowTouch(row) {
  let timer = null;
  let startX = 0, startY = 0;
  let touchId = null;
  let lifted = false;   // long-press fired → the row is picked up
  let dragging = false; // finger moved after lift → it's a drag
  let lastX = 0, lastY = 0;
  let touchTarget = null;

  const clearTimer = () => { if (timer) { clearTimeout(timer); timer = null; } };
  const cleanup = () => {
    clearTimer();
    lifted = false;
    dragging = false;
    touchId = null;
    touchTarget = null;
    row.classList.remove('dragging');
  };

  row.addEventListener('touchstart', (e) => {
    // Don't hijack taps on the row's action buttons (they have their own
    // click handlers and are meant to be tapped, not long-pressed).
    if (e.target.closest('[data-action]')) return;
    const t = e.touches[0];
    if (!t) return;
    cleanup();
    touchId = t.identifier;
    startX = lastX = t.clientX;
    startY = lastY = t.clientY;
    timer = setTimeout(() => {
      timer = null;
      lifted = true;
      dragSource = { path: row.dataset.path, name: row.dataset.name, isDir: row.dataset.isDir === 'true' };
      row.classList.add('dragging');
    }, TOUCH_LIFT_MS);
  }, { passive: true });

  row.addEventListener('touchmove', (e) => {
    let t = null;
    for (const ct of e.touches) if (ct.identifier === touchId) t = ct;
    if (!t) return;
    lastX = t.clientX; lastY = t.clientY;
    if (!lifted) {
      // Not lifted yet: real movement means a scroll — cancel the lift.
      if (Math.abs(t.clientX - startX) > TOUCH_LIFT_MOVE_TOLERANCE ||
          Math.abs(t.clientY - startY) > TOUCH_LIFT_MOVE_TOLERANCE) {
        clearTimer();
      }
      return;
    }
    if (!dragging) {
      if (Math.abs(t.clientX - startX) > TOUCH_DRAG_START_TOLERANCE ||
          Math.abs(t.clientY - startY) > TOUCH_DRAG_START_TOLERANCE) {
        dragging = true;
      } else {
        return; // lifted but not yet moved far enough
      }
    }
    // Track the drop target under the finger and auto-enter folders on hold.
    touchTarget = dropTargetFromPoint(lastX, lastY);
    applyTargetHighlight(touchTarget);
    armAutoEnter(touchTarget && touchTarget.kind === 'dir' ? touchTarget.path : null, touchTarget ? touchTarget.el : null);
  }, { passive: true });

  row.addEventListener('touchend', (e) => {
    clearTimer();
    if (!lifted) return;
    const wasDragging = dragging;
    const target = touchTarget;
    cleanup();
    if (wasDragging) {
      clearAutoEnter();
      if (target && target.kind !== 'none') {
        performMove(target.path);
      } else {
        dragSource = null; // dropped nowhere
      }
      suppressRowClicksUntil = Date.now() + 800;
    } else {
      // Lifted but not dragged → open the context menu.
      if (navigator.vibrate) { try { navigator.vibrate(15); } catch (err) { /* noop */ } }
      openContextMenuAt(startX, startY, row);
      dragSource = null;
      suppressRowClicksUntil = Date.now() + 800;
    }
  }, { passive: true });

  row.addEventListener('touchcancel', () => {
    cleanup();
    dragSource = null;
    clearDropTargets();
    clearAutoEnter();
  });
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
  // All data has been flushed to the server but it hasn't confirmed COMPLETE
  // yet (a long phase on low-bandwidth / high-latency links). Show a clear
  // "finalizing" state so it never reads as a stuck 99%.
  if (t.finalizing) return `${iconSvg('refresh-cw', 'spin')} finalizing`;
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
    // "finalizing": all bytes flushed to the server but COMPLETE not yet
    // confirmed — the row keeps an animated bar instead of freezing at 99%.
    const finalizing = t.status === 'active' && !!t.finalizing;
    const finished = done || t.status === 'error' || t.status === 'cancelled';
    const action = finished
      ? `<button class="btn btn-sm" data-xfer-remove="${t.id}" title="Remove">${iconSvg('x')}</button>`
      : `<button class="btn btn-sm" data-xfer-cancel="${t.id}" title="Cancel">${iconSvg('x')}</button>`;
    const arrow = iconSvg(t.kind === 'upload' ? 'upload' : 'download');
    const sub = t.kind === 'upload'
      ? `Upload · ${formatSize(t.total)}`
      : 'Download';
    const fillW = done ? 100 : transferPct(t);
    return `
      <div class="transfer-row">
        <div class="transfer-top">
          <span class="transfer-name" title="${escapeHtml(t.name)}">${arrow} ${escapeHtml(t.name)}</span>
          <span class="transfer-right">
            <span class="transfer-pct">${done ? '100%' : transferPct(t) + '%'} ${transferStatusLabel(t)}</span>
            ${action}
          </span>
        </div>
        <div class="progress-bar"><div class="progress-fill${finalizing ? ' finalizing' : ''}" style="width:${fillW}%"></div></div>
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
  t.finalizing = false;
  renderTransfers();
  try {
    await Libfw.upload(destPath, token, items, (ev) => {
      if (ev.type === 'progress') updateTransfer(t.id, {
        done: ev.done,
        total: ev.total,
        // Engine reports flushed bytes; done reaching total means everything
        // is on the wire but the server hasn't confirmed — enter finalizing.
        finalizing: !!(ev.total > 0 && ev.done >= ev.total),
      });
    });
    updateTransfer(t.id, { status: 'done', done: t.total, finalizing: false });
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
  t.finalizing = false;
  renderTransfers();
  try {
    const progress = (ev) => {
      if (ev.type === 'progress') updateTransfer(t.id, {
        done: ev.done,
        total: ev.total,
        finalizing: !!(ev.total > 0 && ev.done >= ev.total),
      });
    };
    // libfw-client 0.2.0 saves the file itself (streamed into a user-picked
    // directory via FS API, or via a traditional browser download). The SDK
    // sends the path we give it over WebSocket, so pass the REAL path.
    const done = await Libfw.downloadFile(token, realPath, progress);
    updateTransfer(t.id, { status: 'done', done, finalizing: false });
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
  t.finalizing = false;
  renderTransfers();
  try {
    // libfw-client 0.2.0 downloads the whole tree itself over WebSocket
    // (streamed into a user-picked directory via FS API, or packed into a
    // `.zip` and saved via a normal browser download). It sends the path we
    // give it, so pass the REAL root path of the folder.
    const bytes = await Libfw.downloadFolder(token, realPath, (ev) => {
      if (ev.type === 'progress') updateTransfer(t.id, {
        done: ev.done,
        total: ev.total,
        finalizing: !!(ev.total > 0 && ev.done >= ev.total),
      });
    });
    updateTransfer(t.id, { status: 'done', done: bytes, finalizing: false });
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

// ── Drag & drop to MOVE files/folders ──
//
// Dragging an existing row lets you move it into another folder by dropping
// it onto a folder row (the highlighted drop target) or onto an ancestor in
// the breadcrumb path nav (never "Home"). This is separate from setupDragDrop,
// which handles dropping files from the OS/another app as an UPLOAD.
// `dragSource` (set on dragstart) identifies an internal move; external drags
// carry `Files` in dataTransfer.types and are left to the upload handler.
//
// UX rules:
// - A drag is only armed after HOLDING the row for DRAG_HOLD_MS (plain
//   press-drag does nothing), so clicks and scroll-drags stay unaffected.
// - A folder cannot be dropped onto itself or any of its own subfolders
//   (isValidDropTargetDir), and hovering a valid folder for AUTO_ENTER_MS
//   auto-navigates into it.

let dragSource = null; // { path, name, isDir } of the row currently being dragged

// ── Hold-to-drag ──
// Rows are NOT draggable by default. A mousedown starts a DRAG_HOLD_MS timer;
// if the pointer stays still until it fires, the row is armed (draggable=true)
// and the drag can begin. Moving before the timer or releasing cancels the
// arm, so quick clicks and scrolls work normally.
const DRAG_HOLD_MS = 350;
const DRAG_HOLD_TOLERANCE = 6; // px of pointer travel that cancels the arm
let holdTimer = null;
let holdRow = null;
let holdStartX = 0;
let holdStartY = 0;

function armDragHold(row, e) {
  cancelDragHold();
  holdRow = row;
  holdStartX = e.clientX;
  holdStartY = e.clientY;
  holdTimer = setTimeout(() => {
    holdTimer = null;
    if (holdRow && document.contains(holdRow)) {
      holdRow.setAttribute('draggable', 'true'); // armed: next move starts the drag
    }
  }, DRAG_HOLD_MS);
}

function cancelDragHold() {
  clearTimeout(holdTimer);
  holdTimer = null;
  if (holdRow) {
    holdRow.removeAttribute('draggable');
    holdRow = null;
  }
}

// True while the pointer is over a move drop target, used to skip clearing
// the highlight when dragleave fires between children inside the list.
let dropDepth = 0;

// Auto-enter: while dragging, hovering a valid folder for AUTO_ENTER_MS
// navigates into it (like Windows Explorer / macOS).
const AUTO_ENTER_MS = 800;
let hoverDirPath = null;
let autoEnterTimer = null;

function clearAutoEnter() {
  clearTimeout(autoEnterTimer);
  autoEnterTimer = null;
  hoverDirPath = null;
}

function isInternalMove(e) {
  if (!dragSource) return false;
  const dt = e.dataTransfer;
  if (dt && dt.types) {
    // If the OS/another app is dragging files, that's an upload, not a move.
    if (Array.from(dt.types).includes('Files')) return false;
  }
  return true;
}

// Whether `destDir` is a legal move destination for the current drag:
// - moving to the same location is a no-op → rejected;
// - a folder cannot be moved into itself or any of its own subfolders.
function isValidDropTargetDir(destDir) {
  if (!dragSource) return false;
  const destPath = (destDir ? destDir + '/' : '') + dragSource.name;
  if (destPath === dragSource.path) return false;
  if (dragSource.isDir) {
    if (dragSource.path === destDir) return false;
    if (destDir.startsWith(dragSource.path + '/')) return false;
  }
  return true;
}

function clearDropTargets() {
  dropDepth = 0;
  const list = document.getElementById('file-list');
  if (list) list.classList.remove('drop-current');
  document.querySelectorAll('.file-row.drop-target, #path-nav a.drop-target')
    .forEach(el => el.classList.remove('drop-target'));
}

// Resolve the drop target under a client point (shared by the mouse dragover
// path and the touch-drag path):
// - a folder row → move into that folder;
// - a breadcrumb ancestor link (never Home) → move into that folder;
// - otherwise the blank area of the current directory (move into currentPath).
function dropTargetFromPoint(x, y) {
  const el = document.elementFromPoint(x, y);
  if (!el || !el.closest) return { kind: 'none' };
  const dirRow = el.closest('.file-row.dir');
  if (dirRow && isValidDropTargetDir(dirRow.dataset.path)) {
    return { kind: 'dir', path: dirRow.dataset.path, el: dirRow };
  }
  const navLink = el.closest('#path-nav a[data-path]');
  if (navLink) {
    const dest = navLink.dataset.path || '';
    if (dest !== '' && isValidDropTargetDir(dest)) {
      return { kind: 'dir', path: dest, el: navLink };
    }
  }
  if (isValidDropTargetDir(currentPath)) {
    return { kind: 'current', path: currentPath, el: null };
  }
  return { kind: 'none' };
}

// Highlight the current drop target (folder row / breadcrumb link, or the
// blank area of the current directory).
function applyTargetHighlight(t) {
  clearDropTargets();
  if (!t) return;
  if (t.kind === 'dir' && t.el) {
    t.el.classList.add('drop-target');
  } else if (t.kind === 'current') {
    const list = document.getElementById('file-list');
    if (list) list.classList.add('drop-current');
  }
}

// Auto-enter: hovering a valid folder (a row or a breadcrumb link) for
// AUTO_ENTER_MS navigates into it. Passing null cancels any pending timer.
function armAutoEnter(path, el) {
  if (path === hoverDirPath) return;
  hoverDirPath = path;
  clearTimeout(autoEnterTimer);
  autoEnterTimer = null;
  if (!path) return;
  autoEnterTimer = setTimeout(() => {
    autoEnterTimer = null;
    if (dragSource && hoverDirPath === path && el && document.body.contains(el)) {
      clearAutoEnter();
      navigate(path);
    }
  }, AUTO_ENTER_MS);
}

function setupMoveDragDrop() {
  const el = document.getElementById('file-list');
  if (!el) return;

  // Hold-to-drag cancellation: moving during the hold, or releasing, cancels.
  document.addEventListener('mousemove', (e) => {
    if (holdRow && holdTimer) {
      if (Math.abs(e.clientX - holdStartX) > DRAG_HOLD_TOLERANCE ||
          Math.abs(e.clientY - holdStartY) > DRAG_HOLD_TOLERANCE) {
        cancelDragHold();
      }
    }
  });
  document.addEventListener('mouseup', () => cancelDragHold());

  // Shared dragover: resolve the target under the pointer, highlight it and
  // auto-enter folders when hovered. Used by the file list AND the breadcrumb.
  const onDragOver = (e) => {
    if (!isInternalMove(e)) return;
    const t = dropTargetFromPoint(e.clientX, e.clientY);
    armAutoEnter(t.kind === 'dir' ? t.path : null, t.el);
    if (t.kind === 'none') {
      clearDropTargets();
      return;
    }
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
    applyTargetHighlight(t);
  };

  el.addEventListener('dragenter', (e) => {
    if (!isInternalMove(e)) return;
    e.preventDefault();
    dropDepth++;
    applyTargetHighlight(dropTargetFromPoint(e.clientX, e.clientY));
  });

  el.addEventListener('dragover', onDragOver);

  el.addEventListener('dragleave', (e) => {
    if (!isInternalMove(e)) return;
    // Moving between children inside the list must not clear the highlight
    // (relatedTarget is still within the list).
    if (e.relatedTarget && el.contains(e.relatedTarget)) return;
    dropDepth = Math.max(0, dropDepth - 1);
    if (dropDepth === 0) {
      clearDropTargets();
      clearAutoEnter();
    }
  });

  el.addEventListener('drop', (e) => {
    dropDepth = 0;
    if (!isInternalMove(e)) return;
    const t = dropTargetFromPoint(e.clientX, e.clientY);
    clearDropTargets();
    clearAutoEnter();
    if (t.kind === 'none') return; // folder onto itself/its child, or nothing
    e.preventDefault();
    e.stopPropagation();
    performMove(t.path);
  });

  // Breadcrumb path nav: dragging onto an ancestor link moves into that
  // folder, and hovering one auto-enters it too. "Home" (data-path="") is
  // never a target — dropTargetFromPoint rejects it.
  const nav = document.getElementById('path-nav');
  if (nav) {
    nav.addEventListener('dragenter', (e) => {
      if (!isInternalMove(e)) return;
      e.preventDefault();
    });
    nav.addEventListener('dragover', onDragOver);
    nav.addEventListener('dragleave', (e) => {
      if (!isInternalMove(e)) return;
      if (e.relatedTarget && nav.contains(e.relatedTarget)) return;
      clearDropTargets();
      clearAutoEnter();
    });
    nav.addEventListener('drop', (e) => {
      if (!isInternalMove(e)) return;
      const t = dropTargetFromPoint(e.clientX, e.clientY);
      clearDropTargets();
      clearAutoEnter();
      if (t.kind === 'none') return;
      e.preventDefault();
      e.stopPropagation();
      performMove(t.path);
    });
  }
}

async function performMove(destDir) {
  const src = dragSource;
  dragSource = null;
  if (!src) return;
  const destPath = (destDir ? destDir + '/' : '') + src.name;
  // Dropped back where it already lives: nothing to do.
  if (destPath === src.path) return;
  try {
    await API.moveFile(src.path, destPath);
    loadFiles(currentPath);
  } catch (e) {
    alert('Move failed: ' + e.message);
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
