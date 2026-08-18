# OneShare

A self-hosted web file-sharing application — like alist, but it only serves the local folders you choose to share, and it keeps real filesystem paths out of the browser.

**Features**
- 📂 Responsive web UI to browse and manage local directories
- ⬆⬇ Fast uploads & downloads powered by [libfw](https://github.com/ArthurZhou/libfw) — HTTP streaming, byte-range downloads, upload resume, real (server-confirmed) progress
- ✏️ Move (drag & drop or menu), rename, delete, create folders
- 🗑️ Optional trash directory — deleted files/folders are moved there (relative path preserved) instead of being permanently deleted
- 🔐 OIDC authentication (Authentik, Keycloak, Google, Microsoft Entra ID, …) with ID-token signature verification (JWKS)
- 🔒 ACL-based access control — per path, per user, per group; fail-closed by default
- 👥 Groups for simplified permission management, plus reserved `default` and `guest` groups
- 🌐 Samba-style virtual web root — shared folders appear as top-level shares; real paths never reach non-admin users
- ⚙️ Admin panel to manage ACLs, groups and users
- 🔀 Optional URL prefix (`base_url`) to mount behind a reverse proxy on a shared domain

**Architecture**
- **Backend** — Rust: Axum, [libfw-server](https://github.com/ArthurZhou/libfw), SQLite (rusqlite), openidconnect
- **Frontend** — vanilla JavaScript (no framework), minified with Vite/esbuild
- **Transfers** — the libfw WASM SDK talks to the embedded libfw server over plain HTTP (`/file`, `/dir`) with parallel byte-range downloads and chunked uploads

---

## Quick start

### 1. Configure

```bash
cp config.example.toml config.toml
```

Edit `config.toml`:
- `[oidc]` — your OIDC provider's `issuer_url`, `client_id`, `client_secret`, `redirect_uri`.
- `[server] hmac_secret` — a strong random value (**≥ 16 bytes**; the server refuses to start otherwise).
- `[libfw] path_key` — a 64-hex AES-256 key: `openssl rand -hex 32`.

### 2. Build and run

The frontend is vanilla JS in `frontend/`. Debug builds serve it straight from disk; release builds minify it into `../static` and embed it into the binary. `pnpm build` also re-bundles the libfw WASM SDK into `frontend/vendor/` (served from disk in debug), so run it once after `pnpm install`.

```bash
# One-time frontend build (minifies JS/CSS into ../static AND generates
# frontend/vendor/ — the libfw SDK served from disk in debug)
cd frontend && pnpm install && pnpm build
cd ..

# Debug — serves ./frontend from disk
cargo run

# Release — embeds the minified frontend into the binary
cd frontend && pnpm build
cd ..
cargo run --release
```

Fully static musl binary (what the GitHub release workflow produces):

```bash
rustup target add x86_64-unknown-linux-musl            # once
sudo apt-get install -y musl-tools perl                # once (ring needs a C toolchain + perl)
CC=musl-gcc CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --target x86_64-unknown-linux-musl
```

### 3. Access

Open `http://localhost:3456`. **The first user to log in becomes admin automatically.**

---

## Configuration

All settings live in `config.toml`. `config.example.toml` is a fully commented template.

### `[server]`

| Key | Default | Description |
|-----|---------|-------------|
| `listen_addr` | `0.0.0.0` | Listen address |
| `listen_port` | `3456` | HTTP port |
| `root_dir` | `./data` | Root directory to serve |
| `database_url` | `oneshare.db` | SQLite database path |
| `hmac_secret` | *(required)* | Signs libfw bearer tokens; must be ≥ 16 bytes, not the default |
| `base_url` | `""` | URL prefix behind a reverse proxy, e.g. `/oneshare`; empty = domain root |
| `session_cookie_secure` | `false` | Mark the session cookie `Secure` (set `true` when serving over HTTPS) |
| `allowed_origins` | `[]` | CORS allowlist; empty = same-origin only (recommended) |
| `trash_dir` | `""` | When set, deleted files/folders are **moved** here (relative path preserved) instead of permanently deleted; empty = permanent delete |

### `[oidc]`

| Key | Default | Description |
|-----|---------|-------------|
| `issuer_url` | *(required)* | OIDC issuer URL |
| `client_id` / `client_secret` | *(required)* | OIDC client credentials |
| `redirect_uri` | `http://localhost:3456/auth/callback` | Callback URL (include `base_url` when set) |
| `authorization_endpoint` / `token_endpoint` / `userinfo_endpoint` / `jwks_uri` | *(auto)* | Optional endpoint overrides; when **all** are set, `.well-known` discovery is skipped (offline start) and `jwks_uri` is used for ID-token verification |

### `[libfw]`

| Key | Default | Description |
|-----|---------|-------------|
| `path_key` | *(required)* | 64-hex AES-256 key encrypting paths into opaque shadows (`openssl rand -hex 32`) |
| `compression` | `"none"` | Download compression: `none` or `zrip` |
| `max_upload_size` | `107374182400` | Upper bound for a single upload body (bytes) |
| `concurrency` | `4` | Parallel transfer requests |
| `chunk_size` | `2097152` | Upload chunk and download byte-range size |
| `upload_window` / `download_window` | `4` | Per-file in-flight chunks (progress granularity vs. throughput) |
| `max_retries` / `base_retry_delay_ms` / `max_retry_delay_ms` | `3` / `500` / `30000` | Retry policy |
| `timeout_ms` | `600000` | Per-read idle timeout — keep generous; aborts a transfer if any single read stalls |
| `auto_tune` / `tune_ttl_ms` | `false` / `3600000` | Adaptive client tuning via `/capabilities` |

---

## Reverse proxy & URL prefix

To mount OneShare under a sub-path of a shared domain, set `base_url` and make the proxy forward the **full** path (do **not** strip the prefix):

```toml
[server]
base_url = "/oneshare"

[oidc]
redirect_uri = "https://example.com/oneshare/auth/callback"
```

```nginx
location /oneshare/ {
    proxy_pass http://127.0.0.1:3456;     # no trailing slash: keep the prefix
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
    # Large file uploads/downloads
    client_max_body_size 0;
    proxy_request_buffering off;
    proxy_buffering off;
}
```

With `base_url` empty (or `/`) the app is served at the domain root.

---

## Authentication

OneShare uses **OIDC** (OpenID Connect) for login. On every login the server verifies the ID token's signature (RS256 via the provider's JWKS) plus `iss`/`aud`/`exp`/`nonce`. Users are created on first login.

- **Role → group auto-join**: if the provider's `userinfo` carries a `role` claim whose value matches an existing group name, the user is automatically added to that group (never removed).
- **The first user to log in becomes admin.**

---

## ACL model

- **Read** — list directories and download files
- **Write** — upload, create folders, rename, move, delete
- **Admin** — full control; admin users bypass all ACLs
- **Fail-closed** — a path with **no** matching ACL is accessible to nobody. ACLs grant access; they never default to "readable".

ACL entries can target a specific user or a group, and are **path-prefix** based: an entry for `/photos` also applies to every subdirectory.

### The `default` group

Every user who has not been assigned to any explicit group is treated as a member of the reserved **`default`** group — a single ACL (e.g. `/public → default (read)`) grants base access to all unassigned users. Users with explicit group memberships use exactly those groups and do **not** inherit `default`. It cannot be deleted.

### The `guest` group

Every **unauthenticated** visitor is treated as a member of the reserved **`guest`** group — a single ACL (e.g. `/public → guest (read)`) makes that folder public. Guests are never admins and never see the admin UI; logged-out visitors simply browse whatever `guest` grants (a login link stays in the header). It cannot be deleted.

### Samba-style web root

For non-admin users the browser root (`/`) is a **virtual share root**: every ACL-configured directory the user can read appears as a top-level folder named after its **leaf** directory. ACLs on `/public (r)`, `/private (rw)` and `/nested/public2` show up as three folders — `public`, `private`, `public2` — with the parts above each share hidden. The address bar reflects the current folder (`#/public2/sub`).

Real filesystem paths are **never sent to the frontend** for non-admin users: the server resolves virtual paths internally and ACL checks always run against the real path. Admins are **not** affected by the virtual root — they browse the real filesystem tree so they can configure ACLs on real paths.

---

## API

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/api/me` | Session | Current user info (`{"user": null}` for guests) |
| GET | `/api/files/list?path=` | Session | Directory listing |
| GET | `/api/files/token?path=&op=` | Session | Issue a libfw bearer token (`op=read`\|`write`) |
| GET | `/api/files/names?paths=` | Session | Resolve opaque shadow paths to display names |
| DELETE | `/api/files/delete?path=` | Session | Delete (or move to trash) |
| PUT | `/api/files/rename?path=&new_name=` | Session | Rename |
| PUT | `/api/files/move?source=&destination=` | Session | Move |
| POST | `/api/files/mkdir?path=&name=` | Session | Create folder |
| GET | `/api/admin/users` | Admin | List users |
| GET/POST | `/api/admin/groups` | Admin | List / create groups |
| DELETE | `/api/admin/groups/{id}` | Admin | Delete group |
| GET | `/api/admin/groups/{id}/members` | Admin | List group members |
| POST | `/api/admin/groups/add-user` / `remove-user` | Admin | Manage members |
| GET/POST/DELETE | `/api/admin/acl` | Admin | Manage ACLs |
| GET | `/config.js` | Public | Frontend bootstrap config (base URL, libfw options, version) |
| GET | `/file/{*path}` | Bearer token | Download (libfw; Range/ETag) |
| POST | `/file/{*path}` | Bearer token | Upload (libfw) |
| GET | `/dir/{*path}` | Bearer token | Directory listing (libfw) |
| GET | `/capabilities` | Public | libfw capability advertisement (auto-tune) |
| GET | `/auth/login` / `/auth/callback` · POST `/auth/logout` | — | OIDC flow |

---

## Development

- **Build the frontend:** `cd frontend && pnpm build` — minifies `js/*` + `css/` into `../static` and re-bundles the libfw SDK into `static/vendor/`.
- **Run tests:** `cargo test`.
- **Release build:** `cargo build --release` — embeds the minified frontend; static asset URLs are cache-busted with a content-hash `?v=` and served `immutable`.

---

## Documentation

- [README.zh-CN.md](README.zh-CN.md) — 中文版说明
- [docs/README.md](docs/README.md) — 中文文档索引
- [docs/configuration.md](docs/configuration.md) — 配置详解
- [docs/deployment.md](docs/deployment.md) — 部署与反向代理
- [docs/acl.md](docs/acl.md) — 权限模型
- [docs/development.md](docs/development.md) — 开发与构建
