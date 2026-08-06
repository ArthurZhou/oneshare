use crate::auth::session::get_user_from_cookie;
use crate::models::CurrentUserResponse;
use crate::AppState;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Redirect, Response},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, Debug)]
pub struct CallbackQuery {
    pub code: String,
    pub state: Option<String>,
}

/// Helper: build an HTML error page that shows the error and provides a "back to login" link
fn error_page(status: StatusCode, title: &str, message: &str) -> Response {
    let body = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Login Error</title>
<style>
  * {{ margin:0; padding:0; box-sizing:border-box; }}
  body {{ font-family: -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;
         background:#f5f5f5; display:flex; align-items:center; justify-content:center;
         min-height:100vh; }}
  .card {{ background:#fff; border-radius:12px; padding:40px; max-width:520px; width:90%;
           box-shadow:0 2px 12px rgba(0,0,0,0.1); text-align:center; }}
  h1 {{ color:#dc2626; font-size:22px; margin-bottom:16px; }}
  .msg {{ color:#4b5563; font-size:15px; line-height:1.6; margin-bottom:24px;
          word-break:break-word; }}
  .detail {{ background:#fef2f2; border:1px solid #fecaca; border-radius:8px;
             padding:16px; margin-bottom:24px; text-align:left;
             font-family:monospace; font-size:13px; color:#991b1b;
             white-space:pre-wrap; word-break:break-all; }}
  a.btn {{ display:inline-block; background:#2563eb; color:#fff; text-decoration:none;
           padding:10px 24px; border-radius:6px; font-size:14px; }}
  a.btn:hover {{ background:#1d4ed8; }}
</style>
</head>
<body>
<div class="card">
  <h1>?? {title}</h1>
  <div class="msg">{message}</div>
  <div class="detail">Status: {status_code}</div>
  <a class="btn" href="/auth/login">? Retry Login</a>
</div>
</body>
</html>"#,
        title = title,
        message = message,
        status_code = status.as_u16(),
    );

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(axum::body::Body::from(body))
        .unwrap()
}

pub async fn login(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let (url, csrf_state) = state.oidc_client.authorize_url();

    state
        .oidc_states
        .lock()
        .unwrap()
        .insert(csrf_state, ());

    Redirect::to(&url)
}

pub async fn callback(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Query(query): Query<CallbackQuery>,
) -> Response {
    tracing::debug!(
        "OIDC callback received: code={}..., state={:?}",
        &query.code.chars().take(20).collect::<String>(),
        query.state,
    );

    // Step 1: validate state parameter
    let csrf_secret = match &query.state {
        Some(s) if !s.is_empty() => s.clone(),
        Some(_) | None => {
            tracing::error!(
                "OIDC callback: missing or empty 'state' parameter. \
                 Query: code={}, state={:?}. \
                 This may indicate a CSRF attack or the OIDC provider didn't return the state.",
                query.code,
                query.state,
            );
            return error_page(
                StatusCode::BAD_REQUEST,
                "Missing State Parameter",
                "The OIDC provider did not return a valid state parameter. \
                 This could be due to a misconfigured OIDC provider or a CSRF attack. \
                 Please try logging in again.",
            );
        }
    };

    if state
        .oidc_states
        .lock()
        .unwrap()
        .remove(&csrf_secret)
        .is_none()
    {
        let active_count = state.oidc_states.lock().unwrap().len();
        tracing::error!(
            "OIDC callback: state not found for csrf_state={}. \
             This means: 1) the state expired or was already consumed, \
             2) the login was initiated from a different session, \
             3) the state was tampered with (CSRF). \
             Active stored states: {}. Current stored keys: {:?}",
            csrf_secret,
            active_count,
            state.oidc_states.lock().unwrap().keys().collect::<Vec<_>>(),
        );
        return error_page(
            StatusCode::BAD_REQUEST,
            "Invalid State",
            "The login session has expired or the state token is invalid. \
             This happens if: \
             (1) you waited too long after clicking login, \
             (2) you used the browser's back button, or \
             (3) a different browser/session initiated the login. \
             Please try logging in again.",
        );
    };

    // Step 2: exchange authorization code for tokens
    let user_info = match state
        .oidc_client
        .exchange_code(&query.code)
        .await
    {
        Ok(info) => {
            tracing::debug!(
                "Token exchange succeeded. sub={}, name={}, email={}",
                info.sub, info.name, info.email,
            );
            info
        }
        Err(e) => {
            tracing::error!("OIDC callback: token exchange failed: {}", e);
            return error_page(
                StatusCode::UNAUTHORIZED,
                "Login Failed",
                &format!(
                    "Failed to complete login with the OIDC provider. \
                     Details: {}. \
                     This may be caused by: \
                     (1) incorrect client secret, \
                     (2) redirect URI mismatch, \
                     (3) the authorization code has expired, or \
                     (4) the OIDC provider is not properly configured.",
                    e
                ),
            );
        }
    };

    // Step 3: create or update user in database
    let user = match state
        .db
        .create_user(&user_info.sub, &user_info.name, &user_info.email)
    {
        Ok(u) => {
            tracing::info!(
                "User created/updated: id={}, sub={}, display_name={}, is_admin={}",
                u.id, u.oidc_sub, u.display_name, u.is_admin,
            );
            u
        }
        Err(e) => {
            tracing::error!(
                "OIDC callback: failed to create/update user in DB. sub={}, error={}",
                user_info.sub, e,
            );
            return error_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Database Error",
                &format!(
                    "Failed to save user information to the database. \
                     Error: {}. Please try again or contact the administrator.",
                    e
                ),
            );
        }
    };

    // Step 4: create session
    let new_jar = match state
        .session_manager
        .set_session(jar, &state.db, user.id)
    {
        Ok(j) => {
            j
        }
        Err(e) => {
            tracing::error!(
                "OIDC callback: failed to create session. user_id={}, error={}",
                user.id, e,
            );
            return error_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Session Error",
                &format!(
                    "Failed to create a login session. \
                     Error: {}. Please try again.",
                    e
                ),
            );
        }
    };

    // Step 5: build redirect response with session cookie
    (new_jar, Redirect::to("/")).into_response()
}
pub async fn logout(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> impl IntoResponse {
    let session_id = jar
        .get("fh_session")
        .map(|c| c.value().to_string())
        .unwrap_or_default();

    tracing::info!("Logout: deleting session_id={}", session_id);

    let _ = state.db.delete_session(&session_id);

    let jar = state
        .session_manager
        .remove_session(jar, &state.db, &session_id);

    (jar, Redirect::to("/")).into_response()
}

pub async fn me(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<Json<CurrentUserResponse>, StatusCode> {
    let user = get_user_from_cookie(&jar, &state.db).await?;
    Ok(Json(CurrentUserResponse {
        user: user.map(|u| crate::models::UserInfo {
            id: u.id,
            display_name: u.display_name,
            email: u.email,
            is_admin: u.is_admin == 1,
        }),
    }))
}