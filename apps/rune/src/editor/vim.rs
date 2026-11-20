//! Modal (Vim) editing for the code field: a pure state machine over the
//! document text, the selection and one key.
//!
//! No Rune types, no I/O, no settings: [`step`] returns [`Effect`]s the host
//! applies to its `TextEdit`, so every rule here is testable without a
//! runtime. The host owns the mode chip, the block caret, the register and the
//! find/command bars; this module owns the grammar of pending keys, counts,
//! motions and (in later tasks) operators.

use crate::editor::indent::{self, IndentUnit, TAB_STOP};

/// Settings-store key for the Vim mode switch (`"1"` on, anything else off).
pub const VIM_KEY: &str = "goko.editor.vim";

/// The editing mode of one document.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl Mode {
    /// The footer chip's label.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
        }
    }

    /// Whether the native bar caret is used (Insert) rather than the block.
    pub fn inserts(self) -> bool {
        self == Mode::Insert
    }
}

/// A key the engine understands. The host maps `rune::KeyEvent` onto this, so
/// the engine stays free of Rune types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Key {
    Char(char),
    Enter,
    Escape,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// A Control chord (`Ctrl-r` for redo).
    Ctrl(char),
}

/// What the engine reads of the document: its text, the caret and the
/// resolved indent unit. Visual's fixed end lives in [`VimState`].
#[derive(Clone, Copy, Debug)]
pub struct Doc<'a> {
    pub text: &'a str,
    pub focus: usize,
    pub indent: IndentUnit,
}

/// What the host must do, in order. `Select` moves the selection; `Replace`
/// swaps bytes; the rest are host actions. `Pass` hands the key to the native
/// field (Insert-mode typing).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    Select {
        anchor: usize,
        focus: usize,
    },
    Replace {
        start: usize,
        end: usize,
        text: String,
    },
    Undo,
    Redo,
    Yank {
        text: String,
        linewise: bool,
    },
    Paste {
        before: bool,
        count: usize,
    },
    Find(FindAction),
    Command(CommandAction),
    Scroll(Scroll),
    Pass,
}

/// Find-bar actions the engine asks of the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindAction {
    Open { forward: bool },
    WordUnderCaret,
    Next,
    Prev,
}

/// Command-bar actions the engine asks of the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandAction {
    Open,
}

/// Where `zz`/`zt`/`zb` put the caret line in the viewport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scroll {
    Center,
    Top,
    Bottom,
}

/// The `f`/`t` kind and its character, repeated by `;`/`,`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LastFind {
    kind: FindKind,
    ch: char,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FindKind {
    /// `f`
    To,
    /// `t`
    Till,
    /// `F`
    ToBack,
    /// `T`
    TillBack,
}

impl FindKind {
    fn forward(self) -> bool {
        matches!(self, FindKind::To | FindKind::Till)
    }

    fn till(self) -> bool {
        matches!(self, FindKind::Till | FindKind::TillBack)
    }

    fn reversed(self) -> FindKind {
        match self {
            FindKind::To => FindKind::ToBack,
            FindKind::Till => FindKind::TillBack,
            FindKind::ToBack => FindKind::To,
            FindKind::TillBack => FindKind::Till,
        }
    }
}

/// Keys typed but not yet a command: counts, `g`, a pending `f`/`t`, and the
/// operator/text-object prefixes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pending {
    count: Option<usize>,
    /// The count typed before an operator (`2d3w` keeps 2 here and 3 in
    /// `count`); the effective count is their product.
    op_count: Option<usize>,
    op: Option<Op>,
    text_obj: Option<char>,
    g: bool,
    z: bool,
    find: Option<FindKind>,
}

/// The unnamed register: the last yanked or deleted text.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

impl Register {
    pub fn set(&mut self, text: impl Into<String>, linewise: bool) {
        self.text = text.into();
        self.linewise = linewise;
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// An operator waiting for a motion or text object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
}

/// How a command moved: kept for `.` replay.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Motion {
    Key(Key),
    Find(FindKind, char),
    Object(char, char),
}

/// The last Normal-mode change, replayed by `.`.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Repeat {
    Operator {
        op: Op,
        motion: Motion,
        count: usize,
        inserted: Option<String>,
    },
    Simple {
        key: char,
        count: usize,
    },
    Insert {
        text: String,
    },
}

/// Where a motion lands and how an operator treats the span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MotionPlan {
    target: usize,
    /// The motion's last character is part of the operated text (`e`, `$`,
    /// `f`, `%`).
    inclusive: bool,
    /// Whole lines (`j`, `k`, `G`, `gg`, doubled operators).
    linewise: bool,
}

/// Per-document Vim state. The enabled flag mirrors the global setting.
#[derive(Clone, Debug, Default)]
pub struct VimState {
    pub enabled: bool,
    pub mode: Mode,
    pending: Pending,
    goal_col: Option<usize>,
    last_find: Option<LastFind>,
    last_change: Option<Repeat>,
    /// The Visual selection's fixed end; the caret is the moving end.
    visual_anchor: Option<usize>,
    /// Byte the current `/` or `?` started from, for Enter and `n`/`N`.
    pub search_origin: Option<usize>,
    /// The last `/`/`?` direction; `n` repeats it.
    pub search_forward: bool,
}

impl VimState {
    /// Reset to Normal and drop half-typed keys (setting off, document load).
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.pending = Pending::default();
        self.goal_col = None;
        self.visual_anchor = None;
        self.search_origin = None;
    }

    /// Drop a half-typed operator after the document changed under it.
    pub fn clear_pending(&mut self) {
        self.pending = Pending::default();
        self.goal_col = None;
    }

    /// The byte range the Visual selection covers at `focus`: inclusive of
    /// the character under the caret, whole lines in Visual Line.
    pub fn visual_range(&self, text: &str, focus: usize) -> Option<std::ops::Range<usize>> {
        let anchor = self.visual_anchor?;
        let focus = boundary(text, focus);
        let (a, f) = (anchor.min(focus), anchor.max(focus));
        match self.mode {
            Mode::VisualLine => {
                let start = line_start(text, a);
                let last = line_end(text, f);
                let end = if last < text.len() { last + 1 } else { last };
                (start < end).then_some(start..end)
            }
            Mode::Visual => {
                let end = next_char(text, f).min(text.len());
                (a < end).then_some(a..end)
            }
            _ => None,
        }
    }

    /// Mirror the setting: turning it off returns to Normal and clears keys.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.reset();
        }
    }

    /// The pending keys for the mode chip (`2d`, `f`, `g`); empty when none.
    pub fn pending_label(&self) -> String {
        let mut out = String::new();
        if let Some(n) = self.pending.op_count {
            out.push_str(&n.to_string());
        }
        if let Some(op) = self.pending.op {
            out.push(match op {
                Op::Delete => 'd',
                Op::Change => 'c',
                Op::Yank => 'y',
                Op::Indent => '>',
                Op::Outdent => '<',
            });
        }
        if let Some(n) = self.pending.count {
            out.push_str(&n.to_string());
        }
        if self.pending.g {
            out.push('g');
        }
        if self.pending.z {
            out.push('z');
        }
        if let Some(kind) = self.pending.find {
            out.push(match kind {
                FindKind::To => 'f',
                FindKind::Till => 't',
                FindKind::ToBack => 'F',
                FindKind::TillBack => 'T',
            });
        }
        if let Some(prefix) = self.pending.text_obj {
            out.push(prefix);
        }
        out
    }

    /// Record the net text an Insert session added, for `.`. A `c` session
    /// keeps its operator and gains the inserted text; any other session
    /// repeats the text itself.
    pub fn record_insert(&mut self, inserted: &str) {
        if inserted.is_empty() {
            return;
        }
        self.last_change = match self.last_change.take() {
            Some(Repeat::Operator {
                op: Op::Change,
                motion,
                count,
                ..
            }) => Some(Repeat::Operator {
                op: Op::Change,
                motion,
                count,
                inserted: Some(inserted.to_string()),
            }),
            _ => Some(Repeat::Insert {
                text: inserted.to_string(),
            }),
        };
    }
}

/// One key through the engine. The host compares the mode before and after to
/// notice an Insert session ending.
pub fn step(vim: &mut VimState, doc: &Doc<'_>, key: Key) -> Vec<Effect> {
    if !vim.enabled {
        return vec![Effect::Pass];
    }
    let text = doc.text;
    let focus = boundary(text, doc.focus);
    match vim.mode {
        Mode::Insert => insert_step(vim, text, focus, key),
        Mode::Normal => normal_step(vim, doc, focus, key),
        Mode::Visual | Mode::VisualLine => visual_step(vim, doc, focus, key),
    }
}

fn insert_step(vim: &mut VimState, text: &str, focus: usize, key: Key) -> Vec<Effect> {
    if key == Key::Escape {
        vim.mode = Mode::Normal;
        vim.pending = Pending::default();
        vim.goal_col = None;
        let at = clamp_normal(text, focus);
        return vec![Effect::Select {
            anchor: at,
            focus: at,
        }];
    }
    vec![Effect::Pass]
}

fn normal_step(vim: &mut VimState, doc: &Doc<'_>, focus: usize, key: Key) -> Vec<Effect> {
    let text = doc.text;
    // A pending `f`/`t` takes the next key as its character, whatever it is.
    if let Some(kind) = vim.pending.find {
        vim.pending.find = None;
        let count = vim.pending.count.take().unwrap_or(1);
        let op_count = vim.pending.op_count.take().unwrap_or(1);
        let op = vim.pending.op.take();
        let Some(ch) = key_char(&key) else {
            return vec![];
        };
        vim.last_find = Some(LastFind { kind, ch });
        let Some(at) = find_motion(text, focus, kind, ch, count * op_count) else {
            return vec![];
        };
        let plan = MotionPlan {
            target: at,
            inclusive: true,
            linewise: false,
        };
        return match op {
            Some(op) => {
                let effects = apply_operator_plan(vim, doc, focus, op, plan, None);
                if !effects.is_empty() {
                    vim.last_change = Some(Repeat::Operator {
                        op,
                        motion: Motion::Find(kind, ch),
                        count: count * op_count,
                        inserted: None,
                    });
                }
                effects
            }
            None => {
                vim.goal_col = None;
                select(at)
            }
        };
    }
    // A pending text object takes the next key as its object.
    if let Some(prefix) = vim.pending.text_obj.take() {
        let count = vim.pending.count.take().unwrap_or(1);
        let op_count = vim.pending.op_count.take().unwrap_or(1);
        let op = vim.pending.op.take();
        let Some(object) = key_char(&key) else {
            return vec![];
        };
        let Some(op) = op else {
            return vec![];
        };
        let Some(range) = text_object_range(text, focus, prefix, object) else {
            return vec![];
        };
        let effects = apply_operator_range(vim, doc, op, range, false, None);
        if !effects.is_empty() {
            vim.last_change = Some(Repeat::Operator {
                op,
                motion: Motion::Object(prefix, object),
                count: count * op_count,
                inserted: None,
            });
        }
        return effects;
    }
    // A pending `g` completes `gg`.
    if vim.pending.g {
        vim.pending.g = false;
        let count = vim.pending.count.take();
        let op_count = vim.pending.op_count.take().unwrap_or(1);
        let op = vim.pending.op.take();
        if key == Key::Char('g') {
            let target = goto_line(text, count.map_or(0, |n| n - 1));
            let plan = MotionPlan {
                target,
                inclusive: false,
                linewise: true,
            };
            return match op {
                Some(op) => {
                    let effects = apply_operator_plan(vim, doc, focus, op, plan, None);
                    if !effects.is_empty() {
                        vim.last_change = Some(Repeat::Operator {
                            op,
                            motion: Motion::Key(Key::Char('g')),
                            count: count.unwrap_or(1) * op_count,
                            inserted: None,
                        });
                    }
                    effects
                }
                None => select(target),
            };
        }
        return vec![];
    }
    // A pending `z` completes `zz`/`zt`/`zb`.
    if vim.pending.z {
        vim.pending.z = false;
        vim.pending.count = None;
        return match key {
            Key::Char('z') => vec![Effect::Scroll(Scroll::Center)],
            Key::Char('t') => vec![Effect::Scroll(Scroll::Top)],
            Key::Char('b') => vec![Effect::Scroll(Scroll::Bottom)],
            _ => vec![],
        };
    }
    // Counts accumulate until a command consumes them.
    if let Key::Char(c) = key
        && c.is_ascii_digit()
        && (c != '0' || vim.pending.count.is_some())
    {
        let n = vim.pending.count.unwrap_or(0);
        vim.pending.count = Some((n * 10 + c.to_digit(10).unwrap() as usize).max(1));
        return vec![];
    }
    if key == Key::Escape {
        vim.pending = Pending::default();
        vim.goal_col = None;
        return vec![];
    }
    let count_opt = vim.pending.count;
    vim.pending.count = None;
    let count = count_opt.unwrap_or(1);
    // Operators wait for a motion, a text object or their doubled form.
    if let Key::Char(c @ ('d' | 'c' | 'y' | '>' | '<')) = key {
        let op = match c {
            'd' => Op::Delete,
            'c' => Op::Change,
            'y' => Op::Yank,
            '>' => Op::Indent,
            _ => Op::Outdent,
        };
        if vim.pending.op == Some(op) {
            vim.pending.op = None;
            let total = vim.pending.op_count.take().unwrap_or(1) * count;
            let plan = MotionPlan {
                target: move_line(text, focus, total.saturating_sub(1) as isize, None),
                inclusive: false,
                linewise: true,
            };
            let effects = apply_operator_plan(vim, doc, focus, op, plan, None);
            if !effects.is_empty() {
                vim.last_change = Some(Repeat::Operator {
                    op,
                    motion: Motion::Key(Key::Char(c)),
                    count: total,
                    inserted: None,
                });
            }
            return effects;
        }
        vim.pending.op = Some(op);
        vim.pending.op_count = count_opt;
        return vec![];
    }
    // `i`/`a` after an operator start a text object.
    if let Key::Char(c @ ('i' | 'a')) = key
        && vim.pending.op.is_some()
    {
        vim.pending.text_obj = Some(c);
        vim.pending.count = count_opt;
        return vec![];
    }
    // An operator with a motion.
    if let Some(op) = vim.pending.op {
        let op_count = vim.pending.op_count.unwrap_or(1);
        let has_count = count_opt.is_some() || vim.pending.op_count.is_some();
        if let Some(plan) = motion_plan(text, focus, &key, count * op_count, has_count) {
            vim.pending.op = None;
            vim.pending.op_count = None;
            let effects = apply_operator_plan(vim, doc, focus, op, plan, None);
            if !effects.is_empty() {
                vim.last_change = Some(Repeat::Operator {
                    op,
                    motion: Motion::Key(key),
                    count: count * op_count,
                    inserted: None,
                });
            }
            return effects;
        }
        // Not a motion: drop the operator.
        vim.pending.op = None;
        vim.pending.op_count = None;
        return vec![];
    }
    // Simple edits and commands.
    if let Key::Char(c) = key {
        match c {
            'x' | 'J' | 'p' | 'P' => return simple_edit(vim, doc, focus, c, count, true),
            'D' | 'C' => {
                let op = if c == 'D' { Op::Delete } else { Op::Change };
                let plan = MotionPlan {
                    target: last_char(text, focus),
                    inclusive: true,
                    linewise: false,
                };
                let effects = apply_operator_plan(vim, doc, focus, op, plan, None);
                if !effects.is_empty() {
                    vim.last_change = Some(Repeat::Operator {
                        op,
                        motion: Motion::Key(Key::Char('$')),
                        count: 1,
                        inserted: None,
                    });
                }
                return effects;
            }
            'u' => return vec![Effect::Undo],
            '.' => return repeat_last(vim, doc, focus),
            _ => {}
        }
    }
    if let Key::Ctrl('r') = key {
        return vec![Effect::Redo];
    }
    // Motions, insert entries, search and the command line.
    let keeps_goal = matches!(&key, Key::Char('j') | Key::Char('k') | Key::Down | Key::Up);
    let effects = match key {
        Key::Char('h') | Key::Left => motion_col(vim, text, focus, -(count as isize)),
        Key::Char('l') | Key::Right => motion_col(vim, text, focus, count as isize),
        Key::Char('j') | Key::Down => motion_line(vim, text, focus, count as isize),
        Key::Char('k') | Key::Up => motion_line(vim, text, focus, -(count as isize)),
        Key::Char('0') | Key::Home => select(line_start(text, focus)),
        Key::Char('^') => select(first_non_blank(text, focus)),
        Key::Char('$') | Key::End => select(last_char(text, focus)),
        Key::Char('w') => select(word_forward(text, focus, count, false)),
        Key::Char('W') => select(word_forward(text, focus, count, true)),
        Key::Char('b') => select(word_back(text, focus, count, false)),
        Key::Char('B') => select(word_back(text, focus, count, true)),
        Key::Char('e') => select(word_end(text, focus, count, false)),
        Key::Char('E') => select(word_end(text, focus, count, true)),
        Key::Char('g') => {
            vim.pending.g = true;
            vim.pending.count = count_opt;
            vec![]
        }
        Key::Char('G') => select(if count_opt.is_some() {
            goto_line(text, count - 1)
        } else {
            goto_line(text, line_count(text).saturating_sub(1))
        }),
        Key::Char('z') => {
            vim.pending.z = true;
            vim.pending.count = count_opt;
            vec![]
        }
        Key::Char('f') => {
            vim.pending.find = Some(FindKind::To);
            vim.pending.count = count_opt;
            vec![]
        }
        Key::Char('t') => {
            vim.pending.find = Some(FindKind::Till);
            vim.pending.count = count_opt;
            vec![]
        }
        Key::Char('F') => {
            vim.pending.find = Some(FindKind::ToBack);
            vim.pending.count = count_opt;
            vec![]
        }
        Key::Char('T') => {
            vim.pending.find = Some(FindKind::TillBack);
            vim.pending.count = count_opt;
            vec![]
        }
        Key::Char(';') => repeat_find(vim, text, focus, count, false),
        Key::Char(',') => repeat_find(vim, text, focus, count, true),
        Key::Char('%') => match match_bracket(text, focus) {
            Some(at) => select(at),
            None => vec![],
        },
        Key::Char('{') => select(paragraph_back(text, focus, count)),
        Key::Char('}') => select(paragraph_forward(text, focus, count)),
        Key::Char('v') => {
            vim.mode = Mode::Visual;
            vim.visual_anchor = Some(focus);
            vec![]
        }
        Key::Char('V') => {
            vim.mode = Mode::VisualLine;
            vim.visual_anchor = Some(focus);
            vec![]
        }
        Key::Char('i') => enter_insert(vim, focus),
        Key::Char('a') => {
            let at = insert_after(text, focus);
            enter_insert(vim, at)
        }
        Key::Char('I') => {
            let at = first_non_blank(text, focus);
            enter_insert(vim, at)
        }
        Key::Char('A') => {
            let at = line_end(text, focus);
            enter_insert(vim, at)
        }
        Key::Char('o') => open_line(vim, doc, focus, false),
        Key::Char('O') => open_line(vim, doc, focus, true),
        Key::Char(':') => vec![Effect::Command(CommandAction::Open)],
        Key::Char('/') => open_find(vim, focus, true),
        Key::Char('?') => open_find(vim, focus, false),
        Key::Char('n') => vec![Effect::Find(FindAction::Next)],
        Key::Char('N') => vec![Effect::Find(FindAction::Prev)],
        Key::Char('*') => vec![Effect::Find(FindAction::WordUnderCaret)],
        _ => vec![],
    };
    if !keeps_goal {
        vim.goal_col = None;
    }
    effects
}

/// `o`/`O`: open a line below or above with the resolved indent.
fn open_line(vim: &mut VimState, doc: &Doc<'_>, focus: usize, above: bool) -> Vec<Effect> {
    let text = doc.text;
    let start = line_start(text, focus);
    let end = line_end(text, focus);
    // `auto_indent` returns the newline plus the current line's leading
    // whitespace (and one more level after an opener).
    let indent = indent::auto_indent(text, focus, doc.indent);
    let ws = indent.strip_prefix('\n').unwrap_or("");
    let (at, insert, caret) = if above {
        (start, format!("{ws}\n"), start + ws.len())
    } else {
        let caret = end + indent.len();
        (end, indent, caret)
    };
    vim.mode = Mode::Insert;
    vim.pending = Pending::default();
    vim.goal_col = None;
    vec![
        Effect::Replace {
            start: at,
            end: at,
            text: insert,
        },
        select_effect(caret),
    ]
}

/// A Normal-mode change with no motion: `x`, `J`, `p`, `P`, `>`, `<`.
fn simple_edit(
    vim: &mut VimState,
    doc: &Doc<'_>,
    focus: usize,
    key: char,
    count: usize,
    record: bool,
) -> Vec<Effect> {
    let text = doc.text;
    match key {
        'x' => {
            let end = line_end(text, focus);
            let mut to = focus;
            for _ in 0..count {
                if to >= end {
                    break;
                }
                to = next_char(text, to);
            }
            if to == focus {
                return vec![];
            }
            if record {
                vim.last_change = Some(Repeat::Simple { key, count });
            }
            vec![
                Effect::Yank {
                    text: text[focus..to].to_string(),
                    linewise: false,
                },
                Effect::Replace {
                    start: focus,
                    end: to,
                    text: String::new(),
                },
                select_effect(clamp_normal(text, focus)),
            ]
        }
        'J' => {
            let joins = if count <= 1 { 1 } else { count - 1 };
            let mut edits = join_edits(text, focus, joins);
            if edits.is_empty() {
                return vec![];
            }
            if record {
                vim.last_change = Some(Repeat::Simple { key, count });
            }
            let caret = edits[0].0;
            edits.reverse();
            let mut effects: Vec<Effect> = edits
                .into_iter()
                .map(|(start, end, text)| Effect::Replace { start, end, text })
                .collect();
            effects.push(select_effect(caret));
            effects
        }
        'p' | 'P' => {
            if record {
                vim.last_change = Some(Repeat::Simple { key, count });
            }
            vec![Effect::Paste {
                before: key == 'P',
                count,
            }]
        }
        '>' | '<' => {
            let effects = indent_lines(doc, focus, count, key == '<');
            if !effects.is_empty() && record {
                vim.last_change = Some(Repeat::Simple { key, count });
            }
            effects
        }
        _ => vec![],
    }
}

/// `.`: replay the last change at the caret.
fn repeat_last(vim: &mut VimState, doc: &Doc<'_>, focus: usize) -> Vec<Effect> {
    let text = doc.text;
    let Some(last) = vim.last_change.clone() else {
        return vec![];
    };
    match last {
        Repeat::Operator {
            op,
            motion,
            count,
            inserted,
        } => match motion {
            Motion::Object(prefix, object) => {
                let Some(range) = text_object_range(text, focus, prefix, object) else {
                    return vec![];
                };
                apply_operator_range(vim, doc, op, range, false, inserted.as_deref())
            }
            Motion::Key(key) => {
                let plan = if key == Key::Char('g') {
                    Some(MotionPlan {
                        target: goto_line(text, count.saturating_sub(1)),
                        inclusive: false,
                        linewise: true,
                    })
                } else {
                    motion_plan(text, focus, &key, count, true)
                };
                match plan {
                    Some(plan) => {
                        apply_operator_plan(vim, doc, focus, op, plan, inserted.as_deref())
                    }
                    None => vec![],
                }
            }
            Motion::Find(kind, ch) => match find_motion(text, focus, kind, ch, count) {
                Some(at) => {
                    let plan = MotionPlan {
                        target: at,
                        inclusive: true,
                        linewise: false,
                    };
                    apply_operator_plan(vim, doc, focus, op, plan, inserted.as_deref())
                }
                None => vec![],
            },
        },
        Repeat::Simple { key, count } => simple_edit(vim, doc, focus, key, count, false),
        Repeat::Insert { text: insert } => {
            let at = focus;
            let last = insert.chars().next_back().map_or(0, |c| c.len_utf8());
            vec![
                Effect::Replace {
                    start: at,
                    end: at,
                    text: insert.clone(),
                },
                select_effect(at + insert.len() - last),
            ]
        }
    }
}

/// Apply an operator over a planned motion.
fn apply_operator_plan(
    vim: &mut VimState,
    doc: &Doc<'_>,
    focus: usize,
    op: Op,
    plan: MotionPlan,
    inserted: Option<&str>,
) -> Vec<Effect> {
    let Some(range) = operator_range(doc.text, focus, plan) else {
        return vec![];
    };
    apply_operator_range(vim, doc, op, range, plan.linewise, inserted)
}

/// Apply an operator over an explicit byte range.
fn apply_operator_range(
    vim: &mut VimState,
    doc: &Doc<'_>,
    op: Op,
    range: std::ops::Range<usize>,
    linewise: bool,
    inserted: Option<&str>,
) -> Vec<Effect> {
    let text = doc.text;
    let yanked = text.get(range.clone()).unwrap_or("").to_string();
    match op {
        Op::Yank => vec![
            Effect::Yank {
                text: yanked,
                linewise,
            },
            select_effect(clamp_normal(text, range.start)),
        ],
        Op::Delete => {
            let at = clamp_normal(text, range.start);
            vec![
                Effect::Yank {
                    text: yanked,
                    linewise,
                },
                Effect::Replace {
                    start: range.start,
                    end: range.end,
                    text: String::new(),
                },
                select_effect(at),
            ]
        }
        Op::Change => {
            let at = range.start;
            let mut effects = vec![Effect::Yank {
                text: yanked,
                linewise,
            }];
            match inserted {
                Some(insert) => {
                    effects.push(Effect::Replace {
                        start: range.start,
                        end: range.end,
                        text: insert.to_string(),
                    });
                    // The caret lands on the last inserted character.
                    let last = insert.chars().next_back().map_or(0, |c| c.len_utf8());
                    effects.push(select_effect(at + insert.len() - last));
                }
                None => {
                    vim.mode = Mode::Insert;
                    effects.push(Effect::Replace {
                        start: range.start,
                        end: range.end,
                        text: String::new(),
                    });
                    effects.push(select_effect(at));
                }
            }
            effects
        }
        Op::Indent | Op::Outdent => {
            let (start, end) = line_span(text, range);
            let Some(edit) = indent::plan_indent(text, start, end, doc.indent, op == Op::Outdent)
            else {
                return vec![];
            };
            vec![
                Effect::Replace {
                    start: edit.start,
                    end: edit.end,
                    text: edit.text,
                },
                select_effect(edit.anchor.min(edit.focus)),
            ]
        }
    }
}

/// The byte range an operator covers for `plan`, or `None` when it is empty.
fn operator_range(text: &str, focus: usize, plan: MotionPlan) -> Option<std::ops::Range<usize>> {
    if plan.linewise {
        let start = line_start(text, focus.min(plan.target));
        let last_line = line_start(text, focus.max(plan.target));
        let last = line_end(text, last_line);
        let end = if last < text.len() { last + 1 } else { last };
        return (start < end).then_some(start..end);
    }
    let (start, end) = if plan.target >= focus {
        (
            focus,
            if plan.inclusive {
                next_char(text, plan.target)
            } else {
                plan.target
            },
        )
    } else {
        (
            plan.target,
            if plan.inclusive {
                next_char(text, focus)
            } else {
                focus
            },
        )
    };
    let end = end.min(text.len());
    (start < end).then_some(start..end)
}

/// The line span touched by `range`, including each line's newline.
fn line_span(text: &str, range: std::ops::Range<usize>) -> (usize, usize) {
    let start = line_start(text, range.start);
    let last = if range.end > range.start {
        range.end - 1
    } else {
        range.start
    };
    let last_end = line_end(text, last);
    let end = if last_end < text.len() {
        last_end + 1
    } else {
        last_end
    };
    (start, end)
}

/// The plan for a motion used with an operator.
fn motion_plan(
    text: &str,
    focus: usize,
    key: &Key,
    count: usize,
    has_count: bool,
) -> Option<MotionPlan> {
    let (target, inclusive, linewise) = match key {
        Key::Char('h') | Key::Left => (move_col(text, focus, -(count as isize)), false, false),
        Key::Char('l') | Key::Right => (move_col(text, focus, count as isize), false, false),
        Key::Char('j') | Key::Down => (move_line(text, focus, count as isize, None), false, true),
        Key::Char('k') | Key::Up => (move_line(text, focus, -(count as isize), None), false, true),
        Key::Char('0') | Key::Home => (line_start(text, focus), false, false),
        Key::Char('^') => (first_non_blank(text, focus), false, false),
        Key::Char('$') | Key::End => (last_char(text, focus), true, false),
        Key::Char('w') => (word_forward(text, focus, count, false), false, false),
        Key::Char('W') => (word_forward(text, focus, count, true), false, false),
        Key::Char('b') => (word_back(text, focus, count, false), false, false),
        Key::Char('B') => (word_back(text, focus, count, true), false, false),
        Key::Char('e') => (word_end(text, focus, count, false), true, false),
        Key::Char('E') => (word_end(text, focus, count, true), true, false),
        Key::Char('{') => (paragraph_back(text, focus, count), false, false),
        Key::Char('}') => (paragraph_forward(text, focus, count), false, false),
        Key::Char('%') => (match_bracket(text, focus)?, true, false),
        Key::Char('G') => {
            let line = if has_count {
                count.saturating_sub(1)
            } else {
                line_count(text).saturating_sub(1)
            };
            (goto_line(text, line), false, true)
        }
        _ => return None,
    };
    Some(MotionPlan {
        target,
        inclusive,
        linewise,
    })
}

/// Indent or outdent the caret line and the next `count - 1` lines.
fn indent_lines(doc: &Doc<'_>, focus: usize, count: usize, outdent: bool) -> Vec<Effect> {
    let text = doc.text;
    let first = line_index(text, focus);
    let last = (first + count.saturating_sub(1)).min(line_count(text).saturating_sub(1));
    let start = line_start_of(text, first);
    let last_end = line_end(text, line_start_of(text, last));
    let end = if last_end < text.len() {
        last_end + 1
    } else {
        last_end
    };
    let Some(edit) = indent::plan_indent(text, start, end, doc.indent, outdent) else {
        return vec![];
    };
    vec![
        Effect::Replace {
            start: edit.start,
            end: edit.end,
            text: edit.text,
        },
        select_effect(edit.anchor.min(edit.focus)),
    ]
}

/// The replacements `J` needs: each join removes one newline and the next
/// line's leading whitespace, inserting one space when both sides need it.
fn join_edits(text: &str, focus: usize, joins: usize) -> Vec<(usize, usize, String)> {
    let mut edits = Vec::new();
    let mut line_end_at = line_end(text, focus);
    for _ in 0..joins {
        if line_end_at >= text.len() {
            break;
        }
        let next_start = line_end_at + 1;
        let next_end = line_end(text, next_start);
        let mut first = next_start;
        while first < next_end && matches!(char_at(text, first), ' ' | '\t') {
            first = next_char(text, first);
        }
        let before = &text[line_start(text, focus)..line_end_at];
        let space = !before.is_empty()
            && !before.ends_with(|c: char| c.is_whitespace())
            && first < next_end;
        edits.push((
            line_end_at,
            first,
            if space {
                " ".to_string()
            } else {
                String::new()
            },
        ));
        line_end_at = next_end;
    }
    edits
}

/// The byte range of a text object at the caret.
fn text_object_range(
    text: &str,
    at: usize,
    prefix: char,
    object: char,
) -> Option<std::ops::Range<usize>> {
    let around = prefix == 'a';
    match object {
        'w' => word_object(text, at, around, false),
        'W' => word_object(text, at, around, true),
        '"' | '\'' | '`' => quote_object(text, at, object, around),
        '(' | ')' | 'b' => bracket_object(text, at, '(', ')', around),
        '[' | ']' => bracket_object(text, at, '[', ']', around),
        '{' | '}' | 'B' => bracket_object(text, at, '{', '}', around),
        '<' | '>' => bracket_object(text, at, '<', '>', around),
        _ => None,
    }
}

fn word_object(text: &str, at: usize, around: bool, big: bool) -> Option<std::ops::Range<usize>> {
    let at = clamp_normal(text, at);
    let line_s = line_start(text, at);
    let line_e = line_end(text, at);
    let class_at = |i: usize| -> Class {
        let c = char_at(text, i);
        if big {
            if c.is_whitespace() {
                Class::Space
            } else {
                Class::Word
            }
        } else {
            class(c)
        }
    };
    let cls = class_at(at);
    let mut start = at;
    while start > line_s {
        let prev = prev_char(text, start);
        if class_at(prev) != cls {
            break;
        }
        start = prev;
    }
    let mut end = next_char(text, at);
    while end < line_e && class_at(end) == cls {
        end = next_char(text, end);
    }
    if around {
        if cls == Class::Space {
            let mut e = end;
            if e < text.len() && class_at(e) != Class::Space {
                let next_cls = class_at(e);
                e = next_char(text, e);
                while e < text.len() && class_at(e) == next_cls {
                    e = next_char(text, e);
                }
            }
            return Some(start..e);
        }
        let mut e = end;
        while e < text.len() {
            let c = char_at(text, e);
            if !c.is_whitespace() {
                break;
            }
            e = next_char(text, e);
            if c == '\n' {
                break;
            }
        }
        if e > end {
            return Some(start..e);
        }
        let mut s = start;
        while s > 0 {
            let p = prev_char(text, s);
            if !char_at(text, p).is_whitespace() {
                break;
            }
            s = p;
        }
        return Some(s..end);
    }
    Some(start..end)
}

fn quote_object(
    text: &str,
    at: usize,
    quote: char,
    around: bool,
) -> Option<std::ops::Range<usize>> {
    let line_s = line_start(text, at);
    let line_e = line_end(text, at);
    let mut pairs = Vec::new();
    let mut open: Option<usize> = None;
    let mut i = line_s;
    while i < line_e {
        let c = char_at(text, i);
        if c == '\\' {
            i = next_char(text, i);
            if i < line_e {
                i = next_char(text, i);
            }
            continue;
        }
        if c == quote {
            match open.take() {
                Some(s) => pairs.push((s, i)),
                None => open = Some(i),
            }
        }
        i = next_char(text, i);
    }
    let (s, e) = pairs
        .iter()
        .copied()
        .find(|(s, e)| *s < at && at < *e)
        .or_else(|| pairs.iter().copied().find(|(s, _)| *s >= at))?;
    if around {
        Some(s..next_char(text, e))
    } else {
        Some(next_char(text, s)..e)
    }
}

fn bracket_object(
    text: &str,
    at: usize,
    open: char,
    close: char,
    around: bool,
) -> Option<std::ops::Range<usize>> {
    let opener = if char_at(text, at) == open {
        at
    } else {
        let mut depth = 0i32;
        let mut i = at;
        let mut found = None;
        while i > 0 {
            i = prev_char(text, i);
            let ch = char_at(text, i);
            if ch == close {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    found = Some(i);
                    break;
                }
                depth -= 1;
            }
        }
        found?
    };
    let mut depth = 1i32;
    let mut i = opener;
    let closer = loop {
        i = next_char(text, i);
        if i >= text.len() {
            return None;
        }
        let ch = char_at(text, i);
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                break i;
            }
        }
    };
    if around {
        Some(opener..next_char(text, closer))
    } else {
        Some(next_char(text, opener)..closer)
    }
}

fn visual_step(vim: &mut VimState, doc: &Doc<'_>, focus: usize, key: Key) -> Vec<Effect> {
    let text = doc.text;
    let anchor = vim.visual_anchor.unwrap_or(focus);
    if key == Key::Escape {
        vim.mode = Mode::Normal;
        vim.visual_anchor = None;
        vim.pending = Pending::default();
        vim.goal_col = None;
        return select(clamp_normal(text, focus));
    }
    // A pending `f`/`t` takes the next key as its character.
    if let Some(kind) = vim.pending.find {
        vim.pending.find = None;
        let count = vim.pending.count.take().unwrap_or(1);
        let Some(ch) = key_char(&key) else {
            return vec![];
        };
        vim.last_find = Some(LastFind { kind, ch });
        return match find_motion(text, focus, kind, ch, count) {
            Some(at) => select(at),
            None => vec![],
        };
    }
    if vim.pending.g {
        vim.pending.g = false;
        let count = vim.pending.count.take();
        if key == Key::Char('g') {
            return select(goto_line(text, count.map_or(0, |n| n - 1)));
        }
        return vec![];
    }
    if let Key::Char(c) = key
        && c.is_ascii_digit()
        && (c != '0' || vim.pending.count.is_some())
    {
        let n = vim.pending.count.unwrap_or(0);
        vim.pending.count = Some((n * 10 + c.to_digit(10).unwrap() as usize).max(1));
        return vec![];
    }
    if vim.pending.z {
        vim.pending.z = false;
        vim.pending.count = None;
        return match key {
            Key::Char('z') => vec![Effect::Scroll(Scroll::Center)],
            Key::Char('t') => vec![Effect::Scroll(Scroll::Top)],
            Key::Char('b') => vec![Effect::Scroll(Scroll::Bottom)],
            _ => vec![],
        };
    }
    let count_opt = vim.pending.count;
    vim.pending.count = None;
    let count = count_opt.unwrap_or(1);
    // Operators act on the whole selection.
    if let Key::Char(c @ ('d' | 'c' | 'y' | '>' | '<' | 'x')) = key {
        let op = match c {
            'c' => Op::Change,
            'y' => Op::Yank,
            '>' => Op::Indent,
            '<' => Op::Outdent,
            _ => Op::Delete,
        };
        let linewise = vim.mode == Mode::VisualLine;
        let Some(range) = vim.visual_range(text, focus) else {
            return vec![];
        };
        vim.mode = Mode::Normal;
        vim.visual_anchor = None;
        vim.pending = Pending::default();
        return apply_operator_range(vim, doc, op, range, linewise, None);
    }
    let keeps_goal = matches!(&key, Key::Char('j') | Key::Char('k') | Key::Down | Key::Up);
    let target = match key {
        Key::Char('h') | Key::Left => Some(move_col(text, focus, -(count as isize))),
        Key::Char('l') | Key::Right => Some(move_col(text, focus, count as isize)),
        Key::Char('j') | Key::Down => {
            let at = move_line(text, focus, count as isize, vim.goal_col);
            vim.goal_col = Some(display_col(text, focus));
            Some(at)
        }
        Key::Char('k') | Key::Up => {
            let at = move_line(text, focus, -(count as isize), vim.goal_col);
            vim.goal_col = Some(display_col(text, focus));
            Some(at)
        }
        Key::Char('0') | Key::Home => Some(line_start(text, focus)),
        Key::Char('^') => Some(first_non_blank(text, focus)),
        Key::Char('$') | Key::End => Some(last_char(text, focus)),
        Key::Char('w') => Some(word_forward(text, focus, count, false)),
        Key::Char('W') => Some(word_forward(text, focus, count, true)),
        Key::Char('b') => Some(word_back(text, focus, count, false)),
        Key::Char('B') => Some(word_back(text, focus, count, true)),
        Key::Char('e') => Some(word_end(text, focus, count, false)),
        Key::Char('E') => Some(word_end(text, focus, count, true)),
        Key::Char('%') => match_bracket(text, focus),
        Key::Char('{') => Some(paragraph_back(text, focus, count)),
        Key::Char('}') => Some(paragraph_forward(text, focus, count)),
        Key::Char(';') => vim
            .last_find
            .and_then(|last| find_motion(text, focus, last.kind, last.ch, count)),
        Key::Char(',') => vim
            .last_find
            .and_then(|last| find_motion(text, focus, last.kind.reversed(), last.ch, count)),
        Key::Char('G') => Some(if count_opt.is_some() {
            goto_line(text, count - 1)
        } else {
            goto_line(text, line_count(text).saturating_sub(1))
        }),
        Key::Char('g') => {
            vim.pending.g = true;
            vim.pending.count = count_opt;
            return vec![];
        }
        Key::Char('z') => {
            vim.pending.z = true;
            vim.pending.count = count_opt;
            return vec![];
        }
        Key::Char('f') => {
            vim.pending.find = Some(FindKind::To);
            vim.pending.count = count_opt;
            return vec![];
        }
        Key::Char('t') => {
            vim.pending.find = Some(FindKind::Till);
            vim.pending.count = count_opt;
            return vec![];
        }
        Key::Char('F') => {
            vim.pending.find = Some(FindKind::ToBack);
            vim.pending.count = count_opt;
            return vec![];
        }
        Key::Char('T') => {
            vim.pending.find = Some(FindKind::TillBack);
            vim.pending.count = count_opt;
            return vec![];
        }
        Key::Char('o') => {
            vim.visual_anchor = Some(focus);
            vim.goal_col = None;
            return select(anchor);
        }
        Key::Char('i') | Key::Char('a') => return vec![],
        _ => None,
    };
    if !keeps_goal {
        vim.goal_col = None;
    }
    match target {
        Some(at) => select(at),
        None => vec![],
    }
}

/// The word under `at` for `*`, or `None` on punctuation/whitespace.
pub fn word_under_caret(text: &str, at: usize) -> Option<String> {
    let at = clamp_normal(text, at);
    if class(char_at(text, at)) != Class::Word {
        return None;
    }
    let line_s = line_start(text, at);
    let line_e = line_end(text, at);
    let mut start = at;
    while start > line_s {
        let prev = prev_char(text, start);
        if class(char_at(text, prev)) != Class::Word {
            break;
        }
        start = prev;
    }
    let mut end = next_char(text, at);
    while end < line_e && class(char_at(text, end)) == Class::Word {
        end = next_char(text, end);
    }
    Some(text[start..end].to_string())
}

fn select_effect(at: usize) -> Effect {
    Effect::Select {
        anchor: at,
        focus: at,
    }
}

fn select(at: usize) -> Vec<Effect> {
    vec![select_effect(at)]
}

fn enter_insert(vim: &mut VimState, at: usize) -> Vec<Effect> {
    vim.mode = Mode::Insert;
    vim.pending = Pending::default();
    vim.goal_col = None;
    select(at)
}

fn open_find(vim: &mut VimState, focus: usize, forward: bool) -> Vec<Effect> {
    vim.search_origin = Some(focus);
    vim.search_forward = forward;
    vec![Effect::Find(FindAction::Open { forward })]
}

fn repeat_find(
    vim: &mut VimState,
    text: &str,
    focus: usize,
    count: usize,
    reverse: bool,
) -> Vec<Effect> {
    let Some(last) = vim.last_find else {
        return vec![];
    };
    let kind = if reverse {
        last.kind.reversed()
    } else {
        last.kind
    };
    match find_motion(text, focus, kind, last.ch, count) {
        Some(at) => select(at),
        None => vec![],
    }
}

fn key_char(key: &Key) -> Option<char> {
    match key {
        Key::Char(c) => Some(*c),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Motions
// ---------------------------------------------------------------------------

fn motion_col(vim: &mut VimState, text: &str, focus: usize, delta: isize) -> Vec<Effect> {
    vim.goal_col = None;
    select(move_col(text, focus, delta))
}

fn motion_line(vim: &mut VimState, text: &str, focus: usize, delta: isize) -> Vec<Effect> {
    let at = move_line(text, focus, delta, vim.goal_col);
    vim.goal_col = Some(display_col(text, focus));
    select(at)
}

/// Horizontal movement within the line, clamped to its characters.
fn move_col(text: &str, focus: usize, delta: isize) -> usize {
    let mut at = focus;
    if delta > 0 {
        for _ in 0..delta {
            let end = line_end(text, at);
            if at >= end {
                break;
            }
            at = next_char(text, at);
        }
    } else {
        for _ in 0..delta.unsigned_abs() {
            let start = line_start(text, at);
            if at <= start {
                break;
            }
            at = prev_char(text, at);
        }
    }
    clamp_normal(text, at)
}

/// `j`/`k`: a target line with the goal display column, clamped.
fn move_line(text: &str, focus: usize, delta: isize, goal: Option<usize>) -> usize {
    let idx = line_index(text, focus) as isize;
    let last = line_count(text).saturating_sub(1) as isize;
    let target = (idx + delta).clamp(0, last) as usize;
    let col = goal.unwrap_or_else(|| display_col(text, focus));
    let start = line_start_of(text, target);
    byte_at_col(text, start, col)
}

fn display_col(text: &str, at: usize) -> usize {
    let start = line_start(text, at);
    let mut col = 0;
    let mut i = start;
    while i < at {
        let c = char_at(text, i);
        if c == '\t' {
            col = (col / TAB_STOP as usize + 1) * TAB_STOP as usize;
        } else {
            col += 1;
        }
        i = next_char(text, i);
    }
    col
}

fn byte_at_col(text: &str, start: usize, col: usize) -> usize {
    let end = line_end(text, start);
    let mut cur = 0;
    let mut i = start;
    while i < end {
        let c = char_at(text, i);
        let width = if c == '\t' {
            (cur / TAB_STOP as usize + 1) * TAB_STOP as usize - cur
        } else {
            1
        };
        if cur + width > col {
            return i;
        }
        cur += width;
        i = next_char(text, i);
    }
    if end > start {
        prev_char(text, end).max(start)
    } else {
        start
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Class {
    Word,
    Punct,
    Space,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Punct
    }
}

/// The next key after a run of the current class, skipping whitespace.
fn word_forward(text: &str, focus: usize, count: usize, big: bool) -> usize {
    let mut at = focus;
    for _ in 0..count {
        at = one_word_forward(text, at, big);
    }
    at
}

fn one_word_forward(text: &str, at: usize, big: bool) -> usize {
    let mut i = at;
    let Some(first) = char_at_opt(text, i) else {
        return at;
    };
    let cls = class(first);
    if cls != Class::Space {
        while let Some(c) = char_at_opt(text, i) {
            let stop = if big {
                class(c) == Class::Space
            } else {
                class(c) != cls
            };
            if stop {
                break;
            }
            i = next_char(text, i);
        }
    }
    while let Some(c) = char_at_opt(text, i) {
        if !c.is_whitespace() {
            break;
        }
        i = next_char(text, i);
    }
    clamp_normal(text, i)
}

fn word_back(text: &str, focus: usize, count: usize, big: bool) -> usize {
    let mut at = focus;
    for _ in 0..count {
        at = one_word_back(text, at, big);
    }
    at
}

fn one_word_back(text: &str, at: usize, big: bool) -> usize {
    let mut i = at;
    if i > 0 {
        i = prev_char(text, i);
    }
    while let Some(c) = char_at_opt(text, i) {
        if !c.is_whitespace() {
            break;
        }
        if i == 0 {
            break;
        }
        i = prev_char(text, i);
    }
    let Some(c) = char_at_opt(text, i) else {
        return at;
    };
    let cls = class(c);
    if cls != Class::Space {
        while i > 0 {
            let prev = prev_char(text, i);
            let Some(p) = char_at_opt(text, prev) else {
                break;
            };
            let same = if big {
                class(p) != Class::Space
            } else {
                class(p) == cls
            };
            if !same {
                break;
            }
            i = prev;
        }
    }
    i
}

fn word_end(text: &str, focus: usize, count: usize, big: bool) -> usize {
    let mut at = focus;
    for _ in 0..count {
        at = one_word_end(text, at, big);
    }
    at
}

fn one_word_end(text: &str, at: usize, big: bool) -> usize {
    let mut i = at;
    if let Some(c) = char_at_opt(text, i)
        && !c.is_whitespace()
    {
        i = next_char(text, i);
    }
    while let Some(c) = char_at_opt(text, i) {
        if !c.is_whitespace() {
            break;
        }
        i = next_char(text, i);
    }
    let Some(c) = char_at_opt(text, i) else {
        return at;
    };
    let cls = class(c);
    while i < text.len() {
        let next = next_char(text, i);
        let Some(n) = char_at_opt(text, next) else {
            break;
        };
        let same = if big {
            class(n) != Class::Space
        } else {
            class(n) == cls
        };
        if !same {
            break;
        }
        i = next;
    }
    clamp_normal(text, i)
}

fn find_motion(text: &str, focus: usize, kind: FindKind, ch: char, count: usize) -> Option<usize> {
    let start = line_start(text, focus);
    let end = line_end(text, focus);
    let mut hits = Vec::new();
    if kind.forward() {
        let mut i = next_char(text, focus);
        while i < end {
            if char_at(text, i) == ch {
                hits.push(i);
            }
            i = next_char(text, i);
        }
        hits.get(count.saturating_sub(1)).copied().map(|at| {
            if kind.till() {
                prev_char(text, at).max(start)
            } else {
                at
            }
        })
    } else {
        let mut i = focus;
        while i > start {
            i = prev_char(text, i);
            if char_at(text, i) == ch {
                hits.push(i);
            }
        }
        hits.get(count.saturating_sub(1)).copied().map(|at| {
            if kind.till() {
                next_char(text, at).min(end)
            } else {
                at
            }
        })
    }
}

fn is_bracket(c: char) -> bool {
    matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>')
}

fn bracket_pair(c: char) -> Option<(char, char)> {
    Some(match c {
        '(' | ')' => ('(', ')'),
        '[' | ']' => ('[', ']'),
        '{' | '}' => ('{', '}'),
        '<' | '>' => ('<', '>'),
        _ => return None,
    })
}

/// `%`: the bracket at or after the caret on its line, and its match.
fn match_bracket(text: &str, focus: usize) -> Option<usize> {
    let end = line_end(text, focus);
    let mut i = focus;
    let mut found = None;
    while i < end {
        let c = char_at(text, i);
        if is_bracket(c) {
            found = Some((i, c));
            break;
        }
        i = next_char(text, i);
    }
    let (pos, c) = found?;
    let (open, close) = bracket_pair(c)?;
    let forward = c == open;
    let mut depth = 1i32;
    let mut i = pos;
    if forward {
        while i < text.len() {
            i = next_char(text, i);
            let ch = char_at(text, i);
            if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    } else {
        while i > 0 {
            i = prev_char(text, i);
            let ch = char_at(text, i);
            if ch == close {
                depth += 1;
            } else if ch == open {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// `}`: the next empty line's start, else the last line's first non-blank.
fn paragraph_forward(text: &str, focus: usize, count: usize) -> usize {
    let mut at = focus;
    for _ in 0..count {
        let mut i = line_end(text, at);
        let mut found = None;
        while i < text.len() {
            i = next_char(text, i);
            let start = i;
            let end = line_end(text, start);
            if end == start {
                found = Some(start);
                break;
            }
            i = end;
        }
        at = match found {
            Some(at) => at,
            None => first_non_blank(
                text,
                line_start_of(text, line_count(text).saturating_sub(1)),
            ),
        };
    }
    at
}

/// `{`: the previous empty line's start, else the first line's first non-blank.
fn paragraph_back(text: &str, focus: usize, count: usize) -> usize {
    let mut at = focus;
    for _ in 0..count {
        let mut i = line_start(text, at);
        let mut found = None;
        while i > 0 {
            let prev_end = prev_char(text, i);
            let start = line_start(text, prev_end);
            if line_end(text, start) == start {
                found = Some(start);
                break;
            }
            i = start;
        }
        at = match found {
            Some(at) => at,
            None => first_non_blank(text, 0),
        };
    }
    at
}

fn goto_line(text: &str, idx: usize) -> usize {
    let idx = idx.min(line_count(text).saturating_sub(1));
    first_non_blank(text, line_start_of(text, idx))
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

fn boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn char_at(text: &str, at: usize) -> char {
    char_at_opt(text, at).unwrap_or('\0')
}

fn char_at_opt(text: &str, at: usize) -> Option<char> {
    if at >= text.len() {
        return None;
    }
    text[at..].chars().next()
}

fn next_char(text: &str, at: usize) -> usize {
    char_at_opt(text, at).map_or(at, |c| at + c.len_utf8())
}

fn prev_char(text: &str, at: usize) -> usize {
    text[..at].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn line_start(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text[at..].find('\n').map_or(text.len(), |i| at + i)
}

fn line_index(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text[..at].bytes().filter(|b| *b == b'\n').count()
}

fn line_count(text: &str) -> usize {
    1 + text.bytes().filter(|b| *b == b'\n').count()
}

fn line_start_of(text: &str, idx: usize) -> usize {
    let mut cur = 0;
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            if cur == idx {
                return start;
            }
            start = i + 1;
            cur += 1;
        }
    }
    start
}

fn first_non_blank(text: &str, at: usize) -> usize {
    let start = line_start(text, at);
    let end = line_end(text, at);
    let mut i = start;
    while i < end && matches!(char_at(text, i), ' ' | '\t') {
        i = next_char(text, i);
    }
    if i < end { i } else { start }
}

fn last_char(text: &str, at: usize) -> usize {
    let start = line_start(text, at);
    let end = line_end(text, at);
    if end > start {
        prev_char(text, end).max(start)
    } else {
        start
    }
}

fn insert_after(text: &str, focus: usize) -> usize {
    let end = line_end(text, focus);
    if focus < end {
        next_char(text, focus)
    } else {
        end
    }
}

/// Outside Insert the caret sits on a character: a newline or past-the-end
/// target becomes the previous character, and an empty line keeps its start.
fn clamp_normal(text: &str, at: usize) -> usize {
    let at = boundary(text, at);
    if at < text.len() && char_at(text, at) != '\n' {
        return at;
    }
    let start = line_start(text, at);
    if at > start {
        prev_char(text, at).max(start)
    } else {
        start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &str) -> Vec<Key> {
        s.chars().map(Key::Char).collect()
    }

    fn run(text: &str, at: usize, seq: &[Key]) -> (usize, VimState) {
        run_mode(text, at, Mode::Normal, seq)
    }

    fn run_mode(text: &str, at: usize, mode: Mode, seq: &[Key]) -> (usize, VimState) {
        let mut vim = VimState {
            enabled: true,
            mode,
            ..VimState::default()
        };
        let mut caret = at;
        for key in seq {
            let doc = Doc {
                text,
                focus: caret,
                indent: IndentUnit::Spaces(4),
            };
            let effects = step(&mut vim, &doc, key.clone());
            if let Some(Effect::Select { focus, .. }) = effects.first() {
                caret = *focus;
            }
        }
        (caret, vim)
    }

    fn at(text: &str, start: usize, seq: &str) -> usize {
        run(text, start, &keys(seq)).0
    }

    #[test]
    fn character_and_line_motions_clamp_to_characters() {
        assert_eq!(at("hello world", 0, "w"), 6);
        assert_eq!(at("hello world", 6, "b"), 0);
        assert_eq!(at("hello world", 0, "e"), 4);
        assert_eq!(at("hello world", 0, "3l"), 3);
        assert_eq!(at("one two three", 0, "2w"), 8);
        assert_eq!(at("ab\ncd", 0, "$"), 1);
        assert_eq!(at("ab\ncd", 3, "0"), 3);
        assert_eq!(at("  hi", 0, "^"), 2);
        assert_eq!(at("  hi", 3, "^"), 2);
        assert_eq!(at("a\nb\nc", 0, "G"), 4);
        assert_eq!(at("a\nb\nc", 4, "gg"), 0);
        assert_eq!(at("a\nb\nc", 0, "2G"), 2);
    }

    #[test]
    fn word_motions_cross_lines_without_landing_on_newlines() {
        assert_eq!(at("one\ntwo", 0, "w"), 4);
        assert_eq!(at("one\ntwo", 4, "b"), 0);
        assert_eq!(at("one\ntwo", 0, "e"), 2);
        assert_eq!(at("a b", 2, "e"), 2);
        assert_eq!(at("foo.bar", 0, "W"), 6);
        assert_eq!(at("foo.bar", 0, "w"), 3);
    }

    #[test]
    fn find_motions_and_repeat() {
        assert_eq!(at("hello world", 0, "fo"), 4);
        assert_eq!(at("hello world", 0, "to"), 3);
        assert_eq!(at("hello world", 0, "Fo"), 0);
        assert_eq!(at("hello world", 7, "Fo"), 4);
        assert_eq!(at("hello world", 7, "To"), 5);
        assert_eq!(at("hello world", 0, "fo;"), 7);
        assert_eq!(at("hello world", 0, "fo;,"), 4);
        assert_eq!(at("a-b-c-d", 0, "f-2;"), 5);
        // No match leaves the caret alone.
        assert_eq!(at("hello", 0, "fz"), 0);
    }

    #[test]
    fn brackets_and_paragraphs() {
        assert_eq!(at("(a [b] c)", 0, "%"), 8);
        assert_eq!(at("(a [b] c)", 8, "%"), 0);
        assert_eq!(at("a\n\nb", 0, "}"), 2);
        assert_eq!(at("a\n\nb", 4, "{"), 2);
        assert_eq!(at("a\nb", 0, "}"), 2);
        assert_eq!(at("a\nb", 2, "{"), 0);
    }

    #[test]
    fn vertical_motion_keeps_the_goal_column() {
        let text = "abcdef\nxy\nabcdef";
        // Column 3 on line 0, then a short line, then back to column 3.
        assert_eq!(at(text, 3, "j"), 8);
        assert_eq!(at(text, 3, "jj"), 13);
        assert_eq!(at(text, 13, "k"), 8);
        assert_eq!(at(text, 13, "kk"), 3);
        // A horizontal motion clears the goal column.
        assert_eq!(at(text, 3, "jlk"), 1);
    }

    #[test]
    fn empty_lines_and_edges_stay_on_characters() {
        assert_eq!(at("a\n\nb", 2, "$"), 2);
        assert_eq!(at("a\n\nb", 2, "l"), 2);
        assert_eq!(at("a\n\nb", 2, "h"), 2);
        assert_eq!(at("", 0, "w"), 0);
        assert_eq!(at("", 0, "$"), 0);
        assert_eq!(at("a", 0, "l"), 0);
    }

    #[test]
    fn counts_and_escape() {
        let (caret, vim) = run("abc def", 0, &[Key::Char('2'), Key::Escape]);
        assert_eq!(caret, 0);
        assert_eq!(vim.pending_label(), "");
        // After a dropped count, `l` moves once.
        assert_eq!(
            run("abc def", 0, &[Key::Char('2'), Key::Escape, Key::Char('l')]).0,
            1
        );
        assert_eq!(run("abc def", 0, &keys("2l")).0, 2);
        assert_eq!(run("abc def", 0, &keys("12l")).0, 6);
    }

    #[test]
    fn insert_entries_and_escape_clamp() {
        let (caret, vim) = run("ab", 0, &keys("i"));
        assert_eq!(caret, 0);
        assert_eq!(vim.mode, Mode::Insert);
        let (caret, vim) = run("ab", 0, &keys("a"));
        assert_eq!(caret, 1);
        assert_eq!(vim.mode, Mode::Insert);
        let (caret, vim) = run("  hi", 0, &keys("I"));
        assert_eq!(caret, 2);
        assert_eq!(vim.mode, Mode::Insert);
        let (caret, vim) = run("ab", 0, &keys("A"));
        assert_eq!(caret, 2);
        assert_eq!(vim.mode, Mode::Insert);
        // Escape from Insert clamps onto the last character.
        let (caret, vim) = run_mode("ab", 2, Mode::Insert, &[Key::Escape]);
        assert_eq!(caret, 1);
        assert_eq!(vim.mode, Mode::Normal);
        // An empty line keeps byte 0.
        let (caret, vim) = run_mode("a\n\nb", 2, Mode::Insert, &[Key::Escape]);
        assert_eq!(caret, 2);
        assert_eq!(vim.mode, Mode::Normal);
    }

    #[test]
    fn insert_passes_typing_to_the_field() {
        let mut vim = VimState {
            enabled: true,
            mode: Mode::Insert,
            ..VimState::default()
        };
        let doc = Doc {
            text: "ab",
            focus: 1,
            indent: IndentUnit::Spaces(4),
        };
        assert_eq!(step(&mut vim, &doc, Key::Char('x')), vec![Effect::Pass]);
        assert_eq!(step(&mut vim, &doc, Key::Tab), vec![Effect::Pass]);
        assert_eq!(step(&mut vim, &doc, Key::Enter), vec![Effect::Pass]);
    }

    #[test]
    fn disabled_engine_passes_everything() {
        let mut vim = VimState::default();
        let doc = Doc {
            text: "ab",
            focus: 0,
            indent: IndentUnit::Spaces(4),
        };
        assert_eq!(step(&mut vim, &doc, Key::Char('h')), vec![Effect::Pass]);
    }

    #[test]
    fn unknown_keys_are_consumed_without_moving() {
        let (caret, vim) = run("abc", 1, &[Key::Char('Q')]);
        assert_eq!(caret, 1);
        assert_eq!(vim.mode, Mode::Normal);
        assert_eq!(vim.pending_label(), "");
    }

    #[test]
    fn pending_label_shows_counts_and_find() {
        let (_, vim) = run("abc", 0, &[Key::Char('2')]);
        assert_eq!(vim.pending_label(), "2");
        let (_, vim) = run("abc", 0, &keys("f"));
        assert_eq!(vim.pending_label(), "f");
        let (_, vim) = run("abc", 0, &keys("g"));
        assert_eq!(vim.pending_label(), "g");
    }

    #[test]
    fn search_and_command_open_effects() {
        let (_, vim) = run("abc", 1, &keys("/"));
        assert_eq!(vim.search_origin, Some(1));
        assert!(vim.search_forward);
        let (_, vim) = run("abc", 1, &keys("?"));
        assert!(!vim.search_forward);
        // The command bar leaves the search state alone.
        let (_, vim) = run("abc", 1, &keys("/:"));
        assert_eq!(vim.search_origin, Some(1));
    }

    /// A document the effects are applied to, so operator results are read as
    /// text instead of caret positions.
    struct Buffer {
        text: String,
        caret: usize,
        vim: VimState,
        register: Register,
    }

    impl Buffer {
        fn new(text: &str) -> Buffer {
            Buffer {
                text: text.to_string(),
                caret: 0,
                vim: VimState {
                    enabled: true,
                    ..VimState::default()
                },
                register: Register::default(),
            }
        }

        fn key(&mut self, key: Key) -> Vec<Effect> {
            let doc = Doc {
                text: &self.text,
                focus: self.caret,
                indent: IndentUnit::Spaces(4),
            };
            let effects = step(&mut self.vim, &doc, key);
            self.apply(&effects);
            effects
        }

        fn keys(&mut self, seq: &str) {
            for c in seq.chars() {
                self.key(Key::Char(c));
            }
        }

        fn apply(&mut self, effects: &[Effect]) {
            for effect in effects {
                match effect {
                    Effect::Select { focus, .. } => self.caret = *focus,
                    Effect::Replace { start, end, text } => {
                        self.text.replace_range(*start..*end, text);
                        self.caret = (*start + text.len()).min(self.text.len());
                    }
                    Effect::Yank { text, linewise } => self.register.set(text.clone(), *linewise),
                    Effect::Paste { before, count } => {
                        let text = self.register.text.clone();
                        if text.is_empty() {
                            continue;
                        }
                        let at = if self.register.linewise {
                            if *before {
                                line_start(&self.text, self.caret)
                            } else {
                                let end = line_end(&self.text, self.caret);
                                if end < self.text.len() { end + 1 } else { end }
                            }
                        } else if *before {
                            self.caret
                        } else {
                            next_char(&self.text, self.caret)
                        };
                        let insert = if self.register.linewise && !text.ends_with('\n') {
                            format!("{text}\n")
                        } else {
                            text.clone()
                        };
                        let mut at = at;
                        for _ in 0..*count {
                            self.text.insert_str(at, &insert);
                            at += insert.len();
                        }
                        self.caret = if self.register.linewise {
                            line_start(&self.text, at.saturating_sub(1))
                        } else {
                            at.saturating_sub(1).min(self.text.len().saturating_sub(1))
                        };
                    }
                    _ => {}
                }
            }
        }

        /// Simulate typing `text` in Insert and leaving it with Escape.
        fn insert(&mut self, text: &str) {
            let at = self.caret;
            self.text.insert_str(at, text);
            self.caret = at + text.len();
            self.key(Key::Escape);
            self.vim.record_insert(text);
        }
    }

    #[test]
    fn doubled_operators_delete_change_and_yank_lines() {
        let mut b = Buffer::new("one\ntwo\nthree\n");
        b.caret = 4;
        b.keys("dd");
        assert_eq!(b.text, "one\nthree\n");
        assert_eq!(b.caret, 4);
        assert!(b.register.linewise);
        assert_eq!(b.register.text, "two\n");

        let mut b = Buffer::new("one\ntwo\nthree\nfour\n");
        b.keys("2dd");
        assert_eq!(b.text, "three\nfour\n");

        let mut b = Buffer::new("one\ntwo\n");
        b.caret = 4;
        b.keys("yy");
        assert_eq!(b.text, "one\ntwo\n");
        assert!(b.register.linewise);
        assert_eq!(b.register.text, "two\n");
        assert_eq!(b.caret, 4);
    }

    #[test]
    fn operator_motions_cover_the_right_text() {
        let mut b = Buffer::new("one two three");
        b.keys("dw");
        assert_eq!(b.text, "two three");

        let mut b = Buffer::new("one two");
        b.keys("de");
        assert_eq!(b.text, " two");

        let mut b = Buffer::new("one\ntwo");
        b.caret = 1;
        b.keys("d$");
        assert_eq!(b.text, "o\ntwo");

        let mut b = Buffer::new("one\ntwo");
        b.caret = 1;
        b.keys("D");
        assert_eq!(b.text, "o\ntwo");

        let mut b = Buffer::new("a b c d e f g h");
        b.keys("2d3w");
        assert_eq!(b.text, "g h");

        let mut b = Buffer::new("one\ntwo\nthree\n");
        b.caret = 4;
        b.keys("dj");
        assert_eq!(b.text, "one\n");
    }

    #[test]
    fn operators_over_text_objects() {
        let mut b = Buffer::new("one two");
        b.caret = 1;
        b.keys("ciw");
        assert_eq!(b.text, " two");
        assert_eq!(b.vim.mode, Mode::Insert);
        b.insert("X");
        assert_eq!(b.text, "X two");
        assert_eq!(b.vim.mode, Mode::Normal);
        // `.` repeats the change on the next word.
        b.caret = 4;
        b.keys(".");
        assert_eq!(b.text, "X X");

        let mut b = Buffer::new("say \"hi\" now");
        b.caret = 5;
        b.keys("yi\"");
        assert_eq!(b.text, "say \"hi\" now");
        assert_eq!(b.register.text, "hi");
        assert!(!b.register.linewise);

        let mut b = Buffer::new("say \"hi\" now");
        b.caret = 5;
        b.keys("ci\"");
        b.insert("bye");
        assert_eq!(b.text, "say \"bye\" now");

        let mut b = Buffer::new("(a [b] c)");
        b.keys("d%");
        assert_eq!(b.text, "");
    }

    #[test]
    fn single_key_edits_and_repeat() {
        let mut b = Buffer::new("abcdef");
        b.keys("x");
        assert_eq!(b.text, "bcdef");
        b.keys(".");
        assert_eq!(b.text, "cdef");

        let mut b = Buffer::new("a\nb");
        b.keys("J");
        assert_eq!(b.text, "a b");
        assert_eq!(b.caret, 1);

        let mut b = Buffer::new("a\nb\nc");
        b.keys(">>");
        assert_eq!(b.text, "    a\nb\nc");

        let mut b = Buffer::new("a\nb\nc");
        b.keys(">j");
        assert_eq!(b.text, "    a\n    b\nc");

        let mut b = Buffer::new("a\nb");
        b.keys("<<");
        assert_eq!(
            b.text, "a\nb",
            "outdenting an unindented line changes nothing"
        );
    }

    #[test]
    fn insert_entries_and_paste() {
        let mut b = Buffer::new("one\ntwo\n");
        b.keys("yy");
        b.keys("p");
        assert_eq!(b.text, "one\none\ntwo\n");

        let mut b = Buffer::new("ab");
        b.caret = 0;
        b.keys("a");
        assert_eq!(b.vim.mode, Mode::Insert);
        assert_eq!(b.caret, 1);

        let mut b = Buffer::new("a");
        b.keys("o");
        assert_eq!(b.text, "a\n");
        assert_eq!(b.vim.mode, Mode::Insert);
        assert_eq!(b.caret, 2);

        let mut b = Buffer::new("a");
        b.keys("O");
        assert_eq!(b.text, "\na");
        assert_eq!(b.vim.mode, Mode::Insert);
        assert_eq!(b.caret, 0);

        let mut b = Buffer::new("abcdef");
        b.caret = 2;
        b.keys("A");
        assert_eq!(b.caret, 6);
        b.insert("X");
        assert_eq!(b.text, "abcdefX");
        b.caret = 0;
        b.keys(".");
        assert_eq!(b.text, "XabcdefX");
    }

    #[test]
    fn zz_zt_zb_ask_for_scroll_without_moving() {
        let mut b = Buffer::new("one\ntwo\nthree\n");
        b.caret = 4;
        assert_eq!(b.key(Key::Char('z')), vec![]);
        assert_eq!(b.vim.pending_label(), "z");
        assert_eq!(b.key(Key::Char('z')), vec![Effect::Scroll(Scroll::Center)]);
        assert_eq!(b.caret, 4, "scrolling never moves the caret");
        b.key(Key::Char('z'));
        assert_eq!(b.key(Key::Char('t')), vec![Effect::Scroll(Scroll::Top)]);
        b.key(Key::Char('z'));
        assert_eq!(b.key(Key::Char('b')), vec![Effect::Scroll(Scroll::Bottom)]);
        b.key(Key::Char('z'));
        assert_eq!(
            b.key(Key::Char('x')),
            vec![],
            "an unknown z command is dropped"
        );
        assert_eq!(b.vim.pending_label(), "");
    }

    #[test]
    fn visual_modes_select_and_operate() {
        let mut b = Buffer::new("one two three");
        b.keys("v");
        assert_eq!(b.vim.mode, Mode::Visual);
        b.keys("e");
        assert_eq!(b.vim.visual_range(&b.text, b.caret), Some(0..3));
        b.keys("d");
        assert_eq!(b.text, " two three");
        assert_eq!(b.vim.mode, Mode::Normal);

        // Visual Line deletes whole lines.
        let mut b = Buffer::new("one\ntwo\nthree\n");
        b.caret = 4;
        b.keys("V");
        b.keys("j");
        assert_eq!(b.vim.visual_range(&b.text, b.caret), Some(4..14));
        b.keys("d");
        assert_eq!(b.text, "one\n");

        // `o` swaps ends and `y` yanks the selection.
        let mut b = Buffer::new("one two");
        b.keys("v");
        b.keys("e");
        b.keys("o");
        assert_eq!(b.caret, 0);
        b.keys("y");
        assert_eq!(b.register.text, "one");
        assert_eq!(b.text, "one two");
        assert_eq!(b.vim.mode, Mode::Normal);

        // A Visual change enters Insert and leaves Normal on Escape.
        let mut b = Buffer::new("one two");
        b.keys("v");
        b.keys("e");
        b.keys("c");
        assert_eq!(b.text, " two");
        assert_eq!(b.vim.mode, Mode::Insert);
        b.insert("X");
        assert_eq!(b.text, "X two");
        assert_eq!(b.vim.mode, Mode::Normal);
    }

    #[test]
    fn undo_redo_and_counts_are_effects() {
        let mut b = Buffer::new("abc");
        assert_eq!(b.key(Key::Char('u')), vec![Effect::Undo]);
        assert_eq!(b.key(Key::Ctrl('r')), vec![Effect::Redo]);
        b.keys("2x");
        assert_eq!(b.text, "c");
        let mut b = Buffer::new("abc");
        b.keys("2");
        assert_eq!(b.vim.pending_label(), "2");
        b.keys("d");
        assert_eq!(b.vim.pending_label(), "2d");
        b.keys("d");
        assert_eq!(b.text, "");
    }
}
