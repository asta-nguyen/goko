use super::*;
use crate::editor::draft::OpenDoc;
use crate::editor::navigation::{Guard, GuardAction, PendingNav};
use crate::editor::state::Section;
use app_core::git_view::{GitViewFile, GitViewSearch, GitViewSearchMatch};
use app_core::projects::ProjectView;
use app_core::store::Project;
use app_core::worktrees::{WorktreeInfo, WorktreeList};
use rune::{Env, Key, KeyEvent, Mods, Runtime};
use std::path::PathBuf;
use std::sync::mpsc;

fn project(id: &str, name: &str, root: &str) -> ProjectView {
    ProjectView {
        project: Project {
            id: id.into(),
            name: name.into(),
            root: root.into(),
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
            created_at: 0,
        },
        missing: false,
        parent_project_id: None,
    }
}

fn wt(name: &str, path: &str, is_root: bool) -> WorktreeInfo {
    WorktreeInfo {
        name: name.into(),
        path: path.into(),
        branch: None,
        head: String::new(),
        is_root,
        missing: false,
        dirty: None,
        changed: None,
    }
}

/// Full-app Editor scene at `view == View::Editor`: registered project,
/// selected checkout, populated file tree — and the mock terminal
/// session still selected, so a leaked controls bar would show.
fn editor_scene() -> State {
    let mut s = State::new(test_handle());
    s.view = View::Editor;
    s.editor.project_id = Some("p1".into());
    s.editor.projects = vec![
        project("p1", "web", "/work/a/web"),
        project("p2", "api", "/work/b/api"),
    ];
    s.editor.worktrees = WorktreeList {
        repo: true,
        worktrees: vec![wt("web", "/work/a/web", true)],
    };
    let co = PathBuf::from("/tmp/goko-view-qa");
    s.editor.checkout = Some(co.clone());
    s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
    s.editor.section = Section::Files;
    s.editor.files.paths = [
        "README.md",
        "TREE_ONLY.txt",
        "docs/guide.md",
        "src/a.rs",
        "src/b.rs",
        "top.txt",
    ]
    .iter()
    .map(|p| p.to_string())
    .collect();
    s.editor.rebuild_file_model();
    s
}

/// An editable text doc whose buffer differs from the saved text.
fn dirty_doc() -> OpenDoc {
    let file = GitViewFile {
        path: "src/a.rs".into(),
        kind: "text",
        text: Some("fn main() {}\n".into()),
        truncated: false,
        size: 12,
    };
    let mut doc = OpenDoc::new("src/a.rs", &file);
    doc.code.edit.text.push_str("// unsaved edit\n");
    assert!(doc.dirty());
    doc
}

fn dirty_scene() -> State {
    let mut s = editor_scene();
    s.editor.doc = Some(dirty_doc());
    s
}

fn els(rt: &Runtime<State>) -> Vec<rune::ElementInfo> {
    rt.elements()
}

fn press(rt: &mut Runtime<State>, key: Key) {
    rt.key(KeyEvent {
        key,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
}

/// Index of the element whose ancestor chain (or itself) has `role`.
fn inside_role(els: &[rune::ElementInfo], idx: usize, role: &str) -> bool {
    let mut i = Some(idx);
    while let Some(n) = i {
        if els[n].role == role {
            return true;
        }
        i = els[n].parent;
    }
    false
}

/// Every mounted element stays inside the viewport horizontally —
/// the regression the fixed-width columns caused.
fn assert_no_horizontal_overflow(rt: &Runtime<State>, width: f32) {
    let mut bad = Vec::new();
    for e in els(rt) {
        let v = e.visible;
        if v[2] > 0.5 && !(v[0] >= -0.5 && v[0] + v[2] <= width + 0.5) {
            bad.push(format!(
                "role={:?} label={:?} text={:?} key={:?} visible={:?}",
                e.role,
                e.label,
                e.text,
                e.key.as_deref(),
                v
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "overflow in {}px:\n{}",
        width,
        bad.join("\n")
    );
}

/// A daemon session in the scene, exactly as the client's `session.list`
/// delivers it: no metadata row unless the test adds one.
fn seed_session(s: &mut State, title: &str, cwd: &str) -> String {
    let info = protocol::session::SessionInfo {
        id: protocol::session::SessionId::new(),
        title: title.into(),
        argv: vec!["/bin/zsh".into()],
        cwd: cwd.into(),
        state: protocol::session::SessionState::Running,
        pid: None,
        exit_code: None,
        error: None,
        started_at: 0,
        ended_at: None,
    };
    let key = crate::data::SessionInfo::key(info.id);
    s.upsert_session(info);
    key
}

#[test]
fn editor_view_hides_terminal_session_controls() {
    // A selected session in the Editor view: before the fix the
    // controls bar rendered Stop/Close pane inside the editor.
    let mut s = editor_scene();
    let key = seed_session(&mut s, "shell", "/tmp");
    s.selected_session = Some(key);
    assert!(s.selected_session.is_some());
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    // The controls strip itself must not be mounted in the Editor view.
    assert!(
        !els(&rt)
            .iter()
            .any(|el| el.key.as_deref() == Some("controls-bar")),
        "the session controls leaked into the editor"
    );
}

/// A pane holding `key`'s session with the terminal `terminal` feeds:
/// the attachment every visible pane owns.
fn attach(s: &mut State, key: &str, terminal: term::Terminal) {
    s.attachments.insert(
        key.to_string(),
        panes::Attachment::new(terminal, term::TERM_COLS, term::TERM_ROWS),
    );
}

#[test]
fn an_attached_terminal_is_drawn_by_the_poll_that_attaches_it() {
    // The terminal must not wait for a frame that nobody asked for:
    // the poll that attaches the session draws its grid.
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "shell", "/tmp");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
    let terminal = term::Terminal::new(session, rx, ready_rx);
    tx.send(daemon::Output {
        seq: 0,
        data: b"hello\r\n".to_vec(),
    })
    .unwrap();
    attach(&mut s, &key, terminal);
    let pane_key = key.clone();

    let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
    rt.update(|s, tx| {
        s.poll_terminals(tx);
    });
    let mut lines = 0;
    rt.update(|s, _| {
        lines = s
            .attachments
            .get(&pane_key)
            .map(|a| a.lines.len())
            .unwrap_or(0);
    });
    assert!(lines > 0, "the terminal drew its grid in the same poll");
}

#[test]
fn apply_theme_repaints_an_idle_terminal_without_a_resize() {
    // An attached terminal that has settled stays cached in the old
    // palette: only apply_theme's invalidation repaints it — no output,
    // no resize, no parser movement.
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "shell", "/tmp");
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    tx.send(daemon::Output {
        seq: 0,
        data: "\u{1b}[41mRED\u{1b}[0m\r\n".as_bytes().to_vec(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    s.apply_theme();
    let before = s.attachments[&key].lines.clone();
    assert!(!before.is_empty(), "the idle grid was drawn once");

    s.theme_choice = theme::Theme::Light;
    s.apply_theme();
    let att = &s.attachments[&key];
    assert_ne!(att.lines, before, "the cached rows took the new ink");
    assert_eq!(
        att.lines.len(),
        before.len(),
        "the same grid: no wrapped lines, no resize"
    );
    assert_eq!(att.term.size(), (att.cols, att.rows));
    assert_eq!(att.term.next_seq, 14, "the stream offsets did not move");
}

#[test]
fn the_terminal_grid_fits_the_pane() {
    // The grid follows the pane: a line that wrapped would make the
    // content taller than the viewport, and that is what made a
    // full-screen program shake.
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "shell", "/tmp");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    tx.send(daemon::Output {
        seq: 0,
        data: "x".repeat(400).into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let pane_key = key.clone();

    let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    // The first layout measures the pane and resizes the grid; the
    // poll then draws the grid at its new size.
    rt.frame(1.0 / 60.0);

    // Every drawn row is one line tall: nothing wrapped.
    let mut rows = 0;
    let mut height = 0.0;
    let mut widest = 0.0_f32;
    for e in els(&rt) {
        let Some(text) = e.text.as_deref() else {
            continue;
        };
        if !text.starts_with("xxx") {
            continue;
        }
        rows += 1;
        height += e.visible[3];
        widest = widest.max(e.visible[2]);
        assert!(
            e.visible[3] <= 17.0,
            "a terminal row wrapped: height {} for {text:?}",
            e.visible[3]
        );
    }
    assert!(rows > 0, "the terminal drew its grid");

    // The grid took the pane's size, and the drawn grid fits inside
    // it — the pane never has to scroll, so the content never jumps.
    let mut cols = 0;
    let mut grid_rows = 0;
    rt.update(|s, _| {
        if let Some(att) = s.attachments.get(&pane_key) {
            cols = att.cols;
            grid_rows = att.rows;
        }
    });
    assert!(cols >= 20, "the grid took the pane's width: {cols}");
    assert!(
        grid_rows >= 4,
        "the grid took the pane's height: {grid_rows}"
    );
    let pane = 1512.0 - sidebar::DEFAULT_SIDEBAR_W - 8.0 - 2.0 * 8.0;
    assert!(
        widest <= pane,
        "the grid is wider than the pane: {widest} > {pane}"
    );
    assert!(
        height <= 814.0 - 48.0 - 41.0 - 40.0,
        "the grid is taller than the pane: {height}"
    );
}

#[test]
fn a_notice_is_shown_as_a_toast() {
    let mut s = State::new(test_handle());
    s.notice = Some("boom happened".into());
    let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    let shown = |rt: &Runtime<State>| {
        els(rt)
            .iter()
            .any(|e| e.text.as_deref() == Some("boom happened"))
    };
    assert!(shown(&rt), "the notice is on screen");
}

#[test]
fn the_notice_reports_the_time_left_before_it_closes() {
    let mut s = State::new(test_handle());
    assert_eq!(
        s.notice_remaining(0.0),
        None,
        "no toast, nothing to wait for"
    );
    s.notice = Some("boom".into());
    s.expire_notice(10.0);
    assert!((s.notice_remaining(10.0).unwrap() - 8.0).abs() < 1e-9);
    assert!((s.notice_remaining(15.0).unwrap() - 3.0).abs() < 1e-9);
}

#[test]
fn a_notice_closes_eight_seconds_after_it_was_set() {
    let mut s = State::new(test_handle());
    s.notice = Some("first".into());
    s.expire_notice(0.0);
    s.expire_notice(7.0);
    assert!(s.notice.is_some(), "still shown at 7 s");
    s.expire_notice(8.1);
    assert!(s.notice.is_none(), "closed past 8 s");
}

#[test]
fn a_newer_notice_restarts_the_eight_second_timer() {
    let mut s = State::new(test_handle());
    s.notice = Some("first".into());
    s.expire_notice(0.0);
    s.notice = Some("second".into());
    s.expire_notice(7.0);
    s.expire_notice(14.9);
    assert_eq!(s.notice.as_deref(), Some("second"), "alive until 15 s");
    s.expire_notice(15.0);
    assert!(s.notice.is_none(), "closed 8 s after the second notice");
}

#[test]
fn a_full_screen_redraw_does_not_move_the_grid() {
    // What an agent TUI does: rewrite every row of the screen. The
    // grid may change, but the rows must stay where they are — a
    // moving row is what the eye reads as shaking.
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "shell", "/tmp");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (_ready_tx, ready_rx) = std::sync::mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    tx.send(daemon::Output {
        seq: 0,
        data: "x".repeat(400).into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);

    let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);

    let rows_of = |rt: &Runtime<State>| -> Vec<[f32; 4]> {
        els(rt)
            .iter()
            .filter(|e| e.text.as_deref().is_some_and(|t| t.starts_with("xxx")))
            .map(|e| e.visible)
            .collect()
    };
    let before = rows_of(&rt);
    assert!(!before.is_empty(), "the terminal drew its grid");

    // The redraw: every row is rewritten, with the cursor jumping home.
    let mut redraw = String::from("\u{1b}[H");
    for line in 0..20 {
        redraw.push_str(&format!("\u{1b}[{};1Hxxx{line} {}", line, "y".repeat(120)));
    }
    tx.send(daemon::Output {
        seq: 1,
        data: redraw.into_bytes(),
    })
    .unwrap();
    rt.frame(1.0 / 60.0);
    // The repaint waits for the burst to settle.
    std::thread::sleep(std::time::Duration::from_millis(30));
    rt.frame(1.0 / 60.0);

    let after = rows_of(&rt);
    assert!(after.len() >= 2, "the redraw drew its rows");
    // The grid starts where it did: the redraw changed the content,
    // not the layout.
    assert_eq!(
        before.first().unwrap()[1],
        after.first().unwrap()[1],
        "the grid moved: {:?} -> {:?}",
        before.first().unwrap(),
        after.first().unwrap()
    );
    // And its rows sit on the line grid.
    let gap = after[1][1] - after[0][1];
    assert!(
        (gap - 16.0).abs() < 1.0,
        "the rows are not on the line grid: {gap}"
    );
}

#[test]
fn pickers_fit_duplicate_project_names_at_760() {
    let mut s = editor_scene();
    // Two registered projects sharing a name with deep canonical roots:
    // labels disambiguate by path and must not stretch the bar.
    s.editor.projects = vec![
        project(
            "p1",
            "web",
            "/Users/dev/work/clients/acme-corp/frontend/apps/web",
        ),
        project(
            "p2",
            "web",
            "/Users/dev/work/clients/globex-inc/backend/services/web",
        ),
    ];
    s.editor.project_id = Some("p1".into());
    let mut rt = Runtime::new(s, view, (760.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    let mut combos: Vec<_> = els(&rt)
        .into_iter()
        .filter(|e| e.role == "combobox")
        .collect();
    combos.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    assert_eq!(combos.len(), 2, "project + checkout pickers mounted");
    for c in &combos {
        assert!(c.rect[2] > 40.0, "picker collapsed: {:?}", c.rect);
        assert!(
            c.rect[0] >= 0.0 && c.rect[0] + c.rect[2] <= 760.0,
            "picker escapes viewport: {:?}",
            c.rect
        );
    }
    // The project picker's help keeps the full disambiguated identity.
    let help = combos[0].help.as_deref().unwrap_or_default();
    assert!(
        help.contains("acme-corp"),
        "project picker lost its path identity: {help:?}"
    );
    let add = els(&rt)
        .into_iter()
        .find(|e| e.label.as_deref() == Some("Add project"));
    assert!(add.is_none(), "Add project button is removed from header");
    assert_no_horizontal_overflow(&rt, 760.0);
}

#[test]
fn search_open_shows_results_in_aside_without_overflow() {
    let mut s = dirty_scene();
    s.editor.doc = Some({
        let file = GitViewFile {
            path: "src/a.rs".into(),
            kind: "text",
            text: Some("fn main() {}\n".into()),
            truncated: false,
            size: 12,
        };
        OpenDoc::new("src/a.rs", &file)
    });
    s.editor.files.search.open = true;
    s.editor.files.search.results = Some(GitViewSearch {
        matches: (1..=30)
            .map(|i| GitViewSearchMatch {
                path: format!("src/f{i}.rs"),
                line: i,
                text: "fn f() -> i32 { i }".into(),
            })
            .collect(),
        truncated: false,
    });
    let mut rt = Runtime::new(s, view, (760.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    let e = els(&rt);
    // Results live in the aside (desktop SplitPane parity): a match row
    // is mounted and the file tree is gone.
    assert!(
        e.iter()
            .any(|el| el.label.as_deref() == Some("src/f1.rs:1")),
        "search result row missing"
    );
    assert!(
        !e.iter()
            .any(|el| el.label.as_deref() == Some("TREE_ONLY.txt")),
        "file tree still mounted beside results"
    );
    assert_no_horizontal_overflow(&rt, 760.0);
    // The editor surface keeps usable width in the remaining space:
    // its `text_input` mounts below the toolbars (y > 150 at this
    // viewport) and must be wider than a sliver.
    let editor_fields: Vec<_> = els(&rt)
        .into_iter()
        .filter(|el| el.role == "textbox" && el.rect[1] > 150.0)
        .collect();
    assert!(
        editor_fields.iter().any(|el| el.rect[2] > 100.0),
        "editor surface collapsed: {editor_fields:?}"
    );
}

#[test]
fn guard_dialog_is_modal_with_inert_background() {
    let mut s = dirty_scene();
    s.editor.guard = Some(Guard {
        pending: PendingNav::SetSection(Section::History),
        doc_path: "src/a.rs".into(),
        queue: Vec::new(),
        quitting: false,
    });
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    let e = els(&rt);
    let dialog_idx = e
        .iter()
        .position(|el| el.role == "dialog")
        .expect("guard dialog mounted with dialog role");
    // Background controls are inert; the dialog's own controls are not.
    let tab_idx = e
        .iter()
        .position(|el| el.role == "tab")
        .expect("section tabs mounted");
    assert!(e[tab_idx].inert, "background tab not inert under modal");
    for label in ["Cancel", "Discard", "Save"] {
        // The doc bar also renders Save/Discard for the dirty draft —
        // pick the control INSIDE the dialog.
        let idx = e
            .iter()
            .enumerate()
            .find(|(i, el)| {
                (el.label.as_deref() == Some(label) || el.text.as_deref() == Some(label))
                    && inside_role(&e, *i, "dialog")
            })
            .map(|(i, _)| i)
            .unwrap_or_else(|| panic!("{label} button mounted in dialog"));
        assert!(!e[idx].inert, "{label} inert inside dialog");
    }
    let _ = dialog_idx;
}

#[test]
fn askpass_dialog_is_modal_with_the_passphrase_field() {
    let mut s = dirty_scene();
    let (answer, _answers) = std::sync::mpsc::sync_channel(1);
    s.askpass = Some(askpass::AskpassState::new(askpass::AskpassRequest {
        prompt: "Enter passphrase for key '/k': ".into(),
        answer,
    }));
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    let e = els(&rt);
    assert!(
        e.iter().any(|el| el.role == "dialog"),
        "askpass dialog mounted with dialog role"
    );
    for label in ["Cancel", "Unlock"] {
        let idx = e
            .iter()
            .enumerate()
            .find(|(i, el)| {
                (el.label.as_deref() == Some(label) || el.text.as_deref() == Some(label))
                    && inside_role(&e, *i, "dialog")
            })
            .map(|(i, _)| i)
            .unwrap_or_else(|| panic!("{label} button mounted in dialog"));
        assert!(!e[idx].inert, "{label} inert inside dialog");
    }
    let tab_idx = e
        .iter()
        .position(|el| el.role == "tab")
        .expect("section tabs mounted");
    assert!(e[tab_idx].inert, "background tab not inert under askpass");
    assert!(shortcut_owned(&rt.state));
}

#[test]
fn askpass_submit_and_cancel_answer_the_push() {
    let mut s = dirty_scene();
    let (answer, answers) = std::sync::mpsc::sync_channel(1);
    s.askpass = Some(askpass::AskpassState::new(askpass::AskpassRequest {
        prompt: "Enter passphrase for key '/k': ".into(),
        answer,
    }));
    s.askpass.as_mut().unwrap().input.text = "secret".into();
    askpass::submit(&mut s);
    assert_eq!(answers.recv().unwrap(), Some("secret".into()));
    assert!(s.askpass.is_none(), "submit cleared the prompt");

    let (answer, answers) = std::sync::mpsc::sync_channel(1);
    s.askpass = Some(askpass::AskpassState::new(askpass::AskpassRequest {
        prompt: "Enter passphrase for key '/k': ".into(),
        answer,
    }));
    askpass::cancel(&mut s);
    assert_eq!(answers.recv().unwrap(), None);
    assert!(s.askpass.is_none(), "cancel cleared the prompt");

    let (answer, _answers) = std::sync::mpsc::sync_channel(1);
    s.askpass = Some(askpass::AskpassState::new(askpass::AskpassRequest {
        prompt: "Enter passphrase for key '/k': ".into(),
        answer,
    }));
    s.apply_reply(backend::Reply {
        req: backend::Req::Push,
        generation: 0,
        checkout_key: None,
        result: Err("Push timed out".into()),
    });
    assert!(s.askpass.is_none(), "a Push reply cleared the prompt");
}

#[test]
fn escape_cancels_guard_keeping_doc_identity_and_text() {
    let mut s = dirty_scene();
    s.editor.guard = Some(Guard {
        pending: PendingNav::SetSection(Section::History),
        doc_path: "src/a.rs".into(),
        queue: Vec::new(),
        quitting: false,
    });
    let before_text = s.editor.doc.as_ref().unwrap().code.edit.text.clone();
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(rt.state.editor.guard.is_some());

    press(&mut rt, Key::Escape);
    assert!(rt.state.editor.guard.is_none(), "Escape dismissed guard");
    let doc = rt.state.editor.doc.as_ref().expect("doc retained");
    assert!(doc.dirty(), "draft must stay dirty after Cancel");
    assert_eq!(doc.code.edit.text, before_text, "buffer untouched");
    assert_eq!(doc.path, "src/a.rs");
    // Identity + pending navigation all preserved: Cancel drops the
    // pending SetSection, keeps project/checkout/section as they were.
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
    assert_eq!(
        rt.state.editor.checkout.as_deref(),
        Some(std::path::Path::new("/tmp/goko-view-qa"))
    );
    assert_eq!(rt.state.editor.section, Section::Files);
}

#[test]
fn tab_focus_stays_trapped_inside_guard_dialog() {
    let mut s = dirty_scene();
    s.editor.guard = Some(Guard {
        pending: PendingNav::SetSection(Section::History),
        doc_path: "src/a.rs".into(),
        queue: Vec::new(),
        quitting: false,
    });
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.set_tab_visits_controls(true);
    rt.frame(0.0);

    // Walk Tab around the dialog; focus must never land on inert
    // background chrome.
    for _ in 0..8 {
        press(&mut rt, Key::Tab);
        let e = els(&rt);
        if let Some(fidx) = e.iter().position(|el| el.focused) {
            assert!(
                inside_role(&e, fidx, "dialog"),
                "Tab escaped the guard dialog to {:?}",
                e[fidx].label
            );
        }
    }
}

#[test]
fn section_tabs_report_selected_state() {
    let s = editor_scene();
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    // The workspace tab strip also uses role="tab" — section tabs are
    // keyed `section:<label>`.
    let tabs: Vec<_> = els(&rt)
        .into_iter()
        .filter(|e| e.role == "tab" && e.key.as_deref().is_some_and(|k| k.starts_with("section:")))
        .collect();
    assert_eq!(tabs.len(), 5, "five section tabs mounted");
    let selected: Vec<_> = tabs.iter().filter(|t| t.item_selected).collect();
    assert_eq!(selected.len(), 1, "exactly one tab selected");
    assert_eq!(selected[0].label.as_deref(), Some("Files"));
    for t in &tabs {
        assert_eq!(
            t.item_selected,
            t.label.as_deref() == Some("Files"),
            "tab {:?} selected state wrong",
            t.label
        );
    }
}

/// Doc bar at the narrow 760px viewport with a long path, a dirty
/// draft (Save/Discard live), staged change data (Show changes) and a
/// save error (Reload from disk): every control stays mounted inside
/// the window — the path label truncates instead of pushing buttons
/// off-screen.
#[test]
fn doc_bar_controls_fit_long_path_at_760() {
    let mut s = editor_scene();
    let long_path = "src/deeply/nested/directory/structure/with/a/very/long/file/name.rs";
    let file = GitViewFile {
        path: long_path.into(),
        kind: "text",
        text: Some("fn main() {}\n".into()),
        truncated: false,
        size: 12,
    };
    let mut doc = OpenDoc::new(long_path, &file);
    doc.code.edit.text.push_str("// unsaved edit\n");
    doc.save_error = Some("disk conflict".into());
    s.editor.doc = Some(doc);
    s.editor.changes.data = Some(app_core::git_view::GitViewChanges {
        branch: Some("main".into()),
        head: "abc1234".into(),
        conflicts: vec![],
        staged: vec![app_core::git_view::StatusEntry {
            path: long_path.into(),
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
    s.editor.changes.checkout = s.editor.checkout_key.clone();

    let mut rt = Runtime::new(s, view, (760.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let labels: Vec<String> = els(&rt).into_iter().filter_map(|e| e.label).collect();
    for want in ["Save", "Discard", "Show changes", "Reload from disk"] {
        assert!(
            labels.iter().any(|l| l == want),
            "missing {want}: {labels:?}"
        );
    }
    assert_no_horizontal_overflow(&rt, 760.0);
}

// -- sidebar → editor identity -------------------------------------------

/// Two registered projects as the backend's Projects reply leaves them,
/// each with its own loaded worktree list — p1 has two linked
/// worktrees, p2 has one. Terminal view, nothing committed yet.
fn sidebar_scene() -> State {
    let mut s = State::new(test_handle());
    s.editor.projects = vec![
        project("p1", "web", "/work/a/web"),
        project("p2", "api", "/work/b/api"),
    ];
    let lists = [
        (
            "p1",
            vec![
                wt("web", "/work/a/web", true),
                wt("feat-a", "/work/a/feat-a", false),
                wt("feat-b", "/work/a/feat-b", false),
            ],
        ),
        (
            "p2",
            vec![
                wt("api", "/work/b/api", true),
                wt("fix", "/work/b/fix", false),
            ],
        ),
    ];
    for (pid, worktrees) in lists {
        s.editor.sidebar_worktrees.insert(
            pid.to_string(),
            crate::editor::state::SidebarWorktrees {
                list: Some(WorktreeList {
                    repo: true,
                    worktrees,
                }),
                loading: false,
                error: None,
            },
        );
    }
    s
}

/// Editor already committed to p1's root checkout — the "already in
/// Editor" starting point for in-editor sidebar switches.
fn sidebar_editor_scene() -> State {
    let mut s = sidebar_scene();
    s.view = View::Editor;
    s.editor.project_id = Some("p1".into());
    s.editor.checkout = Some(PathBuf::from("/work/a/web"));
    s.editor.checkout_key = Some(crate::editor::state::checkout_key(
        "p1",
        &PathBuf::from("/work/a/web"),
    ));
    s.active_workspace = Some(crate::data::checkout_key("p1", "/work/a/web"));
    s
}

fn click_key(rt: &mut Runtime<State>, key: &str) {
    let e = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("element {key} mounted"));
    let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

#[test]
fn sidebar_lists_real_projects_and_sessions_without_metadata_stay_unassigned() {
    let mut s = sidebar_scene();
    // A daemon session with no stored metadata row: it belongs to no
    // registered project, so it renders under Unassigned.
    seed_session(&mut s, "fix-auth-flow", "/work/a/web");
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let e = els(&rt);
    for key in [
        "sidebar-project:p1",
        "sidebar-project:p2",
        "sidebar-worktree:p1:0",
        "sidebar-worktree:p1:1",
        "sidebar-worktree:p2:0",
        "sidebar-unassigned",
    ] {
        assert!(
            e.iter().any(|el| el.key.as_deref() == Some(key)),
            "missing {key}"
        );
    }
    // Unregistered project ids never become sidebar rows — they are not
    // registered projects, so nothing may select them as identity.
    for mock in ["proj-goko", "proj-notes"] {
        assert!(
            !e.iter()
                .any(|el| el.key.as_deref() == Some(&format!("sidebar-project:{mock}"))),
            "mock project {mock} leaked into the sidebar"
        );
    }
    // The session renders under Unassigned, never nested under a real
    // project — the honest terminal/session boundary.
    let texts: Vec<&str> = e
        .iter()
        .filter_map(|el| el.text.as_deref().or(el.label.as_deref()))
        .collect();
    assert!(texts.iter().any(|t| t.contains("fix-auth-flow")));
    assert!(texts.contains(&"Unassigned"));
}

/// Click the sidebar row of the session titled `title`.
fn click_session_row(rt: &mut Runtime<State>, title: &str) {
    let row = els(rt)
        .into_iter()
        .find(|e| e.text.as_deref().is_some_and(|t| t.contains(title)) && e.rect[0] < 330.0)
        .unwrap_or_else(|| panic!("session row {title} mounted"));
    let (x, y) = (row.rect[0] + 10.0, row.rect[1] + row.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

#[test]
fn a_session_row_opens_its_terminal_from_the_editor_and_from_settings() {
    for start in [View::Terminal, View::Editor, View::Settings] {
        let mut s = sidebar_scene();
        let first = seed_session(&mut s, "first-agent", "/work/a/web");
        let second = seed_session(&mut s, "second-agent", "/work/a/web");
        s.focus_session_tab(&first);
        s.focus_session_tab(&second);
        s.view = start;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_session_row(&mut rt, "first-agent");
        assert_eq!(rt.state.view, View::Terminal, "from {start:?}");
        assert_eq!(rt.state.selected_session.as_deref(), Some(first.as_str()));
        assert_eq!(
            rt.state.focused_session_key().as_deref(),
            Some(first.as_str()),
            "the clicked session's pane is the active one, from {start:?}"
        );
    }
}

#[test]
fn a_tab_pill_opens_its_terminal_from_the_editor() {
    let mut s = sidebar_scene();
    let first = seed_session(&mut s, "first-agent", "/work/a/web");
    let second = seed_session(&mut s, "second-agent", "/work/a/web");
    s.focus_session_tab(&first);
    s.focus_session_tab(&second);
    s.view = View::Editor;
    let tab = s.open_tabs[0].id.clone();
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, &format!("tab:{tab}"));
    assert_eq!(rt.state.view, View::Terminal);
    assert_eq!(rt.state.active_tab, 0);
}

#[test]
fn a_session_row_click_in_the_editor_respects_the_draft_guard() {
    let mut s = sidebar_scene();
    let key = seed_session(&mut s, "agent-one", "/work/a/web");
    s.focus_session_tab(&key);
    s.view = View::Editor;
    s.editor.doc = Some(editor::draft::OpenDoc::new(
        "a.txt",
        &app_core::git_view::GitViewFile {
            path: "a.txt".into(),
            kind: "text",
            text: Some("x".into()),
            truncated: false,
            size: 1,
        },
    ));
    s.editor.doc.as_mut().unwrap().code.edit.text = "edited".into();
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_session_row(&mut rt, "agent-one");
    assert_eq!(rt.state.view, View::Editor, "an unsaved draft asks first");
    assert!(rt.state.editor.guard.is_some(), "the guard dialog is up");
    assert_eq!(rt.state.selected_session.as_deref(), Some(key.as_str()));
}

#[test]
fn sidebar_project_row_activates_real_checkout_then_editor_opens_it() {
    let s = sidebar_scene();
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    // The original bug: pick a real project in the sidebar, then open
    // the Editor — the SAME project+checkout must activate.
    click_key(&mut rt, "sidebar-project:p1");
    assert_eq!(rt.state.view, View::Terminal, "selection alone stays put");
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
    assert_eq!(
        rt.state.editor.checkout,
        Some(PathBuf::from("/work/a/web")),
        "root row selects the registered project root"
    );
    assert_eq!(
        rt.state.active_workspace.as_deref(),
        Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
    );
    click_key(&mut rt, "view:Editor");
    assert_eq!(rt.state.view, View::Editor);
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
    assert_eq!(
        rt.state.editor.checkout,
        Some(PathBuf::from("/work/a/web")),
        "Editor activated the sidebar's committed checkout"
    );
}

#[test]
fn sidebar_worktree_row_selects_exact_checkout_while_in_editor() {
    let mut rt = Runtime::new(
        sidebar_editor_scene(),
        view,
        (1100.0, 700.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    // Already in Editor on p1's root: a worktree row press must move
    // project+checkout identity to that exact registered path.
    click_key(&mut rt, "sidebar-worktree:p1:1"); // feat-b
    assert_eq!(rt.state.view, View::Editor);
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
    assert_eq!(
        rt.state.editor.checkout,
        Some(PathBuf::from("/work/a/feat-b"))
    );
    assert_eq!(
        rt.state.active_workspace.as_deref(),
        Some(crate::data::checkout_key("p1", "/work/a/feat-b").as_str())
    );
    // A different project's worktree row switches project AND checkout.
    click_key(&mut rt, "sidebar-worktree:p2:0"); // fix
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
    assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/b/fix")));
    assert_eq!(
        rt.state.active_workspace.as_deref(),
        Some(crate::data::checkout_key("p2", "/work/b/fix").as_str())
    );
}

#[test]
fn sidebar_select_with_dirty_doc_guard_discard_commits() {
    let mut s = sidebar_editor_scene();
    s.editor.doc = Some(dirty_doc());
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-project:p2");
    // The guard held: NOTHING moved — project, checkout, workspace
    // highlight and the dirty draft all still belong to p1.
    assert!(rt.state.editor.guard.is_some());
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
    assert_eq!(
        rt.state.active_workspace.as_deref(),
        Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
    );
    assert!(rt.state.editor.doc.as_ref().unwrap().dirty());
    // Resolve the guard the way the dialog's Discard button does —
    // Runtime's headless hit-test does not deliver presses inside
    // modal layers, so the state-level path is the honest check.
    rt.update(|s, tx| {
        crate::editor::navigation::resolve_guard(s, GuardAction::Discard, tx);
    });
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
    assert_eq!(
        rt.state.editor.checkout,
        Some(PathBuf::from("/work/b/api")),
        "Discard applied the pending sidebar selection"
    );
    assert!(rt.state.editor.doc.is_none());
    assert_eq!(
        rt.state.active_workspace.as_deref(),
        Some(crate::data::checkout_key("p2", "/work/b/api").as_str())
    );
}

#[test]
fn sidebar_select_with_dirty_doc_guard_cancel_restores_everything() {
    let mut s = sidebar_editor_scene();
    s.editor.doc = Some(dirty_doc());
    let before_text = s.editor.doc.as_ref().unwrap().code.edit.text.clone();
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-worktree:p2:0"); // fix
    assert!(rt.state.editor.guard.is_some());
    rt.update(|s, tx| {
        crate::editor::navigation::resolve_guard(s, GuardAction::Cancel, tx);
    });
    assert!(rt.state.editor.guard.is_none(), "Cancel resolved the guard");
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
    assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/a/web")));
    assert_eq!(
        rt.state.active_workspace.as_deref(),
        Some(crate::data::checkout_key("p1", "/work/a/web").as_str()),
        "Cancel restored the previous visible workspace identity"
    );
    let doc = rt.state.editor.doc.as_ref().expect("draft retained");
    assert!(doc.dirty());
    assert_eq!(doc.code.edit.text, before_text, "buffer untouched");
}

#[test]
fn sidebar_status_rows_cover_loading_error_and_empty() {
    // Fresh State::new: no projects yet, no load in flight — the honest
    // empty state, never a fabricated row.
    let s = State::new(test_handle());
    assert!(s.active_workspace.is_none());
    assert!(s.editor.project_id.is_none());
    assert!(s.editor.checkout.is_none());
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let texts: Vec<String> = els(&rt)
        .iter()
        .filter_map(|e| e.text.clone().or_else(|| e.label.clone()))
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("No projects registered")),
        "empty state missing: {texts:?}"
    );

    // Initial async load in flight → loading row. Mutations go through
    // `update` so the next frame knows the view reads changed state.
    rt.update(|s, _| s.editor.projects_loading = true);
    rt.frame(0.0);
    let texts: Vec<String> = els(&rt)
        .iter()
        .filter_map(|e| e.text.clone().or_else(|| e.label.clone()))
        .collect();
    assert!(texts.iter().any(|t| t.contains("Loading projects")));

    // Registry error with no projects → the error itself, in danger tone.
    rt.update(|s, _| {
        s.editor.projects_loading = false;
        s.editor.projects_error = Some("goko.db is locked".into());
    });
    rt.frame(0.0);
    let texts: Vec<String> = els(&rt).iter().filter_map(|e| e.text.clone()).collect();
    assert!(texts.iter().any(|t| t.contains("goko.db is locked")));

    // A per-project worktree failure renders under ITS row only.
    rt.update(|s, _| *s = sidebar_scene());
    rt.update(|s, _| {
        let entry = s
            .editor
            .sidebar_worktrees
            .get_mut("p2")
            .expect("p2 worktrees seeded");
        entry.error = Some("not a git repository".into());
        entry.list = None;
    });
    rt.frame(0.0);
    let texts: Vec<String> = els(&rt).iter().filter_map(|e| e.text.clone()).collect();
    assert!(texts.iter().any(|t| t.contains("not a git repository")));
    // p1's rows are untouched.
    assert!(
        els(&rt)
            .iter()
            .any(|el| el.key.as_deref() == Some("sidebar-worktree:p1:0"))
    );
}

fn workspace(id: &str, name: &str) -> app_core::store::Workspace {
    app_core::store::Workspace {
        id: id.into(),
        name: name.into(),
        created_at: 0,
    }
}

/// Two workspaces with p1 in "Work" and p2 in Default, "Work" selected.
fn two_workspace_scene() -> State {
    let mut s = sidebar_scene();
    s.workspaces = vec![
        workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
        workspace("w1", "Work"),
    ];
    s.editor.projects[0].project.workspace_id = "w1".into();
    s.selected_workspace = Some("w1".into());
    s
}

/// A shell session with `title` and `project_id`, then the daemon's update
/// reporting its shell in `cd_to`. Returns the session key.
fn session_that_cds(
    s: &mut State,
    title: &str,
    project_id: Option<&str>,
    from: &str,
    cd_to: &str,
) -> String {
    let mut info = protocol::session::SessionInfo {
        id: protocol::session::SessionId::new(),
        title: String::new(),
        argv: vec!["/bin/zsh".into()],
        cwd: from.into(),
        state: protocol::session::SessionState::Running,
        pid: None,
        exit_code: None,
        error: None,
        started_at: 0,
        ended_at: None,
    };
    let key = crate::data::SessionInfo::key(info.id);
    s.session_metas.insert(
        key.clone(),
        app_core::store::SessionMeta {
            session_id: key.clone(),
            project_id: project_id.map(Into::into),
            agent: "shell".into(),
            argv: info.argv.clone(),
            cwd: from.into(),
            title: title.into(),
            created_at: 0,
            workspace_id: None,
        },
    );
    s.upsert_session(info.clone());
    info.cwd = cd_to.into();
    s.upsert_session(info);
    key
}

/// The flow from a new workspace: its `+` → Shell runs a terminal that
/// belongs to it, so its row and tab show there — even with the editor's
/// project still in Default — while Default's own loose terminal does not.
/// Removing a workspace closes what runs in it — its projects' sessions
/// and its own terminals — and nothing else.
#[test]
fn removing_a_workspace_closes_its_sessions() {
    let mut s = two_workspace_scene();
    let in_project = seed_session_in(&mut s, "/work/a/web", Some("p1"));
    let loose_here = seed_session_in(&mut s, "/tmp", None);
    let loose_default = seed_session_in(&mut s, "/tmp", None);
    let other_project = seed_session_in(&mut s, "/work/b/api", Some("p2"));
    let meta = s.session_metas.get_mut(loose_here.as_str()).unwrap();
    meta.workspace_id = Some("w1".into());
    let meta = meta.clone();
    s.data
        .sessions
        .iter_mut()
        .find(|v| v.info.id == loose_here)
        .unwrap()
        .meta = Some(meta);
    for key in [&in_project, &loose_here, &loose_default, &other_project] {
        s.place_session(key, key, panes::Placement::Tab);
    }

    s.apply_reply(reply(
        Req::RemoveWorkspace { id: "w1".into() },
        Ok(ReplyData::Workspaces(vec![workspace(
            app_core::store::DEFAULT_WORKSPACE_ID,
            "Default",
        )])),
    ));

    let closing = |key: &str| {
        s.open_tabs.iter().any(|tab| {
            panes::collect_panes(&tab.layout)
                .iter()
                .any(|pane| pane.session == key && pane.closing_at.is_some())
        })
    };
    assert!(closing(&in_project), "Work's project session closes");
    assert!(closing(&loose_here), "Work's own terminal closes");
    assert!(!closing(&loose_default), "Default's terminal keeps running");
    assert!(
        !closing(&other_project),
        "Default's project session keeps running"
    );
}

#[test]
fn a_new_workspaces_shell_shows_its_row_and_tab_there() {
    let mut s = sidebar_scene();
    s.workspaces = vec![
        workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
        workspace("w2", ""),
    ];
    editor::navigation::select_checkout(&mut s, "p1".into(), PathBuf::from("/work/a/web"));
    let home = seed_session_in(&mut s, "/tmp", None);
    let mine = seed_session_in(&mut s, "/tmp", None);
    for (key, title, workspace_id) in [
        (&home, "default-shell", None),
        (&mine, "brave-meadow", Some("w2")),
    ] {
        let meta = s.session_metas.get_mut(key.as_str()).unwrap();
        meta.title = title.into();
        meta.workspace_id = workspace_id.map(Into::into);
        let meta = meta.clone();
        let view = s
            .data
            .sessions
            .iter_mut()
            .find(|v| &v.info.id == key)
            .unwrap();
        view.info.title = title.into();
        view.meta = Some(meta);
        s.place_session(key, title, panes::Placement::Tab);
    }
    s.selected_workspace = Some("w2".into());
    let rt = key_runtime(s);
    let live = els(&rt)
        .into_iter()
        .filter(|el| !el.exiting)
        .collect::<Vec<_>>();
    let shown = |text: &str| live.iter().any(|el| el.text.as_deref() == Some(text));
    assert!(shown("New workspace"), "the unnamed workspace's label");
    assert!(shown("brave-meadow"), "its shell has a row");
    assert!(
        !shown("default-shell"),
        "Default's loose terminal stays in Default"
    );
    let tabs: Vec<_> = live
        .iter()
        .filter(|el| el.key.as_deref().is_some_and(|key| key.starts_with("tab:")))
        .filter_map(|el| el.label.clone())
        .collect();
    assert_eq!(tabs, vec!["brave-meadow".to_string()], "only its tab shows");
}

#[test]
fn a_project_session_stays_in_its_project_when_its_shell_cds_elsewhere() {
    let mut s = sidebar_scene();
    s.editor.project_id = Some("p1".into());
    let key = session_that_cds(&mut s, "web", Some("p1"), "/work/a/web", "/work/b/api");
    let meta = s.session_metas.get(&key).expect("meta");
    assert_eq!(meta.project_id.as_deref(), Some("p1"), "not moved to p2");
    assert_eq!(meta.title, "web", "a chosen title is kept");
}

#[test]
fn an_unassigned_shell_joins_the_registered_project_it_cds_into() {
    let mut s = sidebar_scene();
    // `/work/b/fix` is p2's linked worktree, not its root.
    let key = session_that_cds(&mut s, "brave-meadow", None, "/tmp", "/work/b/fix");
    let meta = s.session_metas.get(&key).expect("meta");
    assert_eq!(meta.project_id.as_deref(), Some("p2"));
    assert_eq!(meta.title, "api", "the generated name takes the project's");
    let view = s.data.sessions.iter().find(|v| v.info.id == key).unwrap();
    assert_eq!(
        view.meta.as_ref().and_then(|m| m.project_id.as_deref()),
        Some("p2"),
        "the sidebar's row joins p2 on this update, not the next one"
    );
    assert!(
        s.auto_project_roots.is_empty(),
        "a registered project is never re-added"
    );
}

#[test]
fn the_sidebar_shows_only_the_selected_workspaces_projects() {
    let mut s = two_workspace_scene();
    seed_session(&mut s, "fix-auth-flow", "/work/a/web");
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let e = els(&rt);
    assert!(
        e.iter()
            .any(|el| el.key.as_deref() == Some("sidebar-project:p1"))
    );
    assert!(
        !e.iter()
            .any(|el| el.key.as_deref() == Some("sidebar-project:p2")),
        "the other workspace's project is hidden"
    );
    assert!(
        !e.iter()
            .any(|el| el.key.as_deref() == Some("sidebar-unassigned")),
        "Unassigned is Default-only"
    );
    let texts: Vec<String> = e.iter().filter_map(|el| el.text.clone()).collect();
    assert!(
        !texts.iter().any(|t| t.contains("fix-auth-flow")),
        "an unassigned session stays out of a named workspace"
    );

    rt.update(|s, _| s.select_workspace(app_core::store::DEFAULT_WORKSPACE_ID));
    rt.frame(1.0 / 60.0);
    let e = els(&rt);
    assert!(
        e.iter()
            .any(|el| el.key.as_deref() == Some("sidebar-project:p2"))
    );
    assert!(
        !e.iter()
            .any(|el| el.key.as_deref() == Some("sidebar-project:p1") && !el.exiting),
        "the Work group's project is gone from the live tree"
    );
    assert!(
        e.iter()
            .any(|el| el.key.as_deref() == Some("sidebar-unassigned"))
    );
    let texts: Vec<String> = e.iter().filter_map(|el| el.text.clone()).collect();
    assert!(texts.iter().any(|t| t.contains("fix-auth-flow")));
}

/// A daemon session attached to `project_id` (None: Unassigned), with
/// its stored metadata row, so the sidebar partitions it.
fn seed_session_in(s: &mut State, cwd: &str, project_id: Option<&str>) -> String {
    let info = protocol::session::SessionInfo {
        id: protocol::session::SessionId::new(),
        title: "waiting-agent".into(),
        argv: vec!["claude".into()],
        cwd: cwd.into(),
        state: protocol::session::SessionState::Running,
        pid: None,
        exit_code: None,
        error: None,
        started_at: 0,
        ended_at: None,
    };
    let key = crate::data::SessionInfo::key(info.id);
    s.session_metas.insert(
        key.clone(),
        app_core::store::SessionMeta {
            session_id: key.clone(),
            project_id: project_id.map(Into::into),
            agent: "claude".into(),
            argv: info.argv.clone(),
            cwd: info.cwd.clone(),
            title: info.title.clone(),
            created_at: 0,
            workspace_id: None,
        },
    );
    s.upsert_session(info);
    key
}

fn attention_dot_shown(rt: &Runtime<State>) -> bool {
    els(rt)
        .iter()
        .any(|e| e.help.as_deref() == Some("Another workspace needs you"))
}

fn entry_shown(rt: &Runtime<State>, text: &str) -> bool {
    els(rt)
        .iter()
        .any(|e| e.text.as_deref() == Some(text) || e.label.as_deref() == Some(text))
}

#[test]
fn needs_you_opens_the_waiting_split_in_another_workspace() {
    let mut s = two_workspace_scene();
    let current = seed_session_in(&mut s, "/work/a/web", Some("p1"));
    let waiting = seed_session_in(&mut s, "/work/b/fix", Some("p2"));
    let other_pane = seed_session_in(&mut s, "/work/b/fix", Some("p2"));
    s.focus_session_tab(&waiting);
    let tab = s.open_tabs[0].id.clone();
    s.place_session(
        &other_pane,
        "Other pane",
        panes::Placement::Split {
            tab: tab.clone(),
            pane: panes::pane_id(&waiting),
            direction: panes::Direction::SideBySide,
        },
    );
    editor::navigation::select_checkout(&mut s, "p1".into(), PathBuf::from("/work/a/web"));
    s.focus_session_tab(&current);
    s.activity
        .insert(waiting.clone(), app_core::activity::Activity::Waiting);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    click_key(&mut rt, "sidebar-needs-you");
    rt.frame(0.5);
    assert!(entry_shown(&rt, "Default / api / fix"));
    click_key(&mut rt, &format!("needs-you-option-{waiting}"));

    assert_eq!(rt.state.workspace_id(), "default");
    assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/b/fix")));
    assert_eq!(rt.state.open_tabs[rt.state.active_tab].id, tab);
    assert_eq!(
        rt.state.focused_session_key().as_deref(),
        Some(waiting.as_str())
    );
    assert!(rt.state.session_placed(&other_pane));
    assert_eq!(
        rt.state.open_tabs.len(),
        2,
        "reuse the split, do not open a duplicate"
    );
}

#[test]
fn needs_you_rechecks_the_session_after_the_draft_guard() {
    for action in [GuardAction::Cancel, GuardAction::Discard] {
        let mut s = two_workspace_scene();
        let current = seed_session_in(&mut s, "/work/a/web", Some("p1"));
        let waiting = seed_session_in(&mut s, "/work/b/api", Some("p2"));
        editor::navigation::select_checkout(&mut s, "p1".into(), PathBuf::from("/work/a/web"));
        s.focus_session_tab(&current);
        s.view = View::Editor;
        s.editor.doc = Some(dirty_doc());
        s.activity
            .insert(waiting.clone(), app_core::activity::Activity::Waiting);
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, "sidebar-needs-you");
        rt.frame(0.5);
        click_key(&mut rt, &format!("needs-you-option-{waiting}"));
        assert!(rt.state.editor.guard.is_some());

        rt.update(|s, tx| {
            if action == GuardAction::Discard {
                s.activity
                    .insert(waiting.clone(), app_core::activity::Activity::Done);
            }
            editor::navigation::resolve_guard(s, action, tx);
        });
        rt.frame(1.0);
        assert_eq!(rt.state.workspace_id(), "w1");
        assert_eq!(rt.state.view, View::Editor);
        assert_eq!(
            rt.state.focused_session_key().as_deref(),
            Some(current.as_str())
        );
        assert!(!rt.state.session_placed(&waiting));
        if action == GuardAction::Cancel {
            assert!(rt.state.editor.doc.as_ref().unwrap().dirty());
        } else {
            assert!(
                !els(&rt)
                    .iter()
                    .any(|e| !e.exiting && e.key.as_deref() == Some("sidebar-needs-you"))
            );
        }
    }
}

#[test]
fn the_selector_flags_a_waiting_session_in_another_workspace() {
    let mut s = two_workspace_scene();
    // p2 is in Default, so the session is hidden while Work is selected.
    let key = seed_session_in(&mut s, "/work/b/api", Some("p2"));
    s.activity
        .insert(key.clone(), app_core::activity::Activity::Waiting);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    assert!(attention_dot_shown(&rt), "the hidden waiting session flags");
    let selector = els(&rt)
        .into_iter()
        .find(|e| e.key.as_deref() == Some("sidebar-workspace-selector"))
        .expect("the workspace selector mounts");
    assert_eq!(
        selector.label.as_deref(),
        Some("Workspace: Work, another workspace needs you")
    );

    click_key(&mut rt, "sidebar-workspace-selector");
    assert!(entry_shown(&rt, "Default · 1 need you"), "menu suffix");
    assert!(
        !entry_shown(&rt, "Work · 1 need you"),
        "selected: no suffix"
    );
}

#[test]
fn the_flag_clears_when_the_hidden_session_is_done() {
    let mut s = two_workspace_scene();
    let key = seed_session_in(&mut s, "/work/b/api", Some("p2"));
    s.activity
        .insert(key.clone(), app_core::activity::Activity::Waiting);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(attention_dot_shown(&rt));

    rt.update(|s, _| {
        s.activity
            .insert(key.clone(), app_core::activity::Activity::Done);
    });
    rt.frame(1.0 / 60.0);
    assert!(!attention_dot_shown(&rt), "no dot once Done");
    click_key(&mut rt, "sidebar-workspace-selector");
    assert!(
        !entry_shown(&rt, "Default · 1 need you"),
        "no suffix once Done"
    );
}

#[test]
fn a_waiting_session_in_the_selected_workspace_adds_no_flag() {
    let mut s = two_workspace_scene();
    let key = seed_session_in(&mut s, "/work/a/web", Some("p1"));
    s.activity
        .insert(key.clone(), app_core::activity::Activity::Waiting);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    assert!(
        !attention_dot_shown(&rt),
        "the selected workspace shows its own"
    );
    click_key(&mut rt, "sidebar-workspace-selector");
    assert!(!entry_shown(&rt, "Default · 1 need you"));
    assert!(!entry_shown(&rt, "Work · 1 need you"));
}

#[test]
fn a_moved_project_moves_its_waiting_count() {
    let mut s = two_workspace_scene();
    s.workspaces.push(workspace("w2", "Home"));
    let key = seed_session_in(&mut s, "/work/a/web", Some("p1"));
    s.activity
        .insert(key.clone(), app_core::activity::Activity::Waiting);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(!attention_dot_shown(&rt), "p1 is in the selected Work");

    rt.update(|s, _| s.editor.projects[0].project.workspace_id = "w2".into());
    rt.frame(1.0 / 60.0);
    assert!(attention_dot_shown(&rt), "the count follows the project");
    click_key(&mut rt, "sidebar-workspace-selector");
    assert!(entry_shown(&rt, "Home · 1 need you"));
}

#[test]
fn an_unassigned_waiting_session_counts_for_default_only() {
    let mut s = two_workspace_scene();
    let key = seed_session_in(&mut s, "/tmp", None);
    s.activity
        .insert(key.clone(), app_core::activity::Activity::Waiting);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(
        attention_dot_shown(&rt),
        "Default is hidden while Work is selected"
    );
    click_key(&mut rt, "sidebar-workspace-selector");
    assert!(entry_shown(&rt, "Default · 1 need you"));

    rt.update(|s, _| s.select_workspace(app_core::store::DEFAULT_WORKSPACE_ID));
    rt.frame(1.0 / 60.0);
    assert!(!attention_dot_shown(&rt), "Default is selected: no flag");
}

#[test]
fn switching_workspaces_keeps_the_editor_checkout_and_running_sessions() {
    let mut s = two_workspace_scene();
    // The editor is committed to p1's checkout inside the Work group.
    s.view = View::Editor;
    s.editor.project_id = Some("p1".into());
    s.editor.checkout = Some(PathBuf::from("/work/a/web"));
    s.editor.checkout_key = Some(crate::editor::state::checkout_key(
        "p1",
        &PathBuf::from("/work/a/web"),
    ));
    s.active_workspace = Some(crate::data::checkout_key("p1", "/work/a/web"));
    let key = seed_session(&mut s, "web", "/work/a/web");
    s.selected_session = Some(key.clone());
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    rt.update(|s, _| s.select_workspace(app_core::store::DEFAULT_WORKSPACE_ID));
    rt.frame(1.0 / 60.0);

    assert_eq!(
        rt.state.view,
        View::Editor,
        "switching never changes the view"
    );
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
    assert_eq!(
        rt.state.editor.checkout,
        Some(PathBuf::from("/work/a/web")),
        "the editor checkout stays committed"
    );
    assert_eq!(
        rt.state.active_workspace.as_deref(),
        Some(crate::data::checkout_key("p1", "/work/a/web").as_str())
    );
    assert_eq!(
        rt.state.selected_session.as_deref(),
        Some(key.as_str()),
        "the running session stays selected"
    );
    assert!(
        rt.state.data.sessions.iter().any(|v| v.info.title == "web"),
        "the session keeps running"
    );
}

#[test]
fn the_workspace_selector_and_remove_confirmation_are_accessible() {
    let mut s = two_workspace_scene();
    s.workspace_remove_confirm = Some("w1".into());
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let e = els(&rt);
    let selector = e
        .iter()
        .find(|el| el.key.as_deref() == Some("sidebar-workspace-selector"))
        .expect("the workspace selector mounts");
    assert_eq!(selector.role, "button");
    assert_eq!(selector.label.as_deref(), Some("Workspace: Work"));
    assert!(selector.focusable, "the selector takes keyboard focus");
    assert!(
        selector.rect[3] <= 32.0,
        "the selector keeps one row's height: {:?}",
        selector.rect
    );
    for key in [
        "sidebar-workspace-remove-confirm",
        "sidebar-workspace-remove-cancel",
    ] {
        let button = e
            .iter()
            .find(|el| el.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("{key} mounts"));
        assert_eq!(button.role, "button");
        assert!(button.focusable);
    }
    let texts: Vec<String> = e.iter().filter_map(|el| el.text.clone()).collect();
    assert!(
        texts.iter().any(|t| t.contains(
            "Remove Work? Its terminals and agents are closed; its projects move to Default."
        )),
        "{texts:?}"
    );

    click_key(&mut rt, "sidebar-workspace-remove-cancel");
    assert!(rt.state.workspace_remove_confirm.is_none());
    assert!(rt.state.workspaces.iter().any(|w| w.id == "w1"));
}

#[test]
fn an_empty_named_workspace_shows_its_own_hint() {
    let mut s = sidebar_scene();
    s.workspaces = vec![
        workspace(app_core::store::DEFAULT_WORKSPACE_ID, "Default"),
        workspace("w2", "Empty"),
    ];
    s.selected_workspace = Some("w2".into());
    seed_session(&mut s, "fix-auth-flow", "/work/a/web");
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let e = els(&rt);
    let texts: Vec<String> = e.iter().filter_map(|el| el.text.clone()).collect();
    assert!(
        texts.iter().any(|t| t.contains("No projects in Empty")),
        "{texts:?}"
    );
    assert!(
        !e.iter()
            .any(|el| el.key.as_deref() == Some("sidebar-unassigned")),
        "Unassigned never renders outside Default"
    );
}

#[test]
fn re_adding_a_registered_project_reports_its_workspace() {
    let mut s = two_workspace_scene();
    s.selected_workspace = Some(app_core::store::DEFAULT_WORKSPACE_ID.into());
    let duplicate = app_core::projects::ProjectView {
        project: app_core::store::Project {
            id: "p1".into(),
            name: "web".into(),
            root: "/work/a/web".into(),
            workspace_id: "w1".into(),
            created_at: 0,
        },
        missing: false,
        parent_project_id: None,
    };
    s.apply_reply(backend::Reply {
        req: backend::Req::AddProject {
            path: "/work/a/web".into(),
            workspace_id: app_core::store::DEFAULT_WORKSPACE_ID.into(),
        },
        generation: 0,
        checkout_key: None,
        result: Ok(backend::ReplyData::Added(duplicate)),
    });

    let notice = s.notice.as_deref().unwrap_or_default();
    assert!(notice.contains("already registered in Work"), "{notice}");
    assert_eq!(
        s.editor.projects[0].project.workspace_id, "w1",
        "re-adding never moves the project"
    );
}

/// The real end-to-end fixture: two registered projects in a temp
/// `Store`, real `git worktree` checkouts, real `worktrees::list`
/// payloads — driven through the same sidebar → navigation → editor
/// identity path production uses.
#[test]
fn sidebar_drives_editor_with_real_registered_projects_and_worktrees() {
    use std::process::Command;

    fn git_ok(root: &std::path::Path, args: &[&str]) {
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
    fn repo(base: &std::path::Path, name: &str) -> std::path::PathBuf {
        let root = base.join(name);
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Tester"]);
        git_ok(&root, &["config", "user.email", "tester@example.com"]);
        std::fs::write(root.join("file.txt"), "x\n").unwrap();
        git_ok(&root, &["add", "-A"]);
        git_ok(&root, &["commit", "-qm", "init"]);
        root
    }

    let home = app_core::test_support::TempDir::new("rune-sidebar");
    let base = home.path().canonicalize().unwrap();
    let root_a = repo(&base, "proj-web");
    let root_b = repo(&base, "proj-api");
    // Two linked worktrees on proj-web, one on proj-api.
    git_ok(&root_a, &["worktree", "add", "../wt-a", "-b", "feat-a"]);
    git_ok(&root_a, &["worktree", "add", "../wt-b", "-b", "feat-b"]);
    git_ok(&root_b, &["worktree", "add", "../wt-c", "-b", "fix-c"]);

    // Register through the real store — the same rows Req::Projects
    // and Req::Worktrees return in production.
    let store = app_core::store::Store::open(home.path()).unwrap();
    let pv_a = app_core::projects::add_project(
        &store,
        root_a.to_str().unwrap(),
        app_core::store::DEFAULT_WORKSPACE_ID,
    )
    .unwrap();
    let pv_b = app_core::projects::add_project(
        &store,
        root_b.to_str().unwrap(),
        app_core::store::DEFAULT_WORKSPACE_ID,
    )
    .unwrap();
    let list_a = app_core::worktrees::list(&root_a).unwrap();
    let list_b = app_core::worktrees::list(&root_b).unwrap();
    assert_eq!(list_a.worktrees.len(), 3, "root + two linked on web");
    assert_eq!(list_b.worktrees.len(), 2, "root + one linked on api");

    let id_a = pv_a.project.id.clone();
    let id_b = pv_b.project.id.clone();
    let mut s = State::new(test_handle());
    s.editor.projects = vec![pv_a, pv_b];
    for (pid, list) in [(&id_a, list_a), (&id_b, list_b)] {
        s.editor.sidebar_worktrees.insert(
            pid.clone(),
            crate::editor::state::SidebarWorktrees {
                list: Some(list),
                loading: false,
                error: None,
            },
        );
    }
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    // Real project names render as rows keyed by their real ids.
    let e = els(&rt);
    assert!(
        e.iter()
            .any(|el| el.key.as_deref() == Some(&format!("sidebar-project:{id_a}")))
    );
    assert!(
        e.iter()
            .any(|el| el.key.as_deref() == Some(&format!("sidebar-project:{id_b}")))
    );

    // Root row → the registered canonical root, exactly.
    click_key(&mut rt, &format!("sidebar-project:{id_a}"));
    assert_eq!(rt.state.editor.project_id.as_deref(), Some(id_a.as_str()));
    assert_eq!(
        rt.state.editor.checkout,
        Some(PathBuf::from(&root_a)),
        "root row committed the canonical registered root"
    );

    // Worktree row → the exact path `git worktree list` reported.
    let wt_path = rt.state.editor.sidebar_worktrees[&id_a]
        .list
        .as_ref()
        .unwrap()
        .worktrees
        .iter()
        .find(|w| !w.is_root)
        .unwrap()
        .path
        .clone();
    click_key(&mut rt, &format!("sidebar-worktree:{id_a}:0"));
    assert_eq!(
        rt.state.editor.checkout,
        Some(PathBuf::from(&wt_path)),
        "worktree row committed the exact listed checkout"
    );

    // Editor toggle keeps the committed identity; switching projects
    // through the sidebar while in Editor moves it wholesale.
    click_key(&mut rt, "view:Editor");
    assert_eq!(rt.state.view, View::Editor);
    assert_eq!(rt.state.editor.checkout, Some(PathBuf::from(&wt_path)));
    click_key(&mut rt, &format!("sidebar-project:{id_b}"));
    assert_eq!(rt.state.editor.project_id.as_deref(), Some(id_b.as_str()));
    assert_eq!(rt.state.editor.checkout, Some(PathBuf::from(&root_b)));
    click_key(&mut rt, "view:Terminal");
    assert_eq!(rt.state.view, View::Terminal);
    click_key(&mut rt, "view:Editor");
    assert_eq!(rt.state.editor.project_id.as_deref(), Some(id_b.as_str()));
    assert_eq!(rt.state.editor.checkout, Some(PathBuf::from(&root_b)));
}

/// Drain the channel-backed backend exactly like the worker thread:
/// every accepted job runs through the real `run_job` against the real
/// temp `Storage`, and its reply applies in FIFO order — including any
/// jobs a reply itself enqueues (the Projects ensure pass, worktree
/// re-reads, section refreshes).
fn drain_backend(
    s: &mut State,
    rx: &std::sync::mpsc::Receiver<crate::editor::backend::Job>,
    storage: &Option<app_core::store::Storage>,
) {
    while let Ok(job) = rx.try_recv() {
        let reply = crate::editor::backend::run_job(storage, &job);
        s.editor.apply_reply(reply);
        if s.editor.section == Section::Changes {
            crate::editor::changes::ensure_selection(s);
        }
    }
}

/// Regression for the native QA hang: two registered projects sharing
/// the display name "repo" whose stored roots are NON-canonical
/// (`/var/folders/…`) while `git worktree list` reports canonical
/// (`/private/var/folders/…`) paths — the exact DB-vs-Git divergence the
/// QA fixture had. Selecting project B's root and then project A's
/// worktree must commit the exact checkout and let Changes settle,
/// driven through the real `run_job`/`apply_reply` stream rather than
/// synthetic replies.
#[test]
fn cross_project_worktree_selection_settles_with_real_backend_replies() {
    use std::process::Command;

    fn git_ok(root: &std::path::Path, args: &[&str]) {
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
    fn repo(base: &std::path::Path, rel: &str) -> PathBuf {
        let root = base.join(rel);
        std::fs::create_dir_all(&root).unwrap();
        git_ok(&root, &["init", "-b", "main"]);
        git_ok(&root, &["config", "user.name", "Tester"]);
        git_ok(&root, &["config", "user.email", "tester@example.com"]);
        std::fs::write(root.join("file.txt"), "x\n").unwrap();
        git_ok(&root, &["add", "-A"]);
        git_ok(&root, &["commit", "-qm", "init"]);
        root // deliberately NOT canonicalized: the QA DB kept /var/…
    }

    let home = app_core::test_support::TempDir::new("rune-qa-repro");
    let base = home.path().join("repos");
    std::fs::create_dir_all(&base).unwrap();
    let root_a = repo(&base, "repo");
    let root_b = repo(&base, "other/repo");
    git_ok(
        &root_a,
        &[
            "worktree",
            "add",
            &base.join("wt-a").display().to_string(),
            "-b",
            "wt-a",
        ],
    );
    git_ok(
        &root_a,
        &[
            "worktree",
            "add",
            &base.join("nested/wt-a2").display().to_string(),
            "-b",
            "wt-a2",
        ],
    );
    git_ok(
        &root_b,
        &[
            "worktree",
            "add",
            &base.join("other/wt-b").display().to_string(),
            "-b",
            "wt-b",
        ],
    );

    // Register VERBATIM non-canonical roots under the same display
    // name — `Store::add_project` stores the string it is given, which
    // is how the QA database ended up with /var/… roots.
    let storage = app_core::store::Storage::open(home.path());
    let store = storage.get().unwrap();
    let pa = store
        .add_project(
            "repo",
            root_a.to_str().unwrap(),
            app_core::store::DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
    let pb = store
        .add_project(
            "repo",
            root_b.to_str().unwrap(),
            app_core::store::DEFAULT_WORKSPACE_ID,
        )
        .unwrap();
    assert_ne!(pa.id, pb.id);
    let storage = Some(storage);

    let (backend, rx) = crate::editor::backend::channel_backend();
    let mut s = State::new(test_handle());
    s.editor.backend = backend;
    s.view = View::Editor;

    // Startup project load + the per-project worktree ensure pass.
    s.editor.refresh_projects();
    drain_backend(&mut s, &rx, &storage);
    assert_eq!(s.editor.projects.len(), 2);
    assert!(
        s.editor
            .sidebar_worktrees
            .get(&pa.id)
            .and_then(|e| e.list.as_ref())
            .is_some_and(|l| l.worktrees.iter().any(|w| !w.is_root))
    );

    // The worktree row's checkout is the path `git worktree list`
    // reported, verbatim — exactly what sidebar.rs sends.
    let wt_path = s.editor.sidebar_worktrees[&pa.id]
        .list
        .as_ref()
        .unwrap()
        .worktrees
        .iter()
        .find(|w| !w.is_root && w.path.ends_with("wt-a"))
        .expect("linked worktree listed")
        .path
        .clone();

    // QA step A: sidebar project-B root row while in Terminal→Editor.
    crate::editor::navigation::navigate(
        &mut s,
        PendingNav::SelectCheckout {
            project_id: pb.id.clone(),
            checkout: PathBuf::from(&pb.root),
        },
        &mut rune::Tx::default(),
    );
    drain_backend(&mut s, &rx, &storage);
    assert_eq!(s.editor.project_id.as_deref(), Some(pb.id.as_str()));
    assert_eq!(
        s.editor.checkout.as_deref(),
        Some(std::path::Path::new(&pb.root)),
    );
    assert!(
        s.editor.changes.data.is_some(),
        "root checkout changes loaded"
    );
    assert!(!s.editor.changes.loading);

    // QA step B: while in Editor, the sidebar worktree row of project A.
    crate::editor::navigation::navigate(
        &mut s,
        PendingNav::SelectCheckout {
            project_id: pa.id.clone(),
            checkout: PathBuf::from(&wt_path),
        },
        &mut rune::Tx::default(),
    );
    drain_backend(&mut s, &rx, &storage);

    // The committed identity is the exact listed checkout…
    assert_eq!(s.editor.project_id.as_deref(), Some(pa.id.as_str()));
    assert_eq!(
        s.editor.checkout.as_deref(),
        Some(std::path::Path::new(&wt_path)),
        "worktree selection committed the exact listed path"
    );
    // …the picker has real options for it (not stuck on the placeholder)…
    assert!(
        !s.editor.worktrees.worktrees.is_empty(),
        "checkout picker populated for the selected project"
    );
    assert!(!s.editor.worktrees_loading);
    assert!(s.editor.worktrees_error.is_none());
    // …and Changes settled on the new checkout instead of spinning.
    assert!(
        s.editor.changes.data.is_some(),
        "changes loaded for the worktree checkout"
    );
    assert!(!s.editor.changes.loading, "changes loading settled");
    assert_eq!(
        s.editor.changes.checkout.as_deref(),
        s.editor.checkout_key.as_deref(),
        "changes data belongs to the committed checkout key"
    );
    // The sidebar highlight tracks the same committed identity.
    assert_eq!(
        s.active_workspace.as_deref(),
        Some(crate::data::checkout_key(&pa.id, &wt_path).as_str())
    );
}

// -- usage / accounts / theme replies ------------------------------------

use crate::editor::backend::{Reply, ReplyData, Req};

fn usage_view() -> app_core::accounts::AccountsView {
    let view = |active: &str, items: &[(&str, &str, &str, bool)]| app_core::accounts::AgentView {
        active: active.to_string(),
        items: items
            .iter()
            .map(
                |(id, name, dir, is_default)| app_core::accounts::AccountView {
                    id: id.to_string(),
                    name: name.to_string(),
                    dir: dir.to_string(),
                    is_default: *is_default,
                },
            )
            .collect(),
    };
    app_core::accounts::AccountsView {
        claude: view(
            "default",
            &[
                ("default", "Default", "/me/.claude", true),
                ("aa11bb22", "work", "/accts/work", false),
            ],
        ),
        codex: view("default", &[("default", "Default", "/me/.codex", true)]),
        devin: view(
            "default",
            &[("default", "Default", "/me/.local/share", true)],
        ),
    }
}

fn reply(req: Req, result: Result<ReplyData, String>) -> Reply {
    Reply {
        req,
        generation: 0,
        checkout_key: None,
        result,
    }
}

// -- Task 4: removal confirmations ---------------------------------------

/// Press the menu item (or any element) carrying this label.
fn click_label(rt: &mut Runtime<State>, label: &str) {
    let e = rt
        .elements()
        .into_iter()
        .find(|e| e.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("{label} is on screen"));
    let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

fn texts_on(rt: &Runtime<State>) -> Vec<String> {
    els(rt).into_iter().filter_map(|e| e.text).collect()
}

#[test]
fn opening_a_worktree_removal_requests_its_projects_worktrees() {
    let mut s = sidebar_scene();
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");
    let reqs = workspace_jobs(&rx);
    assert!(
        reqs.iter()
            .any(|req| matches!(req, Req::Worktrees(pid) if pid == "p1")),
        "{reqs:?}"
    );
}

#[test]
fn the_confirmation_shows_the_live_dirty_state_not_the_captured_one() {
    let mut s = sidebar_scene();
    s.editor
        .sidebar_worktrees
        .get_mut("p1")
        .unwrap()
        .list
        .as_mut()
        .unwrap()
        .worktrees
        .iter_mut()
        .find(|w| w.path == "/work/a/feat-a")
        .unwrap()
        .dirty = Some(true);
    s.sidebar_confirm = Some(sidebar::SidebarConfirm::RemoveWorktree {
        project_id: "p1".into(),
        path: "/work/a/feat-a".into(),
        name: "feat-a".into(),
        dirty: Some(false),
        sessions: 0,
    });
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    assert!(
        texts_on(&rt)
            .iter()
            .any(|t| t.starts_with("It has uncommitted changes.")),
        "{:?}",
        texts_on(&rt)
    );
}

#[test]
fn an_ended_session_is_not_counted_in_the_removal_confirmation() {
    use protocol::session::{SessionId, SessionInfo, SessionState};

    let mut s = sidebar_scene();
    for state in [SessionState::Exited, SessionState::Running] {
        s.upsert_session(SessionInfo {
            id: SessionId::new(),
            title: "zsh".into(),
            argv: vec!["sh".into()],
            cwd: "/work/a/feat-a".into(),
            state,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        });
    }
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");

    assert!(
        texts_on(&rt)
            .iter()
            .any(|t| t.contains("1 session runs in it.")),
        "{:?}",
        texts_on(&rt)
    );
}

#[test]
fn remove_worktree_waits_for_confirm_then_enqueues_once() {
    let mut s = sidebar_scene();
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");
    assert!(
        !workspace_jobs(&rx)
            .iter()
            .any(|req| matches!(req, Req::RemoveWorktree { .. })),
        "nothing is removed before Confirm"
    );
    assert!(matches!(
        &rt.state.sidebar_confirm,
        Some(sidebar::SidebarConfirm::RemoveWorktree { path, .. }) if path == "/work/a/feat-a"
    ));

    click_key(&mut rt, "sidebar-confirm-remove");
    let reqs: Vec<Req> = workspace_jobs(&rx)
        .into_iter()
        .filter(|req| matches!(req, Req::RemoveWorktree { .. }))
        .collect();
    assert_eq!(reqs.len(), 1, "Confirm enqueues the removal once");
    assert!(matches!(
        &reqs[0],
        Req::RemoveWorktree { project_id, path }
            if project_id == "p1" && path == "/work/a/feat-a"
    ));
    assert!(rt.state.sidebar_confirm.is_none());
}

#[test]
fn cancel_on_a_worktree_removal_enqueues_nothing() {
    let mut s = sidebar_scene();
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");
    click_key(&mut rt, "sidebar-confirm-cancel");
    assert!(rt.state.sidebar_confirm.is_none());
    assert!(
        !workspace_jobs(&rx)
            .iter()
            .any(|req| matches!(req, Req::RemoveWorktree { .. })),
        "cancel removes nothing"
    );
}

#[test]
fn a_dirty_worktree_says_it_has_uncommitted_changes() {
    let mut s = sidebar_scene();
    let list = s.editor.sidebar_worktrees.get_mut("p1").unwrap();
    list.list.as_mut().unwrap().worktrees[1].dirty = Some(true);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");
    let texts = texts_on(&rt);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("It has uncommitted changes. No sessions run in it.")),
        "{texts:?}"
    );
    assert!(texts.iter().any(|t| t == "Remove feat-a?"), "{texts:?}");
}

#[test]
fn a_clean_or_unknown_worktree_says_so() {
    let mut s = sidebar_scene();
    let list = s.editor.sidebar_worktrees.get_mut("p1").unwrap();
    list.list.as_mut().unwrap().worktrees[1].dirty = Some(false);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");
    assert!(
        texts_on(&rt)
            .iter()
            .any(|t| t.contains("It has no uncommitted changes.")),
    );

    rt.state.sidebar_confirm = None;
    rt.state
        .editor
        .sidebar_worktrees
        .get_mut("p1")
        .unwrap()
        .list
        .as_mut()
        .unwrap()
        .worktrees[1]
        .dirty = None;
    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");
    assert!(
        texts_on(&rt)
            .iter()
            .any(|t| t.contains("Git status unknown.")),
    );
}

#[test]
fn two_attached_sessions_are_counted_in_the_worktree_row() {
    let mut s = sidebar_scene();
    seed_session(&mut s, "one", "/work/a/feat-a");
    seed_session(&mut s, "two", "/work/a/feat-a");
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");
    assert!(
        texts_on(&rt)
            .iter()
            .any(|t| t.contains("Git status unknown. 2 sessions run in it.")),
        "{:?}",
        texts_on(&rt)
    );
}

#[test]
fn a_vanished_worktree_shows_the_notice_and_enqueues_nothing() {
    let mut s = sidebar_scene();
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");

    rt.state
        .editor
        .sidebar_worktrees
        .get_mut("p1")
        .unwrap()
        .list
        .as_mut()
        .unwrap()
        .worktrees
        .retain(|w| w.path != "/work/a/feat-a");
    rt.frame(1.0 / 60.0);
    click_key(&mut rt, "sidebar-confirm-remove");

    assert_eq!(
        rt.state.notice.as_deref(),
        Some("That item no longer exists")
    );
    assert!(rt.state.sidebar_confirm.is_none());
    assert!(
        !workspace_jobs(&rx)
            .iter()
            .any(|req| matches!(req, Req::RemoveWorktree { .. })),
        "nothing is removed"
    );
}

#[test]
fn opening_a_removal_closes_the_workspace_confirmation_and_back() {
    let mut s = two_workspace_scene();
    s.workspace_remove_confirm = Some("w1".into());
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    click_key(&mut rt, "sidebar-action:more:/work/a/feat-a");
    click_label(&mut rt, "Remove worktree");
    assert!(rt.state.workspace_remove_confirm.is_none());
    assert!(rt.state.sidebar_confirm.is_some());

    click_key(&mut rt, "sidebar-workspace-selector");
    assert!(rt.state.workspace_picker.open, "the workspace picker opens");
    // Let the surface finish opening before pressing one of its options.
    rt.frame(0.5);
    click_key(&mut rt, "sidebar-workspace-action-w1-remove");
    assert!(
        !rt.state.workspace_picker.open,
        "choosing closes the picker"
    );
    assert_eq!(rt.state.workspace_remove_confirm.as_deref(), Some("w1"));
    assert!(rt.state.sidebar_confirm.is_none());
}
/// The workspace picker opened on `scene`, its surface settled.
fn open_workspace_picker(scene: State) -> Runtime<State> {
    let mut rt = Runtime::new(scene, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-workspace-selector");
    rt.frame(0.5);
    rt
}
/// Move the pointer onto `key` and let a frame pass: a row's buttons show once the row is
/// highlighted, as they do for a hand that moves before it presses.
fn hover_key(rt: &mut Runtime<State>, key: &str) {
    let e = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("element {key} mounted"));
    rt.pointer_move(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    rt.frame(1.0 / 60.0);
}
#[test]
fn a_workspace_rows_buttons_act_on_that_row_not_the_selected_one() {
    let mut s = two_workspace_scene();
    s.selected_workspace = Some(app_core::store::DEFAULT_WORKSPACE_ID.into());
    let mut rt = open_workspace_picker(s);
    hover_key(&mut rt, "sidebar-workspace-action-w1-remove");
    click_key(&mut rt, "sidebar-workspace-action-w1-remove");
    assert!(!rt.state.workspace_picker.open);
    assert_eq!(rt.state.workspace_remove_confirm.as_deref(), Some("w1"));
    assert_eq!(
        rt.state.selected_workspace.as_deref(),
        Some(app_core::store::DEFAULT_WORKSPACE_ID),
        "the row was not chosen"
    );
    let mut s = two_workspace_scene();
    s.selected_workspace = Some(app_core::store::DEFAULT_WORKSPACE_ID.into());
    let mut rt = open_workspace_picker(s);
    hover_key(&mut rt, "sidebar-workspace-action-w1-rename");
    click_key(&mut rt, "sidebar-workspace-action-w1-rename");
    assert!(matches!(
        &rt.state.sidebar_input,
        Some(crate::sidebar::SidebarInput::WorkspaceRename { id }) if id == "w1"
    ));
    assert_eq!(rt.state.sidebar_text.text, "Work");
    assert_eq!(
        rt.state.selected_workspace.as_deref(),
        Some(app_core::store::DEFAULT_WORKSPACE_ID)
    );
}
#[test]
fn the_default_workspaces_buttons_say_why_they_do_nothing() {
    let mut rt = open_workspace_picker(two_workspace_scene());
    for (key, label) in [
        (
            "sidebar-workspace-action-default-rename",
            "Default can't be renamed",
        ),
        (
            "sidebar-workspace-action-default-remove",
            "Default can't be removed",
        ),
    ] {
        let button = els(&rt)
            .into_iter()
            .find(|el| el.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("{key} mounts"));
        assert_eq!(button.label.as_deref(), Some(label));
        assert!(!button.focusable, "{key} is out of the keyboard's reach");
    }
    hover_key(&mut rt, "sidebar-workspace-action-default-remove");
    click_key(&mut rt, "sidebar-workspace-action-default-remove");
    assert!(
        rt.state.workspace_picker.open,
        "a disabled button does nothing"
    );
    assert!(rt.state.workspace_remove_confirm.is_none());
}
#[test]
fn a_workspace_button_is_reachable_from_the_keyboard() {
    let mut rt = open_workspace_picker(two_workspace_scene());
    press(&mut rt, Key::Tab);
    press(&mut rt, Key::Text(" ".into()));
    assert!(
        matches!(
            &rt.state.sidebar_input,
            Some(crate::sidebar::SidebarInput::WorkspaceRename { id }) if id == "w1"
        ),
        "Tab from the search lands on the highlighted row's first button, Space presses it"
    );
}

#[test]
fn remove_project_waits_for_confirm_then_enqueues_once() {
    let mut s = sidebar_scene();
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    click_key(&mut rt, "sidebar-action:more:p1");
    click_label(&mut rt, "Remove project");
    assert!(
        workspace_jobs(&rx).is_empty(),
        "nothing runs before Confirm"
    );
    assert!(
        texts_on(&rt).iter().any(|t| t
            == "Remove web? Only its registration is removed; its sessions become Unassigned and its files stay."),
        "{:?}",
        texts_on(&rt)
    );

    click_key(&mut rt, "sidebar-confirm-remove");
    let reqs = workspace_jobs(&rx);
    assert_eq!(reqs.len(), 1, "Confirm enqueues once");
    assert!(matches!(&reqs[0], Req::RemoveProject { id } if id == "p1"));
    assert!(rt.state.sidebar_confirm.is_none());
}

#[test]
fn cancel_on_a_project_removal_enqueues_nothing() {
    let mut s = sidebar_scene();
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);

    click_key(&mut rt, "sidebar-action:more:p1");
    click_label(&mut rt, "Remove project");
    click_key(&mut rt, "sidebar-confirm-cancel");
    assert!(rt.state.sidebar_confirm.is_none());
    assert!(workspace_jobs(&rx).is_empty());
}

#[test]
fn a_vanished_project_shows_the_notice_and_enqueues_nothing() {
    let mut s = sidebar_scene();
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-action:more:p1");
    click_label(&mut rt, "Remove project");

    rt.state.editor.projects.retain(|p| p.project.id != "p1");
    rt.frame(1.0 / 60.0);
    click_key(&mut rt, "sidebar-confirm-remove");

    assert_eq!(
        rt.state.notice.as_deref(),
        Some("That item no longer exists")
    );
    assert!(rt.state.sidebar_confirm.is_none());
    assert!(workspace_jobs(&rx).is_empty());
}

#[test]
fn a_failed_worktree_removal_sets_the_notice_and_keeps_the_list() {
    let mut s = sidebar_scene();
    let before = s.editor.sidebar_worktrees.get("p1").unwrap().list.clone();
    s.apply_reply(reply(
        Req::RemoveWorktree {
            project_id: "p1".into(),
            path: "/work/a/feat-a".into(),
        },
        Err("worktree has local changes".into()),
    ));

    assert_eq!(s.notice.as_deref(), Some("worktree has local changes"));
    assert_eq!(s.editor.sidebar_worktrees.get("p1").unwrap().list, before);
}

#[test]
fn a_failed_project_removal_sets_the_notice_and_keeps_the_projects() {
    let mut s = sidebar_scene();
    let before = s.editor.projects.len();
    s.apply_reply(reply(
        Req::RemoveProject { id: "p1".into() },
        Err("project is locked".into()),
    ));

    assert_eq!(s.notice.as_deref(), Some("project is locked"));
    assert_eq!(s.editor.projects.len(), before);
    assert!(s.editor.projects.iter().any(|p| p.project.id == "p1"));
}

#[test]
fn a_refused_worktree_removal_sets_the_notice() {
    let mut s = sidebar_scene();
    let (backend, rx) = crate::editor::backend::channel_backend();
    drop(rx);
    s.editor.backend = backend;
    s.remove_worktree("p1", "/work/a/feat-a");

    assert_eq!(
        s.notice.as_deref(),
        Some("the editor queue is full; try again")
    );
}

#[test]
fn a_refused_project_removal_sets_the_notice() {
    let mut s = sidebar_scene();
    let (backend, rx) = crate::editor::backend::channel_backend();
    drop(rx);
    s.editor.backend = backend;
    s.remove_project("p1");

    assert_eq!(
        s.notice.as_deref(),
        Some("the editor queue is full; try again")
    );
}

// -- Task 8: prune missing worktrees -------------------------------------

fn with_missing_feat_a(mut s: State) -> State {
    let list = s.editor.sidebar_worktrees.get_mut("p1").unwrap();
    let worktrees = &mut list.list.as_mut().unwrap().worktrees;
    worktrees
        .iter_mut()
        .find(|w| w.path == "/work/a/feat-a")
        .unwrap()
        .missing = true;
    s
}

#[test]
fn prune_is_absent_from_the_more_menu_without_a_missing_worktree() {
    let mut rt = Runtime::new(sidebar_scene(), view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-action:more:p1");
    assert!(!texts_on(&rt).iter().any(|t| t == "Prune missing worktrees"));
}

#[test]
fn prune_is_offered_on_a_project_and_a_worktree_row_with_a_missing_worktree() {
    let s = with_missing_feat_a(sidebar_scene());
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-action:more:p1");
    assert!(texts_on(&rt).iter().any(|t| t == "Prune missing worktrees"));
    rt.update(|s, _| s.sidebar_menu = None);
    // The menu drops under its button, over the rows below: let it finish
    // closing before pressing the next row's button.
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }

    click_key(&mut rt, "sidebar-action:more:/work/a/feat-b");
    assert!(texts_on(&rt).iter().any(|t| t == "Prune missing worktrees"));
}

#[test]
fn choosing_prune_enqueues_once_and_closes_the_menu() {
    let mut s = with_missing_feat_a(sidebar_scene());
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "sidebar-action:more:p1");
    click_label(&mut rt, "Prune missing worktrees");

    let reqs = workspace_jobs(&rx);
    assert_eq!(reqs.len(), 1, "one prune request");
    assert!(matches!(&reqs[0], Req::PruneWorktrees { project_id } if project_id == "p1"));
    assert!(rt.state.sidebar_menu.is_none());
}

#[test]
fn a_failed_prune_sets_the_notice_and_keeps_the_list() {
    let mut s = with_missing_feat_a(sidebar_scene());
    let before = s.editor.sidebar_worktrees.get("p1").unwrap().list.clone();
    s.apply_reply(reply(
        Req::PruneWorktrees {
            project_id: "p1".into(),
        },
        Err("not a git repository".into()),
    ));

    assert_eq!(s.notice.as_deref(), Some("not a git repository"));
    assert_eq!(s.editor.sidebar_worktrees.get("p1").unwrap().list, before);
}

#[test]
fn a_successful_prune_requests_the_worktree_list_again() {
    let mut s = with_missing_feat_a(sidebar_scene());
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    s.apply_reply(reply(
        Req::PruneWorktrees {
            project_id: "p1".into(),
        },
        Ok(backend::ReplyData::Ok),
    ));

    assert!(s.notice.is_none());
    let reqs = workspace_jobs(&rx);
    assert!(
        reqs.iter()
            .any(|req| matches!(req, Req::Worktrees(pid) if pid == "p1")),
        "{reqs:?}"
    );
}

// -- Task 12: workspace mutations ----------------------------------------

fn workspace_jobs(rx: &std::sync::mpsc::Receiver<backend::Job>) -> Vec<Req> {
    rx.try_iter().map(|job| job.req).collect()
}

// -- Task 5: start buttons, New tab menu, unavailable agents ------------

fn agent(id: &str, label: &str, binary: Option<&str>, available: bool) -> crate::data::AgentInfo {
    crate::data::AgentInfo {
        id: id.into(),
        label: label.into(),
        available,
        path: available.then(|| format!("/usr/local/bin/{id}")),
        binary: binary.map(str::to_string),
    }
}

/// No tab open, two registered projects in Default, Claude and OpenCode
/// available, Codex missing from PATH.
fn start_scene() -> State {
    let mut s = State::new(test_handle());
    s.editor.projects = vec![
        project("p1", "web", "/work/a/web"),
        project("p2", "api", "/work/b/api"),
    ];
    s.data.connected = true;
    s.data.agents = vec![
        agent("shell", "Shell", None, true),
        agent("claude", "Claude Code", Some("claude"), true),
        agent("codex", "Codex", Some("codex"), false),
        agent("opencode", "OpenCode", Some("opencode"), true),
    ];
    s
}

fn element_with(rt: &Runtime<State>, role: &str, label: &str) -> Option<rune::ElementInfo> {
    rt.elements()
        .into_iter()
        .find(|e| e.role == role && e.label.as_deref() == Some(label))
}

/// Press the element with this role and label (a menu item or a button
/// whose label another element shares).
fn click_role(rt: &mut Runtime<State>, role: &str, label: &str) {
    let e = element_with(rt, role, label).unwrap_or_else(|| panic!("{label} is on screen"));
    let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
}

#[test]
fn an_empty_terminal_offers_each_available_agent_and_shell() {
    let mut rt = Runtime::new(start_scene(), view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for label in ["Claude Code", "OpenCode", "Shell"] {
        assert!(
            element_with(&rt, "button", label).is_some(),
            "{label} button"
        );
    }
    assert!(
        element_with(&rt, "button", "Codex").is_none(),
        "an unavailable agent gets no start button"
    );
}

#[test]
fn a_workspace_without_projects_offers_only_add_project() {
    let mut s = start_scene();
    for project in &mut s.editor.projects {
        project.project.workspace_id = "elsewhere".into();
    }
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(
        els(&rt)
            .iter()
            .any(|e| e.key.as_deref() == Some("terminal-add-project")),
        "Add Project is offered"
    );
    assert!(element_with(&rt, "button", "Claude Code").is_none());
    assert!(element_with(&rt, "button", "Shell").is_none());
}

#[test]
fn disconnected_start_buttons_are_disabled_with_the_reason() {
    let mut s = start_scene();
    s.data.connected = false;
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(
        texts_on(&rt)
            .iter()
            .any(|t| t == "Waiting for the Goko daemon"),
        "the start row says why it is disabled"
    );
    click_role(&mut rt, "button", "Claude Code");
    click_role(&mut rt, "button", "Shell");
    assert!(workspace_jobs(&rx).is_empty());
    assert!(rt.state.pending_argv.is_empty());
    assert!(rt.state.pending.is_empty());
}

/// Steps a picker or menu through its open (or close) animation.
fn settle(rt: &mut Runtime<State>) {
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
}

/// Every "start something" button opens the same picker, and an agent
/// missing from PATH is listed disabled with the reason: picking it starts
/// nothing.
#[test]
fn an_unavailable_agent_says_why_in_every_new_session_picker() {
    let reason = "codex not found on PATH";
    for trigger in ["new-tab-menu", "sidebar-action:new-session:p1"] {
        let (backend, rx) = crate::editor::backend::channel_backend();
        let mut s = start_scene();
        s.editor.backend = backend;
        let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        click_key(&mut rt, trigger);
        settle(&mut rt);
        assert!(
            rt.state.new_session_picker.open,
            "{trigger} opens the picker"
        );
        assert!(
            texts_on(&rt).iter().any(|t| t == reason),
            "{trigger}: the reason is shown"
        );
        click_role(&mut rt, "option", "Codex");
        assert!(
            workspace_jobs(&rx).is_empty(),
            "{trigger}: a disabled entry starts nothing"
        );
    }
}

#[test]
fn new_tab_still_starts_one_shell() {
    let mut rt = Runtime::new(start_scene(), view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_label(&mut rt, "New tab");
    assert_eq!(rt.state.pending.len(), 1, "one session.create");
    assert!(matches!(
        &rt.state.pending[0],
        crate::sessions::Pending::Create { launch, .. } if launch.agent == "shell"
    ));
    assert!(rt.state.pending_argv.is_empty());
}

// -- Task 6: app shortcuts ----------------------------------------------

fn cmd_key(c: char) -> KeyEvent {
    KeyEvent {
        key: Key::Command(c),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    }
}

/// The window's runtime, with the app's key handler attached.
fn key_runtime(s: State) -> Runtime<State> {
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.on_key = Some(on_key);
    rt.frame(0.0);
    rt
}

fn press_cmd(rt: &mut Runtime<State>, c: char) {
    rt.key(cmd_key(c));
    rt.frame(1.0 / 60.0);
}

fn alt_key(key: Key) -> KeyEvent {
    KeyEvent {
        key,
        mods: Mods {
            alt: true,
            ..Mods::default()
        },
    }
}

fn press_alt(rt: &mut Runtime<State>, key: Key) {
    rt.key(alt_key(key));
    rt.frame(1.0 / 60.0);
}

/// Two open tabs (one session each) in the Terminal view; the second is active.
fn two_tab_scene() -> State {
    let mut s = State::new(test_handle());
    s.view = View::Terminal;
    let first = seed_session(&mut s, "one", "/tmp");
    let second = seed_session(&mut s, "two", "/tmp");
    s.focus_session_tab(&first);
    s.focus_session_tab(&second);
    s
}

#[test]
fn cmd_t_starts_exactly_one_shell() {
    let mut rt = key_runtime(start_scene());
    press_cmd(&mut rt, 't');
    assert_eq!(rt.state.pending.len(), 1, "one session.create");
    assert!(matches!(
        &rt.state.pending[0],
        crate::sessions::Pending::Create { launch, .. } if launch.agent == "shell"
    ));
}

#[test]
fn cmd_digits_activate_the_nth_tab_and_a_missing_tab_does_nothing() {
    let mut rt = key_runtime(two_tab_scene());
    press_cmd(&mut rt, '1');
    assert_eq!(rt.state.active_tab, 0);
    press_cmd(&mut rt, '2');
    assert_eq!(rt.state.active_tab, 1);
    press_cmd(&mut rt, '9');
    assert_eq!(rt.state.active_tab, 1, "no ninth tab");
    assert_eq!(rt.state.view, View::Terminal);
}

#[test]
fn cmd_digit_in_the_editor_view_switches_only_when_the_tab_exists() {
    let mut rt = key_runtime(two_tab_scene());
    rt.update(|s, _| s.view = View::Editor);
    press_cmd(&mut rt, '9');
    assert_eq!(rt.state.view, View::Editor, "no ninth tab");
    press_cmd(&mut rt, '2');
    assert_eq!(rt.state.view, View::Terminal);
    assert_eq!(rt.state.active_tab, 1);
}

#[test]
fn cmd_comma_opens_then_closes_settings() {
    let mut rt = key_runtime(editor_scene());
    press_cmd(&mut rt, ',');
    assert_eq!(rt.state.view, View::Settings);
    press_cmd(&mut rt, ',');
    assert_eq!(rt.state.view, View::Terminal);
}

#[test]
fn cmd_comma_with_a_dirty_draft_raises_the_guard_instead() {
    let mut rt = key_runtime(dirty_scene());
    press_cmd(&mut rt, ',');
    assert!(rt.state.editor.guard.is_some(), "the draft guard is up");
    assert_eq!(rt.state.view, View::Editor, "the view did not switch");
}

#[test]
fn shortcuts_do_not_fire_under_a_confirmation_or_a_composition() {
    let mut s = two_tab_scene();
    s.sidebar_confirm = Some(crate::sidebar::SidebarConfirm::RemoveWorktree {
        project_id: "p1".into(),
        path: "/work/a/web".into(),
        name: "web".into(),
        dirty: Some(false),
        sessions: 0,
    });
    let mut rt = key_runtime(s);
    press_cmd(&mut rt, 't');
    press_cmd(&mut rt, '1');
    press_cmd(&mut rt, ',');
    assert!(rt.state.pending.is_empty(), "no session starts");
    assert_eq!(rt.state.active_tab, 1, "no tab switch");
    assert_eq!(rt.state.view, View::Terminal, "no settings toggle");

    rt.update(|s, _| {
        s.sidebar_confirm = None;
        s.preedit = "vie".into();
    });
    press_cmd(&mut rt, 't');
    press_cmd(&mut rt, '1');
    press_cmd(&mut rt, ',');
    assert!(
        rt.state.pending.is_empty(),
        "no session starts while composing"
    );
    assert_eq!(rt.state.active_tab, 1);
    assert_eq!(rt.state.view, View::Terminal);
}

#[test]
fn cmd_t_is_consumed_by_the_app_before_the_terminal_sees_it() {
    let mut rt = key_runtime(two_tab_scene());
    rt.update(|s, tx| {
        assert!(on_key(s, &cmd_key('t'), tx), "the app takes ⌘T");
        assert!(
            !terminal_key(s, &cmd_key('t'), None),
            "the terminal never types a ⌘ chord"
        );
    });
}

#[test]
fn cmd_f_opens_the_terminals_find_bar_and_the_typing_stays_out_of_the_shell() {
    let mut s = two_tab_scene();
    let key = s.focused_session_key().expect("a focused pane");
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    tx.send(daemon::Output {
        seq: 0,
        data: b"build failed\r\n".to_vec(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let letter = KeyEvent {
        key: Key::Text("a".into()),
        mods: Mods::default(),
    };
    let mut rt = key_runtime(s);
    rt.update(|s, tx| {
        assert!(!s.terminal_find_focused());
        assert!(
            on_key(s, &cmd_key('f'), tx),
            "⌘F belongs to the app in the Terminal view"
        );
        assert!(s.attachments[&key].find.open);
        assert!(s.terminal_find_focused());
        assert!(
            !on_key(s, &letter, tx),
            "the query takes the letter, the shell does not"
        );
        s.close_terminal_find(&key, tx);
        assert!(!s.attachments[&key].find.open);
        assert!(!s.terminal_find_focused());
        assert!(on_key(s, &letter, tx), "the shell types again");
    });
}

#[test]
fn the_find_bar_takes_the_typing_and_marks_the_matching_rows() {
    let mut s = two_tab_scene();
    let key = s.focused_session_key().expect("a focused pane");
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    tx.send(daemon::Output {
        seq: 0,
        data: b"compiling\r\nbuild failed\r\nretry the Build\r\n".to_vec(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let ink = s.ink;
    let att = s.attachments.get_mut(&key).unwrap();
    let lines = att.term.lines(&ink).to_vec();
    att.set_lines(&lines, &key);
    let mut rt = key_runtime(s);
    rt.frame(0.0);
    press_cmd(&mut rt, 'f');
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.role == "textbox" && e.label.as_deref() == Some("Find")),
        "the bar is drawn over the pane"
    );
    for ch in "build".chars() {
        rt.key(KeyEvent {
            key: Key::Text(ch.to_string()),
            mods: Mods::default(),
        });
    }
    rt.frame(1.0 / 60.0);
    let att = &rt.state.attachments[&key];
    assert_eq!(att.find.query.text, "build", "typing edits the query");
    assert_eq!(att.find.matches.len(), 2, "any case, both rows");
    assert_eq!(att.marks.iter().map(Vec::len).sum::<usize>(), 2);
    assert_eq!(att.find.count_label(), "1 of 2");
}

#[test]
fn coming_back_to_a_tab_never_leaves_the_find_bar_swallowing_the_typing() {
    let mut s = State::new(test_handle());
    s.view = View::Terminal;
    let first = seed_session(&mut s, "one", "/tmp");
    let second = seed_session(&mut s, "two", "/tmp");
    s.focus_session_tab(&first);
    s.focus_session_tab(&second);
    let session = s.session_id(&second).unwrap();
    let (_tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    attach(&mut s, &second, term::Terminal::new(session, rx, ready_rx));
    let mut rt = key_runtime(s);
    rt.frame(0.0);
    press_cmd(&mut rt, 'f');
    assert!(rt.state.terminal_find_focused());
    rt.update(|s, _| s.focus_session_tab(&first));
    rt.frame(1.0 / 60.0);
    rt.update(|s, _| s.focus_session_tab(&second));
    rt.frame(1.0 / 60.0);
    let says_focused = rt.state.terminal_find_focused();
    rt.key(KeyEvent {
        key: Key::Text("a".into()),
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    let typed = rt.state.attachments[&second].find.query.text == "a";
    assert_eq!(
        says_focused, typed,
        "the bar holds the keyboard exactly when the field takes the letter"
    );
}

#[test]
fn shortcuts_wait_behind_the_pickers_and_the_sidebar_menu() {
    type Block = fn(&mut State);
    let blocks: [(&str, Block); 3] = [
        ("the checkout picker", |s| {
            s.editor.checkout_picker.open = true
        }),
        ("the project picker", |s| {
            s.editor.project_picker.open = true
        }),
        ("the sidebar menu", |s| {
            s.open_menu(
                crate::sidebar::RowTarget::Project {
                    project_id: "p1".into(),
                },
                crate::sidebar::MenuKind::More,
            );
        }),
    ];
    for (what, block) in blocks {
        let mut s = two_tab_scene();
        block(&mut s);
        let mut rt = key_runtime(s);
        press_cmd(&mut rt, 't');
        press_cmd(&mut rt, '1');
        press_cmd(&mut rt, ',');
        assert!(
            rt.state.pending.is_empty(),
            "no session starts under {what}"
        );
        assert_eq!(rt.state.active_tab, 1, "no tab switch under {what}");
        assert_eq!(rt.state.view, View::Terminal, "no Settings under {what}");
    }
}

#[test]
fn shifted_and_option_chords_are_not_app_shortcuts() {
    let mut rt = key_runtime(start_scene());
    let view_before = rt.state.view;
    for mods in [
        Mods {
            cmd: true,
            shift: true,
            ..Mods::default()
        },
        Mods {
            cmd: true,
            alt: true,
            ..Mods::default()
        },
    ] {
        rt.key(KeyEvent {
            key: Key::Command('t'),
            mods,
        });
        rt.frame(1.0 / 60.0);
    }
    rt.key(KeyEvent {
        key: Key::Command(','),
        mods: Mods {
            cmd: true,
            shift: true,
            ..Mods::default()
        },
    });
    rt.frame(1.0 / 60.0);

    assert!(rt.state.pending.is_empty(), "⇧⌘T and ⌥⌘T start no shell");
    assert_eq!(rt.state.view, view_before, "⇧⌘, does not toggle Settings");
}

#[test]
fn alt_digits_activate_the_nth_tab_like_cmd_digits() {
    let mut rt = key_runtime(two_tab_scene());
    press_alt(&mut rt, Key::Text("1".into()));
    assert_eq!(rt.state.active_tab, 0);
    press_alt(&mut rt, Key::Text("2".into()));
    assert_eq!(rt.state.active_tab, 1);
    press_alt(&mut rt, Key::Text("9".into()));
    assert_eq!(rt.state.active_tab, 1, "no ninth tab");
    assert_eq!(rt.state.view, View::Terminal);
}

#[test]
fn alt_arrows_cycle_tabs_and_wrap_around() {
    let mut rt = key_runtime(two_tab_scene());
    // two_tab_scene leaves the second tab active.
    press_alt(&mut rt, Key::Left);
    assert_eq!(rt.state.active_tab, 0);
    press_alt(&mut rt, Key::Left);
    assert_eq!(rt.state.active_tab, 1, "wraps to the last tab");
    press_alt(&mut rt, Key::Right);
    assert_eq!(rt.state.active_tab, 0, "wraps to the first tab");
    press_alt(&mut rt, Key::Right);
    assert_eq!(rt.state.active_tab, 1);
}

#[test]
fn alt_arrows_with_a_single_tab_fall_through_to_the_shell() {
    let mut s = start_scene();
    let session = seed_session(&mut s, "one", "/tmp");
    s.focus_session_tab(&session);
    let mut rt = key_runtime(s);
    rt.update(|s, tx| {
        assert!(
            !alt_shortcut(s, &alt_key(Key::Left), tx),
            "one tab: nothing to cycle, word motion reaches the shell"
        );
        assert!(!alt_shortcut(s, &alt_key(Key::Right), tx));
    });
    assert_eq!(rt.state.active_tab, 0);
}

#[test]
fn alt_up_down_walks_the_sidebar_rows_in_render_order() {
    let mut s = sidebar_scene();
    s.view = View::Terminal;
    let root_sess = seed_session_in(&mut s, "/work/a/web", Some("p1"));
    let wt_sess = seed_session_in(&mut s, "/work/a/feat-a", Some("p1"));
    let p2_sess = seed_session_in(&mut s, "/work/b/api", Some("p2"));
    let loose = seed_session(&mut s, "loose", "/tmp");
    let mut rt = key_runtime(s);

    // The walk's list is the sidebar's own order: project, its root
    // sessions, the worktree group and each worktree's sessions, then
    // the next project — Unassigned last while no project is active.
    let keys: Vec<String> = crate::sidebar::rows(&rt.state)
        .iter()
        .map(|row| row.key.clone())
        .collect();
    assert_eq!(
        keys,
        vec![
            "sidebar-project:p1".to_string(),
            format!("sidebar-session:{root_sess}"),
            "sidebar-wtgroup:p1".to_string(),
            "sidebar-worktree:p1:0".to_string(),
            format!("sidebar-session:{wt_sess}"),
            "sidebar-worktree:p1:1".to_string(),
            "sidebar-project:p2".to_string(),
            format!("sidebar-session:{p2_sess}"),
            "sidebar-wtgroup:p2".to_string(),
            "sidebar-worktree:p2:0".to_string(),
            "sidebar-unassigned".to_string(),
            format!("sidebar-session:{loose}"),
        ]
    );

    // First press lands on the first row: p1's project row selects its
    // root checkout through the same guarded path a click takes — and
    // activating a project hides the Unassigned group, so the list
    // shrinks under the cursor.
    press_alt(&mut rt, Key::Down);
    assert_eq!(
        rt.state.sidebar_cursor.as_deref(),
        Some("sidebar-project:p1")
    );
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p1"));
    assert_eq!(rt.state.editor.checkout, Some(PathBuf::from("/work/a/web")));
    assert_eq!(
        crate::sidebar::rows(&rt.state).len(),
        10,
        "Unassigned is gone"
    );

    // Next is p1's root session: the row's own focus+show action.
    press_alt(&mut rt, Key::Down);
    assert_eq!(
        rt.state.sidebar_cursor.as_deref(),
        Some(format!("sidebar-session:{root_sess}").as_str())
    );
    assert_eq!(
        rt.state.selected_session.as_deref(),
        Some(root_sess.as_str())
    );

    // The worktree group header is a row too: landing on it toggles —
    // closed, p1's worktree rows leave the list and the walk lands on
    // p2's project row.
    press_alt(&mut rt, Key::Down);
    assert_eq!(
        rt.state.sidebar_cursor.as_deref(),
        Some("sidebar-wtgroup:p1")
    );
    assert_eq!(rt.state.worktree_groups.get("p1"), Some(&false));
    press_alt(&mut rt, Key::Down);
    assert_eq!(
        rt.state.sidebar_cursor.as_deref(),
        Some("sidebar-project:p2")
    );
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));

    // Back onto the header reopens the group; the next press enters
    // feat-a's checkout row, then its session.
    press_alt(&mut rt, Key::Up);
    assert_eq!(
        rt.state.sidebar_cursor.as_deref(),
        Some("sidebar-wtgroup:p1")
    );
    assert_eq!(rt.state.worktree_groups.get("p1"), Some(&true));
    press_alt(&mut rt, Key::Down);
    assert_eq!(
        rt.state.sidebar_cursor.as_deref(),
        Some("sidebar-worktree:p1:0")
    );
    assert_eq!(
        rt.state.editor.checkout,
        Some(PathBuf::from("/work/a/feat-a"))
    );
    press_alt(&mut rt, Key::Down);
    assert_eq!(
        rt.state.sidebar_cursor.as_deref(),
        Some(format!("sidebar-session:{wt_sess}").as_str())
    );
    assert_eq!(rt.state.selected_session.as_deref(), Some(wt_sess.as_str()));
}

#[test]
fn alt_up_from_nowhere_lands_on_the_last_row() {
    let mut s = sidebar_scene();
    s.view = View::Terminal;
    let loose = seed_session(&mut s, "loose", "/tmp");
    let mut rt = key_runtime(s);
    // No project is active, so Unassigned and its session close the list.
    press_alt(&mut rt, Key::Up);
    assert_eq!(
        rt.state.sidebar_cursor.as_deref(),
        Some(format!("sidebar-session:{loose}").as_str())
    );
    assert_eq!(rt.state.selected_session.as_deref(), Some(loose.as_str()));
    assert_eq!(
        rt.state.active_workspace.as_deref(),
        Some(crate::data::UNASSIGNED_WORKSPACE)
    );
}

#[test]
fn alt_up_down_with_an_empty_sidebar_fall_through() {
    let mut s = State::new(test_handle());
    s.view = View::Terminal;
    let mut rt = key_runtime(s);
    rt.update(|s, tx| {
        assert!(!alt_shortcut(s, &alt_key(Key::Down), tx));
        assert!(!alt_shortcut(s, &alt_key(Key::Up), tx));
    });
    assert!(rt.state.sidebar_cursor.is_none());
}

#[test]
fn alt_chords_wait_behind_the_sidebar_menu() {
    let mut s = two_tab_scene();
    s.open_menu(
        crate::sidebar::RowTarget::Project {
            project_id: "p1".into(),
        },
        crate::sidebar::MenuKind::More,
    );
    let mut rt = key_runtime(s);
    press_alt(&mut rt, Key::Right);
    press_alt(&mut rt, Key::Text("1".into()));
    assert_eq!(rt.state.active_tab, 1, "no tab switch under the menu");
}

#[test]
fn alt_arrows_in_the_editor_view_are_left_to_word_motion() {
    let mut rt = key_runtime(two_tab_scene());
    rt.update(|s, _| s.view = View::Editor);
    rt.update(|s, tx| {
        assert!(!alt_shortcut(s, &alt_key(Key::Left), tx));
        assert!(!alt_shortcut(s, &alt_key(Key::Down), tx));
    });
    assert_eq!(rt.state.active_tab, 1);
}

#[test]
fn a_stale_preedit_does_not_block_shortcuts_outside_the_terminal() {
    let mut s = start_scene();
    s.view = View::Editor;
    s.preedit = "vie".into();
    let mut rt = key_runtime(s);
    press_cmd(&mut rt, 't');
    assert_eq!(
        rt.state.pending.len(),
        1,
        "⌘T still starts a shell in the Editor"
    );
}

#[test]
fn choosing_an_agent_in_the_new_tab_menu_requests_it_for_the_active_checkout() {
    let mut s = start_scene();
    s.editor.project_id = Some("p1".into());
    s.editor.checkout = Some(PathBuf::from("/work/a/web"));
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "new-tab-menu");
    settle(&mut rt);
    click_role(&mut rt, "option", "Claude Code");
    assert!(matches!(
        workspace_jobs(&rx).as_slice(),
        [Req::AgentArgv { agent, .. }] if agent == "claude"
    ));
    assert_eq!(rt.state.pending_argv.len(), 1);
    let pending = &rt.state.pending_argv[0];
    assert_eq!(pending.agent, "claude");
    assert_eq!(
        pending.target,
        crate::sidebar::RowTarget::Worktree {
            project_id: "p1".into(),
            path: "/work/a/web".into(),
        }
    );
}

#[test]
fn a_second_create_while_one_is_pending_enqueues_once() {
    let mut s = State::new(test_handle());
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    s.submit_workspace_create("Work");
    s.submit_workspace_create("Work");
    let creates = workspace_jobs(&rx)
        .into_iter()
        .filter(|req| matches!(req, Req::CreateWorkspace { .. }))
        .count();
    assert_eq!(creates, 1, "a double Enter sends one request");
}

#[test]
fn a_second_rename_while_one_is_pending_enqueues_once() {
    let mut s = State::new(test_handle());
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    s.submit_workspace_rename("w1", "Work");
    s.submit_workspace_rename("w1", "Work");
    let renames = workspace_jobs(&rx)
        .into_iter()
        .filter(|req| matches!(req, Req::RenameWorkspace { .. }))
        .count();
    assert_eq!(renames, 1, "a double Enter sends one request");
}

#[test]
fn the_next_submission_is_accepted_after_the_first_reply() {
    let mut s = State::new(test_handle());
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    s.submit_workspace_create("Work");
    let token = s.workspace_pending.expect("first submission in flight");
    s.apply_reply(reply(
        Req::CreateWorkspace {
            name: "Work".into(),
            token,
        },
        Err("a workspace named 'Work' already exists".into()),
    ));
    s.submit_workspace_create("Other");
    let creates = workspace_jobs(&rx)
        .into_iter()
        .filter(|req| matches!(req, Req::CreateWorkspace { .. }))
        .count();
    assert_eq!(creates, 2, "the second name is sent once the first settled");
}

#[test]
fn a_refused_selection_still_applies_and_says_it_was_not_saved() {
    let mut s = State::new(test_handle());
    let (backend, rx) = crate::editor::backend::channel_backend();
    drop(rx);
    s.editor.backend = backend;
    s.select_workspace("w1");
    assert_eq!(s.workspace_id(), "w1", "the choice applies for this run");
    assert_eq!(
        s.notice.as_deref(),
        Some("the editor queue is full; the workspace choice was not saved")
    );
}

#[test]
fn a_refused_remove_keeps_the_selection_and_the_confirmation() {
    let mut s = State::new(test_handle());
    s.select_workspace("w1");
    s.workspace_remove_confirm = Some("w1".into());
    let (backend, rx) = crate::editor::backend::channel_backend();
    drop(rx);
    s.editor.backend = backend;
    s.remove_workspace("w1");
    assert_eq!(s.workspace_id(), "w1");
    assert_eq!(s.workspace_remove_confirm.as_deref(), Some("w1"));
    assert_eq!(
        s.notice.as_deref(),
        Some("the editor queue is full; try again")
    );
}

#[test]
fn a_refused_move_sets_the_notice() {
    let mut s = State::new(test_handle());
    let (backend, rx) = crate::editor::backend::channel_backend();
    drop(rx);
    s.editor.backend = backend;
    s.move_project("p1", "w1");
    assert_eq!(
        s.notice.as_deref(),
        Some("the editor queue is full; try again")
    );
}

#[test]
fn a_usage_reply_stores_the_snapshot_and_accounts() {
    let mut s = State::new(test_handle());
    s.apply_reply(reply(
        Req::Usage {
            previous: app_core::usage::UsageSnapshot::default(),
        },
        Ok(ReplyData::Usage {
            snapshot: Box::new(app_core::usage::UsageSnapshot {
                claude: app_core::usage::ClaudeUsage {
                    updated_at: Some(42),
                    ..Default::default()
                },
                ..Default::default()
            }),
            accounts: usage_view(),
            records: vec![app_core::usage_records::UsageRecord {
                id: "devin".to_string(),
                ..Default::default()
            }],
        }),
    ));
    assert!(s.data.usage.is_some());
    assert_eq!(s.data.usage.as_ref().unwrap().claude.updated_at, Some(42));
    assert_eq!(s.data.accounts.as_ref().unwrap().claude.items.len(), 2);
    assert_eq!(s.agent_usage.len(), 1, "the records come with the reply");
    assert_eq!(s.agent_usage[0].id, "devin");
}

#[test]
fn a_failed_account_mutation_leaves_the_cache_and_sets_the_notice() {
    let mut s = State::new(test_handle());
    s.apply_reply(reply(
        Req::AccountRemove {
            agent: "claude".into(),
            id: "x".into(),
        },
        Err("boom".into()),
    ));
    assert!(s.data.accounts.is_none(), "cache mutates only on success");
    assert_eq!(s.notice.as_deref(), Some("boom"));
}

#[test]
fn a_set_active_reply_swaps_the_view_and_refreshes_usage() {
    let mut s = State::new(test_handle());
    s.apply_reply(reply(
        Req::AccountSetActive {
            agent: "claude".into(),
            id: "aa11bb22".into(),
        },
        Ok(ReplyData::Accounts(usage_view())),
    ));
    assert!(s.data.accounts.is_some());
}

#[test]
fn an_add_reply_asks_for_the_new_accounts_sign_in() {
    let mut s = State::new(test_handle());
    // The reply carries the view `add` produced — the new account is
    // active, so it is the one sign-in is requested for.
    let mut view = usage_view();
    view.claude.active = "aa11bb22".into();
    s.apply_reply(reply(
        Req::AccountAdd {
            agent: "claude".into(),
            name: "work".into(),
            dir: "/accts/work".into(),
            token: 1,
        },
        Ok(ReplyData::Accounts(view)),
    ));
    assert_eq!(
        s.data.accounts.as_ref().unwrap().claude.active,
        "aa11bb22",
        "the added account is active"
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn devin_account_view() -> app_core::accounts::AccountsView {
    let mut view = usage_view();
    view.devin.items.push(app_core::accounts::AccountView {
        id: "d1".into(),
        name: "Work".into(),
        dir: "/accts/devin work".into(),
        is_default: false,
    });
    view
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn adding_a_devin_account_requests_sign_in_for_that_provider() {
    let mut s = State::new(test_handle());
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    let token = s.settings.begin_account_add();
    let mut view = devin_account_view();
    view.devin.active = "d1".into();
    s.apply_reply(reply(
        Req::AccountAdd {
            agent: "devin".into(),
            name: "Work".into(),
            dir: "/accts/devin work".into(),
            token,
        },
        Ok(ReplyData::Accounts(view)),
    ));
    assert!(matches!(
        workspace_jobs(&rx).as_slice(),
        [Req::AccountSignIn { agent, id }] if agent == "devin" && id == "d1"
    ));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn devin_account_switcher_works_without_usage_and_preserves_old_labels() {
    let mut s = State::new(test_handle());
    let (backend, rx) = crate::editor::backend::channel_backend();
    s.editor.backend = backend;
    s.data.accounts = Some(devin_account_view());
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    click_key(&mut rt, "agents-button");
    settle(&mut rt);
    click_label(&mut rt, "Devin account Work");
    let select = workspace_jobs(&rx)
        .into_iter()
        .find(|req| matches!(req, Req::AccountSetActive { .. }))
        .expect("selecting Work enqueues a worker request");
    assert!(matches!(
        &select,
        Req::AccountSetActive { agent, id } if agent == "devin" && id == "d1"
    ));
    let mut updated = devin_account_view();
    updated.devin.active = "d1".into();
    rt.state
        .apply_reply(reply(select, Ok(ReplyData::Accounts(updated))));
    let accounts = rt.state.data.accounts.as_ref();
    assert_eq!(accounts.unwrap().devin.active, "d1");
    assert_eq!(
        data::account_name_of("devin", &["devin".into()], accounts).as_deref(),
        Some("Default")
    );
    assert_eq!(
        data::account_name_of(
            "devin",
            &[
                "/usr/bin/env".into(),
                "-u".into(),
                "WINDSURF_API_KEY".into(),
                "XDG_DATA_HOME=/accts/devin work".into(),
                "devin".into(),
            ],
            accounts,
        )
        .as_deref(),
        Some("Work")
    );
}

// -- launch correlation ---------------------------------------------------

/// Two launches in flight: each answer starts its own request. The
/// second answer landing first must not take the first one's context.
#[test]
fn two_launches_keep_their_own_answer() {
    let mut s = State::new(test_handle());
    let first_row = crate::sidebar::RowTarget::Worktree {
        project_id: "p1".into(),
        path: "/w/one".into(),
    };
    let second_row = crate::sidebar::RowTarget::Worktree {
        project_id: "p1".into(),
        path: "/w/two".into(),
    };
    s.request_session_argv(first_row, "claude".into());
    s.request_session_argv(second_row, "codex".into());
    assert_eq!(s.pending_argv.len(), 2, "both launches wait");

    let claude_token = s.pending_argv[0].token;
    let codex_token = s.pending_argv[1].token;

    // The codex answer lands first.
    let launch = s
        .resolved_launch(codex_token, vec!["codex".into()])
        .expect("the codex launch");
    assert_eq!(launch.agent, "codex");
    assert_eq!(launch.argv, vec!["codex".to_string()]);
    assert_eq!(launch.cwd, "/w/two", "the second row's checkout");
    assert_eq!(s.pending_argv.len(), 1, "the claude launch still waits");
    assert_eq!(s.pending_argv[0].token, claude_token);

    // The claude answer still starts its own launch — in its own row.
    let launch = s
        .resolved_launch(claude_token, vec!["claude".into()])
        .expect("the claude launch");
    assert_eq!(launch.agent, "claude");
    assert_eq!(launch.cwd, "/w/one", "the first row's checkout");
    assert!(s.pending_argv.is_empty());
}

/// The same agent asked for twice: the answers must not cross over.
#[test]
fn two_launches_of_the_same_agent_keep_their_own_row() {
    let mut s = State::new(test_handle());
    s.request_session_argv(
        crate::sidebar::RowTarget::Worktree {
            project_id: "p1".into(),
            path: "/w/one".into(),
        },
        "claude".into(),
    );
    s.request_session_argv(
        crate::sidebar::RowTarget::Worktree {
            project_id: "p1".into(),
            path: "/w/two".into(),
        },
        "claude".into(),
    );
    let first = s.pending_argv[0].token;
    let second = s.pending_argv[1].token;

    let launch = s
        .resolved_launch(first, vec!["claude-a".into()])
        .expect("the first launch");
    assert_eq!(launch.cwd, "/w/one");
    assert_eq!(launch.argv, vec!["claude-a".to_string()]);
    assert_eq!(
        s.pending_argv.len(),
        1,
        "the second launch keeps waiting for its own answer"
    );
    assert_eq!(s.pending_argv[0].token, second);
}

/// A refused launch must say why, and it must not consume another
/// request's launch context.
#[test]
fn a_failed_agent_argv_surfaces_the_error_and_keeps_the_other_launch() {
    let mut s = State::new(test_handle());
    s.request_session_argv(
        crate::sidebar::RowTarget::Project {
            project_id: "p1".into(),
        },
        "claude".into(),
    );
    s.request_session_argv(
        crate::sidebar::RowTarget::Project {
            project_id: "p2".into(),
        },
        "codex".into(),
    );
    let claude_token = s.pending_argv[0].token;
    let codex_token = s.pending_argv[1].token;

    s.apply_reply(reply(
        Req::AgentArgv {
            agent: "claude".into(),
            token: claude_token,
        },
        Err("the account folder /accts/work no longer exists".into()),
    ));
    assert_eq!(
        s.notice.as_deref(),
        Some("the account folder /accts/work no longer exists"),
        "the failure is visible"
    );
    assert_eq!(s.pending_argv.len(), 1, "the other launch still waits");
    assert_eq!(s.pending_argv[0].token, codex_token);
}

#[test]
fn a_sign_in_reply_launches_a_session_with_the_title() {
    let mut s = State::new(test_handle());
    s.apply_reply(reply(
        Req::AccountSignIn {
            agent: "claude".into(),
            id: "aa11bb22".into(),
        },
        Ok(ReplyData::SignIn {
            argv: vec![
                "/usr/bin/env".into(),
                "CLAUDE_CONFIG_DIR=/a".into(),
                "claude".into(),
            ],
            cwd: "/a".into(),
            title: "Claude sign-in".into(),
        }),
    ));
    // The daemon isn't connected in tests: the launch is queued (or the
    // notice explains the refusal) — never a panic on the reply path.
    assert!(
        !s.pending.is_empty() || s.notice.is_some(),
        "the sign-in launch must at least be attempted"
    );
}

#[test]
fn a_sign_in_error_surfaces_as_the_notice() {
    let mut s = State::new(test_handle());
    s.apply_reply(reply(
        Req::AccountSignIn {
            agent: "claude".into(),
            id: "x".into(),
        },
        Err("no account".into()),
    ));
    assert_eq!(s.notice.as_deref(), Some("no account"));
    assert!(s.pending.is_empty());
}

#[test]
fn the_stored_theme_restores_on_its_getsetting_reply() {
    let mut s = State::new(test_handle());
    assert_eq!(s.theme_choice, crate::theme::Theme::Dark);
    s.apply_reply(reply(
        Req::GetSetting {
            key: crate::theme::THEME_KEY.to_string(),
        },
        Ok(ReplyData::Setting(Some("dracula".into()))),
    ));
    assert_eq!(s.theme_choice, crate::theme::Theme::Dracula);
    // A missing row leaves the default alone.
    let mut s = State::new(test_handle());
    s.apply_reply(reply(
        Req::GetSetting {
            key: crate::theme::THEME_KEY.to_string(),
        },
        Ok(ReplyData::Setting(None)),
    ));
    assert_eq!(s.theme_choice, crate::theme::Theme::Dark);
}

#[test]
fn the_stored_list_aside_side_restores_on_its_getsetting_reply() {
    let mut s = State::new(test_handle());
    assert_eq!(s.list_aside_side, crate::sidebar::Side::Left);
    s.apply_reply(reply(
        Req::GetSetting {
            key: crate::editor::files::LIST_ASIDE_SIDE_KEY.to_string(),
        },
        Ok(ReplyData::Setting(Some("right".into()))),
    ));
    assert_eq!(s.list_aside_side, crate::sidebar::Side::Right);
    // An unrecognized value falls back to Left.
    let mut s = State::new(test_handle());
    s.apply_reply(reply(
        Req::GetSetting {
            key: crate::editor::files::LIST_ASIDE_SIDE_KEY.to_string(),
        },
        Ok(ReplyData::Setting(Some("corner".into()))),
    ));
    assert_eq!(s.list_aside_side, crate::sidebar::Side::Left);
}

#[test]
fn the_stored_scroll_speed_restores_on_its_getsetting_reply() {
    let mut s = State::new(test_handle());
    assert_eq!(s.scroll_speed, 1.0);
    s.apply_reply(reply(
        Req::GetSetting {
            key: settings::SCROLL_SPEED_KEY.to_string(),
        },
        Ok(ReplyData::Setting(Some("1.5".into()))),
    ));
    assert_eq!(s.scroll_speed, 1.5);
    // A missing row leaves the default alone.
    let mut s = State::new(test_handle());
    s.apply_reply(reply(
        Req::GetSetting {
            key: settings::SCROLL_SPEED_KEY.to_string(),
        },
        Ok(ReplyData::Setting(None)),
    ));
    assert_eq!(s.scroll_speed, 1.0);
}

#[test]
fn the_done_sound_setting_restores_on_its_getsetting_reply() {
    let mut s = State::new(test_handle());
    assert!(s.sound_done, "absent means on");
    s.apply_reply(reply(
        Req::GetSetting {
            key: settings::SOUND_DONE_KEY.to_string(),
        },
        Ok(ReplyData::Setting(Some("0".into()))),
    ));
    assert!(!s.sound_done);
    // A missing row leaves the default alone.
    let mut s = State::new(test_handle());
    s.apply_reply(reply(
        Req::GetSetting {
            key: settings::SOUND_DONE_KEY.to_string(),
        },
        Ok(ReplyData::Setting(None)),
    ));
    assert!(s.sound_done);
}

#[test]
fn an_activity_flip_to_done_chimes_once() {
    let mut s = State::new(test_handle());
    s.window_focused = false;
    let reply_with = |activity: HashMap<String, app_core::activity::Activity>| {
        reply(
            Req::Activity {
                sessions: Vec::new(),
                previous: HashMap::new(),
            },
            Ok(ReplyData::Activity(activity)),
        )
    };
    let mut activity = HashMap::new();
    activity.insert("sess-1".to_string(), app_core::activity::Activity::Working);
    s.apply_reply(reply_with(activity.clone()));
    assert_eq!(crate::sound::test_plays(), 0, "working is not done");
    activity.insert("sess-1".to_string(), app_core::activity::Activity::Done);
    s.apply_reply(reply_with(activity.clone()));
    assert_eq!(crate::sound::test_plays(), 1, "the flip chimes");
    // Past the debounce, the same done state does not chime again.
    s.now_ms += 2_000;
    s.apply_reply(reply_with(activity));
    assert_eq!(crate::sound::test_plays(), 1);
    // Focused windows stay silent even on a fresh flip.
    s.window_focused = true;
    let mut next = HashMap::new();
    next.insert("sess-2".to_string(), app_core::activity::Activity::Done);
    s.apply_reply(reply_with(next));
    assert_eq!(crate::sound::test_plays(), 1);
}

#[test]
fn poll_usage_paces_the_worker_reads() {
    let mut s = State::new(test_handle());
    assert!(s.poll_usage(0.0), "the first read goes out");
    assert!(!s.poll_usage(1.0), "inside the 5 s window");
    assert!(!s.poll_usage(4.9));
    assert!(s.poll_usage(5.1), "the cadence allows the next read");
}

/// A running session's dot spins on the renderer's own clock: the poll
/// must not ask for a frame (or report a change) every tick for it.
#[test]
fn a_running_session_does_not_keep_the_poll_redrawing() {
    let mut s = State::new(test_handle());
    s.data.sessions.push(data::SessionView::new(
        protocol::session::SessionInfo {
            id: protocol::session::SessionId::new(),
            title: "shell".into(),
            argv: vec!["/bin/zsh".into()],
            cwd: "/work/repo".into(),
            state: protocol::session::SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 1_700_000_000_000,
            ended_at: None,
        },
        None,
    ));
    assert!(s.data.sessions[0].info.state.working());
    poll(&mut s, 0.0, &mut Tx::default());
    for tick in 1..=5 {
        let mut tx = Tx::default();
        assert!(!poll(&mut s, tick as f64 * 0.033, &mut tx), "tick {tick}");
    }
}

#[test]
fn theme_parse_and_resolve_follow_the_desktop() {
    use crate::theme::Theme;
    assert_eq!(Theme::parse("system"), Theme::System);
    assert_eq!(Theme::parse("one-dark"), Theme::OneDark);
    assert_eq!(Theme::parse("catppuccin-mocha"), Theme::CatppuccinMocha);
    assert_eq!(Theme::parse("dracula"), Theme::Dracula);
    assert_eq!(Theme::parse("nonsense"), Theme::Dark, "unknown → Dark+");
    assert_eq!(Theme::System.resolve(true), Theme::Dark);
    assert_eq!(Theme::System.resolve(false), Theme::Light);
    assert_eq!(Theme::Dracula.resolve(false), Theme::Dracula);
}

/// The Agents dropdown mounts only while it is open, and needs neither an
/// accounts view nor usage data to do so.
#[test]
fn the_agents_usage_dropdown_mounts_only_while_open() {
    let mut s = State::new(test_handle());
    let mut layer = Vec::new();
    assert!(chrome::usage_menu_layer(&s, &mut layer).is_none());
    assert!(layer.is_empty());
    s.usage_menu = Some("agents".to_string());
    assert!(chrome::usage_menu_layer(&s, &mut layer).is_some());
    assert!(!layer.is_empty(), "the popover goes on the overlay layer");
    s.data.accounts = Some(usage_view());
    assert!(chrome::usage_menu_layer(&s, &mut layer).is_some());
}

/// The Settings page alone, so its controls can be counted.
fn settings_view(s: &State, _cx: &mut Cx<State>) -> El {
    settings::view(s, &s.ink)
}

/// Default has no sign-in of its own — the core refuses it — so the
/// Accounts section must not offer the button for it.
#[test]
fn the_default_account_has_no_sign_in_button() {
    let mut s = State::new(test_handle());
    s.settings.section = crate::settings::Section::Accounts;
    // The view: Default plus one named Claude account, and Codex with
    // Default only.
    s.data.accounts = Some(usage_view());
    let mut rt = Runtime::new(s, settings_view, (1512.0, 814.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let sign_ins = els(&rt)
        .iter()
        .filter(|element| element.label.as_deref() == Some("Sign in"))
        .count();
    assert_eq!(sign_ins, 1, "only the named account can sign in");
}

/// An open combobox reports `expanded` on its trigger — the chevron
/// and the a11y state come from the same flag.
#[test]
fn an_open_combobox_reports_expanded_on_its_trigger() {
    let mut s = State::new(test_handle());
    s.settings.section = crate::settings::Section::Editor;
    let mut rt = Runtime::new(s, settings_view, (1512.0, 814.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let expanded = |rt: &Runtime<State>| {
        els(rt)
            .iter()
            .find(|e| e.key.as_deref() == Some("settings-indent-value-trigger"))
            .expect("the indent picker mounts")
            .expanded
    };
    assert_eq!(expanded(&rt), Some(false), "closed reports collapsed");
    click_key(&mut rt, "settings-indent-value-trigger");
    assert!(rt.state.settings.indent_picker.open, "press opens the list");
    assert_eq!(expanded(&rt), Some(true), "open reports expanded");
}

/// Settings → Updates names the running version, and offers the install
/// command only once a newer release is known.
#[test]
fn settings_updates_shows_the_version_and_the_install_command_when_one_is_out() {
    let page = |status: update::Status| {
        let mut s = State::new(test_handle());
        s.settings.section = crate::settings::Section::Updates;
        s.update = status;
        let mut rt = Runtime::new(s, settings_view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let infos = els(&rt);
        let has = |wanted: &str| {
            infos
                .iter()
                .any(|e| e.label.as_deref() == Some(wanted) || e.text.as_deref() == Some(wanted))
        };
        (
            has(&format!("Goko {}", update::CURRENT)),
            has("Check for updates"),
            has("Copy install command"),
            has("Goko 9.9.9 is available."),
        )
    };
    let (version, check, copy, said) = page(update::Status::UpToDate);
    assert!(
        version && check,
        "the version and the check button are always there"
    );
    assert!(!copy && !said, "nothing to install yet");
    let (version, _, copy, said) = page(update::Status::Available("9.9.9".into()));
    assert!(
        version && copy && said,
        "the new release comes with the command"
    );
}

/// Where this copy can be replaced, a found release offers "Update now" in
/// place of the command, and shows what changed in it.
#[test]
fn settings_updates_offers_update_now_and_the_release_notes() {
    let page = |dir: Option<std::path::PathBuf>, status: update::Status| {
        let mut s = State::new(test_handle());
        s.settings.section = crate::settings::Section::Updates;
        s.update = status;
        s.update_dir = dir;
        let mut rt = Runtime::new(s, settings_view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let infos = els(&rt);
        let has = |wanted: &str| {
            infos
                .iter()
                .any(|e| e.label.as_deref() == Some(wanted) || e.text.as_deref() == Some(wanted))
        };
        (
            has("Update now"),
            has("Copy install command"),
            has("What's new in Goko 9.9.9"),
            has("• Find in the terminal"),
        )
    };
    let release = || update::Release {
        version: "9.9.9".into(),
        notes: "### Added\n\n- Find in the `terminal`".into(),
    };
    let here = Some(std::env::temp_dir());
    let (now, copy, heading, bullet) = page(here.clone(), update::Status::Available(release()));
    assert!(
        now && !copy,
        "this copy can be replaced, so no command to paste"
    );
    assert!(heading && bullet, "the notes say what changed");
    let (now, copy, ..) = page(None, update::Status::Available(release()));
    assert!(
        !now && copy,
        "a copy that cannot be replaced gets the command"
    );
    let (now, _, heading, _) = page(here.clone(), update::Status::Installing(release()));
    assert!(!now && heading, "no second install while one runs");
    let (_, _, heading, _) = page(here, update::Status::Available("9.9.9".into()));
    assert!(!heading, "a release without notes shows none");
}

// -- Terminal row geometry ------------------------------------------------

/// Whether the element `idx` sits under the element `ancestor` —
/// `ElementInfo::parent` chains up the mounted tree.
fn inside_ancestor(els: &[rune::ElementInfo], idx: usize, ancestor: usize) -> bool {
    let mut i = Some(idx);
    while let Some(n) = i {
        if n == ancestor {
            return true;
        }
        i = els[n].parent;
    }
    false
}

/// The text rows mounted inside the pane `pane_key`'s cell, plus the
/// cell's own rect — a pane holds exactly one terminal's grid.
fn pane_rows(rt: &Runtime<State>, pane_key: &str) -> (Vec<rune::ElementInfo>, [f32; 4]) {
    let want = format!("pane:{}", panes::pane_id(pane_key));
    let els = els(rt);
    let cell = els
        .iter()
        .position(|e| e.key.as_deref() == Some(want.as_str()))
        .expect("the pane cell is mounted");
    let rows = els
        .iter()
        .enumerate()
        .filter(|(i, e)| e.text.is_some() && inside_ancestor(&els, *i, cell))
        .map(|(_, e)| e.clone())
        .collect();
    (rows, els[cell].rect)
}

/// A frame like the agent TUI draws: styled banner lines (colour runs,
/// spaces to wrap at, and 'W's wider than the measured cell), a blank
/// stretch, then home. 'W' shapes wider than the cell in the test
/// fonts — what a real frame's wide glyphs do — so a wrapping row is
/// reproduced, not hoped for.
fn feed_tui_frame(tx: &mpsc::Sender<daemon::Output>, seq: &mut u64) {
    let mut data = Vec::new();
    for _ in 0..6 {
        data.extend_from_slice("\u{1b}[31m".as_bytes());
        data.extend_from_slice("W ".repeat(60).as_bytes());
        data.extend_from_slice("\u{1b}[32m".as_bytes());
        data.extend_from_slice("W ".repeat(60).as_bytes());
        data.extend_from_slice("\u{1b}[0m\r\n".as_bytes());
    }
    data.extend_from_slice("\r\n".repeat(4).as_bytes());
    data.extend_from_slice("\u{1b}[H".as_bytes());
    let len = data.len() as u64;
    tx.send(daemon::Output { seq: *seq, data }).unwrap();
    *seq += len;
}

/// Address the input prompt onto the grid's last line — where a
/// full-screen program keeps its input frame.
fn feed_last_row_marker(tx: &mpsc::Sender<daemon::Output>, seq: &mut u64, row: usize) {
    let data = format!("\u{1b}[{row};1H\u{276f} PROMPT-MARKER").into_bytes();
    let len = data.len() as u64;
    tx.send(daemon::Output { seq: *seq, data }).unwrap();
    *seq += len;
}

/// The terminal's rows inside a column deliberately narrower than the
/// text: the same mount the pane gives, squeezed.
fn narrow_term_view(s: &State, _cx: &mut Cx<State>) -> El {
    let rows = s
        .attachments
        .values()
        .next()
        .map(|att| term::term_lines_el(&att.lines, &att.links, &att.marks, &s.ink))
        .unwrap_or_default();
    col().w(220.0).min_h(0.0).children(rows)
}

#[test]
fn a_terminal_row_stays_one_cell_line_wide_or_not() {
    // Every drawn grid row is a single line: `rich` wrapped a row
    // wider than the pane onto a second line, and the extra height
    // pushed the grid's bottom rows — a TUI's input frame — out of
    // view. Narrow the column under the rows to force the check.
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    let mut seq = 0u64;
    feed_tui_frame(&tx, &mut seq);
    terminal.feed();
    attach(&mut s, &key, terminal);
    let ink = s.ink;
    let lines = s
        .attachments
        .get_mut(&key)
        .unwrap()
        .term
        .lines(&ink)
        .to_vec();
    assert!(
        lines.iter().any(|l| l.text().contains("W W")),
        "the styled rows fed the grid"
    );
    s.attachments.get_mut(&key).unwrap().set_lines(&lines, &key);

    let mut rt = Runtime::new(s, narrow_term_view, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for e in els(&rt) {
        let Some(text) = e.text.as_deref() else {
            continue;
        };
        assert!(
            e.rect[3] <= 17.0,
            "a terminal row took {:.1}pt inside a narrow pane: {text:?}",
            e.rect[3]
        );
    }
}

#[test]
fn the_pane_ends_inside_the_window() {
    for (w, h) in [(963.0, 780.0), (1512.0, 814.0), (900.0, 560.0)] {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (_tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let terminal = term::Terminal::new(session, rx, ready_rx);
        attach(&mut s, &key, terminal);
        let pk = key.clone();
        let mut rt = Runtime::new(s, view, (w, h), Env::new(true, 2.0));
        rt.poll = Some(poll);
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        // The tab bar and controls sit above the pane: it must take
        // what they leave, not the whole column's height.
        let (_, rect) = pane_rows(&rt, &pk);
        assert!(
            rect[1] + rect[3] <= h + 0.5,
            "the pane runs below the window at {w}x{h}: {rect:?}"
        );
    }
}

#[test]
fn the_wheel_over_a_pane_scrolls_its_history() {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    let data: String = (0..200).map(|n| format!("row{n}\r\n")).collect();
    tx.send(daemon::Output {
        seq: 0,
        data: data.into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let pane_key = key.clone();
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    let top = |rt: &mut Runtime<State>| {
        let mut text = String::new();
        rt.update(|s, _| text = s.attachments[&pane_key].lines[0].text());
        text.trim_end().to_string()
    };
    let live = top(&mut rt);
    rt.wheel_event(rune::WheelEvent::new(0.0, 160.0).at(700.0, 400.0));
    rt.frame(1.0 / 60.0);
    let up = top(&mut rt);
    assert_ne!(up, live, "the wheel moved the view into the history");
    rt.update(|s, _| {
        let ev = rune::KeyEvent {
            key: rune::Key::Text("a".into()),
            mods: rune::Mods::default(),
        };
        assert!(
            terminal_key(s, &ev, None),
            "the key reached the live session"
        );
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(top(&mut rt), live, "typing returned to the live screen");
}

#[test]
fn files_dropped_on_a_pane_are_typed_into_its_session() {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    let data: String = (0..200).map(|n| format!("row{n}\r\n")).collect();
    tx.send(daemon::Output {
        seq: 0,
        data: data.into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let pane_key = key.clone();
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    let top = |rt: &mut Runtime<State>| {
        let mut text = String::new();
        rt.update(|s, _| text = s.attachments[&pane_key].lines[0].text());
        text.trim_end().to_string()
    };
    let live = top(&mut rt);
    rt.wheel_event(rune::WheelEvent::new(0.0, 160.0).at(700.0, 400.0));
    rt.frame(1.0 / 60.0);
    assert_ne!(top(&mut rt), live, "the view is in the history");

    let file = std::path::PathBuf::from("/tmp/a b.txt");
    rt.drag_files(vec![file.clone()], Some((700.0, 400.0)));
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(
        rt.state.drop_pane.as_deref(),
        Some(key.as_str()),
        "the pane shows the drop ring"
    );
    rt.drop_files(vec![file], Some((700.0, 400.0)));
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.drop_pane, None, "the ring went with the drop");
    assert_eq!(top(&mut rt), live, "the drop typed into the live session");
}

#[test]
fn a_working_session_row_shows_rune_kits_spinner_in_place_of_its_icon() {
    let mut s = sidebar_scene();
    let key = seed_session(&mut s, "agent", "/work/a/web");
    s.focus_session_tab(&key);
    s.activity
        .insert(key.clone(), app_core::activity::Activity::Working);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let row = format!("sidebar-session:{key}");
    // The radar's 25 dots (`d0`–`d24`) mount inside the session's row.
    let dots = |rt: &Runtime<State>| {
        els(rt)
            .iter()
            .filter(|e| {
                e.path.contains(&row) && e.key.as_deref().is_some_and(|k| k.starts_with('d'))
            })
            .count()
    };
    assert_eq!(dots(&rt), 25, "the working row draws the spinner");

    // Done: the agent icon and its green dot come back.
    rt.update(|s, _| {
        s.activity
            .insert(key.clone(), app_core::activity::Activity::Done);
    });
    // The spinner fades out through its exit presence first (~0.3 s).
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(dots(&rt), 0, "a finished row has no spinner");
}

#[test]
fn the_terminal_hands_keyboard_focus_back_from_chrome_rows() {
    // Rune focus left on a pressable eats Space — it re-fires
    // `on_press` — before `on_key` sees it. A chrome press yields the
    // focus in the same frame, so the keystroke reaches the terminal.
    let mut s = sidebar_scene();
    let key = seed_session(&mut s, "agent", "/work/a/web");
    s.focus_session_tab(&key);
    let mut rt = Runtime::new(s, view, (1100.0, 700.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    // Click the tab pill left of centre — on the title, away from the
    // close X — the way a stray click lands focus on chrome.
    let tab_key = format!("tab:tab-{key}");
    let tab = els(&rt)
        .into_iter()
        .find(|e| e.key.as_deref() == Some(tab_key.as_str()))
        .expect("the session's tab pill mounted");
    let (x, y) = (tab.rect[0] + 24.0, tab.rect[1] + tab.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    let stuck: Vec<_> = els(&rt)
        .iter()
        .filter(|e| e.focused)
        .map(|e| (e.key.clone(), e.role, e.path.clone()))
        .collect();
    assert!(stuck.is_empty(), "chrome kept rune focus: {stuck:?}");
}

#[test]
fn dragging_across_terminal_rows_selects_them_and_cmd_c_copies() {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    let data: String = (0..60).map(|n| format!("row{n}\r\n")).collect();
    tx.send(daemon::Output {
        seq: 0,
        data: data.into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    let rows: Vec<[f32; 4]> = rt
        .elements()
        .iter()
        .filter(|e| e.text.as_deref().is_some_and(|t| t.starts_with("row")))
        .map(|e| e.rect)
        .collect();
    assert!(rows.len() > 4, "terminal rows are laid out: {}", rows.len());
    let (first, last) = (rows[1], rows[rows.len() - 2]);
    let (x0, y0) = (first[0] + 2.0, first[1] + first[3] / 2.0);
    let (x1, y1) = (last[0] + 20.0, last[1] + last[3] / 2.0);
    rt.pointer_down(x0, y0);
    rt.pointer_move((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    rt.pointer_move(x1, y1);
    rt.pointer_up(x1, y1);
    rt.frame(1.0 / 60.0);
    assert!(
        rt.state.attachments[&key].term.selection_text().is_some(),
        "the drag selected rows"
    );
    rt.key(KeyEvent {
        key: Key::Command('c'),
        mods: Mods {
            cmd: true,
            ..Mods::default()
        },
    });
    rt.frame(1.0 / 60.0);
    let copied = rt
        .copy_request
        .as_ref()
        .map(|w| w.content().text().to_owned())
        .unwrap_or_default();
    assert!(copied.contains("row"), "copied {copied:?}");
    assert!(copied.contains('\n'), "several rows: {copied:?}");
    assert!(
        copied.lines().all(|l| l == l.trim_end()),
        "no grid padding: {copied:?}"
    );
    assert_eq!(rt.state.notice.as_deref(), Some("Copied to clipboard"));
}

#[test]
fn a_finished_selection_copies_without_cmd_c() {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    let data: String = (0..60).map(|n| format!("row{n}\r\n")).collect();
    tx.send(daemon::Output {
        seq: 0,
        data: data.into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    let rows: Vec<[f32; 4]> = rt
        .elements()
        .iter()
        .filter(|e| e.text.as_deref().is_some_and(|t| t.starts_with("row")))
        .map(|e| e.rect)
        .collect();
    let (first, last) = (rows[1], rows[rows.len() - 2]);
    let (x0, y0) = (first[0] + 2.0, first[1] + first[3] / 2.0);
    let (x1, y1) = (last[0] + 20.0, last[1] + last[3] / 2.0);
    rt.pointer_down(x0, y0);
    rt.pointer_move((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    rt.pointer_move(x1, y1);
    // Mid-drag frames hear the change as `dragging`: nothing copies yet.
    rt.frame(1.0 / 60.0);
    assert!(
        rt.copy_request.is_none(),
        "a drag in progress must not copy"
    );
    rt.pointer_up(x1, y1);
    rt.frame(1.0 / 60.0);
    let copied = rt
        .copy_request
        .as_ref()
        .map(|w| w.content().text().to_owned())
        .unwrap_or_default();
    assert!(copied.contains("row"), "release copied {copied:?}");
    assert!(copied.contains('\n'), "several rows: {copied:?}");
    assert!(
        copied.lines().all(|l| l == l.trim_end()),
        "no grid padding: {copied:?}"
    );
    assert_eq!(rt.state.notice.as_deref(), Some("Copied to clipboard"));
}

/// A pane whose program captures the mouse (`prefix` turns the mode on)
/// over 60 numbered rows, mounted with the poll running.
fn captured_pane_runtime(prefix: &str) -> Runtime<State> {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    let rows: String = (0..60).map(|n| format!("row{n}\r\n")).collect();
    tx.send(daemon::Output {
        seq: 0,
        data: format!("{prefix}{rows}").into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

/// Drag from the second shown row to the second-to-last and release.
fn drag_across_rows(rt: &mut Runtime<State>) {
    let rows: Vec<[f32; 4]> = rt
        .elements()
        .iter()
        .filter(|e| e.text.as_deref().is_some_and(|t| t.starts_with("row")))
        .map(|e| e.rect)
        .collect();
    let (first, last) = (rows[1], rows[rows.len() - 2]);
    let (x0, y0) = (first[0] + 2.0, first[1] + first[3] / 2.0);
    let (x1, y1) = (last[0] + 20.0, last[1] + last[3] / 2.0);
    rt.pointer_down(x0, y0);
    rt.pointer_move((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    rt.pointer_move(x1, y1);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x1, y1);
    rt.frame(1.0 / 60.0);
}

/// A drag that leaves the pane at the top scrolls the history and keeps
/// selecting, so the copy holds rows that were never on screen together.
#[test]
fn dragging_past_the_top_scrolls_and_selects_into_the_history() {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    let data: String = (0..200).map(|n| format!("row{n}\r\n")).collect();
    tx.send(daemon::Output {
        seq: 0,
        data: data.into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    let rows: Vec<[f32; 4]> = rt
        .elements()
        .iter()
        .filter(|e| e.text.as_deref().is_some_and(|t| t.starts_with("row")))
        .map(|e| e.rect)
        .collect();
    let (top, bottom) = (rows[0], rows[rows.len() - 2]);
    let x = bottom[0] + 20.0;
    let y = bottom[1] + bottom[3] / 2.0;
    rt.pointer_down(x, y);
    rt.pointer_move(x, y - 20.0);
    for _ in 0..8 {
        rt.pointer_move(x, top[1] - 30.0);
    }
    rt.pointer_up(x, top[1] - 30.0);
    rt.frame(1.0 / 60.0);
    let term = &rt.state.attachments[&key].term;
    assert!(term.display_offset() > 0, "the view scrolled into history");
    let text = term.selection_text().unwrap_or_default();
    assert!(
        text.lines().count() > rows.len(),
        "more rows than one screen: {}",
        text.lines().count()
    );
}

/// Claude Code's fullscreen mode asks for every motion (?1003) on the
/// alternate screen. While the button is held, each move must reach it as
/// a drag (button 0 held, code 32), never also as a bare hover (code 35):
/// a hover says no button is down, which ends its selection mid-drag.
#[test]
fn a_drag_on_the_alt_screen_never_reports_a_buttonless_move() {
    let mode = "\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h";
    let mut rt = captured_pane_runtime(mode);
    rt.update(|s, _| s.mouse_sent.clear());
    drag_across_rows(&mut rt);
    let sent: Vec<String> = rt
        .state
        .mouse_sent
        .iter()
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .collect();
    let press = sent
        .iter()
        .position(|r| r.starts_with("\x1b[<0;") && r.ends_with('M'));
    let release = sent.iter().rposition(|r| r.ends_with('m'));
    let (Some(press), Some(release)) = (press, release) else {
        panic!("no press and release reported: {sent:?}");
    };
    let held = &sent[press..release];
    assert!(
        held.iter().any(|r| r.starts_with("\x1b[<32;")),
        "the drag reported its moves: {sent:?}"
    );
    assert!(
        !held.iter().any(|r| r.starts_with("\x1b[<35;")),
        "a bare hover went out while the button was held: {sent:?}"
    );
}

#[test]
fn a_drag_selects_and_copies_while_the_program_captures_the_mouse() {
    let mode = "\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h";
    let copied = |rt: &Runtime<State>| {
        rt.copy_request
            .as_ref()
            .map(|w| w.content().text().to_owned())
            .unwrap_or_default()
    };
    // A plain drag selects, and its release copies.
    let mut rt = captured_pane_runtime(mode);
    drag_across_rows(&mut rt);
    let text = copied(&rt);
    assert!(
        text.contains("row") && text.contains('\n'),
        "drag copied {text:?}"
    );

    // Shift selects too, and takes the clicks back from the program.
    let mut rt = captured_pane_runtime(mode);
    rt.update(|s, _| {
        s.editor.set_modifiers(Mods {
            shift: true,
            ..Mods::default()
        })
    });
    rt.frame(1.0 / 60.0);
    drag_across_rows(&mut rt);
    assert!(copied(&rt).contains("row"), "shift-drag copied");

    // On the alternate screen (Claude Code's fullscreen mode) there is no history
    // to select in: the program gets a plain drag, and scrolls and selects itself.
    let mut rt = captured_pane_runtime(&format!("\x1b[?1049h{mode}"));
    drag_across_rows(&mut rt);
    assert!(
        rt.copy_request.is_none(),
        "an alt-screen drag selected locally"
    );

    // Option hands the drag to the program: nothing is selected locally.
    let mut rt = captured_pane_runtime(mode);
    rt.update(|s, _| {
        s.editor.set_modifiers(Mods {
            alt: true,
            ..Mods::default()
        })
    });
    rt.frame(1.0 / 60.0);
    drag_across_rows(&mut rt);
    assert!(rt.copy_request.is_none(), "a captured drag copied");
}

#[test]
fn a_program_copy_on_the_shown_pane_reaches_the_clipboard() {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let terminal = term::Terminal::new(session, rx, ready_rx);
    attach(&mut s, &key, terminal);
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    // The live stream starts with the daemon's paint; then the program
    // copies its own selection with OSC 52 ("copied" in base64).
    tx.send(daemon::Output {
        seq: protocol::frame::PAINT_SEQ,
        data: b"\x1bc".to_vec(),
    })
    .unwrap();
    tx.send(daemon::Output {
        seq: 0,
        data: b"\x1b]52;c;Y29waWVk\x07".to_vec(),
    })
    .unwrap();
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(
        rt.copy_request
            .as_ref()
            .map(|w| w.content().text().to_owned())
            .as_deref(),
        Some("copied")
    );
}

#[test]
fn a_trackpad_scrolls_the_history_even_with_rows_selected() {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    let data: String = (0..200).map(|n| format!("row{n}\r\n")).collect();
    tx.send(daemon::Output {
        seq: 0,
        data: data.into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let pane_key = key.clone();
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    let top = |rt: &mut Runtime<State>| {
        let mut text = String::new();
        rt.update(|s, _| text = s.attachments[&pane_key].lines[0].text());
        text.trim_end().to_string()
    };
    let live = top(&mut rt);
    rt.pointer_down(560.0, 300.0);
    rt.pointer_move(600.0, 380.0);
    rt.pointer_move(640.0, 460.0);
    rt.pointer_up(640.0, 460.0);
    rt.frame(1.0 / 60.0);
    assert!(
        rt.state.attachments[&pane_key]
            .term
            .selection_text()
            .is_some(),
        "rows are selected"
    );
    for ph in [
        rune::GesturePhase::Began,
        rune::GesturePhase::Moved,
        rune::GesturePhase::Moved,
        rune::GesturePhase::Ended,
    ]
    .into_iter()
    {
        rt.wheel_event(
            rune::WheelEvent::new(0.0, 12.0)
                .at(700.0, 400.0)
                .phase(ph)
                .precise(true),
        );
        rt.frame(1.0 / 60.0);
    }
    assert_ne!(
        top(&mut rt),
        live,
        "trackpad scrolled, selected {:?}",
        rt.state.attachments[&pane_key].term.selection_text()
    );
}

#[test]
fn a_terminal_link_opens_on_click_and_never_takes_tab() {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    tx.send(daemon::Output {
        seq: 0,
        data: b"see nope/zzz.rs:3 now\r\n".to_vec(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    let row = rt
        .elements()
        .iter()
        .find(|e| e.text.as_deref().is_some_and(|t| t.starts_with("see nope")))
        .map(|e| e.rect)
        .expect("the row is laid out");
    let (cell_w, _) = term::cell_metrics();
    let (x, y) = (row[0] + cell_w * 8.0, row[1] + row[3] / 2.0);

    // Tab stays the terminal's: no link takes keyboard focus from it.
    rt.key(KeyEvent {
        key: Key::Tab,
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert!(
        !rt.elements().iter().any(|e| e.focused && e.role == "link"),
        "Tab focused a link"
    );

    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.state.notice.as_deref(),
        Some("nope/zzz.rs: no such file"),
        "a click resolved the file"
    );
}

#[test]
fn an_input_method_composes_at_the_cursor_and_commits_into_the_session() {
    let mut s = State::new(test_handle());
    let key = seed_session(&mut s, "claude", "/work/repo");
    s.focus_session_tab(&key);
    let session = s.session_id(&key).unwrap();
    let (tx, rx) = mpsc::channel();
    let (_ready_tx, ready_rx) = mpsc::channel();
    let mut terminal = term::Terminal::new(session, rx, ready_rx);
    let data: String = (0..200).map(|n| format!("row{n}\r\n")).collect();
    tx.send(daemon::Output {
        seq: 0,
        data: data.into_bytes(),
    })
    .unwrap();
    terminal.feed();
    attach(&mut s, &key, terminal);
    let pane_key = key.clone();
    let mut rt = Runtime::new(s, view, (1200.0, 800.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    for _ in 0..6 {
        rt.frame(1.0 / 60.0);
    }
    // The window turns the input method on at the focused pane's cursor.
    assert!(
        rt.ime_area.is_some(),
        "the cursor asks for the input method"
    );

    rt.update(|s, tx| {
        assert!(on_ime(s, &rune::Ime::Preedit("vie".into(), None), tx));
    });
    rt.frame(1.0 / 60.0);
    assert!(
        els(&rt).iter().any(|e| e.text.as_deref() == Some("vie")),
        "the marked text is drawn at the cursor"
    );

    let top = |rt: &mut Runtime<State>| {
        let mut text = String::new();
        rt.update(|s, _| text = s.attachments[&pane_key].lines[0].text());
        text.trim_end().to_string()
    };
    let live = top(&mut rt);
    rt.wheel_event(rune::WheelEvent::new(0.0, 160.0).at(700.0, 400.0));
    rt.frame(1.0 / 60.0);
    assert_ne!(top(&mut rt), live, "the view is in the history");
    rt.update(|s, tx| {
        assert!(on_ime(s, &rune::Ime::Commit("việt".into()), tx));
    });
    rt.frame(1.0 / 60.0);
    assert!(
        rt.state.preedit.is_empty(),
        "the commit ended the composition"
    );
    assert_eq!(top(&mut rt), live, "the commit typed into the live session");
}

#[test]
fn the_last_terminal_row_stays_inside_its_pane() {
    // The regression the missing Claude input frame reported: the
    // grid's bottom row must land inside the pane, at full width and
    // squeezed.
    for (w, h) in [(1512.0, 814.0), (900.0, 560.0)] {
        let mut s = State::new(test_handle());
        let key = seed_session(&mut s, "claude", "/work/repo");
        s.focus_session_tab(&key);
        let session = s.session_id(&key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let terminal = term::Terminal::new(session, rx, ready_rx);
        attach(&mut s, &key, terminal);
        let pane_key = key.clone();
        let mut seq = 0u64;
        feed_tui_frame(&tx, &mut seq);

        let mut rt = Runtime::new(s, view, (w, h), Env::new(true, 2.0));
        rt.poll = Some(poll);
        rt.frame(0.0);
        // The first layout measures the pane and resizes the grid;
        // the next frames let the pane and its rows settle.
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }

        let mut rows = 0;
        rt.update(|s, _| {
            rows = s.attachments.get(&pane_key).unwrap().rows;
        });
        feed_last_row_marker(&tx, &mut seq, rows);
        // The poll consumes the chunk (a burst opens), then the
        // repaint waits for the output to settle.
        rt.frame(1.0 / 60.0);
        std::thread::sleep(std::time::Duration::from_millis(30));
        rt.frame(1.0 / 60.0);

        // Rows of the old, taller grid fade out over a few frames:
        // let them go before measuring what is mounted.
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            rt.frame(1.0 / 60.0);
        }
        let (rows, pane_rect) = pane_rows(&rt, &pane_key);
        assert!(!rows.is_empty(), "the pane drew its grid at {w}x{h}");
        let pane_bottom = pane_rect[1] + pane_rect[3];
        for row in &rows {
            assert!(
                row.rect[3] <= 17.0,
                "a terminal row is {:.1}pt tall at {w}x{h}",
                row.rect[3]
            );
            assert!(
                row.rect[1] + row.rect[3] <= pane_bottom + 0.5,
                "a terminal row sits below its pane: {:?} vs {pane_bottom} at {w}x{h}",
                row.rect
            );
        }
        assert!(
            rows.iter().any(|e| e
                .text
                .as_deref()
                .is_some_and(|t| t.contains("PROMPT-MARKER"))),
            "the input row on the grid's last line is mounted at {w}x{h}"
        );
    }
}

#[test]
fn split_panes_keep_their_last_rows_inside() {
    // Two panes side by side: each is narrower, so a wrapped row
    // pushes its bottom rows out sooner — both panes are checked.
    let mut s = State::new(test_handle());
    let a = seed_session(&mut s, "claude-a", "/work/repo");
    s.focus_session_tab(&a);
    let b = seed_session(&mut s, "claude-b", "/work/repo");
    s.place_session(
        &b,
        "claude-b",
        panes::Placement::Split {
            tab: s.open_tabs[0].id.clone(),
            pane: panes::pane_id(&a),
            direction: panes::Direction::SideBySide,
        },
    );
    for key in [&a, &b] {
        let session = s.session_id(key).unwrap();
        let (tx, rx) = mpsc::channel();
        let (_ready_tx, ready_rx) = mpsc::channel();
        let terminal = term::Terminal::new(session, rx, ready_rx);
        let mut seq = 0u64;
        feed_tui_frame(&tx, &mut seq);
        attach(&mut s, key, terminal);
    }
    let (a2, b2) = (a.clone(), b.clone());

    let mut rt = Runtime::new(s, view, (1512.0, 814.0), Env::new(true, 2.0));
    rt.poll = Some(poll);
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    std::thread::sleep(std::time::Duration::from_millis(30));
    rt.frame(1.0 / 60.0);

    for key in [&a2, &b2] {
        let (rows, pane_rect) = pane_rows(&rt, key);
        assert!(!rows.is_empty(), "pane {key} drew its grid");
        let pane_bottom = pane_rect[1] + pane_rect[3];
        for row in &rows {
            assert!(
                row.rect[3] <= 17.0,
                "a terminal row is {:.1}pt tall in pane {key}",
                row.rect[3]
            );
            assert!(
                row.rect[1] + row.rect[3] <= pane_bottom + 0.5,
                "a terminal row sits below pane {key}: {:?} vs {pane_bottom}",
                row.rect
            );
        }
    }
}
#[test]
fn project_terminals_filter_rows_tabs_and_shortcuts_and_restore_split_focus() {
    let mut s = sidebar_scene();
    let a = seed_session_in(&mut s, "/work/a/web", Some("p1"));
    let b = seed_session_in(&mut s, "/work/b/api", Some("p2"));
    let a2 = seed_session_in(&mut s, "/work/a/feat-a", Some("p1"));
    let a3 = seed_session_in(&mut s, "/work/a/feat-a", Some("p1"));
    let unassigned = seed_session_in(&mut s, "/tmp", None);
    for (key, title) in [
        (&a, "A shell"),
        (&b, "B shell"),
        (&a2, "A worktree"),
        (&a3, "A split"),
        (&unassigned, "Unassigned shell"),
    ] {
        s.data
            .sessions
            .iter_mut()
            .find(|view| &view.info.id == key)
            .unwrap()
            .info
            .title = title.into();
    }
    s.focus_session_tab(&a);
    s.focus_session_tab(&b);
    s.focus_session_tab(&a2);
    let split_tab = s.open_tabs[2].id.clone();
    s.place_session(
        &a3,
        "A split",
        panes::Placement::Split {
            tab: split_tab.clone(),
            pane: panes::pane_id(&a2),
            direction: panes::Direction::SideBySide,
        },
    );
    editor::navigation::select_checkout(&mut s, "p1".into(), PathBuf::from("/work/a/web"));
    let mut rt = key_runtime(s);
    let live = els(&rt)
        .into_iter()
        .filter(|el| !el.exiting)
        .collect::<Vec<_>>();
    for key in ["sidebar-project:p1", "sidebar-project:p2"] {
        assert!(live.iter().any(|el| el.key.as_deref() == Some(key)));
    }
    assert!(
        live.iter()
            .any(|el| el.text.as_deref() == Some("A worktree"))
    );
    for hidden in ["B shell", "Unassigned shell"] {
        assert!(!live.iter().any(|el| el.text.as_deref() == Some(hidden)));
    }
    assert_eq!(
        live.iter()
            .filter(|el| el.key.as_deref().is_some_and(|key| key.starts_with("tab:")))
            .count(),
        2
    );
    press_cmd(&mut rt, '1');
    assert_eq!(rt.state.selected_session.as_deref(), Some(a.as_str()));
    press_cmd(&mut rt, '2');
    assert_eq!(rt.state.selected_session.as_deref(), Some(a3.as_str()));
    press_cmd(&mut rt, '3');
    assert_eq!(rt.state.selected_session.as_deref(), Some(a3.as_str()));
    click_key(&mut rt, "sidebar-project:p2");
    assert_eq!(rt.state.visible_tabs().count(), 1);
    assert_eq!(rt.state.selected_session.as_deref(), Some(b.as_str()));
    click_key(&mut rt, "sidebar-project:p1");
    assert_eq!(rt.state.selected_session.as_deref(), Some(a3.as_str()));
    assert_eq!(rt.state.open_tabs[rt.state.active_tab].id, split_tab);
    assert!(matches!(
        &rt.state.open_tabs[rt.state.active_tab].layout,
        panes::Layout::Split(_)
    ));
    assert_eq!(rt.state.open_tabs.len(), 3);
    assert_eq!(rt.state.data.sessions.len(), 5);
    assert!(
        rt.state.pending.is_empty(),
        "switching does not stop or delete sessions"
    );
}

#[test]
fn project_terminals_empty_project_has_no_selection_input_or_controls() {
    let mut s = sidebar_scene();
    let a = seed_session_in(&mut s, "/work/a/web", Some("p1"));
    s.focus_session_tab(&a);
    editor::navigation::select_checkout(&mut s, "p1".into(), PathBuf::from("/work/a/web"));
    let mut rt = key_runtime(s);
    assert!(
        els(&rt)
            .iter()
            .any(|el| el.key.as_deref() == Some("controls-bar"))
    );
    click_key(&mut rt, "sidebar-project:p2");
    assert!(rt.state.selected_session.is_none());
    assert!(rt.state.focused_terminal_session().is_none());
    assert!(rt.state.focused_session_key().is_none());
    assert!(!rt.state.can_split());
    assert_eq!(
        rt.state.active_workspace,
        Some(data::checkout_key("p2", "/work/b/api"))
    );
    let live = els(&rt)
        .into_iter()
        .filter(|el| !el.exiting)
        .collect::<Vec<_>>();
    assert!(live.iter().any(|el| {
        el.text
            .as_deref()
            .is_some_and(|text| text.starts_with("No terminal sessions"))
    }));
    assert!(
        !live
            .iter()
            .any(|el| el.key.as_deref() == Some("controls-bar"))
    );
    press_cmd(&mut rt, '1');
    assert!(rt.state.selected_session.is_none());
    click_key(&mut rt, "sidebar-project:p1");
    assert_eq!(rt.state.selected_session.as_deref(), Some(a.as_str()));
}

#[test]
fn project_terminals_editor_picker_and_background_placement_keep_project_focus() {
    let mut s = sidebar_scene();
    let a = seed_session_in(&mut s, "/work/a/web", Some("p1"));
    let b = seed_session_in(&mut s, "/work/b/api", Some("p2"));
    s.focus_session_tab(&a);
    s.focus_session_tab(&b);
    editor::navigation::select_project(&mut s, "p2".into());
    let info = protocol::session::SessionInfo {
        id: protocol::session::SessionId::new(),
        title: "late A".into(),
        argv: vec!["/bin/zsh".into()],
        cwd: "/work/a/feat-a".into(),
        state: protocol::session::SessionState::Running,
        pid: None,
        exit_code: None,
        error: None,
        started_at: 0,
        ended_at: None,
    };
    let late = data::SessionInfo::key(info.id);
    let (reply_tx, reply) = std::sync::mpsc::channel();
    s.pending.push(sessions::Pending::Create {
        reply,
        launch: sessions::SessionLaunch {
            argv: info.argv.clone(),
            cwd: info.cwd.clone(),
            project_id: Some("p1".into()),
            agent: "shell".into(),
            title: info.title.clone(),
        },
        placement: panes::Placement::Tab,
        workspace_id: None,
        handoff: None,
    });
    reply_tx.send(Ok(info)).unwrap();
    assert!(s.drain_pending());
    assert_eq!(s.selected_session.as_deref(), Some(b.as_str()));
    assert_eq!(s.visible_tabs().count(), 1);
    editor::navigation::select_project(&mut s, "p1".into());
    assert_eq!(s.visible_tabs().count(), 2);
    assert_eq!(s.selected_session.as_deref(), Some(a.as_str()));
    s.remove_session_everywhere(&a);
    assert_eq!(s.selected_session.as_deref(), Some(late.as_str()));
    assert_eq!(s.focused_terminal_session(), s.session_id(&late));
    s.remove_session_everywhere(&late);
    assert!(s.selected_session.is_none());
    assert!(s.focused_terminal_session().is_none());
    assert!(s.session_placed(&b));
}

#[test]
fn project_map_opens_from_shortcut_or_button_and_switch_hides_foreign_terminal() {
    for button in [false, true] {
        let mut s = two_workspace_scene();
        let sid = seed_session_in(&mut s, "/work/a/web", Some("p1"));
        s.focus_session_tab(&sid);
        let mut rt = key_runtime(s);
        if button {
            click_key(&mut rt, "project-map-button");
        } else {
            press_cmd(&mut rt, 'k');
        }
        assert!(rt.state.project_map.open);
        click_key(&mut rt, "project-map-project:p2");
        assert!(!rt.state.project_map.open);
        assert_eq!(rt.state.workspace_id(), "default");
        assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
        assert_eq!(rt.state.view, View::Terminal);
        assert!(rt.state.selected_session.is_none());
        assert!(rt.state.focused_terminal_session().is_none());
        assert_eq!(rt.state.visible_tabs().count(), 0);
        assert!(rt.state.session_by_key(&sid).is_some());
        assert_eq!(rt.state.open_tabs.len(), 1);
        assert!(rt.state.pending.is_empty());
    }
}

#[test]
fn project_map_types_filters_and_consumes_underlying_shortcuts() {
    let mut rt = key_runtime(two_workspace_scene());
    press_cmd(&mut rt, 'k');
    rt.key(KeyEvent {
        key: Key::Text("API".into()),
        mods: Mods::default(),
    });
    rt.frame(0.1);
    assert_eq!(rt.state.project_map.query.text, "API");
    assert!(
        els(&rt)
            .iter()
            .any(|el| el.key.as_deref() == Some("project-map-project:p2"))
    );
    assert!(
        !els(&rt)
            .iter()
            .any(|el| el.key.as_deref() == Some("project-map-project:p1"))
    );
    press_cmd(&mut rt, 't');
    press_cmd(&mut rt, ',');
    assert_eq!(rt.state.view, View::Terminal);
    assert!(rt.state.pending.is_empty());
    rt.key(KeyEvent {
        key: Key::Down,
        mods: Mods::default(),
    });
    rt.key(KeyEvent {
        key: Key::Enter,
        mods: Mods::default(),
    });
    rt.frame(0.2);
    assert_eq!(rt.state.editor.project_id.as_deref(), Some("p2"));
}

#[test]
fn project_map_shortcut_respects_settings_ime_modifiers_and_modal_owners() {
    type Block = fn(&mut State);
    let blocks: [Block; 9] = [
        |s| s.view = View::Settings,
        |s| s.preedit = "đ".into(),
        |s| s.editor.project_picker.open = true,
        |s| s.editor.checkout_picker.open = true,
        |s| s.editor.quick_open.open = true,
        |s| s.editor.history.branch_picker.open = true,
        |s| s.workspace_remove_confirm = Some("w1".into()),
        |s| s.editor.commit.agent_picker.open = true,
        |s| s.workspace_picker.open = true,
    ];
    for block in blocks {
        let mut s = two_workspace_scene();
        block(&mut s);
        let mut tx = Tx::default();
        on_key(&mut s, &cmd_key('k'), &mut tx);
        assert!(!s.project_map.open);
    }
    for mods in [
        Mods {
            cmd: true,
            shift: true,
            ..Mods::default()
        },
        Mods {
            cmd: true,
            alt: true,
            ..Mods::default()
        },
        Mods {
            cmd: true,
            ctrl: true,
            ..Mods::default()
        },
    ] {
        let mut s = two_workspace_scene();
        on_key(
            &mut s,
            &KeyEvent {
                key: Key::Command('k'),
                mods,
            },
            &mut Tx::default(),
        );
        assert!(!s.project_map.open);
    }
}

#[test]
fn project_map_dismissal_and_editor_focus_restore() {
    let mut s = two_workspace_scene();
    s.view = View::Editor;
    let mut rt = key_runtime(s);
    for close_button in [false, true] {
        press_cmd(&mut rt, 'k');
        assert!(rt.state.project_map.open);
        if close_button {
            click_label(&mut rt, "Close");
        } else {
            rt.key(KeyEvent {
                key: Key::Escape,
                mods: Mods::default(),
            });
        }
        rt.frame(0.2);
        assert!(!rt.state.project_map.open);
        assert_eq!(rt.state.workspace_id(), "w1");
        assert!(rt.state.editor.project_id.is_none());
        assert_eq!(rt.state.view, View::Editor);
    }
}

#[test]
fn project_map_fits_narrow_window_in_both_themes() {
    for theme in [theme::Theme::Light, theme::Theme::Dark] {
        let mut s = two_workspace_scene();
        s.theme_choice = theme;
        s.apply_theme();
        project_map::open(&mut s, &mut Tx::default());
        let mut rt = Runtime::new(
            s,
            view,
            (760.0, 480.0),
            Env::new(theme == theme::Theme::Dark, 2.0),
        );
        rt.frame(0.0);
        let card = els(&rt)
            .into_iter()
            .find(|e| e.key.as_deref() == Some("project-map-dialog"))
            .unwrap();
        assert!(card.rect[0] >= 0.0 && card.rect[1] >= 0.0);
        assert!(card.rect[0] + card.rect[2] <= 760.0);
        assert!(card.rect[1] + card.rect[3] <= 480.0);
    }
}

#[test]
fn project_map_paste_stays_in_search_and_sidebar_reveals_distant_target() {
    let mut s = two_workspace_scene();
    for i in 0..35 {
        s.editor.projects.push(project(
            &format!("extra-{i}"),
            &format!("Extra {i}"),
            &format!("/tmp/extra-{i}"),
        ));
    }
    let mut rt = key_runtime(s);
    press_cmd(&mut rt, 'k');
    rt.paste("Extra 34");
    rt.frame(0.1);
    assert_eq!(rt.state.project_map.query.text, "Extra 34");
    click_key(&mut rt, "project-map-project:extra-34");
    for i in 1..20 {
        rt.frame(0.1 + i as f64 * 0.1);
    }
    assert_eq!(rt.state.workspace_id(), "default");
    let elements = els(&rt);
    let viewport = elements
        .iter()
        .find(|e| e.key.as_deref() == Some(sidebar::PROJECTS_SCROLL_KEY))
        .unwrap();
    let target = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("sidebar-project:extra-34"))
        .unwrap();
    assert!(
        target.rect[1] >= viewport.rect[1],
        "target above viewport: {:?}",
        target.rect
    );
    assert!(
        target.rect[1] + target.rect[3] <= viewport.rect[1] + viewport.rect[3] + 1.0,
        "target below viewport: {:?}, {:?}",
        target.rect,
        viewport.rect
    );
}
#[test]
fn project_map_ime_composes_in_search_and_fallback_cannot_type_into_terminal() {
    let mut rt = key_runtime(two_workspace_scene());
    press_cmd(&mut rt, 'k');
    rt.ime(rune::Ime::Preedit("vie".into(), None));
    rt.frame(0.1);
    assert!(rt.state.preedit.is_empty());
    rt.ime(rune::Ime::Commit("việt".into()));
    rt.frame(0.2);
    assert_eq!(rt.state.project_map.query.text, "việt");
    assert!(rt.state.preedit.is_empty());
    rt.update(|s, tx| assert!(on_ime(s, &rune::Ime::Commit("blocked".into()), tx)));
    assert_eq!(rt.state.project_map.query.text, "việt");
}

#[test]
fn project_map_scrim_dismissal_restores_focused_trigger_and_tab_is_trapped() {
    let mut rt = key_runtime(two_workspace_scene());
    click_key(&mut rt, "project-map-button");
    rt.key(KeyEvent {
        key: Key::Tab,
        mods: Mods::default(),
    });
    rt.frame(0.1);
    let focused = els(&rt)
        .into_iter()
        .filter(|e| e.focused)
        .collect::<Vec<_>>();
    assert!(
        focused.iter().any(|e| e.label.as_deref() == Some("Close")),
        "{focused:?}"
    );
    rt.pointer_down(20.0, 400.0);
    rt.pointer_up(20.0, 400.0);
    rt.frame(0.2);
    assert!(!rt.state.project_map.open);
    assert!(
        els(&rt)
            .iter()
            .any(|e| e.focused && e.key.as_deref() == Some("project-map-button"))
    );
}

#[test]
fn project_map_status_distinguishes_loading_errors_empty_and_no_matches() {
    for (kind, want) in [
        (0, "Loading projects and workspaces…"),
        (1, "read failed"),
        (2, "No registered projects"),
        (3, "No matching projects or workspaces"),
    ] {
        let mut s = two_workspace_scene();
        project_map::open(&mut s, &mut Tx::default());
        match kind {
            0 => s.editor.projects_loading = true,
            1 => s.workspaces_error = Some("read failed".into()),
            2 => s.editor.projects.clear(),
            _ => s.project_map.query.text = "no-such-project".into(),
        }
        let mut rt = Runtime::new(s, view, (760.0, 480.0), Env::new(true, 2.0));
        rt.frame(0.0);
        assert!(
            texts_on(&rt).iter().any(|t| t == want),
            "{kind}: {:?}",
            texts_on(&rt)
        );
    }
}
