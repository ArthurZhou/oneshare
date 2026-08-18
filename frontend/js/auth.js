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
    // Logout is a POST (not a link): a GET logout endpoint is CSRF-able by
    // any page that can make the browser navigate to it (image tags, etc.).
    // POST with SameSite cookies is not cross-site triggerable.
    el.innerHTML = `
      <span>${iconSvg('user')} ${escapeHtml(currentUser.display_name)}</span>
      ${currentUser.is_admin ? '<span class="admin-tag">(admin)</span>' : ''}
      <form action="${API.base}/auth/logout" method="post" style="display:inline">
        <button type="submit" class="btn btn-sm">Logout</button>
      </form>
    `;
    // Show/hide admin button
    const adminBtn = document.getElementById('btn-admin');
    if (adminBtn) adminBtn.style.display = currentUser.is_admin ? '' : 'none';
  } else {
    el.innerHTML = `<a href="${API.base}/auth/login" class="btn btn-sm">Login</a>`;
    const adminBtn = document.getElementById('btn-admin');
    if (adminBtn) adminBtn.style.display = 'none';
  }
}
