//! The `:` command palette. It owns its input, its suggestions and the
//! focus to give back when it closes. It runs nothing itself: it emits
//! `PaletteEvent::Run`, and the workspace runs the command.

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    input::{self as ed, InputEvent, InputState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _,
};

/// The most suggestions the palette lists at once.
const PALETTE_ROWS: usize = 14;

/// Commands that end an agent's work. They run only when typed in full,
/// never as a suggestion for part of the word.
const TYPED_IN_FULL: &[&str] = &["release"];

/// The first words of commands that write to a task. Enter runs the
/// automatic first match only when this word is typed in full, so `a` does
/// not run `assign me`. The first match still shows as selected, and a
/// suggestion picked with ctrl-n or a click runs as usual.
const WRITES: &[&str] = &[
    "assign", "unassign", "status", "label", "wait", "delete", "restore", "depends", "blocks",
    "undepend", "unblock", "mine", "theirs",
];

/// Whether Enter must not run `command`, the suggestion at `selected`, for
/// what was typed.
fn needs_the_full_word(command: &str, typed_word: &str, selected: usize) -> bool {
    let verb = command.split_whitespace().next().unwrap_or_default();
    selected == 0 && WRITES.contains(&verb) && verb != typed_word
}

/// Commands that name a task, one per open task. They show only after the
/// verb and a space, or typing part of a key would list every verb for it.
const TASK_VERBS: &[&str] = &["depends ", "blocks "];

fn hidden_until_typed(command: &str, typed: &str) -> bool {
    TASK_VERBS
        .iter()
        .any(|v| command.starts_with(v) && command.len() > v.len() && !typed.starts_with(v))
}

/// Palette suggestions for `query` out of `commands`: an exact match
/// first, so `:w` saves even though `wait agent` starts with `w`. Then
/// commands that start with it, then commands that contain it.
fn palette_matches(commands: &[String], query: &str) -> Vec<String> {
    let typed = query
        .trim_start()
        .trim_start_matches(':')
        .trim_start()
        .to_lowercase();
    let query = typed.trim();
    let commands: Vec<&String> = commands
        .iter()
        .filter(|c| !hidden_until_typed(c, &typed))
        .collect();
    let mut out: Vec<String> = commands
        .iter()
        .filter(|c| c.as_str() == query)
        .map(|c| c.to_string())
        .collect();
    out.extend(
        commands
            .iter()
            .filter(|c| c.as_str() != query && c.starts_with(query))
            .map(|c| c.to_string()),
    );
    out.extend(
        commands
            .iter()
            .filter(|c| !c.starts_with(query) && c.contains(query))
            .map(|c| c.to_string()),
    );
    out
}

/// The command Enter runs in the palette: the selected suggestion, or with
/// no suggestion what was typed, such as `assign bob`.
fn palette_command(commands: &[String], typed: &str, selected: usize) -> String {
    let word = typed.trim().trim_start_matches(':').trim().to_lowercase();
    let first_word = word.split_whitespace().next().unwrap_or_default();
    match palette_matches(commands, typed).get(selected) {
        Some(c) if TYPED_IN_FULL.contains(&c.as_str()) && *c != word => typed.to_string(),
        Some(c) if needs_the_full_word(c, first_word, selected) => typed.to_string(),
        Some(c) => c.clone(),
        None => typed.to_string(),
    }
}

pub enum PaletteEvent {
    /// Run this command. The palette is already closed.
    Run(String),
    /// A click outside closed the palette.
    Dismissed,
}

pub struct Palette {
    input: Entity<InputState>,
    open: bool,
    /// Index into the current suggestions.
    selected: usize,
    /// The commands it suggests from.
    commands: Vec<String>,
    /// Focus to give back when it closes.
    return_to: Option<FocusHandle>,
    /// Focus when there is nothing to give back: the list.
    fallback: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    pub fn new(fallback: FocusHandle, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command"));
        let subscription = cx.subscribe_in(&input, window, |this, state, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let typed = state.read(cx).value().to_string();
                let command = palette_command(&this.commands, &typed, this.selected);
                this.close(window, cx);
                cx.emit(PaletteEvent::Run(command));
            } else if let InputEvent::Change = event {
                this.selected = 0;
                cx.notify();
            }
        });
        Self {
            input,
            open: false,
            selected: 0,
            commands: Vec::new(),
            return_to: None,
            fallback,
            _subscriptions: vec![subscription],
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_commands(&mut self, commands: Vec<String>) {
        self.commands = commands;
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Closing gives focus back to where it was opened.
        self.return_to = window.focused(cx);
        self.open = true;
        self.selected = 0;
        self.input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// Opens the palette with `text` typed, ready for the rest of the
    /// command.
    pub fn open_with(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open(window, cx);
        self.input.update(cx, |state, cx| {
            state.set_value(text.to_string(), window, cx)
        });
        cx.notify();
    }

    #[cfg(test)]
    pub fn typed(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        let handle = self.return_to.take().unwrap_or(self.fallback.clone());
        window.focus(&handle, cx);
        cx.notify();
    }

    /// A click outside closes the palette, as in Zed or VS Code. The click
    /// keeps the focus it gives. Only a click on something that takes no
    /// focus sends focus back to where the palette opened.
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        let input = self.input.focus_handle(cx);
        let back = self.return_to.take().unwrap_or(self.fallback.clone());
        window.defer(cx, move |window, cx| {
            if input.contains_focused(window, cx) {
                window.focus(&back, cx);
            }
        });
        cx.emit(PaletteEvent::Dismissed);
        cx.notify();
    }

    /// The suggestions for what is typed, at most a screenful.
    fn suggestions(&self, cx: &App) -> Vec<String> {
        let mut out = palette_matches(&self.commands, &self.input.read(cx).value());
        out.truncate(PALETTE_ROWS);
        out
    }

    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let count = self.suggestions(cx).len();
        self.selected = (self.selected + 1).min(count.saturating_sub(1));
        cx.notify();
    }

    pub fn select_prev(&mut self, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }
}

impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let matches = self.suggestions(cx);
        let hover = theme.list_hover;
        let pressed = theme.foreground.opacity(0.12);
        let active_bg = theme.list_active;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let root = div()
            .absolute()
            .top_10()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                v_flex()
                    .id("palette")
                    .key_context("Palette")
                    // Up and Down move the suggestion, not the caret.
                    .capture_action(cx.listener(|this, _: &ed::MoveDown, _, cx| {
                        cx.stop_propagation();
                        this.select_next(cx);
                    }))
                    .capture_action(cx.listener(|this, _: &ed::MoveUp, _, cx| {
                        cx.stop_propagation();
                        this.select_prev(cx);
                    }))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.dismiss(window, cx)))
                    .w_96()
                    .p_1()
                    .gap_px()
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .shadow_lg()
                    .child(
                        crate::shell::widgets::text_input(&self.input)
                            .prefix(div().text_color(muted).child(":")),
                    )
                    .children(matches.into_iter().enumerate().map(|(ix, command)| {
                        let selected = ix == self.selected;
                        let swatch = command.strip_prefix("project-color ").map(|name| {
                            let hex = crate::shell::icons::PROJECT_COLORS
                                .iter()
                                .find(|(n, _)| *n == name)
                                .map(|(_, hex)| *hex);
                            crate::shell::icons::swatch_icon(hex, cx).xsmall()
                        });
                        gpui_kit::component::h_flex()
                            .gap_2()
                            .id(SharedString::from(format!("palette-{command}")))
                            .px_2()
                            .py_0p5()
                            .rounded(theme.radius)
                            .text_sm()
                            .cursor_pointer()
                            .text_color(if selected { fg } else { muted })
                            .when(selected, |this| this.bg(active_bg))
                            .when(!selected, |this| this.hover(move |s| s.bg(hover)))
                            .active(move |s| s.bg(pressed))
                            .children(swatch)
                            .child(command.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close(window, cx);
                                cx.emit(PaletteEvent::Run(command.clone()));
                            }))
                    })),
            );
        crate::shell::motion::enter(root, "palette-enter", cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An exact command wins over longer ones that start with it.
    #[test]
    fn task_commands_show_only_after_the_verb() {
        let commands: Vec<String> = ["depends", "open llm-9", "depends llm-9", "blocks llm-9"]
            .map(String::from)
            .to_vec();
        // Typing a key finds `open`, not every verb for that key.
        assert_eq!(palette_matches(&commands, "llm-9"), ["open llm-9"]);
        assert_eq!(palette_matches(&commands, "dep"), ["depends"]);
        assert_eq!(
            palette_matches(&commands, "depends "),
            ["depends", "depends llm-9"]
        );
        assert_eq!(
            palette_matches(&commands, ":depends llm"),
            ["depends llm-9"]
        );
    }

    #[test]
    fn exact_match_comes_first() {
        let commands: Vec<String> = ["wait agent", "wait owner", "w", "q"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(palette_command(&commands, "w", 0), "w");
        assert_eq!(palette_command(&commands, "wait", 0), "wait agent");
    }

    fn commands() -> Vec<String> {
        [
            "status backlog",
            "status done",
            "assign me",
            "unassign",
            "all",
            "delete",
            "theme dark",
            "sort key",
        ]
        .map(String::from)
        .to_vec()
    }

    /// LLM-120: `assign me` is the first suggestion for `a`, and for `me`
    /// because it contains those letters. Enter must not run it for them.
    #[test]
    fn a_partial_word_does_not_run_a_write() {
        let commands = commands();
        for typed in ["a", "as", "assig", ":a", "me", "ign", "unass"] {
            assert_eq!(palette_command(&commands, typed, 0), typed, "{typed}");
        }
        assert_eq!(palette_command(&commands, "s", 0), "s");
        assert_eq!(palette_command(&commands, "do", 0), "do");
        assert_eq!(palette_command(&commands, "del", 0), "del");
    }

    #[test]
    fn the_full_word_runs_a_write() {
        let commands = commands();
        assert_eq!(palette_command(&commands, "assign", 0), "assign me");
        assert_eq!(palette_command(&commands, ":assign m", 0), "assign me");
        assert_eq!(palette_command(&commands, "unassign", 0), "unassign");
        assert_eq!(palette_command(&commands, "status d", 0), "status done");
        assert_eq!(palette_command(&commands, "delete", 0), "delete");
    }

    #[test]
    fn the_first_match_stays_selected_and_other_commands_still_run_from_part() {
        let commands = commands();
        // The list still puts the first match on top, for every command.
        assert_eq!(palette_matches(&commands, "a")[0], "assign me");
        assert_eq!(palette_command(&commands, "the", 0), "theme dark");
        assert_eq!(palette_command(&commands, "so", 0), "sort key");
    }

    /// Ctrl-n is a choice, so Enter runs what it picked.
    #[test]
    fn a_suggestion_picked_with_ctrl_n_runs() {
        let commands: Vec<String> = ["all", "assign me"].map(String::from).to_vec();
        assert_eq!(palette_command(&commands, "a", 0), "all");
        assert_eq!(palette_command(&commands, "a", 1), "assign me");
    }
}
