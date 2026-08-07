// Auth state
let currentUser = null;

async function checkAuth() {
  try {
    const resp = await API.getMe();
    currentUser = resp.user;
    updateUserUI();
    return currentUser;
  } catch (e) {
    currentUser = null;
    updateUserUI();
    return null;
  }
}

function updateUserUI() {
  const el = document.getElementById('user-info');
  if (!el) return;
  if (currentUser) {
    el.innerHTML = `
      <span>👤 ${escapeHtml(currentUser.display_name)}</span>
      ${currentUser.is_admin ? '<span class="admin-tag">(admin)</span>' : ''}
      <a href="/auth/logout" class="btn btn-sm">Logout</a>
    `;
    // Show/hide admin button
    const adminBtn = document.getElementById('btn-admin');
    if (adminBtn) adminBtn.style.display = currentUser.is_admin ? '' : 'none';
  } else {
    el.innerHTML = '<a href="/auth/login" class="btn btn-sm">Login</a>';
    const adminBtn = document.getElementById('btn-admin');
    if (adminBtn) adminBtn.style.display = 'none';
  }
}
