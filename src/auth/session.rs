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

/// The synthetic user id used for unauthenticated requests. Real users always
/// have positive ids from the `users` table, so -1 can never collide.
pub const GUEST_USER_ID: i64 = -1;

/// A synthetic "guest" user representing an unauthenticated request. Its
/// permissions come entirely from the `guest` group via ACLs; it is never
/// persisted in the `users` table and can never be an admin.
pub fn guest_user() -> UserRow {
    UserRow {
        id: GUEST_USER_ID,
        oidc_sub: "guest".to_string(),
        display_name: "Guest".to_string(),
        email: None,
        is_admin: 0,
    }
}

/// The user (or the synthetic guest) behind a request, plus the groups that
/// apply for ACL decisions.
pub struct RequestUser {
    pub user: UserRow,
    pub groups: Vec<i64>,
}

/// Resolve the user + effective groups for a request.
///
/// - Valid session → the real user and their explicit groups (or the reserved
///   `default` group when they have none).
/// - No/invalid session → the synthetic guest user and the reserved `guest`
///   group, so unauthenticated visitors are governed by guest-group ACLs
///   instead of being rejected with 401 (the frontend no longer redirects to
///   the login page — guests simply see whatever the `guest` group grants).
pub async fn get_request_user(
    jar: &CookieJar,
    db: &Database,
) -> Result<RequestUser, StatusCode> {
    match get_user_from_cookie(jar, db).await? {
        Some(user) => {
            let groups = db
                .get_effective_groups(user.id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(RequestUser { user, groups })
        }
        None => {
            let groups = db
                .get_guest_group_id()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .map(|id| vec![id])
                .unwrap_or_default();
            Ok(RequestUser {
                user: guest_user(),
                groups,
            })
        }
    }
}
