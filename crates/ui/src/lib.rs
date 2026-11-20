//! The llmtodo desktop app's views and model, as a library. The
//! `llmtodo-app` binary opens a window on them, and `llmtodo-ui-bench`
//! draws them through GPUI's headless Metal renderer. `model/backend.rs`
//! talks to the local API server; SPIKE.md records how the GPUI parts work.

pub mod docs;
pub mod inbox;
pub mod issues;
pub mod model;
#[cfg(test)]
pub mod render_count;
pub mod settings;
pub mod shell;
pub mod task;

use gpui_kit::component::ThemeMode;

use shell::{app_menus, theme, workspace};

/// Sets up theme and keys. The tests and the benchmarks call it.
pub fn init(cx: &mut gpui_kit::App) {
    init_without_theme(cx);
    theme::set_mode(ThemeMode::Dark, cx);
}

/// Everything `init` does but the theme. The app sets the saved theme
/// itself, once, with `theme::start` (LLM-6).
pub fn init_without_theme(cx: &mut gpui_kit::App) {
    gpui_kit::init(cx);
    theme::load_fonts(cx);
    workspace::bind_keys(cx);
    app_menus::init(cx);
}
