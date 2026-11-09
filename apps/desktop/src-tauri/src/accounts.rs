//! Multiple named accounts per agent (`claude`, `codex`), persisted as one
//! JSON value under the `accounts` setting.
//!
//! Every agent has an implicit Default account — id `default`, never stored —
//! that keeps today's launch behavior and folder (`$CLAUDE_CONFIG_DIR` or
//! `~/.claude` for Claude, `$CODEX_HOME` or `~/.codex` for Codex). Added
//! accounts are `{id, name, dir}` rows; `accounts.<agent>.active` names the
//! one new sessions use, and a missing, malformed, unknown, or removed active
//! id reads as Default. Each command reads the whole value, changes it, and
//! writes it back in one call; a lost update between two simultaneous
//! commands is accepted.
//!
//! `add` creates the account folder (mode 0700 when missing), makes the
//! account active, and links the entries the agent can share with the default
//! folder — never credentials or Codex `sessions`, and never overwriting an
//! existing entry. `remove` deletes only the stored row, never the folder;
//! Default can be neither renamed nor removed.
//!
//! `sign_in_launch` builds the agent's own login command for an added account
//! — `env NAME=<dir> <bin> … login` as one shell-quoted string — which the UI
//! starts through `session_create`'s custom-command path. Goko never handles
//! the login itself.

use crate::agents;
use crate::error::{BridgeError, Code};
use crate::store::{Storage, Store};
use crate::usage::{codex_home_from, UsageState};
use protocol::message::ErrorCode;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder};
use std::os::unix::fs::{symlink, DirBuilderExt};
use std::path::{Path, PathBuf};
use tauri::State;

/// Settings key holding the whole `accounts` JSON value.
const ACCOUNTS_KEY: &str = "accounts";
/// The implicit Default account's id; it is never stored as an item.
pub(crate) const DEFAULT_ID: &str = "default";
const MAX_NAME_CHARS: usize = 40;

/// The account a launch resolves to. `dir` is `None` for Default, which
/// launches exactly as before; a stored account carries its folder. The
/// folder's existence is checked by the caller at launch time, not here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchAccount {
    pub id: String,
    pub dir: Option<PathBuf>,
}

/// One persisted account row, `{"id","name","dir"}` under
/// `accounts.<agent>.items`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredAccount {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
}

/// One agent's stored accounts: the active id plus the added rows.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentAccounts {
    #[serde(default = "default_active")]
    pub active: String,
    #[serde(default)]
    pub items: Vec<StoredAccount>,
}

impl Default for AgentAccounts {
    fn default() -> Self {
        Self {
            active: default_active(),
            items: Vec::new(),
        }
    }
}

/// The whole `accounts` setting. Missing, unreadable, or malformed JSON
/// reads as Default-only, like `acp.agents`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Accounts {
    #[serde(default)]
    pub claude: AgentAccounts,
    #[serde(default)]
    pub codex: AgentAccounts,
}

/// The `accounts_get` payload: both agents, Default first in each item list.
#[derive(Debug, Serialize)]
pub struct AccountsView {
    pub claude: AgentView,
    pub codex: AgentView,
}

#[derive(Debug, Serialize)]
pub struct AgentView {
    pub active: String,
    pub items: Vec<AccountView>,
}

#[derive(Debug, Serialize)]
pub struct AccountView {
    pub id: String,
    pub name: String,
    pub dir: String,
    pub is_default: bool,
}

/// The `accounts_sign_in` payload: the login command as one shell-quoted
/// string for `session_create`'s `command`, the account folder to run it in,
/// and the session title.
#[derive(Debug, PartialEq, Serialize)]
pub struct SignInLaunch {
    pub command: String,
    pub cwd: String,
    pub title: String,
}

/// The Default account's folder for `agent` from explicit environment
/// values, so tests never mutate the process environment. Claude reads
/// `CLAUDE_CONFIG_DIR` then `$HOME/.claude`; Codex reads `CODEX_HOME` then
/// `$HOME/.codex` through [`codex_home_from`] — the resolution
/// `commands::resolve_launch` uses today. An unknown agent resolves to `None`.
pub fn default_dir_from(
    agent: &str,
    claude_config_dir: Option<OsString>,
    codex_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    match agent {
        "claude" => claude_config_dir
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                home.filter(|home| !home.is_empty())
                    .map(|home| PathBuf::from(home).join(".claude"))
            }),
        "codex" => codex_home_from(codex_home, home),
        _ => None,
    }
}

/// [`default_dir_from`] reading the process environment.
pub fn default_dir(agent: &str) -> Option<PathBuf> {
    default_dir_from(
        agent,
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        std::env::var_os("CODEX_HOME"),
        std::env::var_os("HOME"),
    )
}

/// The stored accounts, or Default-only when the setting is missing,
/// unreadable, or malformed. Infallible: the usage poller calls this every
/// tick and must never fail.
pub fn read(storage: &Storage) -> Accounts {
    storage.get().and_then(load).unwrap_or_default()
}

/// The agent's active account for launch: the stored account `active` names,
/// or Default when the setting is missing or malformed, the active id is
/// unknown or was removed, the agent has no accounts, or the store cannot be
/// read (launching is daemon-backed and must keep working without storage). An
/// unknown agent is `invalid_argument`.
pub fn resolve(agent: &str, storage: &Storage) -> Result<LaunchAccount, BridgeError> {
    let accounts = read(storage);
    let agent_accounts = accounts
        .agent(agent)
        .ok_or_else(|| invalid(format!("unknown agent '{agent}'")))?;
    Ok(
        match agent_accounts
            .items
            .iter()
            .find(|item| item.id == agent_accounts.active)
        {
            Some(item) => LaunchAccount {
                id: item.id.clone(),
                dir: Some(item.dir.clone()),
            },
            None => LaunchAccount {
                id: DEFAULT_ID.to_owned(),
                dir: None,
            },
        },
    )
}

/// The agent's own login command for an added account, for the UI to start
/// through the existing custom-command path. The words are joined with
/// `shlex::try_join` so `resolve_launch`'s `shlex::split` restores them
/// exactly, even when the folder has spaces; `cwd` is the account folder and
/// `title` is `Sign in · <name>`. Default, an unknown id or agent, a missing
/// folder, a binary absent from `path_var`, and an unquotable word are all
/// `invalid_argument`. `path_var` is a parameter so tests never touch the
/// process environment.
pub fn sign_in_launch(
    accounts: &Accounts,
    agent: &str,
    id: &str,
    path_var: Option<&OsStr>,
) -> Result<SignInLaunch, BridgeError> {
    let (agent_accounts, bin, label, env_var, tail) = match agent {
        "claude" => (
            &accounts.claude,
            "claude",
            "Claude Code",
            "CLAUDE_CONFIG_DIR",
            &["auth", "login"][..],
        ),
        "codex" => (
            &accounts.codex,
            "codex",
            "Codex",
            "CODEX_HOME",
            &["login"][..],
        ),
        _ => return Err(invalid(format!("unknown agent '{agent}'"))),
    };
    if id == DEFAULT_ID {
        return Err(invalid(
            "the Default account's login is yours, sign in from a terminal",
        ));
    }
    let account = agent_accounts
        .items
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| invalid(format!("no {agent} account '{id}'")))?;
    if !account.dir.is_dir() {
        return Err(invalid(format!(
            "the account folder {} no longer exists",
            account.dir.display()
        ))
        .with_hint("restore the folder or pick another account in Settings › Accounts"));
    }
    let binary = agents::resolve(bin, path_var).ok_or_else(|| {
        invalid(format!("{label} '{bin}' was not found on PATH")).with_hint("install it")
    })?;
    let mut words = vec![
        "env".to_owned(),
        format!("{env_var}={}", account.dir.display()),
        binary.display().to_string(),
    ];
    words.extend(tail.iter().map(|word| word.to_string()));
    let command = shlex::try_join(words.iter().map(String::as_str))
        .map_err(|error| invalid(format!("the sign-in command cannot be quoted: {error}")))?;
    Ok(SignInLaunch {
        command,
        cwd: account.dir.display().to_string(),
        title: format!("Sign in · {}", account.name),
    })
}

/// Validate and store a new account for `agent`, make it active, create its
/// folder, and link the shared entries from `default_dir` (the agent's
/// Default folder, resolved by the caller). `home` expands a leading `~` in
/// `dir`. Any rejection stores nothing.
pub fn add(
    store: &Store,
    agent: &str,
    name: &str,
    dir: &str,
    default_dir: Option<&Path>,
    home: Option<&OsStr>,
) -> Result<Accounts, BridgeError> {
    let mut accounts = load(store)?;
    let agent_accounts = accounts
        .agent_mut(agent)
        .ok_or_else(|| invalid(format!("unknown agent '{agent}'")))?;
    let name = validate_name(name)?;
    if name_taken(&agent_accounts.items, &name, None) {
        return Err(invalid(format!("an account named '{name}' already exists")));
    }
    let dir = validate_dir(dir, default_dir, &agent_accounts.items, home)?;
    ensure_dir(&dir)?;
    if let Some(default_dir) = default_dir {
        link_shared(agent, default_dir, &dir);
    }
    let id = new_id(&agent_accounts.items);
    agent_accounts.items.push(StoredAccount {
        id: id.clone(),
        name,
        dir,
    });
    agent_accounts.active = id;
    save(store, &accounts)?;
    Ok(accounts)
}

/// Change only the stored name of account `id`. The Default account cannot
/// be renamed; an unknown id is `invalid_argument`.
pub fn rename(store: &Store, agent: &str, id: &str, name: &str) -> Result<Accounts, BridgeError> {
    let mut accounts = load(store)?;
    let agent_accounts = accounts
        .agent_mut(agent)
        .ok_or_else(|| invalid(format!("unknown agent '{agent}'")))?;
    if id == DEFAULT_ID {
        return Err(invalid("the Default account cannot be renamed"));
    }
    let name = validate_name(name)?;
    let index = agent_accounts
        .items
        .iter()
        .position(|item| item.id == id)
        .ok_or_else(|| invalid(format!("no {agent} account '{id}'")))?;
    if name_taken(&agent_accounts.items, &name, Some(id)) {
        return Err(invalid(format!("an account named '{name}' already exists")));
    }
    agent_accounts.items[index].name = name;
    save(store, &accounts)?;
    Ok(accounts)
}

/// Delete the stored row for account `id`, never its folder. Removing the
/// active account makes Default active; an unknown id is a no-op. The
/// Default account cannot be removed.
pub fn remove(store: &Store, agent: &str, id: &str) -> Result<Accounts, BridgeError> {
    let mut accounts = load(store)?;
    let agent_accounts = accounts
        .agent_mut(agent)
        .ok_or_else(|| invalid(format!("unknown agent '{agent}'")))?;
    if id == DEFAULT_ID {
        return Err(invalid("the Default account cannot be removed"));
    }
    if let Some(index) = agent_accounts.items.iter().position(|item| item.id == id) {
        agent_accounts.items.remove(index);
        if agent_accounts.active == id {
            agent_accounts.active = default_active();
        }
    }
    save(store, &accounts)?;
    Ok(accounts)
}

/// Make account `id` — `default` or a stored id — the agent's active account.
/// An unknown id is `invalid_argument`.
pub fn set_active(store: &Store, agent: &str, id: &str) -> Result<Accounts, BridgeError> {
    let mut accounts = load(store)?;
    let agent_accounts = accounts
        .agent_mut(agent)
        .ok_or_else(|| invalid(format!("unknown agent '{agent}'")))?;
    if id == DEFAULT_ID {
        agent_accounts.active = default_active();
    } else if agent_accounts.items.iter().any(|item| item.id == id) {
        agent_accounts.active = id.to_owned();
    } else {
        return Err(invalid(format!("no {agent} account '{id}'")));
    }
    save(store, &accounts)?;
    Ok(accounts)
}

/// Symlink every shared entry that exists in `default_dir` into `dir`,
/// skipping entries already present in `dir` and entries that cannot be
/// linked. Returns the names that were linked. Credential files
/// (`auth.json`, `.credentials.json`, `.claude.json`) and Codex `sessions`
/// are deliberately absent from the shared lists: accounts must not share
/// logins or usage history.
pub fn link_shared(agent: &str, default_dir: &Path, dir: &Path) -> Vec<String> {
    let mut linked = Vec::new();
    for &entry in shared_entries(agent) {
        let source = default_dir.join(entry);
        let target = dir.join(entry);
        if fs::symlink_metadata(&source).is_err() || fs::symlink_metadata(&target).is_ok() {
            continue;
        }
        if symlink(&source, &target).is_ok() {
            linked.push(entry.to_owned());
        }
    }
    linked
}

#[tauri::command]
pub async fn accounts_get(storage: State<'_, Storage>) -> Result<AccountsView, BridgeError> {
    Ok(view(&load(storage.get()?)?))
}

#[tauri::command]
pub async fn accounts_add(
    storage: State<'_, Storage>,
    usage: State<'_, UsageState>,
    agent: String,
    name: String,
    dir: String,
) -> Result<AccountsView, BridgeError> {
    let accounts = add(
        storage.get()?,
        &agent,
        &name,
        &dir,
        default_dir(&agent).as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    usage.refresh();
    Ok(view(&accounts))
}

#[tauri::command]
pub async fn accounts_rename(
    storage: State<'_, Storage>,
    agent: String,
    id: String,
    name: String,
) -> Result<AccountsView, BridgeError> {
    Ok(view(&rename(storage.get()?, &agent, &id, &name)?))
}

#[tauri::command]
pub async fn accounts_remove(
    storage: State<'_, Storage>,
    usage: State<'_, UsageState>,
    agent: String,
    id: String,
) -> Result<AccountsView, BridgeError> {
    let accounts = remove(storage.get()?, &agent, &id)?;
    usage.refresh();
    Ok(view(&accounts))
}

#[tauri::command]
pub async fn accounts_set_active(
    storage: State<'_, Storage>,
    usage: State<'_, UsageState>,
    agent: String,
    id: String,
) -> Result<AccountsView, BridgeError> {
    let accounts = set_active(storage.get()?, &agent, &id)?;
    usage.refresh();
    Ok(view(&accounts))
}

/// The login command for an added account; a broken store reads as Default
/// only, so every rejection here is `invalid_argument`.
#[tauri::command]
pub async fn accounts_sign_in(
    storage: State<'_, Storage>,
    agent: String,
    id: String,
) -> Result<SignInLaunch, BridgeError> {
    sign_in_launch(
        &read(&storage),
        &agent,
        &id,
        std::env::var_os("PATH").as_deref(),
    )
}

/// The stored accounts, propagating storage errors but not malformed JSON.
fn load(store: &Store) -> Result<Accounts, BridgeError> {
    let Some(text) = store.get_setting(ACCOUNTS_KEY)? else {
        return Ok(Accounts::default());
    };
    Ok(parse_accounts(&text))
}

fn parse_accounts(text: &str) -> Accounts {
    serde_json::from_str::<Accounts>(text)
        .map(Accounts::normalized)
        .unwrap_or_default()
}

fn save(store: &Store, accounts: &Accounts) -> Result<(), BridgeError> {
    let text = serde_json::to_string(accounts).map_err(|error| {
        BridgeError::new(
            Code::Storage,
            format!("cannot encode the accounts: {error}"),
        )
    })?;
    store.set_setting(ACCOUNTS_KEY, &text)
}

impl Accounts {
    fn agent(&self, agent: &str) -> Option<&AgentAccounts> {
        match agent {
            "claude" => Some(&self.claude),
            "codex" => Some(&self.codex),
            _ => None,
        }
    }

    fn agent_mut(&mut self, agent: &str) -> Option<&mut AgentAccounts> {
        match agent {
            "claude" => Some(&mut self.claude),
            "codex" => Some(&mut self.codex),
            _ => None,
        }
    }

    /// Canonicalize after parsing: Default is never an item, ids are unique,
    /// and an active id that matches no item falls back to Default.
    fn normalized(mut self) -> Self {
        self.claude.normalize();
        self.codex.normalize();
        self
    }
}

impl AgentAccounts {
    fn normalize(&mut self) {
        self.items
            .retain(|item| !item.id.is_empty() && item.id != DEFAULT_ID);
        let mut ids = std::collections::HashSet::new();
        self.items.retain(|item| ids.insert(item.id.clone()));
        if self.active != DEFAULT_ID && !self.items.iter().any(|item| item.id == self.active) {
            self.active = default_active();
        }
    }
}

/// The first 8 hex characters of a v4 UUID, regenerated on a collision
/// within the agent. It can never be `default`, but the check keeps the
/// invariant explicit.
fn new_id(items: &[StoredAccount]) -> String {
    loop {
        let id = uuid::Uuid::new_v4().to_string();
        let id = id[..8].to_owned();
        if id != DEFAULT_ID && !items.iter().any(|item| item.id == id) {
            return id;
        }
    }
}

/// Trim and bound the name; uniqueness is the caller's check.
fn validate_name(name: &str) -> Result<String, BridgeError> {
    let name = name.trim();
    let len = name.chars().count();
    if len == 0 || len > MAX_NAME_CHARS {
        return Err(invalid("the account name must be 1 to 40 characters"));
    }
    Ok(name.to_owned())
}

/// Case-insensitive, and the implicit Default account's name counts as taken so the
/// account list never shows two rows named Default.
fn name_taken(items: &[StoredAccount], name: &str, except: Option<&str>) -> bool {
    let lowered = name.to_lowercase();
    lowered == "default"
        || items
            .iter()
            .any(|item| Some(item.id.as_str()) != except && item.name.to_lowercase() == lowered)
}

/// Expand `~`, require an absolute path free of NUL and newlines, and reject
/// the agent's Default folder and folders another account already uses.
fn validate_dir(
    raw: &str,
    default_dir: Option<&Path>,
    items: &[StoredAccount],
    home: Option<&OsStr>,
) -> Result<PathBuf, BridgeError> {
    if raw.contains('\0') || raw.contains('\n') || raw.contains('\r') {
        return Err(invalid("the folder must not contain a NUL or newline"));
    }
    let dir = normalize_path(&expand_home(raw, home));
    if !dir.is_absolute() {
        return Err(invalid("the folder must be an absolute path"));
    }
    if default_dir.is_some_and(|default| normalize_path(default) == dir) {
        return Err(invalid("the folder is the agent's default folder"));
    }
    if items.iter().any(|item| item.dir == dir) {
        return Err(invalid("another account already uses this folder"));
    }
    Ok(dir)
}

/// `~` and `~/…` expand from `home`; anything else is returned unchanged.
fn expand_home(dir: &str, home: Option<&OsStr>) -> PathBuf {
    if dir == "~" {
        return home.map_or_else(|| PathBuf::from(dir), PathBuf::from);
    }
    if let Some(rest) = dir.strip_prefix("~/") {
        if let Some(home) = home {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(dir)
}

/// Lexical normalization only: `.` and repeated separators collapse, `..`
/// and symlinks are kept.
fn normalize_path(path: &Path) -> PathBuf {
    path.components().collect()
}

/// Create `dir` (and any missing parents) with mode 0700 when it does not
/// already exist as a directory.
fn ensure_dir(dir: &Path) -> Result<(), BridgeError> {
    if dir.is_dir() {
        return Ok(());
    }
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| {
            invalid(format!(
                "cannot create the account folder {}: {error}",
                dir.display()
            ))
        })
}

/// The entries an agent can share with its Default folder. Anything not
/// listed — credentials, `.claude.json`, Codex `sessions` — is never linked.
fn shared_entries(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" => &[
            "projects",
            "settings.json",
            "CLAUDE.md",
            "skills",
            "commands",
            "agents",
            "plugins",
            "hooks",
        ],
        "codex" => &["config.toml", "AGENTS.md", "skills", "prompts"],
        _ => &[],
    }
}

fn view(accounts: &Accounts) -> AccountsView {
    view_with(
        accounts,
        default_dir("claude").as_deref(),
        default_dir("codex").as_deref(),
    )
}

fn view_with(
    accounts: &Accounts,
    claude_default: Option<&Path>,
    codex_default: Option<&Path>,
) -> AccountsView {
    AccountsView {
        claude: agent_view(&accounts.claude, claude_default),
        codex: agent_view(&accounts.codex, codex_default),
    }
}

fn agent_view(agent_accounts: &AgentAccounts, default_dir: Option<&Path>) -> AgentView {
    let mut items = Vec::with_capacity(agent_accounts.items.len() + 1);
    items.push(AccountView {
        id: DEFAULT_ID.to_owned(),
        name: "Default".to_owned(),
        dir: default_dir.map_or_else(String::new, |dir| dir.display().to_string()),
        is_default: true,
    });
    items.extend(agent_accounts.items.iter().map(|item| AccountView {
        id: item.id.clone(),
        name: item.name.clone(),
        dir: item.dir.display().to_string(),
        is_default: false,
    }));
    AgentView {
        active: agent_accounts.active.clone(),
        items,
    }
}

fn default_active() -> String {
    DEFAULT_ID.to_owned()
}

fn invalid(message: impl Into<String>) -> BridgeError {
    BridgeError::new(Code::Daemon(ErrorCode::InvalidArgument), message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use crate::store::{Storage, Store};
    use crate::test_support::TempDir;
    use protocol::message::ErrorCode;
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn open(dir: &TempDir) -> Storage {
        Storage::open(dir.path())
    }

    fn store(storage: &Storage) -> &Store {
        storage.get().unwrap()
    }

    /// The Default folder tests compare against: inside the temp dir, per agent.
    fn default_folder(home: &TempDir, agent: &str) -> PathBuf {
        home.path().join(format!(".{agent}-default"))
    }

    fn add_ok(storage: &Storage, home: &TempDir, agent: &str, name: &str, dir: &Path) -> Accounts {
        let default = default_folder(home, agent);
        add(
            store(storage),
            agent,
            name,
            &dir.to_string_lossy(),
            Some(default.as_path()),
            Some(home.path().as_os_str()),
        )
        .unwrap()
    }

    /// An executable fake `name` inside `dir`; tests canonicalize it because
    /// `agents::resolve` canonicalizes what it finds.
    fn fake_bin(dir: &Path, name: &str) -> PathBuf {
        let file = dir.join(name);
        fs::write(&file, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        file
    }

    /// A `PATH` value naming `dirs` in order — never the process environment.
    fn path_var(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs.iter().map(|dir| dir.as_os_str())).unwrap()
    }

    /// Everything a `sign_in_launch` test needs: temp storage holding one
    /// added "Work" account for `agent` in `<root>/<folder>`, a `bin` dir with
    /// fake `claude`/`codex`, and a `PATH` naming only it.
    struct SignIn {
        root: TempDir,
        path: OsString,
        accounts: Accounts,
        id: String,
        folder: PathBuf,
    }

    fn sign_in_setup(agent: &str, folder: &str) -> SignIn {
        let root = TempDir::new("accounts-signin");
        let storage = open(&root);
        let bin = root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        fake_bin(&bin, "claude");
        fake_bin(&bin, "codex");
        let path = path_var(&[bin.as_path()]);
        let folder = root.path().join(folder);
        let accounts = add_ok(&storage, &root, agent, "Work", &folder);
        let id = accounts.agent(agent).unwrap().items[0].id.clone();
        SignIn {
            root,
            path,
            accounts,
            id,
            folder,
        }
    }

    #[test]
    fn a_missing_setting_reads_as_default_only() {
        let dir = TempDir::new("accounts-none");
        let storage = open(&dir);

        let accounts = read(&storage);

        assert_eq!(accounts.claude.active, DEFAULT_ID);
        assert!(accounts.claude.items.is_empty());
        assert_eq!(accounts.codex.active, DEFAULT_ID);
        assert!(accounts.codex.items.is_empty());
        let launch = resolve("claude", &storage).unwrap();
        assert_eq!(launch.id, DEFAULT_ID);
        assert_eq!(launch.dir, None);
    }

    #[test]
    fn malformed_accounts_json_reads_as_default_only() {
        let dir = TempDir::new("accounts-malformed");
        let storage = open(&dir);

        for bad in [
            "{not json",
            "[]",
            "42",
            "\"text\"",
            "{\"claude\":\"x\"}",
            "null",
        ] {
            store(&storage).set_setting(ACCOUNTS_KEY, bad).unwrap();
            let accounts = read(&storage);
            assert_eq!(accounts.claude.active, DEFAULT_ID, "input: {bad}");
            assert!(accounts.claude.items.is_empty(), "input: {bad}");
            assert_eq!(accounts.codex.active, DEFAULT_ID, "input: {bad}");
            assert!(accounts.codex.items.is_empty(), "input: {bad}");
        }
    }

    #[test]
    fn default_dir_uses_the_env_override_then_home() {
        let claude_env = Some(OsString::from("/env/claude"));
        let codex_env = Some(OsString::from("/env/codex"));
        let home = Some(OsString::from("/home/u"));

        assert_eq!(
            default_dir_from("claude", claude_env.clone(), None, home.clone()),
            Some(PathBuf::from("/env/claude"))
        );
        assert_eq!(
            default_dir_from("claude", Some(OsString::new()), None, home.clone()),
            Some(PathBuf::from("/home/u/.claude"))
        );
        assert_eq!(
            default_dir_from("claude", None, None, home.clone()),
            Some(PathBuf::from("/home/u/.claude"))
        );
        assert_eq!(
            default_dir_from("codex", None, codex_env.clone(), home.clone()),
            Some(PathBuf::from("/env/codex"))
        );
        assert_eq!(
            default_dir_from("codex", None, Some(OsString::new()), home.clone()),
            Some(PathBuf::from("/home/u/.codex"))
        );
        assert_eq!(
            default_dir_from("codex", None, None, home.clone()),
            Some(PathBuf::from("/home/u/.codex"))
        );
        assert_eq!(default_dir_from("claude", None, None, None), None);
        assert_eq!(default_dir_from("codex", None, None, None), None);
        assert_eq!(default_dir_from("shell", None, None, home), None);
    }

    #[test]
    fn add_stores_the_account_marks_it_active_and_creates_its_folder() {
        let dir = TempDir::new("accounts-add");
        let storage = open(&dir);
        let folder = dir.path().join("work");

        let accounts = add_ok(&storage, &dir, "claude", "Work", &folder);

        let claude = &accounts.claude;
        assert_eq!(claude.items.len(), 1);
        let item = &claude.items[0];
        assert_ne!(item.id, DEFAULT_ID);
        assert_eq!(item.id.len(), 8);
        assert!(item.id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(item.name, "Work");
        assert_eq!(item.dir, folder);
        assert_eq!(claude.active, item.id);
        assert!(accounts.codex.items.is_empty());
        assert_eq!(accounts.codex.active, DEFAULT_ID);

        // Persisted under `accounts` in the documented shape.
        let stored = store(&storage).get_setting(ACCOUNTS_KEY).unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(value["claude"]["active"], serde_json::json!(item.id));
        assert_eq!(value["claude"]["items"][0]["name"], "Work");
        assert_eq!(
            value["claude"]["items"][0]["dir"].as_str().unwrap(),
            folder.to_str().unwrap()
        );
        assert!(value["claude"]["items"][0].get("is_default").is_none());
        assert_eq!(value["codex"]["active"], "default");

        // The folder exists with mode 0700 and resolve() returns the account.
        let mode = fs::metadata(&folder).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let launch = resolve("claude", &storage).unwrap();
        assert_eq!(launch.id, item.id);
        assert_eq!(launch.dir, Some(folder));
    }

    #[test]
    fn add_rejects_an_empty_name() {
        let dir = TempDir::new("accounts-name-empty");
        let storage = open(&dir);
        let folder = dir.path().join("x");

        for name in ["", "   "] {
            let error = add(
                store(&storage),
                "claude",
                name,
                folder.to_str().unwrap(),
                None,
                None,
            )
            .unwrap_err();
            assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        }
        assert!(read(&storage).claude.items.is_empty());
        assert!(store(&storage).get_setting(ACCOUNTS_KEY).unwrap().is_none());
        assert!(!folder.exists());
    }

    #[test]
    fn add_rejects_a_name_over_40_characters() {
        let dir = TempDir::new("accounts-name-long");
        let storage = open(&dir);

        // The boundary itself is fine.
        add_ok(
            &storage,
            &dir,
            "claude",
            &"x".repeat(40),
            &dir.path().join("forty"),
        );

        let error = add(
            store(&storage),
            "claude",
            &"x".repeat(41),
            dir.path().join("long").to_str().unwrap(),
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert_eq!(read(&storage).claude.items.len(), 1);
        assert!(!dir.path().join("long").exists());
    }

    #[test]
    fn add_trims_the_name_before_storing() {
        let dir = TempDir::new("accounts-name-trim");
        let storage = open(&dir);

        let accounts = add_ok(&storage, &dir, "claude", "  Work  ", &dir.path().join("w"));

        assert_eq!(accounts.claude.items[0].name, "Work");
    }

    #[test]
    fn add_rejects_a_name_matching_another_account_case_insensitively() {
        let dir = TempDir::new("accounts-name-dup");
        let storage = open(&dir);
        add_ok(&storage, &dir, "claude", "Work", &dir.path().join("one"));

        for name in ["work", "WORK", "Work"] {
            let error = add(
                store(&storage),
                "claude",
                name,
                dir.path().join("two").to_str().unwrap(),
                None,
                None,
            )
            .unwrap_err();
            assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        }
        // The same name under the other agent is fine.
        add_ok(&storage, &dir, "codex", "work", &dir.path().join("cx"));
        assert_eq!(read(&storage).claude.items.len(), 1);
        assert_eq!(read(&storage).codex.items.len(), 1);
    }

    #[test]
    fn add_and_rename_reject_the_reserved_name_default() {
        let dir = TempDir::new("accounts-name-default");
        let storage = open(&dir);
        let before = add_ok(&storage, &dir, "claude", "Work", &dir.path().join("one"));
        let id = before.claude.items[0].id.clone();

        for name in ["Default", "default", " DEFAULT "] {
            let added = add(
                store(&storage),
                "claude",
                name,
                dir.path().join("two").to_str().unwrap(),
                None,
                None,
            )
            .unwrap_err();
            assert_eq!(added.code, Code::Daemon(ErrorCode::InvalidArgument));
            let renamed = rename(store(&storage), "claude", &id, name).unwrap_err();
            assert_eq!(renamed.code, Code::Daemon(ErrorCode::InvalidArgument));
        }
        let after = read(&storage).claude;
        assert_eq!(after.items.len(), 1);
        assert_eq!(after.items[0].name, "Work");
    }

    #[test]
    fn add_rejects_a_relative_folder() {
        let dir = TempDir::new("accounts-dir-relative");
        let storage = open(&dir);

        let error = add(
            store(&storage),
            "claude",
            "Work",
            "some/relative/dir",
            None,
            Some(dir.path().as_os_str()),
        )
        .unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(read(&storage).claude.items.is_empty());
        assert!(store(&storage).get_setting(ACCOUNTS_KEY).unwrap().is_none());
    }

    #[test]
    fn add_rejects_a_folder_with_a_nul_or_newline() {
        let dir = TempDir::new("accounts-dir-control");
        let storage = open(&dir);

        for bad in ["/tmp/acc\nct", "/tmp/acc\0ct", "/tmp/acc\rct"] {
            let error = add(store(&storage), "claude", "Work", bad, None, None).unwrap_err();
            assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        }
        assert!(read(&storage).claude.items.is_empty());
        assert!(store(&storage).get_setting(ACCOUNTS_KEY).unwrap().is_none());
    }

    #[test]
    fn add_rejects_the_default_folder() {
        let dir = TempDir::new("accounts-dir-default");
        let storage = open(&dir);
        let default = default_folder(&dir, "claude");

        let error = add(
            store(&storage),
            "claude",
            "Work",
            default.to_str().unwrap(),
            Some(default.as_path()),
            Some(dir.path().as_os_str()),
        )
        .unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(read(&storage).claude.items.is_empty());
    }

    #[test]
    fn add_rejects_a_folder_another_account_uses() {
        let dir = TempDir::new("accounts-dir-dup");
        let storage = open(&dir);
        let folder = dir.path().join("shared");
        add_ok(&storage, &dir, "claude", "One", &folder);

        let error = add(
            store(&storage),
            "claude",
            "Two",
            folder.to_str().unwrap(),
            Some(default_folder(&dir, "claude").as_path()),
            Some(dir.path().as_os_str()),
        )
        .unwrap_err();
        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert_eq!(read(&storage).claude.items.len(), 1);
    }

    #[test]
    fn add_expands_a_leading_tilde_from_home() {
        let dir = TempDir::new("accounts-dir-tilde");
        let storage = open(&dir);
        let default = default_folder(&dir, "claude");

        let accounts = add(
            store(&storage),
            "claude",
            "Work",
            "~/acct",
            Some(default.as_path()),
            Some(dir.path().as_os_str()),
        )
        .unwrap();

        let expected = dir.path().join("acct");
        assert_eq!(accounts.claude.items[0].dir, expected);
        assert!(expected.is_dir());
    }

    #[test]
    fn rename_changes_only_the_name() {
        let dir = TempDir::new("accounts-rename");
        let storage = open(&dir);
        let folder = dir.path().join("work");
        let accounts = add_ok(&storage, &dir, "claude", "Work", &folder);
        let id = accounts.claude.items[0].id.clone();

        let renamed = rename(store(&storage), "claude", &id, "  Job  ").unwrap();

        let item = &renamed.claude.items[0];
        assert_eq!(item.id, id);
        assert_eq!(item.name, "Job");
        assert_eq!(item.dir, folder);
        assert_eq!(renamed.claude.active, id);
        assert_eq!(read(&storage).claude.items[0].name, "Job");
        assert_eq!(read(&storage).claude.items[0].dir, folder);
    }

    #[test]
    fn rename_rejects_a_name_used_by_another_account() {
        let dir = TempDir::new("accounts-rename-dup");
        let storage = open(&dir);
        add_ok(&storage, &dir, "claude", "One", &dir.path().join("one"));
        let two = add_ok(&storage, &dir, "claude", "Two", &dir.path().join("two"));
        let two_id = two.claude.items[1].id.clone();

        for name in ["one", "ONE"] {
            let error = rename(store(&storage), "claude", &two_id, name).unwrap_err();
            assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        }
        assert_eq!(read(&storage).claude.items[1].name, "Two");
        // Renaming to its own name in another case is fine.
        rename(store(&storage), "claude", &two_id, "two").unwrap();
        assert_eq!(read(&storage).claude.items[1].name, "two");
    }

    #[test]
    fn rename_rejects_the_default_account() {
        let dir = TempDir::new("accounts-rename-default");
        let storage = open(&dir);

        let error = rename(store(&storage), "claude", DEFAULT_ID, "Job").unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(read(&storage).claude.items.is_empty());
    }

    #[test]
    fn rename_rejects_an_unknown_id() {
        let dir = TempDir::new("accounts-rename-unknown");
        let storage = open(&dir);
        add_ok(&storage, &dir, "claude", "Work", &dir.path().join("w"));

        let error = rename(store(&storage), "claude", "ghost", "Job").unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert_eq!(read(&storage).claude.items[0].name, "Work");
    }

    #[test]
    fn remove_deletes_the_entry_and_never_the_folder() {
        let dir = TempDir::new("accounts-remove");
        let storage = open(&dir);
        let folder = dir.path().join("work");
        let accounts = add_ok(&storage, &dir, "claude", "Work", &folder);
        let id = accounts.claude.items[0].id.clone();

        let removed = remove(store(&storage), "claude", &id).unwrap();

        assert!(removed.claude.items.is_empty());
        assert_eq!(removed.claude.active, DEFAULT_ID);
        assert!(folder.is_dir(), "remove must never delete the folder");
        assert!(read(&storage).claude.items.is_empty());
    }

    #[test]
    fn remove_of_the_active_account_makes_default_active() {
        let dir = TempDir::new("accounts-remove-active");
        let storage = open(&dir);
        add_ok(&storage, &dir, "claude", "One", &dir.path().join("one"));
        let two = add_ok(&storage, &dir, "claude", "Two", &dir.path().join("two"));
        let two_id = two.claude.items[1].id.clone();

        let removed = remove(store(&storage), "claude", &two_id).unwrap();

        assert_eq!(removed.claude.items.len(), 1);
        assert_eq!(removed.claude.active, DEFAULT_ID);
    }

    #[test]
    fn remove_of_another_account_keeps_the_active_one() {
        let dir = TempDir::new("accounts-remove-inactive");
        let storage = open(&dir);
        let one = add_ok(&storage, &dir, "claude", "One", &dir.path().join("one"));
        let one_id = one.claude.items[0].id.clone();
        let two = add_ok(&storage, &dir, "claude", "Two", &dir.path().join("two"));
        let two_id = two.claude.items[1].id.clone();
        set_active(store(&storage), "claude", &one_id).unwrap();

        let removed = remove(store(&storage), "claude", &two_id).unwrap();

        assert_eq!(removed.claude.active, one_id);
        assert_eq!(removed.claude.items.len(), 1);
        assert_eq!(removed.claude.items[0].id, one_id);
    }

    #[test]
    fn remove_rejects_the_default_account() {
        let dir = TempDir::new("accounts-remove-default");
        let storage = open(&dir);

        let error = remove(store(&storage), "claude", DEFAULT_ID).unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(read(&storage).claude.items.is_empty());
    }

    #[test]
    fn remove_of_an_unknown_id_is_a_noop() {
        let dir = TempDir::new("accounts-remove-unknown");
        let storage = open(&dir);
        let accounts = add_ok(&storage, &dir, "claude", "Work", &dir.path().join("w"));
        let id = accounts.claude.items[0].id.clone();

        let removed = remove(store(&storage), "claude", "ghost").unwrap();

        assert_eq!(removed.claude.items.len(), 1);
        assert_eq!(removed.claude.items[0].id, id);
        assert_eq!(removed.claude.active, id);
    }

    #[test]
    fn set_active_rejects_an_unknown_id() {
        let dir = TempDir::new("accounts-active-unknown");
        let storage = open(&dir);
        let accounts = add_ok(&storage, &dir, "claude", "Work", &dir.path().join("w"));
        let id = accounts.claude.items[0].id.clone();

        let error = set_active(store(&storage), "claude", "ghost").unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert_eq!(read(&storage).claude.active, id);
    }

    #[test]
    fn set_active_switches_between_default_and_accounts() {
        let dir = TempDir::new("accounts-active");
        let storage = open(&dir);
        let claude = add_ok(&storage, &dir, "claude", "Work", &dir.path().join("cw"));
        let claude_id = claude.claude.items[0].id.clone();
        let codex = add_ok(&storage, &dir, "codex", "Codex", &dir.path().join("dx"));
        let codex_id = codex.codex.items[0].id.clone();

        let back = set_active(store(&storage), "claude", DEFAULT_ID).unwrap();
        assert_eq!(back.claude.active, DEFAULT_ID);
        // The other agent's active account is untouched.
        assert_eq!(back.codex.active, codex_id);

        let forth = set_active(store(&storage), "claude", &claude_id).unwrap();
        assert_eq!(forth.claude.active, claude_id);
        assert_eq!(forth.codex.active, codex_id);

        let other = set_active(store(&storage), "codex", DEFAULT_ID).unwrap();
        assert_eq!(other.codex.active, DEFAULT_ID);
        assert_eq!(other.claude.active, claude_id);
    }

    #[test]
    fn account_commands_reject_an_unknown_agent() {
        let dir = TempDir::new("accounts-agent-unknown");
        let storage = open(&dir);
        let folder = dir.path().join("x");

        for result in [
            add(
                store(&storage),
                "shell",
                "Work",
                folder.to_str().unwrap(),
                None,
                None,
            ),
            rename(store(&storage), "shell", "x", "Y"),
            remove(store(&storage), "shell", "x"),
            set_active(store(&storage), "shell", "x"),
        ] {
            assert_eq!(
                result.unwrap_err().code,
                Code::Daemon(ErrorCode::InvalidArgument)
            );
        }
        assert!(resolve("shell", &storage).is_err());
        assert!(read(&storage).claude.items.is_empty());
    }

    #[test]
    fn resolve_returns_the_active_account_with_its_folder() {
        let dir = TempDir::new("accounts-resolve");
        let storage = open(&dir);
        let one = add_ok(&storage, &dir, "claude", "One", &dir.path().join("one"));
        let one_id = one.claude.items[0].id.clone();
        let two = add_ok(&storage, &dir, "claude", "Two", &dir.path().join("two"));
        let two_id = two.claude.items[1].id.clone();

        let launch = resolve("claude", &storage).unwrap();
        assert_eq!(launch.id, two_id);
        assert_eq!(launch.dir, Some(dir.path().join("two")));

        set_active(store(&storage), "claude", &one_id).unwrap();
        let launch = resolve("claude", &storage).unwrap();
        assert_eq!(launch.id, one_id);
        assert_eq!(launch.dir, Some(dir.path().join("one")));

        // resolve does not check that the folder exists.
        fs::remove_dir_all(dir.path().join("one")).unwrap();
        let launch = resolve("claude", &storage).unwrap();
        assert_eq!(launch.dir, Some(dir.path().join("one")));

        // Codex is independent and still on Default.
        let launch = resolve("codex", &storage).unwrap();
        assert_eq!(launch.id, DEFAULT_ID);
        assert_eq!(launch.dir, None);
    }

    #[test]
    fn resolve_falls_back_to_default_for_a_stale_active_id() {
        let dir = TempDir::new("accounts-resolve-stale");
        let storage = open(&dir);
        store(&storage)
            .set_setting(
                ACCOUNTS_KEY,
                r#"{"claude":{"active":"ghost","items":[]},"codex":{"active":"default","items":[]}}"#,
            )
            .unwrap();

        let launch = resolve("claude", &storage).unwrap();
        assert_eq!(launch.id, DEFAULT_ID);
        assert_eq!(launch.dir, None);
    }

    #[test]
    fn resolve_falls_back_to_default_when_the_store_cannot_open() {
        let dir = TempDir::new("accounts-resolve-bad");
        fs::create_dir_all(dir.path().join("goko.db")).unwrap();
        let storage = open(&dir);
        assert!(storage.get().is_err());

        let launch = resolve("claude", &storage).unwrap();

        assert_eq!(launch.id, DEFAULT_ID);
        assert_eq!(launch.dir, None);
    }

    #[test]
    fn resolve_rejects_an_unknown_agent() {
        let dir = TempDir::new("accounts-resolve-agent");
        let storage = open(&dir);

        let error = resolve("shell", &storage).unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
    }

    #[test]
    fn the_view_lists_default_first_and_marks_the_active_account() {
        let dir = TempDir::new("accounts-view");
        let storage = open(&dir);
        let work = add_ok(&storage, &dir, "claude", "Work", &dir.path().join("cw"));
        let work_id = work.claude.items[0].id.clone();
        add_ok(&storage, &dir, "codex", "Codex", &dir.path().join("dx"));

        let view = view_with(&read(&storage), Some(dir.path()), None);

        assert_eq!(view.claude.items.len(), 2);
        assert_eq!(view.claude.items[0].id, DEFAULT_ID);
        assert_eq!(view.claude.items[0].name, "Default");
        assert!(view.claude.items[0].is_default);
        assert_eq!(view.claude.items[0].dir, dir.path().display().to_string());
        assert_eq!(view.claude.items[1].id, work_id);
        assert_eq!(view.claude.items[1].name, "Work");
        assert!(!view.claude.items[1].is_default);
        assert_eq!(view.claude.active, work_id);
        assert_eq!(view.codex.items.len(), 2);
        assert_eq!(view.codex.items[0].dir, "");
        assert_eq!(view.codex.items[1].name, "Codex");

        // The wire shape is exactly {claude: {active, items: [{id,name,dir,is_default}]}, …}.
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["claude"]["items"][0]["is_default"], true);
        assert_eq!(
            json["claude"]["items"][1]["dir"],
            serde_json::json!(work.claude.items[0].dir.display().to_string())
        );
        assert_eq!(json["codex"]["items"][0]["id"], "default");
    }

    #[test]
    fn link_shared_links_shared_entries_and_never_credentials() {
        let dir = TempDir::new("accounts-link-claude");
        let default = dir.path().join("default-home");
        let new = dir.path().join("new-home");
        fs::create_dir_all(default.join("projects")).unwrap();
        fs::write(default.join("settings.json"), "{}").unwrap();
        fs::create_dir_all(default.join("skills")).unwrap();
        fs::write(default.join("auth.json"), "{}").unwrap();
        fs::write(default.join(".credentials.json"), "{}").unwrap();
        fs::write(default.join(".claude.json"), "{}").unwrap();
        fs::create_dir_all(&new).unwrap();
        // An existing target is never overwritten — and not reported as linked.
        fs::write(new.join("settings.json"), "mine").unwrap();

        let linked = link_shared("claude", &default, &new);

        // Missing sources (CLAUDE.md, commands, agents, plugins, hooks) are skipped.
        assert_eq!(linked, ["projects", "skills"]);
        for entry in ["projects", "skills"] {
            let meta = fs::symlink_metadata(new.join(entry)).unwrap();
            assert!(meta.file_type().is_symlink(), "{entry} is not a symlink");
            assert_eq!(fs::read_link(new.join(entry)).unwrap(), default.join(entry));
        }
        assert_eq!(
            fs::read_to_string(new.join("settings.json")).unwrap(),
            "mine"
        );
        for absent in [
            "auth.json",
            ".credentials.json",
            ".claude.json",
            "CLAUDE.md",
            "commands",
            "agents",
            "plugins",
            "hooks",
        ] {
            assert!(
                fs::symlink_metadata(new.join(absent)).is_err(),
                "{absent} must not be linked"
            );
        }
    }

    #[test]
    fn link_shared_never_links_codex_sessions() {
        let dir = TempDir::new("accounts-link-codex");
        let default = dir.path().join("default-home");
        let new = dir.path().join("new-home");
        fs::create_dir_all(default.join("sessions/2026/11/08")).unwrap();
        fs::write(default.join("config.toml"), "model = \"x\"").unwrap();
        fs::write(default.join("AGENTS.md"), "agents").unwrap();
        fs::create_dir_all(default.join("skills")).unwrap();
        fs::create_dir_all(default.join("prompts")).unwrap();
        fs::write(default.join("auth.json"), "{}").unwrap();
        fs::create_dir_all(&new).unwrap();

        let linked = link_shared("codex", &default, &new);

        assert_eq!(linked, ["config.toml", "AGENTS.md", "skills", "prompts"]);
        assert!(fs::symlink_metadata(new.join("sessions")).is_err());
        assert!(fs::symlink_metadata(new.join("auth.json")).is_err());
        for entry in ["config.toml", "AGENTS.md", "skills", "prompts"] {
            assert!(
                fs::symlink_metadata(new.join(entry))
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "{entry} is not a symlink"
            );
        }
    }

    #[test]
    fn link_shared_skips_existing_targets_and_failed_links() {
        let dir = TempDir::new("accounts-link-fail");
        let default = dir.path().join("default-home");
        let new = dir.path().join("new-home");
        fs::create_dir_all(default.join("projects")).unwrap();
        fs::write(default.join("settings.json"), "{}").unwrap();
        fs::create_dir_all(default.join("skills")).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("settings.json"), "mine").unwrap();

        // A read-only folder makes every link fail; the call still completes
        // and the pre-existing entry is left alone.
        fs::set_permissions(&new, fs::Permissions::from_mode(0o500)).unwrap();
        let linked = link_shared("claude", &default, &new);
        assert!(linked.is_empty());
        assert!(fs::symlink_metadata(new.join("projects")).is_err());
        assert_eq!(
            fs::read_to_string(new.join("settings.json")).unwrap(),
            "mine"
        );

        // Once writable again, the failed entries link on a later call while
        // the existing one is still skipped.
        fs::set_permissions(&new, fs::Permissions::from_mode(0o700)).unwrap();
        let linked = link_shared("claude", &default, &new);
        assert_eq!(linked, ["projects", "skills"]);
        assert_eq!(
            fs::read_to_string(new.join("settings.json")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn sign_in_claude_quotes_a_folder_with_spaces() {
        // The space proves the env word stays one shell word.
        let s = sign_in_setup("claude", "a b");

        let launch = sign_in_launch(&s.accounts, "claude", &s.id, Some(&s.path)).unwrap();

        let claude = fs::canonicalize(s.root.path().join("bin").join("claude"))
            .unwrap()
            .display()
            .to_string();
        assert_eq!(
            shlex::split(&launch.command).unwrap(),
            vec![
                "env".to_owned(),
                format!("CLAUDE_CONFIG_DIR={}", s.folder.display()),
                claude,
                "auth".to_owned(),
                "login".to_owned(),
            ]
        );
    }

    #[test]
    fn sign_in_codex_quotes_its_home() {
        let s = sign_in_setup("codex", "cx");

        let launch = sign_in_launch(&s.accounts, "codex", &s.id, Some(&s.path)).unwrap();

        let codex = fs::canonicalize(s.root.path().join("bin").join("codex"))
            .unwrap()
            .display()
            .to_string();
        assert_eq!(
            shlex::split(&launch.command).unwrap(),
            vec![
                "env".to_owned(),
                format!("CODEX_HOME={}", s.folder.display()),
                codex,
                "login".to_owned(),
            ]
        );
    }

    #[test]
    fn sign_in_runs_in_the_account_folder_with_a_named_title() {
        let s = sign_in_setup("claude", "work");

        let launch = sign_in_launch(&s.accounts, "claude", &s.id, Some(&s.path)).unwrap();

        assert_eq!(launch.cwd, s.folder.display().to_string());
        assert_eq!(launch.title, "Sign in · Work");
    }

    #[test]
    fn sign_in_rejects_the_default_account() {
        let s = sign_in_setup("claude", "work");

        let error = sign_in_launch(&s.accounts, "claude", DEFAULT_ID, Some(&s.path)).unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(error.message.contains("terminal"));
    }

    #[test]
    fn sign_in_rejects_an_unknown_account_id() {
        let s = sign_in_setup("claude", "work");

        let error = sign_in_launch(&s.accounts, "claude", "ghost", Some(&s.path)).unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(error.message.contains("ghost"));
    }

    #[test]
    fn sign_in_rejects_an_unknown_agent() {
        let s = sign_in_setup("claude", "work");

        let error = sign_in_launch(&s.accounts, "shell", &s.id, Some(&s.path)).unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(error.message.contains("unknown agent"));
    }

    #[test]
    fn sign_in_rejects_a_missing_account_folder() {
        let s = sign_in_setup("claude", "work");
        fs::remove_dir_all(&s.folder).unwrap();

        let error = sign_in_launch(&s.accounts, "claude", &s.id, Some(&s.path)).unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(error.message.contains("no longer exists"));
        assert!(error.hint.unwrap().contains("Settings › Accounts"));
    }

    #[test]
    fn sign_in_rejects_a_binary_missing_from_path() {
        let s = sign_in_setup("claude", "work");
        // A PATH with codex but no claude isolates the binary lookup.
        let bin = s.root.path().join("bin-no-claude");
        fs::create_dir_all(&bin).unwrap();
        fake_bin(&bin, "codex");
        let path = path_var(&[bin.as_path()]);

        let error = sign_in_launch(&s.accounts, "claude", &s.id, Some(&path)).unwrap_err();

        assert_eq!(error.code, Code::Daemon(ErrorCode::InvalidArgument));
        assert!(error.message.contains("not found on PATH"));
        assert!(error.hint.unwrap().contains("install"));
    }
}
