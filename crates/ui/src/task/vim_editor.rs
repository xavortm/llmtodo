//! A small vim layer over GPUI Kit's code editor.
//!
//! GPUI Kit's editor has no vim mode. The workspace wraps the editor in an
//! element with the key context `VimEditor vim=normal|insert`. In normal
//! mode:
//!
//! - Motions bind straight to the editor's own actions, such as
//!   `input::MoveLeft`.
//! - Edits that the editor has no action for, such as `dd`, `o` and `p`, are
//!   planned here as plain text operations and applied with
//!   `EditorState::replace`.
//! - The wrapper swallows key-down events that no binding handled. On macOS,
//!   GPUI types a character only when its key-down event propagates, so
//!   unbound letters do not type.

use std::ops::Range;

use gpui_kit::component::input::{self as ed, EditorState};
use gpui_kit::{App, Context, KeyBinding, Window, actions};

actions!(
    vim,
    [
        Insert,
        Append,
        AppendEnd,
        InsertStart,
        OpenBelow,
        OpenAbove,
        DeleteChar,
        DeleteLine,
        DeleteToEnd,
        YankLine,
        Paste,
        LineStart,
        FirstNonBlank,
        LineEnd,
    ]
);

pub const CONTEXT: &str = "VimEditor";
pub const NORMAL: &str = "VimEditor && vim == normal";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
}

impl VimMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Insert => "insert",
        }
    }
}

pub fn bind_keys(cx: &mut App) {
    let n = Some(NORMAL);
    cx.bind_keys([
        KeyBinding::new("h", ed::MoveLeft, n),
        KeyBinding::new("l", ed::MoveRight, n),
        KeyBinding::new("j", ed::MoveDown, n),
        KeyBinding::new("k", ed::MoveUp, n),
        KeyBinding::new("w", ed::MoveToNextWord, n),
        KeyBinding::new("b", ed::MoveToPreviousWord, n),
        KeyBinding::new("g g", ed::MoveToStart, n),
        KeyBinding::new("shift-g", ed::MoveToEnd, n),
        KeyBinding::new("u", ed::Undo, n),
        KeyBinding::new("ctrl-r", ed::Redo, n),
        KeyBinding::new("0", LineStart, n),
        KeyBinding::new("^", FirstNonBlank, n),
        KeyBinding::new("$", LineEnd, n),
        KeyBinding::new("i", Insert, n),
        KeyBinding::new("a", Append, n),
        KeyBinding::new("shift-a", AppendEnd, n),
        KeyBinding::new("shift-i", InsertStart, n),
        KeyBinding::new("o", OpenBelow, n),
        KeyBinding::new("shift-o", OpenAbove, n),
        KeyBinding::new("x", DeleteChar, n),
        KeyBinding::new("d d", DeleteLine, n),
        KeyBinding::new("shift-d", DeleteToEnd, n),
        KeyBinding::new("y y", YankLine, n),
        KeyBinding::new("p", Paste, n),
    ]);
}

/// The normal-mode commands that change text or the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Insert,
    Append,
    AppendEnd,
    InsertStart,
    OpenBelow,
    OpenAbove,
    DeleteChar,
    DeleteLine,
    DeleteToEnd,
    YankLine,
    Paste,
    LineStart,
    FirstNonBlank,
    LineEnd,
}

/// What a command does to the text, worked out before it touches the editor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Replace this byte range with `text`. `None` leaves the text alone.
    pub replace: Option<(Range<usize>, String)>,
    /// The cursor offset afterwards, in the new text.
    pub cursor: usize,
    /// The new register contents, when the command yanks or deletes.
    pub register: Option<String>,
    pub enter_insert: bool,
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |i| at + i)
}

fn next_char(text: &str, at: usize) -> usize {
    at + text[at..].chars().next().map_or(0, char::len_utf8)
}

fn first_non_blank(text: &str, at: usize) -> usize {
    let start = line_start(text, at);
    let end = line_end(text, at);
    start
        + text[start..end]
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(0)
}

/// The current line with its line break, as `dd` and `yy` take it. On the
/// last line the break before it goes instead, so no empty line remains.
fn whole_line(text: &str, at: usize) -> Range<usize> {
    let start = line_start(text, at);
    let end = line_end(text, at);
    if end < text.len() {
        start..end + 1
    } else if start > 0 {
        start - 1..end
    } else {
        start..end
    }
}

pub fn plan(command: Command, text: &str, cursor: usize, register: Option<&str>) -> Plan {
    let cursor = cursor.min(text.len());
    let moved = |to: usize| Plan {
        cursor: to,
        ..Plan::default()
    };
    let insert_at = |to: usize| Plan {
        cursor: to,
        enter_insert: true,
        ..Plan::default()
    };
    match command {
        Command::Insert => insert_at(cursor),
        Command::Append => insert_at(next_char(text, cursor).min(line_end(text, cursor))),
        Command::AppendEnd => insert_at(line_end(text, cursor)),
        Command::InsertStart => insert_at(first_non_blank(text, cursor)),
        Command::LineStart => moved(line_start(text, cursor)),
        Command::FirstNonBlank => moved(first_non_blank(text, cursor)),
        Command::LineEnd => moved(line_end(text, cursor)),
        Command::OpenBelow => {
            let end = line_end(text, cursor);
            Plan {
                replace: Some((end..end, "\n".into())),
                cursor: end + 1,
                enter_insert: true,
                ..Plan::default()
            }
        }
        Command::OpenAbove => {
            let start = line_start(text, cursor);
            Plan {
                replace: Some((start..start, "\n".into())),
                cursor: start,
                enter_insert: true,
                ..Plan::default()
            }
        }
        Command::DeleteChar => {
            if cursor >= line_end(text, cursor) {
                return moved(cursor);
            }
            let end = next_char(text, cursor);
            Plan {
                replace: Some((cursor..end, String::new())),
                cursor,
                register: Some(text[cursor..end].to_string()),
                ..Plan::default()
            }
        }
        Command::DeleteToEnd => {
            let end = line_end(text, cursor);
            Plan {
                replace: Some((cursor..end, String::new())),
                cursor,
                register: Some(text[cursor..end].to_string()),
                ..Plan::default()
            }
        }
        Command::DeleteLine => {
            let range = whole_line(text, cursor);
            let line = &text[line_start(text, cursor)..line_end(text, cursor)];
            let after = range.start.min(text.len() - range.len());
            let mut new_text = text.to_string();
            new_text.replace_range(range.clone(), "");
            Plan {
                replace: Some((range, String::new())),
                cursor: line_start(&new_text, after.min(new_text.len())),
                register: Some(format!("{line}\n")),
                ..Plan::default()
            }
        }
        Command::YankLine => {
            let line = &text[line_start(text, cursor)..line_end(text, cursor)];
            Plan {
                cursor,
                register: Some(format!("{line}\n")),
                ..Plan::default()
            }
        }
        Command::Paste => match register {
            // Whole lines go below the current line.
            Some(reg) if reg.ends_with('\n') => {
                let end = line_end(text, cursor);
                Plan {
                    replace: Some((end..end, format!("\n{}", reg.trim_end_matches('\n')))),
                    cursor: end + 1,
                    ..Plan::default()
                }
            }
            Some(reg) => {
                let at = next_char(text, cursor).min(line_end(text, cursor));
                Plan {
                    replace: Some((at..at, reg.to_string())),
                    cursor: at + reg.len(),
                    ..Plan::default()
                }
            }
            None => moved(cursor),
        },
    }
}

/// Runs a planned command on the editor. Returns the plan so the caller can
/// keep the register and switch modes.
pub fn apply(
    command: Command,
    editor: &mut EditorState,
    register: Option<&str>,
    window: &mut Window,
    cx: &mut Context<EditorState>,
) -> Plan {
    let text = editor.value().to_string();
    let cursor = editor.cursor();
    let plan = plan(command, &text, cursor, register);
    if let Some((range, new_text)) = &plan.replace {
        editor.set_selected_range(range.clone(), cx);
        editor.replace(new_text.clone(), window, cx);
    }
    editor.set_selected_range(plan.cursor..plan.cursor, cx);
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "one\n  two\nthree";

    fn run(command: Command, cursor: usize, register: Option<&str>) -> (String, Plan) {
        let plan = plan(command, TEXT, cursor, register);
        let mut text = TEXT.to_string();
        if let Some((range, new)) = &plan.replace {
            text.replace_range(range.clone(), new);
        }
        (text, plan)
    }

    #[test]
    fn dd_removes_a_middle_line_and_yanks_it() {
        let (text, plan) = run(Command::DeleteLine, 6, None);
        assert_eq!(text, "one\nthree");
        assert_eq!(plan.register.as_deref(), Some("  two\n"));
        assert_eq!(plan.cursor, 4);
    }

    #[test]
    fn dd_on_the_last_line_leaves_no_blank_line() {
        let (text, plan) = run(Command::DeleteLine, 12, None);
        assert_eq!(text, "one\n  two");
        assert_eq!(plan.cursor, 4);
    }

    #[test]
    fn x_stops_at_the_line_end() {
        assert_eq!(run(Command::DeleteChar, 0, None).0, "ne\n  two\nthree");
        assert_eq!(run(Command::DeleteChar, 3, None).0, TEXT);
    }

    #[test]
    fn o_and_capital_o_open_lines() {
        let (text, plan) = run(Command::OpenBelow, 1, None);
        assert_eq!(text, "one\n\n  two\nthree");
        assert_eq!(plan.cursor, 4);
        assert!(plan.enter_insert);
        let (text, plan) = run(Command::OpenAbove, 6, None);
        assert_eq!(text, "one\n\n  two\nthree");
        assert_eq!(plan.cursor, 4);
    }

    #[test]
    fn p_pastes_a_whole_line_below() {
        let (text, plan) = run(Command::Paste, 0, Some("new\n"));
        assert_eq!(text, "one\nnew\n  two\nthree");
        assert_eq!(plan.cursor, 4);
    }

    #[test]
    fn motions_find_line_edges() {
        assert_eq!(run(Command::FirstNonBlank, 9, None).1.cursor, 6);
        assert_eq!(run(Command::LineStart, 9, None).1.cursor, 4);
        assert_eq!(run(Command::LineEnd, 4, None).1.cursor, 9);
    }
}
