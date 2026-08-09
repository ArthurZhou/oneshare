# OneShare

A self-hosted web file sharing application — like alist but serving only local folders.

**Features:**
- 📂 Browse local directories with a responsive web UI
- ⬆⬇ Upload & download files via [libfw](https://github.com/ArthurZhou/libfw) streaming protocol
- ✏ Move, rename, delete, create folders
- 🔐 OIDC authentication (supports Authentik, Keycloak, Google, etc.)
- 🔒 ACL-based access control (per-path, per-user, per-group)
- 👥 User groups for simplified permission management
- ⚙ Admin panel for managing ACLs and groups

**Architecture:**
- Rust backend (Axum + libfw + SQLite + openidconnect)
- Vanilla JavaScript frontend (no framework)
- libfw for efficient large-file upload/download with resume & range support

## Quick Start

### 1. Configure

Copy the example config and fill in your values:

```bash
cp config.example.toml config.toml
# Edit config.toml with your OIDC provider details
```

### 2. Build and run

The frontend is vanilla JS in `frontend/`. Debug builds serve it straight from
disk (no build step); release builds minify it with Vite and embed the result
into the binary.

```bash
# Debug — serves ./frontend from disk, no frontend build needed
cargo run

# Release — Vite minifies the frontend into ../static, which is embedded
cd frontend && pnpm install && pnpm build
cd ..
cargo run --release
```

For a fully static musl binary (what the GitHub release workflow produces),
run the same frontend build above, then build with the musl target:

```bash
rustup target add x86_64-unknown-linux-musl   # once
sudo apt-get install -y musl-tools perl       # once (ring needs a C toolchain + perl)
CC=musl-gcc CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --target x86_64-unknown-linux-musl
```

### 3. Access

Open `http://localhost:3456` in your browser.

## Configuration

All settings are in `config.toml`:

| Section | Key | Default | Description |
|---------|-----|---------|-------------|
| `[server]` | `listen_addr` | `0.0.0.0` | Listen address |
| `[server]` | `listen_port` | `3456` | HTTP listen port |
| `[server]` | `root_dir` | `./data` | Root directory to serve |
| `[server]` | `database_url` | `oneshare.db` | SQLite database path |
| `[server]` | `session_cookie_key` | *(default)* | Cookie encryption key |
| `[server]` | `hmac_secret` | *(default)* | HMAC secret for libfw bearer tokens |
| `[server]` | `base_url` | `""` | URL prefix (base path) when serving behind a reverse proxy, e.g. `/oneshare`. Empty = domain root. |
| `[oidc]` | `issuer_url` | *(required)* | OIDC issuer URL |
| `[oidc]` | `client_id` | *(required)* | OIDC client ID |
| `[oidc]` | `client_secret` | *(required)* | OIDC client secret |
| `[oidc]` | `redirect_uri` | `http://localhost:3456/auth/callback` | OIDC callback URL |

## Serving behind a reverse proxy (URL prefix)

OneShare can be mounted under a sub-path of a shared domain so other apps can
live on the same hostname. Set `base_url` in `config.toml` and make sure the
reverse proxy forwards the **full** path (do **not** strip the prefix):

```toml
[server]
base_url = "/oneshare"

[oidc]
# The callback must include the prefix
redirect_uri = "https://example.com/oneshare/auth/callback"
```

Example nginx config:

```nginx
location /oneshare/ {
    proxy_pass http://127.0.0.1:3456;   # no trailing slash: keep the prefix
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

With `base_url` empty (or `/`) the app is served at the domain root, which is
the default behavior.

## ACL Model

- **Read**: Can list directories and download files
- **Write**: Can upload, create folders, rename, move, delete
- **Admin**: Full control (currently: all admin users bypass all ACLs)
- **Fail-closed**: a path with **no** matching ACL is not accessible to anyone —
  ACLs grant access, they never default to "readable".

ACL entries can target:
- A specific user (👤 user-level)
- A group (👥 group-level)

Entries are path-prefix-based: an ACL set for `/photos` applies to `/photos` and all subdirectories.

### The `default` group

Every user who has not been assigned to any explicit group is treated as a
member of the reserved **`default`** group. It lets admins grant base
permissions to all unassigned users with a single ACL (e.g. `/public → default
(read)`), and it cannot be deleted. Users with explicit group memberships use
exactly those groups and do *not* inherit `default`.

### The `guest` group

Every **unauthenticated** visitor (not logged in) is treated as a member of
the reserved **`guest`** group. It lets admins grant public read access with a
single ACL (e.g. `/public → guest (read)`), and it cannot be deleted. Guests
are *not* admins and never see the admin UI. The frontend no longer redirects
to the login page for logged-out visitors — they simply browse whatever the
`guest` group grants (a login link stays available in the header). The `guest`
group is separate from `default`: `default` covers logged-in users without
explicit groups, `guest` covers visitors with no session at all.

### Samba-style web root

The browser root (`/`) is a virtual share root, like Samba: it lists every
ACL-configured directory the user can read as a top-level folder named after
its **leaf** directory, hiding whatever lies above it. For example, with ACLs
on `/public (r)`, `/private (rw)` and `/nested/public2`, the user sees three
folders — `public`, `private`, `public2` — and inside `public2` the path shows
only `public2`, never `/nested`. The address bar reflects the current folder
(e.g. `#/public2/sub`).

Real filesystem paths are **never sent to the frontend** for non-admin users:
the server resolves virtual paths internally and ACL checks always run against
the real path. Admins are **not** affected by the virtual root — they browse
the actual filesystem tree so they can see the real paths they need to write
ACL entries for.

The first user to log in automatically becomes admin.

## API

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/file/{*path}` | Bearer token | Download file (Range/ETag support) |
| POST | `/file/{*path}` | Bearer token | Upload file (streaming) |
| GET | `/dir/{*path}` | Bearer token | Directory listing (libfw) |
| GET | `/api/files/token?path=&op=` | Session | Issue a libfw bearer token |
