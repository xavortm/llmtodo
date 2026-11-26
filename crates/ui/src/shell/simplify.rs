//! LLM-586: the Simplify UI setting. On, the app hides all but the
//! essentials: the task panel shows the title, the description and the
//! review panel; the list shows the LLM, key, title and waiting-on
//! columns; the strip of stats under the list hides.
//!
//! The switch lives here as a global, so every view reads one value.
//! Later tweaks for this mode check [`on`] too.

use gpui_kit::{App, Global};

use crate::issues::columns::Column;

struct Simplify(bool);

impl Global for Simplify {}

/// Whether Simplify UI is on.
pub fn on(cx: &App) -> bool {
    cx.try_global::<Simplify>().is_some_and(|s| s.0)
}

pub fn set(on: bool, cx: &mut App) {
    cx.set_global(Simplify(on));
}

/// The list columns Simplify UI keeps.
pub const LIST_COLUMNS: [Column; 4] = [Column::Llm, Column::Key, Column::Title, Column::Waiting];

/// Whether Simplify UI hides `column` from the list.
pub fn hides_column(column: Column, cx: &App) -> bool {
    on(cx) && !LIST_COLUMNS.contains(&column)
}
