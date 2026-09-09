//! Temporary share links（分享）.
//!
//! A logged-in user with READ access to files/folders can mint a secret,
//! unauthenticated link (`/s/{token}`) that serves them until it expires (or
//! forever, if created without a TTL). Receivers need no account: the token
//! in the URL IS the capability, stored server-side in the `share_links`
//! table — real filesystem paths never appear in any URL.
//!
//! Share shapes:
//! - **single file** → the link streams that file directly;
//! - **single folder** → the link renders a browse page for the whole tree;
//! - **multi-item** (a "virtual root" collection) → the link renders a
//!   landing page listing the shared files/folders; folders open as browse
//!   pages, files stream/download. Items live in the `share_items` table.
//!
//! Endpoints:
//! - `POST /api/files/share`      — create a link (requires read access to
//!   every item; guests rejected).
//! - `GET  /api/files/shares`     — list the caller's links (admins: all).
//! - `DELETE /api/files/share/{token}` — revoke (owner or admin).
//!
//! Public (no session) — the share IS a temporary session:
//! - `GET /s/{token}`             — the normal frontend app (boots in share
//!   mode; every API/libfw request it makes is prefixed with `/s/{token}/`).
//! - `GET /s/{token}/{*path}`     — the ShareProxy middleware (main.rs)
//!   swaps the URL token for a VIRTUAL share session cookie and rewrites
//!   the request onto the NORMAL routes (`/api/…`, `/file/…`, `/dir/…`,
//!   static assets). No share-specific authorization exists: visitors are
//!   governed by exactly the same session → user → ACL → virtual-share-root
//!   pipeline as any logged-in user, which is what makes libfw transfers
//!   (streaming downloads, folder ZIPs) work untouched.
//!
//! The visitor's identity is fully VIRTUAL — a synthetic non-admin user with
//! no row in `users`, `groups_`, `user_groups`, `sessions` or `acl_entries`
//! (temporary authorization must never mix with regular users and groups).
//! Its READ grants are synthesized per-request from the share's own stored
//! paths (see `Database::get_share_acl_entries`), and its empty group set
//! blocks the `default`-group fallback — a visitor can only ever reach the
//! shared items, through the same ACL layer as everyone else.

use crate::api::files::{ensure_no_symlink, resolve_checked};
use crate::audit::{self, actions};
use crate::auth::session::{get_request_user, GUEST_USER_ID};
use crate::db::ShareLinkRow;
use crate::models::*;
use crate::AppState;
use axum::response::IntoResponse;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    Json,
};
use axum_extra::extract::cookie::CookieJar;
use std::sync::Arc;

/// URL prefix every public share link lives under. Keep in sync with the
/// routes registered in `main.rs`.
const SHARE_PREFIX: &str = "/s/";

/// Upper bound on items per multi-item share — keeps one request from
/// minting thousands of rows or a giant landing page.
const MAX_SHARE_ITEMS: usize = 50;

fn share_url(token: &str) -> String {
    format!("{}{}", SHARE_PREFIX, token)
}

/// Format a TTL (seconds) as a `YYYY-MM-DD HH:MM:SS` local-time expiry
/// string. `ttl_secs == 0` → `None` (never expires).
fn expiry_from_ttl(ttl_secs: u64) -> Option<String> {
    if ttl_secs == 0 {
        return None;
    }
    let exp = chrono::Local::now() + chrono::Duration::seconds(ttl_secs as i64);
    Some(exp.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn share_row_info(row: &ShareLinkRow, items: Vec<ShareItemInfo>) -> ShareLinkInfo {
    ShareLinkInfo {
        token: row.token.clone(),
        name: row.name.clone(),
        path: row.display_path.clone(),
        is_dir: row.is_dir,
        items,
        creator: row.creator.clone(),
        expires_at: row.expires_at.clone(),
        url: share_url(&row.token),
    }
}

// ── Management (authenticated) ──

/// What the creator asked to share, fully resolved and permission-checked.
struct PrepItem {
    real_path: String,
    display_path: String,
    name: String,
    is_dir: bool,
}

/// Create a share link for one file/folder, or a multi-item collection share.
pub async fn create_share(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<CreateShareRequest>,
) -> Result<Json<ShareLinkResponse>, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;
    // Sharing is a logged-in capability: the synthetic guest row (no session)
    // must never mint links, or anyone could hand out others' readable data.
    if ru.user.id == GUEST_USER_ID {
        return Err(StatusCode::UNAUTHORIZED);
    }
    // Temporary share identities must not mint further shares (a share
    // visitor could otherwise re-share the shared content under a new link,
    // escaping the original link's expiry and audit trail).
    if ru.user.oidc_sub.starts_with(crate::db::SHARE_SUB_PREFIX) {
        return Err(StatusCode::FORBIDDEN);
    }
    if body.items.is_empty() || body.items.len() > MAX_SHARE_ITEMS {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Resolve and validate EVERY item before inserting anything, so a bad
    // path fails the whole request instead of creating a partial share.
    let mut prep: Vec<PrepItem> = Vec::with_capacity(body.items.len());
    for path in &body.items {
        let real =
            resolve_checked(&state, &ru, path, crate::acl::Permission::Read)
                .await?;
        let root = state.config.root_dir().clone();
        ensure_no_symlink(&root, &real)?;
        let full = root.join(&real);
        let meta = std::fs::metadata(&full).map_err(|_| StatusCode::NOT_FOUND)?;
        prep.push(PrepItem {
            name: real.rsplit('/').next().unwrap_or(&real).to_string(),
            real_path: real,
            display_path: path.clone(),
            is_dir: meta.is_dir(),
        });
    }

    let token = uuid::Uuid::new_v4().simple().to_string();
    let expires_at = expiry_from_ttl(body.ttl_secs);
    let is_collection = prep.len() > 1;

    if is_collection {
        // Virtual-root collection: the header row carries no real path; the
        // items live in share_items.
        let name = format!("{} 个项目", prep.len());
        if let Err(e) = state.db.create_share_link(
            &token,
            "",
            "",
            &name,
            true,
            ru.user.id,
            &ru.user.display_name,
            expires_at.as_deref(),
        ) {
            tracing::error!("Failed to create share link: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        for (i, p) in prep.iter().enumerate() {
            if let Err(e) = state.db.add_share_item(
                &token, &p.real_path, &p.display_path, &p.name, p.is_dir, i,
            ) {
                tracing::error!("Failed to store share item: {}", e);
                // Best-effort cleanup so no half-built share lingers.
                let _ = state.db.delete_share_link(&token);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    } else {
        let p = &prep[0];
        if let Err(e) = state.db.create_share_link(
            &token,
            &p.real_path,
            &p.display_path,
            &p.name,
            p.is_dir,
            ru.user.id,
            &ru.user.display_name,
            expires_at.as_deref(),
        ) {
            tracing::error!("Failed to create share link: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    let summary = if is_collection {
        let names: Vec<&str> = prep.iter().map(|p| p.display_path.as_str()).collect();
        format!("{} items: {}", prep.len(), names.join(", "))
    } else {
        prep[0].display_path.clone()
    };

    // No identity provisioning happens here: a share visitor is a VIRTUAL
    // identity resolved per-request from these very rows (share_links +
    // share_items) by `Database::get_share_acl_entries`. Nothing is written
    // to `users`, `groups_`, `user_groups`, `sessions` or `acl_entries`.

    audit::record(
        &state.db,
        &ru.user,
        actions::SHARE_CREATE,
        &summary,
        &match &expires_at {
            Some(e) => format!("expires: {}", e),
            None => "never expires".to_string(),
        },
    );

    let url = share_url(&token);
    Ok(Json(ShareLinkResponse {
        token,
        url,
        expires_at,
    }))
}

/// List share links: the caller's own links; admins see everyone's (with the
/// creator column so they can tell them apart). Collection shares carry their
/// top-level items.
pub async fn list_shares(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<Json<Vec<ShareLinkInfo>>, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;
    let rows = state
        .db
        .list_share_links(if ru.user.is_admin == 1 {
            None
        } else {
            Some(ru.user.id)
        })
        .map_err(|e| {
            tracing::error!("Failed to list share links: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let items = if row.real_path.is_empty() {
            match state.db.list_share_items(&row.token) {
                Ok(items) => items
                    .into_iter()
                    .map(|i| ShareItemInfo {
                        name: i.name,
                        path: i.display_path,
                        is_dir: i.is_dir,
                    })
                    .collect(),
                Err(e) => {
                    tracing::error!("Failed to list share items: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        out.push(share_row_info(row, items));
    }
    Ok(Json(out))
}

/// Revoke a share link. Only its creator or an admin may revoke.
pub async fn revoke_share(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Path(token): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let ru = get_request_user(&jar, &state.db).await?;
    let row = state
        .db
        .get_share_link(&token)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if row.created_by != ru.user.id && ru.user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }

    let deleted = state
        .db
        .delete_share_link(&token)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !deleted {
        return Err(StatusCode::NOT_FOUND);
    }
    // Nothing else to clean: the visitor identity is virtual and its grants
    // are synthesized from the (now deleted) link rows themselves.

    audit::record(
        &state.db,
        &ru.user,
        actions::SHARE_REVOKE,
        &row.display_path,
        "",
    );
    Ok(StatusCode::OK)
}

// ── Public access (temporary session) ──

/// Public share entry point: serve the REAL frontend application at
/// `/s/{token}`.
///
/// The app boots in share mode (it detects the `/s/{token}` URL path and
/// prefixes every API/libfw request with it). The ShareProxy middleware in
/// main.rs swaps the URL token for the identity's standard session cookie and
/// rewrites requests onto the normal routes, so a receiver is simply a
/// temporary, read-only user — the exact same pipeline as everyone else.
/// An invalid/expired token gets a clean 404 instead of a broken app shell.
///
/// `/s/{token}` permanently redirects to `/s/{token}/` (trailing slash):
/// index.html references its assets RELATIVELY (`js/api.js`, …), and a
/// browser resolves those against the page's DIRECTORY — `/s/` without the
/// slash, which would produce `/s/js/api.js` (no token → 404). With the
/// slash they resolve to `/s/{token}/js/…` and flow through the proxy.
pub async fn share_index(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Response, StatusCode> {
    state
        .db
        .get_share_link(&token)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // `base_url` is the configured prefix ("" at the domain root); the
    // redirect must carry it or the browser lands outside the app.
    let target = format!("{}{}/", state.config.base_url(), share_url(&token));
    Ok(axum::response::Redirect::permanent(&target).into_response())
}

/// Serve the app shell at `/s/{token}/` (the redirect target above). The
/// wildcard proxy route cannot match the EMPTY rest of the trailing-slash
/// URL, so this needs its own route; the shell is public and needs no
/// session injection — every API/libfw request the app makes afterwards
/// carries the token in its path and flows through the proxy.
pub async fn share_page(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Response, StatusCode> {
    state
        .db
        .get_share_link(&token)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(crate::statics::serve_index().await)
}