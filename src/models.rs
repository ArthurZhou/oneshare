use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    /// The path the frontend should use for navigation AND file operations.
    /// For non-admin users this is a VIRTUAL path relative to the Samba-style
    /// share root (the server translates it to the real path internally, so
    /// real filesystem paths never reach the browser). For admin users it is
    /// the real path (admins see the actual tree to configure ACLs).
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirListing {
    /// Path of the current directory ("" = the root).
    pub current_path: String,
    pub parent_path: Option<String>,
    /// Whether this listing is the virtual share root (non-admin view listing
    /// ACL shares). When true, uploads/mkdir are not available and an empty
    /// root means "no shares granted". Admins always see the real root here.
    pub is_share_root: bool,
    /// Whether the current directory permits writes (uploads, mkdir, …).
    pub writable: bool,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub path: Option<String>,
}

/// Query for `GET /api/files/size` (recursive size, used by the download
/// pre-flight on browsers without the File System Access API).
#[derive(Debug, Deserialize)]
pub struct SizeQuery {
    pub path: String,
    /// Give up (and report `exceeded`) once the total passes this many bytes,
    /// so probing a wildly oversized tree costs only a partial walk. `None`
    /// walks everything.
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct SizeResponse {
    /// Bytes of every readable, non-hidden file under `path`; the file's own
    /// size when `path` names a file.
    pub size: u64,
    /// `true` when the walk stopped early because `size > limit` — `size` is
    /// then only a lower bound.
    pub exceeded: bool,
}

#[derive(Debug, Deserialize)]
pub struct FileOperation {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct MoveRequest {
    pub source: String,
    pub destination: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameRequest {
    pub path: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
pub struct MkdirRequest {
    pub path: String,
    pub name: String,
}

/// Query for `GET /api/files/content` (file detail + text preview) and
/// `GET /api/files/raw` (binary inline preview).
#[derive(Debug, Deserialize)]
pub struct ContentQuery {
    pub path: String,
}

/// Body of `PUT /api/files/content` (online text edit save).
#[derive(Debug, Deserialize)]
pub struct SaveContentRequest {
    pub path: String,
    pub content: String,
}

/// Response of `GET /api/files/content`: metadata plus (when the file is a
/// small enough text file) its full text content for inline preview/editing.
#[derive(Debug, Serialize)]
pub struct FileContentResponse {
    pub name: String,
    /// Display path (same semantics as `FileEntry::path`).
    pub path: String,
    pub size: u64,
    pub modified: String,
    pub mime_type: String,
    /// Whether the file is previewable/editable as plain text.
    pub is_text: bool,
    /// Whether the caller holds write permission on the file.
    pub writable: bool,
    /// Full text content; `None` for binary files or oversized text files.
    pub content: Option<String>,
    /// True when the file is textual but larger than the preview limit.
    pub truncated: bool,
}

/// Body of `POST /api/files/share` (create a temporary share link).
/// `items` holds one path for a single file/folder share, or several paths
/// for a multi-item "virtual root" collection share. `ttl_secs == 0` means
/// the link never expires.
#[derive(Debug, Deserialize)]
pub struct CreateShareRequest {
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub ttl_secs: u64,
}

/// Response of `POST /api/files/share`: the secret token plus the URL path
/// receivers open (`/s/{token}`, relative — the frontend prefixes its base).
#[derive(Debug, Serialize)]
pub struct ShareLinkResponse {
    pub token: String,
    pub url: String,
    pub expires_at: Option<String>,
}

/// One top-level item of a share, as shown in the owner's management list.
#[derive(Debug, Serialize)]
pub struct ShareItemInfo {
    pub name: String,
    /// Path as the OWNER sees it (never a real filesystem path for
    /// non-admins).
    pub path: String,
    pub is_dir: bool,
}

/// One share link as listed for its owner (`GET /api/files/shares`).
#[derive(Debug, Serialize)]
pub struct ShareLinkInfo {
    pub token: String,
    pub name: String,
    /// Path as the OWNER sees it (never a real filesystem path for
    /// non-admins). Empty for multi-item collections.
    pub path: String,
    pub is_dir: bool,
    /// Top-level items; non-empty only for multi-item collections.
    pub items: Vec<ShareItemInfo>,
    pub creator: String,
    pub expires_at: Option<String>,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct SetAclRequest {
    pub path: String,
    pub user_id: Option<i64>,
    pub group_id: Option<i64>,
    pub permission: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddUserToGroupRequest {
    pub user_id: i64,
    pub group_id: i64,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub token: String,
    pub expires_in: u64,
    /// The opaque shadow path (`v1.…`) this token is bound to. Transfer
    /// requests (`/file/{path}`, `/dir/{path}`) must use it; the real path
    /// never leaves the server.
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct CurrentUserResponse {
    pub user: Option<UserInfo>,
}

#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub id: i64,
    pub display_name: String,
    pub email: Option<String>,
    pub is_admin: bool,
}
