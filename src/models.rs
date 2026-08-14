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
