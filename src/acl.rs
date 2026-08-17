use crate::db::{AclEntryRow, UserRow};

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
/// - When no ACL applies to the path, access is **denied** (fail-closed): an
///   unconfigured path is not readable or writable by anyone.
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
        // No ACL configured for this path: deny by default.
        return false;
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

/// A "share" visible at the web root (Samba-style virtual root).
///
/// Every ACL entry the user can read becomes a root-level folder whose name is
/// the **leaf** segment of the real path and whose `real_path` is the full
/// configured path. For example, an ACL on `/nested/public2` shows up as a
/// top-level folder named `public2`; the `/nested` part above it is hidden
/// from the user, exactly like Samba hides everything above a share mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub virtual_name: String,
    pub real_path: String,
}

/// Compute the shares visible at the virtual root for `user`.
///
/// `user_groups` must already include the `default` group for unassigned
/// users (see [`Database::get_effective_groups`]). Admin users see every
/// ACL-configured path. Leaf-name collisions (e.g. `/a/data` and `/b/data`
/// both granting read) are disambiguated like filesystems do: `data`,
/// `data-2`, `data-3`, …
pub fn user_shares(
    user: &UserRow,
    user_groups: &[i64],
    acl_entries: &[AclEntryRow],
) -> Vec<Share> {
    let is_admin = user.is_admin == 1;
    let mut shares: Vec<Share> = Vec::new();
    let mut used: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for e in acl_entries {
        if !is_admin {
            let matches_user = e.user_id == Some(user.id);
            let matches_group = e
                .group_id
                .map(|gid| user_groups.contains(&gid))
                .unwrap_or(false);
            if !matches_user && !matches_group {
                continue;
            }
        }

        let real = normalize_path(&e.path);
        // A root-level ACL (`path == ""`) grants the whole tree; it is not a
        // named share — callers fall back to listing the real root instead.
        if real.is_empty() {
            continue;
        }

        let base = real
            .rsplit('/')
            .next()
            .unwrap_or(&real)
            .to_string();
        let n = used.entry(base.clone()).or_insert(0);
        // Filesystem-style collision naming: `data`, `data-2`, `data-3`, …
        let name = if *n == 0 {
            base.clone()
        } else {
            format!("{}-{}", base, *n + 1)
        };
        *n += 1;

        shares.push(Share {
            virtual_name: name,
            real_path: real,
        });
    }
    shares
}

/// Whether a path segment is safe to use as a filesystem component.
///
/// Rejects empty, `.`/`..` (traversal), and any segment containing a backslash
/// (`\` is a path separator on Windows, so it could smuggle `..` past the
/// `/`-based ACL prefix matching) or a NUL byte.
pub fn is_safe_segment(seg: &str) -> bool {
    !seg.is_empty() && seg != "." && seg != ".." && !seg.contains('\\') && !seg.contains('\0')
}

/// Validate and normalize a path supplied by the frontend, rejecting any
/// segment that is `.`/`..` or contains a backslash/NUL. Returns the
/// normalized relative path (no leading/trailing slashes) or `None` if unsafe.
///
/// This is the guard that stops a user from escaping their share boundary:
/// ACL matching is prefix-based, so an untrusted `..` (or `\` on Windows)
/// would otherwise let a low-privilege user read/write sibling directories.
pub fn sanitize_path(path: &str) -> Option<String> {
    if path.contains('\0') || path.contains('\\') {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        if seg.is_empty() {
            continue;
        }
        if seg == "." || seg == ".." {
            return None;
        }
        out.push(seg);
    }
    Some(out.join("/"))
}

/// Map a virtual browse path back to a real path using the user's shares.
///
/// The virtual root (`""`) has no real path and returns `None`. Any other
/// path's first segment names a share; the rest is appended underneath it.
/// Every segment below the share is validated with [`is_safe_segment`] so a
/// malicious `..`/`\` in the request cannot escape the share boundary.
pub fn resolve_virtual(virtual_path: &str, shares: &[Share]) -> Option<String> {
    let vp = virtual_path.trim_matches('/');
    if vp.is_empty() {
        return None;
    }
    let mut segs = vp.split('/');
    let first = segs.next()?;
    if !is_safe_segment(first) {
        return None;
    }
    let share = shares.iter().find(|s| s.virtual_name == first)?;
    let rest: Vec<&str> = segs.collect();
    // Reject traversal/backslash in the path below the share.
    if rest.iter().any(|s| !is_safe_segment(s)) {
        return None;
    }
    if rest.is_empty() {
        sanitize_path(&share.real_path)
    } else {
        sanitize_path(&format!("{}/{}", share.real_path, rest.join("/")))
    }
}

/// Map a real path back to the display path the user sees — the inverse of
/// [`resolve_virtual`]. Users who see the real tree (admins, root-ACL
/// holders) get the real path back unchanged; everyone else is mapped through
/// their shares, with the most specific (longest real prefix) share winning.
/// Returns `None` when the real path is not inside any of the user's shares.
pub fn display_path_for(
    user: &UserRow,
    user_groups: &[i64],
    acl_entries: &[AclEntryRow],
    real_path: &str,
) -> Option<String> {
    let real = normalize_path(real_path);
    if user.is_admin == 1 || user_has_root_read(user, user_groups, acl_entries) {
        return Some(real);
    }
    let mut best: Option<(usize, String)> = None; // (real prefix len, display)
    for share in user_shares(user, user_groups, acl_entries) {
        let sr = normalize_path(&share.real_path);
        let cand = if real == sr {
            Some((sr.len(), share.virtual_name.clone()))
        } else if real.starts_with(&format!("{}/", sr)) {
            let rest = &real[sr.len() + 1..];
            Some((sr.len(), format!("{}/{}", share.virtual_name, rest)))
        } else {
            None
        };
        if let Some(c) = cand {
            if best.as_ref().map(|(l, _)| c.0 > *l).unwrap_or(true) {
                best = Some(c);
            }
        }
    }
    best.map(|(_, d)| d)
}

/// Whether a root-level ACL (`path == ""`) grants `user` read access to the
/// whole tree. Used to decide whether the virtual root should fall back to
/// the real root listing.
pub fn user_has_root_read(
    user: &UserRow,
    user_groups: &[i64],
    acl_entries: &[AclEntryRow],
) -> bool {
    acl_entries.iter().any(|e| {
        normalize_path(&e.path).is_empty()
            && (user.is_admin == 1
                || e.user_id == Some(user.id)
                || e.group_id
                    .map(|gid| user_groups.contains(&gid))
                    .unwrap_or(false))
    })
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
    fn no_acl_default_deny() {
        // Fail-closed: an unconfigured path is not readable or writable.
        assert!(!check(&user(1, false), &[], &[], "anything", Permission::Read));
        assert!(!check(&user(1, false), &[], &[], "anything", Permission::Write));
        // Even a read grant on an unrelated sibling does not leak.
        let entries = vec![acl(1, "public", None, Some(10), "read")];
        assert!(!check(&user(1, false), &[10], &entries, "secret", Permission::Read));
    }

    #[test]
    fn default_group_grants_unassigned_users() {
        // user has NO explicit groups → effective groups = [default(99)].
        let entries = vec![acl(1, "public", None, Some(99), "read")];
        let default_only = vec![99];
        assert!(check(&user(1, false), &default_only, &entries, "public", Permission::Read));
        assert!(!check(&user(1, false), &default_only, &entries, "public", Permission::Write));
        // A user WITH explicit groups no longer inherits the default group.
        assert!(!check(&user(1, false), &[10], &entries, "public", Permission::Read));
    }

    #[test]
    fn shares_are_acl_leaf_names() {
        // Group A has /public(r), /private(rw), /nested/public2.
        let entries = vec![
            acl(1, "public", None, Some(10), "read"),
            acl(2, "private", None, Some(10), "write"),
            acl(3, "nested/public2", None, Some(10), "read"),
        ];
        let shares = user_shares(&user(1, false), &[10], &entries);
        let names: Vec<&str> = shares.iter().map(|s| s.virtual_name.as_str()).collect();
        assert_eq!(names, vec!["public", "private", "public2"]);
        // real paths map back to the full configured path.
        let p2 = shares.iter().find(|s| s.virtual_name == "public2").unwrap();
        assert_eq!(p2.real_path, "nested/public2");
    }

    #[test]
    fn shares_ignore_entries_that_do_not_match() {
        let entries = vec![
            acl(1, "public", None, Some(10), "read"),   // matches user 1 (group 10)
            acl(2, "private", None, Some(20), "read"),  // no match
        ];
        let shares = user_shares(&user(1, false), &[10], &entries);
        assert_eq!(shares.len(), 1);
        assert_eq!(shares[0].virtual_name, "public");
    }

    #[test]
    fn shares_collisions_disambiguated() {
        let entries = vec![
            acl(1, "a/data", None, Some(10), "read"),
            acl(2, "b/data", None, Some(10), "read"),
        ];
        let shares = user_shares(&user(1, false), &[10], &entries);
        let names: Vec<&str> = shares.iter().map(|s| s.virtual_name.as_str()).collect();
        assert_eq!(names, vec!["data", "data-2"]);
    }

    #[test]
    fn admin_sees_all_acl_shares() {
        let entries = vec![
            acl(1, "public", None, Some(10), "read"),
            acl(2, "private", None, Some(20), "write"),
        ];
        // Admin has no groups but still sees every ACL-configured share.
        let shares = user_shares(&user(1, true), &[], &entries);
        assert_eq!(shares.len(), 2);
    }

    #[test]
    fn resolve_virtual_maps_to_real() {
        let shares = vec![
            Share { virtual_name: "public".into(), real_path: "public".into() },
            Share { virtual_name: "public2".into(), real_path: "nested/public2".into() },
        ];
        assert_eq!(resolve_virtual("", &shares), None);
        assert_eq!(resolve_virtual("public", &shares), Some("public".to_string()));
        assert_eq!(
            resolve_virtual("public2/sub/deep.txt", &shares),
            Some("nested/public2/sub/deep.txt".to_string())
        );
        assert_eq!(resolve_virtual("unknown", &shares), None);
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

    #[test]
    fn sanitize_rejects_traversal() {
        assert_eq!(sanitize_path("a/b/c.txt").as_deref(), Some("a/b/c.txt"));
        assert_eq!(sanitize_path("").as_deref(), Some(""));
        assert_eq!(sanitize_path("/a//b/").as_deref(), Some("a/b"));
        assert_eq!(sanitize_path("a/../b"), None);
        assert_eq!(sanitize_path("../secret"), None);
        assert_eq!(sanitize_path("a\\..\\b"), None);
        assert_eq!(sanitize_path("a/./b"), None);
        assert_eq!(sanitize_path("a/\0/b"), None);
    }

    #[test]
    fn resolve_virtual_blocks_traversal_escape() {
        let shares = vec![Share { virtual_name: "public2".into(), real_path: "nested/public2".into() }];
        // Normal navigation inside the share still works.
        assert_eq!(
            resolve_virtual("public2/sub/file.txt", &shares),
            Some("nested/public2/sub/file.txt".to_string())
        );
        // `..` cannot escape the share boundary to read sibling/root paths.
        assert_eq!(resolve_virtual("public2/../../secret", &shares), None);
        assert_eq!(resolve_virtual("public2/..", &shares), None);
        assert_eq!(resolve_virtual("public2/./x", &shares), None);
        // Backslash cannot smuggle traversal on Windows.
        assert_eq!(resolve_virtual("public2/..\\secret", &shares), None);
        assert_eq!(resolve_virtual("public2/..\\..\\etc", &shares), None);
    }

    #[test]
    fn traversal_does_not_grant_access() {
        // A user with read on nested/public2 must NOT be able to read a sibling
        // via `..`: the ACL prefix check alone would pass, but resolve_virtual
        // now refuses the path before the ACL engine ever sees it.
        let entries = vec![acl(1, "nested/public2", None, Some(10), "read")];
        let groups = [10];
        let shares = user_shares(&user(1, false), &groups, &entries);
        // The traversal path resolves to None -> caller returns 404/403.
        assert_eq!(resolve_virtual("public2/../../secret", &shares), None);
        assert_eq!(resolve_virtual("public2/..", &shares), None);
    }

    #[test]
    fn display_path_roundtrips_shares() {
        let u = user(1, false);
        let entries = vec![
            acl(1, "nested/public2", Some(1), None, "read"),
            acl(2, "docs", Some(1), None, "read"),
        ];
        let shares = user_shares(&u, &[], &entries);
        // resolve_virtual: display -> real
        assert_eq!(resolve_virtual("public2/a.txt", &shares).as_deref(), Some("nested/public2/a.txt"));
        assert_eq!(resolve_virtual("docs/sub/b.md", &shares).as_deref(), Some("docs/sub/b.md"));
        // display_path_for: real -> display (inverse)
        assert_eq!(display_path_for(&u, &[], &entries, "nested/public2/a.txt").as_deref(), Some("public2/a.txt"));
        assert_eq!(display_path_for(&u, &[], &entries, "nested/public2").as_deref(), Some("public2"));
        assert_eq!(display_path_for(&u, &[], &entries, "docs/sub/b.md").as_deref(), Some("docs/sub/b.md"));
        assert_eq!(display_path_for(&u, &[], &entries, "elsewhere/x"), None);
        // Most specific share wins (nested deeper real path)
        let entries2 = vec![
            acl(1, "nested/public2", Some(1), None, "read"),
            acl(2, "nested/public2/deep", Some(1), None, "read"),
        ];
        assert_eq!(
            display_path_for(&u, &[], &entries2, "nested/public2/deep/f.txt").as_deref(),
            Some("deep/f.txt")
        );
        // Admin sees the real path itself
        assert_eq!(display_path_for(&user(2, true), &[], &entries, "nested/public2/a.txt").as_deref(), Some("nested/public2/a.txt"));
    }


}
