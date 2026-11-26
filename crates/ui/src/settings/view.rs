//! The Settings view. It replaces the issue list and the task panel, like
//! the inbox does. Every change applies at once and goes to `config.toml` or `ui-state.json`, so
//! there is no Save button.
//!
//! `j` / `k` and `Ctrl-N` / `Ctrl-P` move between rows. `Enter` or `Space`
//! flips a switch or enters a number field, and `h` / `l` step a number or
//! pick the theme before or after the current one.
//! `Esc` and the mouse back button leave the view, through `back` in the workspace.
//!
//! The workspace owns the values. This view shows the copy the workspace
//! keeps current, and asks for each change with a `ViewEvent`.

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState, MaskPattern, NumberInput, NumberInputEvent, StepAction},
    select::{Select, SelectEvent, SelectItem, SelectState},
    switch::Switch,
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div,
    prelude::FluentBuilder as _, rems,
};

use llmtodo_api_types::{DatabaseBackend, DatabaseInfo};

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

/// Theme cards on one line in Settings. A fifth theme starts a second line.
const PREVIEWS_PER_ROW: u16 = 4;
/// Width over height of a theme card's picture.
const PREVIEW_ASPECT: f32 = 9. / 5.5;

/// One line of the form, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    HideClosed,
    HideClosedDays,
    Vim,
    Theme,
    Appearance,
    Zoom,
    OpenLast,
    Mascot,
    Animations,
    StrikeDone,
}

const ROWS: [Row; 10] = [
    Row::HideClosed,
    Row::HideClosedDays,
    Row::Vim,
    Row::Theme,
    Row::Appearance,
    Row::Zoom,
    Row::OpenLast,
    Row::Mascot,
    Row::Animations,
    Row::StrikeDone,
];

impl Row {
    fn id(self) -> &'static str {
        match self {
            Self::HideClosed => "settings-row-hide-closed",
            Self::HideClosedDays => "settings-row-hide-days",
            Self::Vim => "settings-row-vim",
            Self::Theme => "settings-row-theme",
            Self::Appearance => "settings-row-appearance",
            Self::Zoom => "settings-row-zoom",
            Self::OpenLast => "settings-row-open-last",
            Self::Mascot => "settings-row-mascot",
            Self::Animations => "settings-row-animations",
            Self::StrikeDone => "settings-row-strike-done",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::HideClosed => "Hide old closed tasks",
            Self::HideClosedDays => "Hide after",
            Self::Vim => "Vim keys in the editor",
            Self::Theme => "Theme",
            Self::Appearance => "Appearance",
            Self::Zoom => "Zoom",
            Self::OpenLast => "Open the last view at start",
            Self::Mascot => "Sudo the cat",
            Self::Animations => "Animations",
            Self::StrikeDone => "Line through done tasks",
        }
    }

    /// The icon before the title. Most rows have none.
    fn icon(self) -> Option<IconName> {
        match self {
            Self::Mascot => Some(IconName::Cat),
            _ => None,
        }
    }

    /// A row that tries out a feature shows an "Experimental" tag.
    fn experimental(self) -> bool {
        self == Self::StrikeDone
    }

    /// A number row holds its own field, the theme row its previews and
    /// the appearance row its buttons. Clicking the row itself does
    /// nothing, so it gets no hover change.
    fn is_switch(self) -> bool {
        !matches!(
            self,
            Self::HideClosedDays | Self::Zoom | Self::Theme | Self::Appearance
        )
    }
}

/// The line under the "Hide after" field: how many tasks the setting hides,
/// or would hide while it is off.
pub fn hidden_hint(on: bool, days: u32, count: usize) -> String {
    let tasks = if count == 1 {
        "1 closed task".to_string()
    } else {
        format!("{count} closed tasks")
    };
    let hint = match (on, count) {
        (_, 0) => "No closed task is this old".into(),
        (true, _) => format!("Hiding {tasks}"),
        (false, 1) => format!("{tasks} is this old"),
        (false, _) => format!("{tasks} are this old"),
    };
    if days == 0 {
        format!("0 hides closed tasks at once. {hint}")
    } else {
        hint
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
    /// Why changes do not reach `config.toml`, such as a read-only file.
    config_note: Option<SharedString>,
    database: Option<DatabaseInfo>,
    /// The folder of `config.toml`: a relative image path starts there.
    config_dir: Option<std::path::PathBuf>,
    /// LLM-567: each slot's opacity field and dropdowns.
    backgrounds: Vec<(Slot, BgControls)>,
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
            0.,
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
            // A change, also a hand edit of config.toml, shows in the
            // background fields and dropdowns.
            cx.observe_in(&values, window, |this, _, window, cx| {
                this.sync_backgrounds(window, cx);
                cx.notify();
            }),
            // The view is cached: it counts hidden tasks from the store.
            cx.observe(&store, |_, _, cx| cx.notify()),
            cx.observe_global_in::<theme::Zoom>(window, |this, window, cx| {
                this.show_zoom(window, cx);
                cx.notify();
            }),
        ];
        let mut subscriptions = subscriptions;
        let backgrounds = Slot::ALL
            .into_iter()
            .map(|slot| {
                (
                    slot,
                    Self::bg_controls(slot, &mut subscriptions, window, cx),
                )
            })
            .collect();
        let mut this = Self {
            store,
            events,
            values,
            focus: cx.focus_handle(),
            selected: 0,
            days,
            zoom,
            scroll: ScrollHandle::new(),
            config_note: None,
            database: None,
            config_dir: None,
            backgrounds,
            _subscriptions: subscriptions,
        };
        this.sync_backgrounds(window, cx);
        this
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

    /// The database the server opened, shown at the end of the form.
    pub fn set_database(&mut self, database: Option<DatabaseInfo>, cx: &mut Context<Self>) {
        if self.database != database {
            self.database = database;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn database_line(&self) -> Option<SharedString> {
        self.database.as_ref().map(database_line)
    }

    /// Shows `note` under the heading, or nothing for `None`.
    pub fn set_config_note(&mut self, note: Option<SharedString>, cx: &mut Context<Self>) {
        if self.config_note != note {
            self.config_note = note;
            cx.notify();
        }
    }

    /// Scrolls to the end, where the Background section sits.
    #[cfg(test)]
    pub fn scroll_to_end(&self) {
        self.scroll.scroll_to_bottom();
    }

    /// The folder relative background paths start from.
    pub fn set_config_dir(&mut self, dir: Option<std::path::PathBuf>) {
        self.config_dir = dir;
    }

    #[cfg(test)]
    pub fn config_note(&self) -> Option<&SharedString> {
        self.config_note.as_ref()
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
        self.write(ViewEvent::SetHideClosedDays(days), window, cx);
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
                .min(MAX_HIDE_DAYS);
                self.write(ViewEvent::SetHideClosedDays(days), window, cx);
                self.show_saved_days(window, cx);
            }
            Row::Zoom => {
                let level = theme::zoom_step(theme::zoom(cx), up);
                self.write(ViewEvent::SetZoom(level), window, cx);
                self.show_zoom(window, cx);
            }
            Row::Theme => {
                let next = theme::color_theme(cx).next(up);
                self.write(ViewEvent::SetColorTheme(next), window, cx);
            }
            Row::Appearance => {
                let next = theme::appearance(cx).step(up);
                self.write(ViewEvent::SetAppearance(next), window, cx);
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
            Row::Theme => {
                let next = theme::color_theme(cx).next(true);
                self.write(ViewEvent::SetColorTheme(next), window, cx);
            }
            Row::Appearance => {
                let next = theme::appearance(cx).cycle();
                self.write(ViewEvent::SetAppearance(next), window, cx);
            }
            Row::OpenLast => {
                self.write(
                    ViewEvent::SetOpenLastView(!values.open_last_view),
                    window,
                    cx,
                );
            }
            Row::Mascot => self.write(ViewEvent::SetMascot(!values.mascot), window, cx),
            Row::Animations => self.write(ViewEvent::SetAnimations(!values.animations), window, cx),
            Row::StrikeDone => {
                self.write(ViewEvent::SetStrikeDone(!values.strike_done), window, cx)
            }
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
            .filter(|i| !i.deleted && i.hidden_by(cutoff))
            .count()
    }

    fn help(&self, row: Row, values: &UiSettings, cx: &App) -> Option<SharedString> {
        let text: SharedString = match row {
            Row::HideClosed => "Hide Done and Canceled tasks from the list. A task that an open task waits for stays".into(),
            Row::HideClosedDays => hidden_hint(
                values.hide_closed,
                values.hide_closed_days,
                self.hidden_count(values.hide_closed_days, cx),
            )
            .into(),
            Row::Vim => "Off: the description editor types as usual".into(),
            Row::OpenLast => "Off: the app always opens All issues".into(),
            Row::Mascot => "Your pixel pet in the sidebar. It works while an agent works, \
                 and sleeps when nothing happens"
                .into(),
            Row::Theme => "Every theme has a light and a dark mode".into(),
            Row::Appearance => "Auto follows macOS".into(),
            Row::Animations => "Short movement when panels open, menus appear, and you hover or click"
                .into(),
            Row::StrikeDone => "A light line through the titles of Done tasks in the list".into(),
            Row::Zoom => return None,
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
            // The previews sit under the title, see `render_row`.
            Row::Theme => div().into_any_element(),
            // Built in `render_row`, which can listen for its clicks.
            Row::Appearance => div().into_any_element(),
            Row::Zoom => self.number_field(row, &self.zoom, "%", false, window, cx),
            Row::OpenLast => switch(values.open_last_view),
            Row::Mascot => switch(values.mascot),
            Row::Animations => switch(values.animations),
            Row::StrikeDone => switch(values.strike_done),
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
        let control = if row == Row::Appearance {
            self.appearance_control(cx)
        } else {
            self.control(row, values, window, cx)
        };
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
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().text_sm().child(row.title()))
                            .when(row.experimental(), |this| {
                                this.child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "{}-experimental",
                                            row.id()
                                        )))
                                        .test_support()
                                        .child(
                                            Tag::secondary()
                                                .outline()
                                                .xsmall()
                                                .child("Experimental"),
                                        ),
                                )
                            }),
                    )
                    .when_some(help, |this, help| {
                        this.child(div().text_xs().text_color(muted).child(help))
                    })
                    .when(row == Row::Theme, |this| {
                        this.child(self.theme_previews(cx))
                    }),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }

    /// Dark, Auto and Light as one segmented control. The picked one stays
    /// pressed.
    fn appearance_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = theme::appearance(cx);
        let row = ROWS.iter().position(|r| *r == Row::Appearance).unwrap_or(0);
        theme::Appearance::ALL
            .into_iter()
            .fold(
                ButtonGroup::new("settings-appearance")
                    .outline()
                    .compact()
                    .small(),
                |group, choice| {
                    group.child(
                        Button::new(SharedString::from(format!(
                            "settings-appearance-{}",
                            choice.id()
                        )))
                        .label(choice.label())
                        .tab_stop(false)
                        .selected(choice == current),
                    )
                },
            )
            .on_click(cx.listener(move |this, clicked: &Vec<usize>, window, cx| {
                let Some(choice) = clicked
                    .first()
                    .and_then(|ix| theme::Appearance::ALL.get(*ix))
                else {
                    return;
                };
                this.selected = row;
                this.write(ViewEvent::SetAppearance(*choice), window, cx);
            }))
            .into_any_element()
    }

    /// One small picture of the app per theme, in the current mode. A
    /// click picks that theme.
    fn theme_previews(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = theme::color_theme(cx);
        let mode = cx.theme().mode;
        let cards: Vec<AnyElement> = theme::ColorTheme::ALL
            .into_iter()
            .map(|name| {
                let colors = theme::preview(name, mode, cx);
                self.theme_card(name, name == current, &colors, cx)
            })
            .collect();
        // One row of equal cards that share the row's width, so every
        // theme fits on one line at any window width.
        div()
            .grid()
            .grid_cols(PREVIEWS_PER_ROW)
            .w_full()
            .pt_2()
            .gap_3()
            .children(cards)
            .into_any_element()
    }

    fn theme_card(
        &self,
        name: theme::ColorTheme,
        picked: bool,
        p: &gpui_kit::component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = cx.theme();
        let ring = if picked { ui.foreground } else { ui.border };
        let hover = ui.muted_foreground;
        let pressed = ui.foreground;
        let label = if picked {
            ui.foreground
        } else {
            ui.muted_foreground
        };
        let bar = |w: f32, color| div().h(rems(0.25)).w(rems(w)).rounded_sm().bg(color);
        let sidebar = v_flex()
            .w(rems(2.25))
            .h_full()
            .flex_none()
            .p_1()
            .gap_1()
            .bg(p.sidebar)
            .border_r_1()
            .border_color(p.sidebar_border)
            .child(bar(1.25, p.muted_foreground))
            .child(
                div()
                    .rounded_sm()
                    .px_0p5()
                    .py_0p5()
                    .bg(p.sidebar_accent)
                    .child(bar(1.0, p.foreground)),
            )
            .child(bar(1.0, p.muted_foreground));
        let row = |selected: bool, tag| {
            h_flex()
                .gap_1()
                .px_1()
                .py_0p5()
                .rounded_sm()
                .when(selected, |this| this.bg(p.list_active))
                .child(div().size(rems(0.3)).rounded_full().bg(tag))
                .child(bar(2.5, p.foreground))
                .child(bar(1.0, p.muted_foreground))
        };
        let page = v_flex()
            .flex_1()
            .h_full()
            .p_1()
            .gap_0p5()
            .bg(p.background)
            .child(div().pb_0p5().child(bar(2.0, p.foreground)))
            .child(row(false, p.green))
            .child(row(true, p.blue))
            .child(row(false, p.yellow))
            .child(
                h_flex().pt_0p5().justify_end().child(
                    div()
                        .h(rems(0.5))
                        .w(rems(1.5))
                        .rounded_sm()
                        .bg(p.button_primary),
                ),
            );
        let group = SharedString::from(format!("settings-theme-{}", name.id()));
        v_flex()
            .id(group.clone())
            .group(group.clone())
            .test_support()
            .w_full()
            .min_w_0()
            .gap_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.selected = ROWS.iter().position(|r| *r == Row::Theme).unwrap_or(0);
                this.write(ViewEvent::SetColorTheme(name), window, cx);
            }))
            .child(
                div()
                    .id(SharedString::from(format!(
                        "settings-theme-{}-frame",
                        name.id()
                    )))
                    // The card takes its column's width and keeps the
                    // picture's shape.
                    .w_full()
                    .aspect_ratio(PREVIEW_ASPECT)
                    .rounded(ui.radius)
                    .border_2()
                    .border_color(ring)
                    .overflow_hidden()
                    .when(!picked, |this| {
                        this.group_hover(group.clone(), move |s| s.border_color(hover))
                            .group_active(group.clone(), move |s| s.border_color(pressed))
                    })
                    .child(h_flex().size_full().child(sidebar).child(page)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(label)
                    .truncate()
                    .child(name.label()),
            )
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
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
            // The workspace draws this view cached, so GPUI lays it out as
            // its own root, where `flex_1` does nothing. It fills by size.
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(crate::shell::backdrop::surface(
                theme.background,
                &[crate::shell::backdrop::Slot::Window],
                cx,
            ))
            .child(
                h_flex().w_full().justify_center().child(
                    v_flex()
                        .w_full()
                        .max_w(rems(40.))
                        .px_4()
                        .py_6()
                        .gap_1()
                        .child(div().px_3().pb_3().text_lg().child("Settings"))
                        .when_some(self.config_note.clone(), |el, note| {
                            el.child(
                                div()
                                    .id("settings-config-note")
                                    .px_3()
                                    .pb_3()
                                    .text_sm()
                                    .text_color(theme.muted_foreground)
                                    .child(note),
                            )
                        })
                        .children(rows)
                        .child(self.render_backgrounds(&values, window, cx))
                        .when_some(self.database.as_ref(), |el, db| {
                            el.child(render_database(db, cx))
                        }),
                ),
            )
    }
}

// LLM-567: the Background section.

use crate::settings::BackgroundValue;
use crate::shell::backdrop::{BackgroundConfig, SizeValue, Slot};

/// Label and `size` value.
const BG_SIZE: [(&str, &str); 4] = [
    ("Cover", "cover"),
    ("Contain", "contain"),
    ("Fill", "fill"),
    ("Actual size", "auto"),
];
const BG_POSITION: [(&str, &str); 9] = [
    ("Center", "center"),
    ("Top", "top"),
    ("Bottom", "bottom"),
    ("Left", "left"),
    ("Right", "right"),
    ("Top left", "top left"),
    ("Top right", "top right"),
    ("Bottom left", "bottom left"),
    ("Bottom right", "bottom right"),
];
const BG_REPEAT: [(&str, &str); 4] = [
    ("Once", "no-repeat"),
    ("Tile", "repeat"),
    ("Across", "repeat-x"),
    ("Down", "repeat-y"),
];

/// One dropdown entry: its label and the value written to `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Choice {
    label: &'static str,
    value: &'static str,
}

impl SelectItem for Choice {
    type Value = &'static str;

    fn title(&self) -> SharedString {
        self.label.into()
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

fn choices(list: &[(&'static str, &'static str)]) -> Vec<Choice> {
    list.iter()
        .map(|(label, value)| Choice { label, value })
        .collect()
}

/// Which background key a dropdown writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BgKey {
    Size,
    Position,
    Repeat,
}

impl BgKey {
    const ALL: [BgKey; 3] = [BgKey::Size, BgKey::Position, BgKey::Repeat];

    fn key(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Position => "position",
            Self::Repeat => "repeat",
        }
    }

    fn list(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Size => &BG_SIZE,
            Self::Position => &BG_POSITION,
            Self::Repeat => &BG_REPEAT,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Size => "Size",
            Self::Position => "Position",
            Self::Repeat => "Repeat",
        }
    }

    /// The entry that means what `c` holds now. A hand-written value
    /// with no entry, such as `size = "40%"`, has none. Values compare by
    /// meaning, so `left top` picks "Top left".
    fn current(self, c: &BackgroundConfig) -> Option<usize> {
        use crate::shell::backdrop::{parse_position, parse_repeat, parse_size};
        let list = self.list();
        match self {
            Self::Size => {
                let now = c
                    .size
                    .as_ref()
                    .map_or(Some(crate::shell::backdrop::BgSize::Cover), parse_size)?;
                list.iter()
                    .position(|(_, v)| parse_size(&SizeValue::Text(v.to_string())) == Some(now))
            }
            Self::Position => {
                let now = parse_position(c.position.as_deref().unwrap_or("center"))?;
                list.iter()
                    .position(|(_, v)| parse_position(v) == Some(now))
            }
            Self::Repeat => {
                let now = parse_repeat(c.repeat.as_deref().unwrap_or("no-repeat"))?;
                list.iter().position(|(_, v)| parse_repeat(v) == Some(now))
            }
        }
    }
}

#[cfg(test)]
impl SettingsView {
    /// The opacity field of `slot`.
    pub fn bg_opacity(&self, slot: Slot) -> Entity<InputState> {
        self.bg(slot).opacity.clone()
    }

    /// The `size`, `position` or `repeat` dropdown of `slot`.
    pub fn bg_select(&self, slot: Slot, key: &str) -> Entity<SelectState<Vec<Choice>>> {
        self.bg(slot)
            .selects
            .iter()
            .find(|(k, _)| k.key() == key)
            .expect("a dropdown")
            .1
            .clone()
    }
}

/// The controls of one slot, made once and kept.
pub struct BgControls {
    opacity: Entity<InputState>,
    selects: Vec<(BgKey, Entity<SelectState<Vec<Choice>>>)>,
}

fn slot_title(slot: Slot) -> &'static str {
    match slot {
        Slot::Window => "Whole window",
        Slot::Sidebar => "Sidebar",
        Slot::List => "Task list",
        Slot::Task => "Task panel",
    }
}

impl SettingsView {
    /// The opacity field and the three dropdowns of `slot`, wired to write
    /// `config.toml`.
    fn bg_controls(
        slot: Slot,
        subscriptions: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BgControls {
        let opacity = cx.new(|cx| {
            InputState::new(window, cx)
                .mask_pattern(whole_number())
                .min(0.)
                .max(100.)
        });
        // No internal stepping: a step event comes here and moves by 5.
        opacity.update(cx, |state, cx| state.set_step(None, window, cx));
        // Like zoom: a typed value applies on Enter or when the field
        // loses focus, not at each digit; the − and + buttons apply at once.
        subscriptions.push(cx.subscribe_in(
            &opacity,
            window,
            move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    this.apply_opacity(slot, window, cx);
                    this.take_focus(window, cx);
                }
                InputEvent::Blur => this.apply_opacity(slot, window, cx),
                InputEvent::Change | InputEvent::Focus => {}
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &opacity,
            window,
            move |this, _, event: &NumberInputEvent, window, cx| {
                let NumberInputEvent::Step(action) = event;
                this.step_opacity(slot, *action, window, cx);
            },
        ));
        let selects = BgKey::ALL
            .into_iter()
            .map(|key| {
                let state = cx.new(|cx| SelectState::new(choices(key.list()), None, window, cx));
                subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _, event: &SelectEvent<Vec<Choice>>, window, cx| {
                        let SelectEvent::Confirm(Some(value)) = event else {
                            return;
                        };
                        this.write(
                            ViewEvent::SetBackground {
                                slot,
                                key: key.key(),
                                value: Some(BackgroundValue::Text(value.to_string())),
                            },
                            window,
                            cx,
                        );
                    },
                ));
                (key, state)
            })
            .collect();
        BgControls { opacity, selects }
    }

    fn bg(&self, slot: Slot) -> &BgControls {
        &self
            .backgrounds
            .iter()
            .find(|(s, _)| *s == slot)
            .expect("every slot has controls")
            .1
    }

    /// Saves the typed opacity, 0 to 100, as 0 to 1.
    fn apply_opacity(&mut self, slot: Slot, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.bg(slot).opacity.clone();
        let typed = field.read(cx).value().trim().parse::<u32>().ok();
        let values = self.values(cx);
        let now = values
            .background
            .get(slot)
            .map(|c| c.opacity.unwrap_or(crate::shell::backdrop::DEFAULT_OPACITY));
        if let (Some(typed), Some(now)) = (typed, now) {
            let typed = typed.min(100);
            if typed != (now.clamp(0., 1.) * 100.).round() as u32 {
                self.write(
                    ViewEvent::SetBackground {
                        slot,
                        key: "opacity",
                        value: Some(BackgroundValue::Number(typed as f32 / 100.)),
                    },
                    window,
                    cx,
                );
                return;
            }
        }
        // Out of range, empty, or no change: show the saved value again.
        self.sync_backgrounds(window, cx);
    }

    /// The − and + buttons, and the arrow keys in the field: 5 at a time.
    fn step_opacity(
        &mut self,
        slot: Slot,
        action: StepAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let values = self.values(cx);
        let Some(c) = values.background.get(slot) else {
            return;
        };
        let now = (c
            .opacity
            .unwrap_or(crate::shell::backdrop::DEFAULT_OPACITY)
            .clamp(0., 1.)
            * 100.)
            .round() as i32;
        let next = match action {
            StepAction::Increment => (now / 5 + 1) * 5,
            StepAction::Decrement => ((now + 4) / 5 - 1) * 5,
        }
        .clamp(0, 100);
        if next != now {
            self.write(
                ViewEvent::SetBackground {
                    slot,
                    key: "opacity",
                    value: Some(BackgroundValue::Number(next as f32 / 100.)),
                },
                window,
                cx,
            );
        }
    }

    /// Shows what `config.toml` holds in each field and dropdown. A field
    /// the person is typing in keeps their text.
    fn sync_backgrounds(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.values(cx);
        for (slot, controls) in &self.backgrounds {
            let Some(c) = values.background.get(*slot) else {
                continue;
            };
            let field = controls.opacity.clone();
            if !field.focus_handle(cx).is_focused(window) {
                let opacity = c.opacity.unwrap_or(crate::shell::backdrop::DEFAULT_OPACITY);
                let text = ((opacity.clamp(0., 1.) * 100.).round() as u32).to_string();
                self.set_text(&field, text, window, cx);
            }
            for (key, state) in &controls.selects {
                let index = key.current(c).map(gpui_kit::component::IndexPath::new);
                if state.read(cx).selected_index(cx) != index {
                    state.update(cx, |s, cx| s.set_selected_index(index, window, cx));
                }
            }
        }
    }

    /// One block per slot: a preview, Choose image… and Remove, and the
    /// choices for an image that is set. Per-project, per-team and
    /// per-mode tables stay in `config.toml`.
    fn render_backgrounds(
        &self,
        values: &UiSettings,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.border;
        v_flex()
            .id("settings-backgrounds")
            .test_support()
            .mt_4()
            .px_3()
            .pt_4()
            .gap_3()
            .border_t_1()
            .border_color(border)
            .child(div().text_sm().child("Background images"))
            .child(div().text_xs().text_color(muted).child(
                "Each image draws behind its part of the window. Images per mode, \
                 project or team, and filters such as blur, are set in config.toml.",
            ))
            .children(
                Slot::ALL.into_iter().map(|slot| {
                    self.render_background(slot, values.background.get(slot), window, cx)
                }),
            )
            .into_any_element()
    }

    fn render_background(
        &self,
        slot: Slot,
        config: Option<&BackgroundConfig>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, radius, page) = (
            theme.muted_foreground,
            theme.border,
            theme.radius,
            theme.background,
        );
        let id = |what: &str| SharedString::from(format!("settings-bg-{}-{what}", slot.key()));
        let image = config
            .map(|c| c.image.trim().to_string())
            .filter(|i| !i.is_empty());
        let path = image
            .as_ref()
            .map(|i| crate::shell::backdrop::resolve_path(i, self.config_dir.as_deref()));
        let name: SharedString = match &path {
            Some(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
                .into(),
            None => "No image".into(),
        };
        let preview = div()
            .w(rems(6.))
            .h(rems(3.75))
            .flex_none()
            .rounded(radius)
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .bg(page)
            .when_some(path.clone(), |el, p| {
                el.child(crate::shell::backdrop::thumbnail(p))
            });
        let header = h_flex()
            .gap_3()
            .child(preview)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().child(slot_title(slot)))
                    .child(
                        div()
                            .id(id("name"))
                            .test_support()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(name),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_none()
                    .child(
                        Button::new(id("choose"))
                            .label("Choose image…")
                            .small()
                            .outline()
                            .tab_stop(false)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.write(ViewEvent::ChooseBackground(slot), window, cx);
                            })),
                    )
                    .child(
                        Button::new(id("remove"))
                            .label("Remove")
                            .small()
                            .ghost()
                            .tab_stop(false)
                            .disabled(image.is_none())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.write(
                                    ViewEvent::SetBackground {
                                        slot,
                                        key: "image",
                                        value: None,
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
            );
        if config.is_none() || image.is_none() {
            return v_flex().gap_2().child(header).into_any_element();
        }
        let controls = self.bg(slot);
        let theme = cx.theme();
        let focused = controls.opacity.focus_handle(cx).is_focused(window);
        let (hover, pressed) = (theme.muted_foreground, theme.foreground);
        let opacity_control = div()
            .id(id("opacity"))
            .test_support()
            .w(rems(8.))
            .h_8()
            .flex()
            .flex_col()
            .justify_center()
            .rounded(theme.radius)
            .border_1()
            .border_color(if focused { theme.ring } else { theme.input })
            .bg(theme.input_background())
            .hover(move |s| s.border_color(hover))
            .active(move |s| s.border_color(pressed))
            .child(
                NumberInput::new(&controls.opacity)
                    .small()
                    .appearance(false)
                    .suffix(div().text_xs().text_color(muted).child("%")),
            )
            .into_any_element();
        let dropdowns: Vec<(&'static str, AnyElement)> = controls
            .selects
            .iter()
            .map(|(key, state)| {
                (
                    key.label(),
                    div()
                        .w(rems(10.))
                        .child(
                            Select::new(state)
                                .id(id(key.key()))
                                .small()
                                .placeholder("Custom")
                                .menu_width(rems(10.)),
                        )
                        .into_any_element(),
                )
            })
            .collect();
        let line = |label: &'static str, control: AnyElement| {
            h_flex()
                .gap_3()
                .child(
                    div()
                        .w(rems(5.))
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(label),
                )
                .child(control)
        };
        v_flex()
            .gap_2()
            .child(header)
            .child(
                v_flex()
                    .pl(rems(6.75))
                    .gap_1p5()
                    .child(line("Opacity", opacity_control))
                    .children(dropdowns.into_iter().map(|(label, el)| line(label, el))),
            )
            .into_any_element()
    }
}

/// What the Database section says about the server's database.
fn database_line(db: &DatabaseInfo) -> SharedString {
    match (db.backend, db.host.as_deref()) {
        (DatabaseBackend::Postgres, Some(host)) => format!("Shared Postgres: {host}").into(),
        (DatabaseBackend::Postgres, None) => "Shared Postgres".into(),
        (DatabaseBackend::Sqlite, _) => "Local SQLite file".into(),
    }
}

/// The read-only Database section. The choice changes in the terminal,
/// because it moves data and stores a password.
fn render_database(db: &DatabaseInfo, cx: &App) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .id("settings-database")
        .mt_4()
        .px_3()
        .pt_4()
        .gap_1()
        .border_t_1()
        .border_color(theme.border)
        .child(div().text_sm().child("Database"))
        .child(
            div()
                .id("settings-database-line")
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(database_line(db)),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Change it with llmtodo db in a terminal."),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_counts_the_hidden_tasks() {
        assert_eq!(hidden_hint(true, 14, 0), "No closed task is this old");
        assert_eq!(hidden_hint(false, 14, 0), "No closed task is this old");
        assert_eq!(hidden_hint(true, 14, 1), "Hiding 1 closed task");
        assert_eq!(hidden_hint(true, 14, 23), "Hiding 23 closed tasks");
        assert_eq!(hidden_hint(false, 14, 1), "1 closed task is this old");
        assert_eq!(hidden_hint(false, 14, 23), "23 closed tasks are this old");
        assert_eq!(
            hidden_hint(true, 0, 3),
            "0 hides closed tasks at once. Hiding 3 closed tasks"
        );
    }
}
