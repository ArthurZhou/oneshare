use crate::acl::{self, Permission};
use crate::auth::session::get_user_from_cookie;
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

pub async fn list(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(query): Query<ListQuery>,
) -> Result<Json<DirListing>, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let rel_path = query.path.unwrap_or_else(|| "".to_string());
    let rel_path = rel_path.trim_start_matches('/').to_string();

    acl::check_permission(&state.db, &user, &rel_path, Permission::Read)
        .map_err(|_| StatusCode::FORBIDDEN)?
        .then_some(())
        .ok_or(StatusCode::FORBIDDEN)?;

    let full_path = state.config.root_dir().join(&rel_path);
    if !full_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let mut entries = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(&full_path) {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                // Skip hidden files
                continue;
            }
            let metadata = entry.metadata().ok();
            let is_dir = entry.file_type().map(|f| f.is_dir()).unwrap_or(false);
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = metadata
                .and_then(|m| m.modified().ok())
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

            let entry_path = if rel_path.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", rel_path, name)
            };

            entries.push(FileEntry {
                name,
                path: entry_path,
                is_dir,
                size,
                modified,
                mime_type: mime,
            });
        }
    }

    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            b.is_dir.cmp(&a.is_dir) // directories first
        } else {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        }
    });

    let parent_path = if rel_path.is_empty() {
        None
    } else {
        let mut parts: Vec<&str> = rel_path.split('/').collect();
        parts.pop();
        if parts.is_empty() {
            Some("".to_string())
        } else {
            Some(parts.join("/"))
        }
    };

    Ok(Json(DirListing {
        current_path: rel_path,
        parent_path,
        entries,
    }))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(body): Json<FileOperation>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let rel_path = body.path.trim_start_matches('/');
    acl::check_permission(&state.db, &user, rel_path, Permission::Write)
        .map_err(|_| StatusCode::FORBIDDEN)?
        .then_some(())
        .ok_or(StatusCode::FORBIDDEN)?;

    let full_path = state.config.root_dir().join(rel_path);
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
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let rel_path = body.path.trim_start_matches('/');
    acl::check_permission(&state.db, &user, rel_path, Permission::Write)
        .map_err(|_| StatusCode::FORBIDDEN)?
        .then_some(())
        .ok_or(StatusCode::FORBIDDEN)?;

    let old_full = state.config.root_dir().join(rel_path);
    if !old_full.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Get parent directory
    let parent = old_full.parent().unwrap_or(state.config.root_dir());
    let new_full = parent.join(&body.new_name);

    // Sanity check on new name
    if body.new_name.contains('/') || body.new_name.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
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
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let src = body.source.trim_start_matches('/');
    let dst = body.destination.trim_start_matches('/');

    acl::check_permission(&state.db, &user, src, Permission::Write)
        .map_err(|_| StatusCode::FORBIDDEN)?;
    acl::check_permission(&state.db, &user, dst, Permission::Write)
        .map_err(|_| StatusCode::FORBIDDEN)?;

    let src_full = state.config.root_dir().join(src);
    let dst_full = state.config.root_dir().join(dst);

    if !src_full.exists() {
        return Err(StatusCode::NOT_FOUND);
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
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let rel_path = body.path.trim_start_matches('/');
    if rel_path.is_empty() {
        acl::check_permission(&state.db, &user, "", Permission::Write)
            .map_err(|_| StatusCode::FORBIDDEN)?;
    } else {
        acl::check_permission(&state.db, &user, rel_path, Permission::Write)
            .map_err(|_| StatusCode::FORBIDDEN)?;
    }

    if body.name.contains('/') || body.name.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    let base = if rel_path.is_empty() {
        state.config.root_dir().clone()
    } else {
        state.config.root_dir().join(rel_path)
    };
    let new_dir = base.join(&body.name);

    std::fs::create_dir_all(&new_dir).map_err(|e| {
        tracing::error!("Failed to mkdir: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::CREATED)
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
    let user = get_user_from_cookie(&jar, &state.db)
        .await?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let rel_path = query.path.trim_start_matches('/');

    let required = match query.op.as_str() {
        "write" => Permission::Write,
        _ => Permission::Read,
    };

    acl::check_permission(&state.db, &user, rel_path, required)
        .map_err(|_| StatusCode::FORBIDDEN)?
        .then_some(())
        .ok_or(StatusCode::FORBIDDEN)?;

    let ttl_secs = 3600u64;

    let permissions: &[&str] = if query.op == "write" {
        &["read", "write"]
    } else {
        &["read"]
    };

    let token = issue_token(
        &state.hmac_key,
        &user.id.to_string(),
        rel_path,
        permissions,
        ttl_secs,
    );

    Ok(Json(TokenResponse {
        token,
        expires_in: ttl_secs,
    }))
}
