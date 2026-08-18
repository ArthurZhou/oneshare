// Admin Panel Management
//
// Three tabs — ACL rules, Groups, Users. Each panel renders into
// #admin-content when its tab is activated; the search boxes filter the
// already-rendered rows client-side (no extra round-trips). Destructive
// actions go through a confirm dialog and feedback is shown with toasts
// instead of alert().

// Caches of the last-fetched records, so edit flows can look up raw values
// instead of re-parsing the DOM.
let aclData = [];
let groupData = [];
let userData = [];
let aclEditingId = null;

// ── UI helpers ──

// Toast feedback (top-right; does not block interaction).
function showToast(message, type = 'info') {
  const container = document.getElementById('toast-container');
  if (!container) return;
  const el = document.createElement('div');
  el.className = `toast ${type}`;
  el.textContent = message;
  container.appendChild(el);
  setTimeout(() => {
    el.classList.add('toast-out');
    setTimeout(() => el.remove(), 300);
  }, 2600);
}

// Custom confirm dialog built on the global modal. Calls onConfirm() only
// when the user confirms; hideModal() restores the OK button for other
// callers.
function confirmDialog(title, message, onConfirm) {
  const titleEl = document.getElementById('modal-title');
  const body = document.getElementById('modal-body');
  const okBtn = document.getElementById('modal-ok');
  const cancelBtn = document.getElementById('modal-cancel');
  const closeBtn = document.getElementById('modal-close');
  const overlay = document.getElementById('modal-overlay');

  titleEl.textContent = title;
  body.innerHTML = `<div class="confirm-msg">${escapeHtml(message)}</div>`;
  okBtn.textContent = '确认';
  okBtn.classList.add('btn-danger');
  okBtn.onclick = async () => {
    hideModal();
    try { await onConfirm(); }
    catch (e) { showToast(e.message || String(e), 'error'); }
  };
  cancelBtn.onclick = hideModal;
  closeBtn.onclick = hideModal;
  overlay.style.display = 'flex';
}

// Attach a client-side filter to a search input that hides non-matching rows.
function wireSearch(inputId, rowSelector) {
  const input = document.getElementById(inputId);
  if (!input) return;
  input.addEventListener('input', () => {
    const q = input.value.trim().toLowerCase();
    document.querySelectorAll(rowSelector).forEach((row) => {
      row.style.display = (!q || row.textContent.toLowerCase().includes(q)) ? '' : 'none';
    });
  });
}

// Update a tab button's label with a live item count, e.g. "ACL (12)".
function setTabCount(tab, count) {
  const btn = document.querySelector(`.admin-tab[data-tab="${tab}"]`);
  if (!btn) return;
  const base = { acl: '访问控制', groups: '群组', users: '用户' }[tab] || tab;
  btn.textContent = `${base} (${count})`;
}

async function loadAdminPanel(tab) {
  const content = document.getElementById('admin-content');
  content.innerHTML = '<div class="admin-loading">加载中…</div>';
  try {
    switch (tab) {
      case 'acl':
        content.innerHTML = await renderAclPanel();
        wireAclEvents();
        break;
      case 'groups':
        content.innerHTML = await renderGroupsPanel();
        wireGroupEvents();
        break;
      case 'users':
        content.innerHTML = await renderUsersPanel();
        wireUserEvents();
        break;
    }
  } catch (e) {
    content.innerHTML =
      `<div class="admin-empty">${iconSvg('alert-circle')} 加载失败: ${escapeHtml(e.message)}</div>`;
  }
}

// ── ACL panel ──

async function renderAclPanel() {
  aclData = await API.getAcl();
  const [users, groups] = await Promise.all([API.getUsers(), API.getGroups()]);
  setTabCount('acl', aclData.length);

  const rows = aclData.map((entry) => {
    const isUser = entry.user_id != null;
    const name = escapeHtml(entry.user_name || entry.group_name || '?');
    const icon = isUser ? iconSvg('user') : iconSvg('users');
    const type = isUser ? 'user' : 'group';
    return `<div class="admin-tr" data-acl-id="${entry.id}">
      <span class="acl-path" title="${escapeHtml(entry.path || '/')}">${iconSvg('folder')} ${escapeHtml(entry.path || '/')}</span>
      <span class="acl-target">${icon} ${name} <span class="acl-type">${type}</span></span>
      <span class="acl-perm perm ${escapeHtml(entry.permission)}">${escapeHtml(entry.permission)}</span>
      <span class="admin-actions">
        <button class="btn btn-sm" data-edit-acl="${entry.id}" title="编辑规则">${iconSvg('edit-2')} 编辑</button>
        <button class="btn btn-sm btn-danger" data-remove-acl="${entry.id}" title="删除规则">${iconSvg('trash-2')}</button>
      </span>
    </div>`;
  }).join('');

  // SVG doesn't render inside <option> labels, so the pickers use plain text.
  const userOpts = users.map((u) =>
    `<option value="user:${u.id}">${escapeHtml(u.display_name)}</option>`).join('');
  const groupOpts = groups.map((g) =>
    `<option value="group:${g.id}">${escapeHtml(g.name)}${g.name === 'default' ? ' (指分配给未分配群组的用户)' : g.name === 'guest' ? ' (未授权的访问者)' : ''}</option>`).join('');

  return `
    <div class="admin-toolbar">
      <input type="text" id="acl-search" class="admin-search" placeholder="搜索路径或目标…">
      <span class="admin-count" id="acl-count">${aclData.length} 条规则${aclData.length === 1 ? '' : ''}</span>
    </div>

    <form id="acl-form" class="admin-form" autocomplete="off">
      <input type="text" id="acl-path" placeholder="/path/to/share" required>
      <select id="acl-target">
        <option value="">— 选择目标 —</option>
        <optgroup label="用户">${userOpts}</optgroup>
        <optgroup label="群组">${groupOpts}</optgroup>
      </select>
      <select id="acl-permission">
        <option value="read">读取</option>
        <option value="write">写入</option>
        <option value="admin">管理员</option>
      </select>
      <button type="submit" class="btn btn-primary" id="acl-submit">${iconSvg('plus')} 添加规则</button>
      <button type="button" class="btn" id="acl-cancel-edit" style="display:none">取消</button>
    </form>

    <div class="admin-table">
      <div class="admin-th">
        <span>路径</span><span>目标</span><span>权限</span><span></span>
      </div>
      <div id="acl-rows">${rows || '<div class="admin-empty">暂无访问控制规则 — 请在上方添加一条。</div>'}</div>
    </div>`;
}

function wireAclEvents() {
  const form = document.getElementById('acl-form');
  const pathInput = document.getElementById('acl-path');
  const targetSelect = document.getElementById('acl-target');
  const permSelect = document.getElementById('acl-permission');
  const submit = document.getElementById('acl-submit');
  const cancelEdit = document.getElementById('acl-cancel-edit');

  function clearEdit() {
    aclEditingId = null;
    form.reset();
    form.classList.remove('editing');
    submit.innerHTML = `${iconSvg('plus')} 添加规则`;
    cancelEdit.style.display = 'none';
    document.querySelectorAll('.admin-tr.editing').forEach((r) => r.classList.remove('editing'));
  }

  function startEdit(entry) {
    aclEditingId = entry.id;
    pathInput.value = entry.path || '';
    targetSelect.value = entry.user_id != null ? `user:${entry.user_id}` : `group:${entry.group_id}`;
    permSelect.value = entry.permission;
    form.classList.add('editing');
    submit.innerHTML = `${iconSvg('check')} 保存`;
    cancelEdit.style.display = '';
    document.querySelectorAll('.admin-tr.editing').forEach((r) => r.classList.remove('editing'));
    const row = document.querySelector(`.admin-tr[data-acl-id="${entry.id}"]`);
    if (row) row.classList.add('editing');
  }

  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const path = pathInput.value.trim();
    const target = targetSelect.value;
    const perm = permSelect.value;
    if (!path || !target) { showToast('输入路径并选择目标', 'error'); return; }

    let userId = null, groupId = null;
    if (target.startsWith('user:')) userId = parseInt(target.slice(5), 10);
    else if (target.startsWith('group:')) groupId = parseInt(target.slice(6), 10);

    const editingId = aclEditingId;
    // Editing without changes: just leave edit mode.
    if (editingId != null) {
      const old = aclData.find((a) => a.id === editingId);
      if (old && old.path === path && old.user_id === userId && old.group_id === groupId && old.permission === perm) {
        clearEdit();
        showToast('无变化', 'info');
        return;
      }
    }

    try {
      // Create the new rule first, then drop the old one during an edit so a
      // failure never leaves the path unprotected.
      await API.setAcl(path, userId, groupId, perm);
      if (editingId != null) await API.removeAcl(editingId);
      showToast(editingId != null ? '规则已更新' : '规则已添加', 'success');
      await loadAdminPanel('acl');
    } catch (err) {
      showToast('错误: ' + err.message, 'error');
    }
  });

  cancelEdit.addEventListener('click', clearEdit);

  document.querySelectorAll('[data-edit-acl]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const entry = aclData.find((a) => a.id === parseInt(btn.dataset.editAcl, 10));
      if (entry) startEdit(entry);
    });
  });

  document.querySelectorAll('[data-remove-acl]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const id = parseInt(btn.dataset.removeAcl, 10);
      const entry = aclData.find((a) => a.id === id);
      const label = entry ? `${entry.path || '/'} → ${entry.user_name || entry.group_name}` : String(id);
      confirmDialog('删除访问控制规则', `删除规则 "${label}" 吗?\n该路径将不再通过此规则可访问。`, async () => {
        await API.removeAcl(id);
        showToast('规则已删除', 'success');
        await loadAdminPanel('acl');
      });
    });
  });

  wireSearch('acl-search', '.admin-tr');
}

// ── Groups panel ──

async function renderGroupsPanel() {
  groupData = await API.getGroups();
  setTabCount('groups', groupData.length);

  const cards = groupData.map((g) => {
    const isDefault = g.name === 'default';
    const isGuest = g.name === 'guest';
    const isReserved = isDefault || isGuest;
    const count = typeof g.member_count === 'number' ? g.member_count : 0;
    const sub = [
      `${count} 个成员${count === 1 ? '' : ''}`,
      isDefault ? '授予权限给没有分配群组的用户' : '',
      isGuest ? '授予权限给未授权的访问者' : '',
      g.description || '',
    ].filter(Boolean).join(' · ');
    return `<div class="admin-card">
      <div class="admin-card-main">
        <span class="admin-card-icon">${iconSvg('users')}</span>
        <div>
          <div class="admin-card-title">
            ${escapeHtml(g.name)}
            ${isDefault ? '<span class="admin-tag">default</span>' : ''}
            ${isGuest ? '<span class="admin-tag">guest</span>' : ''}
          </div>
          <div class="admin-card-sub">${escapeHtml(sub)}</div>
        </div>
      </div>
      <span class="admin-actions">
        <button class="btn btn-sm" data-manage-group="${g.id}" data-name="${escapeHtml(g.name)}"${isReserved ? ' disabled' : ''}>${iconSvg('user')} 管理</button>
        ${isReserved ? '' : `<button class="btn btn-sm btn-danger" data-delete-group="${g.id}" title="删除群组">${iconSvg('trash-2')}</button>`}
      </span>
    </div>`;
  }).join('');

  return `
    <div class="admin-toolbar">
      <input type="text" id="group-search" class="admin-search" placeholder="搜索群组…">
      <span class="admin-count" id="group-count">${groupData.length} 个群组${groupData.length === 1 ? '' : ''}</span>
    </div>

    <form id="group-form" class="admin-form" autocomplete="off">
      <input type="text" id="group-name" placeholder="群组名称" required>
      <input type="text" id="group-desc" placeholder="描述（可选）">
      <button type="submit" class="btn btn-primary">${iconSvg('plus')} 创建群组</button>
    </form>

    <div id="group-list">${cards || '<div class="admin-empty">暂无群组。</div>'}</div>`;
}

function wireGroupEvents() {
  document.getElementById('group-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const name = document.getElementById('group-name').value.trim();
    const desc = document.getElementById('group-desc').value.trim();
    if (!name) { showToast('输入群组名称', 'error'); return; }
    try {
      await API.createGroup(name, desc);
      showToast('群组已创建', 'success');
      await loadAdminPanel('groups');
    } catch (err) { showToast('Error: ' + err.message, 'error'); }
  });

  document.querySelectorAll('[data-delete-group]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const id = parseInt(btn.dataset.deleteGroup, 10);
      const g = groupData.find((x) => x.id === id);
      const name = g ? g.name : String(id);
      confirmDialog('删除群组', `删除群组 "${name}" 吗?成员保留他们的账户;仅丢弃此群组及其访问控制权限。`, async () => {
        await API.deleteGroup(id);
        showToast('群组已删除', 'success');
        await loadAdminPanel('groups');
      });
    });
  });

  document.querySelectorAll('[data-manage-group]').forEach((btn) => {
    btn.addEventListener('click', () => {
      showGroupManage(parseInt(btn.dataset.manageGroup, 10), btn.dataset.name);
    });
  });

  wireSearch('group-search', '.admin-card');
}

async function showGroupManage(groupId, groupName) {
  const isReserved = groupName === 'default' || groupName === 'guest';
  const [users, members] = await Promise.all([API.getUsers(), API.getGroupMembers(groupId)]);
  const memberIds = new Set(members.map((m) => m.id));

  const reservedNote = isReserved
    ? `<div class="member-summary" style="color:var(--error)">${iconSvg('lock')} \"${escapeHtml(groupName)}\" 的成员身份是自动管理的，不能形改。</div>`
    : `<div class="member-summary">此群组中有 ${members.length} 个(${users.length})个用户</div>`;

  const rows = users.slice()
    .sort((a, b) => (memberIds.has(b.id) - memberIds.has(a.id)) || a.display_name.localeCompare(b.display_name))
    .map((u) => {
      const isMember = memberIds.has(u.id);
      return `<div class="modal-user-row${isMember ? ' is-member' : ''}">
        <span>${iconSvg('user')} <strong>${escapeHtml(u.display_name)}</strong>
          ${isMember ? `<span class="member-tag">${iconSvg('check')} 成员</span>` : ''}
        </span>
        <button class="btn btn-sm ${isMember ? 'btn-danger' : 'btn-primary'}" ${isReserved ? 'disabled' : ''} data-toggle-membership="${u.id}" data-member="${isMember}">
          ${isMember ? '移除' : '+ 添加'}
        </button>
      </div>`;
    }).join('');

  const html = `${reservedNote}
    <input type="text" id="gm-search" class="admin-search" placeholder="搜索用户…" style="max-width:none;width:100%">
    <div class="member-list-wrap">${rows || '<div class="admin-empty">暂无用户</div>'}</div>`;

  showModal('管理群组', html, () => hideModal());

  wireSearch('gm-search', '.modal-user-row');

  document.querySelectorAll('[data-toggle-membership]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      if (isReserved) return;
      const userId = parseInt(btn.dataset.toggleMembership, 10);
      const isMember = btn.dataset.member === 'true';
      try {
        if (isMember) await API.removeUserFromGroup(userId, groupId);
        else await API.addUserToGroup(userId, groupId);
        showToast(isMember ? '已从群组中移除' : '已添加到群组', 'success');
        hideModal();
        await showGroupManage(groupId, groupName);
      } catch (e) { showToast('错误: ' + e.message, 'error'); }
    });
  });
}

// ── Users panel ──

async function renderUsersPanel() {
  userData = await API.getUsers();
  const groups = await API.getGroups();
  setTabCount('users', userData.length);

  // Build user → groups membership by reading each group's members.
  const memberships = {}; // userId -> [groupName, …]
  await Promise.all(groups.map(async (g) => {
    try {
      const members = await API.getGroupMembers(g.id);
      members.forEach((m) => {
        (memberships[m.id] = memberships[m.id] || []).push(g.name);
      });
    } catch (e) { /* ignore a failed group */ }
  }));

  const rows = userData.map((u) => {
    const initial = (u.display_name || '?').trim().charAt(0).toUpperCase() || '?';
    const myGroups = memberships[u.id] || [];
    const badges = myGroups.map((n) =>
      `<span class="group-badge">${escapeHtml(n)}</span>`).join('');
    return `<div class="admin-tr">
      <span class="user-main">
        <span class="avatar">${escapeHtml(initial)}</span>
        <span>
          <strong>${escapeHtml(u.display_name)}</strong>
          ${u.email ? `<span class="user-email">${escapeHtml(u.email)}</span>` : ''}
        </span>
      </span>
      <span class="user-groups">${badges || '<span style="color:var(--muted)">—</span>'}</span>
      <span class="user-role">${u.is_admin ? '<span class="admin-tag">管理员</span>' : '<span style="color:var(--muted)">成员</span>'}</span>
    </div>`;
  }).join('');

  return `
    <div class="admin-toolbar">
      <input type="text" id="user-search" class="admin-search" placeholder="搜索用户…">
      <span class="admin-count" id="user-count">${userData.length} 个用户${userData.length === 1 ? '' : ''}</span>
    </div>

    <div class="admin-table" id="user-table">
      <div class="admin-th"><span>用户</span><span>群组</span><span>角色</span></div>
      <div id="user-rows">${rows || '<div class="admin-empty">暂无用户</div>'}</div>
    </div>`;
}

function wireUserEvents() {
  wireSearch('user-search', '#user-rows .admin-tr');
}
