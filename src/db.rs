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
            ",
        )?;

        Ok(())
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

        conn.execute(
            "INSERT INTO users (oidc_sub, display_name, email, is_admin) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(oidc_sub) DO UPDATE SET display_name=?2, email=?3",
            params![oidc_sub, display_name, email, is_admin],
        )?;

        // ????? conn ??????,?????? self.get_user_by_oidc_sub ?? Mutex ??
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

    // ── Groups ──

    pub fn list_groups(&self) -> Result<Vec<GroupRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name, description FROM groups_ ORDER BY name")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(GroupRow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
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
        })
    }

    pub fn delete_group(&self, group_id: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM user_groups WHERE group_id = ?", params![group_id])?;
        conn.execute("DELETE FROM acl_entries WHERE group_id = ?", params![group_id])?;
        conn.execute("DELETE FROM groups_ WHERE id = ?", params![group_id])?;
        Ok(())
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

    pub fn get_acl_for_path(&self, path: &str) -> Result<Vec<AclEntryRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        // Match exact path or parent paths
        let mut stmt = conn.prepare(
            "SELECT id, path, user_id, group_id, permission FROM acl_entries WHERE ?1 LIKE path || '%' ORDER BY length(path) DESC",
        )?;
        let rows = stmt
            .query_map(params![path], |row| {
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
