//! The Settings view. It replaces the issue list and the task panel, like
//! the inbox does. Every change applies at once and goes to `ui.json`, so
//! there is no Save button.
//!
//! `j` / `k` and `Ctrl-N` / `Ctrl-P` move between rows. `Enter` or `Space`
//! flips a switch or enters a number field, and `h` / `l` step a number.
//! `Esc` and the mouse back button leave the view, through `back` in the workspace.
//!
//! The workspace owns the values. This view shows the copy the workspace
//! keeps current, and asks for each change with a `ViewEvent`.

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex,
    input::{InputEvent, InputState, MaskPattern, NumberInput, NumberInputEvent, StepAction},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div,
    prelude::FluentBuilder as _, rems,
};

use crate::model::data::hide_cutoff;
use crate::model::store::IssueStore;
use crate::settings::{MAX_HIDE_DAYS, UiSettings};
use crate::shell::theme::{self, ZOOM_STEPS};
use crate::shell::view_events::{ViewEvent, ViewEvents};

pub const CONTEXT: &str = "Settings";

actions!(
    llmtodo_settings,
    [
        SettingsNext,
        SettingsPrev,
        /// Flip the selected switch, or enter the selected number field.
        SettingsActivate,
        SettingsDecrease,
        SettingsIncrease,
    ]
);

pub fn bind_keys(cx: &mut App) {
    use gpui_kit::KeyBinding;
    // Only in normal mode: while a number field has focus, the keys type.
    let ctx = Some("(Workspace && mode == normal) > Settings");
    cx.bind_keys([
        KeyBinding::new("j", SettingsNext, ctx),
        KeyBinding::new("down", SettingsNext, ctx),
        KeyBinding::new("ctrl-n", SettingsNext, ctx),
        KeyBinding::new("k", SettingsPrev, ctx),
        KeyBinding::new("up", SettingsPrev, ctx),
        KeyBinding::new("ctrl-p", SettingsPrev, ctx),
        KeyBinding::new("enter", SettingsActivate, ctx),
        KeyBinding::new("space", SettingsActivate, ctx),
        KeyBinding::new("h", SettingsDecrease, ctx),
        KeyBinding::new("left", SettingsDecrease, ctx),
        KeyBinding::new("l", SettingsIncrease, ctx),
        KeyBinding::new("right", SettingsIncrease, ctx),
    ]);
}

/// One line of the form, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    HideClosed,
    HideClosedDays,
    Vim,
    DarkMode,
    Zoom,
    OpenLast,
    Mascot,
}

const ROWS: [Row; 7] = [
    Row::HideClosed,
    Row::HideClosedDays,
    Row::Vim,
    Row::DarkMode,
    Row::Zoom,
    Row::OpenLast,
    Row::Mascot,
];

impl Row {
    fn id(self) -> &'static str {
        match self {
            Self::HideClosed => "settings-row-hide-closed",
            Self::HideClosedDays => "settings-row-hide-days",
            Self::Vim => "settings-row-vim",
            Self::DarkMode => "settings-row-dark",
            Self::Zoom => "settings-row-zoom",
            Self::OpenLast => "settings-row-open-last",
            Self::Mascot => "settings-row-mascot",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::HideClosed => "Hide old closed tasks",
            Self::HideClosedDays => "Hide after",
            Self::Vim => "Vim keys in the editor",
            Self::DarkMode => "Dark mode",
            Self::Zoom => "Zoom",
            Self::OpenLast => "Open the last view at start",
            Self::Mascot => "Sudo the cat",
        }
    }

    /// The icon before the title. Most rows have none.
    fn icon(self) -> Option<IconName> {
        match self {
            Self::Mascot => Some(IconName::Cat),
            _ => None,
        }
    }

    /// A number row holds its own field. Clicking the row itself does
    /// nothing, so it gets no hover change.
    fn is_switch(self) -> bool {
        !matches!(self, Self::HideClosedDays | Self::Zoom)
    }
}

/// The line under the "Hide after" field: how many tasks the setting hides,
/// or would hide while it is off.
pub fn hidden_hint(on: bool, count: usize) -> String {
    let tasks = if count == 1 {
        "1 closed task".to_string()
    } else {
        format!("{count} closed tasks")
    };
    match (on, count) {
        (_, 0) => "No closed task is this old".into(),
        (true, _) => format!("Hiding {tasks}"),
        (false, 1) => format!("{tasks} is this old"),
        (false, _) => format!("{tasks} are this old"),
    }
}

/// `zoom` as whole percent, as the field shows it.
fn percent(zoom: f32) -> u32 {
    (zoom * 100.).round() as u32
}

/// A digits-only mask for the number fields.
fn whole_number() -> MaskPattern {
    MaskPattern::Number {
        separator: None,
        fraction: Some(0),
    }
}

pub struct SettingsView {
    store: Entity<IssueStore>,
    events: ViewEvents,
    /// The settings as the workspace last saved them.
    values: Entity<UiSettings>,
    pub focus: FocusHandle,
    /// Index into `ROWS`, for the keyboard.
    selected: usize,
    days: Entity<InputState>,
    zoom: Entity<InputState>,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(
        store: Entity<IssueStore>,
        events: ViewEvents,
        values: Entity<UiSettings>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut number_field = |value: String, min: f64, max: f64, cx: &mut Context<Self>| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .mask_pattern(whole_number())
                    .min(min)
                    .max(max)
                    .default_value(value)
            });
            // No internal stepping. A step event comes here instead, and
            // zoom moves through its fixed levels.
            input.update(cx, |state, cx| state.set_step(None, window, cx));
            input
        };
        let min_zoom = f64::from(percent(ZOOM_STEPS[0]));
        let max_zoom = f64::from(percent(ZOOM_STEPS[ZOOM_STEPS.len() - 1]));
        let days = number_field(
            values.read(cx).hide_closed_days.to_string(),
            1.,
            f64::from(MAX_HIDE_DAYS),
            cx,
        );
        let zoom = number_field(percent(theme::zoom(cx)).to_string(), min_zoom, max_zoom, cx);

        let subscriptions = vec![
            cx.subscribe_in(
                &days,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.apply_typed_days(window, cx),
                    InputEvent::PressEnter { .. } => this.take_focus(window, cx),
                    InputEvent::Blur => this.show_saved_days(window, cx),
                    InputEvent::Focus => {}
                },
            ),
            cx.subscribe_in(
                &days,
                window,
                |this, _, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(action) = event;
                    this.step(Row::HideClosedDays, *action, window, cx);
                },
            ),
            // Zoom waits for Enter or for leaving the field. Applying each
            // digit would resize the app to 12% on the way to 125%.
            cx.subscribe_in(
                &zoom,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => {
                        this.apply_typed_zoom(window, cx);
                        this.take_focus(window, cx);
                    }
                    InputEvent::Blur => this.apply_typed_zoom(window, cx),
                    InputEvent::Change | InputEvent::Focus => {}
                },
            ),
            cx.subscribe_in(
                &zoom,
                window,
                |this, _, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(action) = event;
                    this.step(Row::Zoom, *action, window, cx);
                },
            ),
            // Cmd-plus, Cmd-minus and a restored zoom change the level
            // outside this view.
            cx.observe(&values, |_, _, cx| cx.notify()),
            cx.observe_global_in::<theme::Zoom>(window, |this, window, cx| {
                this.show_zoom(window, cx);
                cx.notify();
            }),
        ];
        Self {
            store,
            events,
            values,
            focus: cx.focus_handle(),
            selected: 0,
            days,
            zoom,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn take_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Whether a number field has focus, so keys type instead of acting.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        self.days.focus_handle(cx).is_focused(window)
            || self.zoom.focus_handle(cx).is_focused(window)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected_row(&self) -> Row {
        ROWS[self.selected]
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn zoom_input(&self) -> &Entity<InputState> {
        &self.zoom
    }

    fn values(&self, cx: &App) -> UiSettings {
        self.values.read(cx).clone()
    }

    /// Asks the workspace for a change and redraws this view.
    fn write(&mut self, event: ViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.events.emit(event, window, cx);
        cx.notify();
    }

    fn apply_typed_days(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(days) = self.days.read(cx).value().parse::<u32>() else {
            return;
        };
        if days >= 1 {
            self.write(ViewEvent::SetHideClosedDays(days), window, cx);
        }
    }

    /// Replaces an empty or out-of-range entry with the value in use.
    fn show_saved_days(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let days = self.values(cx).hide_closed_days;
        self.set_text(&self.days.clone(), days.to_string(), window, cx);
    }

    fn apply_typed_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(value) = self.zoom.read(cx).value().parse::<u32>() {
            let level = value as f32 / 100.;
            if percent(theme::zoom(cx)) != value {
                self.write(ViewEvent::SetZoom(level), window, cx);
            }
        }
        // A level out of range was clamped, and an empty field is not a level.
        self.show_zoom(window, cx);
    }

    fn show_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = percent(theme::zoom(cx)).to_string();
        self.set_text(&self.zoom.clone(), text, window, cx);
    }

    fn set_text(
        &self,
        input: &Entity<InputState>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if input.read(cx).value().as_ref() != text.as_str() {
            input.update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    fn step(&mut self, row: Row, action: StepAction, window: &mut Window, cx: &mut Context<Self>) {
        let up = matches!(action, StepAction::Increment);
        match row {
            Row::HideClosedDays => {
                let values = self.values(cx);
                if !values.hide_closed {
                    return;
                }
                let days = if up {
                    values.hide_closed_days.saturating_add(1)
                } else {
                    values.hide_closed_days.saturating_sub(1)
                }
                .clamp(1, MAX_HIDE_DAYS);
                self.write(ViewEvent::SetHideClosedDays(days), window, cx);
                self.show_saved_days(window, cx);
            }
            Row::Zoom => {
                let level = theme::zoom_step(theme::zoom(cx), up);
                self.write(ViewEvent::SetZoom(level), window, cx);
                self.show_zoom(window, cx);
            }
            _ => {}
        }
    }

    fn activate(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.values(cx);
        match row {
            Row::HideClosed => {
                self.write(ViewEvent::SetHideClosed(!values.hide_closed), window, cx)
            }
            Row::Vim => self.write(ViewEvent::SetVimEditor(!values.vim_editor), window, cx),
            Row::DarkMode => {
                let dark = values.theme != "dark";
                self.write(ViewEvent::SetDark(dark), window, cx);
            }
            Row::OpenLast => {
                self.write(
                    ViewEvent::SetOpenLastView(!values.open_last_view),
                    window,
                    cx,
                );
            }
            Row::Mascot => self.write(ViewEvent::SetMascot(!values.mascot), window, cx),
            Row::HideClosedDays if !values.hide_closed => {}
            Row::HideClosedDays => self.days.update(cx, |state, cx| state.focus(window, cx)),
            Row::Zoom => self.zoom.update(cx, |state, cx| state.focus(window, cx)),
        }
    }

    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        self.selected = if down {
            (self.selected + 1).min(ROWS.len() - 1)
        } else {
            self.selected.saturating_sub(1)
        };
        cx.notify();
    }

    /// The closed tasks old enough to hide, across the whole workspace.
    fn hidden_count(&self, days: u32, cx: &App) -> usize {
        let cutoff = hide_cutoff(days, jiff::Timestamp::now());
        self.store
            .read(cx)
            .iter()
            .filter(|i| !i.deleted && i.closed_before(cutoff))
            .count()
    }

    fn help(&self, row: Row, values: &UiSettings, cx: &App) -> Option<SharedString> {
        let text: SharedString = match row {
            Row::HideClosed => "Hide Done and Canceled tasks from the list".into(),
            Row::HideClosedDays => hidden_hint(
                values.hide_closed,
                self.hidden_count(values.hide_closed_days, cx),
            )
            .into(),
            Row::Vim => "Off: the description editor types as usual".into(),
            Row::OpenLast => "Off: the app always opens All issues".into(),
            Row::Mascot => "Your pixel pet in the sidebar. It works while an agent works, \
                 and sleeps when nothing happens"
                .into(),
            Row::DarkMode | Row::Zoom => return None,
        };
        Some(text)
    }

    /// A number field. The frame sits on a wrapper, because `NumberInput`
    /// has no hover or pressed state of its own around the text.
    fn number_field(
        &self,
        row: Row,
        input: &Entity<InputState>,
        suffix: &'static str,
        disabled: bool,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let hover = theme.muted_foreground;
        let pressed = theme.foreground;
        let muted = theme.muted_foreground;
        let focused = input.focus_handle(cx).is_focused(window);
        div()
            .id(SharedString::from(format!("{}-field", row.id())))
            .w(rems(8.))
            // The same height as `widgets::text_input`.
            .h_8()
            .flex()
            .flex_col()
            .justify_center()
            .rounded(theme.radius)
            .border_1()
            .border_color(if focused { theme.ring } else { theme.input })
            .bg(theme.input_background())
            .when(!disabled, |this| {
                this.hover(move |s| s.border_color(hover))
                    .active(move |s| s.border_color(pressed))
            })
            .child(
                NumberInput::new(input)
                    .small()
                    .appearance(false)
                    .disabled(disabled)
                    .suffix(div().text_xs().text_color(muted).child(suffix)),
            )
            .into_any_element()
    }

    fn control(&self, row: Row, values: &UiSettings, window: &Window, cx: &App) -> AnyElement {
        let switch = |on: bool| {
            // The row handles the click, so the whole row is the target and
            // shows the hover and pressed states.
            Switch::new(SharedString::from(format!("{}-switch", row.id())))
                .small()
                .checked(on)
                .tab_stop(false)
                .into_any_element()
        };
        match row {
            Row::HideClosed => switch(values.hide_closed),
            Row::HideClosedDays => {
                self.number_field(row, &self.days, "days", !values.hide_closed, window, cx)
            }
            Row::Vim => switch(values.vim_editor),
            Row::DarkMode => switch(values.theme == "dark"),
            Row::Zoom => self.number_field(row, &self.zoom, "%", false, window, cx),
            Row::OpenLast => switch(values.open_last_view),
            Row::Mascot => switch(values.mascot),
        }
    }

    fn render_row(
        &self,
        ix: usize,
        row: Row,
        values: &UiSettings,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let control = self.control(row, values, window, cx);
        let help = self.help(row, values, cx);
        let selected = ix == self.selected && self.focus.is_focused(window);
        let theme = cx.theme();
        let hover = if selected {
            crate::shell::theme::selection_hover(theme)
        } else {
            theme.list_hover
        };
        let pressed = crate::shell::theme::row_pressed(theme);
        let selected_bg = crate::shell::theme::selection_bg(theme, true);
        let muted = theme.muted_foreground;
        let radius = theme.radius;
        h_flex()
            .id(row.id())
            .test_support()
            .w_full()
            .min_h_10()
            .flex_none()
            .px_3()
            .py_1p5()
            .gap_4()
            .rounded(radius)
            .when(selected, |this| this.bg(selected_bg))
            .when(row.is_switch(), |this| {
                this.cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .active(move |s| s.bg(pressed))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected = ix;
                        this.activate(row, window, cx);
                    }))
            })
            .when_some(row.icon(), |this, icon| {
                this.child(Icon::new(icon).small().flex_none().text_color(muted))
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().child(row.title()))
                    .when_some(help, |this, help| {
                        this.child(div().text_xs().text_color(muted).child(help))
                    }),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let values = self.values(cx);
        let rows: Vec<AnyElement> = ROWS
            .into_iter()
            .enumerate()
            .map(|(ix, row)| self.render_row(ix, row, &values, window, cx))
            .collect();
        let theme = cx.theme();
        div()
            .id("settings-view")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SettingsNext, _, cx| this.move_selection(true, cx)))
            .on_action(cx.listener(|this, _: &SettingsPrev, _, cx| this.move_selection(false, cx)))
            .on_action(cx.listener(|this, _: &SettingsActivate, window, cx| {
                this.activate(ROWS[this.selected], window, cx);
            }))
            .on_action(cx.listener(|this, _: &SettingsDecrease, window, cx| {
                this.step(ROWS[this.selected], StepAction::Decrement, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SettingsIncrease, window, cx| {
                this.step(ROWS[this.selected], StepAction::Increment, window, cx);
            }))
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(theme.background)
            .child(
                h_flex().w_full().justify_center().child(
                    v_flex()
                        .w_full()
                        .max_w(rems(40.))
                        .px_4()
                        .py_6()
                        .gap_1()
                        .child(div().px_3().pb_3().text_lg().child("Settings"))
                        .children(rows),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_counts_the_hidden_tasks() {
        assert_eq!(hidden_hint(true, 0), "No closed task is this old");
        assert_eq!(hidden_hint(false, 0), "No closed task is this old");
        assert_eq!(hidden_hint(true, 1), "Hiding 1 closed task");
        assert_eq!(hidden_hint(true, 23), "Hiding 23 closed tasks");
        assert_eq!(hidden_hint(false, 1), "1 closed task is this old");
        assert_eq!(hidden_hint(false, 23), "23 closed tasks are this old");
    }
}
