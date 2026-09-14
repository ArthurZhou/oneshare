use crate::acl::{self, Permission};
use crate::audit::{self, actions};
use crate::auth::session::{get_request_user, RequestUser};
use crate::db::{AclEntryRow, UserRow};
use crate::libtoken::issue_token;
use crate::models::*;
use crate::AppState;
use libfw_core::pathmap::PathCodec;
use axum::{
    body::Body,
    extract::{Json, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
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
    // every entry, so listings hide anything the user cannot read. For a
    // share visitor this includes the share's synthesized READ grants.
    let acl_entries = ru.acl_entries(&state.db)?;

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
        // Virtual share root: each share (an ACL-granted path) is a top-level
        // entry. Usually a directory — but ACLs may also point at a single
        // FILE (temporary share identities do exactly that), so stat and
        // render files as file entries instead of dropping them.
        for share in acl::user_shares(&user, &user_groups, &acl_entries) {
            let full = root.join(&share.real_path);
            let Ok(md) = std::fs::metadata(&full) else {
                continue;
            };
            let is_dir = md.is_dir();
            let modified = md
                .modified()
                .ok()
                .and_then(|t| {
                    chrono::DateTime::from_timestamp(
                        t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64,
                        0,
                    )
                })
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            let mime_type = if is_dir {
                "inode/directory".to_string()
            } else {
                mime_guess::from_path(&share.real_path)
                    .first_or_octet_stream()
                    .to_string()
            };
            raw.push((
                FileEntry {
                    name: share.virtual_name.clone(),
                    path: share.virtual_name.clone(),
                    is_dir,
                    size: md.len(),
                    modified,
                    mime_type,
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

/// Recursive byte size of a path, for the frontend's download pre-flight.
///
/// Browsers WITHOUT the File System Access API cannot stream a download to
/// disk: libfw-client buffers the whole transfer in memory and only then
/// saves it (a single file via a normal browser download, a folder as an
/// uncompressed `.zip`, capped by `[libfw] max_fallback_bytes`). The frontend
/// therefore refuses an oversized download *before* starting it — but a
/// folder's total is not in the listing (`FileEntry::size` is the directory
/// entry's own size, not a recursive sum), so it asks the server instead of
/// walking the tree itself.
///
/// The walk mirrors what a download would actually fetch: hidden (`.…`)
/// entries are skipped (as in `build_entry` and the listing) and every entry
/// is ACL-gated on its real path, so files the caller cannot read neither
/// count towards the total nor get downloaded. Symlinks are never followed —
/// they would skew the total (or loop), and the transfer layer refuses them
/// anyway.
///
/// `limit` short-circuits the walk: once the total passes it the handler
/// returns immediately with `exceeded = true`, so the common "too big" answer
/// costs only a partial traversal.
pub async fn size(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(query): Query<SizeQuery>,
) -> Result<Json<SizeResponse>, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;
    let acl_entries = ru.acl_entries(&state.db)?;
    // Resolves the display/real path and requires read on the target itself.
    let start = resolve_checked(&state, &ru, &query.path, Permission::Read).await?;
    let allow = |real: &str| {
        acl::can_access(
            &ru.user,
            &ru.groups,
            &acl_entries,
            real,
            &Permission::Read,
        )
    };
    let (size, exceeded) = walk_size(
        &state.config.root_dir(),
        &start,
        query.limit,
        &allow,
    );
    Ok(Json(SizeResponse { size, exceeded }))
}

/// Recursive byte total of `start` (a real path relative to `root`), counting
/// only files `allow` accepts.
///
/// Mirrors what a download would actually fetch: hidden (`.…`) entries are
/// skipped — as in [`build_entry`] and the listing — and symlinks are never
/// followed (they would skew the total or loop, and the transfer layer
/// refuses them anyway). Returns `(size, exceeded)`; `limit` short-circuits
/// the walk, so `size` is only a lower bound when `exceeded` is true.
fn walk_size(
    root: &std::path::Path,
    start: &str,
    limit: Option<u64>,
    allow: &dyn Fn(&str) -> bool,
) -> (u64, bool) {
    let mut total: u64 = 0;
    // Iterative depth-first walk of real (relative) paths: recursion would
    // need an explicit budget to stay safe on deep trees, and a stack of
    // strings is cheap.
    let mut stack = vec![start.to_string()];
    while let Some(rel) = stack.pop() {
        // `symlink_metadata`: never follow a link, not even for the target.
        let Ok(md) = std::fs::symlink_metadata(root.join(&rel)) else {
            continue;
        };
        if md.file_type().is_symlink() {
            continue;
        }
        if !md.is_dir() {
            total = total.saturating_add(md.len());
            if limit.is_some_and(|l| total > l) {
                return (total, true);
            }
            continue;
        }
        let Ok(read_dir) = std::fs::read_dir(root.join(&rel)) else {
            continue;
        };
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue; // hidden entries (and `.libfw-tmp-*` leftovers)
            }
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if allow(&child) {
                stack.push(child);
            }
        }
    }
    (total, false)
}

/// Reject file-management operations whose target path (or any parent
/// component under the root) contains a symlink.
///
/// libfw's transfer layer already refuses symlinked uploads, but the
/// management APIs (`mkdir`, `delete`, `rename`, `mv`) operate on real
/// paths directly with `std::fs`, and calls like `create_dir_all` and
/// `remove_dir_all` follow symlinks — a symlink planted inside the root
/// would let them create/delete/rename outside it. This check walks every
/// component of `rel` under `root` with `symlink_metadata` (never
/// following links) and rejects the operation if any component is a
/// symlink. A missing component (e.g. the destination of a move that does
/// not exist yet) stops the walk early — nothing after it can exist either.
pub(crate) fn ensure_no_symlink(root: &std::path::Path, rel: &str) -> Result<(), StatusCode> {
    let mut cur = root.to_path_buf();
    for comp in std::path::Path::new(rel).components() {
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) => {
                if md.file_type().is_symlink() {
                    tracing::warn!(
                        "Rejecting file operation through symlink: {}",
                        cur.display()
                    );
                    return Err(StatusCode::FORBIDDEN);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                tracing::error!("symlink check failed for {}: {}", cur.display(), e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }
    Ok(())
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<FileOperation>,
) -> Result<impl IntoResponse, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;

    let real = resolve_checked(&state, &ru, &body.path, Permission::Write).await?;
    // Refuse to act on the filesystem root itself: no UI flow produces this,
    // and both "move the whole root into the trash" and "remove_dir_all the
    // root" would be catastrophic.
    if real.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let root = state.config.root_dir().clone();
    ensure_no_symlink(&root, &real)?;
    let full_path = root.join(&real);
    if !full_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // When a trash directory is configured, "deleting" MOVES the item into it
    // (preserving its relative path so it can be recovered). An empty
    // `trash_dir` deletes permanently, as before.
    //
    // EXCEPTION (by design): items that are already inside the trash are
    // permanently removed instead of being moved deeper into the trash — a
    // move would just shuffle them under `.trash/.trash/…` and never free
    // any space.
    let trash = state.config.trash_path();
    let in_trash = trash
        .as_ref()
        .map(|t| is_inside_trash(t, &full_path))
        .unwrap_or(false);
    if let Some(trash) = trash.filter(|_| !in_trash) {
        let target = move_to_trash(&trash, &full_path, &real).map_err(|e| {
            tracing::error!(
                "Failed to move '{}' to trash '{}': {}",
                full_path.display(),
                trash.display(),
                e
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let detail = match target.strip_prefix(&trash) {
            Ok(rel) => format!("moved to trash: {}", rel.display()),
            Err(_) => format!("moved to trash: {}", target.display()),
        };
        audit::record(&state.db, &ru.user, actions::DELETE, &body.path, &detail);
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

    let detail = if in_trash {
        "permanently deleted (was in trash)"
    } else {
        "permanently deleted"
    };
    audit::record(&state.db, &ru.user, actions::DELETE, &body.path, detail);

    Ok(StatusCode::OK)
}

/// True when `full` IS the trash directory or lies inside it. Component-based
/// comparison, so sibling names that merely share a prefix (`data/.trash-2`)
/// do not count as "inside".
fn is_inside_trash(trash: &std::path::Path, full: &std::path::Path) -> bool {
    full.starts_with(trash)
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

    let real = resolve_checked(&state, &ru, &body.path, Permission::Write).await?;
    let root = state.config.root_dir().clone();
    ensure_no_symlink(&root, &real)?;
    let old_full = root.join(&real);
    if !old_full.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Get parent directory
    let parent = old_full.parent().unwrap_or(&root);
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

    // The new name lives in the same parent (already checked above), but the
    // target itself could be an existing symlink — refusing to rename over
    // it keeps the rename from ever acting through a link.
    if let Some(new_rel) = std::path::Path::new(&real).parent().and_then(|p| {
        if p.as_os_str().is_empty() {
            Some(std::path::PathBuf::from(&body.new_name))
        } else {
            Some(p.join(&body.new_name))
        }
    }) {
        ensure_no_symlink(&root, &new_rel.to_string_lossy())?;
    }

    // Refuse to silently overwrite an existing target (data-loss guard).
    if new_full.exists() && new_full != old_full {
        return Err(StatusCode::CONFLICT);
    }

    std::fs::rename(&old_full, &new_full).map_err(|e| {
        tracing::error!("Failed to rename: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    audit::record(
        &state.db,
        &ru.user,
        actions::RENAME,
        &body.path,
        &format!("renamed to: {}", body.new_name),
    );

    Ok(StatusCode::OK)
}

pub async fn mv(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<MoveRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;

    let src_real = resolve_checked(&state, &ru, &body.source, Permission::Write).await?;
    let dst_real = resolve_checked(&state, &ru, &body.destination, Permission::Write).await?;

    let root = state.config.root_dir().clone();
    ensure_no_symlink(&root, &src_real)?;
    ensure_no_symlink(&root, &dst_real)?;
    let src_full = root.join(&src_real);
    let dst_full = root.join(&dst_real);

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

    audit::record(
        &state.db,
        &ru.user,
        actions::MOVE,
        &body.source,
        &format!("moved to: {}", body.destination),
    );

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
    let real = resolve_checked(&state, &ru, &body.path, Permission::Write).await?;
    let root = state.config.root_dir().clone();
    // Check the parent chain AND the would-be target: `create_dir_all`
    // follows symlinks, so a symlink in the chain (or a same-named symlink
    // already sitting at the target) must refuse the operation.
    ensure_no_symlink(&root, &real)?;
    let new_rel = if real.is_empty() {
        body.name.clone()
    } else {
        format!("{}/{}", real, body.name)
    };
    ensure_no_symlink(&root, &new_rel)?;
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

    let display_new = if body.path.is_empty() {
        body.name.clone()
    } else {
        format!("{}/{}", body.path, body.name)
    };
    audit::record(&state.db, &ru.user, actions::MKDIR, &display_new, "");

    Ok(StatusCode::CREATED)
}

// ── Inline preview & online text editing ──
//
// Three endpoints power the file detail view:
// - `GET /api/files/content` — metadata + (for small text files) the full
//   text content, for the text preview/editor.
// - `PUT /api/files/content` — save edited text back (write permission).
// - `GET /api/files/raw` — inline binary preview (images, video, audio) with
//   session-cookie auth, so `<img src>` just works.
//
// All of them resolve the display path through the same ACL layer as every
// other file operation and never expose real filesystem paths.

/// Maximum size (bytes) of a text file served to / accepted from the editor.
const MAX_TEXT_SIZE: u64 = 1024 * 1024;

/// Leaf name of a real (sanitized) relative path.
fn leaf_name(real: &str) -> &str {
    real.rsplit('/').next().unwrap_or(real)
}

/// Read a file for inline text preview / editing.
///
/// Returns `(is_text, content, truncated)`: within [`MAX_TEXT_SIZE`] the
/// bytes are read once and classified by content — declared text MIME types
/// are served as text, and so is any file that is valid UTF-8 without NUL
/// bytes (catches extension-less scripts/configs). Anything else is binary.
/// Text larger than the limit is reported as `truncated` so the UI can offer
/// download instead. Shared by the authenticated editor endpoint
/// (`get_content`) and the public share detail endpoint.
pub(crate) fn read_text_preview(
    full: &std::path::Path,
    mime: &str,
    size: u64,
) -> (bool, Option<String>, bool) {
    let mut is_text = is_text_mime(mime);
    let mut content = None;
    let mut truncated = false;

    if size <= MAX_TEXT_SIZE {
        match std::fs::read(full) {
            Ok(bytes) => {
                let looks_text = !bytes.contains(&0u8);
                match String::from_utf8(bytes) {
                    Ok(text) if is_text || looks_text => {
                        is_text = true;
                        content = Some(text);
                    }
                    _ => is_text = false,
                }
            }
            Err(e) => {
                // Unreadable file: report as non-text rather than failing the
                // whole detail response (metadata is still useful).
                tracing::error!("Failed to read '{}' for preview: {}", full.display(), e);
                return (false, None, false);
            }
        }
    } else if is_text {
        truncated = true;
    } else {
        is_text = false;
    }
    (is_text, content, truncated)
}

/// Whether a MIME type denotes a plain-text family we preview/edit inline.
/// Extension-less text (Makefile, Dockerfile, …) is still caught by the
/// UTF-8/no-NUL sniff in `get_content`.
fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json"
                | "application/xml"
                | "application/xhtml+xml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/yaml"
                | "application/x-yaml"
                | "application/toml"
                | "application/x-sh"
                | "application/sql"
                | "application/x-httpd-php"
                | "application/x-perl"
                | "application/x-python"
                | "image/svg+xml"
        )
}

/// Whether a response body may be served inline (`<img>`/`<video>`/`<audio>`
/// or a browser-visible page). Deliberately limited to browser-safe media:
/// images (never SVG — it can carry scripts, which would be a stored-XSS
/// hole when served same-origin), video and audio. HTML/PDF/etc. are
/// download-only.
pub(crate) fn is_safe_inline_mime(mime: &str) -> bool {
    let mime = mime.split(';').next().unwrap_or(mime).trim();
    (mime.starts_with("image/") && mime != "image/svg+xml")
        || mime.starts_with("video/")
        || mime.starts_with("audio/")
}

/// Fetch a file's metadata + text content for the detail view / editor.
pub async fn get_content(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(query): Query<crate::models::ContentQuery>,
) -> Result<Json<crate::models::FileContentResponse>, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;
    let real = resolve_checked(&state, &ru, &query.path, Permission::Read).await?;
    let root = state.config.root_dir().clone();
    ensure_no_symlink(&root, &real)?;
    let full = root.join(&real);

    let meta = std::fs::metadata(&full).map_err(|_| StatusCode::NOT_FOUND)?;
    if meta.is_dir() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let name = leaf_name(&real).to_string();
    let mime = mime_guess::from_path(&name)
        .first_or_octet_stream()
        .to_string();

    let acl_entries = ru.acl_entries(&state.db)?;
    let writable =
        acl::can_access(&ru.user, &ru.groups, &acl_entries, &real, &Permission::Write);

    // Text handling: within the size limit we read the bytes once and decide
    // by content — declared text MIME types are served as text, and so is any
    // file that is valid UTF-8 without NUL bytes (catches extension-less
    // scripts/configs). Anything else is binary. Text larger than the limit
    // is reported as `truncated` so the UI can offer download instead.
    let (is_text, content, truncated) = read_text_preview(&full, &mime, meta.len());

    Ok(Json(crate::models::FileContentResponse {
        name,
        path: query.path,
        size: meta.len(),
        modified: meta
            .modified()
            .ok()
            .and_then(|t| {
                chrono::DateTime::from_timestamp(
                    t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64,
                    0,
                )
            })
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        mime_type: mime,
        is_text,
        writable,
        content,
        truncated,
    }))
}

/// Save edited text content back to a file (requires write permission).
///
/// Refuses to write into a directory, over the size limit, or over a file
/// that is not text (NUL byte / invalid UTF-8), so the editor can never
/// silently corrupt a binary file.
pub async fn put_content(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<crate::models::SaveContentRequest>,
) -> Result<StatusCode, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;

    if body.content.len() as u64 > MAX_TEXT_SIZE {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let real =
        resolve_checked(&state, &ru, &body.path, Permission::Write).await?;
    let root = state.config.root_dir().clone();
    ensure_no_symlink(&root, &real)?;
    let full = root.join(&real);

    let meta = std::fs::metadata(&full).map_err(|_| StatusCode::NOT_FOUND)?;
    if meta.is_dir() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Only ever (over)write files we know are text: a NUL byte or invalid
    // UTF-8 in the existing content means the editor is pointed at a binary
    // file, and saving would destroy it.
    match std::fs::read(&full) {
        Ok(existing) => {
            if existing.contains(&0u8) || String::from_utf8(existing).is_err() {
                return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
            }
        }
        Err(e) => {
            tracing::error!("Failed to read '{}' before save: {}", full.display(), e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    std::fs::write(&full, body.content.as_bytes()).map_err(|e| {
        tracing::error!("Failed to write '{}': {}", full.display(), e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    audit::record(
        &state.db,
        &ru.user,
        actions::FILE_SAVE,
        &body.path,
        &format!("saved {} bytes", body.content.len()),
    );

    Ok(StatusCode::OK)
}

/// Parse a single-range `Range: bytes=…` header against a resource of `size`
/// bytes. Only the FIRST range of a multi-range request is honored (browsers
/// send one range per media seek, so this covers real usage).
///
/// Returns `None` for an absent/empty/`bytes=`-less header (serve 200 full
/// body), `Some((start, end))` with `end` inclusive, or `Err(())` when the
/// header is well-formed but unsatisfiable (→ 416).
fn parse_byte_range(header: &str, size: u64) -> Result<Option<(u64, u64)>, ()> {
    // Absent unit / other units (`items=…`): ignore the header and serve the
    // full body (200), per RFC 9110 §14.2.
    let rest = match header.trim().strip_prefix("bytes=") {
        Some(rest) => rest,
        None => return Ok(None),
    };
    let first = rest.split(',').next().unwrap_or("").trim();
    let (start_s, end_s) = first.split_once('-').ok_or(())?;

    let range = if start_s.is_empty() {
        // Suffix range: the LAST `end_s` bytes (`bytes=-500`). An empty or
        // zero suffix means nothing — treat as unsatisfiable.
        let suffix: u64 = end_s.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = size.saturating_sub(suffix);
        (start, size.saturating_sub(1))
    } else {
        let start: u64 = start_s.parse().map_err(|_| ())?;
        let end = if end_s.is_empty() {
            size.saturating_sub(1)
        } else {
            end_s.parse::<u64>().map_err(|_| ())?.min(size.saturating_sub(1))
        };
        if start > end {
            return Err(());
        }
        (start, end)
    };

    if size == 0 || range.0 >= size {
        return Err(());
    }
    Ok(Some(range))
}

/// Build a `Content-Disposition` header value with an ASCII fallback name and
/// an RFC 5987 `filename*=UTF-8''` form, so non-ASCII (Chinese) filenames
/// survive every browser.
fn content_disposition(kind: &str, name: &str) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
    const PCT: &AsciiSet = &CONTROLS.add(b'"').add(b'\\').add(b'%');
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' && !c.is_control() {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "{}; filename=\"{}\"; filename*=UTF-8''{}",
        kind,
        ascii,
        utf8_percent_encode(name, PCT)
    )
}

/// Stream a file off disk as the response body.
///
/// Never loads the file into memory: bytes flow from `tokio::fs::File` through
/// a `ReaderStream` in 64 KiB chunks, so videos of any size preview fine.
/// Honors a single `Range: bytes=…` request (206 + `Content-Range`), which is
/// what makes `<video>`/`<audio>` seeking work; responses always advertise
/// `Accept-Ranges: bytes`.
///
/// Shared by the authenticated inline preview (`/api/files/raw`) and the
/// public share-link endpoints (`/s/{token}`, `/s/{token}/raw`).
pub(crate) async fn serve_file_response(
    full: &std::path::Path,
    mime: &str,
    range: Option<&str>,
    inline: bool,
    name: &str,
) -> Result<Response, StatusCode> {
    use tokio::io::AsyncSeekExt;

    let meta = tokio::fs::metadata(full)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    if meta.is_dir() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let size = meta.len();

    let disposition = content_disposition(
        if inline { "inline" } else { "attachment" },
        name,
    );

    // Unsatisfiable range → 416 with the conventional `*/size` Content-Range.
    let parsed = match range {
        Some(r) => match parse_byte_range(r, size) {
            Ok(parsed) => parsed,
            Err(()) => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header("content-range", format!("bytes */{}", size))
                    .header("accept-ranges", "bytes")
                    .body(Body::empty())
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
            }
        },
        None => None,
    };

    let mut file = tokio::fs::File::open(full)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let (status, _start, length) = match parsed {
        Some((start, end)) => {
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            (
                StatusCode::PARTIAL_CONTENT,
                start,
                end - start + 1,
            )
        }
        None => (StatusCode::OK, 0, size),
    };

    let stream = tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024);

    let mut builder = Response::builder()
        .status(status)
        .header("content-type", mime)
        .header("content-length", length)
        .header("accept-ranges", "bytes")
        .header("content-disposition", disposition)
        // Never let the browser sniff around our declared (safe) type.
        .header("x-content-type-options", "nosniff")
        // Share links are unauthenticated URLs carrying their own secret
        // token; no caching keeps revoked/expired links honest.
        .header("cache-control", "no-cache");
    if let Some((start, end)) = parsed {
        builder = builder.header("content-range", format!("bytes {}-{}/{}", start, end, size));
    }

    builder
        .body(Body::from_stream(stream))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Serve a file's raw bytes for inline preview (`<img>`/`<video>`/`<audio>`).
///
/// Authenticated by session cookie like every other `/api/files` endpoint,
/// so plain element sources work without bearer-token plumbing. Restricted
/// to browser-safe media types (see [`is_safe_inline_mime`]).
///
/// The body is STREAMED (see [`serve_file_response`]) with HTTP Range
/// support — there is no size cap and no buffering: large videos preview
/// and seek without ever being loaded into memory or routed through the
/// libfw transfer protocol.
pub async fn raw(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: axum::http::HeaderMap,
    Query(query): Query<crate::models::ContentQuery>,
) -> Result<Response, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;
    let real = resolve_checked(&state, &ru, &query.path, Permission::Read).await?;
    let root = state.config.root_dir().clone();
    ensure_no_symlink(&root, &real)?;
    let full = root.join(&real);

    let mime = mime_guess::from_path(leaf_name(&real))
        .first_or_octet_stream()
        .to_string();
    if !is_safe_inline_mime(&mime) {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    serve_file_response(
        &full,
        &mime,
        headers.get(axum::http::header::RANGE).and_then(|v| v.to_str().ok()),
        true,
        leaf_name(&real),
    )
    .await
}

/// Resolve a frontend-supplied path to a real path and check `required`
/// permission on it. Returns 403/404 for paths outside the user's access.
pub(crate) async fn resolve_checked(
    state: &Arc<AppState>,
    ru: &RequestUser,
    path: &str,
    required: Permission,
) -> Result<String, StatusCode> {
    let acl_entries = ru.acl_entries(&state.db)?;
    let (user, user_groups) = (&ru.user, &ru.groups);

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
/// Uploads pass **compound** shadows (`{dirShadow}/{literal…}`) as their plan
/// paths too, so each path is decoded whole-first and, failing that, as the
/// longest decodable segment prefix plus the literal remainder (see
/// [`decode_compound`]) — matching libfw's own `resolve_client_path`.
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
    // Use the REQUEST identity's ACL view (`ru.acl_entries`), not the raw
    // global cache: for a share visitor the visible tree comes from the
    // share's synthesized read grants, so resolving a shadow against the
    // global table alone yields no share and answers 403 — which broke every
    // libfw download under a share link (the SDK resolves display names for
    // both file and folder saves through this endpoint).
    let acl_entries = ru.acl_entries(&state.db)?;

    let mut out = HashMap::with_capacity(paths.len());
    let mut seen = std::collections::HashSet::new();
    for shadow in &paths {
        if !seen.insert(*shadow) {
            continue;
        }
        // Whole-string decode first (a plain `v1.…` shadow); if that fails,
        // fall back to hierarchical composition (`{dirShadow}/{literal…}`) so
        // upload plan paths resolve too — mirrors libfw-server's
        // `resolve_client_path`.
        let real = match state.path_codec.decode(shadow) {
            Ok(real) => real,
            Err(_) => match decode_compound(&*state.path_codec, shadow) {
                Some(real) => real,
                None => return Err(StatusCode::BAD_REQUEST),
            },
        };
        let display = acl::display_path_for(&ru.user, &ru.groups, &acl_entries, &real)
            .ok_or(StatusCode::FORBIDDEN)?;
        out.insert(shadow.to_string(), display);
    }
    Ok(Json(out))
}

/// Decode the **longest** decodable segment prefix of `shadow` and append
/// the trailing literal segments to the decoded real path.
///
/// This gives shadow paths hierarchical composition — a shadow for `docs`
/// used as `{shadow}/a.txt` resolves to `docs/a.txt` — mirroring
/// libfw-server's `decode_compound`/`resolve_client_path`, which the upload
/// client relies on for plan paths (`{dirShadow}/{relative/path}`). The
/// search is longest-prefix-first (`a/b/c` tries `a/b`, then `a`), splitting
/// on `/` segment boundaries only so a decoded root can never swallow part of
/// a literal segment. Returns `None` when no prefix decodes; the caller keeps
/// the original decode error semantics (400).
fn decode_compound(codec: &dyn PathCodec, shadow: &str) -> Option<String> {
    let mut end = shadow.len();
    while let Some(slash) = shadow[..end].rfind('/') {
        end = slash;
        let prefix = &shadow[..slash];
        if let Ok(real) = codec.decode(prefix) {
            let rest = &shadow[slash + 1..];
            return Some(if real.is_empty() {
                rest.to_string()
            } else {
                format!("{real}/{rest}")
            });
        }
    }
    None
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
    let acl_entries = ru.acl_entries(&state.db)?;
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
            resolve_checked(&state, &ru, &query.path, permission).await?
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

    // Audit the transfer intent: libfw handles the actual bytes, so token
    // issuance is the one place we can attribute an upload/download to a
    // user. The stored path is the DISPLAY path (a raw shadow input would be
    // unreadable in the log).
    let action = if query.op == "write" {
        actions::TOKEN_WRITE
    } else {
        actions::TOKEN_READ
    };
    let display = acl::display_path_for(&ru.user, &ru.groups, &acl_entries, &real_path)
        .unwrap_or_else(|| query.path.clone());
    audit::record(
        &state.db,
        &ru.user,
        action,
        &display,
        &format!("op={}", query.op),
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

    /// A share visitor's display-name resolution must run against the REQUEST
    /// identity's ACL view (`RequestUser::acl_entries`: global table + the
    /// share's synthesized read grants), never the raw global cache.
    ///
    /// Regression guard for the share-download 403: `get_names` used
    /// `Database::list_acl_entries_cached` (global rows only), where the
    /// virtual share identity (id [`crate::db::SHARE_USER_ID`]) matches no
    /// entry — so every shadow decoded to "not in any of my shares" and the
    /// endpoint answered 403. The libfw SDK resolves display names through
    /// this endpoint for every download (file names and folder/ZIP entry
    /// names), so ALL downloads under a share link failed with a 403.
    #[test]
    fn share_visitor_resolves_names_through_share_grants() {
        let dir = std::env::temp_dir().join(format!("oneshare-share-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Database::new(dir.join("test.db").to_str().unwrap(), None).unwrap();
        db.create_share_link("tok", "pub", "pub", "pub", true, 1, "tester", None)
            .unwrap();

        let ru = crate::auth::session::RequestUser {
            user: crate::auth::session::share_user("tok", "pub"),
            groups: Vec::new(),
            share_entries: db.get_share_acl_entries("tok").unwrap(),
        };

        // The share's real path is reachable, and maps back to the path the
        // visitor sees (a root-level share folder named after its leaf).
        let entries = ru.acl_entries(&db).unwrap();
        assert_eq!(
            acl::display_path_for(&ru.user, &ru.groups, &entries, "pub/a.txt").as_deref(),
            Some("pub/a.txt")
        );

        // The global cache alone (the bug) resolves nothing for this identity.
        let global = db.list_acl_entries_cached().unwrap();
        assert_eq!(
            acl::display_path_for(&ru.user, &ru.groups, &global, "pub/a.txt"),
            None
        );
    }

    /// Build `root/` with a nested tree used by the `walk_size` tests:
    ///
    /// ```text
    /// root/
    ///   a.txt           (4 bytes)
    ///   .hidden.txt     (99 bytes, must not count)
    ///   sub/
    ///     b.txt         (3 bytes)
    ///     blocked.txt   (50 bytes, denied by the allow filter)
    /// ```
    fn size_fixture(name: &str) -> std::path::PathBuf {
        let root = temp_root(name);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), b"aaaa").unwrap();
        std::fs::write(root.join(".hidden.txt"), vec![b'x'; 99]).unwrap();
        std::fs::write(root.join("sub/b.txt"), b"bbb").unwrap();
        std::fs::write(root.join("sub/blocked.txt"), vec![b'y'; 50]).unwrap();
        root
    }

    /// Everyone except the deliberately unreadable entry.
    fn allow_all_but_blocked(real: &str) -> bool {
        !real.ends_with("blocked.txt")
    }

    #[test]
    fn walk_size_sums_the_tree_skipping_hidden_and_denied_entries() {
        let root = size_fixture("walk-size");
        assert_eq!(walk_size(&root, "", None, &allow_all_but_blocked), (7, false));
        // A single file counts its own size (the frontend probes files too
        // when the listing size is not at hand).
        assert_eq!(
            walk_size(&root, "a.txt", None, &allow_all_but_blocked),
            (4, false)
        );
        // An entry the ACL denies contributes nothing.
        assert_eq!(walk_size(&root, "sub", None, &allow_all_but_blocked), (3, false));
        // A missing path is not an error, it just counts nothing.
        assert_eq!(walk_size(&root, "nope", None, &allow_all_but_blocked), (0, false));
    }

    #[test]
    fn walk_size_stops_early_once_past_the_limit() {
        let root = size_fixture("walk-size-limit");
        // Under the cap: the exact total is reported and nothing was skipped.
        assert_eq!(walk_size(&root, "", Some(100), &allow_all_but_blocked), (7, false));
        // Equal to the cap is still fine (`>`, not `>=`).
        assert_eq!(walk_size(&root, "", Some(7), &allow_all_but_blocked), (7, false));
        // Over the cap: the walk short-circuits, so the total is a lower
        // bound — enough for the caller to reject the download.
        let (size, exceeded) = walk_size(&root, "", Some(3), &allow_all_but_blocked);
        assert!(exceeded);
        assert!(size > 3);
    }

    #[test]
    fn byte_range_open_ended_clamps_to_size() {
        // bytes=100- over a 500-byte file → 100..=499
        assert_eq!(parse_byte_range("bytes=100-", 500).unwrap(), Some((100, 499)));
    }

    #[test]
    fn byte_range_end_clamps_beyond_size() {
        // bytes=100-999 over a 500-byte file → end clamps to 499
        assert_eq!(parse_byte_range("bytes=100-999", 500).unwrap(), Some((100, 499)));
    }

    #[test]
    fn byte_range_suffix_reads_from_the_end() {
        // bytes=-100 over a 500-byte file → the last 100 bytes
        assert_eq!(parse_byte_range("bytes=-100", 500).unwrap(), Some((400, 499)));
        // Suffix longer than the file → the whole file
        assert_eq!(parse_byte_range("bytes=-1000", 500).unwrap(), Some((0, 499)));
    }

    #[test]
    fn byte_range_unsatisfiable_requests_error() {
        // start past the end, zero-length suffix, and malformed specs → 416
        assert!(parse_byte_range("bytes=500-", 500).is_err());
        assert!(parse_byte_range("bytes=-0", 500).is_err());
        assert!(parse_byte_range("bytes=300-200", 500).is_err());
        assert!(parse_byte_range("bytes=abc-", 500).is_err());
    }

    #[test]
    fn byte_range_absent_or_foreign_unit_is_ignored() {
        assert_eq!(parse_byte_range("bytes=0-4", 500).unwrap(), Some((0, 4)));
        // No header / wrong unit → serve the full body (200)
        assert_eq!(parse_byte_range("", 500).unwrap(), None);
        assert_eq!(parse_byte_range("items=0-4", 500).unwrap(), None);
        // Only the first range of a multi-range request is honored
        assert_eq!(parse_byte_range("bytes=0-4,10-20", 500).unwrap(), Some((0, 4)));
    }

    #[test]
    fn content_disposition_escapes_non_ascii_and_quotes() {
        let v = content_disposition("inline", "报告 final\"v2\".pdf");
        assert!(v.starts_with("inline;"));
        assert!(v.contains("filename=\"__ final_v2_.pdf\""));
        assert!(v.contains("filename*=UTF-8''"));
    }

    /// Whole-string-only codec (like `EncryptedPathCodec`): only `"docs"`
    /// decodes; compound paths must be split at `/` until the prefix decodes.
    struct DocsOnly;

    impl PathCodec for DocsOnly {
        fn encode(&self, real: &str) -> String {
            real.to_string()
        }
        fn decode(&self, shadow: &str) -> Result<String, libfw_core::pathmap::PathCodecError> {
            if shadow == "docs" {
                Ok("real/docs".to_string())
            } else {
                Err(libfw_core::pathmap::PathCodecError::Unmapped(shadow.to_string()))
            }
        }
    }

    #[test]
    fn decode_compound_resolves_dir_shadow_plus_literal_suffix() {
        let real = decode_compound(&DocsOnly, "docs/sub/deep/file.txt").unwrap();
        assert_eq!(real, "real/docs/sub/deep/file.txt");
    }

    #[test]
    fn decode_compound_prefers_longest_decodable_prefix() {
        struct TwoLevel;
        impl PathCodec for TwoLevel {
            fn encode(&self, real: &str) -> String {
                real.to_string()
            }
            fn decode(&self, shadow: &str) -> Result<String, libfw_core::pathmap::PathCodecError> {
                match shadow {
                    "docs" => Ok("real/docs".to_string()),
                    "docs/sub" => Ok("real/docs/sub".to_string()),
                    _ => Err(libfw_core::pathmap::PathCodecError::Unmapped(shadow.to_string())),
                }
            }
        }
        // "docs/sub" decodes and is longer than "docs", so it must win.
        let real = decode_compound(&TwoLevel, "docs/sub/deep/file.txt").unwrap();
        assert_eq!(real, "real/docs/sub/deep/file.txt");
    }

    #[test]
    fn decode_compound_returns_none_when_nothing_decodes() {
        assert_eq!(decode_compound(&DocsOnly, "other/deep/file.txt"), None);
        // A whole shadow (no `/`) is not a compound — the caller handles it.
        assert_eq!(decode_compound(&DocsOnly, "docs"), None);
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

    #[test]
    fn is_inside_trash_detects_trash_contents_only() {
        let root = temp_root("trash-inside");
        let trash = root.join("data").join(".trash");
        std::fs::create_dir_all(trash.join("docs")).unwrap();

        // The trash dir itself and anything under it are "inside".
        assert!(is_inside_trash(&trash, &trash));
        assert!(is_inside_trash(&trash, &trash.join("docs")));
        assert!(is_inside_trash(&trash, &trash.join("docs").join("a.txt")));

        // Siblings that merely share a name prefix are NOT inside, and
        // neither is the rest of the tree.
        assert!(!is_inside_trash(&trash, &root.join("data").join(".trash-2")));
        assert!(!is_inside_trash(
            &trash,
            &root.join("data").join("trash")
        ));
        assert!(!is_inside_trash(&trash, &root.join("data").join("docs")));

        let _ = std::fs::remove_dir_all(&root);
    }
}
