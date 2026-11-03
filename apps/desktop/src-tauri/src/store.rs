//! The app-owned SQLite store: registered projects, session metadata, and settings.
//!
//! `goko.db` lives in `GOKO_HOME` beside the daemon's files. It holds no terminal or
//! process state — the daemon stays authoritative for everything live; these rows are
//! the app's own bookkeeping.

use crate::error::{BridgeError, Code};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// Schema versions applied in order; its length is the current `user_version`.
const MIGRATIONS: &[&str] = &["
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
"];

/// A registered project row. Whether `root` still exists is computed at list time by the
/// caller; the store keeps the record only.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
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

    /// Insert a project, or return the row already registered for `root`.
    pub fn add_project(&self, name: &str, root: &str) -> Result<Project, BridgeError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, root, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![uuid::Uuid::new_v4().to_string(), name, root, now_ms()],
        )
        .map_err(|error| storage("cannot add the project", error))?;
        conn.query_row(
            "SELECT id, name, root, created_at FROM projects WHERE root = ?1",
            params![root],
            project,
        )
        .map_err(|error| storage("cannot read the project", error))
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, BridgeError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare("SELECT id, name, root, created_at FROM projects ORDER BY created_at, rowid")
            .map_err(|error| storage("cannot list the projects", error))?;
        let rows = stmt
            .query_map([], project)
            .map_err(|error| storage("cannot list the projects", error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| storage("cannot list the projects", error))
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

fn project(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        root: row.get(2)?,
        created_at: row.get(3)?,
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
        again.add_project("p", "/tmp").unwrap();
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

        let first = store.add_project("one", "/tmp").unwrap();
        let second = store.add_project("two", "/var").unwrap();
        let dupe = store.add_project("one again", "/tmp").unwrap();

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
        let project = store.add_project("p", "/tmp").unwrap();
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

        let project = store.add_project("p", "/tmp").unwrap();
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
}
