# OneShare

A self-hosted web file sharing application — like alist but serving only local folders.

**Features:**
- 📂 Browse local directories with a responsive web UI
- ⬆⬇ Upload & download files via [wfw](https://crates.io/crates/wfw-server) streaming protocol
- ✏ Move, rename, delete, create folders
- 🔐 OIDC authentication (supports Authentik, Keycloak, Google, etc.)
- 🔒 ACL-based access control (per-path, per-user, per-group)
- 👥 User groups for simplified permission management
- ⚙ Admin panel for managing ACLs and groups

**Architecture:**
- Rust backend (Axum + wfw-server + SQLite + openidconnect)
- Vanilla JavaScript frontend (no framework)
- wfw for efficient large-file upload/download with resume support

## Quick Start

### 1. Configure

Copy the example config and fill in your values:

```bash
cp config.example.toml config.toml
# Edit config.toml with your OIDC provider details
```

### 2. Build and run

```bash
cd backend
cargo build --release
mkdir -p ../data
../target/release/oneshare-backend
```

### 3. Access

Open `http://localhost:3456` in your browser.

## Configuration

All settings are in `config.toml`:

| Section | Key | Default | Description |
|---------|-----|---------|-------------|
| `[server]` | `listen_addr` | `0.0.0.0` | Listen address |
| `[server]` | `listen_port` | `3456` | Main web UI port (wfw uses port+1) |
| `[server]` | `root_dir` | `./data` | Root directory to serve |
| `[server]` | `database_url` | `oneshare.db` | SQLite database path |
| `[server]` | `session_cookie_key` | *(default)* | Cookie encryption key |
| `[server]` | `hmac_secret` | *(default)* | HMAC secret for wfw download tokens |
| `[oidc]` | `issuer_url` | *(required)* | OIDC issuer URL |
| `[oidc]` | `client_id` | *(required)* | OIDC client ID |
| `[oidc]` | `client_secret` | *(required)* | OIDC client secret |
| `[oidc]` | `redirect_uri` | `http://localhost:3456/auth/callback` | OIDC callback URL |

## ACL Model

- **Read**: Can list directories and download files
- **Write**: Can upload, create folders, rename, move, delete
- **Admin**: Full control (currently: all admin users bypass all ACLs)

ACL entries can target:
- A specific user (👤 user-level)
- A group (👥 group-level)

Entries are path-prefix-based: an ACL set for `/photos` applies to `/photos` and all subdirectories.

The first user to log in automatically becomes admin.

## Ports

- **Port 3456**: Web UI + API
- **Port 3457**: wfw file transfer (internal, managed automatically)
