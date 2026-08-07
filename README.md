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

```bash
cargo run --release
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

ACL entries can target:
- A specific user (👤 user-level)
- A group (👥 group-level)

Entries are path-prefix-based: an ACL set for `/photos` applies to `/photos` and all subdirectories.

The first user to log in automatically becomes admin.

## API

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| GET | `/file/{*path}` | Bearer token | Download file (Range/ETag support) |
| POST | `/file/{*path}` | Bearer token | Upload file (streaming) |
| GET | `/dir/{*path}` | Bearer token | Directory listing (libfw) |
| GET | `/api/files/token?path=&op=` | Session | Issue a libfw bearer token |
