//! Toggle comments on the selected lines (⌘/): a pure plan like
//! `indent::plan_indent`, applied by the editor field as one undo step.

use crate::editor::indent::{IndentEdit, leading_ws_len};

/// How a language writes a comment on one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// `// text`, `# text`, `-- text`.
    Line(&'static str),
    /// `/* text */`, `<!-- text -->`, wrapped on each line.
    Block(&'static str, &'static str),
}

/// The comment style of the file at `path`, or `None` for a type with no
/// comments (JSON, plain text) or one this does not know.
pub fn style_for(path: &str) -> Option<Style> {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let ext = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    if matches!(
        name.as_str(),
        "dockerfile" | "makefile" | ".gitignore" | ".gitattributes"
    ) || name.starts_with(".env")
    {
        return Some(Style::Line("#"));
    }
    Some(match ext {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "go" | "java" | "c" | "h" | "cpp"
        | "cc" | "hpp" | "cs" | "swift" | "kt" | "kts" | "scala" | "dart" | "php" | "zig"
        | "scss" | "less" | "jsonc" | "proto" | "graphql" | "svelte" | "astro" | "tf" => {
            Style::Line("//")
        }
        "py" | "rb" | "sh" | "bash" | "zsh" | "yml" | "yaml" | "toml" | "conf" | "mk" | "r"
        | "pl" | "ps1" | "ex" | "exs" | "nim" | "jl" => Style::Line("#"),
        "sql" | "lua" | "hs" | "elm" => Style::Line("--"),
        "css" => Style::Block("/*", "*/"),
        "html" | "htm" | "xml" | "svg" | "vue" | "md" | "mdx" => Style::Block("<!--", "-->"),
        _ => return None,
    })
}

/// Comment the lines `a..b` touches, or uncomment them when every non-blank
/// one is already commented. Comments go at the shallowest indent, so a block
/// stays aligned; blank lines are left alone. The returned selection keeps
/// the caret on the same text.
pub fn plan_toggle(doc: &str, a: usize, b: usize, style: Style) -> Option<IndentEdit> {
    let (a, b) = (a.min(b).min(doc.len()), a.max(b).min(doc.len()));
    let first = doc[..a].rfind('\n').map_or(0, |i| i + 1);
    let mut last = b;
    if b > a && last > first && doc.as_bytes()[last - 1] == b'\n' {
        last -= 1;
    }
    let end = doc[last..].find('\n').map_or(doc.len(), |i| last + i);
    let lines: Vec<&str> = doc[first..end].split('\n').collect();

    let (open, close) = match style {
        Style::Line(token) => (token, ""),
        Style::Block(open, close) => (open, close),
    };
    let mut targets: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].trim().is_empty())
        .collect();
    if targets.is_empty() {
        targets = (0..lines.len()).collect();
    }
    let commented = |line: &str| {
        line.trim_start().starts_with(open)
            && (close.is_empty() || line.trim_end().ends_with(close))
    };
    let uncomment = targets.iter().all(|&i| commented(lines[i]));
    let indent = targets
        .iter()
        .map(|&i| leading_ws_len(lines[i]))
        .min()
        .unwrap_or(0);

    let mut out = String::with_capacity(doc.len() - (end - first) + (end - first) + 64);
    // (absolute position, bytes removed, bytes added): one per line for a
    // line comment; the caret follows these.
    let mut edits: Vec<(usize, usize, usize)> = Vec::new();
    let mut at = first;
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            at += 1;
        }
        if !targets.contains(&i) {
            out.push_str(line);
        } else if uncomment {
            let ws = leading_ws_len(line);
            let mut rest = &line[ws + open.len()..];
            let gap = usize::from(rest.starts_with(' '));
            rest = &rest[gap..];
            let tail = if close.is_empty() {
                rest
            } else {
                let body = rest.trim_end().strip_suffix(close).unwrap_or(rest);
                body.strip_suffix(' ').unwrap_or(body)
            };
            out.push_str(&line[..ws]);
            out.push_str(tail);
            edits.push((at + ws, open.len() + gap, 0));
        } else {
            out.push_str(&line[..indent]);
            out.push_str(open);
            out.push(' ');
            out.push_str(&line[indent..]);
            if !close.is_empty() {
                out.push(' ');
                out.push_str(close);
            }
            edits.push((at + indent, 0, open.len() + 1));
        }
        at += line.len();
    }
    if out == doc[first..end] {
        return None;
    }
    // A block comment edits both ends of a line: select the whole region.
    let (anchor, focus) = if close.is_empty() {
        (follow(&edits, a), follow(&edits, b))
    } else {
        (first, first + out.len())
    };
    Some(IndentEdit {
        start: first,
        end,
        text: out,
        anchor,
        focus,
    })
}

/// Where `offset` lands once `edits` (position, removed, added) are applied.
fn follow(edits: &[(usize, usize, usize)], offset: usize) -> usize {
    let mut shift = 0isize;
    for &(pos, removed, added) in edits {
        if offset < pos {
            break;
        }
        if offset < pos + removed {
            return (pos as isize + shift) as usize + added;
        }
        shift += added as isize - removed as isize;
    }
    (offset as isize + shift) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: Style = Style::Line("//");

    fn toggle(doc: &str, a: usize, b: usize, style: Style) -> (String, usize, usize) {
        let e = plan_toggle(doc, a, b, style).expect("an edit");
        let out = format!("{}{}{}", &doc[..e.start], e.text, &doc[e.end..]);
        (out, e.anchor, e.focus)
    }

    #[test]
    fn a_line_comments_and_uncomments_and_the_caret_stays_on_its_text() {
        let doc = "fn a() {}\n    let x = 1;\nfn b() {}";
        let caret = doc.find("x").unwrap();
        let (on, anchor, _) = toggle(doc, caret, caret, RUST);
        assert_eq!(on, "fn a() {}\n    // let x = 1;\nfn b() {}");
        assert_eq!(&on[anchor..anchor + 1], "x", "the caret follows its text");
        let (off, anchor, _) = toggle(&on, anchor, anchor, RUST);
        assert_eq!(off, doc);
        assert_eq!(&off[anchor..anchor + 1], "x");
    }

    #[test]
    fn a_selection_comments_every_line_at_the_shallowest_indent() {
        let doc = "if x {\n    a();\n\n        b();\n}";
        let (on, ..) = toggle(doc, 0, doc.len(), RUST);
        assert_eq!(on, "// if x {\n//     a();\n\n//         b();\n// }");
        let (off, ..) = toggle(&on, 0, on.len(), RUST);
        assert_eq!(off, doc, "blank lines stay blank both ways");
        // Inside an indented block the marks line up at the block's indent.
        let (on, ..) = toggle(doc, 8, 30, RUST);
        assert_eq!(on, "if x {\n    // a();\n\n    //     b();\n}");
    }

    #[test]
    fn a_mixed_selection_comments_all_rather_than_uncommenting() {
        let doc = "// a\nb";
        let (on, ..) = toggle(doc, 0, doc.len(), RUST);
        assert_eq!(on, "// // a\n// b");
    }

    #[test]
    fn a_selection_ending_at_a_line_start_leaves_that_line_alone() {
        let doc = "a\nb\nc";
        let (on, ..) = toggle(doc, 0, 4, RUST);
        assert_eq!(on, "# a\n# b\nc".replace('#', "//"));
    }

    #[test]
    fn block_styles_wrap_each_line() {
        let css = Style::Block("/*", "*/");
        let doc = "a { b: c }\n  d { }";
        let (on, anchor, focus) = toggle(doc, 0, doc.len(), css);
        assert_eq!(
            on, "/* a { b: c } */\n/*   d { } */",
            "at the shallowest indent"
        );
        assert_eq!((anchor, focus), (0, on.len()), "the region stays selected");
        let (off, ..) = toggle(&on, 0, on.len(), css);
        assert_eq!(off, doc);
    }

    #[test]
    fn the_style_follows_the_file_and_json_has_none() {
        assert_eq!(style_for("src/a.rs"), Some(Style::Line("//")));
        assert_eq!(style_for("a/b.py"), Some(Style::Line("#")));
        assert_eq!(style_for("Dockerfile"), Some(Style::Line("#")));
        assert_eq!(style_for("x/.env.local"), Some(Style::Line("#")));
        assert_eq!(style_for("q.sql"), Some(Style::Line("--")));
        assert_eq!(style_for("a.css"), Some(Style::Block("/*", "*/")));
        assert_eq!(style_for("a.html"), Some(Style::Block("<!--", "-->")));
        assert_eq!(style_for("a.json"), None);
        assert_eq!(style_for("notes.txt"), None);
    }
}
