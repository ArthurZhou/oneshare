// File Explorer state
let currentPath = '';
let selectedFile = null;

async function loadFiles(path) {
  currentPath = path || '';
  const el = document.getElementById('file-list');
  try {
    const data = await API.listFiles(currentPath);
    renderFiles(data);
    updatePathNav(data);
  } catch (e) {
    el.innerHTML = `<div class="empty">Error: ${escapeHtml(e.message)}</div>`;
  }
}

function updatePathNav(data) {
  const nav = document.getElementById('path-nav');
  const parts = data.current_path.split('/').filter(Boolean);
  let html = '<a href="#" data-path="">📁 Home</a>';
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
      loadFiles(a.dataset.path);
    });
  });
}

function renderFiles(data) {
  const el = document.getElementById('file-list');
  if (data.entries.length === 0) {
    el.innerHTML = '<div class="empty">This folder is empty</div>';
    return;
  }

  let html = `
    <div class="file-row header">
      <div class="file-icon"></div>
      <div class="file-name" style="font-weight:600;color:#94a3b8;font-size:0.8em">Name</div>
      <div class="file-size" style="font-weight:600;color:#94a3b8;font-size:0.8em">Size</div>
      <div class="file-mtime" style="font-weight:600;color:#94a3b8;font-size:0.8em">Modified</div>
      <div class="file-actions"></div>
    </div>`;

  data.entries.forEach(entry => {
    const icon = entry.is_dir ? '📁' : fileIcon(entry.name);
    const sizeStr = entry.is_dir ? '' : formatSize(entry.size);
    const cls = entry.is_dir ? 'file-row dir' : 'file-row';
    html += `
      <div class="${cls}" data-path="${escapeHtml(entry.path)}" data-name="${escapeHtml(entry.name)}" data-is-dir="${entry.is_dir}">
        <div class="file-icon">${icon}</div>
        <div class="file-name" title="${escapeHtml(entry.name)}">${escapeHtml(entry.name)}</div>
        <div class="file-size">${sizeStr}</div>
        <div class="file-mtime">${escapeHtml(entry.modified)}</div>
        <div class="file-actions">
          <button class="btn btn-sm btn-icon" data-action="download" title="${entry.is_dir ? 'Download as ZIP' : 'Download'}">⬇</button>
        </div>
      </div>`;
  });

  el.innerHTML = html;

  // Wire up click events
  el.querySelectorAll('.file-row.dir').forEach(row => {
    row.addEventListener('click', () => loadFiles(row.dataset.path));
  });

  // Wire up download buttons (files download directly, folders download as ZIP)
  el.querySelectorAll('[data-action="download"]').forEach(btn => {
    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      const row = btn.closest('.file-row');
      if (row.dataset.isDir === 'true') {
        downloadFolder(row.dataset.path, row.dataset.name);
      } else {
        downloadFile(row.dataset.path);
      }
    });
  });

  // Wire up context menu
  el.querySelectorAll('.file-row').forEach(row => {
    if (row.classList.contains('header')) return;
    row.addEventListener('contextmenu', (e) => {
      e.preventDefault();
      selectedFile = { path: row.dataset.path, name: row.dataset.name, isDir: row.dataset.isDir === 'true' };
      showContextMenu(e.clientX, e.clientY, selectedFile);
    });
  });
}

function showContextMenu(x, y, file) {
  const menu = document.getElementById('ctx-menu');
  menu.style.display = 'block';
  menu.style.left = x + 'px';
  menu.style.top = y + 'px';

  // Download is available for both files and folders (folders download as ZIP)
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

async function handleContextAction(action, file) {
  switch (action) {
    case 'download':
      if (file.isDir) await downloadFolder(file.path, file.name);
      else await downloadFile(file.path);
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

// ── File operations ──

async function downloadFile(path) {
  try {
    const tokenResp = await API.getToken(path, 'read');
    const downloadUrl = `/file/${encodeURIComponent(path)}`;

    const res = await fetch(downloadUrl, {
      headers: { 'Authorization': `Bearer ${tokenResp.token}` },
    });
    if (!res.ok) throw new Error(`Download failed: HTTP ${res.status}`);
    const blob = await res.blob();
    triggerDownload(blob, path.split('/').pop());
  } catch (e) {
    alert('Download failed: ' + e.message);
  }
}

// Download a folder as a ZIP: enumerate all files under it via libfw's /dir
// endpoint, fetch each file through libfw's /file endpoint, and pack them
// client-side into a store-only ZIP archive.
async function downloadFolder(path, name) {
  try {
    const tokenResp = await API.getToken(path, 'read');
    const files = await collectFolderFiles(path, tokenResp.token);
    if (!files.length) {
      alert('Folder is empty');
      return;
    }

    const entries = [];
    for (const f of files) {
      const res = await fetch(`/file/${encodeURIComponent(f.path)}`, {
        headers: { 'Authorization': `Bearer ${tokenResp.token}` },
      });
      if (!res.ok) throw new Error(`Failed to download ${f.path}: HTTP ${res.status}`);
      const data = await res.arrayBuffer();
      const rel = name + '/' + (path ? f.path.substring(path.length + 1) : f.path);
      entries.push({ path: rel, data });
    }

    const zip = await buildZip(entries);
    triggerDownload(zip, name + '.zip');
  } catch (e) {
    alert('Folder download failed: ' + e.message);
  }
}

// Recursively list every file under `path` using libfw's /dir listing.
async function collectFolderFiles(path, token) {
  const files = [];
  async function walk(dir) {
    const entries = await API.listDir(dir, token);
    for (const entry of entries) {
      if (entry.is_dir) {
        await walk(entry.path);
      } else {
        files.push({ path: entry.path });
      }
    }
  }
  await walk(path);
  return files;
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
  showModal('Move', `
    <label>Move "${escapeHtml(file.name)}" to:</label>
    <input type="text" id="move-input" placeholder="/path/to/destination" value="${escapeHtml(currentPath ? currentPath + '/' : '')}">
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
    ${file.isDir ? '<p style="color:#fca5a5;font-size:0.85em">This will delete the entire folder and all its contents!</p>' : ''}
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

// ── Upload ──

async function handleUpload(items) {
  if (!items.length) return;
  try {
    const tokenResp = await API.getToken(currentPath || '/', 'write');
    const progressEl = document.getElementById('upload-progress');
    const nameEl = document.getElementById('upload-name');
    const pctEl = document.getElementById('upload-pct');
    const fillEl = document.getElementById('progress-fill');
    progressEl.style.display = 'block';

    for (const { file, relPath } of items) {
      nameEl.textContent = `Uploading: ${relPath}`;
      pctEl.textContent = '0%';
      fillEl.style.width = '0%';

      const uploadPath = (currentPath ? currentPath + '/' : '') + relPath;

      await new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest();
        xhr.open('POST', `/file/${encodeURIComponent(uploadPath)}`, true);
        xhr.setRequestHeader('Authorization', `Bearer ${tokenResp.token}`);
        xhr.setRequestHeader('x-libfw-file-meta', JSON.stringify({
          path: uploadPath,
          size: file.size
        }));
        xhr.setRequestHeader('Content-Type', 'application/octet-stream');

        xhr.upload.onprogress = (e) => {
          if (e.lengthComputable) {
            const pct = Math.round((e.loaded / e.total) * 100);
            pctEl.textContent = pct + '%';
            fillEl.style.width = pct + '%';
          }
        };

        xhr.onload = () => {
          if (xhr.status >= 200 && xhr.status < 300) resolve();
          else reject(new Error(`HTTP ${xhr.status}`));
        };
        xhr.onerror = () => reject(new Error('Network error'));
        xhr.send(file);
      });
    }

    progressEl.style.display = 'none';
    loadFiles(currentPath);
  } catch (e) {
    document.getElementById('upload-progress').style.display = 'none';
    alert('Upload failed: ' + e.message);
  }
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
      if (item.kind === 'file' && item.webkitGetAsEntry) {
        entries.push(item.webkitGetAsEntry());
      } else if (item.kind === 'file') {
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
    pdf: '📕', doc: '📘', docx: '📘', xls: '📗', xlsx: '📗', ppt: '📙', pptx: '📙',
    jpg: '🖼', jpeg: '🖼', png: '🖼', gif: '🖼', svg: '🖼', webp: '🖼',
    mp4: '🎬', avi: '🎬', mkv: '🎬', mov: '🎬',
    mp3: '🎵', wav: '🎵', flac: '🎵', aac: '🎵',
    zip: '📦', rar: '📦', tar: '📦', gz: '📦', '7z': '📦',
    txt: '📄', md: '📄', json: '📄', xml: '📄', yaml: '📄', yml: '📄', toml: '📄',
    js: '📜', ts: '📜', jsx: '📜', tsx: '📜', py: '📜', rs: '📜', go: '📜', java: '📜', c: '📜', cpp: '📜',
    html: '🌐', css: '🎨',
  };
  return icons[ext] || '📄';
}

function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

// ── ZIP helpers (store-only; used for folder download) ──

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = (c & 1) ? (0xEDB88320 ^ (c >>> 1)) : (c >>> 1);
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  const u8 = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  let c = 0xFFFFFFFF;
  for (let i = 0; i < u8.length; i++) c = CRC_TABLE[(c ^ u8[i]) & 0xFF] ^ (c >>> 8);
  return (c ^ 0xFFFFFFFF) >>> 0;
}

function dosDateTime(date) {
  const time = ((date.getHours() & 0x1f) << 11)
    | ((date.getMinutes() & 0x3f) << 5)
    | ((date.getSeconds() >> 1) & 0x1f);
  const day = ((date.getFullYear() >= 1980 ? date.getFullYear() - 1980 : 0) << 9)
    | (((date.getMonth() + 1) & 0x0f) << 5)
    | (date.getDate() & 0x1f);
  return { time, day };
}

// Build a valid store-only ZIP archive from [{ path, data }] entries.
async function buildZip(entries) {
  const encoder = new TextEncoder();
  const now = dosDateTime(new Date());
  const localParts = [];
  const centralHeaders = [];
  let offset = 0;

  for (const entry of entries) {
    const nameBytes = encoder.encode(entry.path);
    const data = entry.data instanceof Uint8Array ? entry.data : new Uint8Array(entry.data);
    const crc = crc32(data);
    const size = data.length;

    // Local file header
    const local = new Uint8Array(30 + nameBytes.length);
    const lv = new DataView(local.buffer);
    lv.setUint32(0, 0x04034b50, true);
    lv.setUint16(4, 20, true);
    lv.setUint16(6, 0x0800, true); // UTF-8 filename flag
    lv.setUint16(8, 0, true); // store (no compression)
    lv.setUint16(10, now.time, true);
    lv.setUint16(12, now.day, true);
    lv.setUint32(14, crc, true);
    lv.setUint32(18, size, true);
    lv.setUint32(22, size, true);
    lv.setUint16(26, nameBytes.length, true);
    lv.setUint16(28, 0, true);
    local.set(nameBytes, 30);
    localParts.push(local, data);

    // Central directory header
    const central = new Uint8Array(46 + nameBytes.length);
    const cv = new DataView(central.buffer);
    cv.setUint32(0, 0x02014b50, true);
    cv.setUint16(4, 20, true);
    cv.setUint16(6, 20, true);
    cv.setUint16(8, 0x0800, true);
    cv.setUint16(10, 0, true);
    cv.setUint16(12, now.time, true);
    cv.setUint16(14, now.day, true);
    cv.setUint32(16, crc, true);
    cv.setUint32(20, size, true);
    cv.setUint32(24, size, true);
    cv.setUint16(28, nameBytes.length, true);
    cv.setUint16(30, 0, true);
    central.set(nameBytes, 46);
    cv.setUint32(42, offset, true);

    centralHeaders.push(central);
    offset += local.length + size;
  }

  const centralStart = offset;
  let centralSize = 0;
  for (const c of centralHeaders) centralSize += c.length;

  // End of central directory record
  const eocd = new Uint8Array(22);
  const ev = new DataView(eocd.buffer);
  ev.setUint32(0, 0x06054b50, true);
  ev.setUint16(8, entries.length, true);
  ev.setUint16(10, entries.length, true);
  ev.setUint32(12, centralSize, true);
  ev.setUint32(16, centralStart, true);

  return new Blob([...localParts, ...centralHeaders, eocd], { type: 'application/zip' });
}

function triggerDownload(blob, filename) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  // Defer revoke so the browser has time to start the download.
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
