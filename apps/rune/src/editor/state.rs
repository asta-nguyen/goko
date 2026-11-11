//! Editor workspace state: real projects/checkouts, per-section data,
//! open documents and the worker/ACP handles. Pure data plus the small
//! helpers every section shares; behaviour lives in the sibling modules.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use app_core::acp::AcpAgent;
use app_core::git_view::{self, GitViewBranch, GitViewChanges, GitViewCommit, GitViewStash};
use app_core::projects::ProjectView;
use app_core::worktrees::WorktreeList;
use rune::edit::TextEdit;
use rune_kit::diff::DiffState;
use rune_kit::{FileNode, Kit, TableState, TreeModel};

use crate::State;
use crate::editor::acp::Acp;
use crate::editor::backend::Backend;
use crate::editor::code::CodeState;
use crate::editor::draft::OpenDoc;
use crate::editor::indent::UnitChoice;
use crate::editor::lsp::Lsp;
use crate::editor::navigation::{self, Guard};
use crate::editor::{history_layout, quit_guard};

/// Identity of one checkout inside the editor: `project_id` + canonical path.
pub fn checkout_key(project_id: &str, checkout: &std::path::Path) -> String {
    format!("{project_id}\u{1f}{}", checkout.display())
}

/// The workspace's sections, in tab order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Section {
    Files,
    Changes,
    History,
    Stashes,
    Servers,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Files,
        Section::Changes,
        Section::History,
        Section::Stashes,
        Section::Servers,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Section::Files => "Files",
            Section::Changes => "Changes",
            Section::History => "History",
            Section::Stashes => "Stashes",
            Section::Servers => "Servers",
        }
    }
}

/// A one-line notice under the section bar: load failures keep cached data
/// visible (`stale`), hard failures show `error`.
#[derive(Clone, Debug)]
pub struct Notice {
    pub text: String,
    pub stale: bool,
}

/// How the Files tree treats a row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileMark {
    Tracked,
    /// Ignored by Git: openable, dimmed.
    Ignored,
    /// A wholly ignored directory, shown as a dimmed leaf folder.
    IgnoredDir,
}

pub struct FilesState {
    /// Every path `git ls-files` reported for the checkout (cached).
    pub paths: Vec<String>,
    /// What Git ignores (`dir/` for a collapsed directory), shown dimmed.
    pub ignored: Vec<String>,
    /// The filename filter text (what the input edits).
    pub filter: TextEdit,
    /// Content-search query input (debounced into `search.pending`).
    pub search_edit: TextEdit,
    /// The TreeModel the FileTree view renders; rebuilt from `paths`+`filter`.
    pub model: TreeModel<FileNode<FileMark>>,
    pub model_checkout: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    /// Content-search state (debounced query → worker).
    pub search: search_state::Search,
    /// Ignored directories the user opened: the entries one level below
    /// each, keyed by the directory's path. Cleared with the checkout.
    pub ignored_open: std::collections::BTreeMap<String, Vec<String>>,
    /// The entry awaiting a Move-to-Trash confirmation.
    pub delete_prompt: Option<String>,
    /// The row context menu: the entry it was opened on and where.
    pub menu: Option<FileMenu>,
}

/// A tree row's context menu, opened by a secondary press at (`x`, `y`).
pub struct FileMenu {
    pub id: String,
    pub x: f32,
    pub y: f32,
}

/// Search bookkeeping kept beside the files section: the debounced query and
/// the last result set, both scoped to the checkout they were issued for.
pub mod search_state {
    use app_core::git_view::GitViewSearch;
    #[derive(Default)]
    pub struct Search {
        /// Last query actually issued to the worker (staleness check).
        pub query: String,
        /// Debounce bookkeeping: (query, due-seconds on the poll clock).
        pub pending: Option<(String, f64)>,
        pub results: Option<GitViewSearch>,
        pub checkout: Option<String>,
        pub searching: bool,
        pub error: Option<String>,
        pub open: bool,
    }
}

impl ChangesState {
    /// Empty per-checkout state, used on checkout switches.
    pub fn empty() -> ChangesState {
        ChangesState {
            data: None,
            checkout: None,
            loading: false,
            error: None,
            selected: None,
            layer: None,
            patch_path: None,
            patch: None,
            diff: DiffState::new(Vec::new()),
            patch_loading: false,
            filter: TextEdit::new(""),
            collapsed: HashSet::new(),
            discard_prompt: None,
            busy: None,
        }
    }
}

pub struct ChangesState {
    pub data: Option<GitViewChanges>,
    pub checkout: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    /// Selected status path for the patch pane (`None` = repo overview layers).
    pub selected: Option<String>,
    /// Diff layer currently shown in the patch pane: `working`, `staged`,
    /// `unstaged`, `commit:<id>`, `stash@{n}`… — mirrors the desktop `layer`.
    pub layer: Option<String>,
    pub patch_path: Option<String>,
    /// Loaded patch for `layer`/`patch_path`.
    pub patch: Option<Patch>,
    /// rune-kit diff model backing the patch pane's `DiffView`; refreshed
    /// with `set_files(parse(&patch.raw))` when a matching diff reply lands.
    pub diff: DiffState,
    pub patch_loading: bool,
    /// Filename filter TextEdit for the changes list.
    pub filter: TextEdit,
    /// Collapsed group keys in the changes list.
    pub collapsed: HashSet<String>,
    /// Discard confirmation target (layer + path).
    pub discard_prompt: Option<(String, String)>,
    /// Commit message draft TextEdit lives in CommitState.
    pub busy: Option<&'static str>,
}

/// A fetched unified diff plus its parsed rune-kit model inputs.
#[derive(Clone)]
pub struct Patch {
    pub label: String,
    pub raw: String,
    pub truncated: bool,
    pub binary: bool,
    pub files: Vec<app_core::git_view::StatusEntry>,
    pub detail: Option<git_view::GitViewCommitDetail>,
    /// Checkout key the patch was fetched for — `patch::patch_fresh` hides
    /// the pane when it no longer matches the live checkout.
    pub checkout: String,
}

pub struct HistoryState {
    pub commits: Vec<GitViewCommit>,
    pub next_skip: Option<u64>,
    pub checkout: Option<String>,
    pub branch: Option<String>,
    /// Branch filter options loaded for the picker.
    pub branches: Vec<GitViewBranch>,
    /// Open/search state of the branch `Combobox`.
    pub branch_picker: rune_kit::ComboboxState,
    pub loading: bool,
    pub loading_more: bool,
    pub error: Option<String>,
    /// Selected commit id (detail pane).
    pub selected: Option<String>,
    /// Detail patch for the selected commit.
    pub detail: Option<Patch>,
    pub detail_loading: bool,
    /// Search text filtering commit subjects (desktop match dimming/jump).
    pub filter: TextEdit,
    /// Persisted layout (columns px + detail height), loaded from settings.
    pub layout: history_layout::HistoryLayout,
    pub layout_dirty: bool,
    /// A `SetSetting` write for `layout` is on the worker lane; the reply
    /// (ok or err) releases it so a refused queue can retry bounded.
    pub layout_save_pending: bool,
    /// Jump target while a filter match is active (history view seam).
    pub jump_to: Option<String>,
    /// rune-kit diff model for the commit-detail patch pane.
    pub diff: DiffState,
    /// Table row/keyboard/column-width state for the history `Table`.
    pub table: TableState,
    /// History root height measured by `on_layout_tx`; feeds the detail
    /// pane's render clamp. Kept per-state so parallel `Runtime` instances
    /// (tests, multi-window) never share a stale measurement.
    pub measured_root_height: f32,
    /// History root width measured by `on_layout_tx`; feeds the Right
    /// position's detail-width render clamp.
    pub measured_root_width: f32,
    /// Detail pane height measured by `on_layout_tx`; feeds the metadata
    /// scroll cap in `commit_detail`.
    pub measured_detail_height: f32,
    /// Open commit-row context menu: target commit id plus the secondary
    /// press point in window coordinates. `history::view` renders it with
    /// the kit `context_menu`; `on_close`/item picks and `history::poll`
    /// clear it.
    pub row_menu: Option<HistoryRowMenu>,
    /// Commit-detail copy feedback — `(commit id, copied-at)`; the header
    /// shows "Copied" for `commit_detail::COPY_FEEDBACK_MS` like desktop's
    /// `copyNote` timer, and `history::poll` expires it.
    pub copied_hash: Option<(String, std::time::Instant)>,
}

/// A secondary press on a commit row awaiting its menu choice.
#[derive(Clone, Debug)]
pub struct HistoryRowMenu {
    pub commit: String,
    pub x: f32,
    pub y: f32,
}

pub struct StashesState {
    pub stashes: Vec<GitViewStash>,
    pub checkout: Option<String>,
    /// `true` when the stash list is repository-wide (shared across worktrees).
    pub shared_repo: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub filter: TextEdit,
    /// Selected stash id (`stash@{n}` / oid).
    pub selected: Option<String>,
    pub detail: Option<Patch>,
    /// rune-kit diff model for the stash-detail patch pane.
    pub diff: DiffState,
    pub detail_loading: bool,
}

pub struct CommitState {
    /// Commit message draft (TextEdit so undo/IME work like the desktop textarea).
    pub message: TextEdit,
    /// Generation in flight for this checkout.
    pub generating: bool,
    pub generate_error: Option<String>,
    /// The agent picked for Generate (empty = first configured agent).
    pub agent_id: Option<String>,
    /// Controlled open state for the agent `Select` (same pattern as the
    /// project/checkout pickers).
    pub agent_picker_open: bool,
    /// Commit/push busy labels.
    pub committing: bool,
    pub pushing: bool,
    pub error: Option<String>,
    /// Guards against a stale ACP completion overwriting an edited draft:
    /// the draft's text snapshot when Generate was issued.
    pub gen_snapshot: Option<String>,
    pub gen_checkout: Option<String>,
}

/// A go-to-definition request in flight. `Import` first tries the checkout
/// file set, then ONE config lookup reads the ordered `tsconfig`/`jsconfig`
/// candidates for `paths` aliases before giving up; `Name` issues a content
/// search and picks the single declaration (or falls back to showing the
/// search pane).
pub struct GoToPending {
    /// Checkout-relative path of the document the request came from.
    pub from_path: String,
    pub stage: GoToStage,
}

#[derive(Clone, Debug)]
pub enum GoToStage {
    /// Waiting on config-file reads before the alias-aware resolve retry.
    AwaitConfigs { specifier: String },
    /// Content search issued for a plain identifier.
    SearchName { name: String },
}

/// One project's sidebar worktree state: the last `Req::Worktrees` answer
/// plus its in-flight/error flags. Kept on `EditorState` (keyed by project
/// id, never by position) so a late reply can only ever land on the project
/// it was asked for.
#[derive(Default)]
pub struct SidebarWorktrees {
    /// Last successful list; `None` until the first reply lands.
    pub list: Option<WorktreeList>,
    pub loading: bool,
    pub error: Option<String>,
}

pub struct EditorState {
    /// The shared rune-kit theme (dark palette matches `Ink`).
    pub kit: Kit,

    // -- project / checkout selection -------------------------------------
    pub projects: Vec<ProjectView>,
    pub projects_loading: bool,
    pub projects_error: Option<String>,
    pub project_id: Option<String>,
    /// Per-project worktree lists for the sidebar — every registered
    /// project gets its own entry so replies can't cross projects.
    pub sidebar_worktrees: HashMap<String, SidebarWorktrees>,
    pub worktrees: WorktreeList,
    pub worktrees_loading: bool,
    pub worktrees_error: Option<String>,
    /// The "New worktree" dialog (`worktree_form.rs`): `Some` while open.
    pub worktree_form: Option<crate::editor::worktree_form::Form>,
    /// Env-copy reports shown under each project's sidebar worktree group
    /// after a create resolves (partial success included), keyed by
    /// project id — the desktop's per-row `envResult` Alert.
    pub worktree_env: HashMap<String, crate::editor::worktree_form::EnvNotice>,
    /// Canonical validated checkout path backing every section.
    pub checkout: Option<PathBuf>,
    pub checkout_key: Option<String>,
    /// Checkout switches pending a draft-guard decision.
    pub pending_checkout: Option<(String, PathBuf)>,

    // -- sections ----------------------------------------------------------
    pub section: Section,
    /// Sections ever activated; their state (scroll, selection, caches) stays
    /// mounted so switching back is instant, like the desktop's visited set.
    pub visited: HashSet<Section>,
    pub notice: Option<Notice>,
    pub files: FilesState,
    pub changes: ChangesState,
    pub history: HistoryState,
    pub stashes: StashesState,
    pub commit: CommitState,
    /// Width (px) of the currently mounted horizontal list/detail split
    /// row, measured by `pane_layout::watch_row_width`. Only one section's
    /// split mounts at a time, so a single value suffices; 0 until the
    /// first layout pass (treated as "unmeasured" by the drag clamp).
    pub pane_row_w: f32,

    // -- documents ---------------------------------------------------------
    /// The single open document. The desktop editor keeps one active file
    /// draft; opening another file goes through the same Save/Discard/Cancel
    /// guard as every other navigation, then replaces this slot.
    pub doc: Option<OpenDoc>,
    /// What a media preview needs (image cache, app waker), set in `setup`.
    pub media_host: Option<crate::editor::media::Host>,
    /// Scratch `CodeState` for the `Code` view's accessors when no document
    /// is open (the view is only mounted with a doc, so this is a fallback).
    pub scratch_code: CodeState,
    /// Persisted indent-unit override (`auto` resolves per document); every
    /// mounted [`CodeState`] mirrors it via [`Self::set_indent_choice`].
    pub indent_choice: UnitChoice,
    /// Whether the inline-blame column is shown. Persisted under
    /// `goko.editor.blame`; the column itself additionally hides while the
    /// draft is dirty — the annotations describe the file on disk.
    pub blame_enabled: bool,
    /// Go-to-definition resolution in flight (one at a time).
    pub go_to: Option<GoToPending>,
    /// Token guarding an in-flight file load against stale replies.
    pub doc_load: Option<u64>,
    /// Token of the in-flight "reload from disk" read (`Req::ReloadFile`);
    /// its reply replaces the doc only while this still matches.
    pub doc_reload: Option<u64>,
    /// Save that must resume a pending navigation when it lands.
    pub save_resume: Option<Guard>,

    // -- coordination ------------------------------------------------------
    pub backend: Backend,
    pub acp: Acp,
    /// Language-server subsystem: runtime thread + hub + document sync.
    pub lsp: Lsp,
    /// Dirty-draft flag mirrored for the native quit hook (see quit_guard).
    pub quit_dirty: Arc<AtomicBool>,
    /// Set when the OS-level quit was vetoed; the guard shows Quit semantics.
    pub quit_vetoed: bool,
    /// Navigation guard awaiting Save/Discard/Cancel.
    pub guard: Option<Guard>,
    /// App handle for reopen-after-cancelled-close and worker wakeups.
    pub app: Option<rune::AppHandle<State>>,
    /// Set in `App::on_close` when a dirty draft cancelled the close; the
    /// window hides and `poll` reopens it once the guard resolves.
    pub reopen_after_close: bool,
    /// ACP agents discovered for the Generate picker.
    pub acp_agents: Vec<AcpAgent>,
    /// Monotonic token for file-load requests (stale replies drop).
    pub doc_seq: u64,
    /// Navigation a successful save should resume, applied by `poll`.
    pub pending_apply: Option<navigation::PendingNav>,
    /// Latest issued read generation per request kind (`changes`, `diff`,
    /// `history`, `branches`, `filelist`, `search`, `file`, `stashes`,
    /// `stashdiff`). Replies carrying an older generation for the same
    /// checkout are dropped even when the checkout key still matches.
    pub req_gens: HashMap<&'static str, u64>,
    /// Last modifier set reported by `App::on_modifiers`; the code owner's
    /// `CodeState` reads this for glyph-hit link overlays.
    pub modifiers: rune::Mods,
    /// rune-kit `Select` is fully controlled: these flags are its `open`
    /// state, toggled by `on_toggle`/`on_close`. At most one picker is open.
    pub project_picker_open: bool,
    pub checkout_picker_open: bool,
    /// Global keyboard state: the highlight-only j/k list cursor plus the
    /// section/digit bookkeeping (`keyboard.rs`).
    pub keyboard: crate::editor::keyboard::KeyboardState,
    /// Cmd/Ctrl-P quick file picker (`quick_open.rs`).
    pub quick_open: crate::editor::quick_open::QuickOpenState,
}

impl EditorState {
    pub fn new(kit: Kit, app: rune::AppHandle<State>) -> EditorState {
        let backend = Backend::new(app.clone());
        let acp = Acp::new(app.clone());
        let quit_dirty = Arc::new(AtomicBool::new(false));
        quit_guard::install(quit_dirty.clone(), app.clone());
        EditorState {
            kit,
            projects: Vec::new(),
            projects_loading: false,
            projects_error: None,
            project_id: None,
            sidebar_worktrees: HashMap::new(),
            worktrees: WorktreeList {
                repo: false,
                worktrees: Vec::new(),
            },
            worktrees_loading: false,
            worktrees_error: None,
            worktree_form: None,
            worktree_env: HashMap::new(),
            checkout: None,
            checkout_key: None,
            pending_checkout: None,
            section: Section::Changes,
            visited: HashSet::from([Section::Changes]),
            notice: None,
            files: FilesState {
                paths: Vec::new(),
                ignored: Vec::new(),
                filter: TextEdit::new(""),
                search_edit: TextEdit::new(""),
                model: TreeModel::new(),
                model_checkout: None,
                loading: false,
                error: None,
                search: search_state::Search::default(),
                ignored_open: Default::default(),
                delete_prompt: None,
                menu: None,
            },
            changes: ChangesState {
                data: None,
                checkout: None,
                loading: false,
                error: None,
                selected: None,
                layer: None,
                patch_path: None,
                patch: None,
                diff: DiffState::new(Vec::new()),
                patch_loading: false,
                filter: TextEdit::new(""),
                collapsed: HashSet::new(),
                discard_prompt: None,
                busy: None,
            },
            history: HistoryState {
                commits: Vec::new(),
                next_skip: None,
                checkout: None,
                branch: None,
                branches: Vec::new(),
                branch_picker: rune_kit::ComboboxState::default(),
                loading: false,
                loading_more: false,
                error: None,
                selected: None,
                detail: None,
                detail_loading: false,
                filter: TextEdit::new(""),
                layout: history_layout::HistoryLayout::default(),
                layout_dirty: false,
                layout_save_pending: false,
                jump_to: None,
                diff: DiffState::new(Vec::new()),
                table: TableState::new(),
                measured_root_height: 0.0,
                measured_root_width: 0.0,
                measured_detail_height: 0.0,
                row_menu: None,
                copied_hash: None,
            },
            stashes: StashesState {
                stashes: Vec::new(),
                checkout: None,
                shared_repo: false,
                loading: false,
                error: None,
                filter: TextEdit::new(""),
                selected: None,
                detail: None,
                diff: DiffState::new(Vec::new()),
                detail_loading: false,
            },
            pane_row_w: 0.0,
            commit: CommitState {
                message: TextEdit::new(""),
                generating: false,
                generate_error: None,
                agent_id: None,
                agent_picker_open: false,
                committing: false,
                pushing: false,
                error: None,
                gen_snapshot: None,
                gen_checkout: None,
            },
            doc: None,
            media_host: None,
            scratch_code: CodeState::new("", ""),
            indent_choice: UnitChoice::Auto,
            blame_enabled: true,
            go_to: None,
            doc_load: None,
            doc_reload: None,
            save_resume: None,
            backend,
            acp,
            lsp: Lsp::new(),
            quit_dirty,
            quit_vetoed: false,
            guard: None,
            app: Some(app),
            reopen_after_close: false,
            acp_agents: Vec::new(),
            doc_seq: 0,
            pending_apply: None,
            req_gens: HashMap::new(),
            modifiers: rune::Mods::default(),
            project_picker_open: false,
            checkout_picker_open: false,
            keyboard: Default::default(),
            quick_open: Default::default(),
        }
    }

    /// `true` while the open document has unsaved edits.
    pub fn has_dirty_doc(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.dirty())
    }

    /// The open document, if any. Kept as an accessor so callers don't
    /// depend on the slot's representation.
    pub fn active_doc_mut(&mut self) -> Option<&mut OpenDoc> {
        self.doc.as_mut()
    }
    pub fn active_doc(&self) -> Option<&OpenDoc> {
        self.doc.as_ref()
    }

    /// Refresh the OS-quit dirty flag after any doc mutation.
    pub fn sync_quit_dirty(&self) {
        self.quit_dirty
            .store(self.has_dirty_doc(), std::sync::atomic::Ordering::Release);
    }

    /// `App::on_modifiers` entry point: keep the raw snapshot and mirror the
    /// platform go-to modifier (⌘/Ctrl) onto the active document's code
    /// surface so its glyph-hit link overlay mounts/unmounts.
    pub fn set_modifiers(&mut self, mods: rune::Mods) {
        self.modifiers = mods;
        if let Some(doc) = self.doc.as_mut() {
            doc.code.go_to_mod = mods.cmd;
        }
    }
}
