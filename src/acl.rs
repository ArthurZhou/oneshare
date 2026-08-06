use crate::db::{Database, UserRow};

#[derive(Debug, Clone, PartialEq)]
pub enum Permission {
    Read,
    Write,
    Admin,
}

impl Permission {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Permission::Read),
            "write" => Some(Permission::Write),
            "admin" => Some(Permission::Admin),
            _ => None,
        }
    }

    pub fn covers(&self, required: &Permission) -> bool {
        match self {
            Permission::Admin => true,
            Permission::Write => matches!(required, Permission::Read | Permission::Write),
            Permission::Read => matches!(required, Permission::Read),
        }
    }
}

pub fn check_permission(
    db: &Database,
    user: &UserRow,
    path: &str,
    required: Permission,
) -> Result<bool, String> {
    // Admin users bypass ACL
    if user.is_admin == 1 {
        return Ok(true);
    }

    let acl_entries = db
        .get_acl_for_path(path)
        .map_err(|e| format!("DB error: {}", e))?;

    if acl_entries.is_empty() {
        // No ACL = allow read, deny write/admin by default
        return Ok(matches!(required, Permission::Read));
    }

    let user_groups = db
        .get_user_groups(user.id)
        .map_err(|e| format!("DB error: {}", e))?;

    for entry in &acl_entries {
        let perm = Permission::from_str(&entry.permission)
            .unwrap_or(Permission::Read);

        let matches_user = entry.user_id == Some(user.id);
        let matches_group = entry
            .group_id
            .map(|gid| user_groups.contains(&gid))
            .unwrap_or(false);

        if matches_user || matches_group {
            return Ok(perm.covers(&required));
        }
    }

    // If ACLs exist but none matched, deny
    Ok(false)
}
