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
  okBtn.textContent = 'Confirm';
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
  const base = { acl: 'ACL', groups: 'Groups', users: 'Users' }[tab] || tab;
  btn.textContent = `${base} (${count})`;
}

async function loadAdminPanel(tab) {
  const content = document.getElementById('admin-content');
  content.innerHTML = '<div class="admin-loading">Loading…</div>';
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
      `<div class="admin-empty">${iconSvg('alert-circle')} Failed to load: ${escapeHtml(e.message)}</div>`;
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
        <button class="btn btn-sm" data-edit-acl="${entry.id}" title="Edit rule">${iconSvg('edit-2')} Edit</button>
        <button class="btn btn-sm btn-danger" data-remove-acl="${entry.id}" title="Delete rule">${iconSvg('trash-2')}</button>
      </span>
    </div>`;
  }).join('');

  // SVG doesn't render inside <option> labels, so the pickers use plain text.
  const userOpts = users.map((u) =>
    `<option value="user:${u.id}">${escapeHtml(u.display_name)}</option>`).join('');
  const groupOpts = groups.map((g) =>
    `<option value="group:${g.id}">${escapeHtml(g.name)}${g.name === 'default' ? ' (all unassigned users)' : g.name === 'guest' ? ' (unauthenticated visitors)' : ''}</option>`).join('');

  return `
    <div class="admin-toolbar">
      <input type="text" id="acl-search" class="admin-search" placeholder="Search path or target…">
      <span class="admin-count" id="acl-count">${aclData.length} rule${aclData.length === 1 ? '' : 's'}</span>
    </div>

    <form id="acl-form" class="admin-form" autocomplete="off">
      <input type="text" id="acl-path" placeholder="/path/to/share" required>
      <select id="acl-target">
        <option value="">— target —</option>
        <optgroup label="Users">${userOpts}</optgroup>
        <optgroup label="Groups">${groupOpts}</optgroup>
      </select>
      <select id="acl-permission">
        <option value="read">Read</option>
        <option value="write">Write</option>
        <option value="admin">Admin</option>
      </select>
      <button type="submit" class="btn btn-primary" id="acl-submit">${iconSvg('plus')} Add rule</button>
      <button type="button" class="btn" id="acl-cancel-edit" style="display:none">Cancel</button>
    </form>

    <div class="admin-table">
      <div class="admin-th">
        <span>Path</span><span>Target</span><span>Permission</span><span></span>
      </div>
      <div id="acl-rows">${rows || '<div class="admin-empty">No ACL rules yet — add one above.</div>'}</div>
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
    submit.innerHTML = `${iconSvg('plus')} Add rule`;
    cancelEdit.style.display = 'none';
    document.querySelectorAll('.admin-tr.editing').forEach((r) => r.classList.remove('editing'));
  }

  function startEdit(entry) {
    aclEditingId = entry.id;
    pathInput.value = entry.path || '';
    targetSelect.value = entry.user_id != null ? `user:${entry.user_id}` : `group:${entry.group_id}`;
    permSelect.value = entry.permission;
    form.classList.add('editing');
    submit.innerHTML = `${iconSvg('check')} Save`;
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
    if (!path || !target) { showToast('Enter a path and pick a target', 'error'); return; }

    let userId = null, groupId = null;
    if (target.startsWith('user:')) userId = parseInt(target.slice(5), 10);
    else if (target.startsWith('group:')) groupId = parseInt(target.slice(6), 10);

    const editingId = aclEditingId;
    // Editing without changes: just leave edit mode.
    if (editingId != null) {
      const old = aclData.find((a) => a.id === editingId);
      if (old && old.path === path && old.user_id === userId && old.group_id === groupId && old.permission === perm) {
        clearEdit();
        showToast('No changes', 'info');
        return;
      }
    }

    try {
      // Create the new rule first, then drop the old one during an edit so a
      // failure never leaves the path unprotected.
      await API.setAcl(path, userId, groupId, perm);
      if (editingId != null) await API.removeAcl(editingId);
      showToast(editingId != null ? 'Rule updated' : 'Rule added', 'success');
      await loadAdminPanel('acl');
    } catch (err) {
      showToast('Error: ' + err.message, 'error');
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
      confirmDialog('Delete ACL rule', `Delete the rule "${label}"?\nThe path will no longer be accessible through this rule.`, async () => {
        await API.removeAcl(id);
        showToast('Rule deleted', 'success');
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
      `${count} member${count === 1 ? '' : 's'}`,
      isDefault ? 'grants permissions to users in no group' : '',
      isGuest ? 'grants permissions to unauthenticated visitors' : '',
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
        <button class="btn btn-sm" data-manage-group="${g.id}" data-name="${escapeHtml(g.name)}"${isReserved ? ' disabled' : ''}>${iconSvg('user')} Manage</button>
        ${isReserved ? '' : `<button class="btn btn-sm btn-danger" data-delete-group="${g.id}" title="Delete group">${iconSvg('trash-2')}</button>`}
      </span>
    </div>`;
  }).join('');

  return `
    <div class="admin-toolbar">
      <input type="text" id="group-search" class="admin-search" placeholder="Search groups…">
      <span class="admin-count" id="group-count">${groupData.length} group${groupData.length === 1 ? '' : 's'}</span>
    </div>

    <form id="group-form" class="admin-form" autocomplete="off">
      <input type="text" id="group-name" placeholder="Group name" required>
      <input type="text" id="group-desc" placeholder="Description (optional)">
      <button type="submit" class="btn btn-primary">${iconSvg('plus')} Create group</button>
    </form>

    <div id="group-list">${cards || '<div class="admin-empty">No groups yet.</div>'}</div>`;
}

function wireGroupEvents() {
  document.getElementById('group-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const name = document.getElementById('group-name').value.trim();
    const desc = document.getElementById('group-desc').value.trim();
    if (!name) { showToast('Enter a group name', 'error'); return; }
    try {
      await API.createGroup(name, desc);
      showToast('Group created', 'success');
      await loadAdminPanel('groups');
    } catch (err) { showToast('Error: ' + err.message, 'error'); }
  });

  document.querySelectorAll('[data-delete-group]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const id = parseInt(btn.dataset.deleteGroup, 10);
      const g = groupData.find((x) => x.id === id);
      const name = g ? g.name : String(id);
      confirmDialog('Delete group', `Delete the group "${name}"? Members keep their accounts; only this group and its ACL grants are removed.`, async () => {
        await API.deleteGroup(id);
        showToast('Group deleted', 'success');
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
    ? `<div class="member-summary" style="color:var(--error)">${iconSvg('lock')} Membership of "${escapeHtml(groupName)}" is managed automatically and cannot be changed.</div>`
    : `<div class="member-summary">${members.length} of ${users.length} users are in this group</div>`;

  const rows = users.slice()
    .sort((a, b) => (memberIds.has(b.id) - memberIds.has(a.id)) || a.display_name.localeCompare(b.display_name))
    .map((u) => {
      const isMember = memberIds.has(u.id);
      return `<div class="modal-user-row${isMember ? ' is-member' : ''}">
        <span>${iconSvg('user')} <strong>${escapeHtml(u.display_name)}</strong>
          ${isMember ? `<span class="member-tag">${iconSvg('check')} member</span>` : ''}
        </span>
        <button class="btn btn-sm ${isMember ? 'btn-danger' : 'btn-primary'}" ${isReserved ? 'disabled' : ''} data-toggle-membership="${u.id}" data-member="${isMember}">
          ${isMember ? 'Remove' : '+ Add'}
        </button>
      </div>`;
    }).join('');

  const html = `${reservedNote}
    <input type="text" id="gm-search" class="admin-search" placeholder="Search users…" style="max-width:none;width:100%">
    <div class="member-list-wrap">${rows || '<div class="admin-empty">No users</div>'}</div>`;

  showModal('Manage Group', html, () => hideModal());

  wireSearch('gm-search', '.modal-user-row');

  document.querySelectorAll('[data-toggle-membership]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      if (isReserved) return;
      const userId = parseInt(btn.dataset.toggleMembership, 10);
      const isMember = btn.dataset.member === 'true';
      try {
        if (isMember) await API.removeUserFromGroup(userId, groupId);
        else await API.addUserToGroup(userId, groupId);
        showToast(isMember ? 'Removed from group' : 'Added to group', 'success');
        hideModal();
        await showGroupManage(groupId, groupName);
      } catch (e) { showToast('Error: ' + e.message, 'error'); }
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
      <span class="user-role">${u.is_admin ? '<span class="admin-tag">admin</span>' : '<span style="color:var(--muted)">member</span>'}</span>
    </div>`;
  }).join('');

  return `
    <div class="admin-toolbar">
      <input type="text" id="user-search" class="admin-search" placeholder="Search users…">
      <span class="admin-count" id="user-count">${userData.length} user${userData.length === 1 ? '' : 's'}</span>
    </div>

    <div class="admin-table" id="user-table">
      <div class="admin-th"><span>User</span><span>Groups</span><span>Role</span></div>
      <div id="user-rows">${rows || '<div class="admin-empty">No users</div>'}</div>
    </div>`;
}

function wireUserEvents() {
  wireSearch('user-search', '#user-rows .admin-tr');
}
