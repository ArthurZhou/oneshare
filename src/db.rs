use rusqlite::{Connection, params};
use std::sync::Mutex;

pub struct Database {
    pub conn: Mutex<Connection>,
}

impl Database {
    pub fn new(path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        let db = Database {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                oidc_sub TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL,
                email TEXT,
                is_admin INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id),
                expires_at TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS groups_ (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                description TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS user_groups (
                user_id INTEGER NOT NULL REFERENCES users(id),
                group_id INTEGER NOT NULL REFERENCES groups_(id),
                PRIMARY KEY (user_id, group_id)
            );

            CREATE TABLE IF NOT EXISTS acl_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL,
                user_id INTEGER REFERENCES users(id),
                group_id INTEGER REFERENCES groups_(id),
                permission TEXT NOT NULL CHECK(permission IN ('read','write','admin')),
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                CHECK (user_id IS NOT NULL OR group_id IS NOT NULL)
            );

            CREATE INDEX IF NOT EXISTS idx_acl_path ON acl_entries(path);
            CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);

            CREATE TABLE IF NOT EXISTS audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
                user_id INTEGER,
                username TEXT NOT NULL,
                action TEXT NOT NULL,
                path TEXT NOT NULL DEFAULT '',
                detail TEXT NOT NULL DEFAULT ''
            );

            CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit_log(ts);
            CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_log(action);
            ",
        )?;

        // Seed the reserved "default" group: it defines the permissions of
        // every user who has not been assigned to any explicit group. It is
        // idempotent so existing databases pick it up on the next startup.
        conn.execute(
            "INSERT OR IGNORE INTO groups_ (name, description)
             VALUES ('default', 'Fallback group: applies to users not in any explicit group')",
            [],
        )?;

        // Seed the reserved "guest" group: it defines the permissions of
        // unauthenticated (not logged in) visitors, so an admin can grant
        // public read access with a single ACL.
        conn.execute(
            "INSERT OR IGNORE INTO groups_ (name, description)
             VALUES ('guest', 'Fallback group: applies to unauthenticated (not logged in) visitors')",
            [],
        )?;

        Ok(())
    }

    /// The reserved group name that holds permissions for unassigned users.
    pub const DEFAULT_GROUP_NAME: &'static str = "default";

    /// The reserved group name that holds permissions for unauthenticated
    /// (not logged in) visitors.
    pub const GUEST_GROUP_NAME: &'static str = "guest";

    /// ID of the reserved `default` group, if it exists.
    pub fn get_default_group_id(&self) -> Result<Option<i64>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let id = conn.query_row(
            "SELECT id FROM groups_ WHERE name = ?1",
            params![Self::DEFAULT_GROUP_NAME],
            |r| r.get(0),
        );
        match id {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// ID of the reserved `guest` group, if it exists.
    pub fn get_guest_group_id(&self) -> Result<Option<i64>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let id = conn.query_row(
            "SELECT id FROM groups_ WHERE name = ?1",
            params![Self::GUEST_GROUP_NAME],
            |r| r.get(0),
        );
        match id {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The groups that actually apply to a user for ACL decisions.
    ///
    /// A user with explicit group memberships uses exactly those groups. A
    /// user with **no** memberships is treated as a member of the reserved
    /// `default` group, so admins can grant base permissions to every
    /// unassigned user with a single ACL.
    pub fn get_effective_groups(&self, user_id: i64) -> Result<Vec<i64>, rusqlite::Error> {
        let groups = self.get_user_groups(user_id)?;
        if !groups.is_empty() {
            return Ok(groups);
        }
        // No explicit groups → fall back to the `default` group.
        Ok(self
            .get_default_group_id()?
            .map(|id| vec![id])
            .unwrap_or_default())
    }

    pub fn get_user_by_oidc_sub(
        &self,
        oidc_sub: &str,
    ) -> Result<Option<UserRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, oidc_sub, display_name, email, is_admin FROM users WHERE oidc_sub = ?",
        )?;
        let mut rows = stmt.query(params![oidc_sub])?;
        if let Some(row) = rows.next()? {
            Ok(Some(UserRow {
                id: row.get(0)?,
                oidc_sub: row.get(1)?,
                display_name: row.get(2)?,
                email: row.get(3)?,
                is_admin: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn create_user(
        &self,
        oidc_sub: &str,
        display_name: &str,
        email: &str,
    ) -> Result<UserRow, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        // Check if this is the very first user -> make admin
        let user_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        let is_admin = if user_count == 0 { 1 } else { 0 };

        // Two-step update-or-insert. We deliberately avoid
        // `INSERT ... ON CONFLICT DO UPDATE`: SQLite allocates a rowid on the
        // insert attempt even when the upsert resolves via the UPDATE branch,
        // so every re-login of an existing user would burn a fresh id and
        // inflate the `users.id` sequence (observed ids 1, 5, 34 ...).
        let existing: Option<i64> = match conn.query_row(
            "SELECT id FROM users WHERE oidc_sub = ?1",
            params![oidc_sub],
            |r| r.get(0),
        ) {
            Ok(id) => Some(id),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e),
        };

        if let Some(id) = existing {
            conn.execute(
                "UPDATE users SET display_name = ?1, email = ?2 WHERE id = ?3",
                params![display_name, email, id],
            )?;
        } else {
            conn.execute(
                "INSERT INTO users (oidc_sub, display_name, email, is_admin) VALUES (?1, ?2, ?3, ?4)",
                params![oidc_sub, display_name, email, is_admin],
            )?;
        }

        // ??? conn ???,?????? self.get_user_by_oidc_sub ?? Mutex ??
        let mut stmt = conn.prepare(
            "SELECT id, oidc_sub, display_name, email, is_admin FROM users WHERE oidc_sub = ?",
        )?;
        let mut rows = stmt.query(params![oidc_sub])?;
        if let Some(row) = rows.next()? {
            Ok(UserRow {
                id: row.get(0)?,
                oidc_sub: row.get(1)?,
                display_name: row.get(2)?,
                email: row.get(3)?,
                is_admin: row.get(4)?,
            })
        } else {
            Err(rusqlite::Error::QueryReturnedNoRows)
        }
    }
    pub fn create_session(&self, user_id: i64) -> Result<String, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let session_id = uuid::Uuid::new_v4().to_string();
        let expires = chrono::Utc::now() + chrono::Duration::hours(24);
        conn.execute(
            "INSERT INTO sessions (id, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![session_id, user_id, expires.to_rfc3339()],
        )?;
        Ok(session_id)
    }

    pub fn get_session_user(&self, session_id: &str) -> Result<Option<UserRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT u.id, u.oidc_sub, u.display_name, u.email, u.is_admin
             FROM sessions s JOIN users u ON s.user_id = u.id
             WHERE s.id = ? AND s.expires_at > datetime('now')",
        )?;
        let mut rows = stmt.query(params![session_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(UserRow {
                id: row.get(0)?,
                oidc_sub: row.get(1)?,
                display_name: row.get(2)?,
                email: row.get(3)?,
                is_admin: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM sessions WHERE id = ?", params![session_id])?;
        Ok(())
    }

    /// Remove sessions whose `expires_at` is in the past (called on a timer
    /// by the server; sessions are never read after expiry anyway, this just
    /// keeps the table from growing without bound). Returns the number of
    /// deleted rows.
    pub fn delete_expired_sessions(&self) -> Result<usize, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM sessions WHERE expires_at <= datetime('now')",
            [],
        )?;
        Ok(n)
    }

    // ── Groups ──

    pub fn list_groups(&self) -> Result<Vec<GroupRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT g.id, g.name, g.description, COUNT(ug.user_id)
             FROM groups_ g
             LEFT JOIN user_groups ug ON ug.group_id = g.id
             GROUP BY g.id ORDER BY g.name",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(GroupRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    member_count: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn create_group(&self, name: &str, description: &str) -> Result<GroupRow, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO groups_ (name, description) VALUES (?1, ?2)",
            params![name, description],
        )?;
        let id = conn.last_insert_rowid();
        Ok(GroupRow {
            id,
            name: name.into(),
            description: Some(description.into()),
            member_count: 0,
        })
    }

    pub fn delete_group(&self, group_id: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM user_groups WHERE group_id = ?", params![group_id])?;
        conn.execute("DELETE FROM acl_entries WHERE group_id = ?", params![group_id])?;
        conn.execute("DELETE FROM groups_ WHERE id = ?", params![group_id])?;
        Ok(())
    }

    pub fn get_group_by_id(&self, group_id: i64) -> Result<Option<GroupRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT g.id, g.name, g.description, COUNT(ug.user_id)
             FROM groups_ g
             LEFT JOIN user_groups ug ON ug.group_id = g.id
             WHERE g.id = ?
             GROUP BY g.id",
        )?;
        let mut rows = stmt.query(params![group_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(GroupRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                member_count: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Look up a group by its (unique) name.
    pub fn get_group_by_name(&self, name: &str) -> Result<Option<GroupRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT g.id, g.name, g.description, COUNT(ug.user_id)
             FROM groups_ g
             LEFT JOIN user_groups ug ON ug.group_id = g.id
             WHERE g.name = ?
             GROUP BY g.id",
        )?;
        let mut rows = stmt.query(params![name])?;
        if let Some(row) = rows.next()? {
            Ok(Some(GroupRow {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                member_count: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn add_user_to_group(&self, user_id: i64, group_id: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO user_groups (user_id, group_id) VALUES (?1, ?2)",
            params![user_id, group_id],
        )?;
        Ok(())
    }

    pub fn remove_user_from_group(
        &self,
        user_id: i64,
        group_id: i64,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM user_groups WHERE user_id = ? AND group_id = ?",
            params![user_id, group_id],
        )?;
        Ok(())
    }

    pub fn get_user_groups(&self, user_id: i64) -> Result<Vec<i64>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT group_id FROM user_groups WHERE user_id = ?")?;
        let ids = stmt
            .query_map(params![user_id], |row| row.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        Ok(ids)
    }

    pub fn list_group_members(&self, group_id: i64) -> Result<Vec<UserRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT u.id, u.oidc_sub, u.display_name, u.email, u.is_admin
             FROM user_groups ug
             JOIN users u ON u.id = ug.user_id
             WHERE ug.group_id = ? ORDER BY u.display_name",
        )?;
        let rows = stmt
            .query_map(params![group_id], |row| {
                Ok(UserRow {
                    id: row.get(0)?,
                    oidc_sub: row.get(1)?,
                    display_name: row.get(2)?,
                    email: row.get(3)?,
                    is_admin: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_users(&self) -> Result<Vec<UserRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, oidc_sub, display_name, email, is_admin FROM users ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(UserRow {
                    id: row.get(0)?,
                    oidc_sub: row.get(1)?,
                    display_name: row.get(2)?,
                    email: row.get(3)?,
                    is_admin: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── ACL ──

    pub fn set_acl(
        &self,
        path: &str,
        user_id: Option<i64>,
        group_id: Option<i64>,
        permission: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO acl_entries (path, user_id, group_id, permission) VALUES (?1, ?2, ?3, ?4)",
            params![path, user_id, group_id, permission],
        )?;
        Ok(())
    }

    pub fn remove_acl(&self, acl_id: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM acl_entries WHERE id = ?", params![acl_id])?;
        Ok(())
    }

    /// All ACL entries. Exact/ancestor matching and path normalization are done
    /// in memory by the permission engine so LIKE wildcards in real file names
    /// can't leak permissions and stored paths may use either leading-slash form.
    pub fn list_acl_entries(&self) -> Result<Vec<AclEntryRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, path, user_id, group_id, permission FROM acl_entries ORDER BY length(path) DESC, id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AclEntryRow {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    user_id: row.get(2)?,
                    group_id: row.get(3)?,
                    permission: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_all_acl(&self) -> Result<Vec<AclEntryRowFull>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT a.id, a.path, a.user_id, a.group_id, a.permission,
                    u.display_name as user_name, g.name as group_name
             FROM acl_entries a
             LEFT JOIN users u ON a.user_id = u.id
             LEFT JOIN groups_ g ON a.group_id = g.id
             ORDER BY a.path, a.id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(AclEntryRowFull {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    user_id: row.get(2)?,
                    group_id: row.get(3)?,
                    permission: row.get(4)?,
                    user_name: row.get(5)?,
                    group_name: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn get_user_by_id(&self, user_id: i64) -> Result<Option<UserRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, oidc_sub, display_name, email, is_admin FROM users WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![user_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(UserRow {
                id: row.get(0)?,
                oidc_sub: row.get(1)?,
                display_name: row.get(2)?,
                email: row.get(3)?,
                is_admin: row.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    // ── Audit log ──

    /// Append an audit event. `ts` is filled in by SQLite (local wall-clock
    /// time, so the admin log reads naturally). Callers route through
    /// [`crate::audit::record`], which swallows errors — auditing must never
    /// fail the request that produced the event.
    pub fn insert_audit(
        &self,
        user_id: Option<i64>,
        username: &str,
        action: &str,
        path: &str,
        detail: &str,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO audit_log (user_id, username, action, path, detail)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![user_id, username, action, path, detail],
        )?;
        Ok(())
    }

    /// A page of audit entries (newest first) plus the total count under the
    /// same filter. `action`, when given, is an exact match; `q` is a
    /// substring match (`LIKE %q%`) over username/path/detail.
    pub fn list_audit(
        &self,
        limit: u32,
        offset: u32,
        action: Option<&str>,
        q: Option<&str>,
    ) -> Result<(i64, Vec<AuditRow>), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();

        // Build the WHERE clause dynamically; values go through bound params.
        let mut where_clauses: Vec<String> = Vec::new();
        let mut bind: Vec<String> = Vec::new();
        if let Some(a) = action.filter(|a| !a.is_empty()) {
            where_clauses.push("action = ?".to_string());
            bind.push(a.to_string());
        }
        if let Some(q) = q.map(str::trim).filter(|q| !q.is_empty()) {
            where_clauses.push("(username LIKE ? OR path LIKE ? OR detail LIKE ?)".to_string());
            let like = format!("%{}%", q);
            bind.push(like.clone());
            bind.push(like.clone());
            bind.push(like);
        }
        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        let total: i64 = {
            let sql = format!("SELECT COUNT(*) FROM audit_log {}", where_sql);
            // rusqlite infers parameter types from usage; LIKE needs TEXT.
            let typed: Vec<&dyn rusqlite::ToSql> =
                bind.iter().map(|b| b as &dyn rusqlite::ToSql).collect();
            conn.query_row(&sql, typed.as_slice(), |r| r.get(0))?
        };

        let sql = format!(
            "SELECT id, ts, user_id, username, action, path, detail
             FROM audit_log {} ORDER BY id DESC LIMIT {} OFFSET {}",
            where_sql, limit.max(1), offset.max(0)
        );
        let typed: Vec<&dyn rusqlite::ToSql> =
            bind.iter().map(|b| b as &dyn rusqlite::ToSql).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(typed.as_slice(), |row| {
                Ok(AuditRow {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    user_id: row.get(2)?,
                    username: row.get(3)?,
                    action: row.get(4)?,
                    path: row.get(5)?,
                    detail: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok((total, rows))
    }

    /// Delete every audit entry. Returns the number of removed rows.
    pub fn clear_audit(&self) -> Result<usize, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM audit_log", [])
    }

    /// Delete audit entries older than `days` days. `days == 0` means "keep
    /// forever" and removes nothing. Called at startup and by the hourly
    /// cleanup task so the table cannot grow without bound.
    pub fn prune_audit(&self, days: u32) -> Result<usize, rusqlite::Error> {
        if days == 0 {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        // `days` is a validated u32, so interpolating it into the modifier is safe.
        let n = conn.execute(
            &format!(
                "DELETE FROM audit_log WHERE ts < datetime('now', 'localtime', '-{days} days')"
            ),
            [],
        )?;
        Ok(n)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UserRow {
    pub id: i64,
    pub oidc_sub: String,
    pub display_name: String,
    pub email: Option<String>,
    pub is_admin: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GroupRow {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub member_count: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AclEntryRow {
    pub id: i64,
    pub path: String,
    pub user_id: Option<i64>,
    pub group_id: Option<i64>,
    pub permission: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AclEntryRowFull {
    pub id: i64,
    pub path: String,
    pub user_id: Option<i64>,
    pub group_id: Option<i64>,
    pub permission: String,
    pub user_name: Option<String>,
    pub group_name: Option<String>,
}

/// One audit-log entry (see the `audit_log` table).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditRow {
    pub id: i64,
    /// Local wall-clock time at insert (`datetime('now','localtime')`).
    pub ts: String,
    /// Acting user's id; `None`/`-1` for anonymous (guest) actors.
    pub user_id: Option<i64>,
    /// Display-name snapshot — survives user deletion.
    pub username: String,
    /// Event kind, e.g. `login`, `delete`, `acl.set` (see src/audit.rs).
    pub action: String,
    /// Display path the event applies to (may be empty for non-file events).
    pub path: String,
    /// Free-form extra info (e.g. move destination, trash note).
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> Database {
        let dir = std::env::temp_dir().join(format!("oneshare-audit-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Database::new(dir.join("test.db").to_str().unwrap()).unwrap()
    }

    #[test]
    fn audit_insert_list_and_filter() {
        let db = temp_db("basic");
        db.insert_audit(Some(1), "alice", "login", "", "").unwrap();
        db.insert_audit(None, "guest", "token.read", "docs/a.txt", "op=read")
            .unwrap();
        db.insert_audit(Some(2), "bob", "delete", "docs/a.txt", "permanently deleted")
            .unwrap();

        let (total, rows) = db.list_audit(10, 0, None, None).unwrap();
        assert_eq!(total, 3);
        // Newest first.
        assert_eq!(rows[0].action, "delete");
        assert_eq!(rows[2].username, "alice");

        // Exact-action filter.
        let (total, rows) = db.list_audit(10, 0, Some("token.read"), None).unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].path, "docs/a.txt");

        // Substring filter over username/path/detail.
        let (total, _) = db.list_audit(10, 0, None, Some("alice")).unwrap();
        assert_eq!(total, 1);
        let (total, _) = db.list_audit(10, 0, None, Some("a.txt")).unwrap();
        assert_eq!(total, 2);

        // Pagination.
        let (total, rows) = db.list_audit(1, 1, None, None).unwrap();
        assert_eq!(total, 3);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, "token.read");
    }

    #[test]
    fn audit_clear_and_prune() {
        let db = temp_db("prune");
        db.insert_audit(Some(1), "alice", "login", "", "").unwrap();
        db.insert_audit(Some(2), "bob", "delete", "x.txt", "").unwrap();
        // Age one row artificially so retention has something to remove.
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE audit_log SET ts = datetime('now', 'localtime', '-30 days') WHERE id = 1",
                [],
            )
            .unwrap();
        }
        // days == 0 keeps everything.
        assert_eq!(db.prune_audit(0).unwrap(), 0);
        assert_eq!(db.prune_audit(365).unwrap(), 0);

        let removed = db.prune_audit(7).unwrap();
        assert_eq!(removed, 1);
        // Only bob's recent entry survives.
        let (total, rows) = db.list_audit(10, 0, None, None).unwrap();
        assert_eq!(total, 1);
        assert_eq!(rows[0].username, "bob");

        assert_eq!(db.clear_audit().unwrap(), 1);
        let (total, _) = db.list_audit(10, 0, None, None).unwrap();
        assert_eq!(total, 0);
    }

    #[test]
    fn audit_table_exists_on_migrated_db() {
        // Database::new runs migrate(); a pre-existing DB without the table
        // must also pick it up on next startup (CREATE TABLE IF NOT EXISTS).
        let db = temp_db("migrate");
        let (total, _) = db.list_audit(10, 0, None, None).unwrap();
        assert_eq!(total, 0);
    }
}
