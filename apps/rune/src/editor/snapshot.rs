//! Agent-readable code snapshots: the selection expanded to whole lines, or
//! the whole buffer, carrying the checkout-relative path, its 1-based line
//! range, the language, and a fenced code block. The user copies one to the
//! clipboard or sends it into the focused terminal session.

use rune::{TextEdit, Tx};

use crate::State;
use crate::editor::draft::OpenDoc;
use crate::editor::syntax::language_for_path;

/// Render a snapshot of `edit`'s selection — expanded to the whole lines it
/// touches — or the whole buffer when nothing is selected.
pub fn snapshot(path: &str, edit: &TextEdit, dirty: bool) -> String {
    let (a, b) = edit.range();
    let (start_line, end_line, raw) = if a == b {
        (1, 0, edit.text.as_str())
    } else {
        let first = edit.line_of_byte(a);
        let last = edit.line_of_byte(b.saturating_sub(1));
        let start = edit.line_range(first).start;
        let end = edit.line_range(last).end;
        (first + 1, last + 1, &edit.text[start..end])
    };
    let body = normalize(raw);
    let body = body.trim_end_matches('\n');
    let end_line = if a == b {
        body.split('\n').count().max(1)
    } else {
        end_line
    };
    let lang = language_for_path(path);
    let empty_file = a == b && body.is_empty();
    let header = if empty_file {
        if dirty {
            format!("{path} (empty, unsaved)")
        } else {
            format!("{path} (empty)")
        }
    } else {
        let range = if start_line == end_line {
            start_line.to_string()
        } else {
            format!("{start_line}-{end_line}")
        };
        match (lang, dirty) {
            (Some(lang), true) => format!("{path}:{range} ({lang}, unsaved)"),
            (Some(lang), false) => format!("{path}:{range} ({lang})"),
            (None, true) => format!("{path}:{range} (unsaved)"),
            (None, false) => format!("{path}:{range}"),
        }
    };
    let fence = fence_for(body);
    let info = lang.unwrap_or("");
    let mut out = String::with_capacity(header.len() + body.len() + fence.len() * 2 + 16);
    out.push_str(&header);
    out.push_str("\n\n");
    out.push_str(&fence);
    out.push_str(info);
    out.push('\n');
    if !body.is_empty() {
        out.push_str(body);
        out.push('\n');
    }
    out.push_str(&fence);
    out.push('\n');
    out
}

/// ⌘⇧C: copy the open document's snapshot. The code surface must hold focus
/// so a chord in another field never copies a stale selection.
pub fn copy(s: &mut State, tx: &mut Tx) -> bool {
    let text = match doc_for(s, true) {
        Ok(doc) => snapshot(&doc.path, &doc.code.edit, doc.dirty()),
        Err(message) => {
            s.notice = Some(message.into());
            return true;
        }
    };
    tx.copy(text);
    s.notice = Some("Snapshot copied to clipboard".into());
    true
}

/// The footer chip: send the open document's snapshot to the focused terminal
/// session without requiring the code surface to hold focus.
pub fn send(s: &mut State, tx: &mut Tx) -> bool {
    send_inner(s, tx, false)
}

/// ⌘⇧Enter: as the chip, but only while the code surface holds focus.
pub fn send_chord(s: &mut State, tx: &mut Tx) -> bool {
    send_inner(s, tx, true)
}

fn send_inner(s: &mut State, _tx: &mut Tx, require_focus: bool) -> bool {
    let text = match doc_for(s, require_focus) {
        Ok(doc) => snapshot(&doc.path, &doc.code.edit, doc.dirty()),
        Err(message) => {
            s.notice = Some(message.into());
            return true;
        }
    };
    let Some(session) = s.focused_terminal_session() else {
        s.notice = Some("No live terminal session to send to".into());
        return true;
    };
    let title = s
        .focused_session_key()
        .and_then(|key| s.session_by_key(&key))
        .map(|view| view.title())
        .unwrap_or_else(|| "terminal".into());
    if s.paste_into_terminal(session, &text) {
        s.notice = Some(format!("Snapshot sent to {title}"));
    } else {
        s.notice = Some("No live terminal session to send to".into());
    }
    true
}

fn doc_for(s: &State, require_focus: bool) -> Result<&OpenDoc, &'static str> {
    let doc = s.editor.doc.as_ref().ok_or("Open a file first")?;
    if !doc.editable() {
        return Err("Snapshot needs an editable text file");
    }
    if require_focus && !doc.code.edit.focused {
        return Err("Click into the code first");
    }
    Ok(doc)
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn fence_for(body: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in body.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::session::{SessionId, SessionInfo, SessionState};

    fn edit(text: &str) -> TextEdit {
        TextEdit::new(text)
    }

    fn open(path: &str, text: &str) -> OpenDoc {
        OpenDoc::new(
            path,
            &app_core::git_view::GitViewFile {
                path: path.into(),
                kind: "text",
                text: Some(text.into()),
                truncated: false,
                size: text.len() as u64,
            },
        )
    }

    fn binary(path: &str) -> OpenDoc {
        OpenDoc::new(
            path,
            &app_core::git_view::GitViewFile {
                path: path.into(),
                kind: "binary",
                text: None,
                truncated: false,
                size: 0,
            },
        )
    }

    #[test]
    fn a_selection_expands_to_the_lines_it_touches() {
        let mut e = edit("alpha\nbeta\ngamma\n");
        e.set_selection(7, 9);
        assert_eq!(
            snapshot("src/a.rs", &e, false),
            "src/a.rs:2 (rust)\n\n```rust\nbeta\n```\n"
        );
    }

    #[test]
    fn a_selection_that_ends_at_a_line_start_stops_on_the_previous_line() {
        let mut e = edit("one\ntwo\nthree\n");
        e.set_selection(2, 4);
        assert_eq!(
            snapshot("p.rs", &e, false),
            "p.rs:1 (rust)\n\n```rust\none\n```\n"
        );
    }

    #[test]
    fn nothing_selected_copies_the_whole_file() {
        let e = edit("one\ntwo\n");
        assert_eq!(
            snapshot("a.py", &e, false),
            "a.py:1-2 (python)\n\n```python\none\ntwo\n```\n"
        );
    }

    #[test]
    fn a_single_line_file_uses_one_number() {
        let e = edit("one");
        assert_eq!(
            snapshot("a.rs", &e, false),
            "a.rs:1 (rust)\n\n```rust\none\n```\n"
        );
    }

    #[test]
    fn a_dirty_buffer_is_marked_unsaved() {
        let e = edit("one\n");
        assert_eq!(
            snapshot("a.rs", &e, true),
            "a.rs:1 (rust, unsaved)\n\n```rust\none\n```\n"
        );
    }

    #[test]
    fn the_fence_outgrows_a_backtick_run() {
        let e = edit("```\ncode\n```\n");
        assert_eq!(
            snapshot("a.rs", &e, false),
            "a.rs:1-3 (rust)\n\n````rust\n```\ncode\n```\n````\n"
        );
    }

    #[test]
    fn crlf_is_normalized() {
        let e = edit("one\r\ntwo\r\n");
        assert_eq!(
            snapshot("a.rs", &e, false),
            "a.rs:1-2 (rust)\n\n```rust\none\ntwo\n```\n"
        );
    }

    #[test]
    fn an_empty_file_says_so() {
        let e = edit("");
        assert_eq!(
            snapshot("a.rs", &e, false),
            "a.rs (empty)\n\n```rust\n```\n"
        );
    }

    #[test]
    fn an_unknown_language_leaves_the_fence_bare() {
        let e = edit("x\n");
        assert_eq!(
            snapshot("notes.xyz", &e, false),
            "notes.xyz:1\n\n```\nx\n```\n"
        );
    }

    #[test]
    fn a_selected_empty_line_keeps_its_range() {
        let mut e = edit("one\n\nthree\n");
        e.set_selection(4, 5);
        assert_eq!(
            snapshot("a.rs", &e, false),
            "a.rs:2 (rust)\n\n```rust\n```\n"
        );
    }

    #[test]
    fn copy_without_a_document_only_notices() {
        let mut s = State::new(crate::test_handle());
        assert!(copy(&mut s, &mut Tx::default()));
        assert_eq!(s.notice.as_deref(), Some("Open a file first"));
    }

    #[test]
    fn copy_without_the_code_surface_focused_only_notices() {
        let mut s = State::new(crate::test_handle());
        s.editor.doc = Some(open("src/a.rs", "one\n"));
        assert!(copy(&mut s, &mut Tx::default()));
        assert_eq!(s.notice.as_deref(), Some("Click into the code first"));
    }

    #[test]
    fn copy_refuses_a_binary_document() {
        let mut s = State::new(crate::test_handle());
        s.editor.doc = Some(binary("a.bin"));
        assert!(copy(&mut s, &mut Tx::default()));
        assert_eq!(
            s.notice.as_deref(),
            Some("Snapshot needs an editable text file")
        );
    }

    #[test]
    fn sending_without_a_live_session_only_notices() {
        let mut s = State::new(crate::test_handle());
        s.editor.doc = Some(open("src/a.rs", "one\n"));
        assert!(send(&mut s, &mut Tx::default()));
        assert_eq!(
            s.notice.as_deref(),
            Some("No live terminal session to send to")
        );
    }

    #[test]
    fn sending_writes_to_the_focused_session() {
        let mut s = State::new(crate::test_handle());
        let info = SessionInfo {
            id: SessionId::new(),
            title: "claude".into(),
            argv: vec!["claude".into()],
            cwd: "/work".into(),
            state: SessionState::Running,
            pid: None,
            exit_code: None,
            error: None,
            started_at: 0,
            ended_at: None,
        };
        let key = crate::data::SessionInfo::key(info.id);
        s.upsert_session(info);
        s.focus_session_tab(&key);
        s.editor.doc = Some(open("src/a.rs", "one\n"));
        assert!(send(&mut s, &mut Tx::default()));
        assert_eq!(s.notice.as_deref(), Some("Snapshot sent to claude"));
    }

    #[test]
    fn the_send_chord_needs_the_code_surface_focused() {
        let mut s = State::new(crate::test_handle());
        s.editor.doc = Some(open("src/a.rs", "one\n"));
        assert!(send_chord(&mut s, &mut Tx::default()));
        assert_eq!(s.notice.as_deref(), Some("Click into the code first"));
    }
}
