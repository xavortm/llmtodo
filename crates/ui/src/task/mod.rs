//! The task view: preview, tabs, the Markdown editor and its vim keys.

pub mod fold_highlighter;
pub mod history;
pub mod panel_resize;
pub mod preview;
pub mod review;
pub mod sections;
pub mod task_view;
pub mod vim_editor;

#[cfg(test)]
mod wrap_tests;

#[cfg(test)]
mod review_tests;

#[cfg(test)]
mod prepared_tests;
