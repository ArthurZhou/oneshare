use crate::auth::session::get_user_from_cookie;
use crate::AppState;
use axum::{
    extract::{Json, Path, Query, State},
    http::StatusCode,
};
use axum_extra::extract::cookie::CookieJar;
use std::sync::Arc;

use crate::audit::{self, actions};

// ── Groups ──

pub async fn list_groups(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    let groups = state.db.list_groups().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::to_value(groups).unwrap()))
}

pub async fn create_group(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<crate::models::CreateGroupRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    let group = state
        .db
        .create_group(&body.name, &body.description.unwrap_or_default())
        .map_err(|e| {
            tracing::error!("Create group error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    audit::record(
        &state.db,
        &user,
        actions::GROUP_CREATE,
        "",
        &format!("group={}", group.name),
    );
    Ok(Json(serde_json::to_value(group).unwrap()))
}

pub async fn delete_group(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Path(group_id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    // The reserved `default` and `guest` groups cannot be deleted: they are
    // what grant permissions to users not assigned to any explicit group and
    // to unauthenticated visitors respectively.
    let group = state
        .db
        .get_group_by_id(group_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(g) = &group {
        if g.name == crate::db::Database::DEFAULT_GROUP_NAME
            || g.name == crate::db::Database::GUEST_GROUP_NAME
        {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    state
        .db
        .delete_group(group_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    audit::record(
        &state.db,
        &user,
        actions::GROUP_DELETE,
        "",
        &format!(
            "group={}",
            group
                .map(|g| g.name)
                .unwrap_or_else(|| format!("#{group_id}"))
        ),
    );
    Ok(StatusCode::OK)
}

pub async fn list_group_members(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Path(group_id): Path<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    let members = state
        .db
        .list_group_members(group_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::to_value(members).unwrap()))
}

pub async fn add_user_to_group(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<crate::models::AddUserToGroupRequest>,
) -> Result<StatusCode, StatusCode> {
    let u = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if u.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    // The reserved `default` and `guest` groups have their members derived
    // automatically (unassigned users and unauthenticated visitors) and so
    // must not have their membership edited by hand.
    if is_reserved_group(&state, body.group_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    state
        .db
        .add_user_to_group(body.user_id, body.group_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    audit::record(
        &state.db,
        &u,
        actions::GROUP_MEMBER_ADD,
        "",
        &member_change_detail(&state.db, body.user_id, body.group_id),
    );
    Ok(StatusCode::OK)
}

pub async fn remove_user_from_group(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<crate::models::AddUserToGroupRequest>,
) -> Result<StatusCode, StatusCode> {
    let u = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if u.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    if is_reserved_group(&state, body.group_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    state
        .db
        .remove_user_from_group(body.user_id, body.group_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    audit::record(
        &state.db,
        &u,
        actions::GROUP_MEMBER_REMOVE,
        "",
        &member_change_detail(&state.db, body.user_id, body.group_id),
    );
    Ok(StatusCode::OK)
}

/// Human-readable `"user -> group"` pair for membership audit details.
/// Falls back to `#id` when the referenced row has already disappeared.
fn member_change_detail(db: &crate::db::Database, user_id: i64, group_id: i64) -> String {
    let uname = db
        .get_user_by_id(user_id)
        .ok()
        .flatten()
        .map(|u| u.display_name)
        .unwrap_or_else(|| format!("#{user_id}"));
    let gname = db
        .get_group_by_id(group_id)
        .ok()
        .flatten()
        .map(|g| g.name)
        .unwrap_or_else(|| format!("#{group_id}"));
    format!("{uname} -> {gname}")
}

/// True if `group_id` refers to one of the reserved `default`/`guest` groups,
/// whose membership is managed automatically and cannot be edited by admins.
fn is_reserved_group(state: &AppState, group_id: i64) -> bool {
    match state.db.get_group_by_id(group_id) {
        Ok(Some(g)) => {
            g.name == crate::db::Database::DEFAULT_GROUP_NAME
                || g.name == crate::db::Database::GUEST_GROUP_NAME
        }
        _ => false,
    }
}

// ── Users ──

pub async fn list_users(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    let users = state.db.list_users().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::to_value(users).unwrap()))
}

// ── ACL ──

pub async fn set_acl(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<crate::models::SetAclRequest>,
) -> Result<StatusCode, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    if !matches!(body.permission.as_str(), "read" | "write" | "admin") {
        return Err(StatusCode::BAD_REQUEST);
    }
    if body.user_id.is_none() && body.group_id.is_none() {
        return Err(StatusCode::BAD_REQUEST);
    }
    state
        .db
        .set_acl(&body.path, body.user_id, body.group_id, &body.permission)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    audit::record(
        &state.db,
        &user,
        actions::ACL_SET,
        &body.path,
        &format!(
            "permission={} target={}",
            body.permission,
            match (body.user_id, body.group_id) {
                (Some(uid), _) => format!(
                    "user:{}",
                    state
                        .db
                        .get_user_by_id(uid)
                        .ok()
                        .flatten()
                        .map(|u| u.display_name)
                        .unwrap_or_else(|| format!("#{uid}"))
                ),
                (_, Some(gid)) => format!(
                    "group:{}",
                    state
                        .db
                        .get_group_by_id(gid)
                        .ok()
                        .flatten()
                        .map(|g| g.name)
                        .unwrap_or_else(|| format!("#{gid}"))
                ),
                (None, None) => "?".to_string(),
            }
        ),
    );
    Ok(StatusCode::CREATED)
}

pub async fn remove_acl(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Path(acl_id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    // Capture what is being removed so the audit entry stays meaningful after
    // the row is gone.
    let removed = state
        .db
        .list_acl_entries()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .into_iter()
        .find(|e| e.id == acl_id);
    state
        .db
        .remove_acl(acl_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(entry) = removed {
        audit::record(
            &state.db,
            &user,
            actions::ACL_REMOVE,
            &entry.path,
            &format!("permission={} id={}", entry.permission, entry.id),
        );
    }
    Ok(StatusCode::OK)
}

pub async fn list_acl(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    let acls = state
        .db
        .list_all_acl()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::to_value(acls).unwrap()))
}

// ── Audit log ──

/// Query parameters for [`list_audit`].
#[derive(serde::Deserialize)]
pub struct AuditQuery {
    /// Page size, 1..=500 (default 100).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Row offset into the filtered result set.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Exact action filter (e.g. `delete`, `login`, `acl.set`).
    #[serde(default)]
    pub action: Option<String>,
    /// Substring filter over username/path/detail.
    #[serde(default)]
    pub q: Option<String>,
}

/// Paged audit entries (newest first) plus the total count under the same
/// filter. Admin only.
pub async fn list_audit(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(query): Query<AuditQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let offset = query.offset.unwrap_or(0);
    let (total, entries) = state
        .db
        .list_audit(limit, offset, query.action.as_deref(), query.q.as_deref())
        .map_err(|e| {
            tracing::error!("Failed to list audit log: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(serde_json::json!({ "total": total, "entries": entries })))
}

/// Wipe the whole audit log. The wipe itself is audited (recorded after the
/// clear so the entry survives), so the log always shows who emptied it and
/// when. Admin only.
pub async fn clear_audit(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<StatusCode, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if user.is_admin != 1 {
        return Err(StatusCode::FORBIDDEN);
    }
    let n = state
        .db
        .clear_audit()
        .map_err(|e| {
            tracing::error!("Failed to clear audit log: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    audit::record(
        &state.db,
        &user,
        "audit.clear",
        "",
        &format!("cleared {n} entries"),
    );
    Ok(StatusCode::OK)
}
