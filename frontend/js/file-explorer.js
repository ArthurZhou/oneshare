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
          ${!entry.is_dir ? `<button class="btn btn-sm btn-icon" data-action="download" title="Download">⬇</button>` : ''}
        </div>
      </div>`;
  });

  el.innerHTML = html;

  // Wire up click events
  el.querySelectorAll('.file-row.dir').forEach(row => {
    row.addEventListener('click', () => loadFiles(row.dataset.path));
  });

  // Wire up download buttons
  el.querySelectorAll('[data-action="download"]').forEach(btn => {
    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      const row = btn.closest('.file-row');
      downloadFile(row.dataset.path);
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

  // Update menu items based on file type
  const downloadItem = menu.querySelector('[data-action="download"]');
  if (downloadItem) downloadItem.style.display = file.isDir ? 'none' : '';

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
      await downloadFile(file.path);
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
    // Get wfw token for download
    const tokenResp = await API.getWfwToken(path);
    // Simple HTTP download via wfw
    const wfwBase = `http://${window.location.hostname}:${tokenResp.wfw_port}`;
    const downloadUrl = `${wfwBase}/wfw/download?path=${encodeURIComponent(path)}`;

    // Create a download link
    const a = document.createElement('a');
    a.href = downloadUrl;
    // Use Authorization header via a proxy or direct link with token
    // For simplicity, we do a fetch + blob download
    const res = await fetch(downloadUrl, {
      headers: { 'Authorization': `Bearer ${tokenResp.download_token}` },
    });
    if (!res.ok) throw new Error(`Download failed: HTTP ${res.status}`);
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    a.href = url;
    a.download = path.split('/').pop();
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  } catch (e) {
    alert('Download failed: ' + e.message);
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

async function handleUpload(files) {
  if (!files.length) return;
  try {
    const tokenResp = await API.getWfwToken(currentPath || '/');
    const progressEl = document.getElementById('upload-progress');
    const nameEl = document.getElementById('upload-name');
    const pctEl = document.getElementById('upload-pct');
    const fillEl = document.getElementById('progress-fill');
    progressEl.style.display = 'block';

    for (const file of files) {
      nameEl.textContent = `Uploading: ${file.name}`;
      pctEl.textContent = '0%';
      fillEl.style.width = '0%';

      // Use simple direct upload via wfw
      const wfwBase = `http://${window.location.hostname}:${tokenResp.wfw_port}`;
      const uploadPath = `uploads/${currentUser ? currentUser.id : 'anon'}/${(currentPath ? currentPath + '/' : '')}${file.name}`;

      await new Promise((resolve, reject) => {
        const xhr = new XMLHttpRequest();
        xhr.open('PUT', `${wfwBase}/wfw/upload`, true);
        xhr.setRequestHeader('Authorization', `Bearer ${tokenResp.upload_token}`);
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
    const files = Array.from(e.dataTransfer.files);
    if (files.length > 0) {
      handleUpload(files);
    }
  });
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
