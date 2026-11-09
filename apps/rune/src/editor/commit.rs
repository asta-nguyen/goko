//! Commit panel: the draft message (native `TextArea` — IME, undo and
//! selection come from `TextEdit`), the ACP agent picker with connection
//! status, Generate/Cancel, Commit and Push — pinned under the changed-files
//! list exactly like the desktop `CommitBox`.
//!
//! Enable rules mirror main: Generate needs a *connected* picked agent,
//! staged changes and no conflicts; Commit needs staged changes, no
//! conflicts and a non-blank message; Push stays reachable on a clean tree
//! and is labelled `Push ↑N` / `Push new branch` like the desktop. Git writes
//! ride `enqueue_write`; ACP generate/cancel go through the `acp` module's
//! checkout-scoped wrappers (see the seam note in the module docs of
//! `acp.rs` — the wrappers are added there because `AcpCmd` is private).

use std::collections::BTreeMap;

use app_core::acp::{AcpAgent, AcpStatus, AgentState};
use rune::{El, Key, Tx, col, row};
use rune_kit::{Button, ButtonVariant, ControlSize, Select, TextArea};

use crate::State;
use crate::editor::backend::Req;
use crate::theme::{self, Ink, T11M_LH};

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Agents the picker offers: enabled rows only (desktop filters `enabled`).
pub fn enabled_agents(agents: &[AcpAgent]) -> Vec<&AcpAgent> {
    agents.iter().filter(|a| a.enabled).collect()
}

/// Connection status for `id`, `disconnected` when the hub hasn't reported.
pub fn agent_status(statuses: &BTreeMap<String, AcpStatus>, id: &str) -> AcpStatus {
    statuses.get(id).cloned().unwrap_or(AcpStatus {
        state: AgentState::Disconnected,
        message: None,
    })
}

/// The agent the picker shows as chosen, mirroring `CommitBox`: the stored
/// pick while its agent stays enabled, else the first connected enabled
/// agent, else the first enabled agent found on PATH (`available`).
pub fn picked_agent_id(
    agents: &[AcpAgent],
    statuses: &BTreeMap<String, AcpStatus>,
    stored: Option<&str>,
) -> Option<String> {
    let enabled = enabled_agents(agents);
    if let Some(id) = stored
        && enabled.iter().any(|a| a.id == id)
    {
        return Some(id.to_string());
    }
    enabled
        .iter()
        .find(|a| agent_status(statuses, &a.id).state == AgentState::Connected)
        .or_else(|| enabled.iter().find(|a| a.available))
        .map(|a| a.id.clone())
}

/// Generate enable rule (desktop `canGenerate`).
pub fn can_generate(connected: bool, staged: usize, conflicts: usize, generating: bool) -> bool {
    connected && staged > 0 && conflicts == 0 && !generating
}

/// Commit enable rule (desktop `canCommit`): staged work, no conflicts,
/// non-blank message, no commit in flight.
pub fn can_commit(staged: usize, conflicts: usize, message: &str, committing: bool) -> bool {
    staged > 0 && conflicts == 0 && !message.trim().is_empty() && !committing
}

/// Push enable rule (desktop `canPush`): never while pushing; `ahead == None`
/// means no upstream (push the new branch), `Some(0)` means nothing to push.
pub fn can_push(pushing: bool, ahead: Option<u32>) -> bool {
    !pushing && ahead != Some(0)
}

/// Push label (desktop `pushLabel`).
pub fn push_label(pushing: bool, ahead: Option<u32>) -> String {
    if pushing {
        return "Push".to_string();
    }
    match ahead {
        None => "Push new branch".to_string(),
        Some(n) if n > 0 => format!("Push ↑{n}"),
        _ => "Push".to_string(),
    }
}

/// `(staged, conflicts, ahead)` from the cached changes payload.
pub fn change_counts(s: &State) -> (usize, usize, Option<u32>) {
    match &s.editor.changes.data {
        Some(d) => (d.staged.len(), d.conflicts.len(), d.ahead),
        None => (0, 0, None),
    }
}

// ---------------------------------------------------------------------------
// Actions (handlers get `&mut State`)
// ---------------------------------------------------------------------------

/// Commit the draft: trim like the desktop, keep the draft until the reply
/// (the backend clears `commit.message` only on success). `committing` is
/// armed only when the queue accepts the job — a refusal surfaces the error
/// and leaves the draft untouched.
pub fn commit_now(s: &mut State) {
    let (staged, conflicts, _) = change_counts(s);
    let message = s.editor.commit.message.text.trim().to_string();
    if !can_commit(staged, conflicts, &message, s.editor.commit.committing) {
        return;
    }
    let Some(checkout) = s.editor.checkout.clone() else {
        return;
    };
    let Some(key) = s.editor.checkout_key.clone() else {
        return;
    };
    if !s
        .editor
        .enqueue_write(checkout, key, Req::Commit { message })
    {
        commit_queue_full(s);
        return;
    }
    s.editor.commit.committing = true;
    s.editor.commit.error = None;
}

/// Push the current branch (hooks run server-side; no force, no prompts —
/// that is `git_write::push`'s contract). Same refused-queue contract as
/// [`commit_now`].
pub fn push_now(s: &mut State) {
    let (_, _, ahead) = change_counts(s);
    if !can_push(s.editor.commit.pushing, ahead) {
        return;
    }
    let Some(checkout) = s.editor.checkout.clone() else {
        return;
    };
    let Some(key) = s.editor.checkout_key.clone() else {
        return;
    };
    if !s.editor.enqueue_write(checkout, key, Req::Push) {
        commit_queue_full(s);
        return;
    }
    s.editor.commit.pushing = true;
    s.editor.commit.error = None;
}

/// Queue-refusal surfacing for commit/push, mirroring `apply.rs`'s
/// write-error arm: the panel error line plus the global notice. The draft
/// message is never touched here.
fn commit_queue_full(s: &mut State) {
    let text = "Backend queue is full — try again".to_string();
    s.editor.commit.error = Some(text.clone());
    s.editor.notice = Some(crate::editor::state::Notice { text, stale: false });
}

/// Start ACP generation for the current checkout+draft. The runtime-scoped
/// wrapper lives in `acp.rs` (it owns `AcpCmd`); it snapshots the draft and
/// only fills it if unchanged on completion.
pub fn generate_now(s: &mut State) {
    let _ = crate::editor::acp::generate(s);
}

/// Cancel the in-flight generation for this checkout.
pub fn cancel_generate(s: &mut State) {
    crate::editor::acp::cancel(s);
}

/// Connect the picked agent (existing `Acp::connect` — no borrow seam).
pub fn connect_agent(s: &mut State, agent_id: &str) {
    s.editor.acp.connect(agent_id);
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// The commit panel: message draft, agent picker + Connect, Generate/Cancel,
/// Commit, Push. Render-only (`&State`); every mutation happens in handlers.
pub fn view(s: &State) -> El {
    let ink = Ink::new();
    let k = &s.editor.kit;
    let (staged, conflicts, ahead) = change_counts(s);
    let message = s.editor.commit.message.text.clone();
    let generating = s
        .editor
        .acp
        .generating_for
        .as_deref()
        .is_some_and(|key| Some(key) == s.editor.checkout_key.as_deref());
    let committing = s.editor.commit.committing;
    let pushing = s.editor.commit.pushing;

    let agents: Vec<AcpAgent> = enabled_agents(&s.editor.acp.agents)
        .into_iter()
        .cloned()
        .collect();
    let picked = picked_agent_id(
        &s.editor.acp.agents,
        &s.editor.acp.statuses,
        s.editor.commit.agent_id.as_deref(),
    );
    let picked_status = picked
        .as_deref()
        .map(|id| agent_status(&s.editor.acp.statuses, id));
    let picked_connected = picked_status
        .as_ref()
        .is_some_and(|st| st.state == AgentState::Connected);

    let mut panel = col()
        .w_full()
        .px(10.0)
        .py(8.0)
        .gap(8.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], ink.border);

    // -- message draft -------------------------------------------------------
    panel = panel.child(
        TextArea::new(&s.editor.commit.message)
            .placeholder("Commit message")
            .rows(3)
            .spellcheck(false)
            .on_edit(move |st: &mut State| -> &mut rune::edit::TextEdit {
                &mut st.editor.commit.message
            })
            .on_key(
                move |st: &mut State, ev: &rune::KeyEvent, _tx: &mut Tx| -> bool {
                    if (ev.mods.cmd || ev.mods.ctrl) && matches!(ev.key, Key::Enter) {
                        commit_now(st);
                        return true;
                    }
                    false
                },
            )
            .build(k),
    );

    // -- error line ----------------------------------------------------------
    let error = s
        .editor
        .commit
        .error
        .clone()
        .or_else(|| s.editor.commit.generate_error.clone())
        .or_else(|| s.editor.acp.error.clone());
    if let Some(err) = error {
        panel = panel.child(theme::txt(&err, T11M_LH, ink.danger));
    }

    // -- agent picker + connect ----------------------------------------------
    let mut agent_row = row().w_full().gap(6.0).items_center();
    let label_refs: Vec<&str> = agents.iter().map(|a| a.label.as_str()).collect();
    // Each option carries its live connection state as secondary text — the
    // desktop renders `StatusDot` + the state string inside every item. An
    // agent missing from PATH is marked too: the kit Select has no per-option
    // disable, so `on_choose` rejects it visibly instead.
    let details: Vec<Option<String>> = agents
        .iter()
        .map(|a| {
            let state = agent_state_label(agent_status(&s.editor.acp.statuses, &a.id).state);
            Some(if a.available {
                state.to_string()
            } else {
                format!("{state} · not on PATH")
            })
        })
        .collect();
    let picked_label = picked
        .as_deref()
        .and_then(|id| agents.iter().find(|a| a.id == id).map(|a| a.label.clone()));
    let mut layer = Vec::new();
    // (label, id, available) triples: the picker's options are labels, the
    // stored pick is the id, and `available` gates the choose — same
    // label→entity mapping the project/checkout pickers use.
    let agent_map: Vec<(String, String, bool)> = agents
        .iter()
        .map(|a| (a.label.clone(), a.id.clone(), a.available))
        .collect();
    let select = Select::new("commit-agent", &label_refs)
        .current(picked_label.as_deref())
        .placeholder("No ACP agent")
        .label("Agent")
        // Fixed width (the kit Select can't flex): sized so Select + the
        // widest Connect label still fit the ~280 px aside at 760 px.
        .width(160.0)
        .details(&details)
        .open(s.editor.commit.agent_picker_open)
        .on_toggle(|st: &mut State| {
            st.editor.commit.agent_picker_open = !st.editor.commit.agent_picker_open;
            if st.editor.commit.agent_picker_open {
                st.editor.project_picker_open = false;
                st.editor.checkout_picker_open = false;
            }
        })
        .on_close(|st: &mut State| {
            st.editor.commit.agent_picker_open = false;
        })
        .on_choose(move |st: &mut State, label: &str, _tx: &mut Tx| {
            let Some((_, id, available)) = agent_map.iter().find(|(l, _, _)| l == label) else {
                return;
            };
            let (id, available) = (id.clone(), *available);
            if !available {
                // Desktop disables the item ("was not found on PATH"); the
                // kit Select can't, so reject the pick with the same
                // visible detail and keep the picker open.
                st.editor.acp.error = Some(format!("{label} was not found on PATH"));
                return;
            }
            st.editor.commit.agent_picker_open = false;
            crate::editor::acp::choose_agent(st, &id);
        });
    agent_row = agent_row.child(select.build(k, &mut layer));

    // Connect shows for every non-connected state (desktop `showConnect`);
    // the picked agent's state itself lives in the picker details.
    if let Some(status) = picked_status.as_ref()
        && status.state != AgentState::Connected
    {
        let agent_id = picked.clone().unwrap_or_default();
        let connecting = status.state == AgentState::Connecting;
        agent_row = agent_row.child(
            Button::new(if connecting {
                "Connecting…"
            } else {
                "Connect"
            })
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .disabled(connecting)
            .on_press(move |st: &mut State| {
                let still_connecting = st
                    .editor
                    .acp
                    .statuses
                    .get(&agent_id)
                    .is_some_and(|s| s.state == AgentState::Connecting);
                if !still_connecting {
                    connect_agent(st, &agent_id);
                }
            })
            .build(k),
        );
    }
    panel = panel.child(agent_row);
    // Select popups/overlays collect into `layer`; mount them on the panel.
    for extra in layer.drain(..) {
        panel = panel.child(extra);
    }

    if agents.is_empty() {
        panel = panel.child(theme::txt(
            "Set up an ACP agent in Settings",
            T11M_LH,
            ink.muted,
        ));
    }

    // -- generate / commit / push --------------------------------------------
    // Desktop packs these `size="sm"` left-to-right with no spacer; Small is
    // also what keeps "Push new branch" inside the ~280 px aside.
    let mut actions = row().w_full().gap(6.0).items_center();
    if generating {
        actions = actions.child(
            Button::new("Cancel")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .on_press(|st: &mut State| cancel_generate(st))
                .build(k),
        );
        let asking = picked_label.clone().unwrap_or_else(|| "agent".to_string());
        actions = actions.child(
            theme::txt(&format!("Asking {asking}…"), T11M_LH, ink.muted)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        );
    } else {
        actions = actions.child(
            Button::new("Generate")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .disabled(!can_generate(
                    picked_connected,
                    staged,
                    conflicts,
                    generating,
                ))
                .on_press(|st: &mut State| generate_now(st))
                .build(k),
        );
    }
    actions = actions.child(
        Button::new("Commit")
            .variant(ButtonVariant::Submit)
            .control_size(ControlSize::Small)
            .disabled(!can_commit(staged, conflicts, &message, committing))
            .on_press(|st: &mut State| commit_now(st))
            .build(k),
    );
    actions = actions.child(
        Button::new(push_label(pushing, ahead))
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .disabled(!can_push(pushing, ahead))
            .on_press(|st: &mut State| push_now(st))
            .build(k),
    );
    panel = panel.child(actions);
    panel
}

/// Lowercase connection-state label — the same string the desktop renders
/// inside each agent `SelectItem` (shown here as the option's detail).
fn agent_state_label(state: AgentState) -> &'static str {
    match state {
        AgentState::Disconnected => "disconnected",
        AgentState::Connecting => "connecting",
        AgentState::Connected => "connected",
        AgentState::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, enabled: bool, available: bool) -> AcpAgent {
        AcpAgent {
            id: id.into(),
            label: format!("Agent {id}"),
            enabled,
            command: "cmd".into(),
            prompt: String::new(),
            available,
            is_default: false,
        }
    }

    fn statuses(entries: &[(&str, AgentState)]) -> BTreeMap<String, AcpStatus> {
        entries
            .iter()
            .map(|(id, st)| {
                (
                    id.to_string(),
                    AcpStatus {
                        state: *st,
                        message: None,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn picked_prefers_stored_then_connected_then_available() {
        let agents = vec![
            agent("a", true, true),
            agent("b", true, true),
            agent("c", true, false),
            agent("off", false, true),
        ];
        // Stored pick wins while enabled.
        let st = statuses(&[
            ("a", AgentState::Disconnected),
            ("b", AgentState::Connected),
        ]);
        assert_eq!(
            picked_agent_id(&agents, &st, Some("a")).as_deref(),
            Some("a")
        );
        // Stored disabled/missing → first connected enabled.
        assert_eq!(
            picked_agent_id(&agents, &st, Some("off")).as_deref(),
            Some("b")
        );
        assert_eq!(
            picked_agent_id(&agents, &st, Some("gone")).as_deref(),
            Some("b")
        );
        // Nothing connected → first enabled+available.
        let st2 = statuses(&[]);
        assert_eq!(picked_agent_id(&agents, &st2, None).as_deref(), Some("a"));
        // Disabled agents never picked.
        assert!(picked_agent_id(&[agent("x", false, true)], &st2, None).is_none());
    }

    #[test]
    fn enable_rules_match_desktop() {
        assert!(can_generate(true, 1, 0, false));
        assert!(!can_generate(true, 0, 0, false)); // nothing staged
        assert!(!can_generate(true, 1, 1, false)); // conflicts
        assert!(!can_generate(false, 1, 0, false)); // not connected
        assert!(!can_generate(true, 1, 0, true)); // already generating

        assert!(can_commit(1, 0, "msg", false));
        assert!(!can_commit(1, 0, "   ", false)); // blank draft
        assert!(!can_commit(0, 0, "msg", false));
        assert!(!can_commit(1, 1, "msg", false));
        assert!(!can_commit(1, 0, "msg", true));

        assert!(can_push(false, None)); // no upstream → push new branch
        assert!(can_push(false, Some(3)));
        assert!(!can_push(false, Some(0)));
        assert!(!can_push(true, Some(3)));
    }

    #[test]
    fn push_labels_match_desktop() {
        assert_eq!(push_label(false, None), "Push new branch");
        assert_eq!(push_label(false, Some(2)), "Push ↑2");
        assert_eq!(push_label(true, Some(2)), "Push");
        assert_eq!(push_label(false, Some(0)), "Push");
    }

    #[test]
    fn agent_status_defaults_to_disconnected() {
        let st = BTreeMap::new();
        assert_eq!(agent_status(&st, "nope").state, AgentState::Disconnected);
    }

    // -- headless runtime: the agent picker's real press/choose/close ------

    use rune::{Cx, Env, Key, KeyEvent, Mods, Runtime};

    fn picker_view(s: &State, _cx: &mut Cx<State>) -> El {
        view(s)
    }

    /// Restores `GOKO_HOME` on drop so the throwaway home never leaks into
    /// parallel tests.
    struct HomeGuard(Option<std::ffi::OsString>);

    impl HomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var_os("GOKO_HOME");
            // SAFETY: test-only; workers resolve their home once at spawn and
            // the pointed-at directory outlives this test.
            unsafe { std::env::set_var("GOKO_HOME", path) };
            Self(prev)
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: same test-only env window as `set`.
            unsafe {
                match &self.0 {
                    Some(v) => std::env::set_var("GOKO_HOME", v),
                    None => std::env::remove_var("GOKO_HOME"),
                }
            }
        }
    }

    fn element_center(rt: &Runtime<State>, key: &str) -> (f32, f32) {
        let e = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("element {key} missing"));
        let r = e.rect;
        (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
    }

    fn click(rt: &mut Runtime<State>, x: f32, y: f32) {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn option_elements(rt: &Runtime<State>) -> Vec<rune::runtime::ElementInfo> {
        rt.elements()
            .into_iter()
            .filter(|e| e.role == "option")
            .collect()
    }

    #[test]
    fn agent_picker_opens_lists_and_closes() {
        let home = app_core::test_support::TempDir::new("agent-picker");
        let _guard = HomeGuard::set(home.path());
        let mut s = State::new(crate::test_handle());
        s.editor.acp.agents = vec![agent("a", true, true), agent("b", true, true)];
        let mut rt = Runtime::new(s, picker_view, (800.0, 600.0), Env::new(true, 2.0));
        rt.frame(0.0);

        // Closed: the trigger is mounted but no listbox options exist.
        assert!(
            rt.elements()
                .iter()
                .any(|e| e.key.as_deref() == Some("commit-agent-trigger"))
        );
        assert!(option_elements(&rt).is_empty(), "picker starts closed");

        // Press the trigger: the controlled open flag flips and options mount.
        let (x, y) = element_center(&rt, "commit-agent-trigger");
        click(&mut rt, x, y);
        assert!(
            rt.state.editor.commit.agent_picker_open,
            "trigger press opens the picker"
        );
        let opts = option_elements(&rt);
        assert_eq!(opts.len(), 2, "both enabled agents are listed: {opts:?}");

        // Choosing the second option stores its id and closes the picker.
        let b = opts
            .iter()
            .find(|e| e.key.as_deref() == Some("option1"))
            .expect("option1 mounted");
        let r = b.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        assert_eq!(rt.state.editor.commit.agent_id.as_deref(), Some("b"));
        assert!(!rt.state.editor.commit.agent_picker_open);
        assert!(
            option_elements(&rt).is_empty(),
            "options unmount after choose"
        );

        // Reopen, then Escape closes without changing the pick.
        let (x, y) = element_center(&rt, "commit-agent-trigger");
        click(&mut rt, x, y);
        assert!(rt.state.editor.commit.agent_picker_open);
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.editor.commit.agent_picker_open, "Escape closes");
        assert_eq!(
            rt.state.editor.commit.agent_id.as_deref(),
            Some("b"),
            "Escape keeps the stored pick"
        );

        // Reopen, then a press outside the listbox closes it too.
        let (x, y) = element_center(&rt, "commit-agent-trigger");
        click(&mut rt, x, y);
        assert!(rt.state.editor.commit.agent_picker_open);
        click(&mut rt, 4.0, 4.0);
        assert!(
            !rt.state.editor.commit.agent_picker_open,
            "outside press closes"
        );
    }

    // -- refused-queue regressions: committing/pushing must never stick ---
    //
    // `project_id` stays `None`, so `enqueue_write` returns `false` — the
    // same contract a full or disconnected queue gives. No reply will ever
    // arrive; the caller must settle its own flag and keep the draft.

    fn write_scene() -> (State, app_core::test_support::TempDir, HomeGuard) {
        let home = app_core::test_support::TempDir::new("commit-queue");
        let guard = HomeGuard::set(home.path());
        let mut s = State::new(crate::test_handle());
        let co = std::path::PathBuf::from("/tmp/goko-qa-checkout");
        s.editor.checkout = Some(co.clone());
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s.editor.changes.data = Some(app_core::git_view::GitViewChanges {
            branch: None,
            head: String::new(),
            conflicts: vec![],
            staged: vec![app_core::git_view::StatusEntry {
                path: "a".into(),
                orig_path: None,
                status: "modified",
                added: None,
                removed: None,
            }],
            unstaged: vec![],
            untracked: vec![],
            ahead: None,
        });
        s.editor.commit.message = rune::edit::TextEdit::new("wip");
        (s, home, guard)
    }

    #[test]
    fn rejected_commit_keeps_flag_clear_and_draft() {
        let (mut s, _home, _guard) = write_scene();
        commit_now(&mut s);
        assert!(!s.editor.commit.committing, "refusal never arms committing");
        assert_eq!(
            s.editor.commit.message.text, "wip",
            "draft survives the rejection"
        );
        assert!(s.editor.commit.error.is_some(), "refusal surfaces an error");
        assert!(s.editor.notice.is_some(), "refusal raises a notice");
    }

    #[test]
    fn rejected_push_keeps_flag_clear() {
        let (mut s, _home, _guard) = write_scene();
        push_now(&mut s);
        assert!(!s.editor.commit.pushing, "refusal never arms pushing");
        assert!(s.editor.commit.error.is_some());
        assert!(s.editor.notice.is_some());
    }

    #[test]
    fn accepted_commit_arms_committing() {
        let (mut s, _home, _guard) = write_scene();
        s.editor.project_id = Some("p1".into());
        commit_now(&mut s);
        assert!(s.editor.commit.committing);
        assert!(s.editor.commit.error.is_none());
    }

    // -- narrow-aside overflow: every footer control stays inside ---------

    fn commit_only(s: &State, _cx: &mut Cx<State>) -> El {
        view(s)
    }

    /// The 760 px viewport leaves the aside ~280 px wide (panel padding
    /// included). With the longest labels — "Connecting…"/"Push new branch" —
    /// no footer control may cross the panel's right edge.
    #[test]
    fn commit_actions_fit_narrow_aside() {
        let (mut s, _home, _guard) = write_scene();
        s.editor.acp.agents = vec![agent("a", true, true)];
        s.editor.acp.statuses.insert(
            "a".into(),
            AcpStatus {
                state: AgentState::Connecting,
                message: None,
            },
        );
        let mut rt = Runtime::new(s, commit_only, (280.0, 400.0), Env::new(true, 2.0));
        rt.frame(0.0);

        for prefix in ["Connecting", "Generate", "Commit", "Push"] {
            let e = rt
                .elements()
                .into_iter()
                .find(|e| {
                    e.role == "button" && e.label.as_deref().is_some_and(|l| l.starts_with(prefix))
                })
                .unwrap_or_else(|| panic!("{prefix} button missing"));
            let r = e.rect;
            assert!(
                r[0] >= -0.5 && r[0] + r[2] <= 280.5,
                "{prefix} rect {r:?} escapes the 280 px aside"
            );
        }
        // The agent Select trigger is the other wide control in the footer.
        let sel = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("commit-agent-trigger"))
            .expect("agent trigger mounted");
        assert!(
            sel.rect[0] + sel.rect[2] <= 280.5,
            "agent select overflows: {:?}",
            sel.rect
        );
    }
}
