//! Files section: the checkout's file tree with filename filtering, the
//! single open document with its draft guard, native text surface with a
//! line-number gutter, markdown preview, content search, and save/discard.
//!
//! The document surface uses Rune's `TextEdit` + `text_input` (IME,
//! clipboard, undo/redo, selection all native). Line numbers are a gutter
//! column fed by the same text; the code owner's `code.rs` can later swap
//! this surface behind the same `OpenDoc` state.

use rune::{El, KeyEvent, Tx, col, row};
use rune_kit::{
    FileNode, Kit, MenuItem, ScrollBar, Tree, TreeItem, TreeKey, TreeModel, TreeStyle, context_menu,
};

use crate::State;
use crate::editor::backend::Req;
use crate::editor::draft::{self, DocKind};
use crate::editor::history_layout::PaneKind;
use crate::editor::keyboard;
use crate::editor::navigation::{self, PendingNav};
use crate::editor::pane_layout;
use crate::editor::state::{FileMark, FileMenu};
use crate::theme::{self, Ink, T11M_LH, T12, T12M, T12TN};

/// Files section root: toolbar (filter + search toggle + refresh), then
/// [aside: tree or search results | editor] split — the aside swaps content
/// like the desktop `SplitPane`, the editor flexes to the remaining width.
pub fn files_view(s: &State, ink: &Ink, k: &Kit) -> El {
    let mut root = col().grow(1.0).min_w(0.0).h_full().min_h(0.0);
    if let Some(dialog) = trash_dialog(s, ink) {
        root = root.child(dialog);
    }
    root = root.child(toolbar(s, ink, k));
    root = root.child(
        row()
            .grow(1.0)
            .min_h(0.0)
            .w_full()
            .on_layout_tx(pane_layout::watch_row_width())
            // Desktop SplitPane parity: the aside shows EITHER the file
            // tree or the content-search results — never both — so the
            // editor always owns whatever width remains (no fixed code
            // column + results column + tree overflowing narrow windows).
            .child(
                // Persisted width + grip: the aside gives way before the
                // editor does on narrow windows (flex shrink to PANE_MIN),
                // but never collapses unusable; the detail column keeps
                // SIBLING_MIN so the code surface stays usable.
                col()
                    .w(pane_layout::current(s, PaneKind::FilesAside))
                    .h_full()
                    .shrink()
                    .min_w(pane_layout::PANE_MIN)
                    .border_sides([0.0, 1.0, 0.0, 0.0], ink.border)
                    .child(
                        if s.editor.files.search.open {
                            search_results(s, ink, k)
                        } else {
                            tree_pane(s, ink, k)
                        }
                        .w_full()
                        .h_full(),
                    )
                    .child(pane_layout::grip(
                        s,
                        ink,
                        PaneKind::FilesAside,
                        "Resize file list and editor",
                    )),
            )
            .child(
                col()
                    .grow(1.0)
                    .min_w(pane_layout::SIBLING_MIN)
                    .h_full()
                    .min_h(0.0)
                    .child(doc_bar(s, ink))
                    .child(editor_pane(s, ink, k).grow(1.0).min_w(0.0).h_full()),
            ),
    );
    root
}

fn toolbar(s: &State, ink: &Ink, k: &Kit) -> El {
    let mut bar = row()
        .w_full()
        .h(34.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border);
    bar = bar.child(
        rune_kit::TextInput::new(&s.editor.files.filter)
            .placeholder("Filter files")
            .width(Some(220.0))
            // `on_edit` hands the runtime mutable access to the TextEdit —
            // without it the field renders but can't be typed into.
            .on_edit(|st: &mut State| -> &mut rune::edit::TextEdit { &mut st.editor.files.filter })
            .on_change(|st: &mut State, _tx: &mut Tx| {
                st.editor.rebuild_file_model();
            })
            // `.width()` is a fixed size, not a cap — `shrink` lets the
            // field give way (truncating inside) before the toolbar row
            // pushes Refresh/controls off a narrow window.
            .build(k)
            .shrink()
            .min_w(80.0),
    );
    bar = bar.child(
        theme::btn_sm("Search", None, s.editor.files.search.open, ink).on_press(
            |st: &mut State| {
                st.editor.files.search.open = !st.editor.files.search.open;
            },
        ),
    );
    if s.editor.files.search.open {
        bar = bar.child(
            rune_kit::TextInput::new(&s.editor.files.search_edit)
                .placeholder("Search contents")
                .width(Some(240.0))
                .on_edit(|st: &mut State| -> &mut rune::edit::TextEdit {
                    &mut st.editor.files.search_edit
                })
                .on_change(|st: &mut State, tx: &mut Tx| {
                    crate::editor::search::on_query_change(st, tx);
                })
                .build(k)
                .shrink()
                .min_w(90.0),
        );
    }
    bar = bar.child(rune::spacer());
    bar = bar
        .child(toolbar_icon(&theme::FILE_PLUS, "New File", ink, |st| {
            let dir = selected_dir(st);
            ask_for_new(st, &dir, false);
        }))
        .child(toolbar_icon(&theme::FOLDER_PLUS, "New Folder", ink, |st| {
            let dir = selected_dir(st);
            ask_for_new(st, &dir, true);
        }))
        .child(toolbar_icon(&theme::REFRESH, "Refresh", ink, refresh_files))
        .child(toolbar_icon(
            &theme::COLLAPSE_ALL,
            "Collapse All",
            ink,
            |st| {
                tree_get_mut(st).collapse_all();
            },
        ));
    col().w_full().child(bar)
}

/// One icon-only toolbar button, labelled and with a tooltip.
fn toolbar_icon(
    icon: &'static rune::Icon,
    label: &'static str,
    ink: &Ink,
    on_press: fn(&mut State),
) -> El {
    rune::el()
        .key(format!("files-tool:{label}"))
        .size(28.0, 28.0)
        .radius(8.0)
        .center()
        .role("button")
        .label(label)
        .child(theme::ic(icon, 16.0, ink.text))
        .hover(ink.text.alpha(0.08))
        .press(ink.text.alpha(0.12))
        .press_scale(0.95)
        .motion(theme::STATE_SPRING)
        .help(label)
        .focusable(true)
        .focus_ring(2.0, 1.0, ink.accent)
        .on_press(on_press)
}

/// The folder a new entry goes in: the selected folder, the folder of the
/// selected file, or the checkout root (`""`).
fn selected_dir(s: &State) -> String {
    s.editor
        .files
        .model
        .cursor()
        .map(|id| entry_dir(tree_get(s), id))
        .unwrap_or_default()
}

/// `id` itself when it is a folder, else the folder it sits in.
fn entry_dir(model: &TreeModel<FileNode<FileMark>>, id: &str) -> String {
    let folder = model.get(id).is_some_and(|n| {
        n.dir || matches!(n.data, Some(FileMark::IgnoredDir | FileMark::EmptyDir))
    });
    match (folder, id.rsplit_once('/')) {
        (true, _) => id.to_string(),
        (false, Some((dir, _))) => dir.to_string(),
        (false, None) => String::new(),
    }
}

/// Open the prompt for a new file or folder in `dir`.
fn ask_for_new(s: &mut State, dir: &str, folder: bool) {
    s.ask_for_input(crate::sidebar::SidebarInput::NewEntry {
        dir: dir.to_string(),
        folder,
    });
}

/// Send the create. The new entry opens in the tree once the list re-reads.
pub fn submit_new(s: &mut State, dir: &str, folder: bool, name: &str) {
    s.close_sidebar_input();
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    let (Some(checkout), Some(key)) = (s.editor.checkout.clone(), s.editor.checkout_key.clone())
    else {
        return;
    };
    let req = Req::CreateEntry {
        dir: dir.to_string(),
        name: name.to_string(),
        folder,
    };
    if !s.editor.enqueue_write(checkout, key, req) {
        files_queue_full(s);
    }
}

/// Re-issue the file list for the current checkout (manual refresh).
pub fn refresh_files(s: &mut State) {
    let Some(checkout) = s.editor.checkout.clone() else {
        return;
    };
    let Some(key) = s.editor.checkout_key.clone() else {
        return;
    };
    s.editor.files.loading = true;
    s.editor.files.error = None;
    if !s.editor.enqueue_read(checkout, key, Req::FileList) {
        // The queue refused: no reply will ever clear the flag — settle it
        // here and say why instead of pinning "Loading files…" forever.
        s.editor.files.loading = false;
        files_queue_full(s);
    }
}

/// Queue-refusal surfacing, mirroring `changes::queue_full`: the section
/// error plus the global notice. The draft/doc/identity are untouched.
fn files_queue_full(s: &mut State) {
    let text = "Backend queue is full — try again".to_string();
    s.editor.files.error = Some(text.clone());
    s.editor.notice = Some(crate::editor::state::Notice { text, stale: false });
}

fn tree_pane(s: &State, ink: &Ink, k: &Kit) -> El {
    if s.editor.files.paths.is_empty() {
        let (label, tone) = if s.editor.files.loading {
            ("Loading files…".to_string(), ink.muted)
        } else if let Some(err) = &s.editor.files.error {
            (err.clone(), ink.danger)
        } else {
            ("No files".to_string(), ink.muted)
        };
        return col()
            .grow(1.0)
            .items_center()
            .justify_center()
            .child(theme::txt(&label, T12, tone));
    }
    let dim = ink.muted;
    Tree::new(tree_get, tree_get_mut, FileNode::label)
        .row(move |node, row| {
            let item = TreeItem::file(node, row);
            match node.data {
                Some(FileMark::Tracked) | None => item,
                // Ignored by Git: dimmed like VS Code; a collapsed ignored
                // directory keeps its folder icon.
                Some(FileMark::Ignored) => item.label_color(dim).icon_color(dim),
                Some(FileMark::IgnoredDir) => item
                    .icon(&rune_kit::glyphs::FOLDER)
                    .label_color(dim)
                    .icon_color(dim),
                // A folder made here that holds nothing yet.
                Some(FileMark::EmptyDir) => item.icon(&rune_kit::glyphs::FOLDER),
            }
        })
        // Press/double-press still route through these — `.on_key` below
        // replaces only the kit's key dispatch, not the row click handlers.
        .on_select(|st: &mut State, id: &str, tx: &mut Tx| {
            // Desktop parity: activating a file row opens it — a plain click
            // moves the cursor (this callback) and Enter/double-click routes
            // through `on_open`. Both land in the draft guard via `open_doc`.
            tree_activate(st, id, tx);
        })
        .on_open(|st: &mut State, id: &str, tx: &mut Tx| {
            // Re-activating the already-selected file (Enter/Space or a
            // double-click) still opens — same-file requests only reveal.
            tree_activate(st, id, tx);
        })
        .style(TreeStyle::from_kit(k))
        .build(s, k)
        .on_key(tree_key)
        // A row has no handler of its own for this press, so it lands here:
        // the element under the pointer names the row.
        .on_secondary_press(|_: &mut State, p: rune::PointerPress, tx: &mut Tx| {
            tx.element_at(p.x, p.y, move |st: &mut State, picked, _tx: &mut Tx| {
                let Some(id) = picked.and_then(|e| row_id(tree_get(st), e.path())) else {
                    return;
                };
                tree_get_mut(st).select(&id);
                st.editor.files.menu = Some(FileMenu { id, x: p.x, y: p.y });
            });
        })
}

/// The tree row an element path belongs to: `…/rows/<row id>/<part>`. A row id
/// holds slashes of its own, so it is the longest prefix that names a node.
fn row_id(model: &TreeModel<FileNode<FileMark>>, path: &str) -> Option<String> {
    let rest = path.split_once("/rows/")?.1;
    std::iter::once(rest.len())
        .chain(rest.match_indices('/').map(|(i, _)| i).rev())
        .map(|end| &rest[..end])
        .find(|id| model.contains(id))
        .map(str::to_string)
}

/// The row context menu: copy the path, show it in Finder, rename it.
pub fn menu_layer(s: &State, layer: &mut Vec<El>) -> Option<El> {
    if s.view != crate::View::Editor {
        return None;
    }
    let menu = s.editor.files.menu.as_ref()?;
    let checkout = s.editor.checkout.as_ref()?;
    if !tree_get(s).contains(&menu.id) {
        return None;
    }
    let absolute = checkout.join(&menu.id);
    let (new_dir, new_dir2) = (
        entry_dir(tree_get(s), &menu.id),
        entry_dir(tree_get(s), &menu.id),
    );
    let (full, relative, reveal_path, id, trash_id) = (
        absolute.to_string_lossy().into_owned(),
        menu.id.clone(),
        absolute,
        menu.id.clone(),
        menu.id.clone(),
    );
    let at = rune::PointerPress {
        x: menu.x,
        y: menu.y,
    };
    Some(
        context_menu("files-row", rune::el(), Some(at))
            .item(
                MenuItem::new("Copy Path").on_select_tx(move |st: &mut State, tx: &mut Tx| {
                    st.editor.files.menu = None;
                    tx.copy(full.clone());
                    st.notice = Some("Copied to clipboard".into());
                }),
            )
            .item(MenuItem::new("Copy Relative Path").on_select_tx(
                move |st: &mut State, tx: &mut Tx| {
                    st.editor.files.menu = None;
                    tx.copy(relative.clone());
                    st.notice = Some("Copied to clipboard".into());
                },
            ))
            .item(
                MenuItem::new("Reveal in Finder").on_select(move |st: &mut State| {
                    st.editor.files.menu = None;
                    reveal(&reveal_path);
                }),
            )
            .separator()
            .item(MenuItem::new("New File…").on_select(move |st: &mut State| {
                st.editor.files.menu = None;
                ask_for_new(st, &new_dir, false);
            }))
            .item(
                MenuItem::new("New Folder…").on_select(move |st: &mut State| {
                    st.editor.files.menu = None;
                    ask_for_new(st, &new_dir2, true);
                }),
            )
            .separator()
            .item(MenuItem::new("Rename…").on_select(move |st: &mut State| {
                st.editor.files.menu = None;
                ask_for_rename(st, &id);
            }))
            .item(
                MenuItem::new("Move to Trash…")
                    .danger(true)
                    .on_select(move |st: &mut State| {
                        st.editor.files.menu = None;
                        st.editor.files.delete_prompt = Some(trash_id.clone());
                    }),
            )
            .on_close(|st: &mut State| st.editor.files.menu = None)
            .build(&s.editor.kit, layer),
    )
}

/// Show `path` selected in the system file manager.
// ponytail: macOS `open -R`; elsewhere the folder opens, with no selection.
fn reveal(path: &std::path::Path) {
    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    if let Ok(mut child) = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Open the rename prompt with the entry's current name filled in.
fn ask_for_rename(s: &mut State, id: &str) {
    s.ask_for_input(crate::sidebar::SidebarInput::RenameFile { id: id.to_string() });
    let name = id.rsplit('/').next().unwrap_or(id);
    s.sidebar_text = rune::edit::TextEdit::new(name);
    s.sidebar_text.focused = true;
}

/// The Move-to-Trash confirmation: names the entry, and says it can be put
/// back from the Trash.
fn trash_dialog(s: &State, ink: &Ink) -> Option<El> {
    let id = s.editor.files.delete_prompt.clone()?;
    let name = id.rsplit('/').next().unwrap_or(&id).to_string();
    Some(
        col()
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(rune::Color::BLACK.alpha(0.45))
            .items_center()
            .justify_center()
            .child(
                col()
                    .w(400.0)
                    .gap(12.0)
                    .padding(16.0)
                    .radius(12.0)
                    .fill(ink.panel)
                    .border(1.0, ink.border)
                    .child(theme::txt("Move to Trash?", T12M, ink.text))
                    .child(theme::txt(
                        &format!("Move {name} to the Trash? You can put it back from there."),
                        T12,
                        ink.muted,
                    ))
                    .child(
                        row()
                            .gap(8.0)
                            .justify_end()
                            .w_full()
                            .child(
                                theme::btn_sm("Cancel", None, true, ink).on_press(
                                    |st: &mut State| st.editor.files.delete_prompt = None,
                                ),
                            )
                            .child(theme::btn_sm("Move to Trash", None, false, ink).on_press(
                                move |st: &mut State| {
                                    st.editor.files.delete_prompt = None;
                                    submit_trash(st, &id);
                                },
                            )),
                    ),
            ),
    )
}

/// Send the delete. An open draft under the entry blocks it, so nothing
/// unsaved goes away with the file.
fn submit_trash(s: &mut State, id: &str) {
    let (Some(checkout), Some(key)) = (s.editor.checkout.clone(), s.editor.checkout_key.clone())
    else {
        return;
    };
    let blocked = s.editor.doc.as_ref().is_some_and(|doc| {
        doc.dirty() && (doc.path == id || doc.path.starts_with(&format!("{id}/")))
    });
    if blocked {
        s.notice = Some(format!(
            "save or discard the changes in {id} before deleting it"
        ));
        return;
    }
    let req = Req::TrashPath {
        path: id.to_string(),
    };
    if !s.editor.enqueue_write(checkout, key, req) {
        files_queue_full(s);
    }
}

/// Send the rename. A name that did not change closes the prompt; an open
/// draft under the entry blocks it, so the rename never strands unsaved text.
pub fn submit_rename(s: &mut State, id: &str, name: &str) {
    s.close_sidebar_input();
    let name = name.trim();
    if name.is_empty() || id.rsplit('/').next() == Some(name) {
        return;
    }
    let (Some(checkout), Some(key)) = (s.editor.checkout.clone(), s.editor.checkout_key.clone())
    else {
        return;
    };
    let blocked = s.editor.doc.as_ref().is_some_and(|doc| {
        doc.dirty() && (doc.path == id || doc.path.starts_with(&format!("{id}/")))
    });
    if blocked {
        s.notice = Some(format!(
            "save or discard the changes in {id} before renaming it"
        ));
        return;
    }
    let req = Req::RenamePath {
        path: id.to_string(),
        new_name: name.to_string(),
    };
    if !s.editor.enqueue_write(checkout, key, req) {
        files_queue_full(s);
    }
}

/// What a tree activation means: directories only toggle via the tree's own
/// key path; files open through the draft guard. Shared by cursor-move
/// (`on_select`) and Enter/Space (`on_open`) — both had identical bodies.
fn tree_activate(st: &mut State, id: &str, tx: &mut Tx) {
    if is_openable(st, id) {
        open_doc(st, id, None, tx);
    } else if st
        .editor
        .files
        .model
        .get(id)
        .is_some_and(|n| n.data == Some(FileMark::IgnoredDir))
    {
        open_ignored_dir(st, id);
    }
}

/// An ignored directory is a leaf until opened: ask for its entries, which
/// replace it in the tree. One whose entries were read stays as it is.
fn open_ignored_dir(s: &mut State, dir: &str) {
    if s.editor.files.ignored_open.contains_key(dir) {
        return;
    }
    let (Some(checkout), Some(key)) = (s.editor.checkout.clone(), s.editor.checkout_key.clone())
    else {
        return;
    };
    let req = Req::IgnoredChildren {
        dir: dir.to_string(),
    };
    if !s.editor.enqueue_write(checkout, key, req) {
        files_queue_full(s);
    }
}

/// A file row opens; a folder toggles, and an ignored directory (a leaf row)
/// has nothing to open.
fn is_openable(st: &State, id: &str) -> bool {
    st.editor.files.model.get(id).is_some_and(|n| {
        !n.dir && !matches!(n.data, Some(FileMark::IgnoredDir | FileMark::EmptyDir))
    })
}

/// Composite tree key handler. `.on_key` replaces the tree's built-in
/// dispatch, so global nav keys (`j`/`k`/`1`–`4`) preempt typeahead first —
/// then the same `handle_key` + select/open mapping rune-kit runs is
/// reproduced here (`rune-kit tree/view.rs`): cursor moves fire the old
/// `on_select` open, `Open` fires the old `on_open`.
fn tree_key(st: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if keyboard::preempt_tree_key(st, ev) && keyboard::on_key(st, ev, tx) {
        return true;
    }
    let m = tree_get_mut(st);
    let before = m.cursor().map(str::to_string);
    let out = m.handle_key(ev, FileNode::label, tx);
    let after = m.cursor().map(str::to_string);
    match out {
        TreeKey::Ignored => false,
        TreeKey::Handled => {
            if before != after
                && let Some(id) = after
            {
                tree_activate(st, &id, tx);
            }
            true
        }
        TreeKey::Open(id) => {
            tree_activate(st, &id, tx);
            true
        }
    }
}

fn tree_get(s: &State) -> &TreeModel<FileNode<FileMark>> {
    &s.editor.files.model
}
fn tree_get_mut(s: &mut State) -> &mut TreeModel<FileNode<FileMark>> {
    &mut s.editor.files.model
}

/// Open `path` in the editor pane. Same-file requests only queue a reveal —
/// no reload. Everything routes through the draft guard first.
pub fn open_doc(s: &mut State, path: &str, line: Option<u64>, tx: &mut Tx) {
    if crate::editor::media::opens_externally(path) {
        if let Some(checkout) = &s.editor.checkout {
            crate::panes::open_url(&checkout.join(path).to_string_lossy());
        }
        return;
    }
    navigation::navigate(
        s,
        PendingNav::OpenDoc {
            path: path.to_string(),
            line,
        },
        tx,
    );
}

/// The PendingNav::OpenDoc arm: issue the bounded, generation-tagged file
/// read for the current checkout.
pub fn request_doc_load(s: &mut State, path: &str, line: Option<u64>) {
    let Some(checkout) = s.editor.checkout.clone() else {
        return;
    };
    let Some(key) = s.editor.checkout_key.clone() else {
        return;
    };
    s.editor.doc_seq += 1;
    let token = s.editor.doc_seq;
    if s.editor.enqueue_read(
        checkout,
        key,
        Req::File {
            path: path.to_string(),
            token,
            line,
        },
    ) {
        // `doc_load` is armed only for an accepted job: a refused queue
        // sends no reply, so a pre-set token would wedge every later File
        // reply behind `doc_load == Some(token)` forever.
        s.editor.doc_load = Some(token);
    } else {
        s.editor.doc_load = None;
        files_queue_full(s);
    }
}

/// The Changes layer `path` currently sits in, freshest data for THIS
/// checkout only — desktop `layerFor` priority: conflict, staged, unstaged,
/// untracked. `None` when changes data is absent, belongs to another
/// checkout, or the path has no entry.
fn changed_layer(s: &State, path: &str) -> Option<&'static str> {
    let changes = &s.editor.changes;
    if changes.checkout.is_none() || changes.checkout != s.editor.checkout_key {
        return None;
    }
    let data = changes.data.as_ref()?;
    let has = |entries: &[app_core::git_view::StatusEntry]| entries.iter().any(|e| e.path == path);
    if has(&data.conflicts) {
        Some("conflict")
    } else if has(&data.staged) {
        Some("staged")
    } else if has(&data.unstaged) {
        Some("unstaged")
    } else if has(&data.untracked) {
        Some("untracked")
    } else {
        None
    }
}

/// Enqueue a save for the open doc (guard already passed).
pub fn save_active_doc(s: &mut State) {
    if let Some(err) = s.editor.doc.as_ref().and_then(|d| d.validate_save()) {
        if let Some(doc) = s.editor.doc.as_mut() {
            doc.save_error = Some(err);
        }
        // A failed validation counts as a failed save: resume nothing.
        s.editor.save_resume = None;
        s.editor.sync_quit_dirty();
        return;
    }
    navigation::enqueue_save(s);
}

/// The open file's path bar: name, dirty state, markdown preview toggle,
/// save/discard for the dirty draft, "Show changes" for a path that has a
/// change layer, and "Reload from disk" under a save error (rendered in the
/// error banner below, matching the desktop alert row).
fn doc_bar(s: &State, ink: &Ink) -> El {
    let Some(doc) = s.editor.doc.as_ref() else {
        return col().w_full().h(0.0);
    };
    let dirty = doc.dirty();
    // Desktop `markdown = text !== null && isMarkdownPath(path)`: any text
    // document with an .md/.markdown/.mdx name (case-insensitive) gets the
    // preview toggle — editable or not — and non-editable markdown can
    // still render its preview while raw stays selectable text.
    let is_md = doc.kind == DocKind::Text && draft::is_markdown_path(&doc.path);
    // `flex_wrap` + `min_h`: at the 760px minimum window the editor column
    // can be ~230px — buttons wrap to a second line instead of painting
    // past the window edge (the doc-bar overflow defect).
    let mut bar = row()
        .w_full()
        .min_h(30.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .flex_wrap()
        .border_sides([0.0, 0.0, 1.0, 0.0], ink.border);
    bar = bar.child(if dirty {
        theme::txt("•", T12M, ink.warning)
    } else {
        theme::txt("", T12M, ink.muted)
    });
    // Desktop shows the checkout-relative path alone (no separate basename
    // label) — one flexible, truncating path keeps every button reachable.
    bar = bar.child(
        theme::txt(&doc.path, T12TN, ink.muted)
            .max_w(380.0)
            .truncate_middle()
            .help_when_truncated(doc.path.clone()),
    );
    if is_md {
        bar = bar.child(
            theme::btn_sm(
                if doc.markdown_preview {
                    "Markdown"
                } else {
                    "Preview"
                },
                None,
                doc.markdown_preview,
                ink,
            )
            .on_press(|st: &mut State| {
                if let Some(d) = st.editor.doc.as_mut() {
                    d.markdown_preview = !d.markdown_preview;
                }
            }),
        );
    }
    bar = bar.child(rune::spacer());
    if doc.editable() {
        // Desktop: Save renders whenever the file is editable, disabled
        // unless the draft is dirty, validation passes and no save is in
        // flight (`disabled={!draft || saving || draftTooLarge || draftHasNul}`).
        let can_save = dirty && doc.validate_save().is_none();
        bar = bar.child(
            theme::btn_sm(if doc.saving { "Saving…" } else { "Save" }, None, true, ink)
                .disabled(!can_save)
                .on_press_tx(|st: &mut State, _tx: &mut Tx| {
                    // `.disabled` is ARIA/focus only — the press still fires,
                    // so re-check the same gate the button showed.
                    let ok = st
                        .editor
                        .doc
                        .as_ref()
                        .is_some_and(|d| d.dirty() && d.validate_save().is_none());
                    if ok {
                        save_active_doc(st);
                    }
                }),
        );
    }
    if dirty {
        bar = bar.child(
            theme::btn_sm("Discard", None, true, ink)
                .disabled(doc.saving)
                .on_press(|st: &mut State| {
                    // Same guard as Save: no discard while a save reply is
                    // in flight — `mark_saved` would race the restored text.
                    if st.editor.doc.as_ref().is_some_and(|d| !d.saving) {
                        if let Some(doc) = st.editor.doc.as_mut() {
                            doc.discard();
                        }
                        st.editor.sync_quit_dirty();
                    }
                }),
        );
    }
    if let Some(layer) = changed_layer(s, &doc.path) {
        let path = doc.path.clone();
        bar = bar.child(theme::btn_sm("Show changes", None, true, ink).on_press_tx(
            move |st: &mut State, tx: &mut Tx| {
                navigation::navigate(
                    st,
                    PendingNav::ShowChanges {
                        layer: layer.to_string(),
                        path: path.clone(),
                    },
                    tx,
                );
            },
        ));
    }
    bar
}

/// The code surface: line-number gutter + `text_input` bound to the open
/// doc's `TextEdit`. Read-only kinds render selectable text instead.
fn editor_pane(s: &State, ink: &Ink, k: &Kit) -> El {
    let Some(doc) = s.editor.doc.as_ref() else {
        return col().grow(1.0).items_center().justify_center().child(
            // `truncate` shrinks the line to the column — a narrow
            // editor pane must not push the hint past the window.
            theme::txt("Open a file from the tree or search", T12, ink.muted).truncate_middle(),
        );
    };
    let mut pane = col().grow(1.0).min_w(0.0).h_full().min_h(0.0).fill(ink.bg);
    if !doc.editable() && doc.media.is_none() {
        // One concise reason per document — a truncated text file IS the
        // cap notice; stacking "read only — read only" over a second
        // "…— read only" banner was the duplicate-label defect.
        let reason = match doc.kind {
            DocKind::Text if doc.truncated => {
                "file exceeds the 1 MiB preview cap — read only".to_string()
            }
            DocKind::Binary => "binary file — read only".to_string(),
            DocKind::NonUtf8 => "non-UTF-8 file — read only".to_string(),
            DocKind::Missing => "file missing — read only".to_string(),
            DocKind::Unreadable => "unreadable — read only".to_string(),
            DocKind::Text => "read only".to_string(),
        };
        pane = pane.child(
            row()
                .w_full()
                .h(26.0)
                .px(10.0)
                .items_center()
                .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
                .child(theme::txt(&reason, T11M_LH, ink.warning)),
        );
    }
    if let Some(err) = &doc.save_error {
        pane = pane.child(
            row()
                .w_full()
                .h(30.0)
                .px(10.0)
                .gap(8.0)
                .items_center()
                .border_sides([0.0, 0.0, 1.0, 0.0], ink.border)
                .child(
                    theme::txt(&format!("save failed: {err}"), T11M_LH, ink.danger)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate_middle(),
                )
                // Desktop alert row: the draft survives until the reload
                // reply lands; the button is the escape hatch that replaces
                // baseline AND buffer with fresh disk content.
                .child(
                    theme::btn_sm("Reload from disk", None, false, ink)
                        .disabled(s.editor.doc_reload.is_some())
                        .on_press(|st: &mut State| {
                            // One reload in flight at a time — a second
                            // press would only supersede the first token.
                            if st.editor.doc_reload.is_none() {
                                navigation::request_doc_reload(st);
                            }
                        }),
                ),
        );
    }
    // Desktop surface pick: rendered preview whenever the doc is markdown
    // and preview is toggled — editable or read-only — else the editable
    // code surface, else selectable read-only text.
    let is_md = doc.kind == DocKind::Text && draft::is_markdown_path(&doc.path);
    if let Some(media) = &doc.media {
        pane.child(crate::editor::media::view(media, ink))
    } else if is_md && doc.markdown_preview {
        pane.child(markdown_surface(s, k).grow(1.0).min_h(0.0))
    } else if doc.editable() {
        pane.child(editor_surface(s, ink, k).grow(1.0).min_h(0.0))
    } else {
        pane.child(read_only_surface(s, ink, k).grow(1.0).min_h(0.0))
    }
}

/// Gutter + scroll-synced text input. `TextEdit` owns selection/undo/IME;
/// the gutter re-derives line numbers from `edit.text` each frame.
fn editor_surface(s: &State, ink: &Ink, k: &Kit) -> El {
    crate::editor::code::Code::<State>::new(code_get, code_get_mut)
        .on_changed(|st: &mut State, tx: &mut Tx| {
            // CodeState::text_edited already ran (revision/find/gutter); the
            // draft flag only needs the OS-quit mirror refreshed, plus the
            // syntax-highlight debounce armed for the new revision.
            st.editor.sync_quit_dirty();
            crate::editor::highlight::mark_edit(&mut st.editor);
            // The blame annotations describe the file on disk — a dirty
            // buffer hides the column until the save lands.
            crate::editor::blame::sync_visibility(&mut st.editor);
            tx.wake_after(std::time::Duration::from_millis(150));
        })
        .on_save(|st: &mut State, _tx: &mut Tx| {
            save_active_doc(st);
        })
        .on_go_to(
            |st: &mut State, req: crate::editor::code::GoToRequest, tx: &mut Tx| {
                navigation::start_go_to(st, req, tx);
            },
        )
        .on_indent(|st: &mut State, choice, _tx: &mut Tx| {
            st.editor.set_indent_choice(choice);
        })
        .on_blame(|st: &mut State, enabled, _tx: &mut Tx| {
            crate::editor::blame::set_enabled(&mut st.editor, enabled);
        })
        .build(s, ink, k)
}

/// `Code` view accessors: the open document's code surface, or the scratch
/// state when no doc is mounted (the view only builds with a doc anyway).
fn code_get(s: &State) -> &crate::editor::code::CodeState {
    s.editor
        .doc
        .as_ref()
        .map(|d| &d.code)
        .unwrap_or(&s.editor.scratch_code)
}
fn code_get_mut(s: &mut State) -> &mut crate::editor::code::CodeState {
    s.editor
        .doc
        .as_mut()
        .map(|d| &mut d.code)
        .unwrap_or(&mut s.editor.scratch_code)
}

/// Rendered markdown for `.md` previews (raw source stays in `doc.code.edit`).
fn markdown_surface(s: &State, k: &Kit) -> El {
    let text = s
        .editor
        .doc
        .as_ref()
        .map(|d| d.code.edit.text.clone())
        .unwrap_or_default();
    rune_kit::markdown::Markdown::new(text)
        .on_link(|_s: &mut State, url: &str| crate::panes::open_url(url))
        .build(k)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .h_full()
        .fill(s.ink.bg)
        .scroll_bar(k)
}

/// Non-editable kinds show the text read-only (scrollable/selectable).
fn read_only_surface(s: &State, ink: &Ink, k: &Kit) -> El {
    let text = s
        .editor
        .doc
        .as_ref()
        .map(|d| d.code.edit.text.clone())
        .unwrap_or_default();
    col()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .h_full()
        .fill(ink.bg)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(
            rune::text(text)
                .font(theme::T12.lh(18.0))
                .color(ink.text)
                .selectable(true)
                .px(12.0)
                .py(8.0),
        )
        .scroll_bar(k)
}

/// Right-hand pane while content search is open: grouped results that open
/// the file at the matched line.
fn search_results(s: &State, ink: &Ink, k: &Kit) -> El {
    let pane = col().grow(1.0).min_w(0.0).h_full().min_h(0.0).fill(ink.bg);
    let search = &s.editor.files.search;
    if search.searching {
        return pane.child(theme::txt("Searching…", T12, ink.muted));
    }
    if let Some(err) = &search.error {
        return pane.child(theme::txt(err, T12, ink.danger));
    }
    let Some(results) = &search.results else {
        return pane.child(theme::txt("Type at least 2 characters", T12, ink.muted));
    };
    let mut list = col()
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .key(keyboard::nav_scroll_scope(
            crate::editor::state::Section::Files,
        ))
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(theme::txt(
            &format!(
                "{} matches{}",
                results.matches.len(),
                if results.truncated {
                    " (truncated)"
                } else {
                    ""
                }
            ),
            T11M_LH,
            ink.muted,
        ));
    // Group by path (first-seen order, any backend ordering safe): a file
    // header row followed by indented `line: preview` rows — the desktop
    // flat `path:line text` list made same-file hits hard to scan.
    let mut groups: Vec<(&str, Vec<&app_core::git_view::GitViewSearchMatch>)> = Vec::new();
    for m in results.matches.iter().take(500) {
        match groups.iter_mut().find(|(p, _)| *p == m.path.as_str()) {
            Some((_, ms)) => ms.push(m),
            None => groups.push((m.path.as_str(), vec![m])),
        }
    }
    for (path, matches) in groups {
        list = list.child(
            row()
                .w_full()
                .px(10.0)
                .pt(6.0)
                .pb(2.0)
                .gap(8.0)
                .items_center()
                .child(
                    theme::txt(path, T12M, ink.accent)
                        .min_w(0.0)
                        .grow(1.0)
                        .truncate_middle()
                        .help_when_truncated(path.to_string()),
                )
                .child(theme::txt(&matches.len().to_string(), T11M_LH, ink.muted)),
        );
        for m in matches {
            let file = path.to_string();
            let line = m.line;
            let row_key = keyboard::search_row_key(&file, line);
            let nav_cur = keyboard::nav_active(s, crate::editor::state::Section::Files, &row_key);
            list = list.child(
                row()
                    .w_full()
                    .px(10.0)
                    .pl(22.0)
                    .py(4.0)
                    .gap(8.0)
                    .items_center()
                    .key(row_key)
                    .fill(if nav_cur {
                        ink.accent.alpha(0.14)
                    } else {
                        rune::Color::TRANSPARENT
                    })
                    .hover(ink.text.alpha(0.06))
                    .role("button")
                    .label(format!("{}:{}", file, line))
                    .focusable(true)
                    .focus_ring(2.0, 1.0, ink.accent)
                    .child(theme::txt(&line.to_string(), T12TN, ink.muted).shrink())
                    .child(
                        theme::txt(&m.text, T12, ink.text)
                            .min_w(0.0)
                            .grow(1.0)
                            .truncate_middle(),
                    )
                    .on_press_tx(move |st: &mut State, tx: &mut Tx| {
                        open_doc(st, &file, Some(line), tx);
                    }),
            );
        }
    }
    pane.child(list.scroll_bar(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime};
    use std::path::PathBuf;

    /// Files section with a populated tree: root file + two folders, one
    /// checkout selected, no document open.
    fn scene() -> State {
        let mut s = State::new(crate::test_handle());
        s.view = crate::View::Editor;
        s.editor.project_id = Some("p1".into());
        let co = PathBuf::from("/tmp/goko-files-qa");
        s.editor.checkout = Some(co.clone());
        s.editor.checkout_key = Some(crate::editor::state::checkout_key("p1", &co));
        s.editor.section = crate::editor::state::Section::Files;
        s.editor.files.paths = [
            "README.md",
            "docs/guide.md",
            "src/a.rs",
            "src/b.rs",
            "top.txt",
        ]
        .iter()
        .map(|p| p.to_string())
        .collect();
        s.editor.files.search.open = false;
        s.editor.rebuild_file_model();
        s
    }

    fn files_only(s: &State, _cx: &mut Cx<State>) -> El {
        files_view(s, &s.ink, &s.editor.kit)
    }

    fn click(rt: &mut Runtime<State>, x: f32, y: f32) {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn type_text(rt: &mut Runtime<State>, text: &str) {
        for ch in text.chars() {
            rt.key(KeyEvent {
                key: Key::Text(ch.to_string()),
                mods: Mods::default(),
            });
        }
        rt.frame(1.0 / 60.0);
    }

    fn backspace(rt: &mut Runtime<State>, n: usize) {
        for _ in 0..n {
            rt.key(KeyEvent {
                key: Key::Backspace,
                mods: Mods::default(),
            });
        }
        rt.frame(1.0 / 60.0);
    }

    fn labels(rt: &Runtime<State>) -> Vec<String> {
        rt.elements().into_iter().filter_map(|e| e.label).collect()
    }

    fn textboxes(rt: &Runtime<State>) -> Vec<rune::ElementInfo> {
        let mut v: Vec<_> = rt
            .elements()
            .into_iter()
            .filter(|e| e.role == "textbox")
            .collect();
        v.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        v
    }

    /// The filter field must be a real editable TextEdit (`on_edit`): typed
    /// text lands in state and rebuilds the tree model — the inert-field
    /// regression this guards against.
    #[test]
    fn ignored_entries_join_the_tree_dimmed_and_an_ignored_dir_is_not_openable() {
        let mut s = scene();
        s.editor.files.ignored = vec![".env".into(), ".git/".into(), "target/".into()];
        s.editor.rebuild_file_model();
        let m = &s.editor.files.model;
        assert_eq!(m.get(".env").unwrap().data, Some(FileMark::Ignored));
        assert_eq!(m.get("target").unwrap().data, Some(FileMark::IgnoredDir));
        assert_eq!(m.get(".git").unwrap().data, Some(FileMark::IgnoredDir));
        assert_eq!(m.get("top.txt").unwrap().data, Some(FileMark::Tracked));
        assert!(is_openable(&s, ".env"), "an ignored file opens");
        assert!(!is_openable(&s, "target"), "an ignored directory is a leaf");
        assert!(!is_openable(&s, "src"), "a folder toggles instead");
        s.editor.files.filter = rune::edit::TextEdit::new(".env".to_string());
        s.editor.rebuild_file_model();
        assert!(
            s.editor.files.model.get("target").is_none(),
            "the filter applies"
        );
        assert!(s.editor.files.model.get(".env").is_some());
    }

    #[test]
    fn filter_input_types_and_filters_tree() {
        let mut rt = Runtime::new(scene(), files_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let tb = textboxes(&rt)
            .into_iter()
            .next()
            .expect("filter textbox mounted");
        let r = tb.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);

        type_text(&mut rt, "README");
        assert_eq!(rt.state.editor.files.filter.text, "README");
        let ls = labels(&rt);
        assert!(ls.iter().any(|l| l == "README.md"), "match stays: {ls:?}");
        assert!(
            !ls.iter()
                .any(|l| l == "src" || l == "docs" || l == "top.txt"),
            "non-matches hide: {ls:?}"
        );

        backspace(&mut rt, 6);
        assert_eq!(rt.state.editor.files.filter.text, "");
        let ls = labels(&rt);
        assert!(
            ls.iter().any(|l| l == "src"),
            "cleared filter restores: {ls:?}"
        );
        assert!(ls.iter().any(|l| l == "top.txt"), "{ls:?}");
    }

    /// The content-search field is editable too: typing records the query
    /// and arms the debounce (`search.pending`), proving `on_edit` works.
    #[test]
    fn search_input_types_and_arms_debounce() {
        let mut s = scene();
        s.editor.files.search.open = true;
        let mut rt = Runtime::new(s, files_only, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        // Each TextInput mounts a wrapper + field, both role="textbox" —
        // 2 inputs = 4 elements; the rightmost is the search field.
        let tbs = textboxes(&rt);
        assert_eq!(tbs.len(), 4, "filter + search inputs mounted");
        let r = tbs.last().expect("search textbox").rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);

        type_text(&mut rt, "fn main");
        assert_eq!(rt.state.editor.files.search_edit.text, "fn main");
        let pending = rt
            .state
            .editor
            .files
            .search
            .pending
            .as_ref()
            .map(|(q, _)| q.clone());
        assert_eq!(pending.as_deref(), Some("fn main"));
    }

    /// Desktop parity: a single click on a file row opens it — the press
    /// selects AND fires `on_select`, which routes through the draft guard
    /// into `request_doc_load` (bounded, generation-tagged read).
    #[test]
    fn single_click_file_row_requests_doc_load() {
        let mut rt = Runtime::new(scene(), files_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        // "src" starts closed; expand it first so a.rs is reachable, then
        // click the file row itself.
        let dir = rt
            .elements()
            .into_iter()
            .find(|e| e.role == "treeitem" && e.label.as_deref() == Some("src"))
            .expect("src dir row mounted");
        let r = dir.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.frame(0.0);

        let file = rt
            .elements()
            .into_iter()
            .find(|e| e.role == "treeitem" && e.label.as_deref() == Some("a.rs"))
            .expect("a.rs row visible after expand");
        let r = file.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);

        assert!(
            rt.state.editor.doc_load.is_some(),
            "single click issued the doc read"
        );
        assert_eq!(
            rt.state.editor.files.model.cursor(),
            Some("src/a.rs"),
            "row stayed selected"
        );
    }

    // -- refused-queue regressions: flags must never stick ---------------
    //
    // `project_id = None` makes `enqueue_read` return `false` deterministically
    // — the same contract a full or disconnected queue gives. No reply will
    // ever arrive, so `loading`/`doc_load` must be settled by the caller.

    #[test]
    fn rejected_refresh_clears_loading_and_surfaces_error() {
        let mut s = scene();
        s.editor.project_id = None;
        refresh_files(&mut s);
        assert!(!s.editor.files.loading, "refusal clears loading");
        assert!(s.editor.files.error.is_some(), "refusal sets files.error");
        assert!(s.editor.notice.is_some(), "refusal raises the notice bar");
    }

    #[test]
    fn accepted_refresh_arms_loading() {
        let mut s = scene();
        refresh_files(&mut s);
        assert!(s.editor.files.loading, "accepted read arms loading");
        assert!(s.editor.files.error.is_none());
        assert!(s.editor.notice.is_none());
    }

    #[test]
    fn rejected_doc_load_keeps_doc_and_arms_no_token() {
        let mut s = scene();
        let file = app_core::git_view::GitViewFile {
            path: "src/a.rs".into(),
            kind: "text",
            text: Some("fn main() {}\n".into()),
            truncated: false,
            size: 12,
        };
        s.editor.doc = Some(crate::editor::draft::OpenDoc::new("src/a.rs", &file));
        s.editor.project_id = None;

        request_doc_load(&mut s, "src/b.rs", None);
        assert!(
            s.editor.doc_load.is_none(),
            "refused read arms no dangling doc_load token"
        );
        let doc = s.editor.doc.as_ref().expect("old doc retained");
        assert_eq!(doc.path, "src/a.rs", "previous document untouched");
        assert!(s.editor.files.error.is_some());
        assert!(s.editor.notice.is_some());
    }

    #[test]
    fn accepted_doc_load_arms_token() {
        let mut s = scene();
        request_doc_load(&mut s, "src/a.rs", Some(3));
        assert!(s.editor.doc_load.is_some(), "accepted read arms doc_load");
        assert!(s.editor.files.error.is_none());
        assert!(s.editor.notice.is_none());
    }

    // -- doc-bar seams: Show changes layer, reload-from-disk, markdown ---

    fn open_text_doc(s: &mut State, path: &str, text: &str) {
        let file = app_core::git_view::GitViewFile {
            path: path.into(),
            kind: "text",
            text: Some(text.into()),
            truncated: false,
            size: text.len() as u64,
        };
        s.editor.doc = Some(crate::editor::draft::OpenDoc::new(path, &file));
    }

    fn status(path: &str) -> app_core::git_view::StatusEntry {
        app_core::git_view::StatusEntry {
            path: path.into(),
            orig_path: None,
            status: "modified",
            added: None,
            removed: None,
        }
    }

    fn changes_data() -> app_core::git_view::GitViewChanges {
        app_core::git_view::GitViewChanges {
            branch: Some("main".into()),
            head: "abc1234".into(),
            conflicts: vec![],
            staged: vec![status("src/a.rs")],
            unstaged: vec![status("src/b.rs")],
            untracked: vec![status("new.txt")],
            ahead: None,
        }
    }

    /// `layerFor` parity: conflict > staged > unstaged > untracked, exact
    /// path match, and only while the data belongs to THIS checkout.
    #[test]
    fn changed_layer_priority_and_checkout_freshness() {
        let mut s = scene();
        open_text_doc(&mut s, "src/a.rs", "fn main() {}\n");
        assert!(
            changed_layer(&s, "src/a.rs").is_none(),
            "no data → no layer"
        );

        s.editor.changes.data = Some(changes_data());
        s.editor.changes.checkout = s.editor.checkout_key.clone();
        assert_eq!(changed_layer(&s, "src/a.rs"), Some("staged"));
        assert_eq!(changed_layer(&s, "src/b.rs"), Some("unstaged"));
        assert_eq!(changed_layer(&s, "new.txt"), Some("untracked"));
        assert_eq!(changed_layer(&s, "other.rs"), None, "path must match");

        // A conflict entry wins over staged for the same path.
        s.editor
            .changes
            .data
            .as_mut()
            .unwrap()
            .conflicts
            .push(status("src/a.rs"));
        assert_eq!(changed_layer(&s, "src/a.rs"), Some("conflict"));

        // Data stamped for another checkout never leaks into this bar.
        s.editor.changes.checkout = Some("deadbeef".into());
        assert!(changed_layer(&s, "src/a.rs").is_none());
        s.editor.changes.checkout = None;
        assert!(changed_layer(&s, "src/a.rs").is_none());
    }

    /// Reload arms `doc_reload` only on an accepted enqueue and never
    /// touches the draft before the reply lands.
    #[test]
    fn accepted_reload_arms_token_and_keeps_draft() {
        let mut s = scene();
        open_text_doc(&mut s, "src/a.rs", "fn main() {}\n");
        s.editor.doc.as_mut().unwrap().save_error = Some("disk conflict".into());
        navigation::request_doc_reload(&mut s);
        assert!(s.editor.doc_reload.is_some(), "accepted reload arms token");
        let doc = s.editor.doc.as_ref().unwrap();
        assert_eq!(doc.code.edit.text, "fn main() {}\n", "draft untouched");
        assert_eq!(doc.save_error.as_deref(), Some("disk conflict"));
    }

    #[test]
    fn rejected_reload_arms_no_token_and_keeps_draft() {
        let mut s = scene();
        open_text_doc(&mut s, "src/a.rs", "fn main() {}\n");
        s.editor.doc.as_mut().unwrap().save_error = Some("disk conflict".into());
        s.editor.project_id = None; // enqueue_read refuses deterministically
        navigation::request_doc_reload(&mut s);
        assert!(s.editor.doc_reload.is_none(), "refusal arms no token");
        let doc = s.editor.doc.as_ref().unwrap();
        assert_eq!(doc.path, "src/a.rs");
        assert_eq!(doc.code.edit.text, "fn main() {}\n");
        assert!(s.editor.notice.is_some(), "refusal surfaces a notice");
    }

    /// The inline-blame column and its toggle chip are part of the code
    /// surface: the annotations sit beside the code, one row per line.
    #[test]
    fn the_blame_column_renders_beside_the_code() {
        let mut s = scene();
        open_text_doc(&mut s, "src/a.rs", "fn main() {}\nfn two() {}\n");
        {
            let doc = s.editor.doc.as_mut().unwrap();
            doc.code.blame_visible = true;
            doc.code.blame_text = "Alice · 1m\nBob · 2m".into();
        }
        let mut rt = Runtime::new(s, files_only, (1200.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let els = rt.elements();
        let ls: Vec<&str> = els.iter().filter_map(|e| e.label.as_deref()).collect();
        for e in &els {
            if let Some(l) = &e.label
                && (l == "Blame" || l.contains("Spaces"))
            {
                println!(
                    "PROBE {:?} role={} rect={:?} text={:?}",
                    l, e.role, e.rect, e.text
                );
            }
        }
        assert!(ls.contains(&"Blame"), "the blame chip is missing: {ls:?}");
        let texts: Vec<&str> = els.iter().filter_map(|e| e.text.as_deref()).collect();
        assert!(
            texts.iter().any(|t| t.contains("Alice · 1m")),
            "the blame column is missing: {texts:?}"
        );
    }

    /// The doc bar mounts Show changes / Reload from disk / Save at the
    /// narrow 760px editor width, and Show changes routes through the guard
    /// into `changes::select_entry` + the Changes section.
    #[test]
    fn doc_bar_buttons_present_and_show_changes_navigates() {
        let mut s = scene();
        open_text_doc(&mut s, "src/a.rs", "fn main() {}\n");
        s.editor.changes.data = Some(changes_data());
        s.editor.changes.checkout = s.editor.checkout_key.clone();
        s.editor.doc.as_mut().unwrap().save_error = Some("disk conflict".into());
        let mut rt = Runtime::new(s, files_only, (760.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let ls = labels(&rt);
        for want in ["Save", "Show changes", "Reload from disk"] {
            assert!(ls.iter().any(|l| l == want), "missing {want}: {ls:?}");
        }
        // Clean doc → Save renders disabled; `.disabled` is ARIA/focus-only
        // on ElementInfo, so the press-guard re-check in doc_bar is what a
        // stray click hits (covered by the state tests above).

        let btn = rt
            .elements()
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Show changes"))
            .expect("show changes button");
        let r = btn.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.frame(0.0);
        assert_eq!(
            rt.state.editor.section,
            crate::editor::state::Section::Changes,
            "Show changes lands on the Changes section"
        );
        assert_eq!(
            rt.state.editor.changes.layer.as_deref(),
            Some("staged"),
            "patch pane targeted the staged layer"
        );
        assert_eq!(
            rt.state.editor.changes.patch_path.as_deref(),
            Some("src/a.rs")
        );
    }

    /// Clicking Reload from disk issues `Req::ReloadFile` through the
    /// navigation seam — `doc_reload` arms, the draft stays put.
    #[test]
    fn reload_button_issues_reload_file() {
        let mut s = scene();
        open_text_doc(&mut s, "src/a.rs", "fn main() {}\n");
        s.editor.doc.as_mut().unwrap().save_error = Some("disk conflict".into());
        let mut rt = Runtime::new(s, files_only, (760.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let btn = rt
            .elements()
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Reload from disk"))
            .expect("reload button");
        let r = btn.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.frame(0.0);
        assert!(rt.state.editor.doc_reload.is_some(), "reload token armed");
        assert_eq!(
            rt.state.editor.doc.as_ref().unwrap().code.edit.text,
            "fn main() {}\n",
            "draft preserved until the reply"
        );
    }

    /// `.MDX` (any case) gets the Preview toggle like `.md`; toggling flips
    /// `markdown_preview` and the button label.
    #[test]
    fn markdown_mdx_case_insensitive_toggle() {
        let mut s = scene();
        open_text_doc(&mut s, "docs/README.MDX", "# Title\n");
        let mut rt = Runtime::new(s, files_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let btn = rt
            .elements()
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Preview"))
            .expect("Preview toggle for .MDX");
        let r = btn.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.frame(0.0);
        assert!(
            rt.state.editor.doc.as_ref().unwrap().markdown_preview,
            "preview toggled on"
        );
        let ls = labels(&rt);
        assert!(ls.iter().any(|l| l == "Markdown"), "label flipped: {ls:?}");
    }

    /// A truncated (read-only) markdown file still gets the preview toggle
    /// — desktop shows Preview/Raw for any text markdown doc.
    #[test]
    fn read_only_markdown_still_previews() {
        let mut s = scene();
        let file = app_core::git_view::GitViewFile {
            path: "big.md".into(),
            kind: "text",
            text: Some("# Heading\nbody\n".into()),
            truncated: true,
            size: 4096,
        };
        s.editor.doc = Some(crate::editor::draft::OpenDoc::new("big.md", &file));
        assert!(!s.editor.doc.as_ref().unwrap().editable());
        let mut rt = Runtime::new(s, files_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);

        let btn = rt
            .elements()
            .into_iter()
            .find(|e| e.label.as_deref() == Some("Preview"))
            .expect("Preview toggle on read-only markdown");
        let r = btn.rect;
        click(&mut rt, r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.frame(0.0);
        assert!(
            rt.state.editor.doc.as_ref().unwrap().markdown_preview,
            "read-only markdown previews"
        );
    }

    // -- pane resizing -----------------------------------------------------

    fn pane_grip(rt: &Runtime<State>) -> rune::ElementInfo {
        rt.elements()
            .into_iter()
            .find(|e| {
                e.role == "separator" && e.label.as_deref() == Some("Resize file list and editor")
            })
            .expect("files pane grip mounted")
    }

    fn drag_x(rt: &mut Runtime<State>, x0: f32, y: f32, x1: f32) {
        rt.pointer_move(x0, y);
        rt.pointer_down(x0, y);
        rt.frame(1.0 / 60.0);
        for i in 1..=4 {
            rt.pointer_move(x0 + (x1 - x0) * i as f32 / 4.0, y);
            rt.frame(1.0 / 60.0);
        }
        rt.pointer_up(x1, y);
        rt.frame(1.0 / 60.0);
    }

    /// The grip is a real separator (not a Tab stop), spans the pane height,
    /// and dragging it right widens the persisted aside width.
    #[test]
    fn pane_grip_drags_and_arms_persist() {
        let mut rt = Runtime::new(scene(), files_only, (1100.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        let g = pane_grip(&rt);
        assert!(!g.focusable, "grip is not a Tab stop (kit parity)");
        assert!(g.rect[3] > 100.0, "grip spans the pane height: {g:?}");
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);
        drag_x(&mut rt, x, y, x + 80.0);

        let w = rt.state.editor.history.layout.panes.files_aside;
        assert!((w - 360.0).abs() < 1.0, "drag +80px: 280 -> {w}");
        assert!(
            rt.state.editor.history.layout_dirty,
            "layout_dirty arms the settings lane"
        );
        let json = rt.state.editor.history.layout.to_json();
        assert!(json.contains("\"filesAside\":360"), "stored doc: {json}");
    }

    /// Dragging past either bound clamps: the static PANE_MIN..=PANE_MAX
    /// range AND the viewport max (row width - SIBLING_MIN).
    #[test]
    fn pane_drag_clamps_min_and_viewport() {
        // 500px row leaves 260px for the aside (500 - SIBLING_MIN 240).
        let mut rt = Runtime::new(scene(), files_only, (500.0, 400.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.editor.pane_row_w > 400.0, "row measured");

        let g = pane_grip(&rt);
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);
        drag_x(&mut rt, x, y, x + 500.0);
        let expected = pane_layout::max_width(rt.state.editor.pane_row_w);
        assert!(
            (rt.state.editor.history.layout.panes.files_aside - expected as f64).abs() < 1.0,
            "right drag clamps to viewport max {expected}"
        );
        assert!(expected <= 260.0, "narrow row caps the aside: {expected}");

        // The grip moved with the aside — re-query before the next drag.
        let g = pane_grip(&rt);
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);
        drag_x(&mut rt, x, y, x - 900.0);
        assert_eq!(
            rt.state.editor.history.layout.panes.files_aside,
            pane_layout::PANE_MIN as f64,
            "left drag clamps to PANE_MIN"
        );
    }

    /// At the 760px QA viewport the grip stays inside the window and a
    /// modest drag still lands inside the persisted bounds.
    #[test]
    fn pane_grip_fits_760_viewport() {
        let mut rt = Runtime::new(scene(), files_only, (760.0, 480.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);

        let g = pane_grip(&rt);
        assert!(
            g.rect[0] >= 0.0 && g.rect[0] + g.rect[2] <= 760.5,
            "grip inside viewport: {:?}",
            g.rect
        );
        let (x, y) = (g.rect[0] + g.rect[2] / 2.0, g.rect[1] + g.rect[3] / 2.0);
        drag_x(&mut rt, x, y, x - 60.0);
        let w = rt.state.editor.history.layout.panes.files_aside;
        assert!(
            (pane_layout::PANE_MIN as f64..=pane_layout::PANE_MAX as f64).contains(&w),
            "dragged width {w} stays in static bounds"
        );
    }

    /// A restored oversized width re-clamps through flex on a narrow row:
    /// the stored value stays the user's choice while the rendered aside
    /// yields to the SIBLING_MIN detail floor.
    #[test]
    fn restored_oversized_width_clamps_on_narrow_row() {
        let mut s = scene();
        s.editor
            .history
            .layout
            .panes
            .set(PaneKind::FilesAside, 480.0);
        let mut rt = Runtime::new(s, files_only, (500.0, 400.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 60.0);
        // Stored intent is preserved…
        assert!((rt.state.editor.history.layout.panes.files_aside - 480.0).abs() < f64::EPSILON);
        // …and the viewport max the grip advertises is the clamped one.
        let g = pane_grip(&rt);
        assert!(
            g.rect[0] + g.rect[2] <= 500.5,
            "clamped aside keeps the grip on-screen: {:?}",
            g.rect
        );
    }

    // ---- keyboard seams ----------------------------------------------------

    /// Runtime with the app-level keyboard handler attached, like `main.rs`.
    fn kb_runtime(s: State) -> Runtime<State> {
        let mut rt = Runtime::new(s, files_only, (900.0, 700.0), Env::new(true, 2.0));
        rt.on_key = Some(crate::editor::keyboard::on_key);
        rt.frame(0.0);
        rt
    }

    fn key_text(rt: &mut Runtime<State>, t: &str) {
        rt.key(KeyEvent {
            key: Key::Text(t.to_string()),
            mods: Mods::default(),
        });
        rt.frame(0.0);
    }

    /// Click the tree row labelled `name`; the press also moves DOM focus to
    /// the tree root (the nearest focusable ancestor), which is what routes
    /// later keys through the composite `tree_key` handler.
    fn click_tree_row(rt: &mut Runtime<State>, name: &str) {
        let e = rt
            .elements()
            .into_iter()
            .find(|e| e.role == "treeitem" && e.label.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("tree row {name} mounted"));
        click(rt, e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
        rt.frame(0.0);
        assert!(
            rt.elements().iter().any(|e| e.focused && e.role == "tree"),
            "tree holds focus after row click"
        );
    }

    /// A right-click anywhere on a nested row names that row — the menu
    /// opens on it with the pointer's position, not on the cursor row.
    #[test]
    fn secondary_press_on_a_row_opens_its_menu() {
        let mut rt = kb_runtime(scene());
        click_tree_row(&mut rt, "docs");
        click_tree_row(&mut rt, "README.md");
        let row = rt
            .elements()
            .into_iter()
            .find(|e| e.role == "treeitem" && e.label.as_deref() == Some("guide.md"))
            .expect("the folder opened");
        let (x, y) = (row.rect[0] + 60.0, row.rect[1] + row.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(1.0 / 60.0);
        let menu = rt.state.editor.files.menu.as_ref().expect("menu opened");
        assert_eq!(menu.id, "docs/guide.md");
        assert_eq!((menu.x, menu.y), (x, y));
        assert_eq!(
            rt.state.editor.files.model.cursor(),
            Some("docs/guide.md"),
            "the row is selected"
        );
    }

    /// Clicking an ignored directory asks for its entries instead of doing
    /// nothing, and a second click on one already read sends no second job.
    #[test]
    fn clicking_an_ignored_directory_reads_its_entries() {
        let (backend, jobs) = crate::editor::backend::channel_backend();
        let mut s = scene();
        s.editor.backend = backend;
        s.editor.files.ignored = vec!["target/".into()];
        s.editor.rebuild_file_model();
        let mut rt = kb_runtime(s);
        click_tree_row(&mut rt, "target");
        let sent: Vec<_> = jobs.try_iter().collect();
        assert!(
            sent.iter()
                .any(|j| matches!(&j.req, Req::IgnoredChildren { dir } if dir == "target")),
            "the directory was requested"
        );
        rt.state
            .editor
            .files
            .ignored_open
            .insert("target".into(), Vec::new());
        click_tree_row(&mut rt, "target");
        assert!(jobs.try_iter().next().is_none(), "already read: no new job");
    }

    /// Move to Trash asks first; confirming sends the job, and an open draft
    /// under the entry blocks it.
    #[test]
    fn move_to_trash_confirms_then_sends_and_a_draft_blocks_it() {
        let (backend, jobs) = crate::editor::backend::channel_backend();
        let mut s = scene();
        s.editor.backend = backend;
        s.editor.files.delete_prompt = Some("docs/guide.md".into());
        let mut rt = kb_runtime(s);
        let button = |rt: &Runtime<State>, label: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("{label} mounted"))
        };
        let cancel = button(&rt, "Cancel");
        click(&mut rt, cancel.rect[0] + 5.0, cancel.rect[1] + 5.0);
        assert!(rt.state.editor.files.delete_prompt.is_none());
        assert!(jobs.try_iter().next().is_none(), "cancel sends nothing");

        rt.state.editor.files.delete_prompt = Some("docs/guide.md".into());
        rt.frame(1.0 / 60.0);
        let confirm = button(&rt, "Move to Trash");
        click(&mut rt, confirm.rect[0] + 5.0, confirm.rect[1] + 5.0);
        assert!(rt.state.editor.files.delete_prompt.is_none());
        assert!(
            jobs.try_iter()
                .any(|j| matches!(&j.req, Req::TrashPath { path } if path == "docs/guide.md"))
        );

        let mut doc = crate::editor::draft::OpenDoc::new(
            "docs/guide.md",
            &app_core::git_view::GitViewFile {
                path: "docs/guide.md".into(),
                kind: "text",
                text: Some("x".into()),
                truncated: false,
                size: 1,
            },
        );
        doc.code.edit.text = "edited".into();
        rt.state.editor.doc = Some(doc);
        submit_trash(&mut rt.state, "docs");
        assert!(
            jobs.try_iter().next().is_none(),
            "a draft blocks the delete"
        );
        assert!(
            rt.state
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("save or discard"))
        );
    }

    /// The toolbar's four actions are there; New File asks for a name in the
    /// selected file's folder, Collapse All closes the tree, and the name
    /// sends the create.
    #[test]
    fn the_toolbar_creates_in_the_selected_folder_and_collapses_all() {
        let (backend, jobs) = crate::editor::backend::channel_backend();
        let mut s = scene();
        s.editor.backend = backend;
        let mut rt = kb_runtime(s);
        let tool = |rt: &Runtime<State>, label: &str| {
            rt.elements()
                .into_iter()
                .find(|e| e.role == "button" && e.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("{label} button mounted"))
        };
        for label in ["New File", "New Folder", "Refresh", "Collapse All"] {
            tool(&rt, label);
        }
        click_tree_row(&mut rt, "src");
        click_tree_row(&mut rt, "a.rs");
        let new_file = tool(&rt, "New File");
        click(&mut rt, new_file.rect[0] + 5.0, new_file.rect[1] + 5.0);
        assert_eq!(
            rt.state.sidebar_input,
            Some(crate::sidebar::SidebarInput::NewEntry {
                dir: "src".into(),
                folder: false
            }),
            "a selected file creates beside itself"
        );
        submit_new(&mut rt.state, "src", false, "  c.rs ");
        assert!(rt.state.sidebar_input.is_none());
        assert!(jobs.try_iter().any(|j| matches!(
            &j.req,
            Req::CreateEntry { dir, name, folder: false } if dir == "src" && name == "c.rs"
        )));
        submit_new(&mut rt.state, "src", true, "   ");
        assert!(
            jobs.try_iter().next().is_none(),
            "an empty name sends nothing"
        );

        assert!(rt.state.editor.files.model.is_expanded("src"));
        let collapse = tool(&rt, "Collapse All");
        click(&mut rt, collapse.rect[0] + 5.0, collapse.rect[1] + 5.0);
        assert!(!rt.state.editor.files.model.is_expanded("src"));
    }

    #[test]
    fn a_new_entry_goes_in_the_folder_of_the_row_or_the_root() {
        let s = scene();
        let model = tree_get(&s);
        assert_eq!(entry_dir(model, "src"), "src", "a folder is its own target");
        assert_eq!(entry_dir(model, "src/a.rs"), "src");
        assert_eq!(entry_dir(model, "top.txt"), "", "a root file → the root");
        assert_eq!(selected_dir(&s), "", "nothing selected → the root");
    }

    #[test]
    fn a_row_id_is_the_longest_node_prefix_of_the_element_path() {
        let s = scene();
        let model = tree_get(&s);
        let id = |path: &str| row_id(model, path);
        assert_eq!(
            id("#0/files/viewport/rows/src/a.rs/label"),
            Some("src/a.rs".into())
        );
        assert_eq!(id("#0/files/viewport/rows/src/icon"), Some("src".into()));
        assert_eq!(id("#0/files/viewport/rows/top.txt"), Some("top.txt".into()));
        assert_eq!(id("#0/files/viewport/rows/nope/label"), None);
        assert_eq!(id("#0/files/toolbar"), None);
    }

    /// `j`/`k` must reach global nav instead of the tree's typeahead: the
    /// highlight cursor moves (the model's own cursor follows via `reveal`)
    /// and no document opens.
    #[test]
    fn tree_jk_preempts_typeahead_without_opening() {
        let mut rt = kb_runtime(scene());
        click_tree_row(&mut rt, "docs"); // folder: selects+opens, never opens a doc
        assert!(rt.state.editor.doc.is_none());

        key_text(&mut rt, "j");
        let nav = rt.state.editor.keyboard.nav.clone().expect("j engaged nav");
        assert_eq!(nav.section, crate::editor::state::Section::Files);
        assert_eq!(nav.index, 0);
        // The tree cursor visibly followed the nav row.
        assert_eq!(rt.state.editor.files.model.cursor(), Some(nav.key.as_str()));
        assert!(
            rt.state.editor.doc.is_none() && rt.state.editor.doc_load.is_none(),
            "j/k never opens a document"
        );

        key_text(&mut rt, "j");
        assert_eq!(rt.state.editor.keyboard.nav.as_ref().unwrap().index, 1);
        key_text(&mut rt, "k");
        assert_eq!(rt.state.editor.keyboard.nav.as_ref().unwrap().index, 0);
    }

    /// `1`–`4` switch sections before the tree can read them as typeahead.
    #[test]
    fn tree_digits_preempt_typeahead() {
        let mut rt = kb_runtime(scene());
        click_tree_row(&mut rt, "docs");
        key_text(&mut rt, "2");
        rt.frame(0.0);
        assert_eq!(
            rt.state.editor.section,
            crate::editor::state::Section::Changes,
            "digit 2 switched sections instead of typeahead"
        );
    }

    /// Letters the nav layer doesn't own stay native typeahead, and arrows
    /// keep the kit dispatch — including the existing select→open parity.
    #[test]
    fn tree_letters_and_arrows_stay_native() {
        let mut rt = kb_runtime(scene());
        click_tree_row(&mut rt, "docs");

        // "t" typeahead-jumps to top.txt and the preserved on_select opens it.
        key_text(&mut rt, "t");
        assert_eq!(
            rt.state.editor.files.model.cursor(),
            Some("top.txt"),
            "letter kept native typeahead"
        );
        assert!(
            rt.state.editor.keyboard.nav.is_none(),
            "typeahead letter is not a nav key"
        );
        assert!(
            rt.state.editor.doc_load.is_some() || rt.state.editor.doc.is_some(),
            "select→open parity preserved on typeahead move"
        );

        // Arrow Up moves the native cursor to the previous visible row.
        rt.state.editor.doc_load = None;
        rt.state.editor.doc = None;
        rt.key(KeyEvent {
            key: Key::Up,
            mods: Mods::default(),
        });
        rt.frame(0.0);
        let cur = rt.state.editor.files.model.cursor().map(str::to_string);
        assert!(
            cur.is_some() && cur.as_deref() != Some("top.txt"),
            "arrow kept native move: {cur:?}"
        );
    }
}
