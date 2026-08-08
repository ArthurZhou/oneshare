use crate::db::{Database, UserRow};
use axum::http::StatusCode;
use axum_extra::extract::cookie::{Cookie, CookieJar};

pub const SESSION_COOKIE: &str = "fh_session";

#[derive(Clone)]
pub struct SessionManager;

impl SessionManager {
    pub fn new() -> Self {
        SessionManager
    }

    pub fn set_session(
        &self,
        jar: CookieJar,
        db: &Database,
        user_id: i64,
        secure: bool,
    ) -> Result<CookieJar, String> {
        let session_id = db.create_session(user_id).map_err(|e| e.to_string())?;
        let jar = jar.add(
            Cookie::build((SESSION_COOKIE, session_id))
                .path("/")
                .http_only(true)
                .secure(secure)
                .same_site(axum_extra::extract::cookie::SameSite::Lax)
                .build(),
        );
        Ok(jar)
    }

    pub fn remove_session(&self, jar: CookieJar, db: &Database, session_id: &str) -> CookieJar {
        let _ = db.delete_session(session_id);
        jar.remove(Cookie::build(SESSION_COOKIE).path("/"))
    }
}

/// Helper: extract user from session cookie and AppState
pub async fn get_user_from_cookie(
    jar: &CookieJar,
    db: &Database,
) -> Result<Option<UserRow>, StatusCode> {
    let session_id = match jar.get(SESSION_COOKIE) {
        Some(c) => c.value().to_string(),
        None => return Ok(None),
    };
    db.get_session_user(&session_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
