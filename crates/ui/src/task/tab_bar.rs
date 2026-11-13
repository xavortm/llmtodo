//! The task view's tab bar. It shows the tabs that fit, from the left, and
//! puts the rest under one "More (N)" button at its right end (LLM-197).
//!
//! The tabs are our own, not GPUI Kit's: a kit tab has a fixed height in
//! pixels, so its label runs out of room when the app zooms in. These are
//! sized in rems, like the column titles, and grow with the text.
//!
//! Which tabs fit depends on the width of each label. The bar measures the
//! labels with the text system as it renders, and a canvas records the room
//! the tabs had after layout. When that room changes which tabs fit, the
//! canvas asks for one more frame. The room does not depend on the tabs, so
//! the second frame keeps the same tabs and no loop starts.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable, Sizable as _, h_flex,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyElement, App, Div, EntityId, InteractiveElement, Interactivity, IntoElement,
    ParentElement as _, Pixels, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};

use crate::shell::view_events::{ViewEvent, ViewEvents};
use crate::shell::widgets;

/// A tab's horizontal padding on each side, in rem: `px_3`.
const TAB_PAD_REM: f32 = 0.75;
/// The gap between a tab's icon and its label, in rem: `gap_1`.
const GAP_REM: f32 = 0.25;
/// An extra-small icon's width, in rem: `size_3`.
const ICON_REM: f32 = 0.75;
/// The tab labels' text size, in rem: `text_sm`.
const TEXT_REM: f32 = 0.875;

/// How many tabs, from the left, fit in `room`. `tabs` holds each tab's
/// width. When some tabs do not fit, the More button takes `more(hidden)`
/// of the room, where `hidden` is the number of tabs under it. A room of
/// zero means the bar has not been laid out yet, so every tab shows.
pub fn tabs_that_fit(tabs: &[Pixels], room: Pixels, more: impl Fn(usize) -> Pixels) -> usize {
    let total: Pixels = tabs.iter().sum();
    if room <= Pixels::ZERO || total <= room {
        return tabs.len();
    }
    let mut used = Pixels::ZERO;
    let mut fit = 0;
    for (ix, width) in tabs.iter().enumerate() {
        let hidden = tabs.len() - ix - 1;
        if used + *width + more(hidden) > room {
            break;
        }
        used += *width;
        fit = ix + 1;
    }
    fit
}

/// The More button's label for `hidden` tabs.
pub fn more_label(hidden: usize) -> String {
    format!("More ({hidden})")
}

/// The widths of the bar's parts at the current text size.
struct Widths {
    tabs: Vec<Pixels>,
    /// The More button's width for 1, 2, ... hidden tabs.
    more: Vec<Pixels>,
}

impl Widths {
    fn measure(titles: &[(String, Option<&'static str>)], window: &Window) -> Self {
        let rem = window.rem_size();
        let label = |text: &str| {
            let run = window.text_style().to_run(text.len());
            window
                .text_system()
                .layout_line(text, rem * TEXT_REM, &[run], None)
                .width
                .ceil()
        };
        let pad = rem * (2. * TAB_PAD_REM);
        let icon = rem * (GAP_REM + ICON_REM);
        let tabs = titles
            .iter()
            .map(|(title, read_only)| {
                pad + label(title)
                    + if read_only.is_some() {
                        icon
                    } else {
                        Pixels::ZERO
                    }
            })
            .collect();
        let more = (1..=titles.len())
            .map(|hidden| pad + icon + label(&more_label(hidden)))
            .collect();
        Self { tabs, more }
    }

    fn fit(&self, room: Pixels) -> usize {
        tabs_that_fit(&self.tabs, room, |hidden| {
            hidden
                .checked_sub(1)
                .and_then(|ix| self.more.get(ix))
                .copied()
                .unwrap_or_default()
        })
    }
}

/// What the tab bar shows.
pub struct TabBar<'a> {
    pub titles: &'a [(String, Option<&'static str>)],
    pub active: usize,
    /// The Edit and Preview buttons, at the bar's right end.
    pub toggle: Option<AnyElement>,
    pub events: ViewEvents,
    /// The room the tabs had in the last frame.
    pub room: Rc<Cell<Pixels>>,
    /// The task view, which draws again when the room changes the fit.
    pub view: EntityId,
}

impl TabBar<'_> {
    pub fn render(self, window: &Window, cx: &App) -> impl IntoElement + use<> {
        let widths = Widths::measure(self.titles, window);
        let shown = widths.fit(self.room.get());
        let hidden: Vec<(usize, String)> = self
            .titles
            .iter()
            .enumerate()
            .skip(shown)
            .map(|(ix, (title, _))| (ix, title.clone()))
            .collect();
        let active = self.active;
        let events = self.events.clone();
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        let accent = theme.primary;
        let hover = theme.foreground.opacity(0.08);
        let pressed = theme.foreground.opacity(0.16);
        let colors = TabColors {
            fg,
            muted,
            accent,
            hover,
            pressed,
        };
        let room = self.room.clone();
        let view = self.view;
        // Records the room after layout. When it changes which tabs fit,
        // the task view draws again with the right ones.
        let measure = gpui_kit::canvas(
            move |bounds, _, cx| {
                room.set(bounds.size.width);
                if widths.fit(bounds.size.width) != shown {
                    cx.notify(view);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let more = (!hidden.is_empty()).then(|| {
            let holds_active = hidden.iter().any(|(ix, _)| *ix == active);
            let label = more_label(hidden.len());
            let events = self.events.clone();
            MoreTab::new(label, colors)
                .selected(holds_active)
                .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                    hidden.iter().fold(menu, |menu, (ix, title)| {
                        let events = events.clone();
                        let ix = *ix;
                        menu.item(
                            PopupMenuItem::new(title.clone())
                                .checked(ix == active)
                                .on_click(move |_, window, cx| {
                                    events.emit(ViewEvent::SetTab(ix), window, cx);
                                }),
                        )
                    })
                })
        });
        h_flex()
            .id("detail-tabs")
            .test_support()
            .flex_none()
            .w_full()
            .h_8()
            .px_1()
            .text_sm()
            .border_b_1()
            .border_color(border)
            // The bar's px_1 plus each tab's px_3 puts the first label on
            // the content's px_4 edge. The tabs' room is the bar less the
            // Edit and Preview buttons, so it never depends on the tabs.
            .child(
                h_flex()
                    .id("detail-tabs-room")
                    .test_support()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .child(measure)
                    .children(self.titles.iter().take(shown).enumerate().map(
                        |(ix, (title, read_only))| {
                            tab(ix, title, *read_only, ix == active, colors, &events, cx)
                        },
                    ))
                    .child(div().flex_1())
                    .children(more),
            )
            .when_some(self.toggle, |bar, toggle| {
                bar.child(
                    h_flex()
                        .flex_none()
                        .h_full()
                        .items_center()
                        .pr_1()
                        .child(toggle),
                )
            })
    }
}

#[derive(Clone, Copy)]
struct TabColors {
    fg: gpui_kit::Hsla,
    muted: gpui_kit::Hsla,
    accent: gpui_kit::Hsla,
    hover: gpui_kit::Hsla,
    pressed: gpui_kit::Hsla,
}

/// The shape every tab and the More button share.
fn tab_base(id: impl Into<gpui_kit::ElementId>, selected: bool, c: TabColors) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .h_full()
        .gap_1()
        .px_3()
        .cursor_pointer()
        .whitespace_nowrap()
        .border_b_2()
        .border_color(if selected {
            c.accent
        } else {
            gpui_kit::transparent_black()
        })
        .text_color(if selected { c.fg } else { c.muted })
        .hover(move |s| s.bg(c.hover).text_color(c.fg))
        .active(move |s| s.bg(c.pressed))
}

fn tab(
    ix: usize,
    title: &str,
    read_only: Option<&'static str>,
    selected: bool,
    c: TabColors,
    events: &ViewEvents,
    cx: &App,
) -> impl IntoElement + use<> {
    let events = events.clone();
    tab_base(("detail-tab", ix), selected, c)
        .test_support()
        .when(read_only.is_some(), |tab| {
            tab.child(
                div()
                    .id(("tab-read-only", ix))
                    .test_support()
                    .child(super::task_view::read_only_eye(c.muted)),
            )
            .tooltip(super::task_view::read_only_card(title))
        })
        .child(
            div()
                .when(read_only.is_some(), |d| widgets::card_underline(d, cx))
                .child(title.to_string()),
        )
        .on_click(move |_, window, cx| {
            events.emit(ViewEvent::SetTab(ix), window, cx);
        })
}

/// The "More (N)" button. It looks like a tab, so it shows the tab
/// underline when it holds the open tab, and the pressed fill while its
/// menu is open.
#[derive(IntoElement)]
struct MoreTab {
    base: Stateful<Div>,
    label: SharedString,
    colors: TabColors,
    holds_active: bool,
    open: bool,
}

impl MoreTab {
    fn new(label: String, colors: TabColors) -> Self {
        Self {
            base: div().id("detail-tabs-more"),
            label: label.into(),
            colors,
            holds_active: false,
            open: false,
        }
    }
}

impl Selectable for MoreTab {
    /// Selected means it holds the open tab.
    fn selected(mut self, selected: bool) -> Self {
        self.holds_active = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.holds_active
    }

    fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    fn is_open(&self) -> bool {
        self.open
    }
}

impl Styled for MoreTab {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for MoreTab {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl DropdownMenu for MoreTab {}

impl RenderOnce for MoreTab {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let c = self.colors;
        let shape = tab_base("detail-tabs-more-tab", self.holds_active, c)
            .test_support()
            .when(self.open, |d| d.bg(c.pressed).text_color(c.fg))
            .child(self.label)
            .child(Icon::new(IconName::ChevronDown).xsmall());
        self.base.h_full().flex_none().test_support().child(shape)
    }
}
