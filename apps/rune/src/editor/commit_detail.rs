//! Commit detail metadata pane: hash + copy, subject, HEAD/ref badges,
//! author/committer lines, parents, body, changed-file list. The patch itself
//! is rendered by the shared `patch` pane; this module owns the metadata.
//! Also hosts the "Uncommitted changes" detail shown when the synthetic row
//! is selected.

use std::sync::Mutex;

use app_core::git_view::GitViewCommit;
use rune::{El, TextStyle, Tx, col, el, row};
use rune_kit::ScrollBar;

use crate::State;
use crate::editor::changes::{self, TreeRow};
use crate::editor::history::{self, entry_name, format_time, ref_badge, status_label};
use crate::editor::state::{ListMenu, ListView, Section};
use crate::theme::{self, Ink, T11M, T11MN_BADGE, T12, T12M, T12TN, btn_sm, txt};

/// `font-mono text-xs` — changed-file paths (desktop `ChangedFiles` rows).
const T12MONO: TextStyle = TextStyle::new(12.0, 400.0).mono();

/// Enter or Space activates a focused `role("button")` element — Rune's press
/// is pointer-only, so `on_key` covers keyboard activation.
fn activate_key(ev: &rune::KeyEvent) -> bool {
    matches!(&ev.key, rune::Key::Enter)
        || matches!(&ev.key, rune::Key::Text(t) if t.as_str() == " ")
}

/// Required public entry: renders the selected commit's metadata header.
pub fn view(s: &State) -> El {
    let ink = &s.ink;
    if s.editor.history.selected.as_deref() == Some(history::UNCOMMITTED_ID) {
        return uncommitted_view(s, ink);
    }
    let commit = s
        .editor
        .history
        .selected
        .as_deref()
        .and_then(|id| s.editor.history.commits.iter().find(|c| c.id == id));
    match commit {
        Some(c) => view_for(s, ink, c),
        None => history::empty_pane(ink, "Select a commit to see its patch", ""),
    }
}

/// Metadata header for `commit` (hash, subject, refs, author/committer, parents,
/// body, changed files). The scroll viewport sizes to content up to the cap
/// `meta_cap` computes and scrolls internally beyond it.
pub(crate) fn view_for(s: &State, ink: &Ink, commit: &GitViewCommit) -> El {
    let detail = s.editor.history.detail.as_ref();
    let patch_detail = detail.and_then(|p| p.detail.as_ref());

    let mut body = col().w_full().px(12.0).py(6.0).gap(4.0);

    // Hash + copy.
    let mut top = row().w_full().items_center().gap(6.0);
    top = top.child(txt(
        &commit.id.chars().take(8).collect::<String>(),
        T12TN,
        ink.text,
    ));
    let oid_key = commit.id.clone();
    let oid_press = commit.id.clone();
    top = top.child(
        el().size(20.0, 20.0)
            .items_center()
            .justify_center()
            .hover(ink.panel)
            .cursor(rune::Cursor::Pointer)
            .role("button")
            .label("Copy commit hash")
            .focusable(true)
            .focus_ring(2.0, 1.0, ink.accent)
            .child(txt("⧉", T12, ink.muted))
            .on_press_tx(move |st: &mut State, tx: &mut Tx| {
                mark_copied(st, &oid_press, tx);
                tx.copy(oid_press.clone());
            })
            .on_key(move |st: &mut State, ev: &rune::KeyEvent, tx: &mut Tx| {
                if activate_key(ev) {
                    mark_copied(st, &oid_key, tx);
                    tx.copy(oid_key.clone());
                    true
                } else {
                    false
                }
            }),
    );
    // Desktop `copyNote`: a muted "Copied" span beside the button, cleared
    // by a 1500ms timer (`history::poll` expires it; the handler wakes a
    // frame so it vanishes on time). `tx.copy` reports no failure, so only
    // the success note exists natively.
    if copied_note(s, &commit.id) {
        top = top.child(txt("Copied", T12, ink.muted));
    }
    body = body.child(top);

    body = body.child(txt(&commit.subject, T12M, ink.text));

    // HEAD + ref badges — desktop `ui/badge.tsx` is a 20pt rounded-full pill
    // (HistoryTable only shrinks the font); the 14/15pt boxes used here let
    // the 16pt line-height glyphs poke through the border.
    if commit.head || !commit.refs.is_empty() {
        let mut badges = row().items_center().gap(4.0);
        if commit.head {
            badges = badges.child(
                el().px(6.0)
                    .h(20.0)
                    .radius(10.0)
                    .items_center()
                    .justify_center()
                    .fill(ink.accent)
                    .label("HEAD")
                    .child(txt("HEAD", T11MN_BADGE, ink.white)),
            );
        }
        for r in &commit.refs {
            let (label, is_tag, is_remote) = ref_badge(r);
            let bc = if is_tag {
                ink.warning
            } else if is_remote {
                ink.muted
            } else {
                ink.accent
            };
            badges = badges.child(
                el().px(6.0)
                    .h(20.0)
                    .radius(10.0)
                    .items_center()
                    .justify_center()
                    .border(1.0, bc)
                    .label(r)
                    .child({
                        txt(&label, T11MN_BADGE, bc)
                            .max_w(160.0)
                            .truncate_middle()
                            .help_when_truncated(label.clone())
                    }),
            );
        }
        body = body.child(badges);
    }

    // Author / committer lines.
    let author_email = patch_detail.map(|d| d.author_email.as_str()).unwrap_or("");
    let author_line = if author_email.is_empty() {
        commit.author.clone()
    } else {
        format!("{} <{}>", commit.author, author_email)
    };
    body = body.child(txt(
        &format!("{author_line} · {}", format_time(commit.time)),
        T12,
        ink.muted,
    ));

    if let Some(d) = patch_detail
        && (d.committer != commit.author || d.committer_email != d.author_email)
    {
        let ce = if d.committer_email.is_empty() {
            String::new()
        } else {
            format!(" <{}>", d.committer_email)
        };
        body = body.child(txt(
            &format!("Committer: {}{}", d.committer, ce),
            T12,
            ink.muted,
        ));
    }

    // Parents: clickable when loaded, muted-disabled otherwise.
    if !commit.parents.is_empty() {
        let mut prow = row()
            .items_center()
            .gap(4.0)
            .child(txt("Parents", T12, ink.muted));
        for pid in &commit.parents {
            let short = pid.chars().take(8).collect::<String>();
            let enabled = s.editor.history.commits.iter().any(|c| c.id == *pid);
            let oid = pid.clone();
            let key_oid = pid.clone();
            let btn = txt(&short, T12TN, if enabled { ink.accent } else { ink.muted })
                .px(4.0)
                .py(2.0)
                .role("button")
                .label(format!("Select parent {short}"))
                .focusable(enabled)
                .focus_ring(2.0, 1.0, ink.accent);
            let btn = if enabled {
                btn.cursor(rune::Cursor::Pointer)
                    .hover(ink.panel)
                    .on_press_tx(move |st: &mut State, tx: &mut Tx| {
                        history::select_commit_pub(st, &oid, tx);
                    })
                    .on_key(move |st: &mut State, ev: &rune::KeyEvent, tx: &mut Tx| {
                        if activate_key(ev) {
                            history::select_commit_pub(st, &key_oid, tx);
                            true
                        } else {
                            false
                        }
                    })
            } else {
                btn
            };
            prow = prow.child(btn);
        }
        body = body.child(prow);
    }

    // Body (clamped to ~6 lines with Show more/less, keyed by commit id).
    let body_text = patch_detail.map(|d| d.body.as_str()).unwrap_or("");
    if !body_text.trim().is_empty() {
        let lines = body_text.lines().count();
        let expanded = body_expanded(&commit.id);
        let clamp = !expanded && lines > 6;
        let shown = if clamp {
            body_text.lines().take(6).collect::<Vec<_>>().join("\n")
        } else {
            body_text.to_string()
        };
        body = body.child(txt(&shown, T12, ink.muted));
        if lines > 6 {
            let oid = commit.id.clone();
            let key_oid = commit.id.clone();
            body = body.child(
                btn_sm(
                    if expanded { "Show less" } else { "Show more" },
                    None,
                    false,
                    ink,
                )
                .on_press_tx({
                    let oid = oid.clone();
                    move |_st: &mut State, _tx: &mut Tx| {
                        set_body_expanded(&oid, !body_expanded(&oid))
                    }
                })
                .on_key(
                    move |_st: &mut State, ev: &rune::KeyEvent, _tx: &mut Tx| {
                        if activate_key(ev) {
                            set_body_expanded(&key_oid, !body_expanded(&key_oid));
                            true
                        } else {
                            false
                        }
                    },
                ),
            );
        }
    }

    if commit.parents.len() > 1 {
        body = body.child(txt(
            "Changes compared with the first parent",
            T12,
            ink.muted,
        ));
    }

    // Changed files metadata (status badge + name) — the patch pane renders
    // the diff itself below. Mounted even when empty so the list's View
    // menu still has somewhere to open.
    if let Some(p) = detail {
        body = body.child(changed_files(s, &p.files, ink, ChangedFileHost::History));
    }

    // Scrollable header region: the viewport carries the cap itself
    // (`.max_h` on the scroll element, the same bound `auto_grow` uses) so
    // its layout height — and therefore its scroll range — is
    // min(content, cap). The flex basis must stay `auto` — `.grow(..)`
    // would zero it (Rune's grow is `flex: g 1 0`) and collapse the
    // viewport to 0 pt, which is what hid all of this metadata in the CUA
    // smoke.
    let max_h = meta_cap(s);
    col()
        .key("commit-detail-meta")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(
            col()
                .w_full()
                .min_h(0.0)
                .max_h(max_h)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(body)
                .scroll_bar(&s.editor.kit),
        )
}

/// Desktop `COPY_FEEDBACK_MS`: how long the "Copied" note shows.
pub(crate) const COPY_FEEDBACK_MS: u64 = 1500;

/// Records the copy for the header's temporary "Copied" note and wakes a
/// frame just past the deadline so `history::poll` can expire it on time
/// (desktop's `setTimeout`).
pub(crate) fn mark_copied(st: &mut State, oid: &str, tx: &mut Tx) {
    st.editor.history.copied_hash = Some((oid.to_string(), std::time::Instant::now()));
    tx.wake_after(std::time::Duration::from_millis(COPY_FEEDBACK_MS + 100));
}

/// Whether the "Copied" note should render for this commit right now —
/// fresh timestamp and matching id (a different selection hides it, like
/// desktop's `useEffect` reset on `commit.id`).
fn copied_note(s: &State, oid: &str) -> bool {
    s.editor
        .history
        .copied_hash
        .as_ref()
        .is_some_and(|(id, at)| {
            id == oid && at.elapsed() < std::time::Duration::from_millis(COPY_FEEDBACK_MS)
        })
}

/// Height the metadata viewport may claim inside the detail split: its
/// content height up to `META_MAX_H`, never more than the pane minus the
/// patch pane's floor. Reads the pane's laid-out height (0 until the first
/// measure, when the configured `detail_height` is the safe estimate —
/// `.shrink()` only ever reduces the real pane below it).
fn meta_cap(s: &State) -> f32 {
    let pane_h = history::detail_pane_height(s);
    let configured = s.editor.history.layout.detail_height as f32;
    let avail = if pane_h > 0.0 { pane_h } else { configured };
    avail.clamp(120.0, META_MAX_H)
}

/// Cap for the metadata header inside the detail pane (desktop has no split —
/// everything shares one scroll — so this is the native bound that keeps the
/// patch pane visible). `meta_cap` tightens it further to the pane's
/// measured height minus the patch floor.
pub(crate) const META_MAX_H: f32 = 600.0;

/// Which state drives a changed-file list. The History commit detail is
/// `History`-bound (`view`, `file_menu`, `collapsed_dirs`); the stash
/// detail is `Flat` — always a plain list, no menu or collapse state.
#[derive(Clone, Copy)]
pub(crate) enum ChangedFileHost {
    History,
    Flat,
}

/// Changed-file list (`ChangedFiles` in panes.tsx): mono path with the
/// original name muted, an outline status badge in sans, and `+added` /
/// `−removed` counts in success/danger pushed to the row's end. For
/// `ChangedFileHost::History` the `View` (`List`/`Tree`) comes from
/// the shared `editor.list_view` and a secondary press anywhere on the list opens its
/// context menu; `Flat` always renders the plain list.
pub(crate) fn changed_files(
    s: &State,
    files: &[app_core::git_view::StatusEntry],
    ink: &Ink,
    host: ChangedFileHost,
) -> El {
    let count = files.len();
    let history = matches!(host, ChangedFileHost::History);
    let mut list = col()
        .w_full()
        .gap(2.0)
        .px(12.0)
        .py(8.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border);
    if history {
        // The list's `View` menu is list-level, like the Changes section's.
        list = list.on_secondary_press(|st: &mut State, p: rune::PointerPress, _tx: &mut Tx| {
            st.editor.history.file_menu = Some(ListMenu::Point { x: p.x, y: p.y });
        });
    }
    list = list.child(row().w_full().items_center().py(2.0).child(txt(
        &format!("{count} changed file{}", if count == 1 { "" } else { "s" }),
        T11M,
        ink.muted,
    )));
    match if history {
        s.editor.list_view
    } else {
        ListView::List
    } {
        ListView::List => {
            for f in files {
                list = list.child(changed_file_row(f, ink, 0.0));
            }
        }
        ListView::Tree => {
            let entries: Vec<(&app_core::git_view::StatusEntry, ())> =
                files.iter().map(|f| (f, ())).collect();
            let rows =
                changes::tree_rows(&entries, |p| s.editor.history.collapsed_dirs.contains(p));
            for trow in rows {
                list = list.child(match trow {
                    TreeRow::Dir { path, name, depth } => dir_row(s, ink, &path, &name, depth),
                    TreeRow::File { entry, depth, .. } => {
                        changed_file_row(entry, ink, depth as f32 * changes::DIR_INDENT)
                    }
                });
            }
        }
    }
    list
}

/// A directory header row in the detail's `Tree` view: chevron + folder +
/// last path segment; toggles `path` in `history.collapsed_dirs`.
fn dir_row(s: &State, ink: &Ink, path: &str, name: &str, depth: usize) -> El {
    let collapsed = s.editor.history.collapsed_dirs.contains(path);
    let key = path.to_string();
    el().key(format!("history-dir:{path}"))
        .w_full()
        .h(24.0)
        .px(2.0)
        .pl(4.0 + depth as f32 * changes::DIR_INDENT)
        .gap(4.0)
        .items_center()
        .radius(6.0)
        .hover(ink.text.alpha(0.06))
        .role("button")
        .label(format!("{path} directory"))
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .child(theme::ic(
            if collapsed {
                &theme::CHEVRON_RIGHT
            } else {
                &theme::CHEVRON_DOWN
            },
            11.0,
            ink.muted,
        ))
        .child(theme::ic(&theme::FOLDER, 12.0, ink.muted))
        .child(
            txt(name, T12M, ink.text)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(path.to_string()),
        )
        .on_press(move |st: &mut State| {
            if !st.editor.history.collapsed_dirs.remove(&key) {
                st.editor.history.collapsed_dirs.insert(key.clone());
            }
        })
}

/// One changed file in the commit detail: a press scrolls the patch pane to
/// that file's diff. `indent` is the `Tree` view's extra left padding.
fn changed_file_row(f: &app_core::git_view::StatusEntry, ink: &Ink, indent: f32) -> El {
    let path = f.path.clone();
    let key_path = f.path.clone();
    let mut r = row()
        .w_full()
        .items_center()
        .gap(8.0)
        .px(6.0)
        .pl(6.0 + indent)
        .py(4.0)
        .radius(4.0)
        .cursor(rune::Cursor::Pointer)
        .hover(ink.panel)
        .role("button")
        .label(format!("View diff for {}", f.path))
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .on_press_tx(move |st: &mut State, tx: &mut Tx| {
            if let Some(idx) = st.editor.history.diff.file_index(&path) {
                st.editor.history.diff.scroll_to_file(idx, tx);
            }
        })
        .on_key(move |st: &mut State, ev: &rune::KeyEvent, tx: &mut Tx| {
            if activate_key(ev) {
                if let Some(idx) = st.editor.history.diff.file_index(&key_path) {
                    st.editor.history.diff.scroll_to_file(idx, tx);
                }
                true
            } else {
                false
            }
        });
    r = r.child(
        el().px(6.0)
            .h(20.0)
            .radius(10.0)
            .items_center()
            .justify_center()
            .border(1.0, ink.border)
            .label(status_label(f.status))
            .child(txt(status_label(f.status), T11M, ink.muted)),
    );
    let mut name = row().min_w(0.0).grow(1.0).items_center().gap(0.0);
    if let Some(orig) = &f.orig_path {
        name = name.child(txt(&format!("{orig} → "), T12MONO, ink.muted));
    }
    name = name.child(
        txt(&f.path, T12MONO, ink.text)
            .min_w(0.0)
            .grow(1.0)
            .truncate_middle()
            .help_when_truncated(f.path.clone())
            .label(entry_name(f)),
    );
    r = r.child(name);
    if let Some(added) = f.added {
        r = r.child(el().grow(1.0)).child(
            row()
                .items_center()
                .gap(6.0)
                .child(txt(&format!("+{added}"), T12MONO, ink.success))
                .child(txt(
                    &format!("−{}", f.removed.unwrap_or(0)),
                    T12MONO,
                    ink.danger,
                )),
        );
    }
    r
}

/// `UncommittedDetail`: entry list + "Open in Changes".
pub(crate) fn uncommitted_view(s: &State, ink: &Ink) -> El {
    let mut entries: Vec<&app_core::git_view::StatusEntry> = Vec::new();
    if history::changes_fresh(s)
        && let Some(data) = s.editor.changes.data.as_ref()
    {
        entries.extend(data.conflicts.iter());
        entries.extend(data.staged.iter());
        entries.extend(data.unstaged.iter());
        entries.extend(data.untracked.iter());
    }
    let mut root = col().grow(1.0).min_h(0.0);
    root = root.child(
        row()
            .w_full()
            .items_center()
            .px(12.0)
            .py(6.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
            .child(txt("Uncommitted changes", T12M, ink.text))
            .child(el().grow(1.0))
            .child(btn_sm("Open in Changes", None, true, ink).on_press_tx(
                |st: &mut State, _tx: &mut Tx| {
                    st.editor.section = Section::Changes;
                    st.editor.visited.insert(Section::Changes);
                },
            )),
    );
    let mut list = col()
        .grow(1.0)
        .min_h(0.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .px(4.0);
    for e in entries {
        let mut r = row().w_full().items_center().gap(8.0).px(12.0).py(4.0);
        r = r.child(
            el().px(6.0)
                .h(20.0)
                .radius(10.0)
                .items_center()
                .justify_center()
                .border(1.0, ink.muted)
                .label(status_label(e.status))
                .child(txt(status_label(e.status), T11MN_BADGE, ink.muted)),
        );
        let full_name = entry_name(e);
        r = r.child(
            txt(&full_name, T12TN, ink.text)
                .min_w(0.0)
                .grow(1.0)
                .truncate_middle()
                .help_when_truncated(full_name),
        );
        list = list.child(r);
    }
    root.child(list.scroll_bar(&s.editor.kit))
}

// Body expand state: single-slot per commit id (desktop resets on commit
// change; a map keyed by id is equivalent for the visible commit).
static BODY_EXPANDED: Mutex<Option<(String, bool)>> = Mutex::new(None);

fn body_expanded(id: &str) -> bool {
    BODY_EXPANDED
        .lock()
        .map(|g| g.as_ref().is_some_and(|(k, v)| k == id && *v))
        .unwrap_or(false)
}

fn set_body_expanded(id: &str, v: bool) {
    if let Ok(mut g) = BODY_EXPANDED.lock() {
        *g = Some((id.to_string(), v));
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::state::{Patch, checkout_key};
    use crate::{State, View};
    use app_core::git_view::{GitViewCommit, GitViewCommitDetail, StatusEntry};
    use rune::{Env, Runtime};
    use std::path::PathBuf;

    /// Restores `GOKO_HOME` on drop so the throwaway home never leaks into
    /// parallel tests (same guard the changes-scene tests use).
    struct HomeGuard(Option<std::ffi::OsString>);

    impl HomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var_os("GOKO_HOME");
            // SAFETY: test-only; the pointed-at directory outlives the test.
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

    const MERGE_ID: &str = "0123456789abcdef0123456789abcdef01234567";
    const PARENT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PARENT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DIFF_RAW: &str = "diff --git a/src/a.rs b/src/a.rs\n\
index 1111111..2222222 100644\n\
--- a/src/a.rs\n\
+++ b/src/a.rs\n\
@@ -1,3 +1,4 @@\n\
 fn main() {\n\
+    println!(\"merged\");\n\
 }\n";

    fn oid(c: char) -> String {
        c.to_string().repeat(40)
    }

    fn commit(id: &str, parents: &[&str], subject: &str) -> GitViewCommit {
        GitViewCommit {
            id: id.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            refs: vec![],
            head: false,
            author: "A U Thor".into(),
            time: 1_700_000_000_000,
            subject: subject.into(),
        }
    }

    /// The CUA scene: editor view, checkout selected, History section with a
    /// merge commit selected and its detail patch loaded.
    fn detail_scene() -> (State, app_core::test_support::TempDir, HomeGuard) {
        let home = app_core::test_support::TempDir::new("commit-detail-qa");
        let guard = HomeGuard::set(home.path());
        let mut s = State::new(crate::test_handle());
        s.view = View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = PathBuf::from("/tmp/goko-qa-checkout");
        let key = checkout_key("p1", &co);
        s.editor.checkout_key = Some(key.clone());
        s.editor.checkout = Some(co);
        s.editor.section = Section::History;

        let mut merge = commit(MERGE_ID, &[PARENT_A, PARENT_B], "Merge feature into main");
        merge.head = true;
        merge.refs = vec!["refs/heads/main".into()];
        s.editor.history.commits = vec![
            merge,
            commit(PARENT_A, &[&oid('c')], "parent one work"),
            commit(PARENT_B, &[&oid('c')], "parent two work"),
            commit(&oid('c'), &[], "base commit"),
        ];
        s.editor.history.selected = Some(MERGE_ID.into());
        s.editor.history.detail = Some(Patch {
            label: format!("commit:{MERGE_ID}"),
            raw: DIFF_RAW.into(),
            truncated: false,
            binary: false,
            files: vec![StatusEntry {
                path: "src/a.rs".into(),
                orig_path: None,
                status: "modified",
                added: Some(1),
                removed: Some(0),
            }],
            detail: Some(GitViewCommitDetail {
                author_email: "author@example.com".into(),
                committer: "A U Thor".into(),
                committer_email: "author@example.com".into(),
                body: (1..=10)
                    .map(|i| format!("Body line {i} of the merge commit message."))
                    .collect::<Vec<_>>()
                    .join("\n"),
            }),
            checkout: key,
        });
        s.editor
            .history
            .diff
            .set_files(rune_kit::diff::parse(DIFF_RAW));
        (s, home, guard)
    }

    /// True for elements inside the commit-metadata scroll viewport. The
    /// path keys on `detail_pane`'s child order — metadata is child 0, the
    /// shared patch pane child 1 — which also keeps patch texts (the patch
    /// toolbar repeats the commit hash) out of metadata lookups.
    fn in_meta(e: &rune::runtime::ElementInfo) -> bool {
        e.path.contains("commit-detail-meta")
    }

    /// Finds a live metadata text containing `needle`.
    fn find_meta_text(rt: &Runtime<State>, needle: &str) -> Option<rune::runtime::ElementInfo> {
        rt.elements().into_iter().find(|e| {
            !e.exiting && in_meta(e) && e.text.as_deref().is_some_and(|t| t.contains(needle))
        })
    }

    /// Any metadata element containing `needle` currently has visible
    /// height — for rows that share a needle with a sibling (author vs
    /// committer line).
    fn any_meta_text_visible(rt: &Runtime<State>, needle: &str) -> bool {
        rt.elements().into_iter().any(|e| {
            !e.exiting
                && in_meta(&e)
                && e.text.as_deref().is_some_and(|t| t.contains(needle))
                && e.visible[3] > 4.0
        })
    }

    /// Finds a live element whose label starts with `prefix`.
    fn find_label(rt: &Runtime<State>, prefix: &str) -> Option<rune::runtime::ElementInfo> {
        rt.elements()
            .into_iter()
            .filter(|e| !e.exiting && e.label.as_deref().is_some_and(|l| l.starts_with(prefix)))
            .max_by(|a, b| a.rect[1].total_cmp(&b.rect[1]))
    }

    /// A metadata element that is currently on screen — used as the wheel
    /// anchor so the event lands inside the header's scroll viewport. The
    /// list spans the whole body so an anchor exists at every scroll
    /// offset; hash/author needles are avoided because the patch toolbar
    /// and table repeat them outside the metadata viewport.
    fn visible_detail_text(rt: &Runtime<State>) -> Option<rune::runtime::ElementInfo> {
        [
            "Merge feature into main",
            "A U Thor",
            "Parents",
            "Body line",
            "Changes compared",
        ]
        .iter()
        .filter_map(|n| find_meta_text(rt, n))
        .chain(
            ["Show more", "Show less"]
                .iter()
                .filter_map(|n| find_label(rt, n)),
        )
        .find(|e| e.visible[3] > 4.0)
    }

    /// Wheel over the metadata viewport; positive `dy` scrolls content up
    /// (reveals rows below the fold).
    fn wheel_meta(rt: &mut Runtime<State>, dy: f32) {
        let anchor = visible_detail_text(rt).expect("a visible metadata anchor");
        let v = anchor.visible;
        let (x, y) = (v[0] + v[2] / 2.0, v[1] + v[3] / 2.0);
        rt.wheel_event(rune::WheelEvent::new(0.0, dy).at(x, y));
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
    }

    /// Click the visible center of an element (rect ∩ visible clip).
    fn click_el(rt: &mut Runtime<State>, e: &rune::runtime::ElementInfo) {
        let (r, v) = (e.rect, e.visible);
        let x0 = r[0].max(v[0]);
        let y0 = r[1].max(v[1]);
        let x1 = (r[0] + r[2]).min(v[0] + v[2]);
        let y1 = (r[1] + r[3]).min(v[1] + v[3]);
        let (x, y) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
    }

    /// CUA repro: at both QA viewports the commit metadata must occupy a
    /// positive-height viewport — subject/hash/author visible, parents and
    /// the body reachable through the header's own scroll — while the patch
    /// pane keeps its floor below.
    #[test]
    fn commit_metadata_visible_and_scrollable_at_both_viewports() {
        for (w, h) in [(1512.0, 814.0), (760.0, 480.0)] {
            let (s, _home, _guard) = detail_scene();
            let mut rt = Runtime::new(s, crate::view, (w, h), Env::new(true, 2.0));
            rt.frame(0.0);
            rt.frame(1.0 / 60.0);
            rt.frame(1.0 / 60.0);
            rt.frame(1.0 / 60.0);

            // Hash + subject are the metadata's head — always on screen.
            // Deeper rows (author, parents, body) may sit below the fold at
            // short windows; they must be reachable through the header's
            // own scroll, asserted below.
            for needle in ["01234567", "Merge feature into main"] {
                let e = find_meta_text(&rt, needle)
                    .unwrap_or_else(|| panic!("{needle:?} missing at {w}x{h}"));
                assert!(
                    e.visible[3] > 4.0,
                    "{needle:?} not visible at {w}x{h}: {:?}",
                    e.visible
                );
            }
            // Patch pane kept its floor: the unified/split control mounted
            // with positive height below the metadata.
            let patch_ctl = find_label(&rt, "Unified")
                .unwrap_or_else(|| panic!("patch toolbar missing at {w}x{h}"));
            assert!(
                patch_ctl.visible[3] > 4.0,
                "patch starved at {w}x{h}: {:?}",
                patch_ctl.visible
            );

            // The 10-line body pushes author/parents/changed-files below the
            // fold at short windows: wheel the header's own scroll viewport
            // until both the author line and a parent button have been on
            // screen — the pre-fix collapse left a 0-pt viewport so no wheel
            // delta could ever reveal them. Seen-ever (not simultaneously)
            // because a fast wheel can carry one row out as the next enters.
            let mut author_seen = false;
            let mut parent_seen = false;
            for _ in 0..14 {
                author_seen |= any_meta_text_visible(&rt, "A U Thor <author@example.com>");
                parent_seen |=
                    find_label(&rt, "Select parent aaaaaaaa").is_some_and(|e| e.visible[3] > 4.0);
                if author_seen && parent_seen {
                    break;
                }
                wheel_meta(&mut rt, 90.0);
            }
            assert!(
                author_seen && parent_seen,
                "author/parents unreachable by scroll at {w}x{h}: author={author_seen} parent={parent_seen}"
            );
            // Patch still mounted after scrolling the header.
            assert!(
                find_label(&rt, "Unified").is_some(),
                "patch lost at {w}x{h} after header scroll"
            );
        }
    }

    /// Parent navigation is real: pressing a loaded parent's button selects
    /// it and drops the stale detail, like `selectCommit` on the desktop.
    #[test]
    fn parent_button_selects_loaded_parent() {
        let (s, _home, _guard) = detail_scene();
        let mut rt = Runtime::new(s, crate::view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        // The Parents row sits below the fold — scroll the header until the
        // button is actually clickable, then press it.
        for _ in 0..10 {
            if find_label(&rt, "Select parent aaaaaaaa").is_some_and(|e| e.visible[3] > 8.0) {
                break;
            }
            wheel_meta(&mut rt, 240.0);
        }
        let btn = find_label(&rt, "Select parent aaaaaaaa").expect("parent button mounted");
        assert!(btn.visible[3] > 4.0, "parent button still clipped");
        // AX contract: the control announces as a focusable button, not bare
        // text — keyboard users Tab to it and press Enter/Space.
        assert_eq!(btn.role, "button", "parent control must expose button role");
        assert!(btn.focusable, "parent control must be keyboard-focusable");
        click_el(&mut rt, &btn);

        assert_eq!(
            rt.state.editor.history.selected.as_deref(),
            Some(PARENT_A),
            "parent press must select the parent commit"
        );
        assert!(
            rt.state.editor.history.detail.is_none(),
            "stale detail cleared for the new selection"
        );
    }

    /// "Show more" expands the clamped body through the real press path.
    #[test]
    fn show_more_expands_commit_body() {
        let (s, _home, _guard) = detail_scene();
        let mut rt = Runtime::new(s, crate::view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        // Clamped: the single body text element holds only the first 6 of
        // the 10 message lines, so "Body line 10" cannot appear yet.
        assert!(
            find_meta_text(&rt, "Body line 6 of the merge commit message.").is_some(),
            "clamped body head missing"
        );
        assert!(
            find_meta_text(&rt, "Body line 10 of the merge commit message.").is_none(),
            "line 10 must stay clamped before Show more"
        );

        // Scroll until the toggle is clickable, then press it.
        for _ in 0..10 {
            if find_label(&rt, "Show more").is_some_and(|e| e.visible[3] > 8.0) {
                break;
            }
            wheel_meta(&mut rt, 240.0);
        }
        let btn = find_label(&rt, "Show more").expect("Show more mounted");
        click_el(&mut rt, &btn);

        assert!(
            find_label(&rt, "Show less").is_some(),
            "Show more must flip to Show less"
        );
        assert!(
            find_meta_text(&rt, "Body line 10 of the merge commit message.").is_some(),
            "expanded body must show line 10"
        );
    }

    /// CUA geometry regression at both QA sizes: the muted uppercase table
    /// headers stay mounted, the detail pane (history root's 4th child) ends
    /// inside the window so the patch can't render below it, and each graph
    /// canvas spans the full column width — the kit's 8pt cell padding is
    /// cancelled by a negative margin instead of clipping lanes.
    #[test]
    fn detail_pane_and_graph_fit_window_at_both_viewports() {
        for (w, h) in [(1512.0, 814.0), (760.0, 480.0)] {
            let (s, _home, _guard) = detail_scene();
            let mut rt = Runtime::new(s, crate::view, (w, h), Env::new(true, 2.0));
            rt.frame(0.0);
            rt.frame(1.0 / 60.0);
            rt.frame(1.0 / 60.0);

            for title in ["Graph", "DESCRIPTION", "DATE", "AUTHOR", "COMMIT"] {
                let hdr = rt
                    .elements()
                    .into_iter()
                    .find(|e| !e.exiting && e.text.as_deref() == Some(title) && e.visible[3] > 4.0);
                assert!(hdr.is_some(), "header {title:?} missing at {w}x{h}");
            }

            // The detail pane is the lowest common ancestor of the metadata
            // (short hash) and the patch toggle ("Unified"): `detail_pane`
            // is `col(meta, patch)`, so their paths share exactly the pane's
            // prefix. Its bottom edge must not cross its parent's bottom
            // (the clamp contract) nor the window bottom.
            let els = rt.elements();
            let hash_path = els
                .iter()
                .find(|e| !e.exiting && e.text.as_deref() == Some("01234567"))
                .map(|e| e.path.clone())
                .expect("short hash text");
            let patch_path = els
                .iter()
                .find(|e| !e.exiting && e.text.as_deref() == Some("Unified"))
                .map(|e| e.path.clone())
                .expect("Unified patch toggle");
            let hp: Vec<&str> = hash_path.split('/').collect();
            let pp: Vec<&str> = patch_path.split('/').collect();
            let mut n = 0;
            while n < hp.len() && n < pp.len() && hp[n] == pp[n] {
                n += 1;
            }
            assert!(n > 0, "hash and patch share no ancestor");
            let pane_path = hp[..n].join("/");
            let pane_rect = els
                .iter()
                .find(|e| e.path == pane_path)
                .map(|e| e.rect)
                .unwrap_or_else(|| panic!("detail pane {pane_path:?} not found"));
            let parent_path = hp[..n - 1].join("/");
            let parent_rect = els
                .iter()
                .find(|e| e.path == parent_path)
                .map(|e| e.rect)
                .unwrap_or_else(|| panic!("pane parent {parent_path:?} not found"));
            let (pane_bottom, root_bottom) =
                (pane_rect[1] + pane_rect[3], parent_rect[1] + parent_rect[3]);
            assert!(
                pane_bottom <= root_bottom + 0.5,
                "detail pane bottom {pane_bottom:.1} escapes its parent \
                 bottom {root_bottom:.1} at {w}x{h}"
            );
            assert!(
                pane_bottom <= h + 0.5,
                "detail pane bottom {pane_bottom:.1} exceeds window {h} \
                 (parent bottom {root_bottom:.1}) at {w}x{h}"
            );

            // Graph canvases report role "image"; the scene's single-lane
            // commits give graph_w = 48 (natural width clamped to the 48pt
            // layout minimum). They must be ~48pt wide — the ~32pt padded
            // content box would mean the escape margin failed — and their
            // left edge must sit on the column edge, 8pt left of the "Graph"
            // header text (which keeps the cell's own padding).
            let graph_hdr_x = rt
                .elements()
                .into_iter()
                .find(|e| !e.exiting && e.text.as_deref() == Some("Graph"))
                .map(|e| e.rect[0])
                .expect("Graph header text");
            let canvases: Vec<_> = rt
                .elements()
                .into_iter()
                .filter(|e| {
                    !e.exiting
                        && e.role == "image"
                        && (e.rect[3] - 28.0).abs() < 0.5
                        && e.rect[2] > 30.0
                        && e.rect[2] < 80.0
                })
                .collect();
            assert!(!canvases.is_empty(), "no graph canvas mounted at {w}x{h}");
            for c in &canvases {
                assert!(
                    c.rect[2] >= 44.0,
                    "graph canvas clipped by cell padding at {w}x{h}: {:?}",
                    c.rect
                );
                assert!(
                    (c.rect[0] - (graph_hdr_x - 8.0)).abs() < 1.5,
                    "graph canvas x {:.1} off column edge {:.1} at {w}x{h}",
                    c.rect[0],
                    graph_hdr_x - 8.0
                );
            }
        }
    }

    /// Desktop `copyNote`: pressing the hash copy button shows a temporary
    /// muted "Copied" note for `COPY_FEEDBACK_MS`, then it disappears —
    /// `history::poll` owns the expiry since Rune has no `setTimeout`.
    #[test]
    fn copy_button_shows_temporary_copied_note() {
        let (s, _home, _guard) = detail_scene();
        let mut rt = Runtime::new(s, crate::view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);

        let btn = rt
            .elements()
            .into_iter()
            .find(|e| !e.exiting && e.label.as_deref() == Some("Copy commit hash"))
            .expect("copy button mounted");
        let r = btn.rect;
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);

        assert_eq!(
            rt.state
                .editor
                .history
                .copied_hash
                .as_ref()
                .map(|(id, _)| id.as_str()),
            Some(MERGE_ID),
            "copy marks the note for this commit"
        );
        assert!(
            rt.elements()
                .iter()
                .any(|e| e.text.as_deref() == Some("Copied")),
            "'Copied' note renders beside the button"
        );

        // Age the marker past the desktop 1500ms window: the note hides on
        // the next frame and poll clears the state.
        rt.state.editor.history.copied_hash = Some((
            MERGE_ID.into(),
            std::time::Instant::now() - std::time::Duration::from_secs(2),
        ));
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        assert!(
            !rt.elements()
                .iter()
                .any(|e| !e.exiting && e.text.as_deref() == Some("Copied")),
            "expired note no longer renders"
        );
        let mut tx = Tx::default();
        assert!(crate::editor::history::poll(&mut rt.state, &mut tx));
        assert!(rt.state.editor.history.copied_hash.is_none());
    }

    #[test]
    fn body_expand_state_is_per_commit() {
        set_body_expanded("aaa", true);
        assert!(body_expanded("aaa"));
        assert!(!body_expanded("bbb"));
        set_body_expanded("aaa", false);
        assert!(!body_expanded("aaa"));
    }

    // -- changed-file list View menu -------------------------------------------

    /// Right-click on the changed-file list opens the detail's `View` menu
    /// (the menu itself mounts at the History section level); picking
    /// `Tree` writes the shared `editor.list_view` and closes it.
    #[test]
    fn secondary_press_on_file_list_opens_view_menu() {
        let (mut s, _home, _guard) = detail_scene();
        // An empty body keeps the file list high enough for the menu —
        // anchored at the press — to fit the window.
        s.editor
            .history
            .detail
            .as_mut()
            .unwrap()
            .detail
            .as_mut()
            .unwrap()
            .body = String::new();
        let mut rt = Runtime::new(s, crate::view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);

        let row = rt
            .elements()
            .into_iter()
            .find(|e| in_meta(e) && e.label.as_deref() == Some("View diff for src/a.rs"))
            .expect("changed-file row mounted");
        let (x, y) = (
            row.rect[0] + row.rect[2] / 2.0,
            row.rect[1] + row.rect[3] / 2.0,
        );
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(1.0 / 60.0);

        let m = rt
            .state
            .editor
            .history
            .file_menu
            .as_ref()
            .expect("secondary press opens the file menu");
        assert_eq!(*m, ListMenu::Point { x, y });

        rt.frame(1.0 / 60.0);
        let item = |rt: &Runtime<State>, label: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.role == "menuitemcheckbox" && e.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("menu item {label:?} mounted"))
        };
        assert!(item(&rt, "Tree").item_selected, "Tree is the active view");
        assert!(!item(&rt, "List").item_selected);
        // No Group By in History — only the two view choices.
        assert!(
            !rt.elements()
                .iter()
                .any(|e| e.label.as_deref() == Some("Tracked & Untracked")),
            "the detail menu has no grouping items"
        );

        let list = item(&rt, "List");
        let (x, y) = (
            list.rect[0] + list.rect[2] / 2.0,
            list.rect[1] + list.rect[3] / 2.0,
        );
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.editor.list_view, ListView::List);
        assert!(
            rt.state.editor.history.file_menu.is_none(),
            "picking closes the menu"
        );
    }

    /// `Tree` nests detail files under directory rows that toggle
    /// `history.collapsed_dirs`; the detail's file list is its own host so
    /// a right-click there never opens the commit-row menu.
    #[test]
    fn tree_view_nests_detail_files() {
        let (mut s, _home, _guard) = detail_scene();
        s.editor.history.detail.as_mut().unwrap().files = vec![
            StatusEntry {
                path: "src/lib/a.rs".into(),
                orig_path: None,
                status: "modified",
                added: Some(2),
                removed: Some(0),
            },
            StatusEntry {
                path: "src/b.rs".into(),
                orig_path: None,
                status: "added",
                added: Some(5),
                removed: Some(0),
            },
            StatusEntry {
                path: "readme.md".into(),
                orig_path: None,
                status: "modified",
                added: Some(1),
                removed: Some(1),
            },
        ];
        // No assignment: `Tree` is the session default and the History
        // detail reads the same shared choice the Changes list writes.
        assert_eq!(s.editor.list_view, ListView::Tree);
        let mut rt = Runtime::new(s, crate::view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);

        // Each dir level emits a row; the files nest under `src`/`src/lib`.
        for label in ["src directory", "src/lib directory"] {
            assert!(
                rt.elements()
                    .iter()
                    .any(|e| in_meta(e) && e.label.as_deref() == Some(label)),
                "{label} mounted in Tree view"
            );
        }
        assert!(
            rt.elements()
                .iter()
                .any(|e| in_meta(e) && e.label.as_deref() == Some("View diff for src/lib/a.rs")),
            "nested file row mounted"
        );

        // Collapsing `src` hides both levels of children.
        let dir = rt
            .elements()
            .into_iter()
            .find(|e| in_meta(e) && e.label.as_deref() == Some("src directory"))
            .unwrap();
        let (x, y) = (dir.rect[0] + 40.0, dir.rect[1] + dir.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);

        assert!(rt.state.editor.history.collapsed_dirs.contains("src"));
        rt.frame(1.0 / 60.0);
        assert!(
            !rt.elements()
                .iter()
                .any(|e| in_meta(e) && e.label.as_deref() == Some("View diff for src/lib/a.rs")),
            "collapsed dir hides its files"
        );
    }

    /// A detail with no changed files still mounts the list (header shows
    /// "0 changed files"), so its View menu has somewhere to open.
    #[test]
    fn empty_file_list_still_takes_the_menu() {
        let (mut s, _home, _guard) = detail_scene();
        s.editor.history.detail.as_mut().unwrap().files = vec![];
        let mut rt = Runtime::new(s, crate::view, (1512.0, 814.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);

        let head = rt
            .elements()
            .into_iter()
            .find(|e| in_meta(e) && e.text.as_deref() == Some("0 changed files"))
            .expect("empty list header mounted");
        let (x, y) = (
            head.rect[0] + head.rect[2] / 2.0,
            head.rect[1] + head.rect[3] / 2.0,
        );
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(1.0 / 60.0);
        assert!(
            rt.state.editor.history.file_menu.is_some(),
            "menu opens with no changed files"
        );
    }
}
