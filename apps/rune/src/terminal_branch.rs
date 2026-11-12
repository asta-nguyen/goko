//! Git identity of the focused terminal; at most one background read at a time.

use std::{path::Path, sync::mpsc, time::Duration};

use rune::{El, Tx, row};

use crate::{
    State, View,
    editor::branch,
    panes,
    theme::{self, Ink},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    session: String,
    cwd: String,
}

fn target(s: &State) -> Option<Target> {
    if s.view != View::Terminal {
        return None;
    }
    let tab = s.open_tabs.get(s.active_tab)?;
    let pane = panes::pane_by_id(&tab.layout, &tab.active_pane)?;
    let session = s.session_by_key(&pane.session)?;
    (!session.info.cwd.is_empty()).then(|| Target {
        session: pane.session.clone(),
        cwd: session.info.cwd.clone(),
    })
}

#[derive(Default)]
pub struct BranchState {
    target: Option<Target>,
    data: Option<branch::Branch>,
    checked_at: Option<f64>,
    pending: Option<(Target, mpsc::Receiver<Option<branch::Branch>>)>,
}

impl BranchState {
    fn reconcile(&mut self, target: Option<Target>) -> bool {
        if self.target == target {
            return false;
        }
        self.target = target;
        self.data = None;
        self.checked_at = None;
        true
    }

    fn drain(&mut self) -> bool {
        let Some((target, rx)) = &self.pending else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(data) => data,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        if self.target.as_ref() == Some(target) {
            self.data = result;
        }
        self.pending = None;
        true
    }
}

pub fn poll(s: &mut State, now: f64, tx: &mut Tx) -> bool {
    let target = target(s);
    let state = &mut s.terminal_branch;
    let mut changed = state.reconcile(target.clone());
    changed |= state.drain();
    if state.pending.is_some() {
        tx.wake_after(Duration::from_millis(50));
        return changed;
    }
    let Some(target) = target else { return changed };
    let remaining = state
        .checked_at
        .map_or(0.0, |at| (3.0 - (now - at)).max(0.0));
    if remaining > 0.0 {
        tx.wake_after(Duration::from_secs_f64(remaining));
        return changed;
    }
    let (send, recv) = mpsc::channel();
    let cwd = target.cwd.clone();
    state.checked_at = Some(now);
    state.pending = Some((target, recv));
    std::thread::spawn(move || {
        let _ = send.send(branch::read(Path::new(&cwd)).ok());
    });
    tx.wake_after(Duration::from_millis(50));
    true
}

pub fn view(s: &State, ink: &Ink) -> Option<El> {
    let target = target(s)?;
    let state = &s.terminal_branch;
    if state.target.as_ref() != Some(&target) {
        return None;
    }
    let data = state.data.as_ref()?;
    let label = data
        .name
        .clone()
        .unwrap_or_else(|| format!("Detached · {}", data.head));
    Some(
        row()
            .key("terminal-current-branch")
            .role("group")
            .label(format!("Current branch: {label}"))
            .help(format!("Current branch: {label}\n{}", target.cwd))
            .h(26.0)
            .px(8.0)
            .gap(6.0)
            .items_center()
            .min_w(0.0)
            .max_w(200.0)
            .shrink()
            .radius(6.0)
            .fill(ink.accent.alpha(0.08))
            .border(1.0, ink.accent.alpha(0.25))
            .child(theme::ic(&theme::GIT_BRANCH, 13.0, ink.accent))
            .child(
                theme::txt(&label, theme::T12M, ink.accent)
                    .min_w(0.0)
                    .truncate_middle(),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(s: &mut State, cwd: &str) -> String {
        let id = protocol::session::SessionId::new();
        let key = crate::data::SessionInfo::key(id);
        s.upsert_session(protocol::session::SessionInfo {
            id,
            title: "shell".into(),
            argv: vec![],
            cwd: cwd.into(),
            state: protocol::session::SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        });
        key
    }

    #[test]
    fn header_tracks_focused_session_cwd_and_hides_outside_terminal() {
        let mut s = State::new(crate::test_handle());
        let a = session(&mut s, "/repo/a");
        let b = session(&mut s, "/repo/b");
        s.focus_session_tab(&a);
        s.terminal_branch.target = target(&s);
        s.terminal_branch.data = Some(branch::Branch {
            name: None,
            head: "12345678".into(),
        });
        s.terminal_branch.checked_at = Some(10.0);
        let mut rt = rune::Runtime::new(
            s,
            |s, _| crate::chrome::header(s, &s.ink, 0.0),
            (700.0, 100.0),
            rune::Env::new(true, 2.0),
        );
        rt.frame(0.0);
        let badge = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("terminal-current-branch"))
            .unwrap();
        assert_eq!(
            badge.label.as_deref(),
            Some("Current branch: Detached · 12345678")
        );
        assert!(
            !poll(&mut rt.state, 12.9, &mut Tx::default()),
            "no read before three seconds"
        );
        assert!(rt.state.terminal_branch.pending.is_none());
        rt.state.focus_session_tab(&b);
        assert!(
            view(&rt.state, &rt.state.ink).is_none(),
            "hide before poll reconciles focus"
        );
        rt.state.focus_session_tab(&a);
        rt.state
            .data
            .sessions
            .iter_mut()
            .find(|s| s.info.id == a)
            .unwrap()
            .info
            .cwd = "/elsewhere".into();
        assert!(
            view(&rt.state, &rt.state.ink).is_none(),
            "hide before poll reconciles cwd"
        );
        rt.state.view = View::Editor;
        assert!(target(&rt.state).is_none());
        assert!(view(&rt.state, &rt.state.ink).is_none());
        assert!(poll(&mut rt.state, 13.0, &mut Tx::default()));
        assert!(rt.state.terminal_branch.pending.is_none());
        assert!(rt.state.terminal_branch.data.is_none());
    }

    #[test]
    fn switching_target_discards_stale_replies_and_failures_clear_previous_data() {
        let a = Target {
            session: "a".into(),
            cwd: "/a".into(),
        };
        let b = Target {
            session: "b".into(),
            cwd: "/b".into(),
        };
        let branch = branch::Branch {
            name: Some("topic".into()),
            head: String::new(),
        };
        let (send, recv) = mpsc::channel();
        let mut state = BranchState {
            target: Some(a.clone()),
            data: Some(branch.clone()),
            checked_at: Some(1.0),
            pending: Some((a, recv)),
        };
        assert!(state.reconcile(Some(b.clone())));
        assert!(state.data.is_none());
        assert!(state.checked_at.is_none());
        send.send(Some(branch.clone())).unwrap();
        assert!(state.drain());
        assert!(state.data.is_none());
        let (send, recv) = mpsc::channel();
        state.pending = Some((b, recv));
        state.data = Some(branch);
        send.send(None).unwrap();
        assert!(state.drain());
        assert!(state.data.is_none());
        assert!(state.reconcile(None));
    }
}
