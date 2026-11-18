//! Commit panel: the draft message (native `TextArea` — IME, undo and
//! selection come from `TextEdit`), the ACP agent picker with connection
//! status, Generate/Cancel, Commit and Push — pinned under the changed-files
//! list exactly like the desktop `CommitBox`.
//!
//! Enable rules mirror main: Generate needs a *connected* picked agent,
//! staged changes and no conflicts; Commit needs staged changes, no
//! conflicts and a non-blank message — while a merge, rebase, cherry-pick
//! or revert is in progress it reads Continue instead (a merge keeps the
//! message rule but not the staged one; the rest need only a clean
//! conflict list); Push stays reachable on a clean tree and is labelled
//! `Push ↑N` / `Push new branch` like the desktop. Git writes ride
//! `enqueue_write`; ACP generate/cancel go through the `acp` module's
//! checkout-scoped wrappers (see the seam note in the module docs of
//! `acp.rs` — the wrappers are added there because `AcpCmd` is private).

use std::collections::BTreeMap;

use app_core::acp::{AcpAgent, AcpStatus, AgentState};
use app_core::git_view::GitViewChanges;
use rune::{El, Key, Tx, col, row};
use rune_kit::{Button, ButtonVariant, ControlSize, TextArea};

use crate::State;
use crate::editor::backend::Req;
use crate::editor::state::CommitState;
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
/// non-blank message, no commit in flight. While `operation` is in
/// progress the button is its Continue: a merge keeps the message rule
/// but not the staged one (Git lets the merge commit stand alone); a
/// rebase, cherry-pick or revert needs only a clean conflict list.
pub fn can_commit(
    staged: usize,
    conflicts: usize,
    message: &str,
    committing: bool,
    operation: Option<&str>,
) -> bool {
    match operation {
        Some("merge") => conflicts == 0 && !message.trim().is_empty() && !committing,
        Some(_) => conflicts == 0 && !committing,
        None => staged > 0 && conflicts == 0 && !message.trim().is_empty() && !committing,
    }
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

/// Offer Git's own merge message once per merge: only into an untouched
/// draft and only while `merge_prefilled` is unset — a draft the user
/// cleared stays cleared for that merge. The flag resets when no merge is
/// in progress, so the next merge offers again.
pub fn prefill_merge_message(commit: &mut CommitState, data: &GitViewChanges) {
    if data.operation != Some("merge") {
        commit.merge_prefilled = false;
        return;
    }
    let Some(message) = &data.merge_message else {
        return;
    };
    if commit.merge_prefilled || !commit.message.text.trim().is_empty() {
        return;
    }
    commit.message = rune::edit::TextEdit::new(message.clone());
    commit.merge_prefilled = true;
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
    let operation = s.editor.changes.data.as_ref().and_then(|d| d.operation);
    let message = s.editor.commit.message.text.trim().to_string();
    if s.editor.changes.busy.is_some()
        || !can_commit(
            staged,
            conflicts,
            &message,
            s.editor.commit.committing,
            operation,
        )
    {
        return;
    }
    // A rebase/cherry-pick/revert Continue rides the same serialized write
    // lane as Resolve — `changes.busy` holds until the reply. The draft
    // and `committing` belong to the Commit path alone.
    if let Some(op) = operation
        && op != "merge"
    {
        crate::editor::changes::write_paths(
            &mut s.editor,
            "continue",
            Req::Operation {
                step: "continue".into(),
            },
        );
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
    // While an operation is in progress the primary button is its Continue;
    // a write on the changes lane disables it like it does the row actions.
    let operation = s.editor.changes.data.as_ref().and_then(|d| d.operation);
    let write_busy = s.editor.changes.busy.is_some();

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
    // Each option carries its live connection state as secondary text — the
    // desktop renders `StatusDot` + the state string inside every item. An
    // agent missing from PATH is listed disabled, saying so.
    let options: Vec<rune_kit::ComboboxOption> = agents
        .iter()
        .map(|a| {
            let state = agent_state_label(agent_status(&s.editor.acp.statuses, &a.id).state);
            let mut option = rune_kit::ComboboxOption::new(&a.id, &a.label);
            option.detail = if a.available {
                state.to_string()
            } else {
                format!("{state} · not on PATH")
            };
            option.disabled = !a.available;
            option
        })
        .collect();
    let picked_label = picked
        .as_deref()
        .and_then(|id| agents.iter().find(|a| a.id == id).map(|a| a.label.clone()));
    let mut layer = Vec::new();
    let select = rune_kit::Combobox::new(
        "commit-agent",
        &s.editor.commit.agent_picker,
        &options,
        |st: &mut State| &mut st.editor.commit.agent_picker,
    )
    .current(picked.as_deref())
    .placeholder("No ACP agent")
    .label("Agent")
    .search_placeholder("Search agents")
    // Fixed width: sized so the select + the widest Connect label still fit
    // the ~280 px aside at 760 px; the open list is wider.
    .width(160.0)
    .popover_width(crate::picker::list_width(160.0))
    .on_choose(|st: &mut State, id: &str, _tx: &mut Tx| {
        crate::editor::acp::choose_agent(st, id);
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
                .truncate_middle()
                .help_when_truncated(asking),
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
        Button::new(if operation.is_some() {
            "Continue"
        } else {
            "Commit"
        })
        .variant(ButtonVariant::Submit)
        .control_size(ControlSize::Small)
        .disabled(write_busy || !can_commit(staged, conflicts, &message, committing, operation))
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

        assert!(can_commit(1, 0, "msg", false, None));
        assert!(!can_commit(1, 0, "   ", false, None)); // blank draft
        assert!(!can_commit(0, 0, "msg", false, None));
        assert!(!can_commit(1, 1, "msg", false, None));
        assert!(!can_commit(1, 0, "msg", true, None));

        assert!(can_push(false, None)); // no upstream → push new branch
        assert!(can_push(false, Some(3)));
        assert!(!can_push(false, Some(0)));
        assert!(!can_push(true, Some(3)));
    }

    /// The operation decides the Commit gate: a merge keeps the message
    /// rule but not the staged one; the rest need only a clean conflict
    /// list.
    #[test]
    fn can_commit_follows_the_operation() {
        // No operation: the unchanged rule.
        assert!(!can_commit(0, 0, "msg", false, None));
        assert!(!can_commit(1, 0, "   ", false, None));
        // Merge: nothing staged is fine, the message is still required.
        assert!(can_commit(0, 0, "msg", false, Some("merge")));
        assert!(!can_commit(0, 0, "   ", false, Some("merge")));
        // Rebase/cherry-pick/revert: the message is Git's own — blank is fine.
        assert!(can_commit(0, 0, "   ", false, Some("rebase")));
        assert!(can_commit(0, 0, "", false, Some("cherry-pick")));
        assert!(can_commit(0, 0, "", false, Some("revert")));
        // Any conflict or a commit in flight blocks every shape.
        assert!(!can_commit(0, 1, "msg", false, Some("merge")));
        assert!(!can_commit(0, 1, "", false, Some("rebase")));
        assert!(!can_commit(1, 1, "msg", false, None));
        assert!(!can_commit(0, 0, "msg", true, Some("merge")));
        assert!(!can_commit(0, 0, "", true, Some("rebase")));
        assert!(!can_commit(1, 0, "msg", true, None));
    }

    /// The merge draft is offered once: an empty draft takes `MERGE_MSG`, a
    /// cleared draft is not refilled, a typed draft is never replaced, and
    /// the memory resets when the operation ends.
    #[test]
    fn merge_message_is_prefilled_once() {
        let (mut s, _home, _guard) = write_scene();
        s.editor.commit.message = rune::edit::TextEdit::new("");
        let reply =
            |s: &mut State, operation: Option<&'static str>, merge_message: Option<&str>| {
                s.editor.apply_reply(crate::editor::backend::Reply {
                    req: Req::Changes,
                    generation: 0,
                    checkout_key: s.editor.checkout_key.clone(),
                    result: Ok(crate::editor::backend::ReplyData::Changes(
                        app_core::git_view::GitViewChanges {
                            branch: None,
                            head: String::new(),
                            conflicts: vec![],
                            staged: vec![],
                            unstaged: vec![],
                            untracked: vec![],
                            ahead: None,
                            operation,
                            merge_message: merge_message.map(Into::into),
                        },
                    )),
                });
            };

        reply(&mut s, Some("merge"), Some("Merge branch 'x'"));
        assert_eq!(s.editor.commit.message.text, "Merge branch 'x'");
        assert!(s.editor.commit.merge_prefilled);

        // A draft the user cleared during the same merge is not refilled.
        s.editor.commit.message = rune::edit::TextEdit::new("");
        reply(&mut s, Some("merge"), Some("Merge branch 'x'"));
        assert!(s.editor.commit.message.text.is_empty());
        assert!(s.editor.commit.merge_prefilled);

        // The flag resets once no merge is in progress.
        reply(&mut s, None, None);
        assert!(!s.editor.commit.merge_prefilled);

        // A typed draft is never replaced (and never arms the flag either).
        s.editor.commit.message = rune::edit::TextEdit::new("typed");
        reply(&mut s, Some("merge"), Some("Merge branch 'y'"));
        assert_eq!(s.editor.commit.message.text, "typed");
        assert!(!s.editor.commit.merge_prefilled);

        // A later merge offers the message again into an empty draft.
        s.editor.commit.message = rune::edit::TextEdit::new("");
        reply(&mut s, Some("merge"), Some("Merge branch 'z'"));
        assert_eq!(s.editor.commit.message.text, "Merge branch 'z'");
        assert!(s.editor.commit.merge_prefilled);
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
            rt.state.editor.commit.agent_picker.open,
            "trigger press opens the picker"
        );
        // The list animates open before its rows take presses.
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let opts = option_elements(&rt);
        assert_eq!(opts.len(), 2, "both enabled agents are listed: {opts:?}");

        // Choosing the second option stores its id and closes the picker.
        let b = opts
            .iter()
            .find(|e| e.key.as_deref() == Some("commit-agent-option-b"))
            .expect("agent b's option mounted");
        let r = b.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        assert_eq!(rt.state.editor.commit.agent_id.as_deref(), Some("b"));
        assert!(!rt.state.editor.commit.agent_picker.open);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert!(
            option_elements(&rt).is_empty(),
            "options unmount after choose"
        );

        // Reopen, then Escape closes without changing the pick.
        let (x, y) = element_center(&rt, "commit-agent-trigger");
        click(&mut rt, x, y);
        assert!(rt.state.editor.commit.agent_picker.open);
        rt.key(KeyEvent {
            key: Key::Escape,
            mods: Mods::default(),
        });
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.editor.commit.agent_picker.open, "Escape closes");
        assert_eq!(
            rt.state.editor.commit.agent_id.as_deref(),
            Some("b"),
            "Escape keeps the stored pick"
        );

        // Reopen, then a press outside the listbox closes it too.
        let (x, y) = element_center(&rt, "commit-agent-trigger");
        click(&mut rt, x, y);
        assert!(rt.state.editor.commit.agent_picker.open);
        click(&mut rt, 4.0, 4.0);
        assert!(
            !rt.state.editor.commit.agent_picker.open,
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
            operation: None,
            merge_message: None,
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

    /// While an operation is in progress the primary button reads Continue
    /// and an in-flight changes write disables it; without one it reads
    /// Commit again.
    #[test]
    fn primary_button_reads_continue_during_an_operation() {
        let (mut s, _home, _guard) = write_scene();
        let (backend, jobs) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        s.editor.project_id = Some("p1".into());
        s.editor.changes.data.as_mut().unwrap().operation = Some("rebase");
        let mut rt = Runtime::new(s, commit_only, (400.0, 500.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let button = |rt: &Runtime<State>, label: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.role == "button" && e.label.as_deref() == Some(label))
        };
        assert!(button(&rt, "Continue").is_some(), "operation → Continue");
        assert!(button(&rt, "Commit").is_none());

        // A write in flight disables the button: a disabled kit button
        // carries no press handler, so the click reaches nothing.
        rt.state.editor.changes.busy = Some("resolve");
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        let b = button(&rt, "Continue").expect("Continue mounted");
        click(
            &mut rt,
            b.rect[0] + b.rect[2] / 2.0,
            b.rect[1] + b.rect[3] / 2.0,
        );
        assert!(
            jobs.try_iter().next().is_none(),
            "busy Continue runs nothing"
        );
        assert!(!rt.state.editor.commit.committing);

        // The label flips back the moment the operation ends.
        rt.state.editor.changes.busy = None;
        rt.state.editor.changes.data.as_mut().unwrap().operation = None;
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert!(button(&rt, "Commit").is_some());
        assert!(button(&rt, "Continue").is_none());
    }

    /// Continue on a rebase/cherry-pick/revert enqueues `Req::Operation` on
    /// the changes write lane; on a merge it stays the ordinary Commit.
    #[test]
    fn continue_runs_the_operation_for_a_rebase_and_commits_for_a_merge() {
        let (mut s, _home, _guard) = write_scene();
        let (backend, jobs) = crate::editor::backend::channel_backend();
        s.editor.backend = backend;
        s.editor.project_id = Some("p1".into());

        s.editor.changes.data.as_mut().unwrap().operation = Some("rebase");
        commit_now(&mut s);
        assert!(
            jobs.try_iter()
                .any(|j| matches!(&j.req, Req::Operation { step } if step == "continue")),
            "rebase Continue enqueues the operation"
        );
        assert_eq!(s.editor.changes.busy, Some("continue"));
        assert!(!s.editor.commit.committing);
        assert_eq!(s.editor.commit.message.text, "wip", "draft untouched");

        // A merge Continue is the ordinary Commit — nothing staged is fine,
        // the message is still required. (No worker here: clear the lane
        // the reply would clear.)
        s.editor.changes.busy = None;
        let d = s.editor.changes.data.as_mut().unwrap();
        d.operation = Some("merge");
        d.staged = vec![];
        commit_now(&mut s);
        assert!(
            jobs.try_iter()
                .any(|j| matches!(&j.req, Req::Commit { message } if message == "wip")),
            "merge Continue enqueues the commit"
        );
        assert!(s.editor.commit.committing);
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
