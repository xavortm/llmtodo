//! Markdown highlighting with `+++` sections as the fold ranges.
//!
//! GPUI Kit's code editor asks its highlighter for fold ranges. The built-in
//! tree-sitter highlighter folds syntax nodes, and the Markdown grammar does
//! not know `+++`. This highlighter keeps GPUI Kit's Markdown highlighting
//! and swaps in the section ranges from [`crate::task::sections`].
//!
//! The parsing follows GPUI Kit's own `TreeSitterInputHighlighter` in
//! `gpui-component/crates/component/src/highlighter/input_adapter.rs`
//! (Apache-2.0, longbridge/gpui-component at `1029e2c`). It passes each edit
//! to tree-sitter, parses for at most 2 ms on the main thread, and finishes
//! a longer parse on a background thread. `to_tree_sitter_edit` is copied
//! from that file. The background parse differs: GPUI Kit's helpers for it
//! are private, so this file parses a fresh highlighter there and swaps it
//! in.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use gpui_kit::component::highlighter::SyntaxHighlighter;
use gpui_kit::{Context, HighlightStyle, SharedString, Task, Window};

use crate::task::sections;

pub const LANGUAGE: &str = "markdown";

/// The longest parse on the main thread for one edit.
const SYNC_PARSE_TIMEOUT: Duration = Duration::from_millis(2);
/// Above this size, edits only move the old tree, and every parse runs on
/// a background thread.
const SYNC_PARSE_MAX_BYTES: usize = 256 * 1024;
/// A background parse waits for typing to pause this long.
const PARSE_DEBOUNCE: Duration = Duration::from_millis(150);

/// Install with `EditorState::set_highlighter_factory` before the editor
/// first renders, so GPUI Kit does not install its own.
pub fn factory() -> InputHighlighterFactory {
    Rc::new(|language| {
        (language == LANGUAGE)
            .then(|| Box::new(SectionFoldHighlighter::new()) as Box<dyn InputHighlighter>)
    })
}

pub fn fold_ranges(text: &str) -> Vec<FoldRange> {
    to_folds(sections::section_lines(text))
}

/// `fold_ranges` for the editor's rope, with no copy of the text.
pub fn fold_ranges_of_rope(text: &Rope) -> Vec<FoldRange> {
    to_folds(sections::section_lines_of_rope(text))
}

fn to_folds(sections: Vec<sections::SectionLines>) -> Vec<FoldRange> {
    sections
        .into_iter()
        // A fold keeps its first and last line visible, so it needs a line
        // between them to hide anything.
        .filter(|s| s.end_line > s.start_line + 1)
        .map(|s| FoldRange::new(s.start_line, s.end_line))
        .collect()
}

/// Copied from GPUI Kit's `input_adapter.rs` (Apache-2.0).
fn to_tree_sitter_edit(edit: InputEdit) -> tree_sitter::InputEdit {
    use tree_sitter::Point;
    tree_sitter::InputEdit {
        start_byte: edit.start_byte,
        old_end_byte: edit.old_end_byte,
        new_end_byte: edit.new_end_byte,
        start_position: Point::new(edit.start_position.row, edit.start_position.column),
        old_end_position: Point::new(edit.old_end_position.row, edit.old_end_position.column),
        new_end_position: Point::new(edit.new_end_position.row, edit.new_end_position.column),
    }
}

struct SectionFoldHighlighter {
    inner: Rc<RefCell<SyntaxHighlighter>>,
    /// The background parse on its way. A new one replaces it, and
    /// dropping it cancels it.
    parse_task: Rc<RefCell<Option<Task<()>>>>,
    /// Counts updates, so a background parse of older text never lands.
    generation: Rc<Cell<u64>>,
}

impl SectionFoldHighlighter {
    fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(SyntaxHighlighter::new(LANGUAGE))),
            parse_task: Rc::default(),
            generation: Rc::default(),
        }
    }

    /// After the edits reached the tree: drops a background parse that a
    /// finished parse made needless, or starts one for `text`. Then sets
    /// the fold candidates.
    fn finish_update(
        &mut self,
        completed: bool,
        text: &Rope,
        folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        if completed {
            self.parse_task.borrow_mut().take();
        } else {
            self.parse_later(generation, text.clone(), window, cx);
        }
        // One edit can re-pair every `+++` marker below it, so the
        // incremental fold update cannot be trusted. Replace all candidates
        // after the editor finishes this edit. Folded sections that still
        // exist stay folded. The scan reads each line's start only.
        if folding {
            let folds = fold_ranges_of_rope(text);
            cx.defer_in(window, move |state, _, cx| {
                state.apply_highlighter_fold_candidates(folds, cx);
            });
        }
    }

    /// Parses `text` from scratch on a background thread once typing
    /// pauses, and swaps the result in if no edit came meanwhile.
    fn parse_later(
        &self,
        generation: u64,
        text: Rope,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let inner = self.inner.clone();
        let current = self.generation.clone();
        let task = cx.spawn_in(window, async move |editor, cx| {
            cx.background_executor().timer(PARSE_DEBOUNCE).await;
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    let mut fresh = SyntaxHighlighter::new(LANGUAGE);
                    fresh.update(None, &text, None);
                    fresh
                })
                .await;
            if current.get() == generation {
                *inner.borrow_mut() = parsed;
                editor.update(cx, |_, cx| cx.notify()).ok();
            }
        });
        self.parse_task.borrow_mut().replace(task);
    }
}

impl InputHighlighter for SectionFoldHighlighter {
    fn language(&self) -> SharedString {
        self.inner.borrow().language().clone()
    }

    fn update(
        &mut self,
        edit: Option<InputEdit>,
        text: &Rope,
        folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let edit = edit.map(to_tree_sitter_edit);
        let completed = {
            let mut inner = self.inner.borrow_mut();
            if text.len() > SYNC_PARSE_MAX_BYTES {
                inner.edit_tree(edit, text);
                false
            } else {
                inner.update(edit, text, Some(SYNC_PARSE_TIMEOUT))
            }
        };
        self.finish_update(completed, text, folding, window, cx);
    }

    /// Several edits made as one change: the tree takes each one, then
    /// parses once.
    fn update_batch(
        &mut self,
        edits: &[(InputEdit, Rope)],
        folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let Some(((last, text), before)) = edits.split_last() else {
            return;
        };
        let completed = {
            let mut inner = self.inner.borrow_mut();
            for (edit, text) in before {
                inner.edit_tree(Some(to_tree_sitter_edit(*edit)), text);
            }
            let last = Some(to_tree_sitter_edit(*last));
            if text.len() > SYNC_PARSE_MAX_BYTES {
                inner.edit_tree(last, text);
                false
            } else {
                inner.update(last, text, Some(SYNC_PARSE_TIMEOUT))
            }
        };
        self.finish_update(completed, text, folding, window, cx);
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        self.inner.borrow().styles(range, resolver)
    }

    fn fold_ranges(&self, text: &Rope) -> Vec<FoldRange> {
        fold_ranges_of_rope(text)
    }

    fn fold_ranges_for_edit(&self, _range: Range<usize>, _text: &Rope) -> Vec<FoldRange> {
        // The deferred full update in `finish_update` sets the real
        // candidates.
        Vec::new()
    }
}
