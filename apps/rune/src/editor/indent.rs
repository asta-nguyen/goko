//! Indentation model for the code surface: unit resolution (manual override,
//! content sniff, per-language default), line indent/outdent transforms for
//! `Tab`/`Shift+Tab`, `Enter` auto-indent, and indent-guide tick generation.
//!
//! Everything here is pure string/byte logic so the editing decisions stay
//! testable without a Rune runtime; `code.rs` applies the results to the
//! shared `TextEdit`.

/// One indentation level as inserted whitespace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndentUnit {
    /// `n` space characters per level.
    Spaces(u8),
    /// A literal tab; renders at the framework's fixed 4-column stops.
    Tab,
}

impl IndentUnit {
    /// Visual columns one level occupies. Tabs use the framework's fixed
    /// 4-column stop rule (`mono_tab_stops` in rune's text engine).
    pub fn cols(self) -> u32 {
        match self {
            IndentUnit::Spaces(n) => n as u32,
            IndentUnit::Tab => TAB_STOP,
        }
    }

    /// Short label for the status chip.
    pub fn label(self) -> String {
        match self {
            IndentUnit::Spaces(n) => format!("Spaces: {n}"),
            IndentUnit::Tab => "Tab".to_string(),
        }
    }
}

/// Column width of the framework's hardcoded tab stops.
pub const TAB_STOP: u32 = 4;

/// Deepest guide column drawn; deeper indentation simply shows no ticks.
pub const MAX_GUIDE_COL: u32 = 48;

/// Documents longer than this skip guide generation entirely — the tick
/// count would dominate rebuild cost for little readability gain.
pub const MAX_GUIDE_LINES: usize = 8000;

/// Sample size for content-sniffing the dominant indent unit.
pub const SNIFF_SAMPLE: usize = 100;

/// Settings-store key for the manual override (`auto`, a space count, `tab`).
pub const INDENT_KEY: &str = "goko.editor.indent";

/// Sniffing never scans past this many bytes — a pathological single-line
/// file must not make every keystroke walk the whole document.
const SNIFF_BYTES: usize = 256 * 1024;

/// The persisted override: `Auto` resolves per document, `Fixed` wins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnitChoice {
    Auto,
    Fixed(IndentUnit),
}

impl UnitChoice {
    /// Parse the stored setting value (`auto`, a space count, `tab`).
    pub fn parse(value: &str) -> UnitChoice {
        match value.trim() {
            "tab" => UnitChoice::Fixed(IndentUnit::Tab),
            "auto" => UnitChoice::Auto,
            other => match other.parse::<u8>() {
                Ok(n) if (1..=32).contains(&n) => UnitChoice::Fixed(IndentUnit::Spaces(n)),
                _ => UnitChoice::Auto,
            },
        }
    }

    /// Stored setting value.
    pub fn store_value(self) -> String {
        match self {
            UnitChoice::Auto => "auto".to_string(),
            UnitChoice::Fixed(IndentUnit::Spaces(n)) => n.to_string(),
            UnitChoice::Fixed(IndentUnit::Tab) => "tab".to_string(),
        }
    }

    /// Next choice in the chip's cycle: Auto → 2 → 4 → 8 → Tab → Auto;
    /// non-canonical widths cycle back to Auto.
    pub fn next(self) -> UnitChoice {
        match self {
            UnitChoice::Auto => UnitChoice::Fixed(IndentUnit::Spaces(2)),
            UnitChoice::Fixed(IndentUnit::Spaces(2)) => UnitChoice::Fixed(IndentUnit::Spaces(4)),
            UnitChoice::Fixed(IndentUnit::Spaces(4)) => UnitChoice::Fixed(IndentUnit::Spaces(8)),
            UnitChoice::Fixed(IndentUnit::Spaces(8)) => UnitChoice::Fixed(IndentUnit::Tab),
            _ => UnitChoice::Auto,
        }
    }
}

/// Whitespace string for `unit`.
pub fn indent_string(unit: IndentUnit) -> String {
    match unit {
        IndentUnit::Spaces(n) => " ".repeat(n as usize),
        IndentUnit::Tab => "\t".to_string(),
    }
}

/// The leading whitespace run of one line (no newline).
fn leading_ws(line: &str) -> &str {
    let end = line
        .char_indices()
        .find(|(_, c)| *c != ' ' && *c != '\t')
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    &line[..end]
}

/// Byte length of a line's leading whitespace.
pub fn leading_ws_len(line: &str) -> usize {
    leading_ws(line).len()
}

/// Detect the dominant indentation unit across up to `sample` indented lines,
/// VS Code-style: each *increase* in a line's indent over the previous
/// non-blank line votes for that step (2..=8 spaces; wider jumps are
/// continuation alignment), and each tab-depth increase votes for tabs. Ties
/// resolve to the narrower space width, then tabs. `None` when no line is
/// indented.
pub fn sniff_unit(text: &str, sample: usize) -> Option<IndentUnit> {
    // Cap the scanned prefix at a char boundary so huge/minified files stay
    // cheap; detection only needs the document's opening style anyway.
    let mut end = text.len().min(SNIFF_BYTES);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let text = &text[..end];
    let mut space_scores: std::collections::BTreeMap<u8, u32> = Default::default();
    let mut tabs = 0u32;
    let mut seen = 0usize;
    let mut prev = 0usize; // previous non-blank line's leading-ws length
    for line in text.lines() {
        if seen >= sample {
            break;
        }
        let ws = leading_ws(line);
        if ws.len() == line.len() {
            continue; // blank or whitespace-only lines carry no signal
        }
        if !ws.is_empty() {
            seen += 1;
            if ws.len() > prev {
                if ws.contains('\t') {
                    tabs += 1;
                } else if (2..=8).contains(&(ws.len() - prev)) {
                    *space_scores.entry((ws.len() - prev) as u8).or_default() += 1;
                }
            }
        }
        prev = ws.len();
    }
    // Best space width: highest score, narrowest width on ties.
    let best_space = space_scores
        .iter()
        .map(|(n, s)| (*n, *s))
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
    match (best_space, tabs) {
        (Some((_n, s)), t) if t > s => Some(IndentUnit::Tab),
        (Some((n, _)), _) => Some(IndentUnit::Spaces(n)),
        (None, t) if t > 0 => Some(IndentUnit::Tab),
        _ => None,
    }
}

/// Per-language fallback when content sniffing finds nothing. Extensions map
/// to the conventions the desktop's CodeMirror setup assumes.
pub fn language_default(path: &str) -> Option<IndentUnit> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "go" | "mk" => IndentUnit::Tab,
        "rs" | "py" | "java" | "kt" | "cs" | "c" | "h" | "cpp" | "hpp" | "scala" | "ex" | "exs"
        | "erl" | "hs" | "ml" | "swift" | "m" | "mm" | "php" | "dart" | "zig" | "nim" => {
            IndentUnit::Spaces(4)
        }
        "js" | "jsx" | "ts" | "tsx" | "json" | "jsonc" | "css" | "scss" | "less" | "html"
        | "vue" | "svelte" | "yaml" | "yml" | "toml" | "sql" | "sh" | "rb" | "pl" | "lua" | "r"
        | "jl" | "tf" | "proto" | "graphql" | "md" | "mdx" | "astro" | "ps1" | "bat" => {
            IndentUnit::Spaces(2)
        }
        _ => return None,
    })
}

/// Resolve the effective unit: fixed override wins, else content sniff, else
/// language default, else 4 spaces.
pub fn resolve(text: &str, path: &str, choice: UnitChoice) -> IndentUnit {
    if let UnitChoice::Fixed(unit) = choice {
        return unit;
    }
    sniff_unit(text, SNIFF_SAMPLE)
        .or_else(|| language_default(path))
        .unwrap_or(IndentUnit::Spaces(4))
}

/// A planned text replacement plus the selection to restore afterwards.
#[derive(Clone, Debug)]
pub struct IndentEdit {
    /// Byte range of `doc` the `text` replaces.
    pub start: usize,
    pub end: usize,
    /// Replacement text.
    pub text: String,
    /// Caret/selection anchor+focus to restore after applying, in post-edit
    /// byte offsets.
    pub anchor: usize,
    pub focus: usize,
}

/// Remove up to one indent level from a leading-whitespace run: strips the
/// unit's literal prefix when present, else removes up to `unit.cols()`
/// columns of whitespace (spaces and/or tab stops) from the front.
pub fn outdent_ws(ws: &str, unit: IndentUnit) -> &str {
    let unit_text = indent_string(unit);
    if ws.starts_with(unit_text.as_str()) {
        return &ws[unit_text.len()..];
    }
    let mut cols = 0u32;
    let mut end = 0usize;
    for (i, c) in ws.char_indices() {
        if cols >= unit.cols() {
            break;
        }
        match c {
            ' ' => cols += 1,
            '\t' => cols += TAB_STOP - cols % TAB_STOP,
            _ => break,
        }
        end = i + c.len_utf8();
    }
    &ws[end..]
}

/// Plan the `Tab`/`Shift+Tab` edit for `doc` at selection `(anchor, focus)`
/// (byte offsets, either order). Returns `None` when nothing would change.
///
/// - `Tab`, collapsed caret: insert the unit at the caret.
/// - `Tab`, selection: prepend the unit to every touched line's leading ws.
/// - `Shift+Tab`: outdent every touched line (a caret counts as its line).
pub fn plan_indent(
    doc: &str,
    sel_start: usize,
    sel_end: usize,
    unit: IndentUnit,
    outdent: bool,
) -> Option<IndentEdit> {
    let (a, b) = (sel_start.min(sel_end), sel_start.max(sel_end));
    let unit_str = indent_string(unit);

    if a == b && !outdent {
        let at = a.min(doc.len());
        let caret = at + unit_str.len();
        return Some(IndentEdit {
            start: at,
            end: at,
            text: unit_str,
            anchor: caret,
            focus: caret,
        });
    }

    // Byte span of whole lines touched by the selection. A selection ending
    // exactly at a line start does not touch that trailing line.
    let first_nl = doc[..a.min(doc.len())].rfind('\n').map_or(0, |i| i + 1);
    let mut region_last = b.min(doc.len());
    if b > a && region_last > first_nl && doc.as_bytes().get(region_last - 1) == Some(&b'\n') {
        region_last -= 1;
    }
    let region_end = doc[region_last..]
        .find('\n')
        .map_or(doc.len(), |i| region_last + i);
    let region = &doc[first_nl..region_end];

    // Rebuild each line's leading whitespace.
    let mut out = String::with_capacity(region.len() + 16);
    for line in region.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let ws_len = leading_ws_len(content);
        let ws = &content[..ws_len];
        if outdent {
            out.push_str(outdent_ws(ws, unit));
        } else {
            out.push_str(&unit_str);
            out.push_str(ws);
        }
        out.push_str(&content[ws_len..]);
        if line.ends_with('\n') {
            out.push('\n');
        }
    }

    if out == region {
        return None;
    }

    Some(IndentEdit {
        anchor: remap_offset(first_nl, region, &out, a),
        focus: remap_offset(first_nl, region, &out, b),
        start: first_nl,
        end: region_end,
        text: out,
    })
}

/// Map a byte offset inside the replaced region to its post-edit offset by
/// walking original and rebuilt regions line-by-line in parallel. Offsets
/// inside a line's leading whitespace clamp into the new whitespace run;
/// an offset on a line's newline lands at the end of the rebuilt content.
fn remap_offset(region_start: usize, old_region: &str, new_region: &str, offset: usize) -> usize {
    let rel = offset.saturating_sub(region_start);
    let mut old_pos = 0usize; // offset within old_region of the current line start
    let mut new_pos = 0usize; // offset within new_region
    for (old_line, new_line) in old_region
        .split_inclusive('\n')
        .zip(new_region.split_inclusive('\n'))
    {
        if rel < old_pos + old_line.len() {
            let old_content = old_line.strip_suffix('\n').unwrap_or(old_line);
            let new_content = new_line.strip_suffix('\n').unwrap_or(new_line);
            let old_ws = leading_ws_len(old_content);
            let new_ws = leading_ws_len(new_content);
            let in_line = rel - old_pos;
            let mapped = if in_line < old_ws {
                new_pos + in_line.min(new_ws)
            } else if in_line <= old_content.len() {
                new_pos + (in_line - old_ws) + new_ws
            } else {
                // On the '\n' itself: end of the rebuilt line's content.
                new_pos + new_content.len()
            };
            return region_start + mapped.min(new_region.len());
        }
        old_pos += old_line.len();
        new_pos += new_line.len();
    }
    region_start + new_region.len()
}

/// The text `Enter` should insert: a newline plus the current line's leading
/// whitespace, plus one indent unit when the line's last non-whitespace
/// character opens a block (`{`, `(`, `[`, `:`).
pub fn auto_indent(doc: &str, caret: usize, unit: IndentUnit) -> String {
    let pos = caret.min(doc.len());
    let line_start = doc[..pos].rfind('\n').map_or(0, |i| i + 1);
    let line_end = doc[pos..].find('\n').map_or(doc.len(), |i| pos + i);
    let line = &doc[line_start..line_end];
    let ws = leading_ws(line);
    let opens = line
        .trim_end()
        .chars()
        .next_back()
        .is_some_and(|c| matches!(c, '{' | '(' | '[' | ':'));
    let mut out = String::with_capacity(ws.len() + 8);
    out.push('\n');
    out.push_str(ws);
    if opens {
        out.push_str(&indent_string(unit));
    }
    out
}

/// Visual column a leading-whitespace run reaches: space = 1 column, tab =
/// next multiple of [`TAB_STOP`] — the same rule the framework renderer uses.
fn ws_visual_cols(leading: &str) -> u32 {
    let mut col = 0u32;
    for c in leading.chars() {
        match c {
            ' ' => col += 1,
            '\t' => col += TAB_STOP - col % TAB_STOP,
            _ => break,
        }
    }
    col
}

/// Per-line visual column of leading whitespace (`0` = none), one entry per
/// line, capped at [`MAX_GUIDE_LINES`] — callers skip generation beyond it.
pub fn indent_columns(text: &str) -> Vec<u32> {
    text.split_inclusive('\n')
        .take(MAX_GUIDE_LINES)
        .map(|line| {
            let content = line.strip_suffix('\n').unwrap_or(line);
            ws_visual_cols(&content[..leading_ws_len(content)])
        })
        .collect()
}

/// Merged vertical guide runs `(guide_col, first_line, last_line)` —
/// `last_line` inclusive. A guide sits at every multiple of `unit.cols()`
/// up to [`MAX_GUIDE_COL`] that a line's indent reaches (a guide at the
/// line's own indent column marks the level's content edge, like VS Code),
/// merged across contiguous lines so each run draws as one segment.
pub fn guide_runs(cols: &[u32], unit: IndentUnit) -> Vec<(u32, usize, usize)> {
    let step = unit.cols().max(1);
    let max_g = cols.iter().copied().max().unwrap_or(0).min(MAX_GUIDE_COL);
    let mut runs = Vec::new();
    let mut g = step;
    while g <= max_g {
        let mut start: Option<usize> = None;
        for (i, &c) in cols.iter().enumerate() {
            if c >= g {
                if start.is_none() {
                    start = Some(i);
                }
            } else if let Some(s) = start.take() {
                runs.push((g, s, i - 1));
            }
        }
        if let Some(s) = start {
            runs.push((g, s, cols.len() - 1));
        }
        g += step;
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(doc: &str, e: &IndentEdit) -> String {
        let mut out = String::with_capacity(doc.len() + e.text.len());
        out.push_str(&doc[..e.start]);
        out.push_str(&e.text);
        out.push_str(&doc[e.end..]);
        out
    }

    #[test]
    fn sniff_prefers_dominant_space_width() {
        let doc = "fn a() {\n    x();\n}\nfn b() {\n    y();\n}\nfn c() {\n  p();\n}\n";
        assert_eq!(sniff_unit(doc, 100), Some(IndentUnit::Spaces(4)));
        // An exact tie resolves to the narrower width.
        let tie = "a {\n    x\n}\nb {\n  y\n}\n";
        assert_eq!(sniff_unit(tie, 100), Some(IndentUnit::Spaces(2)));
    }

    #[test]
    fn sniff_uses_step_not_absolute_width() {
        // Nested 4-space code has more 8-wide lines than 4-wide ones.
        let doc = "impl A {\n    fn f() {\n        a();\n        b();\n        c();\n    }\n}\n";
        assert_eq!(sniff_unit(doc, 100), Some(IndentUnit::Spaces(4)));
    }

    #[test]
    fn sniff_detects_tabs() {
        let doc = "package main\n\nfunc f() {\n\tif x {\n\t\tg()\n\t}\n}\n";
        assert_eq!(sniff_unit(doc, 100), Some(IndentUnit::Tab));
    }

    #[test]
    fn sniff_ignores_blank_and_ws_only_lines() {
        let doc = "a\n   \n\t\n    b\n    c\n";
        assert_eq!(sniff_unit(doc, 100), Some(IndentUnit::Spaces(4)));
    }

    #[test]
    fn language_defaults() {
        assert_eq!(language_default("a/b.rs"), Some(IndentUnit::Spaces(4)));
        assert_eq!(language_default("a/b.go"), Some(IndentUnit::Tab));
        assert_eq!(language_default("a/b.ts"), Some(IndentUnit::Spaces(2)));
        assert_eq!(language_default("a/b.unknownext"), None);
    }

    #[test]
    fn resolve_order_override_sniff_language() {
        let doc = "x\n    y\n";
        assert_eq!(
            resolve(doc, "a.go", UnitChoice::Fixed(IndentUnit::Spaces(2))),
            IndentUnit::Spaces(2)
        );
        assert_eq!(
            resolve(doc, "a.go", UnitChoice::Auto),
            IndentUnit::Spaces(4)
        );
        assert_eq!(resolve("x\ny\n", "a.go", UnitChoice::Auto), IndentUnit::Tab);
        assert_eq!(
            resolve("x\ny\n", "a.zzz", UnitChoice::Auto),
            IndentUnit::Spaces(4)
        );
    }

    #[test]
    fn caret_tab_inserts_unit() {
        let doc = "ab\ncd";
        let e = plan_indent(doc, 1, 1, IndentUnit::Spaces(2), false).unwrap();
        assert_eq!((e.start, e.end, e.text.as_str()), (1, 1, "  "));
        assert_eq!((e.anchor, e.focus), (3, 3));
    }

    #[test]
    fn selection_indent_touches_each_line() {
        let doc = "one\ntwo\nthree";
        let e = plan_indent(doc, 5, 9, IndentUnit::Spaces(2), false).unwrap();
        assert_eq!(apply(doc, &e), "one\n  two\n  three");
    }

    #[test]
    fn selection_ending_at_line_start_skips_last_line() {
        let doc = "aaa\nbbb\nccc";
        let e = plan_indent(doc, 1, 8, IndentUnit::Spaces(4), false).unwrap();
        assert_eq!(apply(doc, &e), "    aaa\n    bbb\nccc");
    }

    #[test]
    fn outdent_removes_one_level() {
        let doc = "    aaa\n      bbb\n\tccc";
        let e = plan_indent(doc, 0, doc.len(), IndentUnit::Spaces(4), true).unwrap();
        assert_eq!(apply(doc, &e), "aaa\n  bbb\nccc");
    }

    #[test]
    fn caret_shift_tab_outdents_current_line() {
        let doc = "xx\n    yy\nzz";
        // Caret inside the indented line.
        let e = plan_indent(doc, 8, 8, IndentUnit::Spaces(4), true).unwrap();
        assert_eq!(apply(doc, &e), "xx\nyy\nzz");
        // Caret remaps into the line content.
        assert!(e.anchor >= 3 && e.anchor <= 5);
    }

    #[test]
    fn outdent_ws_partial_spaces() {
        assert_eq!(outdent_ws("   x", IndentUnit::Spaces(4)), "x");
        assert_eq!(outdent_ws("\tx", IndentUnit::Spaces(4)), "x");
        assert_eq!(outdent_ws("        x", IndentUnit::Tab), "    x");
        assert_eq!(outdent_ws("x", IndentUnit::Spaces(2)), "x");
    }

    #[test]
    fn plan_indent_noop_returns_none() {
        let doc = "abc";
        assert!(plan_indent(doc, 0, 0, IndentUnit::Tab, true).is_none());
        let doc2 = "abc\ndef";
        assert!(plan_indent(doc2, 0, doc2.len(), IndentUnit::Spaces(2), true).is_none());
    }

    #[test]
    fn auto_indent_copies_ws_and_openers() {
        let doc = "fn f() {\n    body";
        assert_eq!(auto_indent(doc, 9, IndentUnit::Spaces(4)), "\n    ");
        let doc2 = "if x:\n    pass";
        assert_eq!(auto_indent(doc2, 5, IndentUnit::Spaces(4)), "\n    ");
        let doc3 = "plain line\nnext";
        assert_eq!(auto_indent(doc3, 5, IndentUnit::Spaces(2)), "\n");
        let doc4 = "a {\n        deep";
        assert_eq!(auto_indent(doc4, 8, IndentUnit::Spaces(4)), "\n        ");
    }

    #[test]
    fn indent_columns_measure_visual_width() {
        let cols = indent_columns("fn f() {\n    x();\n\t\ty();\n}\n");
        assert_eq!(cols, vec![0, 4, 8, 0]);
        // A whitespace-only line still reports its width.
        assert_eq!(indent_columns("    \n"), vec![4]);
        // Tab stops are multiples of 4: "  \t" lands on 4, "\t\t" on 8,
        // " \t" (space then tab to the next stop) also lands on 4.
        assert_eq!(indent_columns("  \tx\n\t\ty\n \tz\n"), vec![4, 8, 4]);
    }

    #[test]
    fn indent_columns_capped() {
        let doc = "x\n".repeat(MAX_GUIDE_LINES + 5);
        assert_eq!(indent_columns(&doc).len(), MAX_GUIDE_LINES);
    }

    #[test]
    fn guide_runs_merge_contiguous_lines() {
        // cols: 0,4,8,4,0 with 4-space unit → guide@4 spans lines 1..=3,
        // guide@8 spans line 2 only; unindented lines break runs.
        let cols = [0, 4, 8, 4, 0];
        assert_eq!(
            guide_runs(&cols, IndentUnit::Spaces(4)),
            vec![(4, 1, 3), (8, 2, 2)]
        );
        // 2-space unit emits every multiple the indent reaches.
        assert_eq!(
            guide_runs(&cols, IndentUnit::Spaces(2)),
            vec![(2, 1, 3), (4, 1, 3), (6, 2, 2), (8, 2, 2)]
        );
        // A blank/unindented line splits a run into two.
        assert_eq!(
            guide_runs(&[4, 0, 4], IndentUnit::Spaces(4)),
            vec![(4, 0, 0), (4, 2, 2)]
        );
        // Guides cap at MAX_GUIDE_COL.
        let deep = [MAX_GUIDE_COL + 8];
        let runs = guide_runs(&deep, IndentUnit::Spaces(4));
        assert!(runs.iter().all(|(g, _, _)| *g <= MAX_GUIDE_COL));
        assert_eq!(runs.last().map(|r| r.0), Some(MAX_GUIDE_COL / 4 * 4));
    }

    #[test]
    fn choice_cycle_and_parse() {
        assert_eq!(
            UnitChoice::Auto.next(),
            UnitChoice::Fixed(IndentUnit::Spaces(2))
        );
        assert_eq!(
            UnitChoice::Fixed(IndentUnit::Spaces(4)).next(),
            UnitChoice::Fixed(IndentUnit::Spaces(8))
        );
        assert_eq!(UnitChoice::Fixed(IndentUnit::Tab).next(), UnitChoice::Auto);
        assert_eq!(UnitChoice::parse("auto"), UnitChoice::Auto);
        assert_eq!(
            UnitChoice::parse("4"),
            UnitChoice::Fixed(IndentUnit::Spaces(4))
        );
        assert_eq!(UnitChoice::parse("tab"), UnitChoice::Fixed(IndentUnit::Tab));
        assert_eq!(UnitChoice::parse("bogus"), UnitChoice::Auto);
    }
}
