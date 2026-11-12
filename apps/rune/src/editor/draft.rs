//! Open documents and their draft semantics: original text, edit buffer,
//! read-only/binary/truncated restrictions, dirty detection and the
//! expected-original-text save contract `git_view::save_file` enforces.

use crate::editor::code::CodeState;

/// Save payload cap shared with `app_core::git_view` (1 MiB).
pub const TEXT_CAP: usize = 1024 * 1024;

/// Desktop `isMarkdownPath` (`MarkdownView.tsx`): `.md`, `.markdown` or
/// `.mdx` suffix, case-insensitive.
pub fn is_markdown_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".mdx")
}

/// Newline tokens (`\r\n`, `\r`, `\n`) in order — JS `/\r\n|\r|\n/g` parity.
fn newline_tokens(text: &str) -> Vec<&'static str> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    tokens.push("\r\n");
                    i += 2;
                } else {
                    tokens.push("\r");
                    i += 1;
                }
            }
            b'\n' => {
                tokens.push("\n");
                i += 1;
            }
            _ => i += 1,
        }
    }
    tokens
}

/// Replace every newline token in `text` with `pick()`'s result, called once
/// per token in order — mirrors `text.replace(/\r\n|\r|\n/g, fn)`.
fn map_newlines(text: &str, mut pick: impl FnMut() -> &'static str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if matches!(bytes[i], b'\r' | b'\n') {
            let len = if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            out.push_str(&text[last..i]);
            out.push_str(pick());
            i += len;
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// What a checkout file read produced; only `Text` is editable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DocKind {
    Text,
    Binary,
    NonUtf8,
    Missing,
    Unreadable,
}

impl DocKind {
    pub fn from_git(kind: &str) -> DocKind {
        match kind {
            "text" => DocKind::Text,
            "binary" => DocKind::Binary,
            "nonutf8" => DocKind::NonUtf8,
            "missing" => DocKind::Missing,
            _ => DocKind::Unreadable,
        }
    }
    pub fn editable(self) -> bool {
        self == DocKind::Text
    }
}

/// One open file: the loaded snapshot (`original`), the live code surface
/// (`code.edit` is the `TextEdit` buffer — syntax, find, go-to and the
/// gutter all hang off it) and the guards a save/navigation decision needs.
/// The parent state owns backend calls; this type is pure data plus small
/// helpers.
pub struct OpenDoc {
    /// Checkout-relative path (the id files/search/reveal all use).
    pub path: String,
    pub kind: DocKind,
    /// Text at load or last successful save — the `expected_text` a save sends.
    pub original: String,
    /// Code surface: `code.edit` is the live edit buffer `text_input` edits
    /// in place; `code.read_only` mirrors `editable()`.
    pub code: CodeState,
    /// File exceeded the 1 MiB preview cap: shown but never editable.
    pub truncated: bool,
    pub size: u64,
    /// Save in flight; navigation waits on it rather than racing it.
    pub saving: bool,
    /// Explicit Mark Resolved waits for a successful save before staging.
    pub resolve_after_save: bool,
    /// Last save failure text (disk conflict, IO, cap…).
    pub save_error: Option<String>,
    /// Pending 1-based line reveal from a search result / open request.
    pub reveal_line: Option<u64>,
    /// `.md` preview toggle (raw markdown source ↔ rendered).
    pub markdown_preview: bool,
    /// Last text-edit instant; `highlight::poll` debounces on it before
    /// enqueueing a `Req::Highlight` for the current revision.
    pub highlight_dirty_at: Option<std::time::Instant>,
    /// A highlight job for this doc is on the worker lane; its reply clears
    /// the flag (matched by path, then revision).
    pub highlight_inflight: bool,
    /// The preview of a binary file the pane can show (image, video, audio).
    pub media: Option<crate::editor::media::Media>,
}

impl OpenDoc {
    pub fn new(path: impl Into<String>, file: &app_core::git_view::GitViewFile) -> OpenDoc {
        let path = path.into();
        let kind = DocKind::from_git(file.kind);
        let text = file.text.clone().unwrap_or_default();
        let truncated = file.truncated;
        let mut code = CodeState::new(path.clone(), text.clone());
        code.read_only = !kind.editable() || truncated;
        OpenDoc {
            path,
            kind,
            original: text,
            code,
            truncated,
            size: file.size,
            saving: false,
            resolve_after_save: false,
            save_error: None,
            reveal_line: None,
            markdown_preview: false,
            highlight_dirty_at: None,
            highlight_inflight: false,
            media: None,
        }
    }

    /// Editable only for complete UTF-8 text files within the cap.
    pub fn editable(&self) -> bool {
        self.kind.editable() && !self.truncated
    }

    /// Save payload — exact port of `EditorFiles.tsx` `textForSave`: when the
    /// buffer has the same number of newline tokens as `original`, each is
    /// rewritten to the original's style in order; otherwise every newline
    /// becomes the original's first style (or `\n` when it has none). The
    /// buffer may hold pasted LF inside CRLF content; this restores the
    /// file's own conventions before the bytes hit disk.
    pub fn text_for_save(&self) -> String {
        let original_newlines = newline_tokens(&self.original);
        let edited_newlines = newline_tokens(&self.code.edit.text);
        if original_newlines.len() == edited_newlines.len() {
            let mut index = 0;
            return map_newlines(&self.code.edit.text, || {
                let token = original_newlines[index];
                index += 1;
                token
            });
        }
        let newline = original_newlines.first().copied().unwrap_or("\n");
        map_newlines(&self.code.edit.text, || newline)
    }

    /// `true` when the save payload differs from the last saved text.
    /// Newline-style-only differences normalize away, so a CRLF file stays
    /// clean after edits that only touched line endings.
    pub fn dirty(&self) -> bool {
        self.editable() && self.text_for_save() != self.original
    }

    /// Validate the normalized save payload before enqueueing a save;
    /// returns the user-facing error.
    pub fn validate_save(&self) -> Option<String> {
        if self.saving {
            return Some("already saving".to_string());
        }
        if !self.editable() {
            return Some(match self.kind {
                DocKind::Binary => "binary files cannot be edited".to_string(),
                DocKind::NonUtf8 => "non-UTF-8 files cannot be edited".to_string(),
                DocKind::Missing => "the file is missing".to_string(),
                DocKind::Unreadable => "the file is unreadable".to_string(),
                DocKind::Text => "the file was truncated at the size cap".to_string(),
            });
        }
        let text = self.text_for_save();
        if text.len() > TEXT_CAP {
            return Some("the file exceeds the 1 MiB save limit".to_string());
        }
        if text.contains('\0') {
            return Some("the file contains a NUL byte".to_string());
        }
        None
    }

    /// After a successful save the returned file becomes the new baseline.
    pub fn mark_saved(&mut self, file: &app_core::git_view::GitViewFile) {
        if let Some(text) = &file.text {
            self.original = text.clone();
            // Keep the user's buffer (it is what was written); only refresh
            // metadata that could have changed on disk.
            self.truncated = file.truncated;
            self.size = file.size;
        }
        self.saving = false;
        self.save_error = None;
    }

    /// Discard: restore the buffer to the saved text as ONE undo step —
    /// select-all + insert inside `undo_group` keeps history intact (unlike
    /// `replace_text`, which resets it), then resync revision/find/gutter
    /// through `text_edited`. `insert` folds `\r\n` to `\n` in the buffer;
    /// `text_for_save` maps it back, so dirty state stays correct.
    pub fn discard(&mut self) {
        let original = self.original.clone();
        self.code.edit.undo_group(|edit| {
            let len = edit.text.len();
            edit.set_selection(0, len);
            edit.insert(&original);
        });
        self.code.text_edited();
        self.save_error = None;
        // Programmatic edits bypass the input callback — re-arm the
        // highlight debounce so the restored text gets re-coloured.
        self.highlight_dirty_at = Some(std::time::Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(kind: &str, text: Option<&str>, truncated: bool) -> app_core::git_view::GitViewFile {
        app_core::git_view::GitViewFile {
            path: "a.txt".into(),
            kind: match kind {
                "text" => "text",
                "binary" => "binary",
                "nonutf8" => "nonutf8",
                "missing" => "missing",
                _ => "unreadable",
            },
            text: text.map(str::to_string),
            truncated,
            size: text.map_or(0, |t| t.len() as u64),
        }
    }

    #[test]
    fn dirty_tracks_buffer_vs_original() {
        let mut doc = OpenDoc::new("a.txt", &file("text", Some("hi\n"), false));
        assert!(!doc.dirty());
        doc.code.edit.insert("x");
        assert!(doc.dirty());
        doc.discard();
        assert!(!doc.dirty());
        assert_eq!(doc.code.edit.text, "hi\n");
    }

    #[test]
    fn non_text_and_truncated_are_read_only() {
        for kind in ["binary", "nonutf8", "missing", "unreadable"] {
            let doc = OpenDoc::new("a", &file(kind, None, false));
            assert!(!doc.editable(), "{kind}");
        }
        let doc = OpenDoc::new("a", &file("text", Some("x"), true));
        assert!(!doc.editable());
    }

    #[test]
    fn save_validation_rejects_cap_nul_and_busy() {
        let mut doc = OpenDoc::new("a", &file("text", Some("x"), false));
        assert!(doc.validate_save().is_none());
        doc.code.edit.insert(&"y".repeat(TEXT_CAP));
        assert!(doc.validate_save().is_some());
        let mut doc = OpenDoc::new("a", &file("text", Some("x"), false));
        doc.code.edit.insert("\0");
        assert!(doc.validate_save().unwrap().contains("NUL"));
        doc.saving = true;
        assert!(doc.validate_save().is_some());
    }

    #[test]
    fn newline_round_trip_preserved() {
        let mut doc = OpenDoc::new("a", &file("text", Some("a\r\nb\n"), false));
        doc.code.edit.insert("c");
        assert!(doc.code.edit.text.contains("\r\n"));
    }

    #[test]
    fn markdown_path_matches_desktop_case_insensitive() {
        for p in [
            "a.md",
            "a.markdown",
            "a.mdx",
            "A.MD",
            "Dir/B.MarkDown",
            "x.MdX",
        ] {
            assert!(is_markdown_path(p), "{p}");
        }
        for p in ["a.txt", "a.mdown", "md", "a.md.bak", "a.markdownx"] {
            assert!(!is_markdown_path(p), "{p}");
        }
    }

    /// `textForSave` port: equal newline counts map positionally onto the
    /// original's styles; a changed count collapses to the first style.
    #[test]
    fn text_for_save_positional_and_first_style() {
        // Count differs (3 edited vs 2 original) → every newline becomes
        // the FIRST original style.
        let mut doc = OpenDoc::new("a", &file("text", Some("a\r\nb\nc"), false));
        doc.code.edit.text = "x\ny\rz\nw".into();
        assert_eq!(doc.text_for_save(), "x\r\ny\r\nz\r\nw");

        // Same count → positional styles preserved even when the edited
        // styles differ at each position.
        let mut doc = OpenDoc::new("a", &file("text", Some("a\r\nb\nc"), false));
        doc.code.edit.text = "p\nq\rr".into(); // tokens [\n, \r] vs [\r\n, \n]
        assert_eq!(doc.text_for_save(), "p\r\nq\nr");

        // Original without newlines: added newlines become plain \n.
        let mut doc = OpenDoc::new("a", &file("text", Some("abc"), false));
        doc.code.edit.text = "a\nb".into();
        assert_eq!(doc.text_for_save(), "a\nb");
    }

    /// The phantom-dirty fix: LF pasted into a CRLF file (same newline
    /// count) normalizes back to the file's own styles — clean, not dirty.
    #[test]
    fn newline_only_edits_stay_clean() {
        let mut doc = OpenDoc::new("a", &file("text", Some("a\r\nb\r\nc"), false));
        // Simulate a paste that replaced one CRLF with LF (count preserved).
        doc.code.edit.text = "a\r\nb\nc".into();
        assert_eq!(doc.text_for_save(), "a\r\nb\r\nc");
        assert!(!doc.dirty(), "newline-style-only edit must not be dirty");
        // A real content change still dirties.
        doc.code.edit.text = "a\r\nb\nX".into();
        assert!(doc.dirty());
        // Removing a line changes the count → first original style applies.
        doc.code.edit.text = "a\nb".into();
        assert_eq!(doc.text_for_save(), "a\r\nb");
        assert!(doc.dirty());
    }

    /// `mark_saved` baseline + payload dirty: after a CRLF save the disk
    /// text (CRLF) becomes `original` while the buffer may hold LF — the
    /// payload still compares equal, so no phantom dirty remains.
    #[test]
    fn saved_crlf_doc_stays_clean_with_lf_buffer() {
        let mut doc = OpenDoc::new("a", &file("text", Some("a\r\nb"), false));
        doc.code.edit.text = "a\nb".into(); // user buffer normalized to LF
        let saved = file("text", Some("a\r\nb"), false);
        doc.mark_saved(&saved);
        assert!(!doc.dirty(), "post-save CRLF doc must not stay dirty");
    }

    /// Discard is ONE undo step: buffer restores, undo brings the edit
    /// back, redo re-discards — history survives (replace_text used to
    /// reset it). Gutter/line count resync through `text_edited`.
    #[test]
    fn discard_is_single_undo_step_with_gutter_resync() {
        let mut doc = OpenDoc::new("a", &file("text", Some("one\ntwo\nthree\n"), false));
        assert_eq!(doc.code.line_count(), 4);
        doc.code.edit.set_caret(4);
        doc.code.edit.insert("EDIT\n");
        doc.code.text_edited();
        assert!(doc.dirty());
        assert_eq!(doc.code.line_count(), 5);

        doc.discard();
        assert!(!doc.dirty());
        assert_eq!(doc.code.edit.text, "one\ntwo\nthree\n");
        assert_eq!(doc.code.line_count(), 4, "gutter resynced to 4 lines");
        assert_eq!(doc.code.gutter_text().lines().count(), 4);

        doc.code.edit.undo();
        assert_eq!(doc.code.edit.text, "one\nEDIT\ntwo\nthree\n");
        assert!(doc.dirty(), "undo restores the pre-discard draft");
        doc.code.edit.redo();
        assert_eq!(doc.code.edit.text, "one\ntwo\nthree\n");
        assert!(!doc.dirty());
    }

    /// Discard on a CRLF original restores the exact bytes (insert does
    /// not sanitize programmatic text) and stays clean.
    #[test]
    fn discard_restores_crlf_bytes() {
        let mut doc = OpenDoc::new("a", &file("text", Some("a\r\nb\r\n"), false));
        doc.code.edit.insert("x");
        doc.discard();
        assert_eq!(doc.code.edit.text, "a\r\nb\r\n");
        assert!(!doc.dirty());
    }
}
