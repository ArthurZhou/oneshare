use crate::db::{AclEntryRow, Database, UserRow};

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

/// Canonical form used for ACL matching: no leading/trailing slashes.
/// The root path is the empty string.
pub fn normalize_path(path: &str) -> String {
    path.trim_matches('/').to_string()
}

/// Whether an ACL entry with stored path `entry_path` applies to `path`
/// (the entry is the path itself or one of its ancestors).
fn entry_applies(entry_path: &str, path: &str) -> bool {
    let ep = normalize_path(entry_path);
    let p = normalize_path(path);
    if ep.is_empty() {
        // A root-level ACL applies to every path.
        return true;
    }
    p == ep || p.starts_with(&format!("{}/", ep))
}

/// Pure permission check against pre-fetched ACL entries and group memberships.
///
/// - Admin users bypass ACLs entirely.
/// - When no ACL applies to the path, read is allowed (write/admin denied).
/// - The most specific (longest) matching ACL wins; its first matching
///   user/group rule decides, otherwise access is denied.
pub fn can_access(
    user: &UserRow,
    user_groups: &[i64],
    acl_entries: &[AclEntryRow],
    path: &str,
    required: &Permission,
) -> bool {
    if user.is_admin == 1 {
        return true;
    }

    let mut applicable: Vec<&AclEntryRow> = acl_entries
        .iter()
        .filter(|e| entry_applies(&e.path, path))
        .collect();
    // Most specific (longest path) entry wins; tie-break by id for determinism.
    applicable.sort_by(|a, b| b.path.len().cmp(&a.path.len()).then(a.id.cmp(&b.id)));

    if applicable.is_empty() {
        // No ACL = allow read, deny write/admin by default
        return matches!(required, Permission::Read);
    }

    for entry in applicable {
        let perm = Permission::from_str(&entry.permission).unwrap_or(Permission::Read);

        let matches_user = entry.user_id == Some(user.id);
        let matches_group = entry
            .group_id
            .map(|gid| user_groups.contains(&gid))
            .unwrap_or(false);

        if matches_user || matches_group {
            return perm.covers(required);
        }
    }

    // ACLs exist for this path but none matched the user.
    false
}

pub fn check_permission(
    db: &Database,
    user: &UserRow,
    path: &str,
    required: Permission,
) -> Result<bool, String> {
    let acl_entries = db
        .list_acl_entries()
        .map_err(|e| format!("DB error: {}", e))?;
    let user_groups = db
        .get_user_groups(user.id)
        .map_err(|e| format!("DB error: {}", e))?;
    Ok(can_access(user, &user_groups, &acl_entries, path, &required))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: i64, is_admin: bool) -> UserRow {
        UserRow {
            id,
            oidc_sub: format!("sub-{id}"),
            display_name: format!("user-{id}"),
            email: None,
            is_admin: if is_admin { 1 } else { 0 },
        }
    }

    fn acl(id: i64, path: &str, user_id: Option<i64>, group_id: Option<i64>, permission: &str) -> AclEntryRow {
        AclEntryRow {
            id,
            path: path.to_string(),
            user_id,
            group_id,
            permission: permission.to_string(),
        }
    }

    fn check(user: &UserRow, groups: &[i64], entries: &[AclEntryRow], path: &str, required: Permission) -> bool {
        can_access(user, groups, entries, path, &required)
    }

    #[test]
    fn admin_bypasses_acl() {
        let entries = vec![acl(1, "secret", None, Some(1), "read")];
        assert!(check(&user(1, true), &[], &entries, "secret", Permission::Write));
    }

    #[test]
    fn no_acl_default_read_only() {
        assert!(check(&user(1, false), &[], &[], "anything", Permission::Read));
        assert!(!check(&user(1, false), &[], &[], "anything", Permission::Write));
    }

    #[test]
    fn exact_and_ancestor_match() {
        let entries = vec![acl(1, "project", None, Some(10), "read")];
        let groups = [10];
        // Exact path matches.
        assert!(check(&user(1, false), &groups, &entries, "project", Permission::Read));
        // Child path inherits the parent ACL.
        assert!(check(&user(1, false), &groups, &entries, "project/sub/file.txt", Permission::Read));
        // User not in the group is denied.
        assert!(!check(&user(2, false), &[], &entries, "project", Permission::Read));
    }

    #[test]
    fn sibling_prefix_does_not_leak() {
        // Regression: `LIKE path || '%'` used to let an ACL on /project match
        // /project-secret too (any string prefix). A rule must only ever apply
        // to its own subtree.
        let entries = vec![acl(1, "project", None, Some(10), "read")];
        let groups = [10];
        // Write on a sibling with no ACL of its own is denied by the default.
        assert!(!check(&user(1, false), &groups, &entries, "project-secret", Permission::Write));

        // The leak: /project-secret has its own rule (group 20). The /project
        // rule (group 10) must NOT grant group 10 read access to it.
        let entries2 = vec![
            acl(1, "project", None, Some(10), "read"),
            acl(2, "project-secret", None, Some(20), "read"),
        ];
        assert!(!check(&user(1, false), &[10], &entries2, "project-secret", Permission::Read));
        // The owner group still gets access through the correct rule.
        assert!(check(&user(2, false), &[20], &entries2, "project-secret", Permission::Read));
    }

    #[test]
    fn leading_slash_normalized() {
        // Regression: ACL stored as "/project" must match the listing path
        // "project" (entries carry no leading slash) and vice versa.
        let entries = vec![acl(1, "/project", None, Some(10), "read")];
        let groups = [10];
        assert!(check(&user(1, false), &groups, &entries, "project", Permission::Read));
        assert!(check(&user(1, false), &groups, &entries, "project/sub", Permission::Read));

        let entries2 = vec![acl(1, "project", None, Some(10), "read")];
        assert!(check(&user(1, false), &groups, &entries2, "/project", Permission::Read));
    }

    #[test]
    fn root_acl_applies_to_all() {
        let entries = vec![acl(1, "", None, Some(10), "read")];
        let groups = [10];
        assert!(check(&user(1, false), &groups, &entries, "", Permission::Read));
        assert!(check(&user(1, false), &groups, &entries, "deep/nested/file", Permission::Read));
        assert!(!check(&user(2, false), &[], &entries, "deep/nested/file", Permission::Read));
    }

    #[test]
    fn most_specific_entry_wins() {
        // /secret grants group write; /secret alone grants only read via user.
        let entries = vec![
            acl(1, "secret", Some(5), None, "read"),
            acl(2, "secret/sub", None, Some(10), "write"),
        ];
        // Member of group 10 inherits write from the specific entry.
        assert!(check(&user(1, false), &[10], &entries, "secret/sub", Permission::Write));
        // Non-member (user 5 has only read on the parent) cannot write.
        assert!(!check(&user(5, false), &[], &entries, "secret/sub", Permission::Write));
        // User 5 can still read via the parent rule.
        assert!(check(&user(5, false), &[], &entries, "secret/sub", Permission::Read));
    }

    #[test]
    fn permission_hierarchy() {
        assert!(Permission::Read.covers(&Permission::Read));
        assert!(!Permission::Read.covers(&Permission::Write));
        assert!(Permission::Write.covers(&Permission::Read));
        assert!(Permission::Write.covers(&Permission::Write));
        assert!(Permission::Admin.covers(&Permission::Read));
        assert!(Permission::Admin.covers(&Permission::Write));
    }
}
