//! LLM-560: a short strip under a project's list. Each day is a 10 px
//! slot, oldest on the left: the tasks created that day rise above the
//! middle line, and the tasks completed that day hang below it. A click on
//! a day opens the day's tasks over the window. Enter or a click opens one,
//! and Esc closes the list.
//!
//! The strip is an experiment: if it does not earn its room, remove this
//! file and its two places in the workspace.

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    App, Context, Entity, EventEmitter, FocusHandle, Hsla, InteractiveElement as _, IntoElement,
    KeyBinding, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, actions, div, point, prelude::*, px,
};
use jiff::civil;

use crate::issues::list::IssueList;
use crate::model::data::{Issue, IssueId};
use crate::model::scope::Scope;
use crate::model::store::IssueStore;

actions!(day_tasks, [DayDown, DayUp, DayOpen, DayClose]);

pub const CONTEXT: &str = "DayTasks";

/// Created bars are fainter than completed ones, so finished work reads
/// first.
const CREATED_ALPHA: f32 = 0.55;

/// The strip's sizes at one zoom level. They grow with the app's zoom, as
/// rems do, but each one rounds to whole px, so bars stay sharp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub zoom: f32,
    /// One day's slot: 10 px at 100%.
    pub slot: f32,
    /// The bar inside it: 7 px at 100%, which leaves 5 px inside the
    /// outline for two dots with even gaps at 1x, 1.5x and 2x.
    pub bar: f32,
    /// The bar's offset in its slot.
    pub inset: f32,
    pub pad_top: f32,
    pub pad_bottom: f32,
    /// The row under the bars that holds each week's date.
    pub label_row: f32,
    pub label_font: f32,
    /// The room above the line, and the same below it.
    pub half: f32,
    /// The whole strip.
    pub height: f32,
}

impl Metrics {
    pub fn at(zoom: f32) -> Self {
        let r = |v: f32| (v * zoom).round();
        let slot = r(10.);
        let bar = r(7.).min(slot - 2.);
        let (pad_top, pad_bottom, label_row, half) = (r(8.), r(5.), r(14.), r(30.));
        Self {
            zoom,
            slot,
            bar,
            inset: ((slot - bar) / 2.).floor(),
            pad_top,
            pad_bottom,
            label_row,
            label_font: 10. * zoom,
            half,
            height: pad_top + half * 2. + 1. + label_row + pad_bottom,
        }
    }

    /// The sizes at the app's zoom now.
    pub fn now(cx: &App) -> Self {
        Self::at(crate::shell::theme::zoom(cx))
    }
}
/// The strip shows at most this many days, the newest kept.
const MAX_DAYS: usize = 730;

pub fn bind_keys(cx: &mut App) {
    let ctx = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("j", DayDown, ctx),
        KeyBinding::new("down", DayDown, ctx),
        KeyBinding::new("k", DayUp, ctx),
        KeyBinding::new("up", DayUp, ctx),
        KeyBinding::new("enter", DayOpen, ctx),
        KeyBinding::new("escape", DayClose, ctx),
        KeyBinding::new("q", DayClose, ctx),
    ]);
}

/// One day of a project: the tasks created and the tasks completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day {
    pub date: civil::Date,
    pub created: Vec<IssueId>,
    pub completed: Vec<IssueId>,
}

fn local_date(ts: jiff::Timestamp) -> civil::Date {
    ts.to_zoned(jiff::tz::TimeZone::system()).date()
}

/// Every day from the project's first task to `today`, oldest first.
/// Trashed tasks count nowhere. A task counts as completed on the day it
/// closed with a completed status, so canceled work does not count.
pub fn days<'a>(
    issues: impl IntoIterator<Item = &'a Issue>,
    slug: &str,
    today: civil::Date,
) -> Vec<Day> {
    let mut created: Vec<(civil::Date, IssueId)> = Vec::new();
    let mut completed: Vec<(civil::Date, IssueId)> = Vec::new();
    for issue in issues {
        if issue.deleted || issue.project.as_deref() != Some(slug) {
            continue;
        }
        let added = issue.added;
        if let Ok(date) = civil::Date::new(added.year, added.month, added.day) {
            created.push((date, issue.id));
        }
        if issue.status.is_done()
            && let Some(at) = issue.closed_at
        {
            completed.push((local_date(at), issue.id));
        }
    }
    let Some(first) = created.iter().chain(&completed).map(|(d, _)| *d).min() else {
        return Vec::new();
    };
    let span = (today - first).get_days().max(0) as usize + 1;
    let start = if span > MAX_DAYS {
        today
            .checked_sub(jiff::Span::new().days(MAX_DAYS as i64 - 1))
            .unwrap_or(first)
    } else {
        first
    };
    let count = ((today - start).get_days().max(0) as usize + 1).min(MAX_DAYS);
    let mut out: Vec<Day> = (0..count)
        .filter_map(|n| start.checked_add(jiff::Span::new().days(n as i64)).ok())
        .map(|date| Day {
            date,
            created: Vec::new(),
            completed: Vec::new(),
        })
        .collect();
    let len = out.len();
    let slot = |date: civil::Date| -> Option<usize> {
        let n = (date - start).get_days();
        (n >= 0 && (n as usize) < len).then_some(n as usize)
    };
    for (date, id) in created {
        if let Some(ix) = slot(date) {
            out[ix].created.push(id);
        }
    }
    for (date, id) in completed {
        if let Some(ix) = slot(date) {
            out[ix].completed.push(id);
        }
    }
    out
}

/// `Tue Nov 24`, the day as the tooltip and the list title show it.
pub fn day_label(date: civil::Date) -> String {
    date.strftime("%a %b %-d").to_string()
}

pub enum DayChartEvent {
    /// The owner clicked this day, at this point in the window.
    Pick(civil::Date, gpui_kit::Point<gpui_kit::Pixels>),
    /// The pointer moved onto a day with tasks, or off every such day.
    Hover(Option<(civil::Date, gpui_kit::Point<gpui_kit::Pixels>)>),
}

/// The project slug the list shows, or `None` in any other scope.
pub fn project_of(list: &Entity<IssueList>, cx: &App) -> Option<SharedString> {
    match &list.read(cx).scope {
        Scope::Project(slug) => Some(slug.clone()),
        _ => None,
    }
}

/// The strip. It draws from the store, for the project the list shows.
pub struct DayChart {
    store: Entity<IssueStore>,
    list: Entity<IssueList>,
    /// The project at the last draw, so a list change that keeps the
    /// project does not draw the strip again.
    shown: Option<SharedString>,
    scroll: ScrollHandle,
    /// The day count at the last draw. A new day scrolls the strip to its
    /// newest end, as a new project does.
    drawn: Option<(SharedString, usize)>,
    /// The day under the pointer, by index. The readout shows it.
    hovered: Option<usize>,
    /// The strip's bounds at the last paint. The day list opens on its top
    /// edge.
    bounds: std::rc::Rc<std::cell::Cell<gpui_kit::Bounds<gpui_kit::Pixels>>>,
}

impl EventEmitter<DayChartEvent> for DayChart {}

impl DayChart {
    pub fn new(store: Entity<IssueStore>, list: Entity<IssueList>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        cx.observe(&list, |this: &mut Self, list, cx| {
            if project_of(&list, cx) != this.shown {
                cx.notify();
            }
        })
        .detach();
        Self {
            store,
            list,
            shown: None,
            scroll: ScrollHandle::new(),
            drawn: None,
            hovered: None,
            bounds: Default::default(),
        }
    }

    pub fn slug(&self, cx: &App) -> Option<SharedString> {
        project_of(&self.list, cx)
    }

    pub fn days(&self, cx: &App) -> Vec<Day> {
        let Some(slug) = self.slug(cx) else {
            return Vec::new();
        };
        let today = local_date(jiff::Timestamp::now());
        days(self.store.read(cx).iter(), &slug, today)
    }
}

/// How tall a bar of `n` tasks is against `max`, in `half` px of room,
/// in whole px so its ends sit on the pixel grid.
fn bar_height(n: usize, max: usize, m: Metrics) -> f32 {
    if n == 0 {
        0.
    } else {
        (n as f32 / max.max(1) as f32 * m.half)
            .round()
            .max((3. * m.zoom).round())
    }
}

/// Where the dots go along one side of a dotted bar, in device pixels:
/// the offset of the first dot and the dot count. `room` is the space
/// inside the outline, `dot` one dot's size, and the dots sit `2 * dot`
/// apart. They keep at least one dot of gap from the outline, and the
/// leftover space splits as evenly as the pixel grid allows.
pub fn dot_run(room: i32, dot: i32) -> (i32, i32) {
    let pitch = dot * 2;
    if room < dot * 3 {
        return (0, 0);
    }
    let count = (room - dot * 3) / pitch + 1;
    let span = (count - 1) * pitch + dot;
    ((room - span) / 2, count)
}

/// Created tasks: a 1 px outline with a grid of dots inside. It paints in
/// device pixels, so the outline and every dot land on whole pixels at any
/// scale. The rows count up from the bottom, so the dots of bars side by
/// side line up.
fn dotted_bar(height: f32, color: Hsla, m: Metrics) -> impl IntoElement {
    let zoom = m.zoom;
    gpui_kit::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let scale = window.scale_factor();
            let to_dev = |v: gpui_kit::Pixels| (f32::from(v) * scale).round() as i32;
            let to_px = |v: i32| px(v as f32 / scale);
            let (left, top) = (to_dev(bounds.left()), to_dev(bounds.top()));
            let (w, h) = (to_dev(bounds.size.width), to_dev(bounds.size.height));
            // 1 px at 100%, thicker as the app zooms in.
            let line = (scale * zoom).round().max(1.) as i32;
            let rect = |x: i32, y: i32, w: i32, h: i32| {
                gpui_kit::Bounds::new(
                    point(to_px(left + x), to_px(top + y)),
                    gpui_kit::size(to_px(w), to_px(h)),
                )
            };
            for edge in [
                rect(0, 0, w, line),
                rect(0, h - line, w, line),
                rect(0, line, line, h - 2 * line),
                rect(w - line, line, line, h - 2 * line),
            ] {
                window.paint_quad(gpui_kit::fill(edge, color));
            }
            let dot = line;
            let (x0, cols) = dot_run(w - 2 * line, dot);
            let (x_gap, rows) = dot_run(h - 2 * line, dot);
            // The bottom row keeps the same gap as the sides.
            let bottom_gap = x0.min(x_gap);
            let dots = color.opacity(0.75);
            for row in 0..rows {
                let y = h - line - bottom_gap - dot - row * dot * 2;
                if y < line + dot {
                    break;
                }
                for col in 0..cols {
                    let x = line + x0 + col * dot * 2;
                    window.paint_quad(gpui_kit::fill(rect(x, y, dot, dot), dots));
                }
            }
        },
    )
    .flex_none()
    .w(px(m.bar))
    .h(px(height))
}

/// Completed tasks: a solid bar.
fn solid_bar(height: f32, color: Hsla, m: Metrics) -> impl IntoElement {
    div().flex_none().w(px(m.bar)).h(px(height)).bg(color)
}

/// The icon for created or completed tasks, in the readout and the day
/// list.
fn kind_icon(created: bool) -> gpui_kit::component::Icon {
    use gpui_kit::component::Sizable as _;
    gpui_kit::component::Icon::new(if created {
        gpui_kit::assets::IconName::CirclePlus
    } else {
        gpui_kit::assets::IconName::CircleCheck
    })
    .xsmall()
}

/// The legend's sample of a bar, beside a count.
fn swatch(dotted: bool, color: Hsla, m: Metrics) -> gpui_kit::AnyElement {
    let h = (11. * m.zoom).round();
    if dotted {
        dotted_bar(h, color.opacity(CREATED_ALPHA), m).into_any_element()
    } else {
        solid_bar(h, color, m).into_any_element()
    }
}

/// What the readout shows: the hovered day, or the last seven days when
/// no day is hovered.
pub fn readout(days: &[Day], hovered: Option<usize>) -> (String, usize, usize) {
    match hovered.and_then(|ix| days.get(ix)) {
        Some(day) => (day_label(day.date), day.created.len(), day.completed.len()),
        None => {
            let week = &days[days.len().saturating_sub(7)..];
            (
                "Last 7 days".to_string(),
                week.iter().map(|d| d.created.len()).sum(),
                week.iter().map(|d| d.completed.len()).sum(),
            )
        }
    }
}

impl Render for DayChart {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let days = self.days(cx);
        self.shown = self.slug(cx);
        let Some(slug) = self.shown.clone() else {
            return div().into_any_element();
        };
        if self.drawn.as_ref() != Some(&(slug.clone(), days.len())) {
            self.drawn = Some((slug, days.len()));
            self.hovered = None;
            // The offset clamps at layout, so this lands on the newest day.
            self.scroll.set_offset(point(px(-1e6), px(0.)));
        }
        let theme = cx.theme();
        let color = theme.primary;
        // Created bars step back, so completed work reads first.
        let created_color = color.opacity(CREATED_ALPHA);
        let hover_bg = theme.foreground.opacity(0.14);
        let axis = theme.foreground.opacity(0.28);
        let muted = theme.muted_foreground;
        let hovered = self.hovered;
        let max = days
            .iter()
            .map(|d| d.created.len().max(d.completed.len()))
            .max()
            .unwrap_or(0);
        let m = Metrics::now(cx);
        let half = m.half;
        let (label, made, done) = readout(&days, hovered);
        let slots = days.iter().enumerate().map(|(ix, day)| {
            let date = day.date;
            let empty = day.created.is_empty() && day.completed.is_empty();
            let is_hovered = hovered == Some(ix);
            // While one day is hovered, the others step back.
            let dim = hovered.is_some() && !is_hovered;
            // Each week starts on a Monday, with its date under it.
            let week_start = date.weekday() == jiff::civil::Weekday::Monday;
            v_flex()
                .id(SharedString::from(format!("day-{date}")))
                .test_support()
                .relative()
                .flex_none()
                .w(px(m.slot))
                .h_full()
                .when(dim, |this| this.opacity(0.4))
                .on_hover(cx.listener(move |this, over: &bool, window, cx| {
                    let next = if *over {
                        Some(ix)
                    } else if this.hovered == Some(ix) {
                        None
                    } else {
                        this.hovered
                    };
                    if next != this.hovered {
                        this.hovered = next;
                        cx.notify();
                        // A day with tasks shows its list above the
                        // pointer; anywhere else lets it go.
                        let at = point(window.mouse_position().x, this.bounds.get().top() - px(1.));
                        cx.emit(DayChartEvent::Hover(
                            (next == Some(ix) && !empty).then_some((date, at)),
                        ));
                    }
                }))
                .when(!empty, |this| {
                    this.cursor_pointer().on_click(cx.listener(
                        move |this, event: &gpui_kit::ClickEvent, _, cx| {
                            // Centered on the pointer, on the strip's top edge.
                            // The bounds sit inside the 1 px top border.
                            let top = this.bounds.get().top() - px(1.);
                            let at = point(event.position().x, top);
                            cx.emit(DayChartEvent::Pick(date, at));
                        },
                    ))
                })
                .child(
                    v_flex()
                        .w_full()
                        .pt(px(m.pad_top))
                        .when(is_hovered, |this| this.bg(hover_bg))
                        .child(v_flex().h(px(half)).pl(px(m.inset)).justify_end().when(
                            !day.created.is_empty(),
                            |this| {
                                this.child(dotted_bar(
                                    bar_height(day.created.len(), max, m),
                                    created_color,
                                    m,
                                ))
                            },
                        ))
                        .child(div().w_full().h(px(1.)).bg(axis))
                        .child(v_flex().h(px(half)).pl(px(m.inset)).when(
                            !day.completed.is_empty(),
                            |this| {
                                this.child(solid_bar(
                                    bar_height(day.completed.len(), max, m),
                                    color,
                                    m,
                                ))
                            },
                        )),
                )
                .when(week_start, |this| {
                    this.child(
                        div()
                            .absolute()
                            .left_0()
                            .bottom(px(m.pad_bottom))
                            .h(px(m.label_row))
                            .flex()
                            .items_end()
                            .gap_1()
                            .child(div().w(px(1.)).h_full().bg(axis))
                            .child(
                                div()
                                    .text_size(px(m.label_font))
                                    .line_height(px(m.label_font))
                                    .whitespace_nowrap()
                                    .text_color(muted)
                                    .child(date.strftime("%b %-d").to_string()),
                            ),
                    )
                })
        });
        let count = |n: usize, word: &'static str, created: bool| {
            h_flex()
                .id(word)
                .test_support()
                .gap_1p5()
                .items_center()
                .child(swatch(created, color, m))
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.foreground)
                        .child(n.to_string()),
                )
                .child(kind_icon(created).text_color(muted))
                .child(div().text_color(muted).child(word))
        };
        let today = days.last().map(|d| d.date.strftime("%b %-d").to_string());
        let bounds = self.bounds.clone();
        h_flex()
            .id("day-chart")
            .test_support()
            .relative()
            .size_full()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                gpui_kit::canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                v_flex()
                    .id("day-chart-readout")
                    .test_support()
                    .flex_none()
                    // A fixed width, so a hover never moves the strip
                    // beside it.
                    .w(gpui_kit::rems(10.5))
                    .h_full()
                    .px_4()
                    .justify_center()
                    .gap_1p5()
                    .text_xs()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .border_r_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_color(theme.foreground)
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(label),
                    )
                    .child(count(made, "created", true))
                    .child(count(done, "completed", false)),
            )
            .child(
                h_flex()
                    .id("day-chart-days")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px_2()
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll)
                    .children(slots)
                    // Today's date caps the line on the right.
                    .children(today.map(|today| {
                        div()
                            .id("day-chart-today")
                            .test_support()
                            .flex_none()
                            .h_full()
                            .pl_1p5()
                            .pt(px(m.pad_top + half - m.label_font / 2.))
                            .text_size(px(m.label_font))
                            .line_height(px(m.label_font))
                            .whitespace_nowrap()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(today)
                    })),
            )
            .into_any_element()
    }
}

pub enum DayTasksEvent {
    /// Open this task. The list is already closed.
    Open(IssueId),
}

/// One task of the picked day, as the list shows it.
#[derive(Debug, Clone)]
pub struct DayRow {
    pub id: IssueId,
    pub key: SharedString,
    pub title: SharedString,
    pub completed: bool,
}

/// The picked day's tasks over the window: created first, then completed.
pub struct DayTasks {
    store: Entity<IssueStore>,
    focus: FocusHandle,
    open: Option<(SharedString, civil::Date)>,
    /// Where the owner clicked. The list opens above it, centered on it.
    at: gpui_kit::Point<gpui_kit::Pixels>,
    selected: usize,
    return_to: Option<FocusHandle>,
    fallback: FocusHandle,
    /// Opened by a click: it holds the keyboard and stays until Esc or a
    /// click outside. A list opened by hover follows the pointer instead.
    pinned: bool,
    /// The pointer is over the list, so a hover list stays open.
    inside: bool,
    /// Closes a hover list a moment after the pointer leaves, so the
    /// pointer can cross from the strip onto the list.
    closing: Option<gpui_kit::Task<()>>,
    /// The content's height at the last paint, or 0 before the first. The
    /// card glides to it when the day changes.
    measured: std::rc::Rc<std::cell::Cell<gpui_kit::Pixels>>,
    /// The day the list opened on. Later days fade in once the card fits.
    first_day: Option<civil::Date>,
}

/// How long a hover list waits after the pointer leaves before it closes.
const HOVER_CLOSE_AFTER: std::time::Duration = std::time::Duration::from_millis(250);

impl EventEmitter<DayTasksEvent> for DayTasks {}

impl DayTasks {
    pub fn new(store: Entity<IssueStore>, fallback: FocusHandle, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self {
            store,
            focus: cx.focus_handle(),
            open: None,
            at: point(px(0.), px(0.)),
            selected: 0,
            return_to: None,
            fallback,
            pinned: false,
            inside: false,
            closing: None,
            measured: Default::default(),
            first_day: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn rows(&self, cx: &App) -> Vec<DayRow> {
        let Some((slug, date)) = &self.open else {
            return Vec::new();
        };
        let store = self.store.read(cx);
        let Some(day) = days(store.iter(), slug, *date)
            .pop()
            .filter(|d| d.date == *date)
        else {
            return Vec::new();
        };
        let row = |id: &IssueId, completed: bool| {
            store.get(*id).map(|i| DayRow {
                id: i.id,
                key: i.key.clone(),
                title: i.title.clone(),
                completed,
            })
        };
        day.created
            .iter()
            .filter_map(|id| row(id, false))
            .chain(day.completed.iter().filter_map(|id| row(id, true)))
            .collect()
    }

    pub fn show(
        &mut self,
        slug: SharedString,
        date: civil::Date,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.pinned {
            self.return_to = window.focused(cx);
        }
        self.pinned = true;
        self.closing = None;
        if self.open.is_none() {
            self.fresh(date);
        }
        self.open = Some((slug, date));
        self.at = at;
        self.selected = 0;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The pointer is on a day with tasks: show them, without taking the
    /// keyboard. A list opened by a click stays as it is.
    pub fn peek(
        &mut self,
        slug: SharedString,
        date: civil::Date,
        at: gpui_kit::Point<gpui_kit::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.pinned {
            return;
        }
        self.closing = None;
        if self.open.as_ref().map(|(_, d)| *d) != Some(date) {
            self.selected = 0;
        }
        if self.open.is_none() {
            self.fresh(date);
        }
        self.open = Some((slug, date));
        self.at = at;
        cx.notify();
    }

    /// The pointer left the days with tasks: a hover list closes soon,
    /// unless the pointer reaches it first.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        if self.pinned || self.open.is_none() || self.inside {
            return;
        }
        self.closing = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_CLOSE_AFTER).await;
            this.update(cx, |this, cx| {
                if !this.pinned && !this.inside {
                    this.open = None;
                    this.first_day = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// A list that was closed opens: its card takes its first height at
    /// once, with no glide.
    fn fresh(&mut self, date: civil::Date) {
        self.measured.set(px(0.));
        self.first_day = Some(date);
    }

    /// Opened by a click, so it holds the keyboard.
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.closing = None;
        self.inside = false;
        self.first_day = None;
        if self.open.take().is_none() {
            return;
        }
        if !std::mem::take(&mut self.pinned) {
            // A hover list never took the keyboard, so it gives none back.
            cx.notify();
            return;
        }
        let handle = self.return_to.take().unwrap_or(self.fallback.clone());
        window.focus(&handle, cx);
        cx.notify();
    }

    fn step(&mut self, down: bool, cx: &mut Context<Self>) {
        let count = self.rows(cx).len();
        self.selected = if down {
            (self.selected + 1).min(count.saturating_sub(1))
        } else {
            self.selected.saturating_sub(1)
        };
        cx.notify();
    }

    fn open_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows(cx).into_iter().nth(ix) else {
            return;
        };
        self.close(window, cx);
        cx.emit(DayTasksEvent::Open(row.id));
    }
}

impl Render for DayTasks {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((_, date)) = self.open.clone() else {
            return div().into_any_element();
        };
        let rows = self.rows(cx);
        self.selected = self.selected.min(rows.len().saturating_sub(1));
        let m = Metrics::now(cx);
        let theme = cx.theme();
        let hover = theme.list_hover;
        let pressed = theme.foreground.opacity(0.12);
        let active_bg = theme.list_active;
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let color = theme.primary;
        let made = rows.iter().filter(|r| !r.completed).count();
        let title = format!(
            "{} · {made} created · {} completed",
            day_label(date),
            rows.len() - made
        );
        let body = if rows.is_empty() {
            div()
                .px_2()
                .py_3()
                .text_sm()
                .text_color(muted)
                .child("No task was created or completed that day.")
                .into_any_element()
        } else {
            // A line between the created tasks and the completed ones,
            // when the day has both.
            let split = rows.iter().position(|r| r.completed).filter(|&ix| ix > 0);
            let line = theme.border;
            v_flex()
                .gap_px()
                .children(rows.into_iter().enumerate().flat_map(|(ix, row)| {
                    let selected = ix == self.selected;
                    let divider = (split == Some(ix)).then(|| {
                        div()
                            .id("day-tasks-divider")
                            .test_support()
                            .mx_2()
                            .my_1()
                            .h(px(1.))
                            .bg(line)
                            .into_any_element()
                    });
                    let word = if row.completed {
                        "Completed"
                    } else {
                        "Created"
                    };
                    let item = h_flex()
                        .id(SharedString::from(format!("day-task-{}-{ix}", row.key)))
                        .test_support()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .text_sm()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .when(selected, |this| this.bg(active_bg))
                        .when(!selected, |this| this.hover(move |s| s.bg(hover)))
                        .active(move |s| s.bg(pressed))
                        .child(swatch(!row.completed, color, m))
                        .child(
                            div()
                                .flex_none()
                                .font_family(mono.clone())
                                .child(row.key.clone()),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(row.title))
                        .child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .text_xs()
                                .text_color(muted)
                                .child(kind_icon(!row.completed))
                                .child(word),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_row(ix, window, cx);
                        }));
                    divider
                        .into_iter()
                        .chain(std::iter::once(item.into_any_element()))
                }))
                .into_any_element()
        };
        let content = v_flex()
            .p_1()
            .gap_1()
            .child(
                h_flex()
                    .px_2()
                    .pt_1()
                    .justify_between()
                    .text_xs()
                    .text_color(muted)
                    .child(title)
                    .child("Enter opens · Esc closes"),
            )
            .child(body);
        // In rems, so it grows with zoom.
        let content = div()
            .id("day-tasks-content")
            .max_h(gpui_kit::rems(26.))
            .overflow_y_scroll()
            .child(content);
        // Measures the content each paint. A new height draws again, so
        // the card glides to it.
        let measured = self.measured.clone();
        let measure = gpui_kit::canvas(
            move |bounds, window, _| {
                if measured.get() != bounds.size.height {
                    measured.set(bounds.size.height);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let known = self.measured.get() > px(0.);
        // A later day's content waits for the card to fit it, then fades in.
        let content = div().relative().child(content).child(measure);
        let content = if self.first_day != Some(date) {
            crate::shell::motion::fade_in_after_glide(
                content,
                SharedString::from(format!("day-tasks-fade-{date}")),
                cx,
            )
        } else {
            content.into_any_element()
        };
        let border = px(1.);
        let height = known.then(|| {
            let target = f32::from(self.measured.get() + border * 2.);
            px(crate::shell::motion::glide(
                "day-tasks-height",
                target,
                window,
                cx,
            ))
        });
        let theme = cx.theme();
        let panel = div().child(
            v_flex()
                .id("day-tasks")
                .test_support()
                .key_context(CONTEXT)
                .track_focus(&self.focus)
                .on_action(cx.listener(|this, _: &DayDown, _, cx| this.step(true, cx)))
                .on_action(cx.listener(|this, _: &DayUp, _, cx| this.step(false, cx)))
                .on_action(cx.listener(|this, _: &DayOpen, window, cx| {
                    let ix = this.selected;
                    this.open_row(ix, window, cx);
                }))
                .on_action(cx.listener(|this, _: &DayClose, window, cx| this.close(window, cx)))
                // The list under it takes no hover or click.
                .occlude()
                .on_hover(cx.listener(|this, over: &bool, _, cx| {
                    this.inside = *over;
                    if *over {
                        this.closing = None;
                    } else {
                        this.leave(cx);
                    }
                }))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close(window, cx)))
                .w(gpui_kit::rems(30.))
                .relative()
                .overflow_hidden()
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .border(border)
                .border_color(theme.border)
                .rounded(theme.radius)
                .shadow_lg()
                .map(|this| match height {
                    // The card's height glides; the content sits on top of
                    // it at its own height, clipped while the card grows.
                    Some(h) => this
                        .h(h)
                        .child(div().absolute().top_0().left_0().right_0().child(content)),
                    // Its first paint: the content's own height.
                    None => this.child(content),
                }),
        );
        // On the strip's top edge, over the pointer, and kept 8 px inside
        // the window. It glides along as the pointer moves between days.
        let x = crate::shell::motion::glide("day-tasks-x", f32::from(self.at.x), window, cx);
        let at = point(px(x), self.at.y);
        gpui_kit::deferred(
            gpui_kit::anchored()
                .position(at)
                .anchor(gpui_kit::Anchor::BottomCenter)
                .snap_to_window_with_margin(px(8.))
                .child(crate::shell::motion::rise(panel, "day-tasks-enter", cx)),
        )
        .with_priority(1)
        .into_any_element()
    }
}
