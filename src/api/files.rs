use crate::acl::{self, Permission};
use crate::auth::session::get_request_user;
use crate::db::{AclEntryRow, UserRow};
use crate::libtoken::issue_token;
use crate::models::*;
use crate::AppState;
use axum::{
    extract::{Json, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::cookie::CookieJar;
use std::sync::Arc;

/// Whether the user sees the real filesystem tree (admins, and any user with a
/// root ACL), i.e. their display paths ARE the real paths and must not be
/// translated through the virtual share root.
pub fn sees_real_tree(
    user: &UserRow,
    user_groups: &[i64],
    acl_entries: &[AclEntryRow],
) -> bool {
    user.is_admin == 1 || acl::user_has_root_read(user, user_groups, acl_entries)
}

/// Resolve a path the frontend sent into the real path under `root_dir`.
///
/// - **Admin / root-ACL holder**: the requested path IS the real path (they see
///   the real tree, so they can configure ACLs accurately, unaffected by the
///   virtual root).
/// - **Non-admin**: the requested path is VIRTUAL; it is mapped through the
///   user's shares. `None` means the path is not a share the user can reach.
fn resolve_for_user(
    user: &UserRow,
    user_groups: &[i64],
    acl_entries: &[AclEntryRow],
    path: &str,
) -> Option<String> {
    let p = path.trim_start_matches('/');
    let real = if sees_real_tree(user, user_groups, acl_entries) {
        Some(p.to_string())
    } else {
        let shares = acl::user_shares(user, user_groups, acl_entries);
        acl::resolve_virtual(p, &shares)
    }?;
    // Sanitize the resolved real path (rejects `..`, `\`, NUL) so a low-priv
    // user can never escape their share via path traversal.
    acl::sanitize_path(&real)
}

/// Build a [`FileEntry`] for a real directory entry under `real_dir`, mapping
/// it to the path the user sees. Returns the entry (with the display `path`)
/// and its real path (used only for ACL filtering, never sent to the frontend).
fn build_entry(
    root: &std::path::Path,
    name: &str,
    display_dir: &str,
    real_dir: &str,
) -> Option<(FileEntry, String)> {
    if name.starts_with('.') {
        // Skip hidden files (and libfw's `.libfw-tmp-*` leftovers).
        return None;
    }
    let full = root.join(real_dir).join(name);
    let metadata = std::fs::metadata(&full).ok()?;
    let is_dir = metadata.is_dir();
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| {
            chrono::DateTime::from_timestamp(
                t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64,
                0,
            )
        })
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "".to_string());

    let mime = if is_dir {
        "inode/directory".to_string()
    } else {
        mime_guess::from_path(&name)
            .first_or_octet_stream()
            .to_string()
    };

    let entry_display = if display_dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", display_dir, name)
    };
    let entry_real = if real_dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", real_dir, name)
    };

    Some((
        FileEntry {
            name: name.to_string(),
            path: entry_display,
            is_dir,
            size: metadata.len(),
            modified,
            mime_type: mime,
        },
        entry_real,
    ))
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(query): Query<ListQuery>,
) -> Result<Json<DirListing>, StatusCode> {
    // Unauthenticated requests are treated as the synthetic guest user, whose
    // permissions come from the reserved `guest` group (no 401 redirect).
    let ru = get_request_user(&jar, &state.db).await?;
    let user = &ru.user;
    let user_groups = &ru.groups;

    let requested = query.path.unwrap_or_else(|| "".to_string());
    let requested = requested.trim_start_matches('/').to_string();

    // Fetch the ACL context once and reuse it for the directory itself and
    // every entry, so listings hide anything the user cannot read.
    let acl_entries = state
        .db
        .list_acl_entries()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let root = state.config.root_dir().clone();
    let at_root = requested.is_empty();

    // The real path of the directory to read ("" = the real root).
    let mut real_path = String::new();
    let mut show_share_root = false;

    if sees_real_tree(&user, &user_groups, &acl_entries) {
        // Admins (and root-ACL holders) are NOT affected by the virtual root:
        // they browse the real filesystem tree so they can see real paths while
        // configuring ACLs. Still sanitize to reject `..`/`\` in the request.
        real_path = acl::sanitize_path(&requested).ok_or(StatusCode::BAD_REQUEST)?;
    } else if at_root {
        show_share_root = true;
    } else {
        real_path =
            acl::resolve_virtual(&requested, &acl::user_shares(&user, &user_groups, &acl_entries))
                .ok_or(StatusCode::NOT_FOUND)?;
    }

    // List entries as (FileEntry, real_path) pairs so ACL filtering happens on
    // real paths while the response only carries the display path.
    let mut raw: Vec<(FileEntry, String)> = Vec::new();

    if show_share_root {
        // Virtual share root: each share is a top-level virtual directory.
        for share in acl::user_shares(&user, &user_groups, &acl_entries) {
            let full = root.join(&share.real_path);
            if !full.is_dir() {
                continue;
            }
            let modified = std::fs::metadata(&full)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| {
                    chrono::DateTime::from_timestamp(
                        t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64,
                        0,
                    )
                })
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            raw.push((
                FileEntry {
                    name: share.virtual_name.clone(),
                    path: share.virtual_name.clone(),
                    is_dir: true,
                    size: 0,
                    modified,
                    mime_type: "inode/directory".to_string(),
                },
                share.real_path.clone(),
            ));
        }
    } else {
        // ACL gate on the REAL path (never on a shadow/virtual path).
        if !acl::can_access(&user, &user_groups, &acl_entries, &real_path, &acl::Permission::Read) {
            return Err(StatusCode::FORBIDDEN);
        }

        let full_path = root.join(&real_path);
        if !full_path.exists() {
            return Err(StatusCode::NOT_FOUND);
        }
        if let Ok(read_dir) = std::fs::read_dir(&full_path) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(fe) = build_entry(&root, &name, &requested, &real_path) {
                    raw.push(fe);
                }
            }
        }
    }

    // Only show entries the user can read on the real path; inaccessible
    // files/dirs are hidden.
    raw.retain(|(_, real)| {
        acl::can_access(&user, &user_groups, &acl_entries, real, &acl::Permission::Read)
    });

    let mut entries: Vec<FileEntry> = raw.into_iter().map(|(fe, _)| fe).collect();
    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir) // directories first
        } else {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        }
    });

    let parent_path = if requested.is_empty() {
        None
    } else {
        let mut parts: Vec<&str> = requested.split('/').collect();
        parts.pop();
        if parts.is_empty() {
            Some("".to_string())
        } else {
            Some(parts.join("/"))
        }
    };

    // Whether the current directory allows writes. The share root is a virtual
    // folder with no real target, so it is read-only.
    let writable = !show_share_root
        && acl::can_access(&user, &user_groups, &acl_entries, &real_path, &acl::Permission::Write);

    Ok(Json(DirListing {
        current_path: requested,
        parent_path,
        is_share_root: show_share_root,
        writable,
        entries,
    }))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<FileOperation>,
) -> Result<impl IntoResponse, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;

    let real = resolve_checked(&state, &ru.user, &ru.groups, &body.path, Permission::Write).await?;
    let full_path = state.config.root_dir().join(&real);
    if !full_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    if full_path.is_dir() {
        std::fs::remove_dir_all(&full_path).map_err(|e| {
            tracing::error!("Failed to delete dir: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    } else {
        std::fs::remove_file(&full_path).map_err(|e| {
            tracing::error!("Failed to delete file: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    Ok(StatusCode::OK)
}

pub async fn rename(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<RenameRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;

    let real = resolve_checked(&state, &ru.user, &ru.groups, &body.path, Permission::Write).await?;
    let old_full = state.config.root_dir().join(&real);
    if !old_full.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Get parent directory
    let parent = old_full.parent().unwrap_or(state.config.root_dir());
    let new_full = parent.join(&body.new_name);

    // Sanity check on new name: no path separators, no `.`/`..` (which would
    // be a traversal escape), no backslashes (Windows separator) or NUL bytes.
    // Note: `contains("..")` is intentionally NOT used — a name like `a..b`
    // is perfectly valid; only an exact `.`/`..` must be rejected.
    if body.new_name.is_empty()
        || body.new_name.contains('/')
        || body.new_name.contains('\\')
        || body.new_name.contains('\0')
        || body.new_name == "."
        || body.new_name == ".."
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Refuse to silently overwrite an existing target (data-loss guard).
    if new_full.exists() && new_full != old_full {
        return Err(StatusCode::CONFLICT);
    }

    std::fs::rename(&old_full, &new_full).map_err(|e| {
        tracing::error!("Failed to rename: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::OK)
}

pub async fn mv(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<MoveRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;

    let src_real = resolve_checked(&state, &ru.user, &ru.groups, &body.source, Permission::Write).await?;
    let dst_real = resolve_checked(&state, &ru.user, &ru.groups, &body.destination, Permission::Write).await?;

    let src_full = state.config.root_dir().join(&src_real);
    let dst_full = state.config.root_dir().join(&dst_real);

    if !src_full.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Guard against destructive/pointless moves: same path, moving a directory
    // into itself, or silently overwriting an existing destination.
    if src_real == dst_real {
        return Err(StatusCode::BAD_REQUEST);
    }
    if dst_full.starts_with(&src_full) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if dst_full.exists() {
        return Err(StatusCode::CONFLICT);
    }

    std::fs::rename(&src_full, &dst_full).map_err(|e| {
        tracing::error!("Failed to move: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::OK)
}

pub async fn mkdir(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<MkdirRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;

    if body.name.is_empty()
        || body.name.contains('/')
        || body.name.contains('\\')
        || body.name.contains('\0')
        || body.name == "."
        || body.name == ".."
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Admin: path is real. Non-admin: path is the virtual parent directory
    // (e.g. `public2` → real `nested/public2`); the new folder is created
    // inside the resolved real parent.
    let real = resolve_checked(&state, &ru.user, &ru.groups, &body.path, Permission::Write).await?;
    let base = if real.is_empty() {
        state.config.root_dir().clone()
    } else {
        state.config.root_dir().join(&real)
    };
    let new_dir = base.join(&body.name);

    std::fs::create_dir_all(&new_dir).map_err(|e| {
        tracing::error!("Failed to mkdir: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::CREATED)
}

/// Resolve a frontend-supplied path to a real path and check `required`
/// permission on it. Returns 403/404 for paths outside the user's access.
async fn resolve_checked(
    state: &Arc<AppState>,
    user: &UserRow,
    user_groups: &[i64],
    path: &str,
    required: Permission,
) -> Result<String, StatusCode> {
    let acl_entries = state
        .db
        .list_acl_entries()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let real = resolve_for_user(user, user_groups, &acl_entries, path)
        .ok_or(StatusCode::FORBIDDEN)?;

    acl::can_access(user, user_groups, &acl_entries, &real, &required)
        .then_some(real)
        .ok_or(StatusCode::FORBIDDEN)
}

/// Issue a libfw bearer token for file upload/download via libfw endpoints.
#[derive(serde::Deserialize)]
pub struct TokenQuery {
    pub path: String,
    /// "read" (download) or "write" (upload). Defaults to "read".
    #[serde(default = "default_op")]
    pub op: String,
}

fn default_op() -> String {
    "read".to_string()
}

pub async fn get_token(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(query): Query<TokenQuery>,
) -> Result<Json<TokenResponse>, StatusCode> {
    // Guests (unauthenticated) can get read tokens for anything the `guest`
    // group can read, which is what powers downloads without a session.
    let ru = get_request_user(&jar, &state.db).await?;

    // The token is bound to the REAL path. The frontend only ever supplies a
    // virtual path (non-admin), which we resolve here — the real path travels
    // inside the opaque signed token, never in an API response.
    let real_path = resolve_checked(&state, &ru.user, &ru.groups, &query.path, {
        match query.op.as_str() {
            "write" => Permission::Write,
            _ => Permission::Read,
        }
    })
    .await?;

    let ttl_secs = 3600u64;

    let permissions: &[&str] = if query.op == "write" {
        &["read", "write"]
    } else {
        &["read"]
    };

    let token = issue_token(
        &state.hmac_key,
        &ru.user.id.to_string(),
        &real_path,
        permissions,
        ttl_secs,
    );

    Ok(Json(TokenResponse {
        token,
        expires_in: ttl_secs,
    }))
}
