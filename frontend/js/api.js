// API client for OneShare backend
const API = {
  base: '',

  async fetch(url, opts = {}) {
    const res = await fetch(this.base + url, { ...opts, credentials: 'same-origin' });
    if (!res.ok) {
      if (res.status === 401) {
        // Redirect to login
        window.location.href = '/auth/login';
        throw new Error('Unauthorized');
      }
      if (res.status === 403) {
        throw new Error('Permission denied');
      }
      const txt = await res.text();
      throw new Error(txt || `HTTP ${res.status}`);
    }
    const ct = res.headers.get('content-type') || '';
    if (ct.includes('application/json')) {
      return res.json();
    }
    return res.text();
  },

  // ── Auth ──
  getMe: () => API.fetch('/api/me'),

  // ── Files ──
  listFiles: (path = '') => API.fetch(`/api/files/list?path=${encodeURIComponent(path)}`),
  deleteFile: (path) => API.fetch('/api/files/delete', { method: 'DELETE', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ path }) }),
  renameFile: (path, newName) => API.fetch('/api/files/rename', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ path, new_name: newName }) }),
  moveFile: (source, destination) => API.fetch('/api/files/move', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ source, destination }) }),
  mkdir: (path, name) => API.fetch('/api/files/mkdir', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ path, name }) }),

  // ── WFW Token ──
  getWfwToken: (path) => API.fetch(`/api/files/wfw-token?path=${encodeURIComponent(path)}`),

  // ── Admin ──
  getUsers: () => API.fetch('/api/admin/users'),
  getGroups: () => API.fetch('/api/admin/groups'),
  createGroup: (name, description) => API.fetch('/api/admin/groups', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ name, description }) }),
  deleteGroup: (id) => API.fetch(`/api/admin/groups/${id}`, { method: 'DELETE' }),
  addUserToGroup: (userId, groupId) => API.fetch('/api/admin/groups/add-user', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ user_id: userId, group_id: groupId }) }),
  removeUserFromGroup: (userId, groupId) => API.fetch('/api/admin/groups/remove-user', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ user_id: userId, group_id: groupId }) }),
  getAcl: () => API.fetch('/api/admin/acl'),
  setAcl: (path, userId, groupId, permission) => API.fetch('/api/admin/acl', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ path, user_id: userId, group_id: groupId, permission }) }),
  removeAcl: (id) => API.fetch(`/api/admin/acl/${id}`, { method: 'DELETE' }),
};
