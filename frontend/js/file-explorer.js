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
let currentFilesData = null;
let currentSort = { field: 'name', dir: 'asc' };

// ── Multi-select（多选）──
// path → { path, name, isDir } of every currently selected entry. Selection
// is per-directory: navigating always clears it (the entries no longer exist
// in the new listing). Toggled via the row checkboxes, Ctrl/Cmd+click, or
// Shift+click for a range; the floating bar offers batch download/delete.
let selectedPaths = new Map();
let lastCheckIndex = -1; // last checkbox index, for Shift+click ranges

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

function setHashForFile(path) {
  const target = '#file/' + encodeURIComponent(path);
  if (window.location.hash !== target) {
    suppressHash = true;
    window.location.hash = target;
  }
}

// Route the current hash: `#file/<path>` opens the file PAGE (the same
// content the click used to show in a modal, now as a full page like a
// folder view); `#/<dir>` loads the directory listing.
function routeFromHash() {
  const h = window.location.hash || '';
  if (h.startsWith('#file/')) {
    try {
      openFileDetail({ path: decodeURIComponent(h.slice(6)) }, true);
      return;
    } catch (err) { /* malformed path → fall through to the listing */ }
  }
  loadFiles(getPathFromHash());
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
  routeFromHash();
});

async function loadFiles(path) {
  currentPath = path || '';
  // Navigation always clears the stale row selection: the highlighted file
  // no longer exists in this listing (and the selectedFile was only valid in
  // the previous directory). Context-menu selection (openContextMenuAt) does
  // not navigate, so it is unaffected.
  selectedFile = null;
  // Multi-select is per-directory too: a fresh listing starts unselected.
  selectedPaths.clear();
  lastCheckIndex = -1;
  const el = document.getElementById('file-list');
  try {
    const data = await API.listFiles(currentPath);
    currentWritable = !!data.writable;
    // Single-file share: the link IS the file — open its page directly so
    // the receiver lands on the preview/download view.
    if (SHARE && !currentPath && data.entries.length === 1 && !data.entries[0].is_dir) {
      openFileDetail({ path: data.entries[0].path }, true);
      return;
    }
    renderFiles(data);
    updatePathNav(data);
    updateToolbar();
    updateSelectionBar();
  } catch (e) {
    // The requested directory no longer exists (deleted, moved, or an
    // unknown/deep-link hash): fall back to the root instead of showing a
    // dead-end error, unless we are already at the root (avoid a loop).
    if (e && e.status === 404 && currentPath) {
      navigate('');
      return;
    }
    el.innerHTML = `<div class="empty">错误: ${escapeHtml(e.message)}</div>`;
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
  let html = '<a href="#" data-path="">' + iconSvg('home') + ' 主页</a>';
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

function getSortValue(entry, field) {
  if (field === 'size') return entry.is_dir ? 0 : Number(entry.size || 0);
  if (field === 'mtime') {
    const ms = new Date(entry.modified || 0).getTime();
    return Number.isFinite(ms) ? ms : 0;
  }
  return (entry.name || '').toLowerCase();
}

function compareEntries(a, b, field) {
  const dirCmp = Number(Boolean(b.is_dir)) - Number(Boolean(a.is_dir));
  if (dirCmp !== 0) return dirCmp;
  const av = getSortValue(a, field);
  const bv = getSortValue(b, field);
  if (field === 'name') {
    return String(av).localeCompare(String(bv), undefined, { numeric: true, sensitivity: 'base' });
  }
  if (av < bv) return -1;
  if (av > bv) return 1;
  return 0;
}

function updateSortIndicators() {
  document.querySelectorAll('.file-header-cell[data-sort]').forEach(cell => {
    const active = cell.dataset.sort === currentSort.field;
    const indicator = cell.querySelector('.sort-indicator');
    if (indicator) {
      indicator.innerHTML = active ? iconSvg(currentSort.dir === 'asc' ? 'chevron-up' : 'chevron-down') : '';
    }
    cell.classList.toggle('active', active);
  });
}

function toggleSort(field) {
  if (currentSort.field === field) {
    currentSort.dir = currentSort.dir === 'asc' ? 'desc' : 'asc';
  } else {
    currentSort.field = field;
    currentSort.dir = 'asc';
  }
  if (currentFilesData) {
    renderFiles(currentFilesData);
  }
}

function renderFiles(data) {
  currentFilesData = data;
  const el = document.getElementById('file-list');
  const entries = [...(data.entries || [])].sort((a, b) => {
    const cmp = compareEntries(a, b, currentSort.field);
    return currentSort.dir === 'asc' ? cmp : -cmp;
  });

  if (entries.length === 0) {
    el.innerHTML = data.is_share_root
      ? '<div class="empty">没有共享文件夹 — 请要求管理员为您授予访问权限</div>'
      : '<div class="empty">此文件夹为空</div>';
    return;
  }

  let html = `
    <div class="file-row header">
      <div class="file-check" id="check-all" title="全选 / 取消全选">${iconSvg('square')}</div>
      <div class="file-icon"></div>
      <div class="file-header-cell file-name" data-sort="name">名称 <span class="sort-indicator">${currentSort.field === 'name' ? iconSvg(currentSort.dir === 'asc' ? 'chevron-up' : 'chevron-down') : ''}</span></div>
      <div class="file-header-cell file-size" data-sort="size">大小 <span class="sort-indicator">${currentSort.field === 'size' ? iconSvg(currentSort.dir === 'asc' ? 'chevron-up' : 'chevron-down') : ''}</span></div>
      <div class="file-header-cell file-mtime" data-sort="mtime">修改时间 <span class="sort-indicator">${currentSort.field === 'mtime' ? iconSvg(currentSort.dir === 'asc' ? 'chevron-up' : 'chevron-down') : ''}</span></div>
      <div class="file-actions"></div>
    </div>`;

  entries.forEach((entry, idx) => {
    const icon = entry.is_dir ? iconSvg('folder') : fileIcon(entry.name);
    const sizeStr = entry.is_dir ? '' : formatSize(entry.size);
    const cls = entry.is_dir ? 'file-row dir' : 'file-row';
    const sel = selectedPaths.has(entry.path);
    html += `
      <div class="${cls}${sel ? ' selected' : ''}" data-path="${escapeHtml(entry.path)}" data-name="${escapeHtml(entry.name)}" data-is-dir="${entry.is_dir}" data-idx="${idx}">
        <div class="file-check" data-check title="选择">${iconSvg(sel ? 'check-square' : 'square')}</div>
        <div class="file-icon">${icon}</div>
        <div class="file-name" title="${escapeHtml(entry.name)}">${escapeHtml(entry.name)}</div>
        <div class="file-size">${sizeStr}</div>
        <div class="file-mtime">${escapeHtml(entry.modified)}</div>
        <div class="file-actions">
          <button class="btn btn-sm btn-icon" data-action="download" title="${entry.is_dir ? '下载文件夹' : '下载'}">${iconSvg('download')}</button>
        </div>
      </div>`;
  });

  el.innerHTML = html;
  updateSortIndicators();

  el.querySelectorAll('.file-header-cell[data-sort]').forEach(cell => {
    cell.style.cursor = 'pointer';
    cell.addEventListener('click', () => {
      toggleSort(cell.dataset.sort);
    });
  });

  // Wire up the selection checkboxes (select-all in the header, per-row
  // checkboxes; Shift+click extends from the last touched checkbox).
  const checkAll = document.getElementById('check-all');
  if (checkAll) checkAll.addEventListener('click', (e) => {
    e.stopPropagation();
    toggleSelectAll();
  });
  el.querySelectorAll('[data-check]').forEach(c => {
    c.addEventListener('click', (e) => {
      e.stopPropagation();
      const row = c.closest('.file-row');
      const idx = parseInt(row.dataset.idx, 10);
      if (e.shiftKey && lastCheckIndex >= 0) {
        selectRange(lastCheckIndex, idx);
      } else {
        toggleRowSelection(row);
      }
      lastCheckIndex = idx;
    });
  });

  // Wire up click events. Ctrl/Cmd+click toggles multi-selection instead of
  // opening/navigating; plain clicks keep their original behavior.
  el.querySelectorAll('.file-row.dir').forEach(row => {
    row.addEventListener('click', (e) => {
      if (rowClickSelect(row, e)) return;
      navigate(row.dataset.path);
    });
  });

  // Clicking a file opens its detail view (inline preview for images /
  // media / text, online editing for writable text files).
  el.querySelectorAll('.file-row:not(.dir):not(.header)').forEach(row => {
    row.addEventListener('click', (e) => {
      if (rowClickSelect(row, e)) return;
      openFileDetail({ path: row.dataset.path, name: row.dataset.name });
    });
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
      if (e.target.closest('[data-action], .file-check')) return; // not from action buttons/checkbox
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
      // Multi-select aware: dragging one row of a multi-selection moves ALL
      // selected entries — give them the dragging visual too.
      if (selectedPaths.has(dragSource.path)) {
        document.querySelectorAll('#file-list .file-row').forEach(r => {
          if (selectedPaths.has(r.dataset.path)) r.classList.add('dragging');
        });
      } else {
        row.classList.add('dragging');
      }
    });
    row.addEventListener('dragend', () => {
      dragSource = null;
      document.querySelectorAll('#file-list .file-row.dragging').forEach(r => r.classList.remove('dragging'));
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

// ── Multi-select helpers ──

// Ctrl/Cmd+click on a row toggles its selection instead of opening it.
// Returns true when the click was consumed as a selection toggle.
function rowClickSelect(row, e) {
  if (!(e.ctrlKey || e.metaKey)) return false;
  toggleRowSelection(row);
  lastCheckIndex = parseInt(row.dataset.idx, 10);
  return true;
}

function toggleRowSelection(row) {
  const path = row.dataset.path;
  if (selectedPaths.has(path)) {
    selectedPaths.delete(path);
  } else {
    selectedPaths.set(path, {
      path,
      name: row.dataset.name,
      isDir: row.dataset.isDir === 'true',
    });
  }
  renderSelectionState();
  updateSelectionBar();
}

// Select every entry between two row indexes (Shift+click range).
function selectRange(a, b) {
  const rows = [...document.querySelectorAll('#file-list .file-row:not(.header)')];
  const lo = Math.min(a, b), hi = Math.max(a, b);
  rows.forEach(row => {
    const idx = parseInt(row.dataset.idx, 10);
    if (idx < lo || idx > hi) return;
    if (!selectedPaths.has(row.dataset.path)) {
      selectedPaths.set(row.dataset.path, {
        path: row.dataset.path,
        name: row.dataset.name,
        isDir: row.dataset.isDir === 'true',
      });
    }
  });
  renderSelectionState();
  updateSelectionBar();
}

function toggleSelectAll() {
  const rows = [...document.querySelectorAll('#file-list .file-row:not(.header)')];
  const allSelected = rows.length > 0 && rows.every(r => selectedPaths.has(r.dataset.path));
  rows.forEach(row => {
    if (allSelected) {
      selectedPaths.delete(row.dataset.path);
    } else if (!selectedPaths.has(row.dataset.path)) {
      selectedPaths.set(row.dataset.path, {
        path: row.dataset.path,
        name: row.dataset.name,
        isDir: row.dataset.isDir === 'true',
      });
    }
  });
  renderSelectionState();
  updateSelectionBar();
}

function clearSelection() {
  selectedPaths.clear();
  renderSelectionState();
  updateSelectionBar();
}

// Sync every row's checkbox/selected styling with selectedPaths.
function renderSelectionState() {
  document.querySelectorAll('#file-list .file-row:not(.header)').forEach(row => {
    const sel = selectedPaths.has(row.dataset.path);
    row.classList.toggle('selected', sel);
    const c = row.querySelector('.file-check');
    if (c) c.innerHTML = iconSvg(sel ? 'check-square' : 'square');
  });
  const rows = document.querySelectorAll('#file-list .file-row:not(.header)');
  const all = rows.length > 0 && [...rows].every(r => selectedPaths.has(r.dataset.path));
  const ca = document.getElementById('check-all');
  if (ca) ca.innerHTML = iconSvg(all ? 'check-square' : 'square');
}

// Show/hide the floating batch-action bar.
function updateSelectionBar() {
  const bar = document.getElementById('selection-bar');
  if (!bar) return;
  if (selectedPaths.size === 0) {
    bar.style.display = 'none';
    return;
  }
  bar.style.display = 'flex';
  const count = document.getElementById('selection-count');
  if (count) count.textContent = `已选择 ${selectedPaths.size} 项`;
}

// Batch-download every selected entry as ONE transfer task: transfers run
// sequentially with combined progress, and on FS-API browsers the destination
// directory is picked ONCE for the whole batch (libfw.setBatchReuse) instead
// of one picker per item.
async function downloadSelected() {
  const items = [...selectedPaths.values()];
  if (!items.length) return;
  if (items.length === 1) {
    // Single selection keeps the plain single-task semantics.
    const it = items[0];
    if (it.isDir) downloadFolder(it.path, it.name);
    else downloadFile(it.path, it.name);
    return;
  }
  const t = { kind: 'download', name: `${items.length} 项`, total: 0, done: 0, status: 'active', error: null };
  addTransfer(t);
  t.run = () => runBatchDownloadTask(t, items);
  t.run();
}

async function runBatchDownloadTask(t, items) {
  t.status = 'active';
  t.error = null;
  t.finalizing = false;
  renderTransfers();
  // Known total starts at the sum of the file sizes; folder totals only
  // become known when their own progress events start flowing, and are added
  // to the running total then.
  let knownTotal = items.reduce((s, i) => s + (i.isDir ? 0 : (i.size || 0)), 0);
  let completed = 0; // bytes finished by earlier items
  try {
    Libfw.setBatchReuse(true);
    for (const it of items) {
      let curDone = 0;
      let dirTotalAdded = false;
      // One read token per item (the paths differ); cheap — no prompts.
      const tokenResp = await API.getToken(it.path, 'read');
      const onEvent = (ev) => {
        if (ev.type !== 'progress') return;
        curDone = ev.done || 0;
        if (it.isDir && !dirTotalAdded && ev.total > 0) {
          dirTotalAdded = true;
          knownTotal += ev.total;
        }
        updateTransfer(t.id, { done: completed + curDone, total: knownTotal });
      };
      const run = it.isDir
        ? Libfw.downloadFolder(tokenResp.token, tokenResp.path, onEvent, (id) => { t.engineId = id; })
        : Libfw.downloadFile(tokenResp.token, tokenResp.path, it.name, onEvent, (id) => { t.engineId = id; });
      await run;
      completed += curDone;
      updateTransfer(t.id, { done: completed, total: knownTotal });
    }
    updateTransfer(t.id, { status: 'done', done: knownTotal, total: knownTotal, finalizing: false });
    setTimeout(() => removeTransfer(t.id), 3000);
  } catch (e) {
    const cancelled = e && (e.code === 'cancelled' || e.code === 'abort' || e.name === 'AbortError');
    updateTransfer(t.id, {
      status: cancelled ? 'cancelled' : 'error',
      done: completed,
      total: knownTotal,
      error: cancelled ? '' : (e && e.message) || String(e),
      finalizing: false,
    });
  } finally {
    Libfw.setBatchReuse(false);
  }
}

// Batch-delete every selected entry after an explicit confirmation.
function deleteSelected() {
  const items = [...selectedPaths.values()];
  if (!items.length) return;
  const preview = items.slice(0, 8).map(i => escapeHtml(i.name)).join('、')
    + (items.length > 8 ? ` 等 ${items.length} 项` : '');
  const hasDir = items.some(i => i.isDir);
  showModal('批量删除', `
    <p>确定要删除选中的 ${items.length} 项吗？</p>
    <p style="font-size:0.9em;color:var(--muted);word-break:break-all">${preview}</p>
    ${hasDir ? '<p style="color:var(--error);font-size:0.85em">选中的文件夹及其全部内容都将被删除！</p>' : ''}
  `, async () => {
    let failed = 0;
    for (const it of items) {
      try {
        await API.deleteFile(it.path);
      } catch (e) {
        failed++;
      }
    }
    clearSelection();
    hideModal();
    loadFiles(currentPath);
    if (failed) alert(`${failed} 项删除失败`);
  }, { okText: '删除' });
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
    // Don't hijack taps on the row's action buttons or the selection checkbox
    // (they have their own click handlers and are meant to be tapped, not
    // long-pressed).
    if (e.target.closest('[data-action], .file-check')) return;
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
    case 'share':
      showShareModal([file]);
      break;
    case 'rename':
      showRenameModal(file);
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
  // A user-cancelled row is final: late engine events (e.g. a progress tail
  // or an error after the force-cancel failsafe fired) must not resurrect it.
  if (t.status === 'cancelled' && patch.status && patch.status !== 'cancelled') return;
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
    // User pressed cancel; the engine hasn't confirmed the abort yet.
    case 'cancelling': return `${iconSvg('refresh-cw', 'spin')} 取消中`;
    case 'done': return `${iconSvg('check')} done`;
    case 'error': return `${iconSvg('alert-circle')} 失败`;
    case 'cancelled': return `${iconSvg('x')} 已取消`;
    default: return '';
  }
}

function transferPct(t) {
  if (t.total > 0) {
    const pct = Math.min(100, Math.round(((t.done || 0) / t.total) * 100));
    // Since libfw 0.2.4 the engine reports server-confirmed progress, so
    // `done` only reaches 100% when the transfer is actually complete. Keep
    // a 99 cap while active purely so we never show 100% before the
    // post-completion 'done' state.
    return t.status === 'active' || t.status === 'cancelling' ? Math.min(99, pct) : pct;
  }
  return 0;
}

// While finalizing, show the REAL percentage (98→100%) so the bar visibly
// fills to completion instead of sitting frozen at the 99% active cap.
function finalizingPct(t) {
  if (t.total > 0) return Math.min(100, Math.round(((t.done || 0) / t.total) * 100));
  return 99;
}

// ── Transfers: live adaptive-tuning readout ──
//
// libfw-client's tuning engine (autoTune) emits `{ type: 'tuning', phase,
// params, stats }` events while a transfer runs. The panel shows the latest
// state in a compact bar above the transfer list; `Libfw.tuning` holds the
// most recent snapshot so a transfer started before this UI subscribed still
// renders correctly.

const PHASE_LABEL = {
  uninitialized: 'tuning —',
  ramping: 'tuning ↑',
  settled: 'tuned ✓',
  degraded: 'degraded ⚠',
};

function fmtChunk(n) {
  if (n == null || isNaN(n)) return '—';
  if (n < 1024) return n + ' B';
  const u = ['KiB', 'MiB', 'GiB'];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  return (n >= 100 ? n.toFixed(0) : n.toFixed(1)) + ' ' + u[i];
}

function renderTuningBar() {
  const bar = document.getElementById('tuning-bar');
  if (!bar) return;
  const t = Libfw.tuning;
  // Show the statistics bar from the START of a transfer (with the configured
  // static values) so it is visible while transmitting, not only after the
  // engine has finished a measurement window. It updates live as tuning
  // `{ type: 'tuning' }` events arrive. Hide it when tuning is off or nothing
  // is currently transferring.
  const hasActive = transfers.some(x => x.status === 'active');
  if (!Libfw.opts.autoTune || !hasActive) {
    bar.style.display = 'none';
    return;
  }
  bar.style.display = 'flex';
  const set = (id, v) => {
    const el = document.getElementById(id);
    if (el) el.textContent = v;
  };
  const p = t.params || {};
  const s = t.stats || {};
  // Fall back to the configured values until the engine reports its own.
  const o = Libfw.opts || {};
  set('tuning-phase', (t.phase && PHASE_LABEL[t.phase]) || 'tuning —');
  set('tuning-conc', p.concurrency != null ? p.concurrency : (o.concurrency != null ? o.concurrency : '—'));
  set('tuning-uw', p.uploadWindow != null ? p.uploadWindow : (o.uploadWindow != null ? o.uploadWindow : '—'));
  set('tuning-dw', p.downloadWindow != null ? p.downloadWindow : (o.downloadWindow != null ? o.downloadWindow : '—'));
  set('tuning-chunk', p.chunkSize != null ? fmtChunk(p.chunkSize) : fmtChunk(o.chunkSize));
  set('tuning-rtt', s.rttMs != null ? Math.round(s.rttMs) + 'ms' : '—');
  set('tuning-mbps', s.mbps != null ? s.mbps.toFixed(1) : '—');
}

// Subscribe once on boot: every tuning event refreshes the bar live.
Libfw.onTuningChange = renderTuningBar;

// HTML of one transfer row. `data-sig` carries a signature of the parts that
// require a DOM rebuild when they change (status/error/finalizing); pure
// progress updates only patch numbers in place (see renderTransfers).
function transferRowHtml(t) {
  const done = t.status === 'done';
  // "finalizing": all bytes flushed to the server but COMPLETE not yet
  // confirmed — the row keeps an animated bar instead of freezing at 99%.
  const finalizing = t.status === 'active' && !!t.finalizing;
  const finished = done || t.status === 'error' || t.status === 'cancelled';
  const cancelling = t.status === 'cancelling';
  const action = finished
    ? `<button class="btn btn-sm" data-xfer-remove="${t.id}" title="移除">${iconSvg('x')}</button>`
    : `<button class="btn btn-sm" data-xfer-cancel="${t.id}" title="取消"${cancelling ? ' disabled' : ''}>${iconSvg('x')}</button>`;
  const arrow = iconSvg(t.kind === 'upload' ? 'upload' : 'download');
  const sub = t.kind === 'upload'
    ? `上传 · ${formatSize(t.total)}`
    : '下载';
  const pctVal = done ? 100 : (finalizing ? finalizingPct(t) : transferPct(t));
  return `
    <div class="transfer-row" data-xfer-id="${t.id}" data-sig="${transferRowSig(t)}">
      <div class="transfer-top">
        <span class="transfer-name" title="${escapeHtml(t.name)}">${arrow} ${escapeHtml(t.name)}</span>
        <span class="transfer-right">
          <span class="transfer-pct">${done ? '100%' : pctVal + '%'} ${transferStatusLabel(t)}</span>
          ${action}
        </span>
      </div>
      <div class="progress-bar"><div class="progress-fill${finalizing ? ' finalizing' : ''}" style="width:${pctVal}%"></div></div>
      <div class="transfer-sub">${sub}${t.error ? ` · <span class="err">${escapeHtml(t.error)}</span>` : ''}</div>
    </div>`;
}

// Row signature: when it changes the row HTML must be rebuilt (button kind,
// status label, error text); otherwise the row is patched in place.
function transferRowSig(t) {
  return `${t.status}|${!!t.finalizing}|${t.error || ''}`;
}

function wireTransferActions() {
  document.querySelectorAll('#transfers-list [data-xfer-remove]').forEach(btn => {
    btn.onclick = () => removeTransfer(parseInt(btn.dataset.xferRemove));
  });
  document.querySelectorAll('#transfers-list [data-xfer-cancel]').forEach(btn => {
    btn.onclick = () => {
      const t = transfers.find(x => x.id === parseInt(btn.dataset.xferCancel));
      if (t) requestCancel(t);
    };
  });
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
  renderTuningBar();

  // Rebuild the DOM only when the row SET changes (a transfer added or
  // removed). Progress updates then patch each row's numbers in place —
  // a full innerHTML rebuild on every progress tick used to destroy the
  // hovered X button and re-create it, which read as constant flickering.
  const ids = transfers.map(t => t.id).join(',');
  if (ids !== renderTransfers._rowIds) {
    renderTransfers._rowIds = ids;
    list.innerHTML = transfers.map(transferRowHtml).join('');
    wireTransferActions();
  }

  transfers.forEach(t => {
    const row = list.querySelector(`[data-xfer-id="${t.id}"]`);
    if (!row) return;
    if (row.dataset.sig !== transferRowSig(t)) {
      // Status/error changed → replace just this row (rare, so the momentary
      // hover reset is imperceptible).
      row.outerHTML = transferRowHtml(t);
      wireTransferActions();
      return;
    }
    // Same state: patch the moving numbers only.
    const done = t.status === 'done';
    const finalizing = t.status === 'active' && !!t.finalizing;
    const pctVal = done ? 100 : (finalizing ? finalizingPct(t) : transferPct(t));
    const pctEl = row.querySelector('.transfer-pct');
    if (pctEl) pctEl.innerHTML = `${done ? '100%' : pctVal + '%'} ${transferStatusLabel(t)}`;
    const fillEl = row.querySelector('.progress-fill');
    if (fillEl) fillEl.style.width = pctVal + '%';
  });
}

// User pressed the X on an active transfer.
//
// Feedback is immediate ('cancelling' state) and the abort is targeted at
// the transfer's ENGINE id (learned via the facade's onId callback) — using
// the UI's list id used to hit whatever transfer happened to share that
// number, which is why cancel sometimes did nothing.
//
// Failsafe: the engine only notices the abort at chunk boundaries (large
// chunks on a slow link can delay that a long while). If it hasn't settled
// after a grace period, the row is marked cancelled anyway — the engine's
// eventual rejection then lands on an already-cancelled row (ignored).
const CANCEL_GRACE_MS = 4000;

function requestCancel(t) {
  if (t.status !== 'active' && t.status !== 'cancelling') return;
  const firstClick = t.status === 'active';
  updateTransfer(t.id, { status: 'cancelling' });
  if (firstClick) {
    if (t.engineId != null) {
      Libfw.cancel(t.engineId);
    }
    clearTimeout(t._cancelTimer);
    t._cancelTimer = setTimeout(() => {
      const cur = transfers.find(x => x.id === t.id);
      if (cur && (cur.status === 'active' || cur.status === 'cancelling')) {
        updateTransfer(t.id, { status: 'cancelled', finalizing: false });
      }
    }, CANCEL_GRACE_MS);
  }
}

// ── Upload (via libfw SDK) ──
//
// Uploads go through the libfw-client SDK, which handles chunking, zstd
// compression and x-libfw-offset resume itself. The SDK builds `/file/{path}`
// from each plan entry; the wrapper prefixes every plan path with the
// destination directory shadow (`{dirShadow}/{rel}`) and the server's
// compound-shadow decoder resolves the real target — no per-file tokens are
// minted. Resume state is persisted per plan path in IndexedDB by the SDK.
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
    alert('此文件夹不允许上传');
    return;
  }

  // One write token for the destination; the server binds it to an opaque
  // shadow of the real directory path. This directory token covers the whole
  // subtree (the server authorizes the DECODED real path with prefix
  // semantics); its shadow is reused as the prefix of every plan path.
  let tokenResp;
  try {
    tokenResp = await API.getToken(destPath || '/', 'write');
  } catch (e) {
    alert('上传被拒绝: ' + e.message);
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
  t.run = () => runUploadTask(t, destPath, tokenResp.token, tokenResp.path, items);
  t.run();
}

async function runUploadTask(t, destPath, token, dirShadow, items) {
  t.status = 'active';
  t.error = null;
  t.finalizing = false;
  renderTransfers();
  try {
    await Libfw.upload(destPath, token, dirShadow, items, (ev) => {
      if (ev.type === 'progress') updateTransfer(t.id, {
        done: ev.done,
        total: ev.total,
        // libfw 0.2.4 reports server-confirmed progress; the last ~2% is the
        // brief "finalizing" tail before COMPLETE. Keep the animated bar so
        // it reads as finishing rather than a frozen 99%.
        finalizing: !!(ev.total > 0 && ev.done / ev.total >= 0.98),
      });
    }, (id) => { t.engineId = id; });
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
    // The SDK builds `/file/{path}` from the path we give it, which must be
    // the opaque shadow bound to the token (`tokenResp.path`) — the display
    // path we sent would fail the server's codec decode.
    t.run = () => runFileDownloadTask(t, tokenResp.path, name, tokenResp.token);
    t.run();
  })().catch(e => alert('下载被拒绝: ' + e.message));
}

async function runFileDownloadTask(t, path, name, token) {
  t.status = 'active';
  t.error = null;
  t.finalizing = false;
  renderTransfers();
  try {
    const progress = (ev) => {
      if (ev.type === 'progress') updateTransfer(t.id, {
        done: ev.done,
        total: ev.total,
        finalizing: !!(ev.total > 0 && ev.done / ev.total >= 0.98),
      });
    };
    // libfw-client 0.3.0 saves the file itself (streamed into a user-picked
    // directory via FS API, or via a traditional browser download). The SDK
    // sends the opaque shadow path over HTTP; the embedded server decodes it
    // to the real path the token is bound to. `name` is the display leaf
    // name the file is saved under — the shadow would be a useless filename.
    const done = await Libfw.downloadFile(token, path, name, progress, (id) => { t.engineId = id; });
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
    // Same as single files: the SDK walks `/dir/{path}` from this path, so
    // it must be the opaque shadow, not the display path we sent.
    t.run = () => runFolderDownloadTask(t, tokenResp.path, name, tokenResp.token);
    t.run();
  })().catch(e => alert('Download denied: ' + e.message));
}

async function runFolderDownloadTask(t, path, name, token) {
  t.status = 'active';
  t.error = null;
  t.finalizing = false;
  renderTransfers();
  try {
    // libfw-client 0.3.0 downloads the whole tree itself over HTTP
    // (streamed into a user-picked directory via FS API, or packed into a
    // `.zip` and saved via a normal browser download). It sends the opaque
    // shadow path; the embedded server decodes it to the real root path the
    // token is bound to, and every listed child comes back as a shadow too.
    const bytes = await Libfw.downloadFolder(token, path, (ev) => {
      if (ev.type === 'progress') updateTransfer(t.id, {
        done: ev.done,
        total: ev.total,
        finalizing: !!(ev.total > 0 && ev.done / ev.total >= 0.98),
      });
    }, (id) => { t.engineId = id; });
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
  showModal('重命名', `
    <label>为 "${escapeHtml(file.name)}" 新建名称:</label>
    <input type="text" id="rename-input" value="${escapeHtml(file.name)}">
  `, async () => {
    const newName = document.getElementById('rename-input').value.trim();
    if (!newName || newName === file.name) return;
    try {
      await API.renameFile(file.path, newName);
      loadFiles(currentPath);
      hideModal();
    } catch (e) {
      alert('重命名失败: ' + e.message);
    }
  });
}

function showMoveModal(file) {
  // The destination is entered in the current path space (virtual for
  // non-admins, real for admins); the server resolves it to a real path.
  const placeholder = currentPath ? currentPath + '/' : '/';
  showModal('移动', `
    <label>移动 "${escapeHtml(file.name)}" 到:</label>
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
      alert('移动失败: ' + e.message);
    }
  });
}

function showDeleteConfirm(file) {
  showModal('删除', `
    <p>确定要删除 "${escapeHtml(file.name)}" 吗?</p>
    ${file.isDir ? '<p style="color:var(--error);font-size:0.85em">这将删除整个文件夹及其所有内容！</p>' : ''}
  `, async () => {
    try {
      await API.deleteFile(file.path);
      loadFiles(currentPath);
      hideModal();
    } catch (e) {
      alert('删除失败: ' + e.message);
    }
  });
}

// ── New folder ──

function showMkdirModal() {
  showModal('新建文件夹', `
    <label>文件夹名称:</label>
    <input type="text" id="mkdir-input" placeholder="新文件夹">
  `, async () => {
    const name = document.getElementById('mkdir-input').value.trim();
    if (!name) return;
    try {
      await API.mkdir(currentPath, name);
      loadFiles(currentPath);
      hideModal();
    } catch (e) {
      alert('创建文件夹失败: ' + e.message);
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

// The entries carried by the current drag: the dragged row itself, or —
// when it belongs to the active multi-selection — EVERY selected entry.
function draggedItems() {
  if (!dragSource) return [];
  if (selectedPaths.has(dragSource.path) && selectedPaths.size > 1) {
    return [...selectedPaths.values()];
  }
  return [dragSource];
}

// Which of the dragged items may legally move into `destDir`: not a no-op
// (same location), and a folder never into itself or its own subfolder.
// Invalid ones are skipped at drop time instead of blocking the whole move.
function movableItems(destDir) {
  return draggedItems().filter(it => {
    const destPath = (destDir ? destDir + '/' : '') + it.name;
    if (destPath === it.path) return false;
    if (it.isDir) {
      if (it.path === destDir) return false;
      if (destDir.startsWith(it.path + '/')) return false;
    }
    return true;
  });
}

// Whether `destDir` accepts the current drag (at least one movable item).
function isValidDropTargetDir(destDir) {
  if (!dragSource) return false;
  return movableItems(destDir).length > 0;
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
  // The list background is a valid drop area for the current directory even
  // when the destination is effectively a no-op (same directory): it should
  // accept the release and simply do nothing instead of rejecting the drop.
  if (currentPath !== null && currentPath !== undefined) {
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
  // Dropping on the blank area of the current directory is a valid release
  // target, but it is a no-op for the same folder. Ignore it cleanly instead
  // of issuing a useless move request or failing the drag interaction.
  if (destDir === currentPath) {
    dragSource = null;
    clearDropTargets();
    clearAutoEnter();
    return;
  }
  const items = movableItems(destDir);
  dragSource = null;
  if (!items.length) return;
  let failed = 0;
  for (const it of items) {
    const destPath = (destDir ? destDir + '/' : '') + it.name;
    try {
      await API.moveFile(it.path, destPath);
    } catch (e) {
      failed++;
    }
  }
  // Whatever moved out of this directory is gone from the listing; the
  // selection no longer matches the fresh listing either.
  clearSelection();
  loadFiles(currentPath);
  if (failed) alert(`${failed} 项移动失败`);
}

// ── File page view (clicking a file opens a PAGE, like entering a folder) ──
//
// Clicking a file renders metadata + inline preview + actions into the MAIN
// area — never a modal — with the breadcrumb treating the file as the leaf
// and a 返回 button back to the listing. Images/video/audio stream from the
// raw endpoint (session-cookie auth in normal mode, the share token in share
// mode); small text files load their content and can be edited online when
// the user holds write permission. Real paths never appear here — the
// display path is resolved server-side, exactly like every other operation.
// The view is addressable via `#file/<encoded path>`, so browser back/forward
// and deep links work exactly like directory navigation.

async function openFileDetail(file, fromHash = false) {
  const path = file.path;
  if (!fromHash) setHashForFile(path);
  // The listing context becomes the file's parent directory: 返回 reloads it.
  currentPath = path.split('/').slice(0, -1).join('/');
  // Any multi-selection belonged to the listing we just left.
  clearSelection();
  const el = document.getElementById('file-list');
  el.innerHTML = '<div class="loading">加载中...</div>';
  updateFileNav(path);
  let detail;
  try {
    detail = await API.getFileDetail(path);
  } catch (e) {
    el.innerHTML = `
      <div class="file-view">
        <div class="file-view-head">
          <button class="btn btn-sm" id="fv-back">${iconSvg('corner-up-left')} 返回</button>
          <span class="file-view-title">${iconSvg('alert-circle')} 加载失败</span>
        </div>
        <div class="file-detail-empty">${iconSvg('alert-circle')} ${escapeHtml(e.message)}</div>
      </div>`;
    const back = document.getElementById('fv-back');
    if (back) back.onclick = () => navigate(currentPath);
    return;
  }
  renderFileDetail(detail);
}

// Breadcrumb for the file page: ancestor directories link back into the
// listing; the file itself is the (non-link) leaf.
function updateFileNav(path) {
  const nav = document.getElementById('path-nav');
  const parts = path.split('/').filter(Boolean);
  let html = '<a href="#" data-path="">' + iconSvg('home') + ' 主页</a>';
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
  nav.querySelectorAll('a').forEach(a => {
    a.addEventListener('click', (e) => {
      e.preventDefault();
      navigate(a.dataset.path);
    });
  });
}

function previewHtmlFor(detail) {
  const mime = detail.mime_type || '';
  const isImage = mime.startsWith('image/') && mime !== 'image/svg+xml';
  const isVideo = mime.startsWith('video/');
  const isAudio = mime.startsWith('audio/');
  const src = API.rawUrl(detail.path);

  if (detail.is_text && detail.content != null) {
    return `<pre class="text-preview">${escapeHtml(detail.content)}</pre>`;
  }
  if (detail.is_text && detail.truncated) {
    return `<div class="file-detail-empty">${iconSvg('file-text')} 文本文件过大 — 请下载后查看</div>`;
  }
  if (isImage) {
    return `<div class="media-preview"><img src="${src}" alt="${escapeHtml(detail.name)}" loading="lazy"></div>`;
  }
  if (isVideo) {
    return `<div class="media-preview"><video controls preload="metadata" src="${src}"></video></div>`;
  }
  if (isAudio) {
    return `<div class="media-preview"><audio controls src="${src}"></audio></div>`;
  }
  return `<div class="file-detail-empty">${fileIcon(detail.name)} 此文件类型暂不支持在线预览</div>`;
}

function renderFileDetail(detail) {
  const el = document.getElementById('file-list');
  const meta = `
    <div class="file-detail-meta">
      <div class="meta-row"><span class="meta-label">大小</span><span>${formatSize(detail.size)}</span></div>
      <div class="meta-row"><span class="meta-label">修改时间</span><span>${escapeHtml(detail.modified) || '—'}</span></div>
      <div class="meta-row"><span class="meta-label">类型</span><span title="${escapeHtml(detail.mime_type)}">${escapeHtml(detail.mime_type)}</span></div>
      <div class="meta-row"><span class="meta-label">路径</span><span class="meta-path" title="${escapeHtml(detail.path)}">${escapeHtml(detail.path)}</span></div>
    </div>`;

  const editable = detail.is_text && detail.content != null && detail.writable;
  const actions = `
    <div class="file-detail-actions">
      <button class="btn" id="detail-download">${iconSvg('download')} 下载</button>
      ${SHARE ? '' : `<button class="btn" id="detail-share">${iconSvg('share-2')} 分享</button>`}
      ${editable ? `<button class="btn btn-primary" id="detail-edit">${iconSvg('pen-tool')} 在线编辑</button>` : ''}
    </div>`;

  el.innerHTML = `
    <div class="file-view">
      <div class="file-view-head">
        <button class="btn btn-sm" id="fv-back">${iconSvg('corner-up-left')} 返回</button>
        <span class="file-view-title">${fileIcon(detail.name)} ${escapeHtml(detail.name)}</span>
      </div>
      <div class="file-detail">${meta}<div class="file-detail-preview">${previewHtmlFor(detail)}</div>${actions}</div>
    </div>`;

  document.getElementById('fv-back').onclick = () => navigate(currentPath);
  document.getElementById('detail-download').onclick = () => downloadFile(detail.path, detail.name);
  const shareBtn = document.getElementById('detail-share');
  if (shareBtn) shareBtn.onclick = () =>
    showShareModal([{ path: detail.path, name: detail.name, isDir: false }]);
  const editBtn = document.getElementById('detail-edit');
  if (editBtn) editBtn.onclick = () => enterTextEditMode(detail);
}

// Swap the preview for a textarea. `detail.content` is kept in sync so
// Cancel simply re-renders the (unmodified) preview, and Ctrl+S saves.
function enterTextEditMode(detail) {
  const container = document.querySelector('.file-view .file-detail-preview');
  if (!container) return;
  container.innerHTML = `
    <textarea id="text-editor" class="text-editor" spellcheck="false" wrap="off"></textarea>
    <div class="editor-actions">
      <button class="btn" id="edit-cancel">取消</button>
      <button class="btn btn-primary" id="edit-save">${iconSvg('check')} 保存</button>
    </div>`;

  const ta = document.getElementById('text-editor');
  ta.value = detail.content;
  ta.addEventListener('keydown', (e) => {
    // Ctrl/Cmd+S saves without leaving the editor.
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault();
      save();
      return;
    }
    // Tab inserts spaces instead of moving focus.
    if (e.key === 'Tab') {
      e.preventDefault();
      const s = ta.selectionStart, t = ta.selectionEnd;
      ta.value = ta.value.slice(0, s) + '  ' + ta.value.slice(t);
      ta.selectionStart = ta.selectionEnd = s + 2;
    }
  });
  ta.focus();

  document.getElementById('edit-cancel').onclick = () => renderFileDetail(detail);
  document.getElementById('edit-save').onclick = save;

  async function save() {
    const btn = document.getElementById('edit-save');
    btn.disabled = true;
    btn.textContent = '保存中...';
    try {
      await API.saveFileContent(detail.path, ta.value);
      detail.content = ta.value;
      // Re-render the file page (the listing refreshes when the user goes
      // back — loadFiles would replace this page with the listing).
      renderFileDetail(detail);
    } catch (e) {
      btn.disabled = false;
      btn.textContent = '保存';
      alert('保存失败: ' + e.message);
    }
  }
}

// ── Share links（分享）──
//
// Creates a temporary, no-login link (`/s/{token}`) for file(s)/folder(s)
// the user can read. One item shares that file/folder directly; several
// items create a "virtual root" collection whose landing page lists them.
// The server stores the token → path mapping, so real paths never appear in
// the URL; receivers need no account until the link expires.
function showShareModal(items) {
  if (!currentUser) { alert('请先登录'); return; }
  const arr = Array.isArray(items) ? items : [items];
  if (!arr.length) return;
  const multi = arr.length > 1;
  const title = multi ? `分享 ${arr.length} 项` : `分享 "${arr[0].name}"`;

  // Mint a share link with the TTL currently chosen in the modal and reveal
  // the copyable URL. Reused by the OK button (首次) and the "重新生成"
  // button afterwards.
  async function generateShareLink() {
    const ttl = parseInt(document.getElementById('share-ttl').value, 10) || 0;
    const okBtn = document.getElementById('modal-ok');
    okBtn.disabled = true;
    try {
      const resp = await API.createShare(arr.map(i => i.path), ttl);
      // ONESHARE_BASE is the CANONICAL app prefix (api.js rewrites it to the
      // share-prefixed base when running inside a share URL — sharing from a
      // share page would otherwise build /s/<old>/s/<new>).
      const url = `${location.origin}${ONESHARE_BASE}/s/${resp.token}`;
      const result = document.getElementById('share-result');
      if (result) {
        result.style.display = 'flex';
        const input = document.getElementById('share-url');
        input.value = url;
        document.getElementById('share-copy').onclick = (e) => copyText(url, e.currentTarget);
        input.focus();
        input.select();
      }
      okBtn.disabled = false;
    } catch (e) {
      okBtn.disabled = false;
      alert('创建分享链接失败: ' + e.message);
    }
  }

  const note = multi
    ? `<p class="share-note">将创建一个包含 ${arr.length} 个项目的分享页面：接收者无需登录即可浏览并下载其中的文件。</p>`
    : arr[0].isDir
      ? '<p class="share-note">将分享整个文件夹：接收者无需登录即可浏览并下载其中的文件。</p>'
      : '<p class="share-note">生成的链接无需登录即可访问。</p>';

  showModal(title, `
    <div class="share-form">
      <label>有效期:
        <select id="share-ttl">
          <option value="3600">1 小时</option>
          <option value="86400">1 天</option>
          <option value="604800" selected>7 天</option>
          <option value="2592000">30 天</option>
          <option value="0">永久有效</option>
        </select>
      </label>
      ${note}
      <div id="share-result" class="share-result" style="display:none">
        <input type="text" id="share-url" readonly>
        <button class="btn btn-sm" id="share-copy" title="复制链接">${iconSvg('copy')} 复制</button>
      </div>
    </div>
  `, async () => {
    await generateShareLink();
    // showModal re-arms the OK handler automatically; only the label changes.
    document.getElementById('modal-ok').textContent = '重新生成';
  }, { okText: '生成链接' });
}

// ── Share management（我的分享）──
// Regular users see and revoke their OWN links; admins see everyone's (the
// server filters, the creator column disambiguates).
async function showSharesModal() {
  if (!currentUser) { alert('请先登录'); return; }
  showModal(currentUser.is_admin ? '分享管理（全部用户）' : '我的分享',
    '<div class="shares-empty">加载中...</div>', null, { wide: true, hideFooter: true });

  let shares;
  try {
    shares = await API.listShares();
  } catch (e) {
    document.getElementById('modal-body').innerHTML =
      `<div class="shares-empty">${iconSvg('alert-circle')} 加载失败: ${escapeHtml(e.message)}</div>`;
    return;
  }

  const body = document.getElementById('modal-body');
  if (!shares.length) {
    body.innerHTML = '<div class="shares-empty">还没有创建任何分享链接</div>';
    return;
  }

  body.innerHTML = `
    <div class="shares-list">
      ${shares.map(s => {
        const url = `${location.origin}${API.base}/s/${s.token}`;
        const content = s.items && s.items.length
          ? `${escapeHtml(s.name)} · ${s.items.map(i => escapeHtml(i.name) + (i.isDir ? '/' : '')).join('、')}`
          : `${s.is_dir ? iconSvg('folder') : fileIcon(s.name)} ${escapeHtml(s.name)}`;
        const expires = s.expires_at ? `至 ${escapeHtml(s.expires_at)}` : '永久有效';
        return `
        <div class="share-row" data-token="${escapeHtml(s.token)}">
          <div class="share-main">
            <div class="share-name" title="${escapeHtml(content)}">${content}</div>
            <div class="share-meta">
              ${currentUser.is_admin ? `<span>${iconSvg('user')} ${escapeHtml(s.creator)}</span>` : ''}
              <span>${expires}</span>
              <a href="${API.base}/s/${escapeHtml(s.token)}" target="_blank" rel="noopener">${escapeHtml('/s/' + s.token.slice(0, 12))}…</a>
            </div>
          </div>
          <div class="share-actions">
            <button class="btn btn-sm" data-share-copy title="复制链接">${iconSvg('copy')}</button>
            <button class="btn btn-sm" data-share-open title="打开" onclick="window.open('${API.base}/s/${escapeHtml(s.token)}', '_blank')">${iconSvg('globe')}</button>
            <button class="btn btn-sm btn-danger" data-share-revoke title="撤销分享">${iconSvg('trash-2')}</button>
          </div>
        </div>`;
      }).join('')}
    </div>`;

  body.querySelectorAll('[data-share-copy]').forEach(btn => {
    btn.onclick = () => {
      const row = btn.closest('.share-row');
      const s = shares.find(x => x.token === row.dataset.token);
      if (s) copyText(`${location.origin}${API.base}/s/${s.token}`, btn);
    };
  });
  body.querySelectorAll('[data-share-revoke]').forEach(btn => {
    btn.onclick = async () => {
      btn.disabled = true;
      try {
        await API.revokeShare(btn.closest('.share-row').dataset.token);
        btn.closest('.share-row').remove();
        if (!body.querySelector('.share-row')) {
          body.innerHTML = '<div class="shares-empty">还没有创建任何分享链接</div>';
        }
      } catch (e) {
        btn.disabled = false;
        alert('撤销失败: ' + e.message);
      }
    };
  });
}

// Clipboard write with execCommand fallback (http deployments, permissions).
async function copyText(text, btn) {
  let ok = false;
  try {
    await navigator.clipboard.writeText(text);
    ok = true;
  } catch (e) {
    const input = document.getElementById('share-url');
    if (input) { input.value = text; input.focus(); input.select(); ok = document.execCommand('copy'); }
  }
  if (btn) {
    const old = btn.innerHTML;
    btn.innerHTML = ok ? iconSvg('check') : iconSvg('x');
    setTimeout(() => { btn.innerHTML = old; }, 1500);
  }
}

// ── Modal helpers ──
//
// `opts`:
//   wide       → add the `wide` class (large dialogs, e.g. file preview)
//   hideFooter → hide the OK/Cancel footer (dialogs with their own actions)
//   okText     → custom OK label
function showModal(title, bodyHtml, onOk, opts = {}) {
  const modal = document.getElementById('modal');
  document.getElementById('modal-title').textContent = title;
  document.getElementById('modal-body').innerHTML = bodyHtml;
  modal.classList.toggle('wide', !!opts.wide);
  document.querySelector('#modal .modal-footer').style.display = opts.hideFooter ? 'none' : 'flex';
  document.getElementById('modal-overlay').style.display = 'flex';

  const okBtn = document.getElementById('modal-ok');
  const cancelBtn = document.getElementById('modal-cancel');
  const closeBtn = document.getElementById('modal-close');
  if (opts.okText) okBtn.textContent = opts.okText;
  // A previous dialog may have been dismissed while its OK handler was
  // running (share generation, batch delete…); never open a new one with a
  // dead OK button.
  okBtn.disabled = false;

  // Cancel/Close stay wired for the modal's ENTIRE lifetime: an async OK
  // handler must never trap the user in the dialog (they used to be unbound
  // the moment OK was clicked, leaving only the overlay to click).
  cancelBtn.onclick = () => hideModal();
  closeBtn.onclick = () => hideModal();

  // OK handler: no concurrent re-entry (double-click can't run a destructive
  // action twice), and re-armed after a FAILED handler (bad name, network
  // error) so the action can simply be retried. If the handler closes the
  // dialog, the button stays dead — the modal is gone anyway.
  const runOk = async () => {
    okBtn.onclick = null;
    okBtn.disabled = true;
    try {
      if (onOk) await onOk();
    } finally {
      const overlay = document.getElementById('modal-overlay');
      if (overlay && overlay.style.display !== 'none') {
        okBtn.disabled = false;
        okBtn.onclick = runOk;
      }
    }
  };
  okBtn.onclick = runOk;

  // Focus input if present
  const input = document.getElementById('modal-body').querySelector('input');
  if (input) setTimeout(() => input.focus(), 100);
}

function hideModal() {
  document.getElementById('modal-overlay').style.display = 'none';
  // Reset the OK button (a confirm dialog may have restyled it) so the next
  // modal starts in its default state, however this modal was dismissed.
  const okBtn = document.getElementById('modal-ok');
  if (okBtn) {
    okBtn.textContent = '确定';
    okBtn.classList.remove('btn-danger');
    okBtn.disabled = false;
  }
  // Reset wide/footer overrides from the previous dialog.
  const modal = document.getElementById('modal');
  if (modal) modal.classList.remove('wide');
  const footer = document.querySelector('#modal .modal-footer');
  if (footer) footer.style.display = 'flex';
}

// ── Utilities ──

// ── Share mode UI adjustments ──
// A share visitor IS a temporary read-only user (the backend's ShareProxy
// swaps the URL token for their session), so the server already rejects any
// write; here we merely hide the controls that can never succeed, while the
// listing, breadcrumb, file pages, previews and libfw downloads run through
// the exact same code as the normal app.
function applyShareMode() {
  if (!SHARE) return;
  ['btn-upload', 'btn-upload-folder', 'btn-mkdir', 'btn-my-shares', 'btn-admin'].forEach(id => {
    const b = document.getElementById(id);
    if (b) b.style.display = 'none';
  });
  // Context menu: receivers can only download.
  document.querySelectorAll('#ctx-menu .ctx-item').forEach(item => {
    if (item.dataset.action !== 'download') item.style.display = 'none';
  });
  // Selection bar: batch download stays; share/delete need a real account.
  ['sel-share', 'sel-delete'].forEach(id => {
    const b = document.getElementById(id);
    if (b) b.style.display = 'none';
  });
}

// In share mode the tab title carries the share name (the temporary
// identity's display name); the user-info slot is rendered by auth.js from
// that identity's session (updateUserUI).
function applyShareHeader() {
  if (!SHARE) return;
  const name = currentUser && currentUser.display_name;
  document.title = name ? `共享 - ${name}` : 'OneShare - 共享';
}

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
