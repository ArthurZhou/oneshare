//! Audit logging.
//!
//! Every security-relevant event (logins, file management operations,
//! transfer-token issuance, admin changes) is appended to the `audit_log`
//! table so an admin can answer "who did what, when" from the admin panel
//! (`GET /api/admin/audit`).
//!
//! Design rules:
//! - Auditing must never break the request that produced the event: a failed
//!   insert is logged and swallowed.
//! - Events are recorded only AFTER the underlying operation succeeded —
//!   failures stay in the tracing log, not in the audit trail.
//! - Stored paths are the DISPLAY paths the acting user used (never real
//!   filesystem paths for non-admins), consistent with what the rest of the
//!   frontend sees.

use crate::auth::session::GUEST_USER_ID;
use crate::db::{Database, UserRow};

/// Record an audit event performed by `actor`.
///
/// Anonymous (guest) actors are stored with `user_id = NULL` and the name
/// `guest`, so the log stays readable even though no user row exists for
/// them. The synthetic guest row carries id `-1`.
pub fn record(db: &Database, actor: &UserRow, action: &str, path: &str, detail: &str) {
    let (user_id, username) = if actor.id == GUEST_USER_ID {
        (None, "guest".to_string())
    } else {
        (Some(actor.id), actor.display_name.clone())
    };
    if let Err(e) = db.insert_audit(user_id, &username, action, path, detail) {
        // Never fail the request because auditing failed.
        tracing::error!(
            "Failed to write audit entry (action={}, path={}): {}",
            action,
            path,
            e
        );
    }
}

/// Audit event kinds. Keep in sync with the labels/filter list in
/// `frontend/js/admin.js` (`AUDIT_ACTIONS`) and the docs.
pub mod actions {
    pub const LOGIN: &str = "login";
    pub const LOGOUT: &str = "logout";
    pub const MKDIR: &str = "mkdir";
    pub const RENAME: &str = "rename";
    pub const MOVE: &str = "move";
    pub const DELETE: &str = "delete";
    /// A text file's content was saved through the online editor.
    pub const FILE_SAVE: &str = "file.save";
    /// A read (download) transfer token was issued.
    pub const TOKEN_READ: &str = "token.read";
    /// A write (upload) transfer token was issued.
    pub const TOKEN_WRITE: &str = "token.write";
    pub const GROUP_CREATE: &str = "group.create";
    pub const GROUP_DELETE: &str = "group.delete";
    pub const GROUP_MEMBER_ADD: &str = "group.member.add";
    pub const GROUP_MEMBER_REMOVE: &str = "group.member.remove";
    pub const ACL_SET: &str = "acl.set";
    pub const ACL_REMOVE: &str = "acl.remove";
}
