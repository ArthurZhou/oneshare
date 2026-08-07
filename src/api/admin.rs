use crate::auth::session::get_user_from_cookie;
use crate::AppState;
use axum::{
    extract::{Json, Path, State},
    http::StatusCode,
};
use axum_extra::extract::cookie::CookieJar;
use std::sync::Arc;

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
    state
        .db
        .delete_group(group_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    state
        .db
        .add_user_to_group(body.user_id, body.group_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
    state
        .db
        .remove_user_from_group(body.user_id, body.group_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::OK)
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
    state
        .db
        .remove_acl(acl_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
