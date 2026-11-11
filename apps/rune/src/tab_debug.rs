//! Tab diagnostics use stable tab IDs; session actions always resolve that tab's focused pane.

use rune::{El, Tx, el};
use rune_kit::{MenuItem, context_menu};

use crate::{State, View, panes};
use app_core::session_debug::{Identity, memory_kib};

pub struct MenuTarget {
    pub tab: String,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy)]
enum CopyKind {
    Tab,
    Session,
    Agent,
    Debug,
}

pub fn inspection_sessions(s: &State) -> Vec<String> {
    s.tab_menu
        .as_ref()
        .and_then(|m| s.open_tabs.iter().find(|t| t.id == m.tab))
        .map(|tab| {
            panes::collect_panes(&tab.layout)
                .iter()
                .map(|p| p.session.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn focused_session<'a>(s: &'a State, tab: &panes::Tab) -> Option<&'a crate::data::SessionView> {
    let pane = panes::pane_by_id(&tab.layout, &tab.active_pane)?;
    s.session_by_key(&pane.session)
}

fn identity<'a>(s: &'a State, key: &str) -> Result<&'a Identity, &'a str> {
    if !s
        .session_by_key(key)
        .is_some_and(|view| app_core::session_debug::live(view.info.state.into()))
    {
        return Err("Session is not running; current agent ID is unavailable");
    }
    if s.debug_at.is_none_or(|at| s.clock - at > 6.0) {
        return Err("Waiting for a current process sample");
    }
    match s.debug.identities.get(key) {
        Some(Ok(id)) => Ok(id),
        Some(Err(reason)) => Err(reason),
        None => Err("Checking agent session ID…"),
    }
}

fn memory_label(s: &State, tab: &panes::Tab) -> String {
    let live: Vec<String> = panes::collect_panes(&tab.layout)
        .iter()
        .filter_map(|pane| s.session_by_key(&pane.session))
        .filter(|view| app_core::session_debug::live(view.info.state.into()))
        .map(|view| view.info.id.clone())
        .collect();
    if live.is_empty() {
        return "Tab RAM: not running".into();
    }
    if s.debug_at.is_none_or(|at| s.clock - at > 6.0) {
        return "Tab RAM: waiting for a current sample".into();
    }
    match memory_kib(&s.debug, &live) {
        Some(kib) => format!("Tab RAM ≈ {:.1} MB", kib as f64 * 1024.0 / 1_000_000.0),
        None => "Tab RAM: process sample unavailable".into(),
    }
}

fn debug_info(s: &State, tab: &panes::Tab) -> String {
    let mut lines = vec![format!("Goko {}", env!("CARGO_PKG_VERSION")), format!("Tab ID: {}", tab.id), memory_label(s, tab),
        "RAM: local process RSS estimate; shared pages may be counted more than once; excludes Goko UI, daemon, shared agent servers, and cloud memory.".into()];
    for pane in panes::collect_panes(&tab.layout) {
        lines.push(format!(
            "\nPane ID: {}{}",
            pane.id,
            if pane.id == tab.active_pane {
                " (focused)"
            } else {
                ""
            }
        ));
        lines.push(format!("Goko Session ID: {}", pane.session));
        if let Some(view) = s.session_by_key(&pane.session) {
            lines.push(format!("Agent: {}", view.agent_id().unwrap_or("shell")));
            lines.push(format!("State: {:?}", view.info.state));
            lines.push(format!(
                "PID: {}",
                view.info
                    .pid
                    .map_or_else(|| "unavailable".into(), |pid| pid.to_string())
            ));
            match identity(s, &pane.session) {
                Ok(id) => {
                    lines.push(format!("Agent Session ID ({}): {}", id.agent, id.id));
                    lines.push(format!("ID source: {}", id.source));
                }
                Err(reason) => lines.push(format!("Agent Session ID: unavailable — {reason}")),
            }
        }
    }
    lines.join("\n")
}

fn copy_value(s: &State, tab_id: &str, kind: CopyKind) -> Option<String> {
    let tab = s.open_tabs.iter().find(|t| t.id == tab_id)?;
    match kind {
        CopyKind::Tab => Some(tab.id.clone()),
        CopyKind::Session => Some(focused_session(s, tab)?.info.id.clone()),
        CopyKind::Agent => Some(
            identity(s, &focused_session(s, tab)?.info.id)
                .ok()?
                .id
                .clone(),
        ),
        CopyKind::Debug => Some(debug_info(s, tab)),
    }
}

fn copy_item(label: &str, tab: &str, kind: CopyKind) -> MenuItem {
    let tab = tab.to_owned();
    MenuItem::new(label).on_select_tx(move |s: &mut State, tx: &mut Tx| {
        let value = copy_value(s, &tab, kind);
        s.tab_menu = None;
        if let Some(value) = value {
            tx.copy(value);
            s.notice = Some("Copied to clipboard".into());
        } else {
            s.notice = Some("This tab or session is no longer available".into());
        }
    })
}

pub fn menu_layer(s: &State, layer: &mut Vec<El>) -> Option<El> {
    if s.view != View::Terminal {
        return None;
    }
    let target = s.tab_menu.as_ref()?;
    let tab = s.visible_tabs().find(|(_, t)| t.id == target.tab)?.1;
    let session = focused_session(s, tab);
    let id = session.map_or(Err("Focused session is unavailable"), |view| {
        identity(s, &view.info.id)
    });
    let description = match &id {
        Ok(id) => format!("{} — {}", id.agent, id.source),
        Err(reason) => reason.to_string(),
    };
    Some(
        context_menu(
            "tab-debug",
            el(),
            Some(rune::PointerPress {
                x: target.x,
                y: target.y,
            }),
        )
        .min_width(320.0)
        .item(copy_item("Copy Tab ID", &tab.id, CopyKind::Tab))
        .item(
            copy_item("Copy Goko Session ID", &tab.id, CopyKind::Session)
                .description("Focused pane in this tab")
                .disabled(session.is_none()),
        )
        .item(
            copy_item("Copy Agent Session ID", &tab.id, CopyKind::Agent)
                .description(description)
                .disabled(id.is_err()),
        )
        .item(
            copy_item("Copy Debug Info", &tab.id, CopyKind::Debug)
                .description("Tab and all panes, IDs, PIDs, and RAM"),
        )
        .separator()
        .item(
            MenuItem::new(memory_label(s, tab))
                .disabled(true)
                .description("Approximate RSS; excludes UI, servers, and cloud"),
        )
        .on_close(|s: &mut State| s.tab_menu = None)
        .build(&s.editor.kit, layer),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::session::{SessionId, SessionInfo, SessionState};
    use std::collections::HashMap;

    fn scene() -> State {
        let mut s = State::new(crate::test_handle());
        for (pid, title) in [(10, "first"), (20, "second")] {
            let id = SessionId::new();
            s.upsert_session(SessionInfo {
                id,
                title: title.into(),
                argv: vec!["/bin/zsh".into()],
                cwd: "/tmp".into(),
                state: SessionState::Running,
                pid: Some(pid),
                exit_code: None,
                error: None,
                started_at: 1,
                ended_at: None,
            });
            s.place_session(&id.as_uuid().to_string(), title, panes::Placement::Tab);
        }
        s.active_tab = 0;
        s.sync_selection();
        s
    }

    #[test]
    fn copy_targets_clicked_tab_and_focused_split_pane_and_sums_unique_rss() {
        let mut s = scene();
        assert_eq!(s.open_tabs.len(), 2);
        let tab_id = s.open_tabs[0].id.clone();
        let a = panes::collect_panes(&s.open_tabs[0].layout)[0].clone();
        let b = panes::collect_panes(&s.open_tabs[1].layout)[0].clone();
        s.open_tabs[0].layout = panes::Layout::Split(panes::Split {
            id: "split".into(),
            direction: panes::Direction::SideBySide,
            ratio: 0.5,
            first: Box::new(panes::Layout::Pane(a.clone())),
            second: Box::new(panes::Layout::Pane(b.clone())),
        });
        s.open_tabs[0].active_pane = b.id.clone();
        s.active_tab = 1;
        s.debug_at = Some(s.clock);
        s.debug.trees = HashMap::from([
            (a.session.clone(), vec![10, 30]),
            (b.session.clone(), vec![20, 30]),
        ]);
        s.debug.rss_kib = HashMap::from([(10, 1000), (20, 2000), (30, 4000)]);
        s.debug.identities.insert(
            b.session.clone(),
            Ok(Identity {
                agent: "devin".into(),
                id: "private-peanut".into(),
                source: "lock".into(),
            }),
        );
        assert_eq!(copy_value(&s, &tab_id, CopyKind::Tab), Some(tab_id.clone()));
        assert_eq!(
            copy_value(&s, &tab_id, CopyKind::Session),
            Some(b.session.clone())
        );
        assert_eq!(
            copy_value(&s, &tab_id, CopyKind::Agent).as_deref(),
            Some("private-peanut")
        );
        let debug = copy_value(&s, &tab_id, CopyKind::Debug).unwrap();
        assert!(debug.contains(&a.session) && debug.contains(&b.session));
        assert!(debug.contains("Tab RAM ≈ 7.2 MB"));
        assert!(debug.contains("PID: 10") && debug.contains("PID: 20"));
        s.clock += 7.0;
        assert!(copy_value(&s, &tab_id, CopyKind::Agent).is_none());
        assert!(memory_label(&s, &s.open_tabs[0]).contains("waiting"));
        s.open_tabs.remove(0);
        assert!(copy_value(&s, &tab_id, CopyKind::Tab).is_none());
    }

    #[test]
    fn right_click_menu_copies_the_inactive_tab_id() {
        let s = scene();
        let tab_id = s.open_tabs[1].id.clone();
        let mut rt = rune::Runtime::new(s, crate::view, (1000.0, 700.0), rune::Env::new(true, 2.0));
        let clipboard = rune::clipboard::memory();
        rt.use_clipboard(&clipboard);
        rt.on_key = Some(crate::on_key);
        rt.frame(0.0);
        let tab = rt
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(&format!("tab:{tab_id}")))
            .expect("tab mounted");
        let (x, y) = (tab.rect[0] + 15.0, tab.rect[1] + tab.rect[3] / 2.0);
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.tab_menu.as_ref().unwrap().tab, tab_id);
        assert_eq!(
            rt.state.active_tab, 0,
            "right click does not activate the target tab"
        );
        for label in [
            "Copy Tab ID",
            "Copy Goko Session ID",
            "Copy Agent Session ID",
            "Copy Debug Info",
        ] {
            assert!(
                rt.elements()
                    .iter()
                    .any(|e| e.text.as_deref() == Some(label)),
                "{label} mounted"
            );
        }
        let item = rt
            .elements()
            .into_iter()
            .find(|e| !e.exiting && e.text.as_deref() == Some("Copy Tab ID"))
            .unwrap();
        let (x, y) = (item.rect[0] + 10.0, item.rect[1] + item.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.tab_menu.is_none());
        assert_eq!(
            clipboard.text().map(|text| text.to_string()),
            Some(tab_id.clone())
        );
        rt.update(move |s, _| {
            s.tab_menu = Some(MenuTarget {
                tab: tab_id,
                x: 400.0,
                y: 120.0,
            })
        });
        rt.frame(1.0 / 60.0);
        rt.key(rune::KeyEvent {
            key: rune::Key::Escape,
            mods: Default::default(),
        });
        rt.frame(1.0 / 60.0);
        assert!(rt.state.tab_menu.is_none(), "Escape closes the menu");
    }
}
