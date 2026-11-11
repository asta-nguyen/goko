//! ACP commit-message generation on an owned Tokio runtime.
//!
//! The `AcpHub` and its agent connections live on a dedicated runtime thread
//! so `generate_commit_message`'s timeouts, denied permissions, temp-session
//! isolation and child-process cleanup are exactly the shared backend's. The
//! UI sends commands over an unbounded channel; completions come back through
//! `AppHandle::update` carrying the checkout key they were issued for, so a
//! stale generation can never overwrite another checkout's draft.

use std::path::PathBuf;
use std::sync::mpsc;

use app_core::acp::{self, AcpHub};
#[cfg(not(test))]
use app_core::home;
use app_core::store::Storage;
use rune::AppHandle;
use tokio::sync::mpsc::UnboundedSender;

use crate::State;

/// What the runtime thread should do next.
enum AcpCmd {
    Connect {
        agent_id: String,
    },
    Generate {
        agent_id: String,
        project_id: String,
        checkout_path: String,
        checkout_key: String,
        /// Per-run nonce from `Acp::run_seq`; the completion applies only
        /// while it still names the tracked run.
        run: u64,
        /// Snapshot of the commit draft when Generate was pressed; the
        /// completion only fills the draft if it is unchanged.
        draft_snapshot: String,
    },
    Cancel {
        project_id: String,
        checkout_path: String,
    },
    /// Persist the commit-panel agent pick under `commit.agent` — the same
    /// settings row the desktop's `useCommitAgent` mutation writes.
    PersistAgent {
        agent_id: String,
        /// `Acp::pick_seq` of this pick; only the latest pick's ack may
        /// resolve the pending slot.
        pick: u64,
    },
    RefreshAgents {
        /// `Acp::pick_seq` at send time; the stored-preference publish is
        /// skipped when a pick happened while the worker read the store.
        pick_seq: u64,
    },
    /// The Settings ACP page's save: validate and store the override rows
    /// and disconnect agents whose `enabled`/`command` changed — the
    /// desktop's `acp_agents_set`. Runs here because `agents_set` needs the
    /// hub this thread owns.
    SetAgents {
        overrides: Vec<app_core::acp::AcpOverride>,
    },
    /// The Settings ACP page's Reset: drop one agent's stored override so
    /// the preset is restored, preserving every other saved agent.
    ResetAgent {
        agent_id: String,
    },
    Shutdown,
}

/// UI-side handle: command sender plus the last reported agent states.
pub struct Acp {
    tx: Option<UnboundedSender<AcpCmd>>,
    /// Join handle for the runtime thread; `shutdown` waits on it so no
    /// agent process can outlive the app.
    thread: Option<std::thread::JoinHandle<()>>,
    pub agents: Vec<acp::AcpAgent>,
    pub statuses: std::collections::BTreeMap<String, acp::AcpStatus>,
    /// Checkout key a Generate is running for (drives the Cancel button).
    pub generating_for: Option<String>,
    pub error: Option<String>,
    /// Monotonic Generate run id; `active_run` names the one the UI still
    /// tracks, so a cancelled or superseded completion can never report
    /// back into the draft or clear a newer run's flags.
    run_seq: u64,
    active_run: Option<u64>,
    /// Monotonic commit-agent pick counter — `RefreshAgents` snapshots it
    /// so a stored-preference publish can't overwrite a newer user pick.
    pick_seq: u64,
    /// Latest pick awaiting its `commit.agent` write ack: (seq, the
    /// selection it replaced — restored if the write fails).
    pending_pick: Option<(u64, Option<String>)>,
}

impl Acp {
    pub fn new(handle: AppHandle<State>) -> Acp {
        let (ready_tx, ready_rx) = mpsc::channel::<UnboundedSender<AcpCmd>>();
        let thread = std::thread::Builder::new()
            .name("goko-editor-acp".into())
            .spawn(move || runtime_main(handle, ready_tx))
            .expect("spawn editor acp runtime");
        let tx = ready_rx
            .recv()
            .expect("acp runtime sends its command channel");
        Acp {
            tx: Some(tx),
            thread: Some(thread),
            agents: Vec::new(),
            statuses: Default::default(),
            generating_for: None,
            error: None,
            run_seq: 0,
            active_run: None,
            pick_seq: 0,
            pending_pick: None,
        }
    }

    fn send(&mut self, cmd: AcpCmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
    }

    /// Ask the runtime thread to republish agents + connection statuses.
    pub fn refresh(&mut self) {
        self.send(AcpCmd::RefreshAgents {
            pick_seq: self.pick_seq,
        });
    }

    /// Store the settings page's override rows, then republish the agents
    /// so the page and the commit picker see the effective list.
    pub fn set_agents(&mut self, overrides: Vec<app_core::acp::AcpOverride>) {
        self.send(AcpCmd::SetAgents { overrides });
    }

    /// Reset one agent to its preset, through the same runtime that owns
    /// the hub and the settings store.
    pub fn reset_agent(&mut self, agent_id: &str) {
        self.send(AcpCmd::ResetAgent {
            agent_id: agent_id.to_string(),
        });
    }

    pub fn connect(&mut self, agent_id: &str) {
        self.send(AcpCmd::Connect {
            agent_id: agent_id.to_string(),
        });
    }

    /// Begin Generate for the current checkout+draft. Returns `false` when
    /// no agent is configured or a generation is already running here.
    /// Takes the identity fields it needs so callers can split borrows
    /// (`s.editor.acp.generate(&s.editor.commit, …)` stays disjoint).
    pub fn generate(
        &mut self,
        commit: &crate::editor::state::CommitState,
        project_id: &Option<String>,
        checkout: &Option<PathBuf>,
        checkout_key: &Option<String>,
    ) -> bool {
        if self.generating_for.is_some() {
            return false;
        }
        let Some(agent_id) = self.pick_agent(commit.agent_id.as_deref()) else {
            self.error = Some("no ACP agent is configured for commit messages".into());
            return false;
        };
        let (Some(project_id), Some(checkout), Some(key)) =
            (project_id.clone(), checkout.clone(), checkout_key.clone())
        else {
            return false;
        };
        let draft = commit.message.text.clone();
        self.run_seq += 1;
        let run = self.run_seq;
        self.generating_for = Some(key.clone());
        self.active_run = Some(run);
        self.error = None;
        self.send(AcpCmd::Generate {
            agent_id,
            project_id,
            checkout_path: checkout.display().to_string(),
            checkout_key: key,
            run,
            draft_snapshot: draft,
        });
        true
    }

    /// The agent Generate runs with — the same pick the commit panel
    /// shows: the stored preference while its agent stays enabled, else
    /// first connected, else first enabled+available. Disabled agents are
    /// never a fallthrough.
    fn pick_agent(&self, preferred: Option<&str>) -> Option<String> {
        crate::editor::commit::picked_agent_id(&self.agents, &self.statuses, preferred)
    }

    /// User picked `agent_id` in the commit panel: reflect it immediately
    /// (the picker stays responsive), arm the pending pick, and enqueue
    /// the `commit.agent` write — the same optimistic-then-persist shape
    /// as the desktop's `settingsService.set` mutation. The ack resolves
    /// through `EditorState::apply_agent_pick_ack`.
    pub fn choose_agent(&mut self, commit: &mut crate::editor::state::CommitState, agent_id: &str) {
        self.pick_seq += 1;
        let pick = self.pick_seq;
        self.pending_pick = Some((pick, commit.agent_id.clone()));
        commit.agent_id = Some(agent_id.to_string());
        self.error = None;
        self.send(AcpCmd::PersistAgent {
            agent_id: agent_id.to_string(),
            pick,
        });
    }

    pub fn cancel(&mut self, project_id: &Option<String>, checkout: &Option<PathBuf>) {
        let (Some(project_id), Some(checkout)) = (project_id.clone(), checkout.clone()) else {
            return;
        };
        // Invalidate before enqueue: the tracked run is dead UI-side now,
        // so a completion already in flight can never fill the draft or
        // clear a newer run's flags — even if the Cancel command is
        // delayed or the job already finished.
        self.generating_for = None;
        self.active_run = None;
        self.send(AcpCmd::Cancel {
            project_id,
            checkout_path: checkout.display().to_string(),
        });
    }

    /// App exit: close the command channel, then wait for the runtime thread
    /// to finish `hub.shutdown()` — agent child processes must not outlive
    /// the app. Blocks the UI thread briefly; called from `on_quit`.
    pub fn shutdown(&mut self) {
        self.send(AcpCmd::Shutdown);
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn runtime_main(handle: AppHandle<State>, ready: mpsc::Sender<UnboundedSender<AcpCmd>>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return,
    };
    runtime.block_on(async move {
        let hub = AcpHub::new();
        // The runtime thread owns its storage for agent config lookups.
        // Unit tests get `None`: no real GOKO_HOME, agent configs, or agent
        // processes may be touched from `cargo test`.
        #[cfg(not(test))]
        let storage = home::home_dir().ok().map(|h| Storage::open(&h));
        #[cfg(test)]
        let storage: Option<Storage> = None;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AcpCmd>();
        let _ = ready.send(tx);

        // `pick_seq` 0: no pick can precede the runtime's first publish.
        publish_agents(&handle, &hub, &storage, 0);

        while let Some(cmd) = rx.recv().await {
            if dispatch(&hub, &storage, &handle, cmd).await {
                break;
            }
        }
        hub.shutdown().await;
    });
}

/// The canonical checkout `Generate` runs Git/cwd work against:
/// `projects::git_checkout` revalidates the registered project and
/// returns the canonical path, so the raw UI path — possibly an alias
/// (`/var` vs `/private/var`, symlinked roots) — never reaches agent
/// work. `None` when storage is unavailable or the checkout is no
/// longer a registered project checkout. The hub job key is a separate
/// concern: Generate and Cancel share the raw UI path string, exactly
/// like the desktop frontend, and Cancel deliberately does not
/// revalidate so it still reaches a job whose project was deleted
/// mid-run.
fn canonical_checkout(
    storage: &Option<Storage>,
    project_id: &str,
    checkout_path: &str,
) -> Option<PathBuf> {
    storage.as_ref().and_then(|st| {
        st.get().ok().and_then(|store| {
            app_core::projects::git_checkout(store, project_id, checkout_path).ok()
        })
    })
}

/// Run one command on the runtime thread; returns `true` only for
/// `Shutdown`, which ends the receive loop. Split from `runtime_main` so
/// tests can drive the same dispatch against a real `AcpHub` and a temp
/// `Storage` — `AppHandle::update` deliveries just queue without the app
/// loop, which is harmless for the assertions these tests make.
async fn dispatch(
    hub: &AcpHub,
    storage: &Option<Storage>,
    handle: &AppHandle<State>,
    cmd: AcpCmd,
) -> bool {
    match cmd {
        AcpCmd::RefreshAgents { pick_seq } => publish_agents(handle, hub, storage, pick_seq),
        AcpCmd::SetAgents { overrides } => {
            let Some(store) = storage.as_ref() else {
                let hh = handle.clone();
                hh.update(|s: &mut State, _| {
                    s.notice = Some("the settings store is unavailable".into());
                });
                return false;
            };
            // A bad row rejects the whole save; the error surfaces as the
            // app's notice like every other worker-side failure.
            let result = acp::agents_set(store, hub, overrides).await;
            if let Err(error) = result {
                let hh = handle.clone();
                let message = error.to_string();
                hh.update(move |s: &mut State, _| {
                    s.notice = Some(message);
                });
            }
            publish_agents(handle, hub, storage, 0);
        }
        AcpCmd::ResetAgent { agent_id } => {
            let Some(store) = storage.as_ref() else {
                let hh = handle.clone();
                hh.update(|s: &mut State, _| {
                    s.notice = Some("the settings store is unavailable".into());
                });
                return false;
            };
            let result = acp::agents_reset(store, hub, &agent_id).await;
            if let Err(error) = result {
                let hh = handle.clone();
                let message = error.to_string();
                hh.update(move |s: &mut State, _| {
                    s.notice = Some(message);
                });
            }
            publish_agents(handle, hub, storage, 0);
        }
        AcpCmd::PersistAgent { agent_id, pick } => {
            // Same `settings` row the desktop's `commit.agent` mutation
            // writes — synchronous SQLite on this worker, acked back so a
            // failed write restores the previous pick instead of silently
            // losing the preference.
            let result = match storage.as_ref().and_then(|st| st.get().ok()) {
                Some(store) => store.set_setting(COMMIT_AGENT_KEY, &agent_id),
                None => Err(app_core::error::BridgeError::new(
                    app_core::error::Code::Storage,
                    "the settings store is unavailable",
                )),
            };
            let hh = handle.clone();
            hh.update(move |s: &mut State, _| {
                s.editor.apply_agent_pick_ack(pick, result);
            });
        }
        AcpCmd::Connect { agent_id } => {
            let config = agent_config(storage, &agent_id);
            match config {
                Some(config) => {
                    let h = hub.clone();
                    let hh = handle.clone();
                    let aid = agent_id.clone();
                    tokio::spawn(async move {
                        let result = h.connect(&config).await;
                        hh.update(move |s: &mut State, _| {
                            // Statuses always refresh; a connect
                            // failure surfaces as a notice, success
                            // clears it.
                            let statuses = h.status();
                            publish_statuses_into(s, &statuses);
                            match &result {
                                Ok(()) => {
                                    s.editor.acp.error = None;
                                    s.editor.notice = None;
                                }
                                Err(e) => {
                                    let msg = format!("could not connect {aid}: {e}");
                                    s.editor.acp.error = Some(msg.clone());
                                    s.editor.notice = Some(crate::editor::state::Notice {
                                        text: msg,
                                        stale: false,
                                    });
                                }
                            }
                        });
                    });
                }
                None => {
                    let hh = handle.clone();
                    hh.update(move |s: &mut State, _| {
                        s.editor.acp.error = Some(format!("agent {agent_id} is not configured"));
                    });
                }
            }
        }
        AcpCmd::Generate {
            agent_id,
            project_id,
            checkout_path,
            checkout_key,
            run,
            draft_snapshot,
        } => {
            // Revalidate the checkout identity on the worker side:
            // registered project + canonical path, same as every
            // backend job — ACP must never run against an arbitrary
            // or swapped path. The canonical path feeds Git/cwd work
            // only; the hub job key stays the raw UI path so Cancel —
            // which must keep working even if the project was deleted
            // while the job ran — always lands on the same key.
            let Some(canonical) = canonical_checkout(storage, &project_id, &checkout_path) else {
                let hh = handle.clone();
                let key = checkout_key.clone();
                let snap = draft_snapshot.clone();
                hh.update(move |s: &mut State, _| {
                    s.editor.apply_acp_result(
                        run,
                        key,
                        snap,
                        Err(acp_err(
                            "the checkout is no longer a registered project checkout",
                        )),
                    );
                });
                return false;
            };
            let Some(config) = agent_config(storage, &agent_id) else {
                let hh = handle.clone();
                let key = checkout_key.clone();
                let snap = draft_snapshot.clone();
                hh.update(move |s: &mut State, _| {
                    s.editor.apply_acp_result(
                        run,
                        key,
                        snap,
                        Err(acp_err("the agent is not configured")),
                    );
                });
                return false;
            };
            let h = hub.clone();
            let hh = handle.clone();
            tokio::spawn(async move {
                let result = h
                    .generate_commit_message(config, project_id, checkout_path, canonical)
                    .await;
                hh.update(move |s: &mut State, _| {
                    s.editor
                        .apply_acp_result(run, checkout_key, draft_snapshot, result);
                });
            });
        }
        AcpCmd::Cancel {
            project_id,
            checkout_path,
        } => {
            // Generate keys its hub job on this same raw UI path, so the
            // lookup always hits — no revalidation here: the project may
            // have been deleted while the job was running, and the job
            // key is a plain string that still identifies it.
            hub.cancel(&project_id, &checkout_path);
        }
        AcpCmd::Shutdown => return true,
    }
    false
}

fn acp_err(message: &str) -> app_core::error::BridgeError {
    app_core::error::BridgeError::new(
        app_core::error::Code::Daemon(protocol::message::ErrorCode::NotFound),
        message,
    )
}

/// `true` for the sentinel an aborted Generate reports — a user cancel or
/// a superseded job (`app_core::acp`'s `cancelled()`). Expected, so the
/// completion stays silent: never an error line, never a draft write.
fn is_cancelled(error: &app_core::error::BridgeError) -> bool {
    error.code == app_core::error::Code::Daemon(protocol::message::ErrorCode::Internal)
        && error.message == "cancelled"
}

fn agent_config(storage: &Option<Storage>, agent_id: &str) -> Option<app_core::acp::AgentConfig> {
    let storage = storage.as_ref()?;
    acp::agent_config(storage, agent_id).ok()
}

/// The settings row the desktop's `useCommitAgent` reads/writes — shared
/// `GOKO_HOME` storage means both frontends see the same preference.
const COMMIT_AGENT_KEY: &str = "commit.agent";

/// The persisted `commit.agent` preference: `None` when unset or the store
/// can't answer — a missing/malformed value just falls back to the
/// connected→available pick, like the desktop's `storedAgent === null`.
fn commit_agent_setting(storage: &Option<Storage>) -> Option<String> {
    storage.as_ref().and_then(|st| {
        st.get()
            .ok()
            .and_then(|store| store.get_setting(COMMIT_AGENT_KEY).ok().flatten())
    })
}

fn publish_agents(
    handle: &AppHandle<State>,
    hub: &AcpHub,
    storage: &Option<Storage>,
    pick_guard: u64,
) {
    let agents = storage.as_ref().and_then(|s| acp::agents_get(s).ok());
    let stored_agent = commit_agent_setting(storage);
    let statuses = hub.status();
    handle.update(move |s: &mut State, _| {
        publish_statuses_into(s, &statuses);
        // Only overwrite the agent list when the store actually answered —
        // a transient storage miss must not blank the picker.
        if let Some(agents) = &agents {
            s.editor.acp.agents = agents.clone();
            s.editor.acp_agents = agents.clone();
        }
        s.editor.apply_stored_agent(stored_agent, pick_guard);
    });
}

/// Statuses-only publish used after connect/disconnect where the agent list
/// is already known (or storage is unavailable).
fn publish_statuses_into(
    s: &mut State,
    statuses: &std::collections::BTreeMap<String, acp::AcpStatus>,
) {
    s.editor.acp.statuses = statuses.clone();
}

/// `commit.rs` entry points: free functions so the call site reads like the
/// desktop's `generateCommitMessage()`/`cancelGeneration()` buttons. Field
/// borrows stay disjoint (`acp` vs `commit`/`project_id`/`checkout`).
pub fn generate(s: &mut State) -> bool {
    let e = &mut s.editor;
    e.acp
        .generate(&e.commit, &e.project_id, &e.checkout, &e.checkout_key)
}

pub fn cancel(s: &mut State) {
    let e = &mut s.editor;
    e.acp.cancel(&e.project_id, &e.checkout);
}

/// Commit-panel agent pick: persist `commit.agent` through the ACP
/// runtime's Storage and reflect the choice optimistically (the write
/// ack restores the previous pick if it fails — see
/// `EditorState::apply_agent_pick_ack`).
pub fn choose_agent(s: &mut State, agent_id: &str) {
    let e = &mut s.editor;
    e.acp.choose_agent(&mut e.commit, agent_id);
}

impl crate::editor::state::EditorState {
    /// Apply a Generate completion: fill the commit draft only when the run
    /// is still the tracked one, the draft is still the snapshot it was
    /// generated from, and the checkout matches.
    pub fn apply_acp_result(
        &mut self,
        run: u64,
        key: String,
        snapshot: String,
        result: Result<String, app_core::error::BridgeError>,
    ) {
        // Only the run the UI still tracks may report: a completion for a
        // cancelled or superseded run drops entirely — it must not clear
        // the newer run's flags, touch the draft, or surface an error.
        if self.acp.active_run != Some(run) {
            return;
        }
        self.acp.active_run = None;
        // Only the generation this state is tracking clears the flag — a
        // stale reply for an older key must not cancel the Cancel button of
        // a newer run, and `commit.generating` follows the same rule.
        if self.acp.generating_for.as_deref() == Some(key.as_str()) {
            self.acp.generating_for = None;
            self.commit.generating = false;
        }
        if self.checkout_key.as_deref() != Some(key.as_str()) {
            return; // stale: another checkout is selected
        }
        // A generation the user cancelled reports `cancelled`: expected,
        // so it stays silent like the desktop — no error line, and never
        // a draft write even when the draft moved on while it wound down.
        if matches!(&result, Err(e) if is_cancelled(e)) {
            return;
        }
        if self.commit.message.text != snapshot {
            // The user edited (or another generation replaced) the draft —
            // a late reply must not clobber it.
            self.commit.generate_error =
                Some("generation finished after the draft changed; result discarded".into());
            return;
        }
        match result {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    self.commit.generate_error = Some("the agent returned an empty message".into());
                } else {
                    self.commit.message = rune::edit::TextEdit::new(trimmed.to_string());
                    self.commit.generate_error = None;
                }
            }
            Err(e) => {
                self.commit.generate_error = Some(e.to_string());
                self.acp.error = Some(e.to_string());
            }
        }
    }

    /// Ack of the worker's `commit.agent` write: only the latest pick may
    /// resolve — a stale ack leaves the newer pending pick untouched. The
    /// optimistic selection stays on success; a failed write restores the
    /// pre-pick agent and surfaces the error, so the preference is never
    /// silently lost.
    pub fn apply_agent_pick_ack(
        &mut self,
        pick: u64,
        result: Result<(), app_core::error::BridgeError>,
    ) {
        if self.acp.pending_pick.as_ref().map(|(p, _)| *p) != Some(pick) {
            return;
        }
        let (_, prev) = self
            .acp
            .pending_pick
            .take()
            .expect("pending pick checked above");
        if let Err(e) = result {
            self.commit.agent_id = prev;
            self.acp.error = Some(format!("could not save the commit agent: {e}"));
        }
    }

    /// Publish the stored `commit.agent` preference read on the worker —
    /// but only when `pick_guard` still equals the live `pick_seq`, i.e. no
    /// user pick happened while the worker read the store. A newer pick
    /// always wins over a stale refresh.
    pub fn apply_stored_agent(&mut self, stored: Option<String>, pick_guard: u64) {
        if self.acp.pick_seq == pick_guard {
            self.commit.agent_id = stored;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::editor::state::EditorState;
    use app_core::error::{BridgeError, Code};

    /// The sentinel `AcpHub` reports for an aborted generation — the same
    /// shape `app_core::acp`'s private `cancelled()` builds.
    fn cancelled() -> BridgeError {
        BridgeError::new(
            Code::Daemon(protocol::message::ErrorCode::Internal),
            "cancelled",
        )
    }

    fn editor() -> EditorState {
        EditorState::new(rune_kit::Kit::neutral(true), crate::test_handle())
    }

    /// Checkout `k1` mid-generation with the draft at its snapshot; the
    /// tracked run is nonce 1 so `apply_acp_result(1, …)` reports for it.
    fn generating(e: &mut EditorState) {
        e.checkout_key = Some("k1".into());
        e.acp.generating_for = Some("k1".into());
        e.acp.active_run = Some(1);
        e.commit.generating = true;
        e.commit.message = rune::edit::TextEdit::new("draft".to_string());
        e.commit.gen_snapshot = Some("draft".into());
    }

    #[test]
    fn stale_key_reply_preserves_running_generation() {
        let mut e = editor();
        generating(&mut e);
        // A reply for another checkout must not clear the in-flight run's
        // flags — the Cancel button and `generating` stay honest.
        e.apply_acp_result(1, "k2".into(), "draft".into(), Ok("late".into()));
        assert_eq!(e.acp.generating_for.as_deref(), Some("k1"));
        assert!(e.commit.generating);
        assert_eq!(e.commit.message.text, "draft");
    }

    #[test]
    fn matching_key_fills_unchanged_draft() {
        let mut e = editor();
        generating(&mut e);
        e.apply_acp_result(1, "k1".into(), "draft".into(), Ok("  feat: thing  ".into()));
        assert!(e.acp.generating_for.is_none());
        assert!(!e.commit.generating);
        assert_eq!(e.commit.message.text, "feat: thing");
        assert!(e.commit.generate_error.is_none());
    }

    #[test]
    fn edited_draft_discards_late_result() {
        let mut e = editor();
        generating(&mut e);
        e.commit.message = rune::edit::TextEdit::new("user edit".to_string());
        e.apply_acp_result(1, "k1".into(), "draft".into(), Ok("generated".into()));
        // The user's text wins; the stale completion is reported, not applied.
        assert_eq!(e.commit.message.text, "user edit");
        assert!(e.commit.generate_error.is_some());
        assert!(!e.commit.generating);
    }

    #[test]
    fn failure_surfaces_error_and_clears_flag() {
        let mut e = editor();
        generating(&mut e);
        e.apply_acp_result(
            1,
            "k1".into(),
            "draft".into(),
            Err(BridgeError::new(Code::Storage, "agent exploded")),
        );
        assert!(e.acp.generating_for.is_none());
        assert!(!e.commit.generating);
        assert!(e.commit.generate_error.is_some());
        assert!(e.acp.error.is_some());
    }

    #[test]
    fn empty_agent_message_is_an_error_not_a_clear() {
        let mut e = editor();
        generating(&mut e);
        e.apply_acp_result(1, "k1".into(), "draft".into(), Ok("   ".into()));
        assert_eq!(e.commit.message.text, "draft");
        assert!(e.commit.generate_error.is_some());
    }

    #[test]
    fn cancelled_result_keeps_draft_and_stays_silent() {
        let mut e = editor();
        generating(&mut e);
        e.apply_acp_result(1, "k1".into(), "draft".into(), Err(cancelled()));
        // A user-cancelled generation is expected: flags resolve, the draft
        // is untouched, and no error line appears (desktop parity).
        assert!(e.acp.generating_for.is_none());
        assert!(!e.commit.generating);
        assert_eq!(e.commit.message.text, "draft");
        assert!(e.commit.generate_error.is_none());
        assert!(e.acp.error.is_none());
    }

    #[test]
    fn cancelled_result_after_an_edit_is_still_silent() {
        let mut e = editor();
        generating(&mut e);
        e.commit.message = rune::edit::TextEdit::new("user edit".to_string());
        e.apply_acp_result(1, "k1".into(), "draft".into(), Err(cancelled()));
        // The draft moved on, but the run was cancelled — not "finished
        // after the draft changed", so no notice either.
        assert_eq!(e.commit.message.text, "user edit");
        assert!(e.commit.generate_error.is_none());
        assert!(e.acp.error.is_none());
    }

    /// Arm a real Generate through `Acp::generate` for a fixed project +
    /// checkout, returning the run nonce it claimed. Sends land on the
    /// runtime thread spawned by `EditorState::new`; under `cfg(test)` its
    /// storage is `None`, so dispatch reports the validation error into a
    /// queued `update` that never runs — harmless here.
    fn armed_run(e: &mut EditorState) -> u64 {
        e.project_id = Some("p1".into());
        e.checkout = Some(std::path::PathBuf::from("/tmp/goko-acp-co"));
        e.checkout_key = Some(crate::editor::state::checkout_key(
            "p1",
            &std::path::PathBuf::from("/tmp/goko-acp-co"),
        ));
        e.acp.agents = vec![app_core::acp::AcpAgent {
            id: "a".into(),
            label: "A".into(),
            enabled: true,
            command: "unused".into(),
            prompt: String::new(),
            available: true,
            is_default: false,
        }];
        e.commit.generating = true;
        e.commit.message = rune::edit::TextEdit::new("draft".to_string());
        assert!(
            e.acp
                .generate(&e.commit, &e.project_id, &e.checkout, &e.checkout_key)
        );
        e.acp.active_run.expect("generate arms a tracked run")
    }

    #[test]
    fn cancel_invalidates_the_run_before_any_reply() {
        let mut e = editor();
        let run = armed_run(&mut e);
        let key = e.checkout_key.clone().unwrap();
        e.acp.cancel(&e.project_id, &e.checkout);
        // The tracked run is dead UI-side the moment Cancel is pressed —
        // before the command is even processed by the worker.
        assert!(e.acp.generating_for.is_none());
        assert!(e.acp.active_run.is_none());
        // Its late success drops entirely: no draft write, no flags, no
        // error — even though key and snapshot would still have matched.
        e.apply_acp_result(run, key, "draft".into(), Ok("stale".into()));
        assert_eq!(e.commit.message.text, "draft");
        assert!(e.commit.generate_error.is_none());
        assert!(e.acp.error.is_none());
    }

    #[test]
    fn superseded_run_result_drops_while_new_run_keeps_going() {
        let mut e = editor();
        let first = armed_run(&mut e);
        let key = e.checkout_key.clone().unwrap();
        e.acp.cancel(&e.project_id, &e.checkout);
        // Same checkout, unchanged draft: key AND snapshot are identical
        // across the two runs — only the per-run nonce tells them apart.
        let second = armed_run(&mut e);
        assert_ne!(first, second);
        assert_eq!(e.checkout_key.as_deref(), Some(key.as_str()));
        // The cancelled run's late success must neither fill the draft
        // nor clear the replacement run's flags.
        e.apply_acp_result(first, key.clone(), "draft".into(), Ok("stale".into()));
        assert_eq!(e.commit.message.text, "draft");
        assert_eq!(e.acp.generating_for.as_deref(), Some(key.as_str()));
        assert!(e.commit.generating);
        assert!(e.commit.generate_error.is_none());
        assert!(e.acp.error.is_none());
        // The live run still reports normally.
        e.apply_acp_result(second, key, "draft".into(), Ok("fresh".into()));
        assert_eq!(e.commit.message.text, "fresh");
        assert!(e.acp.generating_for.is_none());
        assert!(!e.commit.generating);
    }

    // -- commit.agent persistence (desktop useCommitAgent parity) ---------

    fn acp_agent(id: &str, enabled: bool, available: bool) -> app_core::acp::AcpAgent {
        app_core::acp::AcpAgent {
            id: id.into(),
            label: format!("Agent {id}"),
            enabled,
            command: "cmd".into(),
            prompt: String::new(),
            available,
            is_default: false,
        }
    }

    #[test]
    fn pick_agent_matches_desktop_fallback_and_never_disabled() {
        use app_core::acp::AgentState;

        let mut e = editor();
        e.acp.agents = vec![
            acp_agent("a", true, true),
            acp_agent("b", true, true),
            acp_agent("off", false, true),
            acp_agent("unavail", true, false),
        ];
        e.acp.statuses.insert(
            "b".into(),
            app_core::acp::AcpStatus {
                state: AgentState::Connected,
                message: None,
            },
        );
        // Stored pick on a disabled agent → first connected enabled,
        // never the disabled row.
        assert_eq!(e.acp.pick_agent(Some("off")).as_deref(), Some("b"));
        // Stored pick on enabled-but-unavailable still wins — the desktop
        // keeps the preference while the agent stays enabled.
        assert_eq!(
            e.acp.pick_agent(Some("unavail")).as_deref(),
            Some("unavail")
        );
        // Unknown stored id → connected enabled.
        assert_eq!(e.acp.pick_agent(Some("gone")).as_deref(), Some("b"));
        // Nothing connected → first enabled+available.
        e.acp.statuses.clear();
        assert_eq!(e.acp.pick_agent(None).as_deref(), Some("a"));
        // Only disabled agents → None: no fallthrough for Generate.
        let mut e2 = editor();
        e2.acp.agents = vec![acp_agent("x", false, true)];
        assert!(e2.acp.pick_agent(None).is_none());
        assert!(e2.acp.pick_agent(Some("x")).is_none());
    }

    #[test]
    fn choose_agent_reflects_pick_and_arms_ack() {
        let mut e = editor();
        e.commit.agent_id = Some("a".into());
        e.acp.choose_agent(&mut e.commit, "b");
        // Optimistic selection is immediate; the pending pick remembers
        // what it replaced for a possible rollback.
        assert_eq!(e.commit.agent_id.as_deref(), Some("b"));
        assert_eq!(e.acp.pending_pick, Some((1, Some("a".into()))));
        e.apply_agent_pick_ack(1, Ok(()));
        assert_eq!(e.commit.agent_id.as_deref(), Some("b"));
        assert!(e.acp.pending_pick.is_none());
    }

    #[test]
    fn failed_pick_write_restores_previous_agent_and_surfaces_error() {
        let mut e = editor();
        e.commit.agent_id = Some("a".into());
        e.acp.choose_agent(&mut e.commit, "b");
        e.apply_agent_pick_ack(1, Err(BridgeError::new(Code::Storage, "disk full")));
        // Old preference restored, pending cleared, failure is visible.
        assert_eq!(e.commit.agent_id.as_deref(), Some("a"));
        assert!(e.acp.pending_pick.is_none());
        assert!(
            e.acp
                .error
                .as_deref()
                .is_some_and(|m| m.contains("commit agent"))
        );
    }

    #[test]
    fn stale_pick_ack_leaves_newer_pick_pending() {
        let mut e = editor();
        e.commit.agent_id = Some("a".into());
        e.acp.choose_agent(&mut e.commit, "b"); // pick 1
        e.acp.choose_agent(&mut e.commit, "c"); // pick 2, replaced "b"
        // Pick 1's ack lands late — it must not resolve pick 2's slot.
        e.apply_agent_pick_ack(1, Ok(()));
        assert_eq!(e.commit.agent_id.as_deref(), Some("c"));
        assert_eq!(e.acp.pending_pick, Some((2, Some("b".into()))));
        // Pick 2's own failure restores the selection it replaced.
        e.apply_agent_pick_ack(2, Err(BridgeError::new(Code::Storage, "io")));
        assert_eq!(e.commit.agent_id.as_deref(), Some("b"));
        assert!(e.acp.pending_pick.is_none());
    }

    #[test]
    fn stored_agent_publish_never_overwrites_a_newer_pick() {
        let mut e = editor();
        // A refresh snapshot taken before any pick applies the stored value.
        e.apply_stored_agent(Some("a".into()), 0);
        assert_eq!(e.commit.agent_id.as_deref(), Some("a"));
        // The pick bumps pick_seq: the stale refresh's stored value drops.
        e.acp.choose_agent(&mut e.commit, "b");
        e.apply_stored_agent(Some("a".into()), 0);
        assert_eq!(e.commit.agent_id.as_deref(), Some("b"));
        // A refresh sent after the pick may publish again — by then the
        // worker's store read already sees the persisted value.
        e.apply_stored_agent(Some("b".into()), 1);
        assert_eq!(e.commit.agent_id.as_deref(), Some("b"));
        // Missing stored value → None → the connected→available fallback.
        let mut e2 = editor();
        e2.apply_stored_agent(None, 0);
        assert!(e2.commit.agent_id.is_none());
    }

    #[test]
    fn unknown_stored_agent_publishes_but_pick_falls_back() {
        let mut e = editor();
        e.acp.agents = vec![acp_agent("a", true, true)];
        // The raw preference is kept (desktop keeps `storedAgent` verbatim)…
        e.apply_stored_agent(Some("ghost".into()), 0);
        assert_eq!(e.commit.agent_id.as_deref(), Some("ghost"));
        // …but Generate resolves through the same enabled-fallback chain,
        // so a malformed value can never reach an agent.
        assert_eq!(
            e.acp.pick_agent(e.commit.agent_id.as_deref()).as_deref(),
            Some("a")
        );
    }

    /// The worker-side half of `commit.agent`: `PersistAgent` writes the
    /// same settings row the desktop mutation uses, and a fresh UI state
    /// reloads it through the read `publish_agents` performs — persist /
    /// reload across state, temp GOKO_HOME only.
    #[tokio::test]
    async fn commit_agent_persists_and_reloads_through_worker_storage() {
        use app_core::acp::AcpHub;
        use app_core::store::Storage;
        use app_core::test_support::TempDir;

        use super::{AcpCmd, commit_agent_setting, dispatch};

        let home = TempDir::new("acp-agent-store");
        let storage = Some(Storage::open(home.path()));
        let hub = AcpHub::new();
        let handle = crate::test_handle();

        // Missing key → safe fallback, exactly like `storedAgent === null`.
        assert!(commit_agent_setting(&storage).is_none());

        // The worker writes `commit.agent`; the ack queues harmlessly
        // without the app loop.
        assert!(
            !dispatch(
                &hub,
                &storage,
                &handle,
                AcpCmd::PersistAgent {
                    agent_id: "opencode".into(),
                    pick: 1,
                },
            )
            .await
        );
        let saved = storage
            .as_ref()
            .unwrap()
            .get()
            .unwrap()
            .get_setting("commit.agent")
            .unwrap();
        assert_eq!(saved.as_deref(), Some("opencode"));

        // A brand-new UI state picks the preference up on refresh.
        let mut e = editor();
        e.apply_stored_agent(commit_agent_setting(&storage), 0);
        assert_eq!(e.commit.agent_id.as_deref(), Some("opencode"));
    }

    // -- generate/cancel lifecycle against a real hub and fake agent ------

    /// Generate and Cancel must agree on the hub job key, which is the
    /// raw UI path on both sides (desktop parity) — while the canonical
    /// `projects::git_checkout` path feeds the Git/cwd work. This drives
    /// `dispatch` — the same code `runtime_main` runs — with a temp
    /// `Storage` (temp GOKO_HOME), a temp registered git project reached
    /// through a symlink alias, and the fake ACP agent, so no real home,
    /// network or user process is touched. `AppHandle::update` deliveries
    /// queue harmlessly without the app loop; the agent's own log is the
    /// observable wire evidence.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_reaches_the_job_under_the_aliased_checkout() {
        use std::path::{Path, PathBuf};
        use std::process::Command;
        use std::time::Duration;

        use app_core::acp::{AcpHub, AcpOverride, AgentState};
        use app_core::store::Storage;
        use app_core::test_support::TempDir;

        use super::{AcpCmd, dispatch};

        fn git_ok(root: &Path, args: &[&str]) {
            let out = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .expect("git run");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }

        fn log_events(dir: &Path) -> Vec<serde_json::Value> {
            std::fs::read_to_string(dir.join("agent.log"))
                .unwrap_or_default()
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        }

        async fn wait_log(dir: &Path, event: &str) {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if log_events(dir)
                        .iter()
                        .any(|entry| entry["event"].as_str() == Some(event))
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("the agent never logged {event}"));
        }

        let home = TempDir::new("acp-alias-home");
        let storage = Storage::open(home.path());
        let hub = AcpHub::new();

        // A registered project whose checkout the UI knows through a
        // symlink — textually different from the canonical path the
        // worker resolves for Git work (the `/var` vs `/private/var`
        // case, made explicit).
        let repo_dir = TempDir::new("acp-alias-repo");
        let repo = repo_dir.path().canonicalize().unwrap();
        git_ok(&repo, &["init", "-b", "main"]);
        git_ok(&repo, &["config", "user.name", "Tester"]);
        git_ok(&repo, &["config", "user.email", "tester@example.com"]);
        std::fs::write(repo.join("tracked.txt"), "tracked\n").unwrap();
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-qm", "init"]);
        std::fs::write(repo.join("demo.txt"), "demo\n").unwrap();
        git_ok(&repo, &["add", "demo.txt"]);

        let link_dir = TempDir::new("acp-alias-link");
        let alias: PathBuf = link_dir.path().join("aliased-checkout");
        std::os::unix::fs::symlink(&repo, &alias).unwrap();
        let raw_checkout = alias.display().to_string();
        assert_ne!(raw_checkout, repo.display().to_string());

        let project_id = storage
            .get()
            .unwrap()
            .add_project(
                "p",
                repo_dir.path().to_str().unwrap(),
                app_core::store::DEFAULT_WORKSPACE_ID,
            )
            .unwrap()
            .id;

        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/app-core/tests/fixtures/fake_acp_agent.py");
        app_core::acp::agents_set(
            &storage,
            &hub,
            vec![AcpOverride {
                id: "opencode".into(),
                enabled: true,
                command: format!(
                    "FAKE_ACP_CASE=hang_prompt FAKE_ACP_LOG={} python3 {}",
                    home.path().join("agent.log").display(),
                    fixture.display()
                ),
                prompt: String::new(),
            }],
        )
        .await
        .unwrap();
        let config = app_core::acp::agent_config(&storage, "opencode").unwrap();
        hub.connect(&config).await.unwrap();
        assert_eq!(hub.status()["opencode"].state, AgentState::Connected);

        let storage = Some(storage);
        let handle = crate::test_handle();
        let key = crate::editor::state::checkout_key(&project_id, &alias);

        // Generate through the aliased UI path: the worker validates and
        // canonicalizes for Git work, but keys the hub job on the raw
        // path — the same string Cancel will send.
        assert!(
            !dispatch(
                &hub,
                &storage,
                &handle,
                AcpCmd::Generate {
                    agent_id: "opencode".into(),
                    project_id: project_id.clone(),
                    checkout_path: raw_checkout.clone(),
                    checkout_key: key,
                    run: 1,
                    draft_snapshot: "draft".into(),
                },
            )
            .await
        );
        wait_log(home.path(), "prompt").await;

        // The prompt proves Git/cwd work ran against the canonical
        // checkout: the staged demo.txt made it into the agent's input.
        let prompts: Vec<String> = log_events(home.path())
            .iter()
            .filter(|entry| entry["event"].as_str() == Some("prompt"))
            .filter_map(|entry| entry["text"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(prompts.len(), 1);
        assert!(
            prompts[0].contains("demo.txt"),
            "prompt should carry the staged diff: {}",
            prompts[0]
        );

        // Cancel carries the same raw UI path; it must still find the job.
        assert!(
            !dispatch(
                &hub,
                &storage,
                &handle,
                AcpCmd::Cancel {
                    project_id,
                    checkout_path: raw_checkout,
                },
            )
            .await
        );
        wait_log(home.path(), "session_cancel").await;

        // The connection survives the cancelled turn.
        assert_eq!(hub.status()["opencode"].state, AgentState::Connected);
        hub.shutdown().await;
    }

    /// The Settings page's save rides `agents_set` on this runtime: a valid
    /// override is stored and republished, a bad row stores nothing.
    #[tokio::test]
    async fn set_agents_stores_valid_rows_and_rejects_bad_ones() {
        use super::{AcpCmd, dispatch};
        use app_core::acp::AcpHub;
        use app_core::store::Storage;
        use app_core::test_support::TempDir;

        let home = TempDir::new("acp-set-agents");
        let storage = Some(Storage::open(home.path()));
        let hub = AcpHub::new();
        let handle = crate::test_handle();

        let preset_id = app_core::acp::agents_get(storage.as_ref().unwrap())
            .unwrap()
            .first()
            .unwrap()
            .id
            .clone();
        let overrides = vec![app_core::acp::AcpOverride {
            id: preset_id.clone(),
            enabled: true,
            command: "fake-acp --flag".into(),
            prompt: "new prompt".into(),
        }];
        assert!(
            !dispatch(
                &hub,
                &storage,
                &handle,
                AcpCmd::SetAgents {
                    overrides: overrides.clone(),
                },
            )
            .await
        );
        let agents = app_core::acp::agents_get(storage.as_ref().unwrap()).unwrap();
        let saved = agents.iter().find(|a| a.id == preset_id).unwrap();
        assert_eq!(saved.command, "fake-acp --flag");
        assert_eq!(saved.prompt, "new prompt");

        // A row naming no preset rejects the whole call — nothing stores.
        assert!(
            !dispatch(
                &hub,
                &storage,
                &handle,
                AcpCmd::SetAgents {
                    overrides: vec![app_core::acp::AcpOverride {
                        id: "ghost".into(),
                        enabled: true,
                        command: "x".into(),
                        prompt: "y".into(),
                    }],
                },
            )
            .await
        );
        let agents = app_core::acp::agents_get(storage.as_ref().unwrap()).unwrap();
        let saved = agents.iter().find(|a| a.id == preset_id).unwrap();
        assert_eq!(saved.command, "fake-acp --flag", "the earlier save stands");
        hub.shutdown().await;
    }

    /// The Settings page's Reset persists: the stored override is gone and
    /// the preset is back — a local draft alone would let the next store
    /// read recreate the customized row.
    #[tokio::test]
    async fn reset_agent_removes_the_stored_override() {
        use super::{AcpCmd, dispatch};
        use app_core::acp::AcpHub;
        use app_core::store::Storage;
        use app_core::test_support::TempDir;

        let home = TempDir::new("acp-reset-agent");
        let storage = Some(Storage::open(home.path()));
        let hub = AcpHub::new();
        let handle = crate::test_handle();

        let preset_id = app_core::acp::agents_get(storage.as_ref().unwrap())
            .unwrap()
            .first()
            .unwrap()
            .id
            .clone();
        assert!(
            !dispatch(
                &hub,
                &storage,
                &handle,
                AcpCmd::SetAgents {
                    overrides: vec![app_core::acp::AcpOverride {
                        id: preset_id.clone(),
                        enabled: true,
                        command: "fake-acp --flag".into(),
                        prompt: "new prompt".into(),
                    }],
                },
            )
            .await
        );
        let saved = app_core::acp::agents_get(storage.as_ref().unwrap())
            .unwrap()
            .into_iter()
            .find(|a| a.id == preset_id)
            .unwrap();
        assert_eq!(saved.command, "fake-acp --flag");
        assert!(!saved.is_default);

        // Reset restores the preset and stores nothing for that agent.
        assert!(
            !dispatch(
                &hub,
                &storage,
                &handle,
                AcpCmd::ResetAgent {
                    agent_id: preset_id.clone(),
                },
            )
            .await
        );
        let after = app_core::acp::agents_get(storage.as_ref().unwrap())
            .unwrap()
            .into_iter()
            .find(|a| a.id == preset_id)
            .unwrap();
        assert!(after.is_default, "the preset is back: {after:?}");
        assert_ne!(after.command, "fake-acp --flag");
        hub.shutdown().await;
    }
}
