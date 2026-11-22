//! Hand a terminal agent's work to a fresh session: read the exact saved
//! conversation of the source pane from its own profile, build a bounded,
//! editable draft, and validate the optional native Claude fork.
//!
//! Everything here only reads. Nothing is written, linked or sent, and no
//! lookup ever picks "the newest" session or another account's history:
//! each reader opens one expected path or one database row for one ID.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use crate::session_debug::{valid_id, Identity};

/// The generated draft and the approved review are each at most this many
/// bytes of UTF-8.
pub const DRAFT_LIMIT: usize = 16 * 1024;
/// The first user message is kept, cut to this many bytes.
const FIRST_LIMIT: usize = 2 * 1024;
/// A single history record past this size is skipped.
const RECORD_LIMIT: usize = 2 * 1024 * 1024;
/// At most this many bytes of one history are read.
const READ_LIMIT: u64 = 24 * 1024 * 1024;
/// At most this many database rows of one conversation are read.
const ROW_LIMIT: usize = 20_000;
/// At most this many directory entries are looked at to find a file.
const DIR_LIMIT: usize = 50_000;
/// Changed paths listed in the draft; the rest are counted.
const PATH_LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Message {
    role: Role,
    text: String,
}

/// Visible user and assistant text of one conversation, oldest first.
#[derive(Debug, Default)]
struct Transcript {
    messages: Vec<Message>,
    /// Part of the history was too large to read whole.
    shortened: bool,
    /// Claude only: the verified transcript file.
    file: Option<PathBuf>,
}

impl Transcript {
    /// Consecutive text of one role is one message.
    fn push(&mut self, role: Role, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        match self.messages.last_mut() {
            Some(last) if last.role == role => {
                last.text.push_str("\n\n");
                last.text.push_str(text);
            }
            _ => self.messages.push(Message {
                role,
                text: text.to_owned(),
            }),
        }
    }
}

/// Why a conversation could not be read.
#[derive(Debug, PartialEq, Eq)]
enum ReadError {
    Missing,
    Locked,
    Corrupt(String),
    Unsupported(String),
    WrongCheckout,
    Unsafe(PathBuf),
    Unverifiable,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Missing => {
                f.write_str("the conversation is not in this account's local history")
            }
            ReadError::Locked => f.write_str("the history database is busy; try again"),
            ReadError::Corrupt(why) => write!(f, "the saved history cannot be read ({why})"),
            ReadError::Unsupported(why) => f.write_str(why),
            ReadError::WrongCheckout => {
                f.write_str("the conversation belongs to a different folder")
            }
            ReadError::Unsafe(path) => write!(
                f,
                "{} is a link that redirects the history lookup",
                path.display()
            ),
            ReadError::Unverifiable => f.write_str(
                "Antigravity transcripts record no folder, so a typed ID cannot be verified",
            ),
        }
    }
}

/// The Default history folders, resolved from the environment once so tests
/// can pass their own.
#[derive(Clone, Debug, Default)]
pub struct Folders {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
    /// `$XDG_DATA_HOME` or `~/.local/share`: Devin's and OpenCode's Default.
    pub data: Option<PathBuf>,
    /// `$AGY_DIR` or `~/.gemini/antigravity-cli`.
    pub agy: Option<PathBuf>,
}

impl Folders {
    pub fn from_env() -> Folders {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);
        Folders {
            claude: crate::accounts::default_dir("claude"),
            codex: crate::accounts::default_dir("codex"),
            data: std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .or_else(|| home.as_ref().map(|home| home.join(".local/share"))),
            agy: std::env::var_os("AGY_DIR")
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
                .or_else(|| home.map(|home| home.join(".gemini/antigravity-cli"))),
        }
    }
}

/// `NAME=value` words a launch set through `/usr/bin/env`.
fn launch_env(argv: &[String]) -> HashMap<&str, &str> {
    let mut vars = HashMap::new();
    if argv
        .first()
        .and_then(|first| Path::new(first).file_name())
        .is_none_or(|name| name != "env")
    {
        return vars;
    }
    let mut words = argv.iter().skip(1);
    while let Some(word) = words.next() {
        if word == "-u" {
            words.next();
            continue;
        }
        match word.split_once('=') {
            Some((name, value)) => {
                vars.insert(name, value);
            }
            None => break,
        }
    }
    vars
}

/// The history folder the source pane was launched with: its account's
/// folder variable, else the agent's Default folder.
pub fn source_profile(agent: &str, argv: &[String], folders: &Folders) -> Option<PathBuf> {
    let (var, default) = match agent {
        "claude" => ("CLAUDE_CONFIG_DIR", &folders.claude),
        "codex" => ("CODEX_HOME", &folders.codex),
        "devin" | "opencode" => ("XDG_DATA_HOME", &folders.data),
        "agy" => ("AGY_DIR", &folders.agy),
        _ => return None,
    };
    launch_env(argv)
        .get(var)
        .map(PathBuf::from)
        .or_else(|| default.clone())
}

/// Whether a typed conversation ID can be verified for `agent`.
fn accepts_manual_id(agent: &str) -> bool {
    matches!(agent, "claude" | "codex" | "devin" | "opencode")
}

/// The conversation `id` of `agent` in `profile`, verified to belong to
/// `checkout`. `manual` marks a typed ID, which needs checkout metadata.
fn read(
    agent: &str,
    profile: &Path,
    id: &str,
    checkout: &Path,
    manual: bool,
) -> Result<Transcript, ReadError> {
    if !valid_id(id) {
        return Err(ReadError::Unsupported(
            "that is not a conversation ID".into(),
        ));
    }
    match agent {
        "claude" => claude(profile, id, checkout),
        "codex" => codex(profile, id, checkout),
        "devin" => devin(profile, id, checkout),
        "opencode" => opencode(profile, id, checkout),
        "agy" if manual => Err(ReadError::Unverifiable),
        "agy" => agy(profile, id),
        _ => Err(ReadError::Unsupported(
            "this agent has no local history reader".into(),
        )),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn io_error(error: std::io::Error) -> ReadError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ReadError::Missing,
        _ => ReadError::Corrupt(error.to_string()),
    }
}

/// `root` joined with `parts`, refusing any part that is a symbolic link.
fn plain_path(root: &Path, parts: &[&str]) -> Result<PathBuf, ReadError> {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(ReadError::Unsafe(path)),
            Ok(_) => {}
            Err(_) => return Err(ReadError::Missing),
        }
    }
    Ok(path)
}

/// One line of at most `RECORD_LIMIT` bytes: `None` at the end, else the
/// line (`None` when it was too long) and whether a newline ended it.
type Line = Option<(Option<Vec<u8>>, bool)>;

fn bounded_line(reader: &mut impl BufRead) -> Result<Line, ReadError> {
    let mut line = Vec::new();
    let mut over = false;
    loop {
        let buf = reader.fill_buf().map_err(io_error)?;
        if buf.is_empty() {
            return Ok((!line.is_empty() || over).then(|| ((!over).then_some(line), false)));
        }
        let end = buf.iter().position(|&byte| byte == b'\n');
        let chunk = &buf[..end.unwrap_or(buf.len())];
        if !over {
            if line.len() + chunk.len() > RECORD_LIMIT {
                over = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let used = end.map_or(buf.len(), |end| end + 1);
        reader.consume(used);
        if end.is_some() {
            return Ok(Some(((!over).then_some(line), true)));
        }
    }
}

/// Each JSON record of `path`, oldest first. A file past the read budget is
/// read from its last `READ_LIMIT` bytes and an oversized record is
/// skipped; both set `shortened`. A malformed record is corrupt unless it is
/// the unterminated last line a running CLI is still writing.
fn jsonl(path: &Path, shortened: &mut bool, mut each: impl FnMut(Value)) -> Result<(), ReadError> {
    let mut file = File::open(path).map_err(io_error)?;
    let len = file.metadata().map_err(io_error)?.len();
    let tail = len > READ_LIMIT;
    if tail {
        file.seek(SeekFrom::Start(len - READ_LIMIT))
            .map_err(io_error)?;
        *shortened = true;
    }
    let mut reader = BufReader::new(file);
    if tail {
        bounded_line(&mut reader)?;
    }
    while let Some((line, complete)) = bounded_line(&mut reader)? {
        let Some(line) = line else {
            *shortened = true;
            continue;
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(&line) {
            Ok(value) => each(value),
            Err(_) if !complete => break,
            Err(error) => return Err(ReadError::Corrupt(format!("a record is not JSON: {error}"))),
        }
    }
    Ok(())
}

fn first_record(path: &Path) -> Result<Value, ReadError> {
    let mut reader = BufReader::new(File::open(path).map_err(io_error)?);
    let Some((Some(line), _)) = bounded_line(&mut reader)? else {
        return Err(ReadError::Corrupt(
            "the first record is missing or too large".into(),
        ));
    };
    serde_json::from_slice(&line).map_err(|error| ReadError::Corrupt(error.to_string()))
}

/// Claude's own command, hook and notification wrappers are not conversation.
const CLAUDE_INJECTED: &[&str] = &[
    "<task-notification",
    "<command-",
    "<local-command-",
    "<bash-",
    "<system-reminder",
];

fn claude_texts(role: Role, content: &Value) -> Vec<String> {
    let keep = |text: &str| {
        role == Role::Assistant || !CLAUDE_INJECTED.iter().any(|p| text.starts_with(p))
    };
    match content {
        Value::String(text) if role == Role::User && keep(text) => vec![text.clone()],
        Value::Array(items) => items
            .iter()
            .filter(|item| item["type"] == "text")
            .filter_map(|item| item["text"].as_str())
            .filter(|text| keep(text))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn claude(profile: &Path, id: &str, checkout: &Path) -> Result<Transcript, ReadError> {
    // The top-level `projects` may be the shared link to Default's history;
    // nothing below it may redirect the lookup.
    let projects = profile.join("projects");
    let name = format!("{id}.jsonl");
    let mut file = None;
    for entry in fs::read_dir(&projects)
        .map_err(io_error)?
        .take(DIR_LIMIT)
        .flatten()
    {
        let candidate = entry.path().join(&name);
        if fs::symlink_metadata(&candidate).is_err() {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
            return Err(ReadError::Unsafe(entry.path()));
        }
        let meta = fs::symlink_metadata(&candidate).map_err(io_error)?;
        if meta.file_type().is_symlink() {
            return Err(ReadError::Unsafe(candidate));
        }
        if meta.is_file() {
            file = Some(candidate);
            break;
        }
    }
    let file = file.ok_or(ReadError::Missing)?;

    struct Record {
        uuid: String,
        role: Role,
        texts: Vec<String>,
    }
    let mut parents: HashMap<String, Option<String>> = HashMap::new();
    let mut records = Vec::new();
    let mut cwd: Option<String> = None;
    let mut shortened = false;
    jsonl(&file, &mut shortened, |value| {
        let Some(uuid) = value["uuid"].as_str() else {
            return;
        };
        parents.insert(
            uuid.to_owned(),
            value["parentUuid"].as_str().map(str::to_owned),
        );
        let role = match value["type"].as_str() {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            _ => return,
        };
        if value["isSidechain"].as_bool() == Some(true) {
            return;
        }
        if cwd.is_none() {
            cwd = value["cwd"].as_str().map(str::to_owned);
        }
        let texts = if value["isMeta"].as_bool() == Some(true) {
            Vec::new()
        } else {
            claude_texts(role, &value["message"]["content"])
        };
        records.push(Record {
            uuid: uuid.to_owned(),
            role,
            texts,
        });
    })?;
    let cwd = cwd.ok_or(ReadError::Missing)?;
    if !same_dir(Path::new(&cwd), checkout) {
        return Err(ReadError::WrongCheckout);
    }
    // A rewind leaves the abandoned turns in the file: keep only the chain
    // that leads to the last message.
    let mut chain = HashSet::new();
    let mut at = records.last().map(|record| record.uuid.clone());
    while let Some(uuid) = at {
        if !chain.insert(uuid.clone()) {
            break;
        }
        at = parents.get(&uuid).cloned().flatten();
    }
    let mut transcript = Transcript {
        shortened,
        file: Some(file),
        ..Default::default()
    };
    for record in records
        .into_iter()
        .filter(|record| chain.contains(&record.uuid))
    {
        for text in &record.texts {
            transcript.push(record.role, text);
        }
    }
    Ok(transcript)
}

/// Codex's injected instructions and context are not conversation.
const CODEX_INJECTED: &[&str] = &[
    "# AGENTS.md instructions",
    "<environment_context",
    "<user_instructions",
    "<codex_internal_context",
    "<image",
];

fn find_rollout(sessions: &Path, id: &str) -> Result<PathBuf, ReadError> {
    let plain = format!("-{id}.jsonl");
    let mut seen = 0;
    let mut compressed = false;
    let mut linked = None;
    let mut stack = vec![(sessions.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > DIR_LIMIT {
                return Err(ReadError::Unsupported(
                    "there are too many saved Codex sessions to search".into(),
                ));
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() && depth < 3 {
                stack.push((entry.path(), depth + 1));
                continue;
            }
            if kind.is_symlink() && depth < 3 && !name.starts_with("rollout-") {
                linked.get_or_insert(entry.path());
                continue;
            }
            if !name.starts_with("rollout-") {
                continue;
            }
            if name.ends_with(&plain) {
                if kind.is_symlink() {
                    return Err(ReadError::Unsafe(entry.path()));
                }
                if kind.is_file() {
                    return Ok(entry.path());
                }
            } else if name.contains(&format!("{plain}.")) {
                compressed = true;
            }
        }
    }
    Err(if compressed {
        ReadError::Unsupported(
            "the Codex rollout is compressed; only uncompressed rollouts can be read".into(),
        )
    } else if let Some(path) = linked {
        ReadError::Unsafe(path)
    } else {
        ReadError::Missing
    })
}

fn codex(profile: &Path, id: &str, checkout: &Path) -> Result<Transcript, ReadError> {
    let file = find_rollout(&profile.join("sessions"), id)?;
    let meta = first_record(&file)?;
    if meta["type"] != "session_meta" || meta["payload"]["id"].as_str() != Some(id) {
        return Err(ReadError::Corrupt(
            "the rollout does not start with its own session record".into(),
        ));
    }
    if !meta["payload"]["cwd"]
        .as_str()
        .is_some_and(|cwd| same_dir(Path::new(cwd), checkout))
    {
        return Err(ReadError::WrongCheckout);
    }
    let mut transcript = Transcript::default();
    let mut shortened = false;
    jsonl(&file, &mut shortened, |value| {
        let payload = &value["payload"];
        if value["type"] != "response_item" || payload["type"] != "message" {
            return;
        }
        let (role, kind) = match payload["role"].as_str() {
            Some("user") => (Role::User, "input_text"),
            Some("assistant") => (Role::Assistant, "output_text"),
            _ => return,
        };
        for item in payload["content"].as_array().into_iter().flatten() {
            let Some(text) = item["text"].as_str().filter(|_| item["type"] == kind) else {
                continue;
            };
            if role == Role::User && CODEX_INJECTED.iter().any(|p| text.starts_with(p)) {
                continue;
            }
            transcript.push(role, text);
        }
    })?;
    transcript.shortened |= shortened;
    Ok(transcript)
}

fn open_db(path: &Path) -> Result<Connection, ReadError> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(db_error)?;
    db.busy_timeout(Duration::ZERO).map_err(db_error)?;
    Ok(db)
}

fn db_error(error: rusqlite::Error) -> ReadError {
    match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            ReadError::Locked
        }
        _ => ReadError::Corrupt(error.to_string()),
    }
}

fn role_of(role: Option<&str>) -> Option<Role> {
    match role {
        Some("user") => Some(Role::User),
        Some("assistant") => Some(Role::Assistant),
        _ => None,
    }
}

fn devin(profile: &Path, id: &str, checkout: &Path) -> Result<Transcript, ReadError> {
    let db = open_db(&plain_path(profile, &["devin", "cli", "sessions.db"])?)?;
    let (cwd, head): (String, Option<i64>) = db
        .query_row(
            "SELECT working_directory, main_chain_id FROM sessions WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(db_error)?
        .ok_or(ReadError::Missing)?;
    if !same_dir(Path::new(&cwd), checkout) {
        return Err(ReadError::WrongCheckout);
    }
    // The main chain runs from the session's head node back to its root;
    // retried and abandoned branches, and subagent chains, are off it.
    let mut node_query = db
        .prepare(
            "SELECT parent_node_id, CASE WHEN length(chat_message) <= ?3 THEN chat_message END, \
             length(chat_message) FROM message_nodes WHERE session_id = ?1 AND node_id = ?2",
        )
        .map_err(db_error)?;
    let mut transcript = Transcript::default();
    let mut nodes = Vec::new();
    let mut seen = HashSet::new();
    let mut read = 0_u64;
    let mut at = head;
    while let Some(node) = at {
        if nodes.len() >= ROW_LIMIT || read > READ_LIMIT {
            transcript.shortened = true;
            break;
        }
        if !seen.insert(node) {
            return Err(ReadError::Corrupt("the message chain loops".into()));
        }
        let (parent, message, len): (Option<i64>, Option<String>, i64) = node_query
            .query_row(params![id, node, RECORD_LIMIT as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .optional()
            .map_err(db_error)?
            .ok_or_else(|| ReadError::Corrupt("the message chain is broken".into()))?;
        read += len.max(0) as u64;
        match message {
            Some(message) => nodes.push(message),
            None => transcript.shortened = true,
        }
        at = parent;
    }
    for message in nodes.iter().rev() {
        let value: Value =
            serde_json::from_str(message).map_err(|error| ReadError::Corrupt(error.to_string()))?;
        if let (Some(role), Some(text)) =
            (role_of(value["role"].as_str()), value["content"].as_str())
        {
            transcript.push(role, text);
        }
    }
    Ok(transcript)
}

fn opencode(profile: &Path, id: &str, checkout: &Path) -> Result<Transcript, ReadError> {
    let db = open_db(&plain_path(profile, &["opencode", "opencode.db"])?)?;
    let directory: Option<String> = db
        .query_row("SELECT directory FROM session WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(db_error)?;
    let Some(directory) = directory else {
        let newer = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_v2 WHERE id = ?1)",
                [id],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false);
        return Err(if newer {
            ReadError::Unsupported(
                "this OpenCode session uses a newer storage format Goko cannot read yet".into(),
            )
        } else {
            ReadError::Missing
        });
    };
    if !same_dir(Path::new(&directory), checkout) {
        return Err(ReadError::WrongCheckout);
    }
    // Newest first so the row limit drops the oldest text.
    let mut query = db
        .prepare(
            "SELECT json_extract(m.data, '$.role'), CASE WHEN length(p.data) <= ?2 THEN p.data END \
             FROM message m JOIN part p ON p.message_id = m.id \
             WHERE m.session_id = ?1 AND json_extract(p.data, '$.type') = 'text' \
             ORDER BY m.time_created DESC, m.id DESC, p.id DESC LIMIT ?3",
        )
        .map_err(db_error)?;
    let rows = query
        .query_map(params![id, RECORD_LIMIT as i64, ROW_LIMIT as i64], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .map_err(db_error)?;
    let mut transcript = Transcript::default();
    let mut parts = Vec::new();
    let mut read = 0_u64;
    for row in rows {
        let (role, data) = row.map_err(db_error)?;
        let Some(data) = data else {
            transcript.shortened = true;
            continue;
        };
        read += data.len() as u64;
        if read > READ_LIMIT {
            transcript.shortened = true;
            break;
        }
        parts.push((role, data));
    }
    transcript.shortened |= parts.len() >= ROW_LIMIT;
    for (role, data) in parts.iter().rev() {
        let Some(role) = role_of(role.as_deref()) else {
            continue;
        };
        let part: Value =
            serde_json::from_str(data).map_err(|error| ReadError::Corrupt(error.to_string()))?;
        if part["synthetic"].as_bool() == Some(true) || part["ignored"].as_bool() == Some(true) {
            continue;
        }
        if let Some(text) = part["text"].as_str() {
            transcript.push(role, text);
        }
    }
    Ok(transcript)
}

/// The user's own words inside Antigravity's `<USER_REQUEST>` wrapper.
fn user_request(content: &str) -> &str {
    let open = "<USER_REQUEST>";
    match (content.find(open), content.find("</USER_REQUEST>")) {
        (Some(start), Some(end)) if start + open.len() <= end => &content[start + open.len()..end],
        _ => content,
    }
}

fn agy(profile: &Path, id: &str) -> Result<Transcript, ReadError> {
    let file = plain_path(
        profile,
        &["brain", id, ".system_generated", "logs", "transcript.jsonl"],
    )?;
    let mut transcript = Transcript::default();
    let mut cut = false;
    let mut shortened = false;
    jsonl(&file, &mut shortened, |value| {
        let role = match value["type"].as_str() {
            Some("USER_INPUT") => Role::User,
            Some("PLANNER_RESPONSE") => Role::Assistant,
            _ => return,
        };
        let Some(content) = value["content"].as_str() else {
            return;
        };
        if value["truncated_fields"]
            .as_array()
            .is_some_and(|fields| fields.iter().any(|field| field == "content"))
        {
            cut = true;
        }
        let text = if role == Role::User {
            user_request(content)
        } else {
            content
        };
        transcript.push(role, text);
    })?;
    transcript.shortened = shortened || cut;
    Ok(transcript)
}

/// A short Git picture of the checkout: no diffs, only names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitSnapshot {
    pub branch: Option<String>,
    pub head: String,
    pub paths: Vec<String>,
}

/// `None` for a folder that is not a Git worktree.
pub fn git_snapshot(checkout: &Path) -> Option<GitSnapshot> {
    let changes = crate::git_view::changes(checkout).ok()?;
    Some(GitSnapshot {
        paths: crate::code_index::changed_paths(&changes),
        branch: changes.branch,
        head: changes.head,
    })
}

/// The pane a handoff starts from.
pub struct Source<'a> {
    pub agent: &'a str,
    pub argv: &'a [String],
    pub checkout: &'a Path,
    /// A running pane's freshly sampled identity, or an exited pane's
    /// launch ID; the reason when there is neither.
    pub identity: Result<Identity, String>,
    /// A conversation ID the user typed.
    pub manual_id: Option<&'a str>,
}

/// The verified Claude transcript an explicit fork resumes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForkSource {
    pub id: String,
    pub file: PathBuf,
}

/// What the review dialog shows.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub draft: String,
    /// One line: which conversation was read, or why none was.
    pub status: String,
    pub conversation: Option<String>,
    pub shortened: bool,
    pub omitted: usize,
    pub accepts_manual_id: bool,
    /// Claude only, macOS/Linux: the source of an explicit fork.
    pub fork: Option<ForkSource>,
}

/// Read the source pane's conversation and build its review draft.
pub fn prepare(source: Source, folders: &Folders, git: Option<&GitSnapshot>) -> Prepared {
    let agent = source.agent;
    let label = crate::agents::label(agent).unwrap_or(agent);
    let manual = source.manual_id.map(str::trim).filter(|id| !id.is_empty());
    let evidence = match (manual, source.identity) {
        (Some(id), _) => Ok((id.to_owned(), "typed conversation ID".to_owned(), true)),
        (None, Ok(identity)) if identity.agent == agent => {
            Ok((identity.id, identity.source, false))
        }
        (None, Ok(identity)) => Err(format!("the pane runs {}, not {label}", identity.agent)),
        (None, Err(reason)) => Err(reason),
    };
    let profile = source_profile(agent, source.argv, folders);
    let read = evidence.and_then(|(id, how, manual)| {
        let profile = profile
            .as_deref()
            .ok_or_else(|| "this agent has no local history folder here".to_owned())?;
        read(agent, profile, &id, source.checkout, manual)
            .map(|transcript| (id, how, transcript))
            .map_err(|error| error.to_string())
    });
    let (draft, omitted) = draft(
        label,
        source.checkout,
        git,
        read.as_ref()
            .map(|(id, how, transcript)| (id.as_str(), how.as_str(), transcript))
            .map_err(String::as_str),
    );
    let accepts = accepts_manual_id(agent);
    match read {
        Ok((id, how, transcript)) => {
            let shortened = transcript.shortened || omitted > 0;
            Prepared {
                draft,
                status: format!(
                    "Conversation {id} — {how}{}",
                    if shortened { "; shortened" } else { "" }
                ),
                shortened,
                omitted,
                accepts_manual_id: accepts,
                fork: (agent == "claude" && !cfg!(windows))
                    .then_some(transcript.file)
                    .flatten()
                    .map(|file| ForkSource {
                        id: id.clone(),
                        file,
                    }),
                conversation: Some(id),
            }
        }
        Err(reason) => Prepared {
            draft,
            status: format!("No verified conversation text: {reason}"),
            conversation: None,
            shortened: false,
            omitted: 0,
            accepts_manual_id: accepts,
            fork: None,
        },
    }
}

/// Text without terminal control: escape sequences and control characters
/// other than newline and tab are dropped.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\n' | '\t' => out.push(c),
            '\r' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn floor(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn ceil(text: &str, mut at: usize) -> usize {
    at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

const CUT: &str = "[…shortened]";

fn speaker(role: Role) -> &'static str {
    match role {
        Role::User => "User",
        Role::Assistant => "Assistant",
    }
}

fn block(role: Role, text: &str) -> String {
    format!("{}:\n{}\n\n", speaker(role), text)
}

/// `role`'s block cut to `limit` bytes, keeping the start.
fn block_start(role: Role, text: &str, limit: usize) -> String {
    let whole = block(role, text);
    if whole.len() <= limit {
        return whole;
    }
    let room = limit.saturating_sub(block(role, "").len() + CUT.len() + 1);
    block(role, &format!("{}\n{CUT}", &text[..floor(text, room)]))
}

/// `role`'s block cut to `limit` bytes, keeping the end.
fn block_end(role: Role, text: &str, limit: usize) -> String {
    let room = limit.saturating_sub(block(role, "").len() + CUT.len() + 1);
    let start = ceil(text, text.len().saturating_sub(room));
    block(role, &format!("{CUT}\n{}", &text[start..]))
}

const NEXT: &str = "## Next step\n\n";
const HISTORY: &str = "## Earlier conversation\n\n";

/// The review draft and how many messages it left out. It never exceeds
/// [`DRAFT_LIMIT`]: past it, the first user message (cut to 2 KiB) and the
/// newest messages that fit are kept, oldest dropped first.
fn draft(
    label: &str,
    checkout: &Path,
    git: Option<&GitSnapshot>,
    read: Result<(&str, &str, &Transcript), &str>,
) -> (String, usize) {
    let mut head = format!("# Handoff from {label}\n\n");
    match &read {
        Ok((id, how, _)) => head.push_str(&format!("Conversation: {id} ({how})\n")),
        Err(reason) => head.push_str(&format!(
            "Conversation: not available ({})\n",
            clean(reason)
        )),
    }
    head.push_str(&format!(
        "Folder: {}\n",
        clean(&checkout.display().to_string())
    ));
    if let Some(git) = git {
        let branch = git
            .branch
            .as_deref()
            .map_or_else(|| "detached".to_owned(), clean);
        let at = if git.head.is_empty() {
            "no commits".to_owned()
        } else {
            clean(&git.head)
        };
        head.push_str(&format!("Branch: {branch} at {at}\n"));
        if !git.paths.is_empty() {
            head.push_str(&format!("Changed paths ({}):\n", git.paths.len()));
            for path in git.paths.iter().take(PATH_LIMIT) {
                let path = clean(path);
                head.push_str(&format!("- {}\n", &path[..floor(&path, 300)]));
            }
            if git.paths.len() > PATH_LIMIT {
                head.push_str(&format!("- … {} more\n", git.paths.len() - PATH_LIMIT));
            }
        }
    }
    head.push('\n');
    let Ok((_, _, transcript)) = read else {
        head.push_str(NEXT);
        return (head, 0);
    };
    let messages: Vec<(Role, String)> = transcript
        .messages
        .iter()
        .map(|message| (message.role, clean(&message.text)))
        .filter(|(_, text)| !text.trim().is_empty())
        .collect();
    if messages.is_empty() {
        head.push_str(NEXT);
        return (head, 0);
    }
    let reserve = 64;
    let room = DRAFT_LIMIT.saturating_sub(head.len() + HISTORY.len() + NEXT.len() + reserve);
    let blocks: Vec<String> = messages
        .iter()
        .map(|(role, text)| block(*role, text))
        .collect();
    let mut body = String::new();
    let mut omitted = 0;
    if blocks.iter().map(String::len).sum::<usize>() <= room {
        blocks.iter().for_each(|block| body.push_str(block));
    } else {
        let first = messages.iter().position(|(role, _)| *role == Role::User);
        let first_block = first
            .map(|at| block_start(Role::User, &messages[at].1, FIRST_LIMIT.min(room)))
            .unwrap_or_default();
        let mut left = room.saturating_sub(first_block.len());
        let candidates: Vec<usize> = (first.map_or(0, |at| at + 1)..messages.len()).collect();
        let mut kept = Vec::new();
        for &at in candidates.iter().rev() {
            if blocks[at].len() > left {
                break;
            }
            left -= blocks[at].len();
            kept.push(blocks[at].clone());
        }
        if let (true, Some(&newest)) = (kept.is_empty(), candidates.last()) {
            kept.push(block_end(messages[newest].0, &messages[newest].1, left));
        }
        kept.reverse();
        omitted = messages.len() - kept.len() - usize::from(first.is_some());
        body.push_str(&first_block);
        if omitted > 0 {
            body.push_str(&format!("[{omitted} earlier messages omitted]\n\n"));
        }
        kept.iter().for_each(|block| body.push_str(block));
    }
    let mut out = head;
    out.push_str(HISTORY);
    out.push_str(&body);
    out.push_str(NEXT);
    if out.len() > DRAFT_LIMIT {
        out.truncate(floor(&out, DRAFT_LIMIT));
    }
    (out, omitted)
}

/// The words that fork `source` into a destination Claude profile, after
/// checking that the destination resumes from the same `projects` folder
/// (the shared link named Claude profiles get) and the transcript is still
/// there. Never a transcript path: Claude resumes by ID.
pub fn validate_fork(
    source: &ForkSource,
    destination: Option<&Path>,
    folders: &Folders,
) -> Result<Vec<String>, String> {
    if cfg!(windows) {
        return Err("a native Claude fork is not available on Windows".into());
    }
    let meta = fs::symlink_metadata(&source.file)
        .map_err(|_| "the source Claude conversation is no longer saved".to_owned())?;
    if meta.file_type().is_symlink() || !meta.is_file() || !valid_id(&source.id) {
        return Err("the source Claude conversation cannot be verified".into());
    }
    let projects = source
        .file
        .parent()
        .and_then(Path::parent)
        .and_then(|projects| fs::canonicalize(projects).ok())
        .ok_or_else(|| "the source Claude history is no longer available".to_owned())?;
    let profile = destination
        .map(Path::to_path_buf)
        .or_else(|| folders.claude.clone())
        .ok_or_else(|| "the destination has no Claude folder".to_owned())?;
    if fs::canonicalize(profile.join("projects")).ok() != Some(projects) {
        return Err("the destination account does not share this Claude history".into());
    }
    Ok(vec![
        "--resume".into(),
        source.id.clone(),
        "--fork-session".into(),
    ])
}

#[cfg(test)]
mod tests;
