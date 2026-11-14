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
//!
//! With animations on, the tabs share one hover background that slides to
//! the tab under the pointer (LLM-220). See [`TabHover`].

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable, Sizable as _, h_flex,
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyElement, App, Bounds, ContentMask, Context, Div, Entity, EntityId, InteractiveElement,
    Interactivity, IntoElement, ParentElement as _, Pixels, Render, RenderOnce, SharedString,
    Stateful, StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};

use crate::shell::motion::{self, Glide};
use crate::shell::view_events::{ViewEvent, ViewEvents};
use crate::shell::widgets;

/// A tab's hover fill: the foreground at this opacity.
const HOVER_ALPHA: f32 = 0.08;
/// A tab's fill while pressed: the foreground at this opacity.
const PRESSED_ALPHA: f32 = 0.16;

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
    /// The shared hover background.
    pub hover: Entity<TabHover>,
}

impl TabBar<'_> {
    pub fn render(self, window: &Window, cx: &mut App) -> impl IntoElement + use<> {
        self.hover.update(cx, |hover, _| hover.drawn = true);
        // Off, each tab shows its own hover fill, as before LLM-220.
        let hover = motion::on(cx).then(|| self.hover.clone());
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
        let colors = TabColors {
            fg,
            muted,
            accent,
            hover: theme.foreground.opacity(HOVER_ALPHA),
            pressed: theme.foreground.opacity(PRESSED_ALPHA),
            shared: hover.is_some(),
            pressed_under: theme.foreground.opacity(under(PRESSED_ALPHA, HOVER_ALPHA)),
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
        let has_more = !hidden.is_empty();
        let more = has_more.then(|| {
            let holds_active = hidden.iter().any(|(ix, _)| *ix == active);
            let label = more_label(hidden.len());
            let events = self.events.clone();
            MoreTab::new(label, colors, hover.clone().map(|hover| (hover, shown)))
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
                    // Records where each tab is, for the shared hover
                    // background. The children are the measure canvas,
                    // which fills the room, the tabs, a spacer and the
                    // More button.
                    .when_some(hover.clone(), |room, hover| {
                        room.on_children_prepainted(move |bounds, _, cx| {
                            hover.update(cx, |hover, _| hover.record(&bounds, shown, has_more));
                        })
                    })
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
                            let selected = ix == active;
                            let hover = hover.clone();
                            tab(ix, title, *read_only, selected, colors, &events, hover, cx)
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
    /// Whether the tabs share the sliding hover background. The tabs then
    /// draw no hover fill of their own.
    shared: bool,
    /// The pressed fill under the shared background, so the two together
    /// match `pressed`.
    pressed_under: gpui_kit::Hsla,
}

/// The opacity a fill under a see-through `over` fill of the same color
/// needs, so the two together show at `total`.
fn under(total: f32, over: f32) -> f32 {
    1. - (1. - total) / (1. - over)
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
        .map(|tab| {
            if c.shared {
                tab.hover(move |s| s.text_color(c.fg))
                    .active(move |s| s.bg(c.pressed_under))
            } else {
                tab.hover(move |s| s.bg(c.hover).text_color(c.fg))
                    .active(move |s| s.bg(c.pressed))
            }
        })
}

/// Tells the shared hover background when the pointer enters or leaves
/// the item `ix`.
fn track_hover(el: Stateful<Div>, hover: Option<Entity<TabHover>>, ix: usize) -> Stateful<Div> {
    el.when_some(hover, |el, hover| {
        el.on_hover(move |hovered, _, cx| {
            hover.update(cx, |hover, cx| hover.hover(ix, *hovered, cx));
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn tab(
    ix: usize,
    title: &str,
    read_only: Option<&'static str>,
    selected: bool,
    c: TabColors,
    events: &ViewEvents,
    hover: Option<Entity<TabHover>>,
    cx: &App,
) -> impl IntoElement + use<> {
    let events = events.clone();
    track_hover(tab_base(("detail-tab", ix), selected, c), hover, ix)
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
    /// The shared hover background, and the button's index in it.
    hover: Option<(Entity<TabHover>, usize)>,
    holds_active: bool,
    open: bool,
}

impl MoreTab {
    fn new(label: String, colors: TabColors, hover: Option<(Entity<TabHover>, usize)>) -> Self {
        Self {
            base: div().id("detail-tabs-more"),
            label: label.into(),
            colors,
            hover,
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
        let (hover, ix) = self.hover.unzip();
        let shape = tab_base("detail-tabs-more-tab", self.holds_active, c);
        let shape = track_hover(shape, hover, ix.unwrap_or_default())
            .test_support()
            .when(self.open, |d| d.bg(c.pressed).text_color(c.fg))
            .child(self.label)
            .child(Icon::new(IconName::ChevronDown).xsmall());
        self.base.h_full().flex_none().test_support().child(shape)
    }
}

/// The tab bar's shared hover background, with animations on (LLM-220).
/// It slides to the tab under the pointer and takes its width.
///
/// It is its own view, which the workspace draws over the task view. So
/// its frames draw only this view and never the task view, which stays
/// cached. The tab bar records where each tab is as it lays out. The fill
/// is the tabs' see-through hover color, and a hovered label already has
/// the foreground color, so a fill over the tab looks the same as one
/// behind it.
#[derive(Default)]
pub struct TabHover {
    /// Whether the task view's last render drew the tab bar.
    drawn: bool,
    /// The tabs' room, in window coordinates, from the last layout.
    room: Bounds<Pixels>,
    /// Each tab's bounds, then the More button's, relative to the room.
    items: Vec<Bounds<Pixels>>,
    glide: Glide,
}

impl TabHover {
    /// The task view calls this as it starts to render. Its tab bar sets
    /// it back, if it draws one.
    pub fn start_frame(&mut self) {
        self.drawn = false;
    }

    /// Where the hover background is now, in window coordinates, and its
    /// opacity. None when it does not show.
    pub fn shown(&self, cx: &App) -> Option<(Bounds<Pixels>, f32)> {
        if !self.drawn || !motion::on(cx) {
            return None;
        }
        let now = cx.background_executor().now();
        let (rect, alpha) = self.glide.sample(&self.items, now).0?;
        Some((
            Bounds::new(rect.origin + self.room.origin, rect.size),
            alpha,
        ))
    }

    fn record(&mut self, children: &[Bounds<Pixels>], shown: usize, has_more: bool) {
        let Some(room) = children.first().copied() else {
            return;
        };
        let relative = |b: &Bounds<Pixels>| Bounds::new(b.origin - room.origin, b.size);
        self.room = room;
        self.items.clear();
        self.items
            .extend(children.iter().skip(1).take(shown).map(relative));
        if has_more && let Some(more) = children.last() {
            self.items.push(relative(more));
        }
    }

    fn hover(&mut self, ix: usize, hovered: bool, cx: &mut Context<Self>) {
        let item = if hovered {
            Some(ix)
        } else if self.glide.hovered() == Some(ix) {
            None
        } else {
            // The pointer already moved on to another tab.
            return;
        };
        let now = cx.background_executor().now();
        self.glide.hover(item, &self.items, now);
        cx.notify();
    }
}

impl Render for TabHover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        if !self.drawn || !motion::on(cx) {
            // The bar went away, or animations are off: a later bar starts
            // clear.
            self.glide.clear();
        } else if self
            .glide
            .sample(&self.items, cx.background_executor().now())
            .1
        {
            window.request_animation_frame();
        }
        let this = cx.entity();
        // It paints after the task view has laid out, so it uses this
        // frame's tab bounds.
        gpui_kit::canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let hover = this.read(cx);
                let Some((rect, alpha)) = hover.shown(cx) else {
                    return;
                };
                let color = cx.theme().foreground.opacity(HOVER_ALPHA * alpha);
                let mask = ContentMask { bounds: hover.room };
                window.with_content_mask(Some(mask), |window| {
                    window.paint_quad(gpui_kit::fill(rect, color));
                });
            },
        )
        .absolute()
        .size_0()
    }
}
