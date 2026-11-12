//! Pure parsing and resolution helpers for inline Git conflict markers.

use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Choice {
    Current,
    Incoming,
    Both,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conflict {
    pub range: Range<usize>,
    pub current: Range<usize>,
    pub incoming: Range<usize>,
    pub start_line: usize,
}

impl Conflict {
    pub fn replacement(&self, text: &str, choice: Choice) -> String {
        match choice {
            Choice::Current => text[self.current.clone()].to_string(),
            Choice::Incoming => text[self.incoming.clone()].to_string(),
            Choice::Both => format!(
                "{}{}",
                &text[self.current.clone()],
                &text[self.incoming.clone()]
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Marker {
    Start(usize),
    Base(usize),
    Separator(usize),
    End(usize),
}

fn marker(line: &str) -> Option<Marker> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let bytes = line.as_bytes();
    let (byte, kind) = match bytes.first()? {
        b'<' => (b'<', 0),
        b'|' => (b'|', 1),
        b'=' => (b'=', 2),
        b'>' => (b'>', 3),
        _ => return None,
    };
    let width = bytes.iter().take_while(|&&b| b == byte).count();
    if width < 7 || (kind == 2 && width != bytes.len()) {
        return None;
    }
    match kind {
        0 => Some(Marker::Start(width)),
        1 => Some(Marker::Base(width)),
        2 => Some(Marker::Separator(width)),
        _ => Some(Marker::End(width)),
    }
}

fn lines(text: &str) -> impl Iterator<Item = (usize, usize, usize, &str)> {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start >= text.len() {
            return None;
        }
        let rest = &text[start..];
        let end = rest.find('\n').map_or(text.len(), |n| start + n);
        let after = if end < text.len() { end + 1 } else { end };
        let result = (start, end, after, &text[start..end]);
        start = after;
        Some((result.0, result.1, result.2, result.3))
    })
}

pub fn has_markers(text: &str) -> bool {
    lines(text).any(|(_, _, _, line)| marker(line).is_some())
}

/// Markers outside complete parsed blocks need manual repair.
pub fn has_unparsed_markers(text: &str, blocks: &[Conflict]) -> bool {
    let mut end = 0;
    for block in blocks {
        if has_markers(&text[end..block.range.start]) {
            return true;
        }
        end = block.range.end;
    }
    has_markers(&text[end..])
}

pub fn parse(text: &str) -> Vec<Conflict> {
    let rows: Vec<_> = lines(text).collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let (start, _, after_start, line) = rows[i];
        let Some(Marker::Start(width)) = marker(line) else {
            i += 1;
            continue;
        };
        let mut separator = None;
        let mut base = None;
        let mut end = None;
        let mut malformed = false;
        let mut j = i + 1;
        while j < rows.len() {
            match marker(rows[j].3) {
                Some(Marker::Start(_)) => {
                    malformed = true;
                    break;
                }
                Some(Marker::Base(w)) if w == width && separator.is_none() && base.is_none() => {
                    base = Some(j)
                }
                Some(Marker::Base(_)) => {
                    malformed = true;
                    break;
                }
                Some(Marker::Separator(w)) if w == width && separator.is_none() => {
                    separator = Some(j)
                }
                Some(Marker::End(w)) if w == width && separator.is_some() => {
                    end = Some(j);
                    break;
                }
                Some(Marker::Separator(_) | Marker::End(_)) => {
                    malformed = true;
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        if let (false, Some(sep), Some(end_i)) = (malformed, separator, end) {
            let current_start = after_start;
            let current_end = base.map_or(rows[sep].0, |base_i| rows[base_i].0);
            let incoming_start = rows[sep].2;
            let incoming_end = rows[end_i].0;
            result.push(Conflict {
                range: start..rows[end_i].2,
                current: current_start..current_end,
                incoming: incoming_start..incoming_end,
                start_line: i,
            });
            i = end_i + 1;
        } else {
            if malformed {
                // Skip the malformed outer block so a nested marker cannot be
                // reinterpreted as an independent conflict.
                i = rows[j..]
                    .iter()
                    .position(|row| matches!(marker(row.3), Some(Marker::End(w)) if w == width))
                    .map_or(rows.len(), |offset| j + offset + 1);
            } else {
                i += 1;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resolves_multiple_crlf_diff3_and_eof() {
        let text = "before\r\n<<<<<<< ours\r\né🙂\r\n||||||| base\r\nold\r\n=======\r\nnew\r\n>>>>>>> theirs\r\nafter\n<<<<<<< x\n=======\n>>>>>>> y";
        let conflicts = parse(text);
        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflicts[0].start_line, 1);
        assert_eq!(
            conflicts[0].replacement(text, Choice::Both),
            "é🙂\r\nnew\r\n"
        );
        assert_eq!(conflicts[1].replacement(text, Choice::Current), "");
        let mut resolved = text.to_string();
        for c in conflicts.into_iter().rev() {
            resolved.replace_range(c.range.clone(), &c.replacement(text, Choice::Incoming));
        }
        assert!(resolved.starts_with("before\r\nnew\r\nafter\n"));
    }

    #[test]
    fn rejects_bad_blocks_but_flags_incomplete_markers() {
        assert!(parse("<<<<<<< x\na\n<<<<<<< nested\n=======\nb\n>>>>>>> x").is_empty());
        assert!(parse("<<<<<<< x\na\n=======\nb\n>>>>>>> x\n=======").len() == 1);
        assert!(has_markers("text\n<<<<<<< incomplete"));
        assert!(!has_markers(" text\n<<<<<<"));
    }
    #[test]
    fn widths_empty_sides_and_base_are_preserved() {
        for style in ["", "||||||||| ancestor\nbase\n"] {
            let text = format!("pré\n<<<<<<<<< ours\nα\n{style}=========\nβ\n>>>>>>>>> theirs");
            let blocks = parse(&text);
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0].replacement(&text, Choice::Current), "α\n");
            assert_eq!(blocks[0].replacement(&text, Choice::Incoming), "β\n");
            let mut edited = text.clone();
            edited.replace_range(
                blocks[0].range.clone(),
                &blocks[0].replacement(&text, Choice::Both),
            );
            assert_eq!(edited, "pré\nα\nβ\n");
        }
        let text = "<<<<<<< ours\n=======\n>>>>>>> theirs";
        let c = &parse(text)[0];
        assert_eq!(c.replacement(text, Choice::Both), "");
        for text in [
            "<<<<<<< ours\na\n========\nb\n>>>>>>> theirs",
            "<<<<<<< ours\na\n|||||||| base\nb\n=======\nc\n>>>>>>> theirs",
            "<<<<<<< ours\na\n=======\nb\n>>>>>>>> theirs",
            "<<<<<<< ours\na\n=======\nb",
        ] {
            assert!(parse(text).is_empty(), "malformed: {text}");
            assert!(has_markers(text));
        }
    }
    #[test]
    fn malformed_markers_alongside_valid_blocks_need_manual_repair() {
        let valid = "<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\n";
        assert!(!has_unparsed_markers(valid, &parse(valid)));
        for text in [
            format!("{valid}<<<<<<< incomplete"),
            format!("=======\n{valid}"),
        ] {
            assert_eq!(parse(&text).len(), 1);
            assert!(has_unparsed_markers(&text, &parse(&text)));
        }
    }
}
