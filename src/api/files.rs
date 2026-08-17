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
use std::collections::HashMap;
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

    // When a trash directory is configured, "deleting" MOVES the item into it
    // (preserving its relative path so it can be recovered). An empty
    // `trash_dir` deletes permanently, as before.
    if let Some(trash) = state.config.trash_path() {
        move_to_trash(&trash, &full_path, &real).map_err(|e| {
            tracing::error!(
                "Failed to move '{}' to trash '{}': {}",
                full_path.display(),
                trash.display(),
                e
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        return Ok(StatusCode::OK);
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

/// Find a non-colliding destination for `real` inside the trash directory,
/// preserving the item's relative path structure (so the source location is
/// easy to recover) and appending `" (N)"` to the leaf name when a same-named
/// item already exists there (typical OS trash/duplicate behavior).
fn unique_trash_path(trash: &std::path::Path, real: &str) -> std::path::PathBuf {
    let target = trash.join(real);
    if !target.exists() {
        return target;
    }
    let leaf = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("item")
        .to_string();
    let (stem, ext) = match leaf.rfind('.') {
        Some(i) if i > 0 => (leaf[..i].to_string(), leaf[i..].to_string()),
        _ => (leaf.clone(), String::new()),
    };
    let parent = target.parent().unwrap_or(trash).to_path_buf();
    for n in 1..100_000 {
        let candidate = parent.join(format!("{} ({}){}", stem, n, ext));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Effectively unreachable; fall back to a timestamp suffix.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    parent.join(format!("{}-{}{}", stem, ts, ext))
}

/// Move `src` into the trash directory at `real`'s relative path (creating
/// parent dirs as needed). Falls back to copy+remove when `rename` fails
/// (e.g. the trash lives on a different filesystem, EXDEV), so a trash dir
/// on another volume still works. Returns the final trash location.
fn move_to_trash(
    trash: &std::path::Path,
    src: &std::path::Path,
    real: &str,
) -> std::io::Result<std::path::PathBuf> {
    let target = unique_trash_path(trash, real);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(src, &target) {
        Ok(()) => Ok(target),
        Err(first_err) => {
            tracing::warn!(
                "rename '{}' -> '{}' failed ({}); falling back to copy+remove",
                src.display(),
                target.display(),
                first_err
            );
            // Cross-device (EXDEV) or similar: copy then remove the source so
            // the delete still succeeds and the original is only removed once
            // the trash copy is complete.
            let copied = if src.is_dir() {
                copy_dir_recursive(src, &target)
                    .and_then(|()| std::fs::remove_dir_all(src))
            } else {
                std::fs::copy(src, &target).and_then(|_| std::fs::remove_file(src))
            };
            match copied {
                Ok(()) => Ok(target),
                Err(_) => Err(first_err),
            }
        }
    }
}

/// Recursively copy a directory tree (used for cross-device trash moves).
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
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

/// Decode a batch of opaque shadow paths back to the display paths the user
/// sees (`{shadow: display}`).
///
/// The libfw SDK writes downloaded files using the transfer path it was
/// given — which is an opaque `v1.…` shadow, so without this the user would
/// get files/folders named after shadows. The frontend calls this while a
/// folder download walks `/dir`, then maps shadows to display names locally.
/// Users can only resolve shadows they could read anyway: every decoded real
/// path must lie inside the caller's shares, otherwise the whole request 403s.
#[derive(serde::Deserialize)]
pub struct NamesQuery {
    /// Comma-separated shadow paths (shadows are base64url + `.`, so no
    /// escaping issues) — one request per batch of ≤200.
    pub paths: String,
}

pub async fn get_names(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(query): Query<NamesQuery>,
) -> Result<Json<HashMap<String, String>>, StatusCode> {
    let paths: Vec<&str> = query.paths.split(',').filter(|s| !s.is_empty()).collect();
    if paths.is_empty() || paths.len() > 200 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let ru = get_request_user(&jar, &state.db).await?;
    let acl_entries = state
        .db
        .list_acl_entries()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut out = HashMap::with_capacity(paths.len());
    let mut seen = std::collections::HashSet::new();
    for shadow in &paths {
        if !seen.insert(*shadow) {
            continue;
        }
        let real = state
            .path_codec
            .decode(shadow)
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        let display = acl::display_path_for(&ru.user, &ru.groups, &acl_entries, &real)
            .ok_or(StatusCode::FORBIDDEN)?;
        out.insert(shadow.to_string(), display);
    }
    Ok(Json(out))
}

pub async fn get_token(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(query): Query<TokenQuery>,
) -> Result<Json<TokenResponse>, StatusCode> {
    // Guests (unauthenticated) can get read tokens for anything the `guest`
    // group can read, which is what powers downloads without a session.
    let ru = get_request_user(&jar, &state.db).await?;

    // The token is bound to an OPAQUE shadow of the real path
    // (`EncryptedPathCodec` → `v1.<base64url>`), so the browser never holds
    // the real path — not in the token, not in the transfer URL. libfw
    // decodes the shadow it receives on `/file`/`/dir` back to the real path
    // and authorizes it against this token (via `CodecPathValidator`).
    //
    // Input `path` may be:
    // - a shadow (`v1.…`) the client got from a `/dir` listing or a previous
    //   token — decoded here, then ACL-gated on the real path; or
    // - a display path (non-admin ACL share) / real path (admin) — resolved
    //   through the ACL layer as before. The response's `path` field is the
    //   shadow; transfer URLs MUST use it.
    let acl_entries = state
        .db
        .list_acl_entries()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let permission = match query.op.as_str() {
        "write" => Permission::Write,
        _ => Permission::Read,
    };
    let real_path = match state.path_codec.decode(&query.path) {
        Ok(real) => {
            // Shadow input: still gate on ACL (share may have been revoked
            // since the listing was served).
            acl::can_access(&ru.user, &ru.groups, &acl_entries, &real, &permission)
                .then_some(real)
                .ok_or(StatusCode::FORBIDDEN)?
        }
        Err(_) => {
            // Not a shadow: resolve the display/real path through the ACL
            // layer (admin/root-ACL users pass real paths; non-admins pass
            // share display paths).
            resolve_checked(&state, &ru.user, &ru.groups, &query.path, permission).await?
        }
    };

    // Bind the token to a fresh shadow of the real path. Every encode uses a
    // random nonce, so even the same file yields distinct shadows per token.
    let shadow = state.path_codec.encode(&real_path);

    let ttl_secs = 3600u64;

    let permissions: &[&str] = if query.op == "write" {
        &["read", "write"]
    } else {
        &["read"]
    };

    let token = issue_token(
        &state.hmac_key,
        &ru.user.id.to_string(),
        &shadow,
        permissions,
        ttl_secs,
    );

    Ok(Json(TokenResponse {
        token,
        expires_in: ttl_secs,
        // The shadow bound to this token. The frontend must use it as the
        // transfer path (`/file/{path}` / `/dir/{path}`) — the display path
        // it sent would fail libfw's codec decode.
        path: shadow,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("oneshare-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn unique_trash_path_preserves_structure_and_avoids_collision() {
        let root = temp_root("trash-path");
        let trash = root.join("data").join(".trash");
        std::fs::create_dir_all(&trash).unwrap();

        // First time: the relative path is preserved verbatim.
        let p1 = unique_trash_path(&trash, "docs/report.txt");
        assert_eq!(p1, trash.join("docs").join("report.txt"));

        // Existing item in the trash: the leaf gets a " (N)" suffix.
        std::fs::create_dir_all(p1.parent().unwrap()).unwrap();
        std::fs::write(&p1, "x").unwrap();
        let p2 = unique_trash_path(&trash, "docs/report.txt");
        assert_ne!(p2, p1);
        assert!(p2.to_string_lossy().ends_with("report (1).txt"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn move_to_trash_moves_file_and_preserves_relative_path() {
        let root = temp_root("trash-move-file");
        let src_file = root.join("data").join("docs").join("a.txt");
        std::fs::create_dir_all(src_file.parent().unwrap()).unwrap();
        std::fs::write(&src_file, "hello").unwrap();
        let trash = root.join("data").join(".trash");

        let target = move_to_trash(&trash, &src_file, "docs/a.txt").unwrap();
        assert_eq!(target, trash.join("docs").join("a.txt"));
        assert!(!src_file.exists(), "source must no longer exist");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn move_to_trash_moves_whole_dir() {
        let root = temp_root("trash-move-dir");
        let src_dir = root.join("data").join("folder");
        std::fs::create_dir_all(src_dir.join("sub")).unwrap();
        std::fs::write(src_dir.join("sub").join("f.txt"), "x").unwrap();
        let trash = root.join("data").join(".trash");

        let target = move_to_trash(&trash, &src_dir, "folder").unwrap();
        assert!(!src_dir.exists(), "source dir must no longer exist");
        assert!(target.join("sub").join("f.txt").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn move_to_trash_renames_on_collision() {
        let root = temp_root("trash-move-collision");
        let src_file = root.join("data").join("a.txt");
        std::fs::create_dir_all(src_file.parent().unwrap()).unwrap();
        std::fs::write(&src_file, "new").unwrap();
        let trash = root.join("data").join(".trash");
        // Pre-existing trash item with the same relative path.
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(trash.join("a.txt"), "old").unwrap();

        let target = move_to_trash(&trash, &src_file, "a.txt").unwrap();
        assert!(target.to_string_lossy().ends_with("a (1).txt"));
        assert!(!src_file.exists());
        assert_eq!(std::fs::read_to_string(trash.join("a.txt")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");

        let _ = std::fs::remove_dir_all(&root);
    }
}
