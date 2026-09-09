use crate::db::{self, Database, UserRow, AclEntryRow};
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
    // Share sessions never touch the `sessions` table: the cookie value is
    // `share:<token>` and the share token IS the capability, resolved against
    // `share_links` on every request (so revocation/expiry is instant).
    if let Some(token) = session_id.strip_prefix(SHARE_COOKIE_PREFIX) {
        return Ok(db
            .get_share_link(token)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map(|share| share_user(token, &share.name)));
    }
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

/// Cookie value prefix identifying a share session, injected by the ShareProxy
/// middleware as `share:<token>`. Real session ids are UUIDs, so the prefix can
/// never collide with one.
pub const SHARE_COOKIE_PREFIX: &str = "share:";

/// The synthetic user representing a share-link visitor. Like the guest, it
/// is never persisted in the `users` table (no admin user/group pollution);
/// its permissions come from [`db::Database::get_share_acl_entries`], which
/// synthesizes read-only ACL entries owned by [`db::SHARE_USER_ID`] from the
/// share's own stored paths.
pub fn share_user(token: &str, title: &str) -> UserRow {
    UserRow {
        id: db::SHARE_USER_ID,
        oidc_sub: format!("{}{}", db::SHARE_SUB_PREFIX, token),
        display_name: title.to_string(),
        email: None,
        is_admin: 0,
    }
}

/// Whether `user` is a temporary share-link identity (see [`share_user`]).
pub fn is_share_user(user: &UserRow) -> bool {
    user.oidc_sub.starts_with(db::SHARE_SUB_PREFIX)
}

/// The user (or a synthetic guest/share identity) behind a request, plus the
/// groups that apply for ACL decisions.
pub struct RequestUser {
    pub user: UserRow,
    pub groups: Vec<i64>,
    /// Extra ACL entries that apply ONLY to this request's identity.
    /// Non-empty for share visitors (the synthesized read grants of their
    /// share link); always empty for real users, admins and guests.
    pub share_entries: Vec<AclEntryRow>,
}

impl RequestUser {
    /// The ACL entries that govern this request: the global table plus, for
    /// share visitors, their share's synthesized read grants. Every handler
    /// resolves permissions through this instead of the raw ACL cache, so the
    /// share grants flow through the exact same `can_access`/`user_shares`
    /// engine as everyone else's — no share-specific authorization code.
    pub fn acl_entries(
        &self,
        db: &Database,
    ) -> Result<std::sync::Arc<Vec<AclEntryRow>>, StatusCode> {
        let base = db
            .list_acl_entries_cached()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if self.share_entries.is_empty() {
            return Ok(base);
        }
        let mut all = base.as_ref().clone();
        all.extend(self.share_entries.iter().cloned());
        Ok(std::sync::Arc::new(all))
    }
}

/// Resolve the user + effective groups for a request.
///
/// - Share session (`share:<token>` cookie, injected by the ShareProxy) → the
///   synthetic share identity with NO groups (blocking the `default` fallback)
///   and the share's synthesized READ grants as extra ACL entries.
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
            if is_share_user(&user) {
                let token = user
                    .oidc_sub
                    .strip_prefix(db::SHARE_SUB_PREFIX)
                    .unwrap_or("")
                    .to_string();
                let share_entries = db
                    .get_share_acl_entries(&token)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                // No groups at all: an empty membership must NOT fall back to
                // the `default` group, or a share visitor would inherit
                // whatever the default group grants. The share grants live in
                // `share_entries` instead.
                return Ok(RequestUser {
                    user,
                    groups: Vec::new(),
                    share_entries,
                });
            }
            let groups = db
                .get_effective_groups(user.id)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            Ok(RequestUser {
                user,
                groups,
                share_entries: Vec::new(),
            })
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
                share_entries: Vec::new(),
            })
        }
    }
}
