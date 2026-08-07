// Admin Panel Management

async function loadAdminPanel(tab) {
  const content = document.getElementById('admin-content');
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
    content.innerHTML = `<div class="empty">Error: ${escapeHtml(e.message)}</div>`;
  }
}

// ── ACL Panel ──

async function renderAclPanel() {
  const [acl, users, groups] = await Promise.all([
    API.getAcl(), API.getUsers(), API.getGroups()
  ]);

  // ACL entry rows
  let rows = '';
  acl.forEach(entry => {
    const target = entry.user_name || entry.group_name || '?';
    const icon = entry.user_id ? '👤' : '👥';
    rows += `<div class="admin-acl-row">
      <div>📂 ${escapeHtml(entry.path)} &nbsp; ${icon} ${escapeHtml(target)} &nbsp; <span class="perm">${entry.permission}</span></div>
      <button class="btn btn-sm btn-danger" data-remove-acl="${entry.id}">×</button>
    </div>`;
  });

  const userOpts = users.map(u => `<option value="user:${u.id}">👤 ${escapeHtml(u.display_name)}</option>`).join('');
  const groupOpts = groups.map(g => `<option value="group:${g.id}">👥 ${escapeHtml(g.name)}</option>`).join('');

  return `
    <div class="admin-acl-add">
      <input type="text" id="acl-path" placeholder="/path" style="flex:1;min-width:120px">
      <select id="acl-target">
        <option value="">-- User/Group --</option>
        ${userOpts}
        ${groupOpts}
      </select>
      <select id="acl-permission">
        <option value="read">Read</option>
        <option value="write">Write</option>
        <option value="admin">Admin</option>
      </select>
      <button class="btn btn-primary btn-sm" id="acl-add-btn">Add</button>
    </div>
    ${rows || '<div class="empty">No ACL entries</div>'}
  `;
}

function wireAclEvents() {
  document.getElementById('acl-add-btn')?.addEventListener('click', async () => {
    const path = document.getElementById('acl-path').value.trim();
    const target = document.getElementById('acl-target').value;
    const perm = document.getElementById('acl-permission').value;
    if (!path || !target) return alert('Fill in all fields');

    let userId = null, groupId = null;
    if (target.startsWith('user:')) userId = parseInt(target.slice(5));
    else if (target.startsWith('group:')) groupId = parseInt(target.slice(6));

    try {
      await API.setAcl(path, userId, groupId, perm);
      loadAdminPanel('acl');
    } catch (e) { alert('Error: ' + e.message); }
  });

  document.querySelectorAll('[data-remove-acl]').forEach(btn => {
    btn.addEventListener('click', async () => {
      try {
        await API.removeAcl(parseInt(btn.dataset.removeAcl));
        loadAdminPanel('acl');
      } catch (e) { alert('Error: ' + e.message); }
    });
  });
}

// ── Groups Panel ──

async function renderGroupsPanel() {
  const [groups, users] = await Promise.all([API.getGroups(), API.getUsers()]);

  let rows = '';
  groups.forEach(g => {
    rows += `<div class="admin-group-row">
      <div>👥 <strong>${escapeHtml(g.name)}</strong> ${g.description ? `— ${escapeHtml(g.description)}` : ''}</div>
      <div>
        <button class="btn btn-sm" data-manage-group="${g.id}" data-name="${escapeHtml(g.name)}">👤 Manage</button>
        <button class="btn btn-sm btn-danger" data-delete-group="${g.id}">×</button>
      </div>
    </div>`;
  });

  return `
    <div class="admin-form-row">
      <input type="text" id="group-name" placeholder="Group name" style="flex:1">
      <input type="text" id="group-desc" placeholder="Description" style="flex:1">
      <button class="btn btn-primary btn-sm" id="group-create">Create</button>
    </div>
    ${rows || '<div class="empty">No groups</div>'}
  `;
}

function wireGroupEvents() {
  document.getElementById('group-create')?.addEventListener('click', async () => {
    const name = document.getElementById('group-name').value.trim();
    const desc = document.getElementById('group-desc').value.trim();
    if (!name) return alert('Enter a group name');
    try {
      await API.createGroup(name, desc);
      loadAdminPanel('groups');
    } catch (e) { alert('Error: ' + e.message); }
  });

  document.querySelectorAll('[data-delete-group]').forEach(btn => {
    btn.addEventListener('click', async () => {
      if (!confirm('Delete this group?')) return;
      try {
        await API.deleteGroup(parseInt(btn.dataset.deleteGroup));
        loadAdminPanel('groups');
      } catch (e) { alert('Error: ' + e.message); }
    });
  });

  document.querySelectorAll('[data-manage-group]').forEach(btn => {
    btn.addEventListener('click', async () => {
      const gid = parseInt(btn.dataset.manageGroup);
      const gname = btn.dataset.name;
      await showGroupManage(gid, gname);
    });
  });
}

async function showGroupManage(groupId, groupName) {
  const users = await API.getUsers();
  // Show modal with user list and add/remove buttons
  let html = `<h3 style="margin-bottom:12px">Manage: ${escapeHtml(groupName)}</h3>`;
  html += '<div style="max-height:300px;overflow-y:auto">';
  users.forEach(u => {
    html += `<div class="modal-user-row">
      <span>👤 ${escapeHtml(u.display_name)}</span>
      <div>
        <button class="btn btn-sm" data-add-user="${u.id}" data-gid="${groupId}">+ Add</button>
        <button class="btn btn-sm btn-danger" data-remove-user="${u.id}" data-gid="${groupId}">- Remove</button>
      </div>
    </div>`;
  });
  html += '</div>';

  showModal(`Manage Group`, html, () => hideModal());

  // Wire buttons
  setTimeout(() => {
    document.querySelectorAll('[data-add-user]').forEach(btn => {
      btn.onclick = async () => {
        try {
          await API.addUserToGroup(parseInt(btn.dataset.addUser), parseInt(btn.dataset.gid));
          hideModal();
          showGroupManage(groupId, groupName);
        } catch (e) { alert('Error: ' + e.message); }
      };
    });
    document.querySelectorAll('[data-remove-user]').forEach(btn => {
      btn.onclick = async () => {
        try {
          await API.removeUserFromGroup(parseInt(btn.dataset.removeUser), parseInt(btn.dataset.gid));
          hideModal();
          showGroupManage(groupId, groupName);
        } catch (e) { alert('Error: ' + e.message); }
      };
    });
  }, 100);
}

// ── Users Panel ──

async function renderUsersPanel() {
  const users = await API.getUsers();
  let rows = users.map(u => `
    <div class="admin-user-row">
      <div>👤 <strong>${escapeHtml(u.display_name)}</strong> ${u.email ? `(${escapeHtml(u.email)})` : ''} ${u.is_admin ? '<span class="admin-tag">[admin]</span>' : ''}</div>
      <div style="color:var(--muted);font-size:0.8em">ID: ${u.id}</div>
    </div>
  `).join('');
  return rows || '<div class="empty">No users</div>';
}

function wireUserEvents() {
  // Currently read-only; could add user role management
}
