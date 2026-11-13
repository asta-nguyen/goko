//! Concept search: a derived, per-checkout SQLite FTS5 index of the working
//! tree's text chunks, ranked with BM25.
//!
//! The index lives in `GOKO_HOME/search/<fnv1a64(canonical checkout)>.db` and
//! is derived data: deleting it only costs a rebuild. A full pass lists the
//! checkout with `git_view::file_list` and reads each file through
//! `git_view::file`, so the skip rules (binary, non-UTF-8, over 1 MiB, a
//! symlink resolving outside) are the ones the editor already uses. A sync
//! re-evaluates only the paths `git status` names, which is what makes a file
//! added or edited a moment ago searchable without waiting for a full pass.
//!
//! The index holds each chunk's words and line range, not its text: the
//! full-text table is contentless, which keeps the file at about a third of
//! the source's size instead of more than twice it. `search` ranks in the
//! index and reads the preview line of each result from the working tree.
//!
//! Index writes happen on the caller's indexer thread; `search` opens the
//! index read-only and never writes.

use crate::error::{BridgeError, Code};
use crate::git_view::{
    self, GitViewChanges, GitViewSearch, GitViewSearchMatch, SEARCH_PREVIEW_CHARS,
};
use protocol::message::ErrorCode;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Files a full pass indexes at most; a sync adds a file only below this.
pub const MAX_INDEXED_FILES: usize = 20_000;
const CHUNK_LINES: usize = 40;
const CHUNK_BYTES: usize = 4096;
const CHUNK_OVERLAP: usize = 4;
const CONCEPT_RESULT_CAP: usize = 50;
/// 2: `chunks_fts` became contentless. An index of another version is not
/// read, and the next full pass replaces it.
const SCHEMA_VERSION: i64 = 2;
/// Files per transaction of a full pass. Each commit makes the full-text
/// index merge what it holds, so few large ones build faster than many small.
const BATCH_FILES: usize = 1000;
/// Ranked chunks a query looks at for its `CONCEPT_RESULT_CAP` results.
/// Chunks overlap, so two can name the same line; this leaves room for that.
const RANKED_CHUNKS: usize = 1000;
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const SCHEMA: &str = "
CREATE TABLE meta (id INTEGER PRIMARY KEY CHECK (id = 1), path TEXT NOT NULL);
CREATE TABLE files (path TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL);
CREATE TABLE chunks (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL
);
CREATE INDEX chunks_path ON chunks(path);
CREATE VIRTUAL TABLE chunks_fts USING fts5(
    path, idents, body, content='', contentless_delete=1, tokenize='porter unicode61'
);
";

/// One chunk of a file: 1-based inclusive line range and its text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chunk {
    pub start_line: u64,
    pub end_line: u64,
    pub text: String,
}

/// What a full pass indexed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RefreshStats {
    /// Files with chunks after the pass.
    pub files: usize,
    /// The listing was longer than the pass limit.
    pub truncated: bool,
}

/// What a working-tree sync changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SyncStats {
    pub reindexed: usize,
    pub removed: usize,
    pub unchanged: usize,
}

#[derive(Clone, Copy)]
enum OpenMode {
    CreateOrRebuild,
    ReadOnly,
    Write,
}

enum PathOutcome {
    Reindexed,
    Removed,
    Unchanged,
    Skipped,
}

/// Canonicalize a checkout and require a directory.
fn canonical_checkout(checkout: &Path) -> Result<PathBuf, BridgeError> {
    let canonical = std::fs::canonicalize(checkout)
        .map_err(|error| invalid(format!("the checkout is unavailable: {error}")))?;
    if !canonical.is_dir() {
        return Err(invalid("the checkout is not a directory"));
    }
    Ok(canonical)
}

/// The index file for one checkout: one file per canonical path.
pub fn index_path(home: &Path, checkout: &Path) -> PathBuf {
    let canonical = canonical_checkout(checkout).unwrap_or_else(|_| checkout.to_path_buf());
    home.join("search").join(format!(
        "{:016x}.db",
        fnv1a64(canonical.as_os_str().as_bytes())
    ))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Run one full pass over the checkout.
pub fn refresh(
    home: &Path,
    checkout: &Path,
    progress: impl FnMut(usize, usize) -> bool,
) -> Result<RefreshStats, BridgeError> {
    refresh_with(home, checkout, MAX_INDEXED_FILES, progress)
}

fn refresh_with(
    home: &Path,
    checkout: &Path,
    limit: usize,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<RefreshStats, BridgeError> {
    let checkout = canonical_checkout(checkout)?;
    let mut conn = match open_index(home, &checkout, OpenMode::CreateOrRebuild)? {
        Some(conn) => conn,
        None => return Err(invalid("the search index could not be opened")),
    };
    let listed = git_view::file_list(&checkout)?;
    let truncated = listed.len() > limit;
    let listed: Vec<String> = listed.into_iter().take(limit).collect();
    let total = listed.len();
    let mut listed_set: HashSet<String> = HashSet::with_capacity(total);
    let mut tx = conn.transaction().map_err(db)?;
    let mut done = 0usize;
    for path in &listed {
        listed_set.insert(path.clone());
        reevaluate_path(&tx, &checkout, path)?;
        done += 1;
        if !progress(done, total) {
            tx.commit().map_err(db)?;
            return Ok(RefreshStats {
                files: indexed_files(&conn)?,
                truncated,
            });
        }
        if done.is_multiple_of(BATCH_FILES) {
            tx.commit().map_err(db)?;
            tx = conn.transaction().map_err(db)?;
        }
    }
    let stored: Vec<String> = {
        let mut stmt = tx.prepare("SELECT path FROM files").map_err(db)?;
        let rows = stmt.query_map([], |row| row.get(0)).map_err(db)?;
        rows.collect::<Result<_, _>>().map_err(db)?
    };
    for path in stored {
        if !listed_set.contains(&path) {
            delete_path(&tx, &path)?;
        }
    }
    tx.commit().map_err(db)?;
    Ok(RefreshStats {
        files: indexed_files(&conn)?,
        truncated,
    })
}

/// Re-evaluate the paths `git status` names, without listing or statting the
/// rest of the checkout.
pub fn sync_changed(
    home: &Path,
    checkout: &Path,
    stop: &AtomicBool,
) -> Result<SyncStats, BridgeError> {
    sync_with(home, checkout, MAX_INDEXED_FILES, stop)
}

fn sync_with(
    home: &Path,
    checkout: &Path,
    limit: usize,
    stop: &AtomicBool,
) -> Result<SyncStats, BridgeError> {
    let checkout = canonical_checkout(checkout)?;
    let changes = git_view::status(&checkout)?;
    let Some(mut conn) = open_index(home, &checkout, OpenMode::Write)? else {
        return Ok(SyncStats::default());
    };
    let paths = changed_paths(&changes);
    let mut stats = SyncStats::default();
    let mut rows: usize = conn
        .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
        .map_err(db)? as usize;
    let mut tx = conn.transaction().map_err(db)?;
    let mut batch = 0usize;
    for path in paths {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let exists = tx
            .query_row("SELECT 1 FROM files WHERE path = ?1", [&path], |_| Ok(()))
            .optional()
            .map_err(db)?
            .is_some();
        if !exists && rows >= limit {
            continue;
        }
        match reevaluate_path(&tx, &checkout, &path)? {
            PathOutcome::Reindexed => {
                stats.reindexed += 1;
                if !exists {
                    rows += 1;
                }
            }
            PathOutcome::Removed => stats.removed += 1,
            PathOutcome::Skipped => {
                stats.removed += 1;
                if !exists {
                    rows += 1;
                }
            }
            PathOutcome::Unchanged => stats.unchanged += 1,
        }
        batch += 1;
        if batch == BATCH_FILES {
            tx.commit().map_err(db)?;
            tx = conn.transaction().map_err(db)?;
            batch = 0;
        }
    }
    tx.commit().map_err(db)?;
    Ok(stats)
}

/// The distinct paths a status names, destination and rename/copy source.
pub fn changed_paths(changes: &GitViewChanges) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for entry in changes
        .conflicts
        .iter()
        .chain(&changes.staged)
        .chain(&changes.unstaged)
        .chain(&changes.untracked)
    {
        for path in std::iter::once(&entry.path).chain(entry.orig_path.iter()) {
            if seen.insert(path.clone()) {
                paths.push(path.clone());
            }
        }
    }
    paths
}

/// Stat one path, compare it with its `files` row, and replace or drop its
/// chunks and row. Shared by a full pass and a sync.
fn reevaluate_path(
    conn: &Connection,
    checkout: &Path,
    path: &str,
) -> Result<PathOutcome, BridgeError> {
    let meta = std::fs::metadata(checkout.join(path)).ok();
    let (size, mtime) = meta
        .as_ref()
        .map_or((0, 0), |meta| (meta.len() as i64, mtime_ns(meta)));
    let stored: Option<(i64, i64)> = conn
        .query_row(
            "SELECT size, mtime_ns FROM files WHERE path = ?1",
            [path],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db)?;
    if stored == Some((size, mtime)) {
        return Ok(PathOutcome::Unchanged);
    }
    delete_path_rows(conn, path)?;
    if meta.is_none() {
        conn.execute("DELETE FROM files WHERE path = ?1", [path])
            .map_err(db)?;
        return Ok(PathOutcome::Removed);
    }
    match git_view::file(checkout, path) {
        Ok(file) if file.kind == "text" && !file.truncated => {
            if let Some(text) = file.text {
                insert_chunks(conn, path, &chunk_text(&text))?;
                record_file(conn, path, size, mtime)?;
                Ok(PathOutcome::Reindexed)
            } else {
                record_file(conn, path, size, mtime)?;
                Ok(PathOutcome::Skipped)
            }
        }
        Ok(file) if file.kind == "missing" => {
            conn.execute("DELETE FROM files WHERE path = ?1", [path])
                .map_err(db)?;
            Ok(PathOutcome::Removed)
        }
        _ => {
            record_file(conn, path, size, mtime)?;
            Ok(PathOutcome::Skipped)
        }
    }
}

/// Ranked search over the index. Never writes or creates anything.
pub fn search(home: &Path, checkout: &Path, query: &str) -> Result<GitViewSearch, BridgeError> {
    let words = split_words(query);
    if words.is_empty() {
        return Err(invalid("No searchable words"));
    }
    let checkout = canonical_checkout(checkout)?;
    let Some(conn) = open_index(home, &checkout, OpenMode::ReadOnly)? else {
        return Ok(GitViewSearch {
            matches: Vec::new(),
            truncated: false,
        });
    };
    let (mut matches, mut truncated) =
        query_matches(&conn, &checkout, &match_expression(&words, "AND"), &words)?;
    if matches.is_empty() {
        (matches, truncated) =
            query_matches(&conn, &checkout, &match_expression(&words, "OR"), &words)?;
    }
    Ok(GitViewSearch { matches, truncated })
}

/// The best `CONCEPT_RESULT_CAP` results for `expression`. The index ranks
/// and returns ids only, the best `RANKED_CHUNKS` of them; a chunk's lines
/// are read from the working tree for the results that are kept, so a query
/// that matches nearly every chunk does not carry every chunk's text.
fn query_matches(
    conn: &Connection,
    checkout: &Path,
    expression: &str,
    words: &[String],
) -> Result<(Vec<GitViewSearchMatch>, bool), BridgeError> {
    let ranked: Vec<i64> = {
        let mut stmt = conn
            .prepare(
                "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1
                 ORDER BY bm25(chunks_fts, 2.0, 1.5, 1.0) LIMIT ?2",
            )
            .map_err(db)?;
        let rows = stmt
            .query_map(params![expression, RANKED_CHUNKS as i64], |row| row.get(0))
            .map_err(db)?;
        rows.collect::<Result<_, _>>().map_err(db)?
    };
    let mut chunk_stmt = conn
        .prepare("SELECT path, start_line, end_line FROM chunks WHERE id = ?1")
        .map_err(db)?;
    let mut files: HashMap<String, Option<String>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut matches = Vec::new();
    // More ranked chunks than were looked at means more results than shown.
    let mut truncated = ranked.len() == RANKED_CHUNKS;
    for id in ranked {
        let Some((path, start, end)) = chunk_stmt
            .query_row([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? as u64,
                ))
            })
            .optional()
            .map_err(db)?
        else {
            continue;
        };
        let text = files
            .entry(path.clone())
            .or_insert_with(|| indexed_text(checkout, &path));
        // A file that is gone or no longer text since it was indexed has no
        // line to show.
        let Some(body) = text
            .as_deref()
            .and_then(|text| chunk_lines(text, start, end))
        else {
            continue;
        };
        let line = result_line(start, &body, words);
        if !seen.insert((path.clone(), line)) {
            continue;
        }
        if matches.len() == CONCEPT_RESULT_CAP {
            truncated = true;
            break;
        }
        let text = line_text(start, &body, line);
        matches.push(GitViewSearchMatch { path, line, text });
    }
    Ok((matches, truncated))
}

/// The working-tree text of an indexed file, under the rules the index
/// itself reads files by.
fn indexed_text(checkout: &Path, path: &str) -> Option<String> {
    match git_view::file(checkout, path) {
        Ok(file) if file.kind == "text" && !file.truncated => file.text,
        _ => None,
    }
}

/// Lines `start..=end` (1-based) of `text`, joined the way `chunk_text`
/// joins them; `None` when the file is now shorter than the chunk's start.
fn chunk_lines(text: &str, start: u64, end: u64) -> Option<String> {
    let first = usize::try_from(start.checked_sub(1)?).ok()?;
    let count = usize::try_from(end.saturating_sub(start) + 1).ok()?;
    let lines: Vec<&str> = text.lines().skip(first).take(count).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn match_expression(words: &[String], operator: &str) -> String {
    let mut parts = Vec::with_capacity(words.len());
    for (index, word) in words.iter().enumerate() {
        let quoted = format!("\"{word}\"");
        if index + 1 == words.len() {
            parts.push(format!("{quoted}*"));
        } else {
            parts.push(quoted);
        }
    }
    parts.join(&format!(" {operator} "))
}

fn result_line(start: u64, body: &str, words: &[String]) -> u64 {
    let lines: Vec<&str> = body.split('\n').collect();
    for (index, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        if words.iter().any(|word| lower.contains(word)) {
            return start + index as u64;
        }
    }
    if let Some(index) = lines.iter().position(|line| !line.trim().is_empty()) {
        return start + index as u64;
    }
    start
}

fn line_text(start: u64, body: &str, line: u64) -> String {
    let index = line.saturating_sub(start) as usize;
    body.split('\n')
        .nth(index)
        .unwrap_or_default()
        .chars()
        .take(SEARCH_PREVIEW_CHARS)
        .collect()
}

/// Open the index in one of its three modes.
fn open_index(
    home: &Path,
    checkout: &Path,
    mode: OpenMode,
) -> Result<Option<Connection>, BridgeError> {
    let checkout = canonical_checkout(checkout)?;
    let path = index_path(home, &checkout);
    match mode {
        OpenMode::CreateOrRebuild => Ok(Some(open_for_refresh(&path, &checkout)?)),
        OpenMode::ReadOnly => open_existing(&path, &checkout),
        OpenMode::Write => open_for_write(&path, &checkout),
    }
}

fn open_for_refresh(path: &Path, checkout: &Path) -> Result<Connection, BridgeError> {
    if let Some(parent) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(io)?;
    }
    create_file(path)?;
    let canonical = checkout.to_string_lossy();
    if let Ok(conn) = open_checked(path, &canonical) {
        return Ok(conn);
    }
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(sidecar(path, "-wal"));
    let _ = std::fs::remove_file(sidecar(path, "-shm"));
    create_fresh(path, &canonical)
}

fn open_existing(path: &Path, checkout: &Path) -> Result<Option<Connection>, BridgeError> {
    if !path.is_file() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(db)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(db)?;
    match check_schema(&conn, &checkout.to_string_lossy()) {
        Ok(true) => Ok(Some(conn)),
        Ok(false) => Ok(None),
        Err(error) => Err(error),
    }
}

fn open_for_write(path: &Path, checkout: &Path) -> Result<Option<Connection>, BridgeError> {
    if !path.is_file() {
        return Ok(None);
    }
    let Ok(conn) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE) else {
        return Ok(None);
    };
    let _ = conn.busy_timeout(BUSY_TIMEOUT);
    match check_schema(&conn, &checkout.to_string_lossy()) {
        Ok(true) => Ok(Some(conn)),
        _ => Ok(None),
    }
}

fn open_checked(path: &Path, canonical: &str) -> Result<Connection, BridgeError> {
    let conn = Connection::open(path).map_err(db)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(db)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;").map_err(db)?;
    if !check_schema(&conn, canonical)? {
        return Err(invalid("the search index schema does not match"));
    }
    Ok(conn)
}

fn create_fresh(path: &Path, canonical: &str) -> Result<Connection, BridgeError> {
    create_file(path)?;
    let conn = Connection::open(path).map_err(db)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(db)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;").map_err(db)?;
    conn.execute_batch(SCHEMA).map_err(db)?;
    conn.execute("INSERT INTO meta(id, path) VALUES (1, ?1)", [canonical])
        .map_err(db)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(db)?;
    Ok(conn)
}

fn check_schema(conn: &Connection, canonical: &str) -> Result<bool, BridgeError> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db)?;
    if version != SCHEMA_VERSION {
        return Ok(false);
    }
    let has_meta = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(db)?
        .is_some();
    if !has_meta {
        return Ok(false);
    }
    let meta: Option<String> = conn
        .query_row("SELECT path FROM meta WHERE id = 1", [], |row| row.get(0))
        .optional()
        .map_err(db)?;
    Ok(meta.as_deref() == Some(canonical))
}

fn create_file(path: &Path) -> Result<(), BridgeError> {
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(io)?;
    Ok(())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn insert_chunks(conn: &Connection, path: &str, chunks: &[Chunk]) -> Result<(), BridgeError> {
    let mut chunk_stmt = conn
        .prepare("INSERT INTO chunks(path, start_line, end_line) VALUES (?1, ?2, ?3)")
        .map_err(db)?;
    let mut fts_stmt = conn
        .prepare("INSERT INTO chunks_fts(rowid, path, idents, body) VALUES (?1, ?2, ?3, ?4)")
        .map_err(db)?;
    for chunk in chunks {
        let idents = ident_words(&chunk.text);
        chunk_stmt
            .execute(params![
                path,
                chunk.start_line as i64,
                chunk.end_line as i64
            ])
            .map_err(db)?;
        let id = conn.last_insert_rowid();
        fts_stmt
            .execute(params![id, path, idents, chunk.text])
            .map_err(db)?;
    }
    Ok(())
}

fn delete_path_rows(conn: &Connection, path: &str) -> Result<(), BridgeError> {
    let ids: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT id FROM chunks WHERE path = ?1")
            .map_err(db)?;
        let rows = stmt.query_map([path], |row| row.get(0)).map_err(db)?;
        rows.collect::<Result<_, _>>().map_err(db)?
    };
    for id in ids {
        conn.execute("DELETE FROM chunks_fts WHERE rowid = ?1", [id])
            .map_err(db)?;
    }
    conn.execute("DELETE FROM chunks WHERE path = ?1", [path])
        .map_err(db)?;
    Ok(())
}

fn delete_path(conn: &Connection, path: &str) -> Result<(), BridgeError> {
    delete_path_rows(conn, path)?;
    conn.execute("DELETE FROM files WHERE path = ?1", [path])
        .map_err(db)?;
    Ok(())
}

fn record_file(conn: &Connection, path: &str, size: i64, mtime: i64) -> Result<(), BridgeError> {
    conn.execute(
        "INSERT INTO files(path, size, mtime_ns) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET size = excluded.size, mtime_ns = excluded.mtime_ns",
        params![path, size, mtime],
    )
    .map_err(db)?;
    Ok(())
}

fn indexed_files(conn: &Connection) -> Result<usize, BridgeError> {
    let count: i64 = conn
        .query_row("SELECT COUNT(DISTINCT path) FROM chunks", [], |row| {
            row.get(0)
        })
        .map_err(db)?;
    Ok(count as usize)
}

fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_nanos().min(i64::MAX as u128) as i64)
}

/// Delete index files whose recorded checkout directory no longer exists.
pub fn remove_orphans(home: &Path) {
    let Ok(entries) = std::fs::read_dir(home.join("search")) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("db") {
            continue;
        }
        let Ok(conn) = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
            continue;
        };
        let meta: Option<String> = conn
            .query_row("SELECT path FROM meta WHERE id = 1", [], |row| row.get(0))
            .optional()
            .unwrap_or(None);
        let Some(checkout) = meta else {
            continue;
        };
        if !Path::new(&checkout).is_dir() {
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(sidecar(&path, "-wal"));
            let _ = std::fs::remove_file(sidecar(&path, "-shm"));
        }
    }
}

/// Split text into lowercase words at non-alphanumerics, `_`, case changes,
/// and letter/digit boundaries.
pub fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if !c.is_alphanumeric() {
            push_word(&mut words, &mut current);
            previous = Some(c);
            continue;
        }
        if let Some(prev) = previous {
            let boundary = (prev.is_lowercase() && c.is_uppercase())
                || (prev.is_uppercase()
                    && c.is_uppercase()
                    && chars.peek().is_some_and(|next| next.is_lowercase()))
                || (prev.is_alphabetic() && c.is_numeric())
                || (prev.is_numeric() && c.is_alphabetic());
            if boundary {
                push_word(&mut words, &mut current);
            }
        }
        current.push(c);
        previous = Some(c);
    }
    push_word(&mut words, &mut current);
    words
}

fn push_word(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(current.to_lowercase());
        current.clear();
    }
}

/// The `idents` text for one chunk: the words of every identifier token that
/// yields at least two words, in order and without deduplication. A token
/// that yields one word is already in `body` and contributes nothing.
pub fn ident_words(text: &str) -> String {
    let mut words = Vec::new();
    let mut token = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            token.push(c);
        } else {
            push_ident_token(&mut words, &mut token);
        }
    }
    push_ident_token(&mut words, &mut token);
    words.join(" ")
}

fn push_ident_token(words: &mut Vec<String>, token: &mut String) {
    if token.is_empty() {
        return;
    }
    let split = split_words(token);
    if split.len() >= 2 {
        words.extend(split);
    }
    token.clear();
}

/// Split a file into overlapping line chunks.
pub fn chunk_text(text: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < lines.len() {
        let mut end = start;
        let mut bytes = 0usize;
        while end < lines.len() && end - start < CHUNK_LINES {
            let line_bytes = lines[end].len() + 1;
            if end > start && bytes + line_bytes > CHUNK_BYTES {
                break;
            }
            bytes += line_bytes;
            end += 1;
            if bytes > CHUNK_BYTES {
                break;
            }
        }
        if end < lines.len() {
            let half = start + (end - start) / 2;
            if let Some(blank) = (half..end)
                .rev()
                .find(|index| lines[*index].trim().is_empty())
            {
                end = blank + 1;
            }
        }
        let mut chunk = String::new();
        for (offset, line) in lines[start..end].iter().enumerate() {
            if offset > 0 {
                if chunk.len() >= CHUNK_BYTES {
                    break;
                }
                chunk.push('\n');
            }
            let remaining = CHUNK_BYTES.saturating_sub(chunk.len());
            if line.len() > remaining {
                chunk.push_str(truncate_bytes(line, remaining));
                break;
            }
            chunk.push_str(line);
        }
        chunks.push(Chunk {
            start_line: start as u64 + 1,
            end_line: end as u64,
            text: chunk,
        });
        if end >= lines.len() {
            break;
        }
        start = (start + 1).max(end.saturating_sub(CHUNK_OVERLAP));
    }
    chunks
}

fn truncate_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::InvalidArgument), message)
}

fn db(error: rusqlite::Error) -> BridgeError {
    BridgeError::new(Code::Storage, format!("search index: {error}"))
}

fn io(error: std::io::Error) -> BridgeError {
    BridgeError::new(Code::Storage, format!("search index: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;
    use std::process::Command;

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git run");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository(label: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(label);
        let root = dir.path().canonicalize().unwrap();
        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.name", "Tester"]);
        git(&root, &["config", "user.email", "tester@example.com"]);
        fs::write(root.join("tracked.txt"), "tracked needle\n").unwrap();
        fs::write(root.join("second.txt"), "second\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "init"]);
        (dir, root)
    }

    fn home() -> TempDir {
        TempDir::new("code-index-home")
    }

    fn pass(home: &Path, root: &Path) {
        refresh(home, root, |_, _| true).unwrap();
    }

    fn paths(home: &Path, root: &Path, query: &str) -> Vec<String> {
        search(home, root, query)
            .unwrap()
            .matches
            .into_iter()
            .map(|m| m.path)
            .collect()
    }

    fn chunk_count(home: &Path, root: &Path, path: &str) -> i64 {
        let conn = open_index(home, root, OpenMode::ReadOnly).unwrap().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE path = ?1",
            [path],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn file_row(home: &Path, root: &Path, path: &str) -> bool {
        let conn = open_index(home, root, OpenMode::ReadOnly).unwrap().unwrap();
        conn.query_row("SELECT 1 FROM files WHERE path = ?1", [path], |_| Ok(()))
            .optional()
            .unwrap()
            .is_some()
    }

    #[test]
    fn split_words_splits_case_digits_and_separators() {
        assert_eq!(split_words("ReconnectBackoff"), ["reconnect", "backoff"]);
        assert_eq!(
            split_words("reconnect_backoff_ms"),
            ["reconnect", "backoff", "ms"]
        );
        assert_eq!(split_words("HTTPServer2"), ["http", "server", "2"]);
        assert!(split_words("").is_empty());
    }

    #[test]
    fn ident_words_keeps_only_compound_tokens() {
        assert_eq!(
            ident_words("fn reconnect_backoff_ms(x) { // retry backoff after ReconnectBackoff }"),
            "reconnect backoff ms reconnect backoff"
        );
        assert_eq!(ident_words("plain prose words only"), "");
    }

    #[test]
    fn chunk_text_overlaps_and_breaks_at_blank_lines() {
        let hundred: String = (1..=100)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let ranges: Vec<(u64, u64)> = chunk_text(&hundred)
            .iter()
            .map(|c| (c.start_line, c.end_line))
            .collect();
        assert_eq!(ranges, [(1, 40), (37, 76), (73, 100)]);

        let forty: String = (1..=40)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(chunk_text(&forty).len(), 1);

        let forty_one: String = (1..=41)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let ranges: Vec<(u64, u64)> = chunk_text(&forty_one)
            .iter()
            .map(|c| (c.start_line, c.end_line))
            .collect();
        assert_eq!(ranges, [(1, 40), (37, 41)]);

        let mut blank = String::new();
        for n in 1..=45 {
            if n == 30 {
                blank.push('\n');
            } else {
                blank.push_str(&format!("line{n}\n"));
            }
        }
        let chunks = chunk_text(&blank);
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, 30));
        assert_eq!(chunks[1].start_line, 27);

        let long = format!("{}tail", "x".repeat(5000));
        let chunks = chunk_text(&long);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.len() <= CHUNK_BYTES);
        assert!(std::str::from_utf8(chunks[0].text.as_bytes()).is_ok());

        let leading = chunk_text("\n\nneedle\n");
        assert_eq!(leading.len(), 1);
        assert_eq!(leading[0].text, "\n\nneedle");
        assert_eq!((leading[0].start_line, leading[0].end_line), (1, 3));

        assert!(chunk_text("").is_empty());
    }

    #[test]
    fn leading_blank_lines_keep_the_result_line() {
        let (_dir, root) = repository("code-index-blanks");
        let home = home();
        fs::write(root.join("blank.txt"), "\n\nzebraneedle\n").unwrap();
        pass(home.path(), &root);
        let matches = search(home.path(), &root, "zebraneedle").unwrap().matches;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line, 3);
        assert_eq!(matches[0].text, "zebraneedle");
    }

    #[test]
    fn fts5_is_available_in_the_linked_sqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE f USING fts5(path, idents, body, tokenize='porter unicode61');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO f(path, idents, body) VALUES ('a.rs', 'reconnect backoff', 'x')",
            [],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM f WHERE f MATCH 'reconnect'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn indexed_set_skips_ignored_binary_large_and_outside() {
        let (_dir, root) = repository("code-index-set");
        let home = home();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), "ignored needle\n").unwrap();
        fs::write(root.join("untracked.txt"), "untracked needle\n").unwrap();
        fs::write(root.join("binary.bin"), b"binary\0needle").unwrap();
        fs::write(root.join("large.txt"), "x".repeat(1024 * 1024 + 1)).unwrap();
        let outside_dir = TempDir::new("code-index-outside");
        let outside = outside_dir.path().join("outside.txt");
        fs::write(&outside, "outside needle\n").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();

        pass(home.path(), &root);
        let found = paths(home.path(), &root, "needle");
        assert!(found.contains(&"tracked.txt".to_string()));
        assert!(found.contains(&"untracked.txt".to_string()));
        assert!(!found.contains(&"ignored.txt".to_string()));
        assert!(!found.contains(&"binary.bin".to_string()));
        assert!(!found.contains(&"large.txt".to_string()));
        assert!(!found.contains(&"link.txt".to_string()));
        assert!(!found.iter().any(|path| path.contains("outside")));
        assert_eq!(refresh(home.path(), &root, |_, _| true).unwrap().files, 4);
    }

    #[test]
    fn incremental_pass_rechunks_only_changed_files() {
        let (_dir, root) = repository("code-index-incremental");
        let home = home();
        pass(home.path(), &root);
        fs::write(root.join("tracked.txt"), "changed needle\n").unwrap();
        fs::write(root.join("added.txt"), "added needle\n").unwrap();
        fs::remove_file(root.join("second.txt")).unwrap();
        pass(home.path(), &root);
        assert_eq!(paths(home.path(), &root, "changed"), ["tracked.txt"]);
        assert_eq!(paths(home.path(), &root, "added"), ["added.txt"]);
        assert!(paths(home.path(), &root, "second").is_empty());
        assert!(!file_row(home.path(), &root, "second.txt"));
        assert_eq!(refresh(home.path(), &root, |_, _| true).unwrap().files, 2);
    }

    #[test]
    fn text_to_binary_drops_chunks() {
        let (_dir, root) = repository("code-index-binary");
        let home = home();
        pass(home.path(), &root);
        fs::write(root.join("tracked.txt"), b"now\0binary").unwrap();
        pass(home.path(), &root);
        assert!(paths(home.path(), &root, "tracked").is_empty());
        assert_eq!(chunk_count(home.path(), &root, "tracked.txt"), 0);
        assert!(file_row(home.path(), &root, "tracked.txt"));
    }

    #[test]
    fn symlinked_checkout_shares_one_index_file() {
        let (_dir, root) = repository("code-index-symlink");
        let home = home();
        let link = home.path().join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert_eq!(
            index_path(home.path(), &root),
            index_path(home.path(), &link)
        );
    }

    #[test]
    fn limit_truncates_the_listing() {
        let (_dir, root) = repository("code-index-limit");
        let home = home();
        let stats = refresh_with(home.path(), &root, 1, |_, _| true).unwrap();
        assert!(stats.truncated);
        assert_eq!(stats.files, 1);
        assert!(!paths(home.path(), &root, "second").is_empty());
        assert!(paths(home.path(), &root, "tracked").is_empty());
    }

    #[test]
    fn stopped_pass_leaves_a_valid_index() {
        let (_dir, root) = repository("code-index-stop");
        let home = home();
        refresh_with(home.path(), &root, MAX_INDEXED_FILES, |done, _| done < 1).unwrap();
        pass(home.path(), &root);
        assert!(!paths(home.path(), &root, "needle").is_empty());
    }

    #[test]
    fn two_threads_refresh_the_same_checkout() {
        let (_dir, root) = repository("code-index-threads");
        let home = home();
        pass(home.path(), &root);
        std::thread::scope(|scope| {
            let home_a = home.path().to_path_buf();
            let home_b = home.path().to_path_buf();
            let root_a = root.clone();
            let root_b = root.clone();
            let a = scope.spawn(move || refresh(&home_a, &root_a, |_, _| true).unwrap());
            let b = scope.spawn(move || refresh(&home_b, &root_b, |_, _| true).unwrap());
            assert!(a.join().is_ok());
            assert!(b.join().is_ok());
        });
    }

    #[test]
    fn directory_and_file_modes_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, root) = repository("code-index-modes");
        let home = home();
        pass(home.path(), &root);
        let dir_mode = fs::metadata(home.path().join("search"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(index_path(home.path(), &root))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn two_checkouts_have_two_index_files() {
        let (_dir_a, root_a) = repository("code-index-a");
        let (_dir_b, root_b) = repository("code-index-b");
        let home = home();
        assert_ne!(
            index_path(home.path(), &root_a),
            index_path(home.path(), &root_b)
        );
        pass(home.path(), &root_a);
        pass(home.path(), &root_b);
        assert!(index_path(home.path(), &root_a).is_file());
        assert!(index_path(home.path(), &root_b).is_file());
    }

    #[test]
    fn remove_orphans_deletes_missing_checkouts_only() {
        let (_dir_a, root_a) = repository("code-index-orphan-a");
        let (_dir_b, root_b) = repository("code-index-orphan-b");
        let home = home();
        pass(home.path(), &root_a);
        pass(home.path(), &root_b);
        fs::remove_dir_all(&root_a).unwrap();
        remove_orphans(home.path());
        assert!(!index_path(home.path(), &root_a).exists());
        assert!(index_path(home.path(), &root_b).is_file());
    }

    #[test]
    fn unwritable_search_directory_errors() {
        let (_dir, root) = repository("code-index-unwritable");
        let home = home();
        fs::write(home.path().join("search"), "not a directory").unwrap();
        assert!(refresh(home.path(), &root, |_, _| true).is_err());
    }

    #[test]
    fn non_repository_errors() {
        let dir = TempDir::new("code-index-plain");
        let home = home();
        assert!(refresh(home.path(), dir.path(), |_, _| true).is_err());
    }

    #[test]
    fn garbage_index_is_rebuilt() {
        let (_dir, root) = repository("code-index-garbage");
        let home = home();
        pass(home.path(), &root);
        fs::write(index_path(home.path(), &root), b"garbage").unwrap();
        pass(home.path(), &root);
        assert!(!paths(home.path(), &root, "needle").is_empty());
    }

    #[test]
    fn ranking_puts_identifiers_above_prose() {
        let (_dir, root) = repository("code-index-ranking");
        let home = home();
        fs::write(
            root.join("a.rs"),
            "fn reconnect_backoff_ms() { let ReconnectBackoff = 1; }\n",
        )
        .unwrap();
        fs::write(root.join("b.rs"), "// backoff only\n").unwrap();
        pass(home.path(), &root);
        let found = paths(home.path(), &root, "reconnect backoff");
        assert_eq!(found.first().map(String::as_str), Some("a.rs"));
        let found = paths(home.path(), &root, "backoff");
        assert_eq!(found.first().map(String::as_str), Some("a.rs"));
        assert!(!paths(home.path(), &root, "reconnect backoff").is_empty());
    }

    #[test]
    fn prefix_and_or_fallback_work() {
        let (_dir, root) = repository("code-index-query");
        let home = home();
        fs::write(root.join("alpha.txt"), "alpha only\n").unwrap();
        fs::write(root.join("beta.txt"), "beta only\n").unwrap();
        pass(home.path(), &root);
        assert_eq!(paths(home.path(), &root, "alph"), ["alpha.txt"]);
        let found = paths(home.path(), &root, "alpha beta");
        assert!(found.contains(&"alpha.txt".to_string()));
        assert!(found.contains(&"beta.txt".to_string()));
    }

    #[test]
    fn line_rule_dedupes_overlapping_chunks() {
        let (_dir, root) = repository("code-index-lines");
        let home = home();
        let mut text = String::new();
        for n in 1..=45 {
            if n == 38 {
                text.push_str("zebraneedle\n");
            } else {
                text.push_str(&format!("line{n}\n"));
            }
        }
        fs::write(root.join("lines.txt"), text).unwrap();
        pass(home.path(), &root);
        let matches = search(home.path(), &root, "zebraneedle").unwrap().matches;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line, 38);
        assert_eq!(matches[0].text, "zebraneedle");
    }

    #[test]
    fn the_index_holds_words_not_text_and_previews_come_from_the_file() {
        let (_dir, root) = repository("code-index-contentless");
        let sentence = "the quick brown heron crossed seventeen rivers";
        fs::write(root.join("story.txt"), format!("intro\n{sentence}\n")).unwrap();
        let home = home();
        pass(home.path(), &root);

        let index = index_path(home.path(), &root);
        let mut stored = fs::read(&index).unwrap();
        stored.extend(fs::read(sidecar(&index, "-wal")).unwrap_or_default());
        let held = |needle: &str| {
            stored
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        };
        assert!(held("story.txt"), "the path is stored");
        assert!(!held(sentence), "the file's text is not");

        let found = search(home.path(), &root, "heron").unwrap();
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].path, "story.txt");
        assert_eq!(found.matches[0].line, 2);
        assert_eq!(found.matches[0].text, sentence);
    }

    #[test]
    fn a_file_gone_since_it_was_indexed_gives_no_result() {
        let (_dir, root) = repository("code-index-gone");
        fs::write(root.join("gone.txt"), "vanishing zebra\n").unwrap();
        fs::write(root.join("kept.txt"), "lasting zebra\n").unwrap();
        let home = home();
        pass(home.path(), &root);
        fs::remove_file(root.join("gone.txt")).unwrap();
        // No sync ran: the index still names the file, and the search skips it.
        assert_eq!(paths(home.path(), &root, "zebra"), ["kept.txt"]);
    }

    #[test]
    fn cap_truncates_results() {
        let (_dir, root) = repository("code-index-cap");
        let home = home();
        for n in 0..60 {
            fs::write(root.join(format!("f{n}.txt")), "needle\n").unwrap();
        }
        pass(home.path(), &root);
        let view = search(home.path(), &root, "needle").unwrap();
        assert_eq!(view.matches.len(), CONCEPT_RESULT_CAP);
        assert!(view.truncated);
    }

    #[test]
    fn hostile_queries_never_error() {
        let (_dir, root) = repository("code-index-hostile");
        let home = home();
        pass(home.path(), &root);
        for query in [
            "\"",
            "\"unbalanced",
            "*",
            "-x",
            "a:b",
            "AND",
            "OR",
            "NEAR(",
            ")",
        ] {
            match search(home.path(), &root, query) {
                Ok(_) => {}
                Err(error) => assert_eq!(error.message, "No searchable words", "{query}"),
            }
        }
    }

    #[test]
    fn search_before_a_pass_is_empty_and_writes_nothing() {
        let (_dir, root) = repository("code-index-empty");
        let home = home();
        let view = search(home.path(), &root, "needle").unwrap();
        assert!(view.matches.is_empty());
        assert!(!home.path().join("search").exists());
    }

    #[test]
    fn version_mismatch_answers_empty_and_is_untouched() {
        let (_dir, root) = repository("code-index-version");
        let home = home();
        pass(home.path(), &root);
        let path = index_path(home.path(), &root);
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        assert!(search(home.path(), &root, "needle")
            .unwrap()
            .matches
            .is_empty());
        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 99);
        pass(home.path(), &root);
        assert!(!paths(home.path(), &root, "needle").is_empty());
    }

    #[test]
    fn corrupt_index_errors_without_repairing() {
        let (_dir, root) = repository("code-index-corrupt");
        let home = home();
        pass(home.path(), &root);
        let path = index_path(home.path(), &root);
        fs::write(&path, b"garbage").unwrap();
        assert!(search(home.path(), &root, "needle").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"garbage");
    }

    #[test]
    fn sync_adds_edits_and_deletes() {
        let (_dir, root) = repository("code-index-sync");
        let home = home();
        pass(home.path(), &root);
        let stop = AtomicBool::new(false);
        fs::write(root.join("new.txt"), "new needle\n").unwrap();
        assert_eq!(
            sync_changed(home.path(), &root, &stop).unwrap().reindexed,
            1
        );
        assert!(!paths(home.path(), &root, "needle").is_empty());
        fs::write(root.join("tracked.txt"), "edited needle\n").unwrap();
        assert_eq!(
            sync_changed(home.path(), &root, &stop).unwrap().reindexed,
            1
        );
        assert_eq!(paths(home.path(), &root, "edited"), ["tracked.txt"]);
        fs::remove_file(root.join("second.txt")).unwrap();
        assert_eq!(sync_changed(home.path(), &root, &stop).unwrap().removed, 1);
        assert!(!file_row(home.path(), &root, "second.txt"));
    }

    #[test]
    fn sync_rename_and_copy() {
        let (_dir, root) = repository("code-index-rename");
        let home = home();
        pass(home.path(), &root);
        let stop = AtomicBool::new(false);
        git(&root, &["mv", "tracked.txt", "renamed.txt"]);
        sync_changed(home.path(), &root, &stop).unwrap();
        assert!(!paths(home.path(), &root, "tracked").is_empty());
        assert!(!file_row(home.path(), &root, "tracked.txt"));
        assert!(paths(home.path(), &root, "needle").contains(&"renamed.txt".to_string()));

        fs::copy(root.join("renamed.txt"), root.join("copy.txt")).unwrap();
        sync_changed(home.path(), &root, &stop).unwrap();
        assert!(paths(home.path(), &root, "needle").contains(&"copy.txt".to_string()));
        assert!(paths(home.path(), &root, "needle").contains(&"renamed.txt".to_string()));
    }

    #[test]
    fn sync_without_changes_reads_nothing() {
        let (_dir, root) = repository("code-index-sync-none");
        let home = home();
        pass(home.path(), &root);
        let stop = AtomicBool::new(false);
        let stats = sync_changed(home.path(), &root, &stop).unwrap();
        assert_eq!((stats.reindexed, stats.removed), (0, 0));
        let path = root.join("tracked.txt");
        let mut perms = fs::metadata(&path).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o000);
        fs::set_permissions(&path, perms).unwrap();
        let stats = sync_changed(home.path(), &root, &stop).unwrap();
        assert_eq!((stats.reindexed, stats.removed), (0, 0));
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();
    }

    #[test]
    fn sync_without_an_index_does_nothing() {
        let (_dir, root) = repository("code-index-sync-empty");
        let home = home();
        let stop = AtomicBool::new(false);
        let stats = sync_changed(home.path(), &root, &stop).unwrap();
        assert_eq!((stats.reindexed, stats.removed, stats.unchanged), (0, 0, 0));
        assert!(!home.path().join("search").exists());
    }

    #[test]
    fn sync_on_a_non_repository_errors() {
        let dir = TempDir::new("code-index-sync-plain");
        let home = home();
        let stop = AtomicBool::new(false);
        assert!(sync_changed(home.path(), dir.path(), &stop).is_err());
    }

    #[test]
    fn sync_limit_does_not_add_new_paths_beyond_it() {
        let (_dir, root) = repository("code-index-sync-limit");
        let home = home();
        pass(home.path(), &root);
        let stop = AtomicBool::new(false);
        fs::write(root.join("extra.txt"), "extra needle\n").unwrap();
        let stats = sync_with(home.path(), &root, 2, &stop).unwrap();
        assert_eq!(stats.reindexed, 0);
        assert!(!file_row(home.path(), &root, "extra.txt"));
    }

    #[test]
    fn changed_paths_dedupes_and_keeps_rename_sources() {
        use crate::git_view::StatusEntry;
        let changes = GitViewChanges {
            branch: None,
            head: String::new(),
            conflicts: Vec::new(),
            staged: vec![StatusEntry {
                path: "b.txt".into(),
                orig_path: Some("a.txt".into()),
                status: "renamed",
                added: None,
                removed: None,
            }],
            unstaged: vec![StatusEntry {
                path: "b.txt".into(),
                orig_path: None,
                status: "modified",
                added: None,
                removed: None,
            }],
            untracked: vec![StatusEntry {
                path: "c.txt".into(),
                orig_path: None,
                status: "untracked",
                added: None,
                removed: None,
            }],
            ahead: None,
        };
        assert_eq!(changed_paths(&changes), ["b.txt", "a.txt", "c.txt"]);
    }
}
