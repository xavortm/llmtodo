//! The background image controls (LLM-567, LLM-580). One view serves two
//! places: the Background images section in Settings, which lists every
//! slot with the basic controls, and the floating Backgrounds panel,
//! which edits one slot at a time with every control while the app shows
//! the result behind it.
//!
//! Each change goes to the workspace as `ViewEvent::SetBackground`, which
//! writes one key of `[app.background.<slot>]` and applies it at once.
//! The view shows what `config.toml` holds, also after a hand edit.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _,
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    input::{InputEvent, InputState, MaskPattern, NumberInput, NumberInputEvent, StepAction},
    select::{Select, SelectEvent, SelectItem, SelectState},
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div,
    prelude::FluentBuilder as _, rems,
};

use crate::settings::{BackgroundValue, UiSettings};
use crate::shell::backdrop::{self, BackgroundConfig, SizeValue, Slot};
use crate::shell::view_events::{ViewEvent, ViewEvents};

pub const PANEL_CONTEXT: &str = "BackgroundPanel";

actions!(
    llmtodo_backgrounds,
    [
        /// Closes the floating Backgrounds panel.
        CloseBackgroundPanel,
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([gpui_kit::KeyBinding::new(
        "escape",
        CloseBackgroundPanel,
        Some(PANEL_CONTEXT),
    )]);
}

/// Where the view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Settings: every slot, with opacity, size, position and repeat.
    Settings,
    /// The floating panel: one slot, with every control.
    Panel,
}

// Number fields.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumKey {
    Opacity,
    Blur,
    Brightness,
    TintStrength,
}

impl NumKey {
    const ALL: [NumKey; 4] = [
        NumKey::Opacity,
        NumKey::Blur,
        NumKey::Brightness,
        NumKey::TintStrength,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Opacity => "opacity",
            Self::Blur => "blur",
            Self::Brightness => "brightness",
            Self::TintStrength => "tint_opacity",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Opacity => "Opacity",
            Self::Blur => "Blur",
            Self::Brightness => "Brightness",
            Self::TintStrength => "Tint strength",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Blur => "px",
            _ => "%",
        }
    }

    fn max(self) -> u32 {
        match self {
            Self::Opacity | Self::TintStrength => 100,
            Self::Blur => backdrop::MAX_BLUR as u32,
            Self::Brightness => (backdrop::MAX_BRIGHTNESS * 100.) as u32,
        }
    }

    fn step(self) -> u32 {
        match self {
            Self::Opacity | Self::TintStrength => 5,
            Self::Blur => 2,
            Self::Brightness => 10,
        }
    }

    /// Px stay px; shares show as percent.
    fn per_unit(self) -> f32 {
        match self {
            Self::Blur => 1.,
            _ => 100.,
        }
    }

    /// The value as the field shows it, with the default when unset.
    fn shown(self, c: &BackgroundConfig) -> u32 {
        let value = match self {
            Self::Opacity => c.opacity.unwrap_or(backdrop::DEFAULT_OPACITY),
            Self::Blur => c.blur.unwrap_or(0.),
            Self::Brightness => c.brightness.unwrap_or(1.),
            Self::TintStrength => c.tint_opacity.unwrap_or(backdrop::DEFAULT_TINT_OPACITY),
        };
        let shown = (value.max(0.) * self.per_unit()).round();
        if shown.is_finite() { shown as u32 } else { 0 }
    }

    fn value(self, shown: u32) -> BackgroundValue {
        BackgroundValue::Number(shown.min(self.max()) as f32 / self.per_unit())
    }
}

// Dropdowns.

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

const SIZE: [(&str, &str); 4] = [
    ("Cover", "cover"),
    ("Contain", "contain"),
    ("Fill", "fill"),
    ("Actual size", "auto"),
];
const POSITION: [(&str, &str); 9] = [
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
const REPEAT: [(&str, &str); 4] = [
    ("Once", "no-repeat"),
    ("Tile", "repeat"),
    ("Across", "repeat-x"),
    ("Down", "repeat-y"),
];
/// `page` and `accent` are the theme's colors when picked; `none` takes
/// the tint out.
const TINT: [(&str, &str); 5] = [
    ("None", "none"),
    ("Page color", "page"),
    ("Accent color", "accent"),
    ("Black", "#000000"),
    ("White", "#ffffff"),
];
const ATTACHMENT: [(&str, &str); 2] = [("Stays put", "fixed"), ("Scrolls", "scroll")];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgKey {
    Size,
    Position,
    Repeat,
    Tint,
    Attachment,
}

impl BgKey {
    const ALL: [BgKey; 5] = [
        BgKey::Size,
        BgKey::Position,
        BgKey::Repeat,
        BgKey::Tint,
        BgKey::Attachment,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Position => "position",
            Self::Repeat => "repeat",
            Self::Tint => "tint",
            Self::Attachment => "attachment",
        }
    }

    fn list(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Size => &SIZE,
            Self::Position => &POSITION,
            Self::Repeat => &REPEAT,
            Self::Tint => &TINT,
            Self::Attachment => &ATTACHMENT,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Size => "Size",
            Self::Position => "Position",
            Self::Repeat => "Repeat",
            Self::Tint => "Tint",
            Self::Attachment => "Scrolling",
        }
    }

    /// The entry that means what `c` holds now. A hand-written value
    /// with no entry, such as `size = "40%"`, has none. Values compare by
    /// meaning, so `left top` picks "Top left".
    fn current(self, c: &BackgroundConfig) -> Option<usize> {
        use crate::shell::backdrop::{parse_position, parse_repeat, parse_size};
        let list = self.list();
        let find = |v: &str| list.iter().position(|(_, x)| *x == v);
        match self {
            Self::Size => {
                let now = c
                    .size
                    .as_ref()
                    .map_or(Some(backdrop::BgSize::Cover), parse_size)?;
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
            Self::Tint => match c.tint.as_deref().map(str::trim) {
                None | Some("") => find("none"),
                Some(t) => find(&t.to_ascii_lowercase()),
            },
            Self::Attachment => find(
                &c.attachment
                    .as_deref()
                    .unwrap_or("fixed")
                    .trim()
                    .to_ascii_lowercase(),
            ),
        }
    }

    /// Whether `mode` shows this dropdown for `slot`.
    fn shows(self, mode: Mode, slot: Slot) -> bool {
        match self {
            Self::Size | Self::Position | Self::Repeat => true,
            Self::Tint => mode == Mode::Panel,
            Self::Attachment => mode == Mode::Panel && matches!(slot, Slot::List | Slot::Task),
        }
    }
}

/// The controls of one slot, made once and kept.
pub struct BgControls {
    numbers: Vec<(NumKey, Entity<InputState>)>,
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

fn short_title(slot: Slot) -> &'static str {
    match slot {
        Slot::Window => "Window",
        Slot::Sidebar => "Sidebar",
        Slot::List => "List",
        Slot::Task => "Task",
    }
}

/// `#rrggbb` for a theme color.
fn hex(color: gpui_kit::Hsla) -> String {
    let c = color.to_rgb();
    let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
}

pub struct BackgroundEditor {
    events: ViewEvents,
    values: Entity<UiSettings>,
    mode: Mode,
    /// The slot the panel edits.
    slot: Slot,
    /// The folder of `config.toml`: a relative image path starts there.
    config_dir: Option<std::path::PathBuf>,
    controls: Vec<(Slot, BgControls)>,
    pub focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl BackgroundEditor {
    pub fn new(
        mode: Mode,
        events: ViewEvents,
        values: Entity<UiSettings>,
        config_dir: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = vec![
            // A change, also a hand edit of config.toml, shows at once.
            cx.observe_in(&values, window, |this, _, window, cx| {
                this.sync(window, cx);
                cx.notify();
            }),
        ];
        let controls = Slot::ALL
            .into_iter()
            .map(|slot| (slot, Self::make(slot, &mut subscriptions, window, cx)))
            .collect();
        let mut this = Self {
            events,
            values,
            mode,
            slot: Slot::Window,
            config_dir,
            controls,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.sync(window, cx);
        this
    }

    pub fn set_config_dir(&mut self, dir: Option<std::path::PathBuf>) {
        self.config_dir = dir;
    }

    pub fn slot(&self) -> Slot {
        self.slot
    }

    pub fn set_slot(&mut self, slot: Slot, cx: &mut Context<Self>) {
        self.slot = slot;
        cx.notify();
    }

    /// The `opacity`, `blur`, `brightness` or `tint_opacity` field.
    pub fn number(&self, slot: Slot, key: &str) -> Entity<InputState> {
        self.of(slot)
            .numbers
            .iter()
            .find(|(k, _)| k.key() == key)
            .expect("a field")
            .1
            .clone()
    }

    /// The `size`, `position`, `repeat`, `tint` or `attachment` dropdown.
    pub fn select(&self, slot: Slot, key: &str) -> Entity<SelectState<Vec<Choice>>> {
        self.of(slot)
            .selects
            .iter()
            .find(|(k, _)| k.key() == key)
            .expect("a dropdown")
            .1
            .clone()
    }

    fn of(&self, slot: Slot) -> &BgControls {
        &self
            .controls
            .iter()
            .find(|(s, _)| *s == slot)
            .expect("every slot has controls")
            .1
    }

    fn write(&mut self, event: ViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.events.emit(event, window, cx);
        cx.notify();
    }

    fn set(
        &mut self,
        slot: Slot,
        key: &'static str,
        value: Option<BackgroundValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.write(ViewEvent::SetBackground { slot, key, value }, window, cx);
    }

    /// The fields and dropdowns of `slot`, wired to write `config.toml`.
    fn make(
        slot: Slot,
        subscriptions: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BgControls {
        let numbers = NumKey::ALL
            .into_iter()
            .map(|key| {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .mask_pattern(MaskPattern::Number {
                            separator: None,
                            fraction: Some(0),
                        })
                        .min(0.)
                        .max(f64::from(key.max()))
                });
                // No internal stepping: steps come here, a fixed amount.
                input.update(cx, |state, cx| state.set_step(None, window, cx));
                // A typed value applies on Enter or when the field loses
                // focus, not at each digit.
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => {
                            this.apply_number(slot, key, window, cx);
                            window.focus(&this.focus, cx);
                        }
                        InputEvent::Blur => this.apply_number(slot, key, window, cx),
                        InputEvent::Change | InputEvent::Focus => {}
                    },
                ));
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event: &NumberInputEvent, window, cx| {
                        let NumberInputEvent::Step(action) = event;
                        this.step_number(slot, key, *action, window, cx);
                    },
                ));
                (key, input)
            })
            .collect();
        let selects = BgKey::ALL
            .into_iter()
            .map(|key| {
                let items: Vec<Choice> = key
                    .list()
                    .iter()
                    .map(|(label, value)| Choice { label, value })
                    .collect();
                let state = cx.new(|cx| SelectState::new(items, None, window, cx));
                subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _, event: &SelectEvent<Vec<Choice>>, window, cx| {
                        let SelectEvent::Confirm(Some(value)) = event else {
                            return;
                        };
                        let theme = cx.theme();
                        let value = match (key, *value) {
                            (BgKey::Tint, "none") => None,
                            (BgKey::Tint, "page") => Some(hex(theme.background)),
                            (BgKey::Tint, "accent") => Some(hex(theme.accent)),
                            (_, v) => Some(v.to_string()),
                        };
                        this.set(
                            slot,
                            key.key(),
                            value.map(BackgroundValue::Text),
                            window,
                            cx,
                        );
                    },
                ));
                (key, state)
            })
            .collect();
        BgControls { numbers, selects }
    }

    /// Saves a typed number. Out of range, empty or the same: shows the
    /// saved value again.
    fn apply_number(
        &mut self,
        slot: Slot,
        key: NumKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let field = self.number(slot, key.key());
        let typed = field.read(cx).value().trim().parse::<u32>().ok();
        let values = self.values.read(cx).clone();
        if let (Some(typed), Some(c)) = (typed, values.background.get(slot)) {
            let typed = typed.min(key.max());
            if typed != key.shown(c) {
                self.set(slot, key.key(), Some(key.value(typed)), window, cx);
                return;
            }
        }
        self.sync(window, cx);
    }

    /// The − and + buttons, and the arrow keys in a field.
    fn step_number(
        &mut self,
        slot: Slot,
        key: NumKey,
        action: StepAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let values = self.values.read(cx).clone();
        let Some(c) = values.background.get(slot) else {
            return;
        };
        let (now, step) = (key.shown(c) as i64, key.step() as i64);
        let next = match action {
            StepAction::Increment => (now / step + 1) * step,
            StepAction::Decrement => ((now + step - 1) / step - 1) * step,
        }
        .clamp(0, i64::from(key.max())) as u32;
        if next as i64 != now {
            self.set(slot, key.key(), Some(key.value(next)), window, cx);
        }
    }

    /// Shows what `config.toml` holds. A field being typed in keeps its
    /// text.
    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.values.read(cx).clone();
        for (slot, controls) in &self.controls {
            let Some(c) = values.background.get(*slot) else {
                continue;
            };
            for (key, field) in &controls.numbers {
                if field.focus_handle(cx).is_focused(window) {
                    continue;
                }
                let text = key.shown(c).to_string();
                if field.read(cx).value().as_ref() != text.as_str() {
                    field.update(cx, |state, cx| state.set_value(text, window, cx));
                }
            }
            for (key, state) in &controls.selects {
                let index = key.current(c).map(gpui_kit::component::IndexPath::new);
                if state.read(cx).selected_index(cx) != index {
                    state.update(cx, |s, cx| s.set_selected_index(index, window, cx));
                }
            }
        }
    }

    fn number_field(
        &self,
        slot: Slot,
        key: NumKey,
        input: &Entity<InputState>,
        window: &Window,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let (hover, pressed, muted) = (
            theme.muted_foreground,
            theme.foreground,
            theme.muted_foreground,
        );
        let focused = input.focus_handle(cx).is_focused(window);
        div()
            .id(SharedString::from(format!(
                "settings-bg-{}-{}",
                slot.key(),
                key.key()
            )))
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
                NumberInput::new(input)
                    .small()
                    .appearance(false)
                    .suffix(div().text_xs().text_color(muted).child(key.suffix())),
            )
            .into_any_element()
    }

    /// The preview, the name, Choose image… and Remove.
    fn render_header(
        &self,
        slot: Slot,
        config: Option<&BackgroundConfig>,
        cx: &mut Context<Self>,
    ) -> (AnyElement, bool) {
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
            .map(|i| backdrop::resolve_path(i, self.config_dir.as_deref()));
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
            .when_some(path, |el, p| el.child(backdrop::thumbnail(p)));
        let header = h_flex()
            .gap_3()
            .child(preview)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .when(self.mode == Mode::Settings, |el| {
                        el.child(div().text_sm().child(slot_title(slot)))
                    })
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
                                this.set(slot, "image", None, window, cx);
                            })),
                    ),
            )
            .into_any_element();
        (header, image.is_some())
    }

    /// One slot: the header and, with an image, its controls.
    fn render_slot(&self, slot: Slot, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let values = self.values.read(cx).clone();
        let config = values.background.get(slot);
        let (header, set) = self.render_header(slot, config, cx);
        let muted = cx.theme().muted_foreground;
        let mode = self.mode;
        let controls = self.of(slot);
        let line = |label: &'static str, control: AnyElement| {
            h_flex()
                .gap_3()
                .child(
                    div()
                        .w(rems(6.))
                        .flex_none()
                        .text_xs()
                        .text_color(muted)
                        .child(label),
                )
                .child(control)
        };
        let mut lines: Vec<AnyElement> = Vec::new();
        if set {
            let number = |key: NumKey| {
                let (_, input) = controls
                    .numbers
                    .iter()
                    .find(|(k, _)| *k == key)
                    .expect("a field");
                line(key.label(), self.number_field(slot, key, input, window, cx))
                    .into_any_element()
            };
            let dropdown = |key: BgKey| {
                let (_, state) = controls
                    .selects
                    .iter()
                    .find(|(k, _)| *k == key)
                    .expect("a dropdown");
                line(
                    key.label(),
                    div()
                        .w(rems(10.))
                        .child(
                            Select::new(state)
                                .id(SharedString::from(format!(
                                    "settings-bg-{}-{}",
                                    slot.key(),
                                    key.key()
                                )))
                                .small()
                                .placeholder("Custom")
                                .menu_width(rems(10.)),
                        )
                        .into_any_element(),
                )
                .into_any_element()
            };
            lines.push(number(NumKey::Opacity));
            for key in [BgKey::Size, BgKey::Position, BgKey::Repeat] {
                lines.push(dropdown(key));
            }
            if mode == Mode::Panel {
                if BgKey::Attachment.shows(mode, slot) {
                    lines.push(dropdown(BgKey::Attachment));
                }
                lines.push(number(NumKey::Blur));
                lines.push(number(NumKey::Brightness));
                let grayscale = config.and_then(|c| c.grayscale).unwrap_or(false);
                lines.push(
                    line(
                        "Grayscale",
                        Switch::new(SharedString::from(format!(
                            "settings-bg-{}-grayscale",
                            slot.key()
                        )))
                        .small()
                        .checked(grayscale)
                        .on_click(cx.listener(move |this, on: &bool, window, cx| {
                            this.set(
                                slot,
                                "grayscale",
                                Some(BackgroundValue::Bool(*on)),
                                window,
                                cx,
                            );
                        }))
                        .into_any_element(),
                    )
                    .into_any_element(),
                );
                lines.push(dropdown(BgKey::Tint));
                if config.is_some_and(|c| c.tint.as_deref().is_some_and(|t| !t.trim().is_empty())) {
                    lines.push(number(NumKey::TintStrength));
                }
            }
        }
        let indent = if mode == Mode::Settings {
            rems(6.75)
        } else {
            rems(0.)
        };
        v_flex()
            .gap_2()
            .child(header)
            .when(!lines.is_empty(), |el| {
                el.child(v_flex().pl(indent).gap_1p5().children(lines))
            })
            .into_any_element()
    }
}

impl Render for BackgroundEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        match self.mode {
            Mode::Settings => v_flex()
                .id("settings-backgrounds")
                .test_support()
                .gap_3()
                .child(
                    h_flex()
                        .justify_between()
                        .child(div().text_sm().child("Background images"))
                        .child(
                            Button::new("settings-bg-edit-live")
                                .label("Edit live…")
                                .small()
                                .outline()
                                .tab_stop(false)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.write(ViewEvent::EditBackgroundsLive, window, cx);
                                })),
                        ),
                )
                .child(div().text_xs().text_color(muted).child(
                    "Each image draws behind its part of the window. Edit live shows \
                     the app while you change them. Images per mode, project or team \
                     are set in config.toml.",
                ))
                .children(
                    Slot::ALL
                        .into_iter()
                        .map(|slot| self.render_slot(slot, window, cx)),
                )
                .into_any_element(),
            Mode::Panel => {
                let current = self.slot;
                let picker = Slot::ALL
                    .into_iter()
                    .fold(
                        ButtonGroup::new("bg-panel-slots")
                            .outline()
                            .compact()
                            .small(),
                        |group, slot| {
                            group.child(
                                Button::new(SharedString::from(format!(
                                    "bg-panel-slot-{}",
                                    slot.key()
                                )))
                                .label(short_title(slot))
                                .tab_stop(false)
                                .selected(slot == current),
                            )
                        },
                    )
                    .on_click(cx.listener(|this, clicked: &Vec<usize>, _, cx| {
                        if let Some(slot) = clicked.first().and_then(|ix| Slot::ALL.get(*ix)) {
                            this.set_slot(*slot, cx);
                        }
                    }));
                v_flex()
                    .id("bg-panel-body")
                    .test_support()
                    .key_context(PANEL_CONTEXT)
                    .track_focus(&self.focus)
                    .gap_3()
                    .child(picker)
                    .child(self.render_slot(current, window, cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Images per mode, project or team are set in config.toml."),
                    )
                    .into_any_element()
            }
        }
    }
}
