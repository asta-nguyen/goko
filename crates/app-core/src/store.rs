//! The app-owned SQLite store: registered projects, session metadata, and settings.
//!
//! `goko.db` lives in `GOKO_HOME` beside the daemon's files. It holds no terminal or
//! process state — the daemon stays authoritative for everything live; these rows are
//! the app's own bookkeeping.

use crate::error::{BridgeError, Code};
use protocol::message::ErrorCode;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// The permanent Default workspace id. Every database has this row; it cannot
/// be renamed or removed, and it receives the projects of a removed workspace.
pub const DEFAULT_WORKSPACE_ID: &str = "default";

/// Schema versions applied in order; its length is the current `user_version`.
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE projects (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        root TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
        agent TEXT NOT NULL,
        argv TEXT NOT NULL,
        cwd TEXT NOT NULL,
        title TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL
    );
    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
",
    "
    CREATE TABLE workspaces (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE project_workspaces (
        project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
        workspace_id TEXT NOT NULL REFERENCES workspaces(id)
    );
    INSERT INTO workspaces (id, name, created_at) VALUES ('default', 'Default', 0);
    INSERT INTO project_workspaces (project_id, workspace_id) SELECT id, 'default' FROM projects;
",
];

/// A registered project row. Whether `root` still exists is computed at list time by the
/// caller; the store keeps the record only.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
    /// The workspace this project belongs to (Default when no membership row
    /// exists, which migration and every write prevent).
    pub workspace_id: String,
    /// Unix epoch milliseconds.
    pub created_at: i64,
}

/// A named group of registered projects. The Default workspace always exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    /// Unix epoch milliseconds.
    pub created_at: i64,
}

/// The app's record of why a daemon session exists. `project_id` is `None` for sessions
/// created outside a project and nulled when their project is removed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub project_id: Option<String>,
    pub agent: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub title: String,
    /// Unix epoch milliseconds.
    pub created_at: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

/// Managed state: the store, or the open/migration error that stands in for it, so
/// storage-backed commands reject `storage` while daemon-backed commands keep working.
pub struct Storage(Result<Store, BridgeError>);

impl Storage {
    pub fn open(home: &Path) -> Self {
        Self(Store::open(home))
    }

    /// The store, or a clone of the error that kept it from opening.
    pub fn get(&self) -> Result<&Store, BridgeError> {
        self.0.as_ref().map_err(Clone::clone)
    }
}

impl Store {
    /// Open `goko.db` inside `home`, creating both as needed (dir mode 0700, file mode
    /// 0600), then apply pending migrations.
    pub fn open(home: &Path) -> Result<Self, BridgeError> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(home)
            .map_err(|error| storage("cannot create the app directory", error))?;
        // Create the file explicitly so a fresh database always has mode 0600.
        let path = home.join("goko.db");
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(|error| storage("cannot open goko.db", error))?;
        let mut conn =
            Connection::open(&path).map_err(|error| storage("cannot open goko.db", error))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| storage("cannot enable foreign keys", error))?;
        migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Insert a project into `workspace_id`, or return the row already
    /// registered for `root` with its membership unchanged. The project and
    /// its membership are written together or not at all.
    pub fn add_project(
        &self,
        name: &str,
        root: &str,
        workspace_id: &str,
    ) -> Result<Project, BridgeError> {
        let mut conn = self.lock()?;
        let tx = conn
            .transaction()
            .map_err(|error| storage("cannot add the project", error))?;
        require_workspace(&tx, workspace_id)?;
        tx.execute(
            "INSERT OR IGNORE INTO projects (id, name, root, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![uuid::Uuid::new_v4().to_string(), name, root, now_ms()],
        )
        .map_err(|error| storage("cannot add the project", error))?;
        let id: String = tx
            .query_row(
                "SELECT id FROM projects WHERE root = ?1",
                params![root],
                |row| row.get(0),
            )
            .map_err(|error| storage("cannot read the project", error))?;
        tx.execute(
            "INSERT OR IGNORE INTO project_workspaces (project_id, workspace_id) VALUES (?1, ?2)",
            params![id, workspace_id],
        )
        .map_err(|error| storage("cannot add the project", error))?;
        let project = tx
            .query_row(
                &format!("{} WHERE p.root = ?1", project_select()),
                params![root],
                project,
            )
            .map_err(|error| storage("cannot read the project", error))?;
        tx.commit()
            .map_err(|error| storage("cannot add the project", error))?;
        Ok(project)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, BridgeError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(&format!(
                "{} ORDER BY p.created_at, p.rowid",
                project_select()
            ))
            .map_err(|error| storage("cannot list the projects", error))?;
        let rows = stmt
            .query_map([], project)
            .map_err(|error| storage("cannot list the projects", error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage("cannot list the projects", error))
    }

    /// Every workspace, Default first, then creation order.
    pub fn list_workspaces(&self) -> Result<Vec<Workspace>, BridgeError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT id, name, created_at FROM workspaces \
                 ORDER BY (id = '{DEFAULT_WORKSPACE_ID}') DESC, created_at, rowid"
            ))
            .map_err(|error| storage("cannot list the workspaces", error))?;
        let rows = stmt
            .query_map([], workspace)
            .map_err(|error| storage("cannot list the workspaces", error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage("cannot list the workspaces", error))
    }

    /// Create a named workspace. Names are trimmed, nonempty, and unique
    /// ignoring ASCII case.
    pub fn create_workspace(&self, name: &str) -> Result<Workspace, BridgeError> {
        let conn = self.lock()?;
        let name = checked_workspace_name(&conn, name, None)?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO workspaces (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, name, now_ms()],
        )
        .map_err(|error| storage("cannot create the workspace", error))?;
        read_workspace(&conn, &id)
    }

    /// Rename a named workspace. The Default workspace cannot be renamed.
    pub fn rename_workspace(&self, id: &str, name: &str) -> Result<Workspace, BridgeError> {
        let conn = self.lock()?;
        let existing = read_workspace(&conn, id)?;
        if existing.id == DEFAULT_WORKSPACE_ID {
            return Err(invalid("the Default workspace cannot be renamed"));
        }
        let name = checked_workspace_name(&conn, name, Some(id))?;
        conn.execute(
            "UPDATE workspaces SET name = ?1 WHERE id = ?2",
            params![name, id],
        )
        .map_err(|error| storage("cannot rename the workspace", error))?;
        read_workspace(&conn, id)
    }

    /// Remove a named workspace, moving every member project to Default in
    /// the same transaction. No project or session row is deleted.
    pub fn remove_workspace(&self, id: &str) -> Result<(), BridgeError> {
        let mut conn = self.lock()?;
        if id == DEFAULT_WORKSPACE_ID {
            return Err(invalid("the Default workspace cannot be removed"));
        }
        let tx = conn
            .transaction()
            .map_err(|error| storage("cannot remove the workspace", error))?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?1)",
                params![id],
                |row| row.get(0),
            )
            .map_err(|error| storage("cannot remove the workspace", error))?;
        if !exists {
            return Err(not_found(format!("no workspace '{id}' exists")));
        }
        tx.execute(
            "UPDATE project_workspaces SET workspace_id = ?1 WHERE workspace_id = ?2",
            params![DEFAULT_WORKSPACE_ID, id],
        )
        .map_err(|error| storage("cannot remove the workspace", error))?;
        tx.execute("DELETE FROM workspaces WHERE id = ?1", params![id])
            .map_err(|error| storage("cannot remove the workspace", error))?;
        tx.commit()
            .map_err(|error| storage("cannot remove the workspace", error))?;
        Ok(())
    }

    /// Move one project's membership. A missing project or workspace id is an
    /// error and changes nothing else.
    pub fn move_project(&self, project_id: &str, workspace_id: &str) -> Result<(), BridgeError> {
        let mut conn = self.lock()?;
        let tx = conn
            .transaction()
            .map_err(|error| storage("cannot move the project", error))?;
        let project_exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
                params![project_id],
                |row| row.get(0),
            )
            .map_err(|error| storage("cannot move the project", error))?;
        if !project_exists {
            return Err(not_found(format!(
                "no project '{project_id}' is registered"
            )));
        }
        require_workspace(&tx, workspace_id)?;
        tx.execute(
            "INSERT INTO project_workspaces (project_id, workspace_id) VALUES (?1, ?2) \
             ON CONFLICT (project_id) DO UPDATE SET workspace_id = excluded.workspace_id",
            params![project_id, workspace_id],
        )
        .map_err(|error| storage("cannot move the project", error))?;
        tx.commit()
            .map_err(|error| storage("cannot move the project", error))?;
        Ok(())
    }

    /// True when a row was deleted. `sessions.project_id` is nulled by the foreign key.
    pub fn remove_project(&self, id: &str) -> Result<bool, BridgeError> {
        self.lock()?
            .execute("DELETE FROM projects WHERE id = ?1", params![id])
            .map(|deleted| deleted > 0)
            .map_err(|error| storage("cannot remove the project", error))
    }

    pub fn insert_session_meta(&self, meta: &SessionMeta) -> Result<(), BridgeError> {
        let argv = serde_json::to_string(&meta.argv)
            .map_err(|error| storage("cannot encode the session argv", error))?;
        self.lock()?
            .execute(
                "INSERT INTO sessions (id, project_id, agent, argv, cwd, title, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    meta.session_id,
                    meta.project_id,
                    meta.agent,
                    argv,
                    meta.cwd,
                    meta.title,
                    meta.created_at,
                ],
            )
            .map(drop)
            .map_err(|error| storage("cannot record the session", error))
    }

    pub fn list_session_meta(&self) -> Result<Vec<SessionMeta>, BridgeError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, project_id, agent, argv, cwd, title, created_at FROM sessions \
                 ORDER BY created_at, rowid",
            )
            .map_err(|error| storage("cannot list the session metadata", error))?;
        let rows = stmt
            .query_map([], session_meta)
            .map_err(|error| storage("cannot list the session metadata", error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage("cannot list the session metadata", error))
    }

    /// Idempotently remove the app-owned metadata for a finished daemon session.
    pub fn delete_session_meta(&self, id: &str) -> Result<(), BridgeError> {
        self.lock()?
            .execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map(drop)
            .map_err(|error| storage("cannot delete the session metadata", error))
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, BridgeError> {
        self.lock()?
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage("cannot read the setting", error))
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), BridgeError> {
        self.lock()?
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) \
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(drop)
            .map_err(|error| storage("cannot write the setting", error))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, BridgeError> {
        self.conn
            .lock()
            .map_err(|_| BridgeError::new(Code::Storage, "the store lock is poisoned"))
    }
}

/// Apply every migration past `user_version`, each in its own transaction.
fn migrate(conn: &mut Connection) -> Result<(), BridgeError> {
    let version: usize = conn
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|error| storage("cannot read the goko.db schema version", error))?
        .try_into()
        .unwrap_or_default();
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version) {
        let tx = conn
            .transaction()
            .map_err(|error| storage("cannot migrate goko.db", error))?;
        tx.execute_batch(migration)
            .and_then(|()| tx.execute_batch(&format!("PRAGMA user_version = {}", index + 1)))
            .map_err(|error| storage("cannot migrate goko.db", error))?;
        tx.commit()
            .map_err(|error| storage("cannot migrate goko.db", error))?;
    }
    Ok(())
}

/// The project row plus its workspace membership, one row per project.
fn project_select() -> String {
    format!(
        "SELECT p.id, p.name, p.root, p.created_at, COALESCE(pw.workspace_id, '{DEFAULT_WORKSPACE_ID}') \
         FROM projects p LEFT JOIN project_workspaces pw ON pw.project_id = p.id"
    )
}

/// One workspace row by id; `not_found` when no row has it.
fn read_workspace(conn: &Connection, id: &str) -> Result<Workspace, BridgeError> {
    conn.query_row(
        "SELECT id, name, created_at FROM workspaces WHERE id = ?1",
        params![id],
        workspace,
    )
    .optional()
    .map_err(|error| storage("cannot read the workspace", error))?
    .ok_or_else(|| not_found(format!("no workspace '{id}' exists")))
}

/// Trim and validate a workspace name, rejecting blanks and names already
/// taken ignoring ASCII case. `exclude` skips the row being renamed.
fn checked_workspace_name(
    conn: &Connection,
    name: &str,
    exclude: Option<&str>,
) -> Result<String, BridgeError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("the workspace name is empty"));
    }
    let taken: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE name = ?1 COLLATE NOCASE AND id IS NOT ?2)",
            params![name, exclude],
            |row| row.get(0),
        )
        .map_err(|error| storage("cannot validate the workspace name", error))?;
    if taken {
        return Err(invalid(format!(
            "a workspace named '{name}' already exists"
        )));
    }
    Ok(name.to_string())
}

/// `not_found` when `workspace_id` names no workspace.
fn require_workspace(conn: &Connection, workspace_id: &str) -> Result<(), BridgeError> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?1)",
            params![workspace_id],
            |row| row.get(0),
        )
        .map_err(|error| storage("cannot read the workspace", error))?;
    if exists {
        Ok(())
    } else {
        Err(not_found(format!("no workspace '{workspace_id}' exists")))
    }
}

fn project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        root: row.get(2)?,
        created_at: row.get(3)?,
        workspace_id: row.get(4)?,
    })
}

fn workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: row.get(2)?,
    })
}

fn session_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMeta> {
    let argv: String = row.get(3)?;
    Ok(SessionMeta {
        session_id: row.get(0)?,
        project_id: row.get(1)?,
        agent: row.get(2)?,
        argv: serde_json::from_str(&argv).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        cwd: row.get(4)?,
        title: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

fn storage(what: &str, error: impl std::fmt::Display) -> BridgeError {
    BridgeError::new(Code::Storage, format!("{what}: {error}"))
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::InvalidArgument), message)
}

fn not_found(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::NotFound), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::os::unix::fs::PermissionsExt;

    fn open(dir: &TempDir) -> Store {
        Store::open(dir.path()).unwrap()
    }

    fn meta(session_id: &str, project_id: Option<&str>) -> SessionMeta {
        SessionMeta {
            session_id: session_id.into(),
            project_id: project_id.map(str::to_owned),
            agent: "shell".into(),
            argv: vec!["/bin/zsh".into()],
            cwd: "/tmp".into(),
            title: String::new(),
            created_at: 1,
        }
    }

    #[test]
    fn open_creates_goko_db_mode_0600_and_reopening_is_idempotent() {
        let dir = TempDir::new("store-open");
        let store = open(&dir);
        let conn = store.conn.lock().unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        drop(conn);
        drop(store);

        assert_eq!(version, MIGRATIONS.len() as i64);
        assert_eq!(
            dir.path()
                .join("goko.db")
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        // Reopening applies no migration and keeps the data.
        let again = open(&dir);
        again
            .add_project("p", "/tmp", DEFAULT_WORKSPACE_ID)
            .unwrap();
        assert_eq!(again.list_projects().unwrap().len(), 1);
        assert_eq!(again.list_projects().unwrap()[0].name, "p");
    }

    #[test]
    fn a_store_that_cannot_open_keeps_the_error() {
        let dir = TempDir::new("store-bad");
        std::fs::create_dir_all(dir.path().join("goko.db")).unwrap();

        let storage = Storage::open(dir.path());

        match storage.get() {
            Err(error) => assert_eq!(error.code, Code::Storage),
            Ok(_) => panic!("expected the open error to be retained"),
        }
    }

    #[test]
    fn add_then_list_round_trips_and_duplicate_root_returns_the_original_row() {
        let dir = TempDir::new("store-add");
        let store = open(&dir);

        let first = store
            .add_project("one", "/tmp", DEFAULT_WORKSPACE_ID)
            .unwrap();
        let second = store
            .add_project("two", "/var", DEFAULT_WORKSPACE_ID)
            .unwrap();
        let dupe = store
            .add_project("one again", "/tmp", DEFAULT_WORKSPACE_ID)
            .unwrap();

        assert_eq!(dupe.id, first.id);
        assert_eq!(dupe.name, "one", "the existing row wins, name unchanged");
        assert_eq!(
            store.list_projects().unwrap(),
            vec![first.clone(), second.clone()]
        );
    }

    #[test]
    fn removing_a_project_nulls_session_project_id() {
        let dir = TempDir::new("store-rm");
        let store = open(&dir);
        let project = store
            .add_project("p", "/tmp", DEFAULT_WORKSPACE_ID)
            .unwrap();
        store
            .insert_session_meta(&meta("s1", Some(&project.id)))
            .unwrap();
        store.insert_session_meta(&meta("s2", None)).unwrap();

        assert!(store.remove_project(&project.id).unwrap());
        assert!(!store.remove_project(&project.id).unwrap(), "already gone");

        let metas = store.list_session_meta().unwrap();
        assert_eq!(metas[0].project_id, None);
        assert_eq!(metas[1].project_id, None);
    }

    #[test]
    fn settings_and_session_meta_round_trip() {
        let dir = TempDir::new("store-set");
        let store = open(&dir);

        assert_eq!(store.get_setting("missing").unwrap(), None);
        store.set_setting("k", "v1").unwrap();
        store.set_setting("k", "v2").unwrap();
        assert_eq!(store.get_setting("k").unwrap().as_deref(), Some("v2"));

        let project = store
            .add_project("p", "/tmp", DEFAULT_WORKSPACE_ID)
            .unwrap();
        let meta = meta("s1", Some(&project.id));
        store.insert_session_meta(&meta).unwrap();
        assert_eq!(store.list_session_meta().unwrap(), vec![meta]);
    }

    #[test]
    fn deleting_session_meta_is_idempotent_and_a_storage_failure_is_retryable() {
        let dir = TempDir::new("store-delete-session");
        let store = open(&dir);
        store.insert_session_meta(&meta("s1", None)).unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER deny_session_delete BEFORE DELETE ON sessions \
                 BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
            )
            .unwrap();

        assert_eq!(
            store.delete_session_meta("s1").unwrap_err().code,
            Code::Storage
        );
        assert_eq!(store.list_session_meta().unwrap(), vec![meta("s1", None)]);

        store
            .conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER deny_session_delete")
            .unwrap();
        store.delete_session_meta("s1").unwrap();
        store.delete_session_meta("s1").unwrap();
        assert!(store.list_session_meta().unwrap().is_empty());
    }

    #[test]
    fn migrating_an_existing_database_assigns_every_project_to_default() {
        let dir = TempDir::new("store-migrate-workspaces");
        let path = dir.path().join("goko.db");
        // A version-1 database: the first migration only, with real rows.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.execute_batch("PRAGMA user_version = 1").unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, root, created_at) VALUES ('p1', 'old', '/tmp/old', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, project_id, agent, argv, cwd, title, created_at) \
             VALUES ('s1', 'p1', 'shell', '[\"/bin/zsh\"]', '/tmp/old', '', 2)",
            [],
        )
        .unwrap();
        drop(conn);

        let store = Store::open(dir.path()).unwrap();

        let projects = store.list_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "p1");
        assert_eq!(projects[0].workspace_id, DEFAULT_WORKSPACE_ID);
        let workspaces = store.list_workspaces().unwrap();
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, DEFAULT_WORKSPACE_ID);
        assert_eq!(workspaces[0].name, "Default");
        // Session associations survive the migration untouched.
        assert_eq!(
            store.list_session_meta().unwrap()[0].project_id.as_deref(),
            Some("p1")
        );
    }

    #[test]
    fn a_fresh_database_has_only_the_default_workspace() {
        let dir = TempDir::new("store-fresh-workspaces");
        let store = open(&dir);

        assert_eq!(
            store.list_workspaces().unwrap(),
            vec![Workspace {
                id: DEFAULT_WORKSPACE_ID.into(),
                name: "Default".into(),
                created_at: 0,
            }]
        );
    }

    #[test]
    fn workspaces_create_rename_and_reject_blank_or_duplicate_names() {
        let dir = TempDir::new("store-workspace-names");
        let store = open(&dir);

        let work = store.create_workspace("  Work  ").unwrap();
        assert_eq!(work.name, "Work", "the name is trimmed");
        let other = store.create_workspace("Play").unwrap();

        assert_eq!(
            store.create_workspace("   ").unwrap_err().code,
            Code::Daemon(ErrorCode::InvalidArgument)
        );
        assert_eq!(
            store.create_workspace("work").unwrap_err().code,
            Code::Daemon(ErrorCode::InvalidArgument),
            "names are unique ignoring case"
        );
        assert_eq!(
            store.create_workspace("Default").unwrap_err().code,
            Code::Daemon(ErrorCode::InvalidArgument),
            "the Default name is taken"
        );

        let renamed = store.rename_workspace(&work.id, "Work two").unwrap();
        assert_eq!(renamed.name, "Work two");
        assert_eq!(
            store.rename_workspace(&work.id, "PLAY").unwrap_err().code,
            Code::Daemon(ErrorCode::InvalidArgument),
            "renaming into another workspace's name is refused"
        );
        assert_eq!(
            store
                .rename_workspace(DEFAULT_WORKSPACE_ID, "Home")
                .unwrap_err()
                .code,
            Code::Daemon(ErrorCode::InvalidArgument)
        );
        assert_eq!(
            store.rename_workspace("missing", "Home").unwrap_err().code,
            Code::Daemon(ErrorCode::NotFound)
        );

        let names: Vec<String> = store
            .list_workspaces()
            .unwrap()
            .into_iter()
            .map(|workspace| workspace.name)
            .collect();
        assert_eq!(names, vec!["Default", "Work two", "Play"]);
        assert_ne!(other.id, work.id);
    }

    #[test]
    fn adding_a_duplicate_root_keeps_its_original_workspace() {
        let dir = TempDir::new("store-dupe-workspace");
        let store = open(&dir);
        let work = store.create_workspace("Work").unwrap();

        let first = store.add_project("p", "/tmp", &work.id).unwrap();
        assert_eq!(first.workspace_id, work.id);
        let again = store
            .add_project("p again", "/tmp", DEFAULT_WORKSPACE_ID)
            .unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(
            again.workspace_id, work.id,
            "re-adding never moves the project"
        );
        assert_eq!(
            store.add_project("q", "/var", "missing").unwrap_err().code,
            Code::Daemon(ErrorCode::NotFound)
        );
    }

    #[test]
    fn moving_a_project_changes_only_its_membership() {
        let dir = TempDir::new("store-move-project");
        let store = open(&dir);
        let work = store.create_workspace("Work").unwrap();
        let one = store
            .add_project("one", "/tmp", DEFAULT_WORKSPACE_ID)
            .unwrap();
        let two = store
            .add_project("two", "/var", DEFAULT_WORKSPACE_ID)
            .unwrap();

        store.move_project(&one.id, &work.id).unwrap();

        let projects = store.list_projects().unwrap();
        assert_eq!(projects[0].id, one.id);
        assert_eq!(projects[0].workspace_id, work.id);
        assert_eq!(projects[1].id, two.id);
        assert_eq!(projects[1].workspace_id, DEFAULT_WORKSPACE_ID);
        assert_eq!(projects[0].root, one.root);
        assert_eq!(projects[0].created_at, one.created_at);

        assert_eq!(
            store.move_project("missing", &work.id).unwrap_err().code,
            Code::Daemon(ErrorCode::NotFound)
        );
        assert_eq!(
            store.move_project(&one.id, "missing").unwrap_err().code,
            Code::Daemon(ErrorCode::NotFound)
        );
        assert_eq!(
            store.list_projects().unwrap()[0].workspace_id,
            work.id,
            "failed moves change nothing"
        );
    }

    #[test]
    fn removing_a_workspace_moves_its_projects_to_default_atomically() {
        let dir = TempDir::new("store-remove-workspace");
        let store = open(&dir);
        let work = store.create_workspace("Work").unwrap();
        let project = store.add_project("p", "/tmp", &work.id).unwrap();
        store
            .insert_session_meta(&meta("s1", Some(&project.id)))
            .unwrap();

        // A forced failure keeps the transfer and the delete together.
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER deny_workspace_delete BEFORE DELETE ON workspaces \
                 BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
            )
            .unwrap();
        assert_eq!(
            store.remove_workspace(&work.id).unwrap_err().code,
            Code::Storage
        );
        assert_eq!(
            store.list_projects().unwrap()[0].workspace_id,
            work.id,
            "the transfer rolled back with the delete"
        );
        assert!(store
            .list_workspaces()
            .unwrap()
            .iter()
            .any(|w| w.id == work.id));
        store
            .conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER deny_workspace_delete")
            .unwrap();

        store.remove_workspace(&work.id).unwrap();

        assert_eq!(
            store.list_projects().unwrap()[0].workspace_id,
            DEFAULT_WORKSPACE_ID
        );
        assert!(
            !store
                .list_workspaces()
                .unwrap()
                .iter()
                .any(|w| w.id == work.id),
            "the workspace is gone"
        );
        assert_eq!(
            store.list_session_meta().unwrap()[0].project_id.as_deref(),
            Some(project.id.as_str()),
            "no session row was touched"
        );
        assert_eq!(
            store
                .remove_workspace(DEFAULT_WORKSPACE_ID)
                .unwrap_err()
                .code,
            Code::Daemon(ErrorCode::InvalidArgument)
        );
        assert_eq!(
            store.remove_workspace("missing").unwrap_err().code,
            Code::Daemon(ErrorCode::NotFound)
        );
    }

    #[test]
    fn removing_a_project_cascades_its_membership() {
        let dir = TempDir::new("store-cascade-membership");
        let store = open(&dir);
        let work = store.create_workspace("Work").unwrap();
        let project = store.add_project("p", "/tmp", &work.id).unwrap();

        assert!(store.remove_project(&project.id).unwrap());

        let memberships: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM project_workspaces", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(memberships, 0);
    }
}
