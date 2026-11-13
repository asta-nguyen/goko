//! Files search: a 250 ms debounce on the query input, generation-tagged
//! `Req::Search` reads through the shared backend for Content mode, and the
//! Concept flow — an indexer thread that runs full passes and working-tree
//! syncs, and a query that waits for the sync (at most `SYNC_WAIT`) before it
//! is issued, so a file added a moment ago is found by the next query.
//!
//! `on_query_change` runs inside the TextInput `on_change` handler (has
//! `&mut State` + `&mut Tx`); `poll` runs from `AppSpec::poll` and issues
//! the pending query once the debounce wake arrives.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::{AppHandle, Tx};

use crate::State;
use crate::editor::backend::Req;
use crate::editor::state::search_state::{IndexStatus, PassRecord, Search, SearchMode, SyncState};

/// Minimum query length before a search is issued (matches the desktop).
pub const MIN_QUERY: usize = 2;
/// How long a Concept query waits for its working-tree sync.
pub const SYNC_WAIT: f64 = 1.0;
/// How often a full reconciliation pass is retried before a query.
pub const REFRESH_INTERVAL: f64 = 10.0;

/// One event from the indexer thread to the pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexEvent {
    Progress {
        checkout_key: String,
        done: usize,
        total: usize,
    },
    Done {
        checkout_key: String,
        truncated: bool,
    },
    Failed {
        checkout_key: String,
        message: String,
    },
    Synced {
        checkout_key: String,
    },
}

enum IndexJob {
    Pass { checkout: PathBuf, key: String },
    Sync { checkout: PathBuf, key: String },
}

/// The one indexer thread: full passes and working-tree syncs, served first
/// come first served.
pub struct Indexer {
    tx: Option<Sender<IndexJob>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    /// Sync keys queued and not started; a running sync is removed from it.
    syncs: Arc<Mutex<HashSet<String>>>,
    /// Pass keys queued or running; removed when the pass ends.
    passes: Arc<Mutex<HashSet<String>>>,
    /// Test seam: an idle indexer records requests instead of running them.
    #[cfg(test)]
    requested: Arc<Mutex<Vec<IndexJob>>>,
}

impl Indexer {
    /// Production constructor: a thread over `GOKO_HOME`, or an idle indexer
    /// when the home cannot be resolved.
    pub fn new(handle: AppHandle<State>) -> Self {
        #[cfg(test)]
        {
            let _ = handle;
            Self::idle()
        }
        #[cfg(not(test))]
        {
            match app_core::home::home_dir() {
                Ok(home) => Self::with_home(handle, home),
                Err(_) => Self::idle(),
            }
        }
    }

    fn idle() -> Self {
        Self {
            tx: None,
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
            syncs: Arc::new(Mutex::new(HashSet::new())),
            passes: Arc::new(Mutex::new(HashSet::new())),
            #[cfg(test)]
            requested: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// A real thread over an explicit home (tests and the app alike).
    pub fn with_home(handle: AppHandle<State>, home: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let syncs = Arc::new(Mutex::new(HashSet::new()));
        let passes = Arc::new(Mutex::new(HashSet::new()));
        let worker = {
            let stop = stop.clone();
            let syncs = syncs.clone();
            let passes = passes.clone();
            std::thread::Builder::new()
                .name("goko-concept-index".into())
                .spawn(move || indexer_thread(rx, handle, home, stop, syncs, passes))
                .expect("spawn concept indexer")
        };
        Self {
            tx: Some(tx),
            stop,
            worker: Some(worker),
            syncs,
            passes,
            #[cfg(test)]
            requested: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Queue a pass for the checkout unless one is already queued or running.
    pub fn request_pass(&mut self, checkout: PathBuf, key: String) -> bool {
        let mut passes = self.passes.lock().unwrap();
        if !passes.insert(key.clone()) {
            return false;
        }
        if let Some(tx) = &self.tx {
            if tx
                .send(IndexJob::Pass {
                    checkout,
                    key: key.clone(),
                })
                .is_err()
            {
                passes.remove(&key);
                return false;
            }
            return true;
        }
        #[cfg(test)]
        {
            self.requested
                .lock()
                .unwrap()
                .push(IndexJob::Pass { checkout, key });
            true
        }
        #[cfg(not(test))]
        {
            passes.remove(&key);
            false
        }
    }

    /// Queue a sync unless one is already queued and not started.
    pub fn request_sync(&mut self, checkout: PathBuf, key: String) -> bool {
        let mut syncs = self.syncs.lock().unwrap();
        if !syncs.insert(key.clone()) {
            return false;
        }
        if let Some(tx) = &self.tx {
            if tx
                .send(IndexJob::Sync {
                    checkout,
                    key: key.clone(),
                })
                .is_err()
            {
                syncs.remove(&key);
                return false;
            }
            return true;
        }
        #[cfg(test)]
        {
            self.requested
                .lock()
                .unwrap()
                .push(IndexJob::Sync { checkout, key });
            true
        }
        #[cfg(not(test))]
        {
            syncs.remove(&key);
            false
        }
    }

    /// Stop at the next file boundary; the thread is not joined (the process
    /// is exiting).
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.tx = None;
        self.worker = None;
    }
}

fn indexer_thread(
    rx: Receiver<IndexJob>,
    handle: AppHandle<State>,
    home: PathBuf,
    stop: Arc<AtomicBool>,
    syncs: Arc<Mutex<HashSet<String>>>,
    passes: Arc<Mutex<HashSet<String>>>,
) {
    app_core::code_index::remove_orphans(&home);
    while let Ok(job) = rx.recv() {
        match job {
            IndexJob::Pass { checkout, key } => {
                let mut sink = event_sink(handle.clone());
                run_pass(&home, &checkout, &key, &stop, &mut sink);
                passes.lock().unwrap().remove(&key);
            }
            IndexJob::Sync { checkout, key } => {
                syncs.lock().unwrap().remove(&key);
                let mut sink = event_sink(handle.clone());
                let queued = syncs.clone();
                let queued_key = key.clone();
                let mut wrapped = |event: IndexEvent| {
                    // A newer sync queued behind this one will report the
                    // result; an intermediate Synced must not release a query.
                    if matches!(event, IndexEvent::Synced { .. })
                        && queued.lock().unwrap().contains(&queued_key)
                    {
                        return;
                    }
                    sink(event);
                };
                run_sync(&home, &checkout, &key, &stop, &mut wrapped);
            }
        }
    }
}

fn event_sink(handle: AppHandle<State>) -> impl FnMut(IndexEvent) {
    move |event| {
        let handle = handle.clone();
        handle.update(move |state: &mut State, tx: &mut Tx| {
            apply_index_event(state, tx, event);
        });
    }
}

/// Run one full pass and report its progress and outcome.
pub fn run_pass(
    home: &Path,
    checkout: &Path,
    key: &str,
    stop: &AtomicBool,
    sink: &mut dyn FnMut(IndexEvent),
) {
    let result = app_core::code_index::refresh(home, checkout, |done, total| {
        if done.is_multiple_of(100) {
            sink(IndexEvent::Progress {
                checkout_key: key.to_string(),
                done,
                total,
            });
        }
        !stop.load(Ordering::SeqCst)
    });
    if stop.load(Ordering::SeqCst) {
        return;
    }
    match result {
        Ok(stats) => sink(IndexEvent::Done {
            checkout_key: key.to_string(),
            truncated: stats.truncated,
        }),
        Err(error) => sink(IndexEvent::Failed {
            checkout_key: key.to_string(),
            message: error.to_string(),
        }),
    }
}

/// Run one working-tree sync; always ends with `Synced`, so a waiting query is
/// never stranded.
pub fn run_sync(
    home: &Path,
    checkout: &Path,
    key: &str,
    stop: &AtomicBool,
    sink: &mut dyn FnMut(IndexEvent),
) {
    let _ = app_core::code_index::sync_changed(home, checkout, stop);
    sink(IndexEvent::Synced {
        checkout_key: key.to_string(),
    });
}

/// Apply one indexer event to the pane state.
pub fn apply_index_event(state: &mut State, tx: &mut Tx, event: IndexEvent) {
    let clock = state.clock;
    let input = state.editor.files.search_edit.text.trim().to_string();
    let live = state.editor.checkout_key.clone();
    match event {
        IndexEvent::Done {
            checkout_key,
            truncated,
        } => {
            state.editor.files.search.last_pass.insert(
                checkout_key.clone(),
                PassRecord {
                    checkout_key: checkout_key.clone(),
                    finished: clock,
                    ok: true,
                },
            );
            if live.as_deref() != Some(checkout_key.as_str()) {
                return;
            }
            let search = &mut state.editor.files.search;
            search.index = if truncated {
                IndexStatus::Truncated
            } else {
                IndexStatus::Idle
            };
            if search.mode == SearchMode::Concept
                && search.open
                && !search.query.is_empty()
                && search.sync == SyncState::Idle
                && input == search.query
            {
                search.pending = Some((search.query.clone(), 0.0));
                tx.wake_after(Duration::from_millis(0));
            }
        }
        IndexEvent::Failed {
            checkout_key,
            message,
        } => {
            state.editor.files.search.last_pass.insert(
                checkout_key.clone(),
                PassRecord {
                    checkout_key: checkout_key.clone(),
                    finished: clock,
                    ok: false,
                },
            );
            if live.as_deref() != Some(checkout_key.as_str()) {
                return;
            }
            state.editor.files.search.index = IndexStatus::Failed(message);
        }
        IndexEvent::Progress {
            checkout_key,
            done,
            total,
        } => {
            if live.as_deref() != Some(checkout_key.as_str()) {
                return;
            }
            state.editor.files.search.index = IndexStatus::Building { done, total };
        }
        IndexEvent::Synced { checkout_key } => {
            if live.as_deref() != Some(checkout_key.as_str()) {
                return;
            }
            let sync = std::mem::take(&mut state.editor.files.search.sync);
            let query = match sync {
                SyncState::Waiting { query, .. } | SyncState::Expired { query } => query,
                SyncState::Idle => return,
            };
            if state.editor.files.search.mode == SearchMode::Concept && input == query {
                issue_concept(state, query);
            }
        }
    }
}

fn issue_concept(state: &mut State, query: String) {
    let Some(checkout) = state.editor.checkout.clone() else {
        return;
    };
    let Some(key) = state.editor.checkout_key.clone() else {
        return;
    };
    state.editor.files.search.query = query.clone();
    state.editor.files.search.error = None;
    state
        .editor
        .enqueue_read(checkout, key, Req::ConceptSearch { query });
}

fn clear_search(search: &mut Search) {
    search.pending = None;
    search.results = None;
    search.query.clear();
    search.searching = false;
    search.error = None;
    search.sync = SyncState::Idle;
}

/// TextInput `on_change`: record the latest query and arm the debounce.
pub fn on_query_change(s: &mut State, tx: &mut Tx) {
    let query = s.editor.files.search_edit.text.trim().to_string();
    s.editor.files.search.sync = SyncState::Idle;
    if query.chars().count() < MIN_QUERY {
        s.editor.files.search.pending = None;
        s.editor.files.search.results = None;
        s.editor.files.search.error = None;
        // A cleared or one-character box has nothing to show: the tree returns.
        s.editor.files.search.open = false;
        if s.editor.files.search.mode == SearchMode::Concept {
            s.editor.files.search.searching = false;
        }
        return;
    }
    s.editor.files.search.pending = Some((query, 0.0));
    tx.wake_after(Duration::from_millis(250));
}

/// Switch the pane's mode, clearing the previous mode's state and issuing the
/// existing input again for the new mode.
pub fn set_mode(state: &mut State, tx: &mut Tx, mode: SearchMode) {
    if state.editor.files.search.mode == mode {
        return;
    }
    state.editor.files.search.mode = mode;
    clear_search(&mut state.editor.files.search);
    let input = state.editor.files.search_edit.text.trim().to_string();
    if input.chars().count() >= MIN_QUERY {
        state.editor.files.search.pending = Some((input, 0.0));
        tx.wake_after(Duration::from_millis(250));
    }
    if mode == SearchMode::Concept {
        let now = state.clock;
        ensure_index(state, now);
    }
}

/// Put a Concept pane back in Content mode; Content panes are left alone.
pub fn force_content(search: &mut Search) {
    if search.mode != SearchMode::Concept {
        return;
    }
    search.mode = SearchMode::Content;
    clear_search(search);
}

/// Request a full pass when the checkout has none or its last one is stale.
pub fn ensure_index(state: &mut State, now: f64) -> bool {
    let Some(checkout) = state.editor.checkout.clone() else {
        return false;
    };
    let Some(key) = state.editor.checkout_key.clone() else {
        return false;
    };
    let due = state
        .editor
        .files
        .search
        .last_pass
        .get(&key)
        .is_none_or(|record| now - record.finished > REFRESH_INTERVAL);
    if !due || !state.editor.indexer.request_pass(checkout, key) {
        return false;
    }
    state.editor.files.search.index = IndexStatus::Building { done: 0, total: 0 };
    true
}

/// `AppSpec::poll` hook: expire a waiting sync, then issue the debounced query.
pub fn poll(state: &mut State, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    let expired = match &state.editor.files.search.sync {
        SyncState::Waiting { query, deadline } if now >= *deadline => Some(query.clone()),
        _ => None,
    };
    if let Some(query) = expired {
        state.editor.files.search.sync = SyncState::Expired {
            query: query.clone(),
        };
        state.editor.files.search.searching = true;
        state.editor.files.search.error = None;
        issue_concept(state, query);
        changed = true;
    }
    let Some((query, _due)) = state.editor.files.search.pending.take() else {
        return changed;
    };
    let Some(checkout) = state.editor.checkout.clone() else {
        return changed;
    };
    let Some(key) = state.editor.checkout_key.clone() else {
        return changed;
    };
    if state.editor.files.search_edit.text.trim() != query {
        state.editor.files.search.pending =
            Some((state.editor.files.search_edit.text.trim().to_string(), 0.0));
        return true;
    }
    // Issuing a query makes the aside show its results (the filter hides them).
    state.editor.files.search.open = true;
    if state.editor.files.search.mode == SearchMode::Concept {
        let has_pass = state
            .editor
            .files
            .search
            .last_pass
            .get(&key)
            .is_some_and(|record| record.ok);
        if has_pass {
            let _ = state.editor.indexer.request_sync(checkout, key.clone());
            let _ = ensure_index(state, now);
            let search = &mut state.editor.files.search;
            search.query = query.clone();
            search.searching = true;
            search.results = None;
            search.error = None;
            search.sync = SyncState::Waiting {
                query,
                deadline: now + SYNC_WAIT,
            };
            tx.wake_after(Duration::from_secs_f64(SYNC_WAIT));
            return true;
        }
        let _ = ensure_index(state, now);
        let search = &mut state.editor.files.search;
        search.query = query.clone();
        search.searching = true;
        search.results = None;
        search.error = None;
        state
            .editor
            .enqueue_read(checkout, key, Req::ConceptSearch { query });
        return true;
    }
    let search = &mut state.editor.files.search;
    search.query = query.clone();
    search.searching = true;
    search.error = None;
    search.results = None;
    state
        .editor
        .enqueue_read(checkout, key, Req::Search { query });
    true
}

/// Clear search state on checkout switch and show the tree again. The chosen
/// mode is kept; a box in Concept mode starts a pass for the new checkout when
/// none has finished for it.
pub fn reset(s: &mut State) {
    clear_search(&mut s.editor.files.search);
    s.editor.files.search.open = false;
    s.editor.files.search.checkout = s.editor.checkout_key.clone();
    s.editor.files.search.index = IndexStatus::Idle;
    if s.editor.files.search.mode == SearchMode::Concept {
        let now = s.clock;
        ensure_index(s, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::state::checkout_key;
    use app_core::test_support::TempDir;
    use std::fs;
    use std::process::Command;

    fn state_with_checkout() -> State {
        let mut s = State::new(crate::test_handle());
        s.editor.project_id = Some("p1".into());
        s.editor.checkout = Some(PathBuf::from("/tmp/co"));
        s.editor.checkout_key = Some(checkout_key("p1", &PathBuf::from("/tmp/co")));
        s
    }

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

    fn repository(label: &str, files: &[(&str, &str)]) -> (TempDir, PathBuf) {
        let dir = TempDir::new(label);
        let root = dir.path().canonicalize().unwrap();
        git(&root, &["init", "-b", "main"]);
        git(&root, &["config", "user.name", "Tester"]);
        git(&root, &["config", "user.email", "tester@example.com"]);
        for (name, text) in files {
            fs::write(root.join(name), text).unwrap();
        }
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "init"]);
        (dir, root)
    }

    fn requests(s: &State) -> Vec<String> {
        s.editor
            .indexer
            .requested
            .lock()
            .unwrap()
            .iter()
            .map(|job| match job {
                IndexJob::Pass { .. } => "pass".to_string(),
                IndexJob::Sync { .. } => "sync".to_string(),
            })
            .collect()
    }

    #[test]
    fn debounce_keeps_only_latest_query() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        s.editor.files.search_edit.text = "fir".into();
        on_query_change(&mut s, &mut tx);
        s.editor.files.search_edit.text = "first".into();
        on_query_change(&mut s, &mut tx);
        assert_eq!(
            s.editor
                .files
                .search
                .pending
                .as_ref()
                .map(|(q, _)| q.as_str()),
            Some("first")
        );
    }

    #[test]
    fn short_queries_never_issue() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        s.editor.files.search_edit.text = "a".into();
        on_query_change(&mut s, &mut tx);
        assert!(s.editor.files.search.pending.is_none());
        assert!(s.editor.files.search.results.is_none());
        s.editor.files.search_edit.text = "ab".into();
        on_query_change(&mut s, &mut tx);
        assert!(s.editor.files.search.pending.is_some());
    }

    #[test]
    fn poll_issues_pending_query_once() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s, 0.0, &mut tx));
        assert!(s.editor.files.search.searching);
        assert_eq!(s.editor.files.search.query, "needle");
        assert!(!poll(&mut s, 0.0, &mut tx));
    }

    #[test]
    fn set_mode_rearms_the_input_in_both_directions() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        s.editor.files.search_edit.text = "needle".into();
        set_mode(&mut s, &mut tx, SearchMode::Concept);
        assert_eq!(s.editor.files.search.mode, SearchMode::Concept);
        assert!(s.editor.files.search.pending.is_some());
        assert!(poll(&mut s, 0.0, &mut tx));
        assert_eq!(s.editor.files.search.query, "needle");
        set_mode(&mut s, &mut tx, SearchMode::Content);
        assert_eq!(s.editor.files.search.mode, SearchMode::Content);
        assert!(s.editor.files.search.pending.is_some());
        assert!(poll(&mut s, 0.0, &mut tx));
        assert_eq!(s.editor.files.search.query, "needle");
    }

    #[test]
    fn set_mode_clears_searching_with_an_empty_input() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.files.search.searching = true;
        set_mode(&mut s, &mut tx, SearchMode::Content);
        assert!(!s.editor.files.search.searching);
        assert!(s.editor.files.search.pending.is_none());
    }

    #[test]
    fn force_content_only_flips_a_concept_pane() {
        let mut search = Search::default();
        search.mode = SearchMode::Concept;
        search.query = "needle".into();
        force_content(&mut search);
        assert_eq!(search.mode, SearchMode::Content);
        assert!(search.query.is_empty());
        let mut content = Search::default();
        content.query = "keep".into();
        force_content(&mut content);
        assert_eq!(content.query, "keep");
    }

    #[test]
    fn concept_query_waits_for_a_sync_when_a_pass_succeeded() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        let key = s.editor.checkout_key.clone().unwrap();
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.files.search.last_pass.insert(
            key.clone(),
            PassRecord {
                checkout_key: key.clone(),
                finished: 0.0,
                ok: true,
            },
        );
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s, 1.0, &mut tx));
        assert!(matches!(
            s.editor.files.search.sync,
            SyncState::Waiting { deadline, .. } if (deadline - 2.0).abs() < f64::EPSILON
        ));
        assert!(s.editor.files.search.searching);
        assert_eq!(requests(&s), ["sync"]);
        apply_index_event(&mut s, &mut tx, IndexEvent::Synced { checkout_key: key });
        assert_eq!(s.editor.files.search.sync, SyncState::Idle);
        assert_eq!(s.editor.files.search.query, "needle");
    }

    #[test]
    fn expired_sync_issues_and_a_late_synced_reissues() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.files.search.sync = SyncState::Waiting {
            query: "needle".into(),
            deadline: 1.0,
        };
        assert!(poll(&mut s, 2.0, &mut tx));
        assert!(matches!(
            s.editor.files.search.sync,
            SyncState::Expired { .. }
        ));
        assert_eq!(s.editor.files.search.query, "needle");
        let before = s.editor.files.search.results.clone();
        let key = s.editor.checkout_key.clone().unwrap();
        apply_index_event(&mut s, &mut tx, IndexEvent::Synced { checkout_key: key });
        assert_eq!(s.editor.files.search.sync, SyncState::Idle);
        assert_eq!(s.editor.files.search.results, before);
    }

    #[test]
    fn no_pass_issues_immediately_and_requests_a_pass() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s, 0.0, &mut tx));
        assert_eq!(s.editor.files.search.sync, SyncState::Idle);
        assert_eq!(s.editor.files.search.query, "needle");
        assert_eq!(requests(&s), ["pass"]);
    }

    #[test]
    fn a_failed_pass_is_retried_once_per_interval() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        let key = s.editor.checkout_key.clone().unwrap();
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.files.search.last_pass.insert(
            key.clone(),
            PassRecord {
                checkout_key: key,
                finished: 0.0,
                ok: false,
            },
        );
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s, 5.0, &mut tx));
        assert!(requests(&s).is_empty());
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s, 11.0, &mut tx));
        assert_eq!(requests(&s), ["pass"]);
    }

    #[test]
    fn a_sync_is_requested_before_a_due_pass() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        let key = s.editor.checkout_key.clone().unwrap();
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.files.search.last_pass.insert(
            key.clone(),
            PassRecord {
                checkout_key: key,
                finished: 0.0,
                ok: true,
            },
        );
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s, 11.0, &mut tx));
        assert_eq!(requests(&s), ["sync", "pass"]);
    }

    #[test]
    fn ensure_index_coalesces_per_checkout() {
        let mut s = state_with_checkout();
        assert!(ensure_index(&mut s, 0.0));
        assert!(!ensure_index(&mut s, 0.0));
        assert_eq!(requests(&s), ["pass"]);
    }

    #[test]
    fn returning_to_a_checkout_reuses_its_pass_record() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        let key_a = s.editor.checkout_key.clone().unwrap();
        s.editor.files.search.last_pass.insert(
            key_a.clone(),
            PassRecord {
                checkout_key: key_a.clone(),
                finished: 0.0,
                ok: true,
            },
        );
        s.editor.files.search.open = true;
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.checkout = Some(PathBuf::from("/tmp/co-b"));
        s.editor.checkout_key = Some(checkout_key("p1", &PathBuf::from("/tmp/co-b")));
        reset(&mut s);
        assert_eq!(requests(&s), ["pass"]);
        s.editor.checkout = Some(PathBuf::from("/tmp/co"));
        s.editor.checkout_key = Some(key_a);
        reset(&mut s);
        assert_eq!(requests(&s), ["pass"]);
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s, 5.0, &mut tx));
        assert!(matches!(
            s.editor.files.search.sync,
            SyncState::Waiting { .. }
        ));
    }

    #[test]
    fn run_pass_emits_progress_then_done() {
        let (_dir, root) = repository("search-run-pass", &[("tracked.txt", "tracked needle\n")]);
        for n in 0..249 {
            fs::write(root.join(format!("f{n}.txt")), "needle\n").unwrap();
        }
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "many"]);
        let home = TempDir::new("search-run-home");
        let stop = AtomicBool::new(false);
        let mut events = Vec::new();
        run_pass(home.path(), &root, "k", &stop, &mut |event| {
            events.push(event)
        });
        assert!(events.iter().any(|event| matches!(
            event,
            IndexEvent::Progress {
                done: 100,
                total: 250,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            IndexEvent::Progress {
                done: 200,
                total: 250,
                ..
            }
        )));
        assert!(matches!(events.last(), Some(IndexEvent::Done { .. })));
        assert!(
            !app_core::code_index::search(home.path(), &root, "needle")
                .unwrap()
                .matches
                .is_empty()
        );
    }

    #[test]
    fn a_stopped_pass_emits_no_done() {
        let (_dir, root) = repository("search-stop-pass", &[("a.txt", "needle\n")]);
        for n in 0..149 {
            fs::write(root.join(format!("f{n}.txt")), "needle\n").unwrap();
        }
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "many"]);
        let home = TempDir::new("search-stop-home");
        let stop = AtomicBool::new(false);
        let mut events = Vec::new();
        run_pass(home.path(), &root, "k", &stop, &mut |event| {
            if matches!(event, IndexEvent::Progress { done: 100, .. }) {
                stop.store(true, Ordering::SeqCst);
            }
            events.push(event);
        });
        assert!(
            events
                .iter()
                .any(|event| matches!(event, IndexEvent::Progress { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, IndexEvent::Done { .. }))
        );
    }

    #[test]
    fn a_pass_on_a_non_repository_fails() {
        let dir = TempDir::new("search-non-repo");
        let home = TempDir::new("search-non-repo-home");
        let stop = AtomicBool::new(false);
        let mut events = Vec::new();
        run_pass(home.path(), dir.path(), "k", &stop, &mut |event| {
            events.push(event)
        });
        assert!(matches!(events.last(), Some(IndexEvent::Failed { .. })));
    }

    #[test]
    fn run_sync_reports_a_file_added_just_before() {
        let (_dir, root) = repository("search-run-sync", &[("a.txt", "alpha\n")]);
        let home = TempDir::new("search-sync-home");
        app_core::code_index::refresh(home.path(), &root, |_, _| true).unwrap();
        fs::write(root.join("b.txt"), "zebraneedle\n").unwrap();
        let stop = AtomicBool::new(false);
        let mut events = Vec::new();
        run_sync(home.path(), &root, "k", &stop, &mut |event| {
            events.push(event)
        });
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], IndexEvent::Synced { .. }));
        assert!(
            !app_core::code_index::search(home.path(), &root, "zebraneedle")
                .unwrap()
                .matches
                .is_empty()
        );
    }

    #[test]
    fn run_sync_still_reports_synced_on_failure() {
        let dir = TempDir::new("search-sync-plain");
        let home = TempDir::new("search-sync-plain-home");
        let stop = AtomicBool::new(false);
        let mut events = Vec::new();
        run_sync(home.path(), dir.path(), "k", &stop, &mut |event| {
            events.push(event)
        });
        assert!(matches!(events.last(), Some(IndexEvent::Synced { .. })));
    }

    #[test]
    fn request_sync_coalesces_queued_requests() {
        let mut indexer = Indexer::idle();
        let root = PathBuf::from("/tmp/co");
        assert!(indexer.request_sync(root.clone(), "k".into()));
        assert!(!indexer.request_sync(root.clone(), "k".into()));
        indexer.syncs.lock().unwrap().remove("k");
        assert!(indexer.request_sync(root, "k".into()));
    }

    #[test]
    fn apply_index_event_records_and_reissues_only_where_it_should() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        let key = s.editor.checkout_key.clone().unwrap();
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.files.search.open = true;
        s.editor.files.search.query = "needle".into();
        s.editor.files.search_edit.text = "needle".into();
        apply_index_event(
            &mut s,
            &mut tx,
            IndexEvent::Progress {
                checkout_key: key.clone(),
                done: 10,
                total: 20,
            },
        );
        assert_eq!(
            s.editor.files.search.index,
            IndexStatus::Building {
                done: 10,
                total: 20
            }
        );
        apply_index_event(
            &mut s,
            &mut tx,
            IndexEvent::Done {
                checkout_key: key.clone(),
                truncated: false,
            },
        );
        assert!(s.editor.files.search.last_pass[&key].ok);
        assert!(s.editor.files.search.pending.is_some());
        apply_index_event(
            &mut s,
            &mut tx,
            IndexEvent::Failed {
                checkout_key: key.clone(),
                message: "boom".into(),
            },
        );
        assert!(!s.editor.files.search.last_pass[&key].ok);
        assert_eq!(
            s.editor.files.search.index,
            IndexStatus::Failed("boom".into())
        );
        apply_index_event(
            &mut s,
            &mut tx,
            IndexEvent::Done {
                checkout_key: "other".into(),
                truncated: false,
            },
        );
        assert!(!s.editor.files.search.last_pass["other"].ok == false);
    }

    #[test]
    fn reset_returns_the_status_to_idle() {
        let mut s = state_with_checkout();
        s.editor.files.search.index = IndexStatus::Building { done: 1, total: 2 };
        s.editor.files.search.results = None;
        reset(&mut s);
        assert_eq!(s.editor.files.search.index, IndexStatus::Idle);
    }

    #[test]
    fn issuing_a_query_shows_the_results_and_a_short_box_hides_them() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        assert!(!s.editor.files.search.open);
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(
            !s.editor.files.search.open,
            "typing alone shows nothing until the debounce fires"
        );
        assert!(poll(&mut s, 1.0, &mut tx));
        assert!(s.editor.files.search.open);
        s.editor.files.search_edit.text = "n".into();
        on_query_change(&mut s, &mut tx);
        assert!(!s.editor.files.search.open);
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s, 2.0, &mut tx));
        assert!(s.editor.files.search.open);
        s.editor.files.search_edit.text.clear();
        on_query_change(&mut s, &mut tx);
        assert!(!s.editor.files.search.open);
    }

    #[test]
    fn concept_queries_show_the_results_too() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.files.search_edit.text = "needle".into();
        on_query_change(&mut s, &mut tx);
        assert!(poll(&mut s, 1.0, &mut tx));
        assert!(s.editor.files.search.open);
    }

    #[test]
    fn a_finished_pass_or_sync_never_pulls_the_aside_back_to_the_results() {
        let mut s = state_with_checkout();
        let mut tx = Tx::default();
        let key = s.editor.checkout_key.clone().unwrap();
        s.editor.files.search.mode = SearchMode::Concept;
        s.editor.files.search.open = false;
        s.editor.files.search.query = "needle".into();
        s.editor.files.search_edit.text = "needle".into();
        apply_index_event(
            &mut s,
            &mut tx,
            IndexEvent::Done {
                checkout_key: key.clone(),
                truncated: false,
            },
        );
        assert!(s.editor.files.search.pending.is_none());
        assert!(!s.editor.files.search.open);
        s.editor.files.search.sync = SyncState::Waiting {
            query: "needle".into(),
            deadline: 9.0,
        };
        apply_index_event(&mut s, &mut tx, IndexEvent::Synced { checkout_key: key });
        assert_eq!(s.editor.files.search.sync, SyncState::Idle);
        assert!(
            !s.editor.files.search.open,
            "the sync reply is stored, the filtered tree stays"
        );
    }

    #[test]
    fn a_checkout_switch_shows_the_tree_keeps_the_mode_and_requests_a_pass() {
        let mut s = state_with_checkout();
        s.editor.files.search.open = true;
        s.editor.files.search.mode = SearchMode::Concept;
        reset(&mut s);
        assert!(!s.editor.files.search.open);
        assert_eq!(s.editor.files.search.mode, SearchMode::Concept);
        assert_eq!(
            requests(&s),
            ["pass"],
            "no open pane is needed to start the pass"
        );
        let mut s = state_with_checkout();
        s.editor.files.search.open = true;
        reset(&mut s);
        assert!(requests(&s).is_empty(), "Text mode starts no pass");
    }
}
