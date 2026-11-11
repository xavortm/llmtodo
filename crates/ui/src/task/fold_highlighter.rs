//! Markdown highlighting with `+++` sections as the fold ranges.
//!
//! GPUI Kit's code editor asks its highlighter for fold ranges. The built-in
//! tree-sitter highlighter folds syntax nodes, and the Markdown grammar does
//! not know `+++`. This highlighter keeps GPUI Kit's Markdown highlighting
//! and swaps in the section ranges from [`crate::task::sections`].

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use gpui_kit::component::highlighter::SyntaxHighlighter;
use gpui_kit::{Context, HighlightStyle, SharedString, Window};

use crate::task::sections;

pub const LANGUAGE: &str = "markdown";

/// Install with `EditorState::set_highlighter_factory` before the editor
/// first renders, so GPUI Kit does not install its own.
pub fn factory() -> InputHighlighterFactory {
    Rc::new(|language| {
        (language == LANGUAGE)
            .then(|| Box::new(SectionFoldHighlighter::new()) as Box<dyn InputHighlighter>)
    })
}

pub fn fold_ranges(text: &str) -> Vec<FoldRange> {
    sections::section_lines(text)
        .into_iter()
        // A fold keeps its first and last line visible, so it needs a line
        // between them to hide anything.
        .filter(|s| s.end_line > s.start_line + 1)
        .map(|s| FoldRange::new(s.start_line, s.end_line))
        .collect()
}

struct SectionFoldHighlighter {
    inner: SyntaxHighlighter,
}

impl SectionFoldHighlighter {
    fn new() -> Self {
        Self {
            inner: SyntaxHighlighter::new(LANGUAGE),
        }
    }
}

impl InputHighlighter for SectionFoldHighlighter {
    fn language(&self) -> SharedString {
        self.inner.language().clone()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        // Full reparse with no timeout. Issue descriptions are small, and it
        // keeps this adapter free of tree-sitter types. The real app should
        // pass the edit through for incremental parsing.
        self.inner.update(None, text, None);

        // One edit can re-pair every `+++` marker below it, so the
        // incremental fold update cannot be trusted. Replace all candidates
        // after the editor finishes this edit. Folded sections that still
        // exist stay folded.
        if folding {
            let folds = fold_ranges(&text.to_string());
            cx.defer_in(window, move |state, _, cx| {
                state.apply_highlighter_fold_candidates(folds, cx);
            });
        }
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        self.inner.styles(range, resolver)
    }

    fn fold_ranges(&self, text: &Rope) -> Vec<FoldRange> {
        fold_ranges(&text.to_string())
    }

    fn fold_ranges_for_edit(&self, _range: Range<usize>, _text: &Rope) -> Vec<FoldRange> {
        // The deferred full update in `update` sets the real candidates.
        Vec::new()
    }
}
