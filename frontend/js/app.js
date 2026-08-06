// Main app entry point
document.addEventListener('DOMContentLoaded', async () => {
  // Check auth first
  await checkAuth();

  // Load initial file listing
  loadFiles('');

  // Setup event listeners
  setupEventListeners();
  setupDragDrop();

  // Close context menu on click outside
  document.addEventListener('click', () => hideContextMenu());
});

function setupEventListeners() {
  // Upload button
  document.getElementById('btn-upload')?.addEventListener('click', () => {
    document.getElementById('file-input').click();
  });

  // Upload Folder button
  document.getElementById('btn-upload-folder')?.addEventListener('click', () => {
    document.getElementById('folder-input').click();
  });

  // File input change
  document.getElementById('file-input')?.addEventListener('change', (e) => {
    if (e.target.files.length > 0) {
      handleUpload(Array.from(e.target.files).map(f => ({ file: f, relPath: f.name })));
      e.target.value = '';
    }
  });

  // Folder input change
  document.getElementById('folder-input')?.addEventListener('change', (e) => {
    if (e.target.files.length > 0) {
      handleUpload(Array.from(e.target.files).map(f => ({ file: f, relPath: f.webkitRelativePath || f.name })));
      e.target.value = '';
    }
  });

  // New folder button
  document.getElementById('btn-mkdir')?.addEventListener('click', () => {
    if (!currentUser) { alert('Please login first'); return; }
    showMkdirModal();
  });

  // Refresh button
  document.getElementById('btn-refresh')?.addEventListener('click', () => {
    loadFiles(currentPath);
  });

  // Admin button
  document.getElementById('btn-admin')?.addEventListener('click', () => {
    toggleAdminPanel();
  });

  // Admin close button
  document.getElementById('admin-close')?.addEventListener('click', () => {
    document.getElementById('admin-panel').style.display = 'none';
  });

  // Admin tabs
  document.querySelectorAll('.admin-tab').forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll('.admin-tab').forEach(t => t.classList.remove('active'));
      tab.classList.add('active');
      loadAdminPanel(tab.dataset.tab);
    });
  });

  // Keyboard shortcuts
  document.addEventListener('keydown', (e) => {
    if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA') return;
    switch (e.key) {
      case 'Delete':
        if (selectedFile) handleContextAction('delete', selectedFile);
        break;
      case 'F2':
        if (selectedFile) handleContextAction('rename', selectedFile);
        break;
      case 'F5':
        e.preventDefault();
        loadFiles(currentPath);
        break;
    }
  });

  // Modal overlay click to close
  document.getElementById('modal-overlay')?.addEventListener('click', (e) => {
    if (e.target === document.getElementById('modal-overlay')) {
      hideModal();
    }
  });

  // ESC to close modal
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      hideModal();
      hideContextMenu();
    }
  });
}

function toggleAdminPanel() {
  const panel = document.getElementById('admin-panel');
  if (panel.style.display === 'none' || !panel.style.display) {
    panel.style.display = 'flex';
    // Activate first tab
    const firstTab = document.querySelector('.admin-tab');
    if (firstTab) firstTab.click();
  } else {
    panel.style.display = 'none';
  }
}
