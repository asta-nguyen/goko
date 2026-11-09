//! Go-to-definition resolution — a Rust port of the desktop
//! `features/editor/go-to-definition.ts` at commit
//! `1fc1005a90f931f2cd2de9982e808b95542b6822`, kept pure: callers supply the
//! file list, config texts and content-search matches; nothing here touches
//! the filesystem, the network or UI state.
//!
//! Byte offsets are UTF-8 byte offsets everywhere. `token_at`'s `column` is a
//! byte offset into `line` and its `from`/`to` are byte offsets into `line`,
//! matching the desktop's column semantics (a column at a token's end still
//! counts as inside it).

use std::collections::HashSet;

/// A clickable token; `from`/`to` are byte columns in its line, `to` inclusive
/// (the specifier's closing quote, or one past a name's last byte).
#[derive(Clone, Debug, PartialEq)]
pub struct GoToToken {
    pub target: GoToTarget,
    pub from: usize,
    pub to: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GoToTarget {
    /// An import/require specifier, resolved as a module path.
    Import(String),
    /// A plain identifier, resolved by declaration search.
    Name(String),
}

/// A `tsconfig` `paths` entry with its targets already resolved to
/// checkout-relative templates.
#[derive(Clone, Debug, PartialEq)]
pub struct PathAlias {
    pub pattern: String,
    pub targets: Vec<String>,
}

/// One content-search hit, as the backend reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchMatch {
    pub path: String,
    /// Same units the backend uses (the desktop's 1-based line number).
    pub line: u32,
    pub text: String,
}

/// The declaration `pick_definition` chose.
#[derive(Clone, Debug, PartialEq)]
pub struct Definition {
    pub path: String,
    pub line: u32,
}

// ---------------------------------------------------------------------------
// tokenAt
// ---------------------------------------------------------------------------

const KEYWORDS: &[&str] = &[
    "const",
    "let",
    "var",
    "function",
    "class",
    "interface",
    "type",
    "enum",
    "import",
    "export",
    "from",
    "return",
    "if",
    "else",
    "for",
    "while",
    "new",
    "this",
    "async",
    "await",
    "def",
    "fn",
    "pub",
    "use",
];

/// JS `\w` plus the identifier characters the word scanner accepts
/// (`\p{L}`, `\p{N}`, `_`, `$`).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// JS `\w` only (ASCII), for the declaration patterns' `[\w$]` boundaries.
fn is_js_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_js_word_or_dollar(c: char) -> bool {
    is_js_word(c) || c == '$'
}

/// `\b` before a keyword: start of text or a non-word char ahead of it.
fn boundary_before(text: &str, byte: usize) -> bool {
    byte == 0
        || text[..byte]
            .chars()
            .next_back()
            .is_none_or(|c| !is_js_word(c))
}

fn is_ws(c: char) -> bool {
    c.is_whitespace()
}

/// The `(from, to, specifier)` of one import match: `from`/`import`/`require`
/// followed by optional `(` and whitespace, a quote, the specifier and the
/// same quote. `to` is the closing quote's byte offset.
fn import_match_at(line: &str, kw_end: usize) -> Option<(usize, usize, String)> {
    let mut pos = kw_end;
    let skip_ws = |mut p: usize| {
        while let Some(c) = line[p..].chars().next() {
            if !is_ws(c) {
                break;
            }
            p += c.len_utf8();
        }
        p
    };
    pos = skip_ws(pos);
    if line[pos..].starts_with('(') {
        pos += 1;
        pos = skip_ws(pos);
    }
    let quote = line[pos..].chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    pos += quote.len_utf8();
    let spec_start = pos;
    let mut spec_end = None;
    for (i, c) in line[pos..].char_indices() {
        if c == quote {
            spec_end = Some(pos + i);
            break;
        }
    }
    let spec_end = spec_end?;
    if spec_end == spec_start {
        return None; // `.+?` needs at least one character
    }
    let specifier = line[spec_start..spec_end].to_string();
    Some((spec_start, spec_end, specifier))
}

/// Every `from|import|require '…'` specifier in `line`, in order.
fn import_matches(line: &str) -> Vec<(usize, usize, String)> {
    const KWS: &[&str] = &["from", "import", "require"];
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < line.len() {
        // Advance to the next possible keyword start on a char boundary.
        let rel = KWS
            .iter()
            .filter_map(|kw| line[pos..].find(*kw).map(|i| (pos + i, *kw)))
            .min_by_key(|&(i, _)| i);
        let Some((at, kw)) = rel else { break };
        if boundary_before(line, at)
            && let Some(m) = import_match_at(line, at + kw.len())
        {
            out.push(m);
        }
        pos = at + kw.len();
    }
    out
}

/// Runs of `[\p{L}\p{N}_$]+` in `line`: `(start, end, text)` with `end` exclusive.
fn words(line: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices() {
        if is_word_char(c) {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push((s, i, &line[s..i]));
        }
    }
    if let Some(s) = start {
        out.push((s, line.len(), &line[s..]));
    }
    out
}

/// Every go-to token in `line`, import specifiers first (they win over names
/// at the same column, as the desktop's `tokenAt` checks them first), then
/// identifier-like words that don't sit inside a specifier.
pub fn tokens_in_line(line: &str) -> Vec<GoToToken> {
    let imports: Vec<GoToToken> = import_matches(line)
        .into_iter()
        .map(|(from, to, specifier)| GoToToken {
            target: GoToTarget::Import(specifier),
            from,
            to,
        })
        .collect();
    let mut out = imports.clone();
    'words: for (from, to, name) in words(line) {
        for imp in &imports {
            if from >= imp.from && from <= imp.to {
                continue 'words;
            }
        }
        // JS `.length` counts UTF-16 units.
        if name.encode_utf16().count() < 2
            || name.chars().next().is_some_and(|c| c.is_ascii_digit())
            || KEYWORDS.contains(&name)
        {
            continue;
        }
        out.push(GoToToken {
            target: GoToTarget::Name(name.to_string()),
            from,
            to,
        });
    }
    out
}

/// The import specifier or name under `column` (a byte offset into `line`),
/// if it is worth a lookup.
pub fn token_at(line: &str, column: usize) -> Option<GoToToken> {
    tokens_in_line(line)
        .into_iter()
        .find(|t| column >= t.from && column <= t.to)
}

// ---------------------------------------------------------------------------
// resolveImport
// ---------------------------------------------------------------------------

const EXTENSIONS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".json"];

/// ESM sources import `./x.js` for what is `./x.ts` on disk.
fn js_to_ts(ext: &str) -> &'static [&'static str] {
    match ext {
        ".js" => &[".ts", ".tsx"],
        ".jsx" => &[".tsx"],
        ".mjs" => &[".mts"],
        ".cjs" => &[".cts"],
        _ => &[],
    }
}

fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// `base` + `relative`, normalised; `None` when `..` climbs out of the checkout.
fn join(base: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for segment in relative.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            if parts.is_empty() {
                return None;
            }
            parts.pop();
        } else {
            parts.push(segment);
        }
    }
    Some(parts.join("/"))
}

/// `/\.[^./]+$/` — the last dot-extension with no `/` or `.` inside it.
fn extension_of(base: &str) -> Option<&str> {
    let dot = base.rfind('.')?;
    let ext = &base[dot..];
    (ext.len() > 1 && !ext[1..].contains(['.', '/'])).then_some(ext)
}

fn first_existing(files: &HashSet<String>, base: &str) -> Option<String> {
    let swapped: Vec<String> = extension_of(base)
        .map(|ext| {
            js_to_ts(ext)
                .iter()
                .map(|e| format!("{}{e}", &base[..base.len() - ext.len()]))
                .collect()
        })
        .unwrap_or_default();
    let index = if base.is_empty() {
        "index".to_string()
    } else {
        format!("{base}/index")
    };
    let mut candidates: Vec<String> = Vec::with_capacity(2 + 2 * EXTENSIONS.len());
    candidates.push(base.to_string());
    candidates.extend(swapped);
    candidates.extend(EXTENSIONS.iter().map(|e| format!("{base}{e}")));
    candidates.extend(EXTENSIONS.iter().map(|e| format!("{index}{e}")));
    candidates.into_iter().find(|c| files.contains(c))
}

/// The checkout file a specifier names: relative paths, then `paths` aliases.
/// Packages resolve to `None`.
pub fn resolve_import(
    files: &HashSet<String>,
    from_path: &str,
    specifier: &str,
    aliases: &[PathAlias],
) -> Option<String> {
    if specifier == "."
        || specifier == ".."
        || specifier.starts_with("./")
        || specifier.starts_with("../")
    {
        let base = join(dirname(from_path), specifier)?;
        return first_existing(files, &base);
    }
    for alias in aliases {
        let star = alias.pattern.find('*');
        let wildcard = match star {
            None => {
                if specifier != alias.pattern {
                    continue;
                }
                ""
            }
            Some(star) => {
                let head = &alias.pattern[..star];
                let tail = &alias.pattern[star + 1..];
                if specifier.len() < head.len() + tail.len()
                    || !specifier.starts_with(head)
                    || !specifier.ends_with(tail)
                {
                    continue;
                }
                &specifier[head.len()..specifier.len() - tail.len()]
            }
        };
        for target in &alias.targets {
            // JS `replace('*', wildcard)` replaces the first `*` only.
            let substituted = match target.find('*') {
                Some(i) => format!("{}{}{}", &target[..i], wildcard, &target[i + 1..]),
                None => target.clone(),
            };
            if let Some(hit) = first_existing(files, &substituted) {
                return Some(hit);
            }
        }
    }
    None
}

/// `tsconfig.json`/`jsconfig.json` paths from the file's directory up to the
/// checkout root, nearest first.
pub fn config_candidates(from_path: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut dir = dirname(from_path);
    loop {
        for name in ["tsconfig.json", "jsconfig.json"] {
            found.push(if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}/{name}")
            });
        }
        if dir.is_empty() {
            return found;
        }
        dir = dirname(dir);
    }
}

// ---------------------------------------------------------------------------
// parseTsconfigPaths — JSONC-tolerant, minimal JSON reader
// ---------------------------------------------------------------------------

/// Removes `//` and `/* */` comments outside strings, then trailing commas —
/// the desktop's `stripJsonc`. The trailing-comma pass is deliberately not
/// string-aware, matching the JS regex it ports.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            _ => out.push(c),
        }
    }
    strip_trailing_commas(&out)
}

fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut keep = vec![true; chars.len()];
    for i in 0..chars.len() {
        if chars[i] != ',' {
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if j < chars.len() && matches!(chars[j], '}' | ']') {
            keep[i] = false;
        }
    }
    chars
        .iter()
        .zip(keep)
        .filter_map(|(&c, k)| k.then_some(c))
        .collect()
}

/// The subset of JSON `parse_tsconfig_paths` reads.
#[derive(Debug)]
enum Json {
    Null,
    Bool,
    Num,
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

struct JsonParser<'a> {
    text: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn ws(&mut self) {
        while self.pos < self.text.len() && self.text[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }
    fn expect(&mut self, b: u8) -> bool {
        self.ws();
        if self.text.get(self.pos) == Some(&b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.text.get(self.pos)? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string().map(Json::Str),
            b't' => self.lit("true", Json::Bool),
            b'f' => self.lit("false", Json::Bool),
            b'n' => self.lit("null", Json::Null),
            _ => self.number(),
        }
    }
    fn lit(&mut self, word: &str, v: Json) -> Option<Json> {
        if self.text[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Some(v)
        } else {
            None
        }
    }
    fn number(&mut self) -> Option<Json> {
        let start = self.pos;
        while self.pos < self.text.len()
            && matches!(
                self.text[self.pos],
                b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'
            )
        {
            self.pos += 1;
        }
        (self.pos > start).then_some(Json::Num)
    }
    fn string(&mut self) -> Option<String> {
        if self.text.get(self.pos) != Some(&b'"') {
            return None;
        }
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.text.len() {
            let b = self.text[self.pos];
            self.pos += 1;
            match b {
                b'"' => return Some(out),
                b'\\' => {
                    let e = *self.text.get(self.pos)?;
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self.text.get(self.pos..self.pos + 4)?;
                            let code =
                                u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
                            self.pos += 4;
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        _ => return None,
                    }
                }
                _ => {
                    // Collect the full UTF-8 sequence.
                    let len = utf8_len(b);
                    let end = self.pos - 1 + len;
                    let s = std::str::from_utf8(self.text.get(self.pos - 1..end)?).ok()?;
                    out.push_str(s);
                    self.pos = end;
                }
            }
        }
        None
    }
    fn array(&mut self) -> Option<Json> {
        self.pos += 1;
        let mut items = Vec::new();
        self.ws();
        if self.expect(b']') {
            return Some(Json::Arr(items));
        }
        loop {
            items.push(self.value()?);
            self.ws();
            if self.expect(b']') {
                return Some(Json::Arr(items));
            }
            if !self.expect(b',') {
                return None;
            }
        }
    }
    fn object(&mut self) -> Option<Json> {
        self.pos += 1;
        let mut entries = Vec::new();
        self.ws();
        if self.expect(b'}') {
            return Some(Json::Obj(entries));
        }
        loop {
            self.ws();
            let key = self.string()?;
            if !self.expect(b':') {
                return None;
            }
            let value = self.value()?;
            entries.push((key, value));
            self.ws();
            if self.expect(b'}') {
                return Some(Json::Obj(entries));
            }
            if !self.expect(b',') {
                return None;
            }
        }
    }
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

fn json_get<'a>(obj: &'a Json, key: &str) -> Option<&'a Json> {
    match obj {
        Json::Obj(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

/// `compilerOptions.paths` of one config (`extends` is not followed);
/// unreadable configs give no aliases.
pub fn parse_tsconfig_paths(text: &str, config_path: &str) -> Vec<PathAlias> {
    let stripped = strip_jsonc(text);
    let mut parser = JsonParser {
        text: stripped.as_bytes(),
        pos: 0,
    };
    let Some(root) = parser.value() else {
        return Vec::new();
    };
    let Some(options) = json_get(&root, "compilerOptions") else {
        return Vec::new();
    };
    let Json::Obj(_) = options else {
        return Vec::new();
    };
    let paths = json_get(options, "paths");
    let Some(Json::Obj(path_entries)) = paths else {
        return Vec::new();
    };
    let base_url = match json_get(options, "baseUrl") {
        Some(Json::Str(s)) => s.as_str(),
        _ => ".",
    };
    let Some(root_dir) = join(dirname(config_path), base_url) else {
        return Vec::new();
    };
    path_entries
        .iter()
        .filter_map(|(pattern, targets)| {
            let Json::Arr(items) = targets else {
                return None;
            };
            let resolved: Vec<String> = items
                .iter()
                .filter_map(|t| match t {
                    Json::Str(s) => join(&root_dir, s),
                    _ => None,
                })
                .collect();
            (!resolved.is_empty()).then(|| PathAlias {
                pattern: pattern.clone(),
                targets: resolved,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// pickDefinition
// ---------------------------------------------------------------------------

/// `name` with the declaration patterns' `(?<![\w$])`/`(?![\w$])` boundaries:
/// returns the byte end of the match when `text[pos..]` starts with `name`
/// and the next char is not `[\w$]`.
fn name_end_at(text: &str, pos: usize, name: &str) -> Option<usize> {
    let end = pos + name.len();
    if !text[pos..].starts_with(name) {
        return None;
    }
    match text[end..].chars().next() {
        Some(c) if is_js_word_or_dollar(c) => None,
        _ => Some(end),
    }
}

/// `\s+` then `name`: returns the position after `name` when found.
fn ws_then_name(text: &str, mut pos: usize, name: &str) -> Option<usize> {
    let mut saw = false;
    while let Some(c) = text[pos..].chars().next() {
        if !is_ws(c) {
            break;
        }
        saw = true;
        pos += c.len_utf8();
    }
    if !saw {
        return None;
    }
    name_end_at(text, pos, name)
}

const DECL_KEYWORDS: &[&str] = &[
    "function",
    "class",
    "interface",
    "type",
    "enum",
    "namespace",
    "struct",
    "trait",
    "fn",
    "func",
    "def",
    "mod",
    "module",
    "object",
    "const",
    "let",
    "var",
    "val",
    "static",
];

/// Pattern 1: `\b(?:kw|function\*?)\s+NAME` anywhere in the line.
fn declares_keyword(text: &str, name: &str) -> bool {
    for (i, _) in text.char_indices() {
        if !boundary_before(text, i) {
            continue;
        }
        for kw in DECL_KEYWORDS {
            let Some(after_kw) = text[i..].strip_prefix(kw).map(|_| i + kw.len()) else {
                continue;
            };
            // `function` also accepts a `*` (generators).
            let after_kw = if *kw == "function" && text[after_kw..].starts_with('*') {
                after_kw + 1
            } else {
                after_kw
            };
            if ws_then_name(text, after_kw, name).is_some() {
                return true;
            }
        }
    }
    false
}

/// Pattern 2: `\bfunc\s*\([^)]*\)\s*NAME` — a Go method (receiver or plain func
/// with a param list before the name only matches the receiver form; plain
/// `func name(` is caught by pattern 1).
fn declares_go_func(text: &str, name: &str) -> bool {
    for (i, _) in text.char_indices() {
        if !boundary_before(text, i) || !text[i..].starts_with("func") {
            continue;
        }
        let mut pos = i + 4;
        while text[pos..].chars().next().is_some_and(is_ws) {
            pos += text[pos..].chars().next().unwrap().len_utf8();
        }
        if !text[pos..].starts_with('(') {
            continue;
        }
        let Some(close) = text[pos..].find(')') else {
            continue;
        };
        pos += close + 1;
        while text[pos..].chars().next().is_some_and(is_ws) {
            pos += text[pos..].chars().next().unwrap().len_utf8();
        }
        if name_end_at(text, pos, name).is_some() {
            return true;
        }
    }
    false
}

const METHOD_MODS: &[&str] = &[
    "public",
    "private",
    "protected",
    "static",
    "async",
    "override",
    "abstract",
    "readonly",
    "get",
    "set",
];

/// `[^>]*` inside `<...>`.
fn scan_until_gt(text: &str, mut pos: usize) -> Option<usize> {
    while pos < text.len() {
        if text[pos..].starts_with('>') {
            return Some(pos + 1);
        }
        pos += text[pos..].chars().next()?.len_utf8();
    }
    None
}

/// `\([^)]*\)` at `pos`: returns the position after `)`.
fn scan_parens(text: &str, pos: usize) -> Option<usize> {
    if !text[pos..].starts_with('(') {
        return None;
    }
    let close = text[pos..].find(')')?;
    Some(pos + close + 1)
}

/// Pattern 3 (anchored): `^\s*(mods\s+)*NAME\s*(<…>)?\(…\)\s*(:\s*type)?\{\s*$`.
fn declares_method(text: &str, name: &str) -> bool {
    let mut pos = 0;
    while text[pos..].chars().next().is_some_and(is_ws) {
        pos += text[pos..].chars().next().unwrap().len_utf8();
    }
    loop {
        let mut advanced = false;
        for m in METHOD_MODS {
            if boundary_before(text, pos)
                && text[pos..].starts_with(m)
                && text[pos + m.len()..].chars().next().is_some_and(is_ws)
            {
                pos += m.len();
                while text[pos..].chars().next().is_some_and(is_ws) {
                    pos += text[pos..].chars().next().unwrap().len_utf8();
                }
                advanced = true;
                break;
            }
        }
        if !advanced {
            break;
        }
    }
    let Some(end) = name_end_at(text, pos, name) else {
        return false;
    };
    pos = end;
    while text[pos..].chars().next().is_some_and(is_ws) {
        pos += text[pos..].chars().next().unwrap().len_utf8();
    }
    if text[pos..].starts_with('<')
        && let Some(after) = scan_until_gt(text, pos + 1)
    {
        pos = after;
    }
    let Some(after_params) = scan_parens(text, pos) else {
        return false;
    };
    pos = after_params;
    while text[pos..].chars().next().is_some_and(is_ws) {
        pos += text[pos..].chars().next().unwrap().len_utf8();
    }
    if text[pos..].starts_with(':') {
        pos += 1;
        while text[pos..].chars().next().is_some_and(is_ws) {
            pos += text[pos..].chars().next().unwrap().len_utf8();
        }
        // `[^ {;=]+` — at least one char that isn't `{`, `;` or `=`.
        let mut saw = false;
        while pos < text.len() {
            let c = text[pos..].chars().next().unwrap();
            if c == '{' || c == ';' || c == '=' {
                break;
            }
            saw = true;
            pos += c.len_utf8();
        }
        if !saw {
            return false;
        }
    }
    if !text[pos..].starts_with('{') {
        return false;
    }
    pos += 1;
    while text[pos..].chars().next().is_some_and(is_ws) {
        pos += text[pos..].chars().next().unwrap().len_utf8();
    }
    pos == text.len()
}

/// `(?:async\s+)?` at `pos`: returns the position after it (or `pos`).
fn skip_async(text: &str, mut pos: usize) -> usize {
    if boundary_before(text, pos) && text[pos..].starts_with("async") {
        let after = pos + 5;
        if text[after..].chars().next().is_some_and(is_ws) {
            pos = after;
            while text[pos..].chars().next().is_some_and(is_ws) {
                pos += text[pos..].chars().next().unwrap().len_utf8();
            }
        }
    }
    pos
}

/// Patterns 4+5 share the `NAME\s*[:=]\s*` prefix: `sep_pos` is after the
/// separator's whitespace and `async` skipped.
fn name_sep_prefix(text: &str, name: &str) -> Option<usize> {
    for (i, _) in text.char_indices() {
        // The `id` lookbehind: not preceded by `[\w$]`.
        if i > 0
            && text[..i]
                .chars()
                .next_back()
                .is_some_and(is_js_word_or_dollar)
        {
            continue;
        }
        let Some(end) = name_end_at(text, i, name) else {
            continue;
        };
        let mut pos = end;
        while text[pos..].chars().next().is_some_and(is_ws) {
            pos += text[pos..].chars().next().unwrap().len_utf8();
        }
        if !matches!(text[pos..].chars().next(), Some(':') | Some('=')) {
            continue;
        }
        pos += 1;
        while text[pos..].chars().next().is_some_and(is_ws) {
            pos += text[pos..].chars().next().unwrap().len_utf8();
        }
        pos = skip_async(text, pos);
        return Some(pos);
    }
    None
}

/// Pattern 4: `NAME\s*[:=]\s*(async )?(\(…\)|[\w$]+)\s*(:\s*[^=]+)?=>`.
fn declares_arrow(text: &str, name: &str) -> bool {
    let Some(mut pos) = name_sep_prefix(text, name) else {
        return false;
    };
    if text[pos..].starts_with('(') {
        let Some(after) = scan_parens(text, pos) else {
            return false;
        };
        pos = after;
    } else {
        let start = pos;
        while pos < text.len() && text[pos..].chars().next().is_some_and(is_js_word_or_dollar) {
            pos += text[pos..].chars().next().unwrap().len_utf8();
        }
        if pos == start {
            return false;
        }
    }
    while text[pos..].chars().next().is_some_and(is_ws) {
        pos += text[pos..].chars().next().unwrap().len_utf8();
    }
    if text[pos..].starts_with(':') {
        pos += 1;
        while text[pos..].chars().next().is_some_and(is_ws) {
            pos += text[pos..].chars().next().unwrap().len_utf8();
        }
        // `[^=]+` — at least one char that isn't `=`.
        let mut saw = false;
        while pos < text.len() && !text[pos..].starts_with('=') {
            saw = true;
            pos += text[pos..].chars().next().unwrap().len_utf8();
        }
        if !saw {
            return false;
        }
    }
    text[pos..].starts_with("=>")
}

/// Pattern 5: `NAME\s*[:=]\s*(async )?function\b`.
fn declares_function_expr(text: &str, name: &str) -> bool {
    let Some(pos) = name_sep_prefix(text, name) else {
        return false;
    };
    if !text[pos..].starts_with("function") {
        return false;
    }
    let after = pos + 8;
    text[after..].chars().next().is_none_or(|c| !is_js_word(c))
}

/// Whether a source line declares `name`: JS/TS, Python, Rust, Go and similar
/// keyword declarations.
fn declares(name: &str) -> impl Fn(&str) -> bool + '_ {
    move |text| {
        declares_keyword(text, name)
            || declares_go_func(text, name)
            || declares_method(text, name)
            || declares_arrow(text, name)
            || declares_function_expr(text, name)
    }
}

/// The declaration to jump to among content-search matches for `name`: the
/// open file's own first, else the only one in the checkout. `None` (none, or
/// several) means "show the search".
pub fn pick_definition(
    matches: &[SearchMatch],
    name: &str,
    current_path: &str,
) -> Option<Definition> {
    let test = declares(name);
    let found: Vec<&SearchMatch> = matches.iter().filter(|m| test(&m.text)).collect();
    let hit = found
        .iter()
        .copied()
        .find(|m| m.path == current_path)
        .or_else(|| (found.len() == 1).then(|| found[0]));
    hit.map(|m| Definition {
        path: m.path.clone(),
        line: m.line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn aliases(list: &[(&str, &[&str])]) -> Vec<PathAlias> {
        list.iter()
            .map(|(p, ts)| PathAlias {
                pattern: p.to_string(),
                targets: ts.iter().map(|s| s.to_string()).collect(),
            })
            .collect()
    }

    fn m(path: &str, line: u32, text: &str) -> SearchMatch {
        SearchMatch {
            path: path.to_string(),
            line,
            text: text.to_string(),
        }
    }

    #[test]
    fn token_at_finds_specifiers_of_every_import_form() {
        let line = "import { x } from './util'";
        let at = line.find("util").unwrap();
        let hit = token_at(line, at).unwrap();
        assert_eq!(hit.target, GoToTarget::Import("./util".into()));
        assert_eq!(
            (hit.from, hit.to),
            (line.find("./util").unwrap(), line.len() - 1)
        );
        assert_eq!(
            token_at("const m = require(\"../a/b\")", 20)
                .unwrap()
                .target,
            GoToTarget::Import("../a/b".into())
        );
        assert_eq!(
            token_at("const m = await import('./lazy')", 28)
                .unwrap()
                .target,
            GoToTarget::Import("./lazy".into())
        );
        assert_eq!(
            token_at("import './side-effect'", 10).unwrap().target,
            GoToTarget::Import("./side-effect".into())
        );
        assert_eq!(
            token_at("export * from \"@/x\"", 17).unwrap().target,
            GoToTarget::Import("@/x".into())
        );
    }

    #[test]
    fn token_at_finds_names_including_word_end() {
        let line = "  const total = sumAll(items)";
        let at = line.find("sumAll").unwrap();
        assert_eq!(
            token_at(line, at + 2).unwrap().target,
            GoToTarget::Name("sumAll".into())
        );
        assert_eq!(
            token_at(line, at + 6).unwrap().target,
            GoToTarget::Name("sumAll".into())
        );
        let hit = token_at(line, at).unwrap();
        assert_eq!((hit.from, hit.to), (at, at + 6));
        assert_eq!(
            token_at("$store.get_value()", 3).unwrap().target,
            GoToTarget::Name("$store".into())
        );
    }

    #[test]
    fn token_at_names_inside_import_lines_outside_strings() {
        let line = "import { Button } from './Button'";
        assert_eq!(
            token_at(line, 11).unwrap().target,
            GoToTarget::Name("Button".into())
        );
    }

    #[test]
    fn token_at_ignores_short_numbers_keywords_and_blanks() {
        assert!(token_at("a + b", 0).is_none());
        assert!(token_at("x = 42", 5).is_none());
        assert!(token_at("const x = 1", 2).is_none());
        assert!(token_at("foo   bar", 5).is_none());
        assert!(token_at("", 0).is_none());
    }

    #[test]
    fn token_at_keeps_accented_identifiers() {
        assert_eq!(
            token_at("const tiếng = 1", 8).unwrap().target,
            GoToTarget::Name("tiếng".into())
        );
    }

    #[test]
    fn resolve_import_relative_paths() {
        let f = files(&[
            "src/app.ts",
            "src/util.ts",
            "src/lib/index.ts",
            "src/components/Button.tsx",
            "src/data.json",
            "src/styles.css",
            "apps/desktop/src/App.tsx",
            "apps/desktop/src/stores/ui.ts",
        ]);
        assert_eq!(
            resolve_import(&f, "src/app.ts", "./util", &[]).as_deref(),
            Some("src/util.ts")
        );
        assert_eq!(
            resolve_import(&f, "src/app.ts", "./lib", &[]).as_deref(),
            Some("src/lib/index.ts")
        );
        assert_eq!(
            resolve_import(&f, "src/lib/index.ts", "../components/Button", &[]).as_deref(),
            Some("src/components/Button.tsx")
        );
        assert_eq!(
            resolve_import(&f, "src/app.ts", "./data.json", &[]).as_deref(),
            Some("src/data.json")
        );
        assert_eq!(
            resolve_import(&f, "src/app.ts", "./styles.css", &[]).as_deref(),
            Some("src/styles.css")
        );
        assert_eq!(resolve_import(&f, "src/app.ts", ".", &[]), None);
    }

    #[test]
    fn resolve_import_js_to_ts() {
        let f = files(&["src/util.ts", "src/components/Button.tsx"]);
        assert_eq!(
            resolve_import(&f, "src/app.ts", "./util.js", &[]).as_deref(),
            Some("src/util.ts")
        );
        assert_eq!(
            resolve_import(&f, "src/app.ts", "./components/Button.jsx", &[]).as_deref(),
            Some("src/components/Button.tsx")
        );
        assert_eq!(
            resolve_import(&f, "src/app.ts", "./components/Card.js", &[]),
            None
        );
    }

    #[test]
    fn resolve_import_packages_missing_and_escapes() {
        let f = files(&["src/util.ts"]);
        assert_eq!(resolve_import(&f, "src/app.ts", "react", &[]), None);
        assert_eq!(resolve_import(&f, "src/app.ts", "./missing", &[]), None);
        assert_eq!(resolve_import(&f, "src/app.ts", "../../outside", &[]), None);
    }

    #[test]
    fn resolve_import_aliases() {
        let f = files(&["apps/desktop/src/stores/ui.ts", "src/util.ts"]);
        let al = aliases(&[
            ("@/*", &["apps/desktop/src/*"]),
            ("~util", &["nowhere/x", "src/util"]),
        ]);
        assert_eq!(
            resolve_import(&f, "apps/desktop/src/App.tsx", "@/stores/ui", &al).as_deref(),
            Some("apps/desktop/src/stores/ui.ts")
        );
        assert_eq!(
            resolve_import(&f, "apps/desktop/src/App.tsx", "~util", &al).as_deref(),
            Some("src/util.ts")
        );
        assert_eq!(
            resolve_import(&f, "apps/desktop/src/App.tsx", "~util/extra", &al),
            None
        );
        assert_eq!(
            resolve_import(&f, "apps/desktop/src/App.tsx", "@/nope", &al),
            None
        );
        assert_eq!(
            resolve_import(&f, "apps/desktop/src/App.tsx", "@/stores/ui", &[]),
            None
        );
    }

    #[test]
    fn config_candidates_nearest_first() {
        assert_eq!(
            config_candidates("apps/desktop/src/App.tsx"),
            [
                "apps/desktop/src/tsconfig.json",
                "apps/desktop/src/jsconfig.json",
                "apps/desktop/tsconfig.json",
                "apps/desktop/jsconfig.json",
                "apps/tsconfig.json",
                "apps/jsconfig.json",
                "tsconfig.json",
                "jsconfig.json",
            ]
        );
        assert_eq!(
            config_candidates("index.ts"),
            ["tsconfig.json", "jsconfig.json"]
        );
    }

    #[test]
    fn parse_tsconfig_jsonc_comments_and_commas() {
        let text = "{\n  // aliases\n  \"compilerOptions\": {\n    \"strict\": true, /* inline */\n    \"paths\": { \"@/*\": [\"./src/*\"], \"url\": [\"http://x//y\"], },\n  },\n}";
        assert_eq!(
            parse_tsconfig_paths(text, "apps/desktop/tsconfig.json"),
            aliases(&[
                ("@/*", &["apps/desktop/src/*"]),
                ("url", &["apps/desktop/http:/x/y"])
            ])
        );
    }

    #[test]
    fn parse_tsconfig_baseurl_and_malformed() {
        assert_eq!(
            parse_tsconfig_paths(
                "{\"compilerOptions\":{\"baseUrl\":\"src\",\"paths\":{\"lib/*\":[\"lib/*\"],\"bad\":\"x\",\"empty\":[]}}}",
                "tsconfig.json"
            ),
            aliases(&[("lib/*", &["src/lib/*"])])
        );
        assert!(parse_tsconfig_paths("{\"compilerOptions\":{}}", "tsconfig.json").is_empty());
        assert!(parse_tsconfig_paths("not json", "tsconfig.json").is_empty());
        assert!(
            parse_tsconfig_paths(
                "{\"compilerOptions\":{\"paths\":{\"@/*\":[\"../../x/*\"]}}}",
                "tsconfig.json"
            )
            .is_empty()
        );
    }

    #[test]
    fn pick_definition_recognises_declarations() {
        let declarations = [
            "export function sumAll(items: number[]) {",
            "export default async function sumAll() {",
            "function* sumAll() {",
            "const sumAll = (items) => items.length",
            "export const sumAll: Fn = async (a, b) => {",
            "sumAll: (a) => a,",
            "class sumAll extends Base {",
            "export interface sumAll {",
            "type sumAll = string",
            "  async sumAll(items: number[]): Promise<void> {",
            "  private static sumAll() {",
            "def sumAll(items):",
            "pub fn sumAll<T>(items: &[T]) -> usize {",
            "func sumAll(items []int) int {",
            "func (s *Store) sumAll(items []int) int {",
            "sumAll = function () {",
            "let sumAll",
        ];
        for text in declarations {
            let got = pick_definition(&[m("a.ts", 3, text)], "sumAll", "other.ts");
            assert_eq!(
                got,
                Some(Definition {
                    path: "a.ts".into(),
                    line: 3
                }),
                "{text}"
            );
        }
    }

    #[test]
    fn pick_definition_rejects_uses_and_substrings() {
        let uses = [
            "const total = sumAll(items)",
            "  sumAll(items)",
            "return sumAll",
            "import { sumAll } from \"./a\"",
            "function sumAllItems() {",
            "function mysumAll() {",
            "const $sumAll = 1",
        ];
        for text in uses {
            assert_eq!(
                pick_definition(&[m("a.ts", 3, text)], "sumAll", "other.ts"),
                None,
                "{text}"
            );
        }
    }

    #[test]
    fn pick_definition_escapes_name_characters() {
        assert_eq!(
            pick_definition(&[m("a.js", 1, "const $store = 1")], "$store", "a.js"),
            Some(Definition {
                path: "a.js".into(),
                line: 1
            })
        );
        assert_eq!(
            pick_definition(&[m("a.js", 1, "const xstore = 1")], "$store", "a.js"),
            None
        );
    }

    #[test]
    fn pick_definition_prefers_local_then_unique() {
        let elsewhere = m("b.ts", 9, "function run() {");
        let local = m("a.ts", 4, "function run() {");
        let local_later = m("a.ts", 20, "const run = () => 1");
        assert_eq!(
            pick_definition(
                &[elsewhere.clone(), local.clone(), local_later],
                "run",
                "a.ts"
            ),
            Some(Definition {
                path: "a.ts".into(),
                line: 4
            })
        );
        assert_eq!(
            pick_definition(&[m("c.ts", 1, "run()"), elsewhere.clone()], "run", "a.ts"),
            Some(Definition {
                path: "b.ts".into(),
                line: 9
            })
        );
        assert_eq!(
            pick_definition(
                &[elsewhere.clone(), m("c.ts", 2, "class run {")],
                "run",
                "a.ts"
            ),
            None
        );
        assert_eq!(
            pick_definition(&[m("c.ts", 1, "run()")], "run", "a.ts"),
            None
        );
        assert_eq!(pick_definition(&[], "run", "a.ts"), None);
    }
}
