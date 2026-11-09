//! Syntax highlighting for the native code surface, on rune-kit's built-in
//! scanners. Highlighting is a pure, `Send`-safe result the parent can compute
//! on a worker thread and drop when its revision is stale; the UI converts the
//! owned ranges to [`rune::Span`] decorations at render time.
//!
//! The scanner budget is rune-kit's own: only the first [`MAX_SOURCE_BYTES`]
//! of a source — cut back to a line end — and its first [`MAX_RANGES`]
//! coloured ranges get colour, scanned as ONE prefix so multiline language
//! state (block comments, strings) is never split into independent chunks.
//! The rest of the document stays plain and fully editable.

use std::ops::Range;

use rune_kit::syntax::{self, Highlight};

/// Mirrors `rune_kit::syntax`'s scanner budget: bytes of source coloured.
pub const MAX_SOURCE_BYTES: usize = 32 * 1024;
/// Mirrors `rune_kit::syntax`'s scanner budget: coloured ranges kept.
pub const MAX_RANGES: usize = 8192;

/// Filename → scanner language for the extensions the built-in scanners know.
/// Anything else is plain text.
pub fn language_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    language_for_ext(&ext)
}

fn language_for_ext(ext: &str) -> Option<&'static str> {
    // Every name here is one `rune_kit::syntax::known` accepts.
    Some(match ext {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "js",
        "jsx" => "jsx",
        "ts" | "mts" | "cts" => "ts",
        "tsx" => "tsx",
        "json" | "jsonc" | "json5" => "json",
        "py" | "pyi" | "py3" => "python",
        "go" => "go",
        "swift" => "swift",
        "sh" | "bash" | "zsh" => "sh",
        "sql" => "sql",
        "html" | "htm" | "xhtml" | "xml" | "svg" => "html",
        "css" => "css",
        "yaml" | "yml" => "yaml",
        "diff" | "patch" => "diff",
        _ => return None,
    })
}

/// How much of `text` the scanners colour: all of it, or the bytes up to the
/// last line end within [`MAX_SOURCE_BYTES`] (0 when the first 32 KiB has no
/// line end — the file then draws plain). Always a char boundary.
pub fn coloured_len(text: &str) -> usize {
    if text.len() <= MAX_SOURCE_BYTES {
        return text.len();
    }
    text.as_bytes()[..MAX_SOURCE_BYTES]
        .iter()
        .rposition(|&b| b == b'\n')
        .unwrap_or(0)
}

/// One highlight job's output: owned, `Send`-safe ranges (no `Rc`, no UI
/// types) stamped with the document `revision` they were computed for. Apply
/// decorations only while `result.revision == code.revision`.
#[derive(Clone, Debug)]
pub struct HighlightResult {
    /// The document revision this result describes.
    pub revision: u64,
    /// The scanner language used, when the path named a known one.
    pub language: Option<String>,
    /// The source exceeded [`MAX_SOURCE_BYTES`]: `ranges` cover only the
    /// coloured prefix (cut at a line end); the tail renders plain.
    pub capped: bool,
    /// The range budget truncated colouring inside the prefix.
    pub token_capped: bool,
    /// `(byte range, paint)` pairs in document order — `rune_kit`'s
    /// [`Highlight`] alias, owned so it can cross a worker channel.
    pub ranges: Vec<Highlight>,
}

/// Highlight `text` for `language` (a [`language_for_path`] name, or `None`
/// for plain text) at document `revision`. Pure aside from rune-kit's
/// thread-local scan cache; safe to call on a worker thread.
pub fn highlight_revision(revision: u64, language: Option<&str>, text: &str) -> HighlightResult {
    let language = language.map(str::to_string);
    let capped = text.len() > MAX_SOURCE_BYTES;
    let prefix = coloured_len(text);
    let ranges: Vec<Highlight> = syntax::highlight(language.as_deref(), &text[..prefix]).to_vec();
    let token_capped = ranges.len() >= MAX_RANGES;
    HighlightResult {
        revision,
        language,
        capped,
        token_capped,
        ranges,
    }
}

// ---------------------------------------------------------------------------
// Lexical trivia for go-to filtering
// ---------------------------------------------------------------------------

/// What a trivia range is. Comments suppress every go-to token; strings
/// suppress names only — an import specifier *is* a string literal, so it
/// must stay clickable (the desktop never trivia-filters import targets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trivia {
    Comment,
    String,
}

#[derive(Clone, Copy, PartialEq)]
enum Sq {
    None,
    /// `'…'` ends at the closing quote or the line end.
    Line,
    /// `'…'` may span lines (shell).
    Multi,
    /// Rust/Go `'x'` char and rune literals only — `'a` lifetimes are code.
    Char,
}

#[derive(Clone, Copy, PartialEq)]
enum Bt {
    None,
    /// `` `…` `` raw string; `escapes` is false for Go (no `\` escapes).
    Multi {
        escapes: bool,
    },
    /// JS `` `…${expr}…` `` — `${}` segments scan as code.
    Template,
}

/// Per-language lexical rules, kept small: enough to keep go-to links out of
/// comments and strings document-wide. A missed edge costs a stray link,
/// never a wrong navigation.
#[derive(Clone, Copy)]
struct Dialect {
    /// Line-comment openers (`//`, `#`, `--`), checked in order.
    line: &'static [&'static str],
    /// `#` opens a comment only at a word start (shell, YAML): `a#b` is code.
    hash_ws: bool,
    /// `/* … */` comments exist.
    block: bool,
    /// `/*` nests inside `/* … */` (Rust, Swift).
    nested_block: bool,
    /// `<!-- … -->` markup comments (HTML/XML).
    html: bool,
    squote: Sq,
    /// `"` opens a string; `dquote_multi` allows raw newlines inside.
    dquote: bool,
    dquote_multi: bool,
    backtick: Bt,
    /// Triple-quoted strings: bit 1 = `'''`, bit 2 = `"""` (Python both,
    /// Swift only `"""`).
    triple: u8,
    /// Rust raw strings `r"…"` / `r#"…"#` / `br#"…"#`.
    raw_hash: bool,
    /// `''`/`""` doubled quote is an escape, not the end (SQL).
    sq_double: bool,
}

const JS: Dialect = Dialect {
    line: &["//"],
    hash_ws: false,
    block: true,
    nested_block: false,
    html: false,
    squote: Sq::Line,
    dquote: true,
    dquote_multi: false,
    backtick: Bt::Template,
    triple: 0,
    raw_hash: false,
    sq_double: false,
};
const RUST: Dialect = Dialect {
    line: &["//"],
    hash_ws: false,
    block: true,
    nested_block: true,
    html: false,
    squote: Sq::Char,
    dquote: true,
    dquote_multi: true,
    backtick: Bt::None,
    triple: 0,
    raw_hash: true,
    sq_double: false,
};
const GO: Dialect = Dialect {
    line: &["//"],
    hash_ws: false,
    block: true,
    nested_block: false,
    html: false,
    squote: Sq::Char,
    dquote: true,
    dquote_multi: false,
    backtick: Bt::Multi { escapes: false },
    triple: 0,
    raw_hash: false,
    sq_double: false,
};
const SWIFT: Dialect = Dialect {
    line: &["//"],
    hash_ws: false,
    block: true,
    nested_block: true,
    html: false,
    squote: Sq::None,
    dquote: true,
    dquote_multi: false,
    backtick: Bt::None,
    triple: 2,
    raw_hash: false,
    sq_double: false,
};
const PYTHON: Dialect = Dialect {
    line: &["#"],
    hash_ws: false,
    block: false,
    nested_block: false,
    html: false,
    squote: Sq::Line,
    dquote: true,
    dquote_multi: false,
    backtick: Bt::None,
    triple: 3,
    raw_hash: false,
    sq_double: false,
};
const SHELL: Dialect = Dialect {
    line: &["#"],
    hash_ws: true,
    block: false,
    nested_block: false,
    html: false,
    squote: Sq::Multi,
    dquote: true,
    dquote_multi: true,
    backtick: Bt::Multi { escapes: true },
    triple: 0,
    raw_hash: false,
    sq_double: false,
};
const SQL: Dialect = Dialect {
    line: &["--"],
    hash_ws: false,
    block: true,
    nested_block: false,
    html: false,
    squote: Sq::Multi,
    dquote: true,
    dquote_multi: false,
    backtick: Bt::None,
    triple: 0,
    raw_hash: false,
    sq_double: true,
};
const HTML: Dialect = Dialect {
    line: &[],
    hash_ws: false,
    block: false,
    nested_block: false,
    html: true,
    squote: Sq::Line,
    dquote: true,
    dquote_multi: false,
    backtick: Bt::None,
    triple: 0,
    raw_hash: false,
    sq_double: false,
};
const CSS: Dialect = Dialect {
    line: &[],
    hash_ws: false,
    block: true,
    nested_block: false,
    html: false,
    squote: Sq::Line,
    dquote: true,
    dquote_multi: false,
    backtick: Bt::None,
    triple: 0,
    raw_hash: false,
    sq_double: false,
};
const YAML: Dialect = Dialect {
    line: &["#"],
    hash_ws: true,
    block: false,
    nested_block: false,
    html: false,
    squote: Sq::Line,
    dquote: true,
    dquote_multi: true,
    backtick: Bt::None,
    triple: 0,
    raw_hash: false,
    sq_double: false,
};

impl Dialect {
    fn for_name(lang: &str) -> Option<Dialect> {
        Some(match lang {
            "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" | "json" | "jsonc"
            | "json5" | "javascript" | "typescript" => JS,
            "rust" | "rs" => RUST,
            "go" | "golang" => GO,
            "swift" => SWIFT,
            "python" | "py" | "pyi" | "py3" | "python3" => PYTHON,
            "sh" | "bash" | "zsh" | "shell" | "shellscript" => SHELL,
            "sql" | "mysql" | "postgres" | "postgresql" | "sqlite" => SQL,
            "html" | "htm" | "xhtml" | "xml" | "svg" => HTML,
            "css" => CSS,
            "yaml" | "yml" => YAML,
            _ => return None,
        })
    }
}

/// One open construct on the scanner stack.
#[derive(Clone, Copy)]
enum Mode {
    /// Inside `${ … }` of a JS template; a `}` at depth 0 returns to it.
    Expr { depth: u32 },
    /// `//`/`#`/`--` to end of line; `html` waits for `-->` instead.
    Line { start: usize, html: bool },
    /// `/* … */`; `depth` counts opens when the dialect nests.
    Block { start: usize, depth: u32 },
    /// Inside a quoted string or template.
    Str {
        /// Byte where the current trivia segment began (a `${}` splits a
        /// template into segments so expression code stays linkable).
        start: usize,
        quote: u8,
        /// Raw newlines allowed inside.
        multi: bool,
        /// `\` escapes the next byte.
        escapes: bool,
        /// Rust raw string: the `#` count whose `"` + `#`*n terminates it.
        raw: Option<usize>,
        /// JS template: `${` pushes [`Mode::Expr`].
        template: bool,
        /// `'''`/`"""` triple-quoted terminator.
        triple: bool,
        /// SQL-style doubled-quote escape.
        sq_double: bool,
    },
}

/// Comment and string-literal byte ranges of `text`, scanned whole — unlike
/// the colour budget there is no size cap, so go-to exclusion works past the
/// 32 KiB highlight prefix and while a syntax job is pending or stale.
/// Language comes from [`language_for_path`]; unknown languages get no trivia
/// (plain text links everything, as the desktop's empty syntax tree does).
pub fn trivia_ranges(text: &str, language: Option<&str>) -> Vec<(Range<usize>, Trivia)> {
    let Some(d) = language.and_then(Dialect::for_name) else {
        return Vec::new();
    };
    let mut lx = Lex {
        text,
        b: text.as_bytes(),
        d,
        modes: Vec::new(),
        out: Vec::new(),
        i: 0,
    };
    while lx.i < lx.b.len() {
        lx.step();
    }
    lx.finish();
    lx.out
}

struct Lex<'a> {
    text: &'a str,
    b: &'a [u8],
    d: Dialect,
    modes: Vec<Mode>,
    out: Vec<(Range<usize>, Trivia)>,
    i: usize,
}

impl Lex<'_> {
    fn step(&mut self) {
        match self.modes.last().copied() {
            None => self.code(false),
            Some(Mode::Expr { .. }) => self.code(true),
            Some(Mode::Line { .. }) => self.line(),
            Some(Mode::Block { .. }) => self.block(),
            Some(Mode::Str { .. }) => self.string(),
        }
    }

    fn emit(&mut self, start: usize, end: usize, kind: Trivia) {
        if end > start {
            self.out.push((start..end, kind));
        }
    }

    /// Ordinary code (or a template `${}` expression when `expr`).
    fn code(&mut self, expr: bool) {
        let i = self.i;
        let c = self.b[i];
        if expr {
            match c {
                b'{' => {
                    if let Some(Mode::Expr { depth }) = self.modes.last_mut() {
                        *depth += 1;
                    }
                    self.i += 1;
                    return;
                }
                b'}' => {
                    let at_zero = matches!(self.modes.last(), Some(Mode::Expr { depth: 0 }));
                    if at_zero {
                        self.modes.pop();
                        // Resume the template's string segment after `}`.
                        if let Some(Mode::Str {
                            start,
                            template: true,
                            ..
                        }) = self.modes.last_mut()
                        {
                            *start = i + 1;
                        }
                    } else if let Some(Mode::Expr { depth }) = self.modes.last_mut() {
                        *depth -= 1;
                    }
                    self.i += 1;
                    return;
                }
                _ => {}
            }
        }
        for &op in self.d.line {
            if !self.b[i..].starts_with(op.as_bytes()) {
                continue;
            }
            if op == "#" && self.d.hash_ws && !self.hash_ok(i) {
                continue;
            }
            self.modes.push(Mode::Line {
                start: i,
                html: false,
            });
            self.i += op.len();
            return;
        }
        if self.d.block && self.b[i..].starts_with(b"/*") {
            self.modes.push(Mode::Block { start: i, depth: 1 });
            self.i += 2;
            return;
        }
        if self.d.html && self.b[i..].starts_with(b"<!--") {
            self.modes.push(Mode::Line {
                start: i,
                html: true,
            });
            self.i += 4;
            return;
        }
        if self.try_string() {
            return;
        }
        self.i += 1;
    }

    /// `#` at a shell/YAML word start: line start, whitespace or an operator
    /// before it. `a#b` stays code.
    fn hash_ok(&self, i: usize) -> bool {
        if i == 0 {
            return true;
        }
        let p = self.b[i - 1];
        p.is_ascii_whitespace()
            || matches!(
                p,
                b';' | b'|' | b'&' | b'(' | b')' | b'{' | b'}' | b'<' | b'>'
            )
    }

    /// `r`/`br` not inside an identifier, `#`* then `"` — a Rust raw string.
    /// Pushes the mode and advances past the opening `"`.
    fn raw_string_start(&mut self) -> bool {
        let i = self.i;
        let b = self.b;
        let boundary = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        let after_r = if b[i] == b'r' && boundary {
            i + 1
        } else if b[i] == b'b' && b.get(i + 1) == Some(&b'r') && boundary {
            i + 2
        } else {
            return false;
        };
        let mut k = after_r;
        while b.get(k) == Some(&b'#') {
            k += 1;
        }
        if b.get(k) != Some(&b'"') {
            return false;
        }
        self.push_str(b'"', true, false, Some(k - after_r), false, false, false);
        self.i = k + 1;
        true
    }

    /// `'x'`/`'\n'` char-literal lookahead for Rust/Go `'`: one char (or an
    /// escape) between the quotes on the spot. `'a` lifetimes fail it.
    fn char_literal(&self, i: usize) -> bool {
        let mut cs = self.text[i + 1..].chars();
        matches!(
            (cs.next(), cs.next()),
            (Some('\\'), _) | (Some(_), Some('\''))
        )
    }

    /// Push a [`Mode::Str`] opening at byte `i` (the delimiter position).
    /// The seven flags map 1:1 onto [`Mode::Str`]'s fields; a spec struct
    /// would only rename the same parameters at every call site.
    #[allow(clippy::too_many_arguments)]
    fn push_str(
        &mut self,
        quote: u8,
        multi: bool,
        escapes: bool,
        raw: Option<usize>,
        template: bool,
        triple: bool,
        sq_double: bool,
    ) {
        self.modes.push(Mode::Str {
            start: self.i,
            quote,
            multi,
            escapes,
            raw,
            template,
            triple,
            sq_double,
        });
    }

    /// String/template openers at `self.i`; pushes the mode and advances past
    /// the delimiter. Returns whether one matched.
    fn try_string(&mut self) -> bool {
        let i = self.i;
        let c = self.b[i];
        let d = self.d;
        if d.triple != 0 {
            if c == b'\'' && d.triple & 1 != 0 && self.b[i..].starts_with(b"'''") {
                self.push_str(b'\'', true, true, None, false, true, d.sq_double);
                self.i += 3;
                return true;
            }
            if c == b'"' && d.triple & 2 != 0 && self.b[i..].starts_with(b"\"\"\"") {
                self.push_str(b'"', true, true, None, false, true, false);
                self.i += 3;
                return true;
            }
        }
        if d.dquote && c == b'"' {
            self.push_str(b'"', d.dquote_multi, true, None, false, false, false);
            self.i += 1;
            return true;
        }
        if c == b'\'' {
            match d.squote {
                Sq::Line => {
                    self.push_str(b'\'', false, true, None, false, false, d.sq_double);
                    self.i += 1;
                    return true;
                }
                Sq::Multi => {
                    self.push_str(b'\'', true, true, None, false, false, d.sq_double);
                    self.i += 1;
                    return true;
                }
                Sq::Char if self.char_literal(i) => {
                    self.push_str(b'\'', false, true, None, false, false, d.sq_double);
                    self.i += 1;
                    return true;
                }
                _ => {}
            }
        }
        if c == b'`' {
            match d.backtick {
                Bt::Multi { escapes } => {
                    self.push_str(b'`', true, escapes, None, false, false, false);
                    self.i += 1;
                    return true;
                }
                Bt::Template => {
                    self.push_str(b'`', true, true, None, true, false, false);
                    self.i += 1;
                    return true;
                }
                Bt::None => {}
            }
        }
        if d.raw_hash && self.raw_string_start() {
            return true;
        }
        false
    }

    /// Inside a `//`/`#`/`--` line comment or an `<!--` markup comment.
    fn line(&mut self) {
        let i = self.i;
        let html = matches!(self.modes.last(), Some(Mode::Line { html: true, .. }));
        let end = if html && self.b[i..].starts_with(b"-->") {
            Some(i + 3)
        } else if !html && self.b[i] == b'\n' {
            Some(i)
        } else {
            None
        };
        if let Some(end) = end {
            let start = match self.modes.last() {
                Some(Mode::Line { start, .. }) => *start,
                _ => i,
            };
            self.emit(start, end, Trivia::Comment);
            self.modes.pop();
            self.i = if html { end } else { i + 1 };
        } else {
            self.i += 1;
        }
    }

    /// Inside `/* … */`.
    fn block(&mut self) {
        let i = self.i;
        let (start, depth) = match self.modes.last() {
            Some(&Mode::Block { start, depth }) => (start, depth),
            _ => return,
        };
        if self.b[i..].starts_with(b"*/") {
            if depth == 1 {
                self.emit(start, i + 2, Trivia::Comment);
                self.modes.pop();
            } else if let Some(Mode::Block { depth, .. }) = self.modes.last_mut() {
                *depth -= 1;
            }
            self.i += 2;
        } else if self.d.nested_block && self.b[i..].starts_with(b"/*") {
            if let Some(Mode::Block { depth, .. }) = self.modes.last_mut() {
                *depth += 1;
            }
            self.i += 2;
        } else {
            self.i += 1;
        }
    }

    /// Inside a quoted string, template or triple-quoted string.
    fn string(&mut self) {
        let i = self.i;
        let Some(Mode::Str {
            start,
            quote,
            multi,
            escapes,
            raw,
            template,
            triple,
            sq_double,
        }) = self.modes.last().copied()
        else {
            return;
        };
        let c = self.b[i];
        if let Some(k) = raw {
            // Rust raw string: `"` followed by exactly the opening `#` count.
            if c == b'"'
                && self
                    .b
                    .get(i + 1..i + 1 + k)
                    .is_some_and(|s| s.iter().all(|&x| x == b'#'))
            {
                let end = i + 1 + k;
                self.emit(start, end, Trivia::String);
                self.modes.pop();
                self.i = end;
            } else {
                self.i += 1;
            }
            return;
        }
        if escapes && c == b'\\' {
            self.i = (i + 2).min(self.b.len());
            return;
        }
        if template && c == b'$' && self.b.get(i + 1) == Some(&b'{') {
            // `${` opens a code segment: close this string segment, scan the
            // expression as code; the `}` handler resumes the template after.
            self.emit(start, i, Trivia::String);
            if let Some(Mode::Str { start, .. }) = self.modes.last_mut() {
                *start = i + 2;
            }
            self.modes.push(Mode::Expr { depth: 0 });
            self.i += 2;
            return;
        }
        if triple {
            let t = [quote, quote, quote];
            if self.b[i..].starts_with(&t) {
                self.emit(start, i + 3, Trivia::String);
                self.modes.pop();
                self.i += 3;
            } else {
                self.i += 1;
            }
            return;
        }
        if sq_double && c == quote && self.b.get(i + 1) == Some(&quote) {
            self.i += 2;
            return;
        }
        if c == quote {
            self.emit(start, i + 1, Trivia::String);
            self.modes.pop();
            self.i += 1;
            return;
        }
        if c == b'\n' && !multi {
            // Unterminated single-line string: trivia ends before the newline.
            self.emit(start, i, Trivia::String);
            self.modes.pop();
            return;
        }
        self.i += 1;
    }

    /// EOF: everything still open runs to the end (an unterminated block
    /// comment or multiline string does comment out the rest of the file).
    fn finish(&mut self) {
        let n = self.b.len();
        while let Some(m) = self.modes.pop() {
            match m {
                Mode::Line { start, .. } | Mode::Block { start, .. } => {
                    self.emit(start, n, Trivia::Comment)
                }
                Mode::Str { start, .. } => self.emit(start, n, Trivia::String),
                Mode::Expr { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_kit::syntax::{Paint, Syntax};

    fn assert_send<T: Send>() {}

    #[test]
    fn result_is_send_safe() {
        assert_send::<HighlightResult>();
        assert_send::<Vec<Highlight>>();
    }

    #[test]
    fn language_for_path_maps_known_extensions() {
        assert_eq!(language_for_path("src/main.rs"), Some("rust"));
        assert_eq!(language_for_path("App.TSX"), Some("tsx"));
        assert_eq!(language_for_path("dir.with.dots/file.test.ts"), Some("ts"));
        assert_eq!(language_for_path(".eslintrc.json"), Some("json"));
        assert_eq!(language_for_path("Dockerfile"), None);
        assert_eq!(language_for_path("README"), None);
        assert_eq!(language_for_path("x.unknownext"), None);
        assert_eq!(language_for_path("x."), None);
    }

    #[test]
    fn coloured_len_respects_budget_and_line_ends() {
        assert_eq!(coloured_len("short"), 5);
        let small = "a".repeat(MAX_SOURCE_BYTES);
        assert_eq!(coloured_len(&small), MAX_SOURCE_BYTES);
        // Over budget: cut back to the last newline inside 32 KiB.
        let mut big = String::from("x");
        big.push_str(&"y".repeat(100));
        big.push('\n');
        big.push_str(&"z".repeat(MAX_SOURCE_BYTES));
        big.push('\n');
        big.push_str(&"tail".repeat(100));
        let n = coloured_len(&big);
        assert!(
            n <= MAX_SOURCE_BYTES && big.as_bytes()[n] == b'\n'
                || n == big.len().min(MAX_SOURCE_BYTES)
        );
        assert!(big.is_char_boundary(n));
        // No line end in the first 32 KiB → nothing is coloured.
        let one_line = "q".repeat(MAX_SOURCE_BYTES + 10);
        assert_eq!(coloured_len(&one_line), 0);
        // Multibyte text: the cut never lands inside a character.
        let emoji = "🎉".repeat(MAX_SOURCE_BYTES / 4 + 10);
        let n = coloured_len(&emoji);
        assert!(emoji.is_char_boundary(n));
    }

    #[test]
    fn highlight_covers_small_files() {
        let src = "fn main() { let s = \"hi\"; /* c */ }";
        let r = highlight_revision(7, Some("rust"), src);
        assert_eq!(r.revision, 7);
        assert_eq!(r.language.as_deref(), Some("rust"));
        assert!(!r.capped && !r.token_capped);
        assert!(!r.ranges.is_empty());
        assert!(
            r.ranges
                .iter()
                .all(|(range, _)| range.end <= src.len() && src.is_char_boundary(range.start))
        );
        // Comments and strings are trivia for go-to (the whole-document
        // lexical scanner the overlay actually uses).
        let trivia = trivia_ranges(src, r.language.as_deref());
        assert!(
            trivia
                .iter()
                .any(|(r, k)| *k == Trivia::Comment && &src[r.clone()] == "/* c */")
        );
        assert!(
            trivia
                .iter()
                .any(|(r, k)| *k == Trivia::String && &src[r.clone()] == "\"hi\"")
        );
    }

    #[test]
    fn highlight_reports_caps() {
        // Over the source budget → capped, ranges stay inside the prefix.
        let mut big = String::new();
        while big.len() < MAX_SOURCE_BYTES + 100 {
            big.push_str("let x = 1;\n");
        }
        let r = highlight_revision(1, Some("js"), &big);
        assert!(r.capped);
        let prefix = coloured_len(&big);
        assert!(r.ranges.iter().all(|(range, _)| range.end <= prefix));
        // Over the range budget → token_capped.
        let many = "1 ".repeat(MAX_RANGES + 100);
        let r = highlight_revision(2, Some("js"), &many);
        assert!(r.token_capped);
        assert_eq!(r.ranges.len(), MAX_RANGES);
    }

    #[test]
    fn highlight_unknown_language_is_plain() {
        let r = highlight_revision(3, None, "anything at all");
        assert!(r.ranges.is_empty());
        assert!(!r.capped);
        assert_eq!(r.language, None);
        let r = highlight_revision(3, Some("cobol"), "anything at all");
        assert!(
            r.ranges.is_empty(),
            "an unrecognised language scans to nothing"
        );
    }

    #[test]
    fn paints_cover_every_syntax_kind() {
        // Every Paint::Kind maps through SyntaxTheme::paint without panic.
        let theme = rune_kit::syntax::SyntaxTheme::neutral(true);
        for k in [
            Syntax::Keyword,
            Syntax::String,
            Syntax::Number,
            Syntax::Comment,
            Syntax::Type,
            Syntax::Function,
            Syntax::Property,
            Syntax::Punctuation,
            Syntax::Inserted,
            Syntax::Deleted,
            Syntax::Parameter,
        ] {
            let _ = theme.paint(Paint::Kind(k));
        }
    }

    // -- trivia_ranges: the go-to classifier -------------------------------

    fn trivia_text(text: &str, lang: &str) -> Vec<String> {
        trivia_ranges(text, Some(lang))
            .iter()
            .map(|(r, _)| text[r.clone()].to_string())
            .collect()
    }

    #[test]
    fn trivia_covers_line_block_and_nested_comments() {
        let src = "// one\n/* two\n still two */ /* outer /* inner */ tail */\ncode";
        let t = trivia_text(src, "rust");
        assert!(t.contains(&"// one".to_string()));
        assert!(t.iter().any(|s| s.contains("still two")), "{t:?}");
        // Nested block comments are one range in Rust.
        assert!(t.iter().any(|s| s.contains("inner */ tail")), "{t:?}");
        assert!(!t.iter().any(|s| s == "code"));
    }

    #[test]
    fn trivia_strings_span_lines_and_keep_template_code() {
        // Rust strings may span lines; JS templates split at ${} so the
        // expression inside stays code (and linkable).
        let src = "let s = \"a\nb\";\nlet t = `x${name}y`;";
        let t = trivia_text(src, "rust");
        assert!(t.contains(&"\"a\nb\"".to_string()), "{t:?}");
        let js = trivia_text("let t = `x${name}y`;", "js");
        assert_eq!(
            js,
            vec!["`x".to_string(), "y`".to_string()],
            "template segments only: {js:?}"
        );
    }

    #[test]
    fn trivia_rust_raw_strings_and_char_literals() {
        let src = "let a = r#\"raw # not end\"#;\nlet b = 'x';\nlet c = &'a str;";
        let t = trivia_text(src, "rust");
        assert!(
            t.iter()
                .any(|s| s.starts_with("r#\"") && s.ends_with("\"#")),
            "{t:?}"
        );
        assert!(t.contains(&"'x'".to_string()), "{t:?}");
        // 'a is a lifetime, not a string: nothing beyond 'x' and the raw
        // string may be trivia.
        assert_eq!(t.len(), 2, "{t:?}");
    }

    #[test]
    fn trivia_hash_comments_need_word_start() {
        // Shell/YAML `#` comments need whitespace or an operator before
        // them; `a#b` stays code. Python `#` is unconditional.
        let sh = trivia_text("echo a#b # real\necho done", "sh");
        assert_eq!(sh, vec!["# real".to_string()], "{sh:?}");
        let py = trivia_text("x = 1#note\ny = 2", "python");
        assert_eq!(py, vec!["#note".to_string()], "{py:?}");
    }

    #[test]
    fn trivia_unterminated_constructs_run_to_eof() {
        // An unterminated block comment or multiline string swallows the
        // rest — matching how the language lexer would treat it.
        let t = trivia_text("code /* never ends\nmore code", "rust");
        assert_eq!(t.len(), 1);
        assert!(t[0].contains("never ends"));
        let t = trivia_text("code \"never ends\nmore", "rust");
        assert_eq!(t.len(), 1);
        assert!(t[0].contains("never ends"));
        // But a single-line string stops at the newline.
        let t = trivia_text("\"oops\ncode", "js");
        assert_eq!(t, vec!["\"oops".to_string()], "{t:?}");
    }

    #[test]
    fn trivia_unknown_language_and_empty_text() {
        assert!(trivia_ranges("// x", None).is_empty());
        assert!(trivia_ranges("// x", Some("cobol")).is_empty());
        assert!(trivia_ranges("", Some("rust")).is_empty());
    }
}
