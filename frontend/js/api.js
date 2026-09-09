// API client for OneShare backend

// URL prefix the app is mounted under (set by the server-served config.js).
// Empty string means the domain root. Never has a trailing slash.
const ONESHARE_BASE = ((typeof window.ONESHARE_BASE === 'string' && window.ONESHARE_BASE) || '').replace(/\/+$/, '');

// ── Share mode（分享视图）──
//
// When the app is served from a public share link (`/s/{token}`), EVERY
// request — API, config, libfw /file//dir transfers — must stay under the
// share URL. The backend's ShareProxy swaps the URL token for the temporary
// identity's standard session cookie, so from here on a share visitor is
// just an ordinary (read-only) logged-in user: no share-specific endpoints,
// no special authorization.
const SHARE = (() => {
  let p = location.pathname;
  if (ONESHARE_BASE && p.startsWith(ONESHARE_BASE)) p = p.slice(ONESHARE_BASE.length);
  // Tokens are uuid-simple hex (32 chars); keep the pattern tight so the
  // normal app never mistakes itself for a share.
  const m = p.match(/^\/s\/([0-9a-f]{32})\/?$/);
  return m ? { token: m[1] } : null;
})();

const API_BASE = ONESHARE_BASE + (SHARE ? '/s/' + SHARE.token : '');
// libfw.js reads window.ONESHARE_BASE at load time (it runs AFTER this
// script), so override it here to prefix its /file and /dir URLs too.
if (SHARE) window.ONESHARE_BASE = API_BASE;

const API = {
  base: API_BASE,

  // Percent-encode each path segment separately so `/` stays a separator in the
  // URL (the server's `{*path}` capture then decodes each segment correctly).
  encodePath: (path) => String(path).split('/').map(encodeURIComponent).join('/'),

  async fetch(url, opts = {}) {
    const res = await fetch(this.base + url, { ...opts, credentials: 'same-origin' });
    if (!res.ok) {
      // No auto-redirect to the login page: unauthenticated visitors are
      // treated as guests with `guest`-group permissions, so the file APIs no
      // longer return 401 for them. A 401 here means a session is genuinely
      // invalid/expired — surface the error instead of bouncing the user.
      if (res.status === 401) {
        throw new Error('Not signed in');
      }
      if (res.status === 403) {
        throw new Error('Permission denied');
      }
      const txt = await res.text();
      const err = new Error(txt || `HTTP ${res.status}`);
      // Attach the HTTP status so callers (e.g. the file explorer) can react
      // to specific codes, like auto-falling back to the root on a 404.
      err.status = res.status;
      throw err;
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

  // ── libfw Token ──
  getToken: (path, op = 'read') => API.fetch(`/api/files/token?path=${encodeURIComponent(path)}&op=${op}`),

  // ── Share links（分享）──
  // items: one path for a single share, or several paths for a multi-item
  // "virtual root" collection (the receiver gets a landing view).
  createShare: (items, ttlSecs) => API.fetch('/api/files/share', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ items, ttl_secs: ttlSecs }) }),
  listShares: () => API.fetch('/api/files/shares'),
  revokeShare: (token) => API.fetch(`/api/files/share/${encodeURIComponent(token)}`, { method: 'DELETE' }),

  // ── File detail / inline preview / online edit ──
  // Metadata + (for small text files) full text content for preview & editing.
  getFileDetail: (path) => API.fetch(`/api/files/content?path=${encodeURIComponent(path)}`),
  // Save edited text content back to the server (requires write permission).
  saveFileContent: (path, content) => API.fetch('/api/files/content', { method: 'PUT', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ path, content }) }),
  // Inline binary preview URL (<img>/<video>/<audio>); auth via session cookie
  // (in share mode the temporary identity's session, injected by the proxy).
  rawUrl: (path) => `${API.base}/api/files/raw?path=${encodeURIComponent(path)}`,

  // ── Admin ──
  getUsers: () => API.fetch('/api/admin/users'),
  getGroups: () => API.fetch('/api/admin/groups'),
  createGroup: (name, description) => API.fetch('/api/admin/groups', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ name, description }) }),
  deleteGroup: (id) => API.fetch(`/api/admin/groups/${id}`, { method: 'DELETE' }),
  getGroupMembers: (id) => API.fetch(`/api/admin/groups/${id}/members`),
  addUserToGroup: (userId, groupId) => API.fetch('/api/admin/groups/add-user', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ user_id: userId, group_id: groupId }) }),
  removeUserFromGroup: (userId, groupId) => API.fetch('/api/admin/groups/remove-user', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ user_id: userId, group_id: groupId }) }),
  getAcl: () => API.fetch('/api/admin/acl'),
  setAcl: (path, userId, groupId, permission) => API.fetch('/api/admin/acl', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ path, user_id: userId, group_id: groupId, permission }) }),
  removeAcl: (id) => API.fetch(`/api/admin/acl/${id}`, { method: 'DELETE' }),

  // ── Audit log ──
  // Paged audit entries. `params`: { limit, offset, action, q }.
  getAudit: (params = {}) => {
    const qs = new URLSearchParams();
    if (params.limit != null) qs.set('limit', params.limit);
    if (params.offset != null) qs.set('offset', params.offset);
    if (params.action) qs.set('action', params.action);
    if (params.q) qs.set('q', params.q);
    return API.fetch(`/api/admin/audit?${qs.toString()}`);
  },
  clearAudit: () => API.fetch('/api/admin/audit', { method: 'DELETE' }),
};
