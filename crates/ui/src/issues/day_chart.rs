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

/// The strip's height, bars and padding together.
pub const HEIGHT: f32 = 72.;
/// One day's slot, and the bar inside it.
const SLOT: f32 = 10.;
const BAR: f32 = 8.;
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
    /// The owner clicked this day.
    Pick(civil::Date),
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

/// How tall a bar of `n` tasks is against `max`, in `half` px of room.
fn bar_height(n: usize, max: usize, half: f32) -> f32 {
    if n == 0 {
        0.
    } else {
        (n as f32 / max.max(1) as f32 * half).max(3.)
    }
}

/// Created tasks: an outline with a grid of dots inside.
fn dotted_bar(height: f32, color: Hsla) -> impl IntoElement {
    div()
        .w(px(BAR))
        .h(px(height))
        .border_1()
        .border_color(color)
        .rounded(px(1.5))
        .overflow_hidden()
        .child(
            gpui_kit::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let dot = gpui_kit::size(px(1.), px(1.));
                    let dots = color.opacity(0.75);
                    let mut y = bounds.top() + px(1.);
                    while y + px(1.) <= bounds.bottom() - px(0.5) {
                        let mut x = bounds.left() + px(1.);
                        while x + px(1.) <= bounds.right() - px(0.5) {
                            window.paint_quad(gpui_kit::fill(
                                gpui_kit::Bounds::new(point(x, y), dot),
                                dots,
                            ));
                            x += px(2.);
                        }
                        y += px(2.);
                    }
                },
            )
            .size_full(),
        )
}

/// Completed tasks: a solid bar.
fn solid_bar(height: f32, color: Hsla) -> impl IntoElement {
    div().w(px(BAR)).h(px(height)).bg(color).rounded(px(1.5))
}

/// The legend's sample of a bar, beside a count.
fn swatch(dotted: bool, color: Hsla) -> gpui_kit::AnyElement {
    if dotted {
        dotted_bar(10., color).into_any_element()
    } else {
        solid_bar(10., color).into_any_element()
    }
}

/// What the readout at the right shows: the hovered day, or the last
/// seven days when no day is hovered.
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
        let hover_bg = theme.foreground.opacity(0.14);
        let axis = theme.foreground.opacity(0.28);
        let muted = theme.muted_foreground;
        let hovered = self.hovered;
        let max = days
            .iter()
            .map(|d| d.created.len().max(d.completed.len()))
            .max()
            .unwrap_or(0);
        // The padding takes 8 px above and below, the line 1 px.
        let half = (HEIGHT - 17.) / 2.;
        let (label, made, done) = readout(&days, hovered);
        let slots = days.iter().enumerate().map(|(ix, day)| {
            let date = day.date;
            let empty = day.created.is_empty() && day.completed.is_empty();
            let is_hovered = hovered == Some(ix);
            // While one day is hovered, the others step back.
            let dim = hovered.is_some() && !is_hovered;
            v_flex()
                .id(SharedString::from(format!("day-{date}")))
                .test_support()
                .flex_none()
                .w(px(SLOT))
                .h_full()
                .items_center()
                .rounded(px(2.))
                .when(dim, |this| this.opacity(0.4))
                .when(is_hovered, |this| this.bg(hover_bg))
                .on_hover(cx.listener(move |this, over: &bool, _, cx| {
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
                    }
                }))
                .when(!empty, |this| {
                    this.cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(DayChartEvent::Pick(date));
                        }))
                })
                .child(
                    v_flex()
                        .h(px(half))
                        .justify_end()
                        .when(!day.created.is_empty(), |this| {
                            this.child(dotted_bar(bar_height(day.created.len(), max, half), color))
                        }),
                )
                .child(div().w_full().h(px(1.)).bg(axis))
                .child(
                    v_flex()
                        .h(px(half))
                        .when(!day.completed.is_empty(), |this| {
                            this.child(solid_bar(bar_height(day.completed.len(), max, half), color))
                        }),
                )
        });
        let count = |n: usize, word: &'static str, dotted: bool| {
            h_flex()
                .gap_1p5()
                .items_center()
                .child(swatch(dotted, color))
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.foreground)
                        .child(n.to_string()),
                )
                .child(div().text_color(muted).child(word))
        };
        h_flex()
            .id("day-chart")
            .test_support()
            .size_full()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .id("day-chart-days")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .py(px(8.))
                    .px_2()
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll)
                    .children(slots),
            )
            .child(
                v_flex()
                    .id("day-chart-readout")
                    .test_support()
                    .flex_none()
                    // Wide enough for any day, so a hover never moves it.
                    .min_w(gpui_kit::rems(13.))
                    .h_full()
                    .px_4()
                    .justify_center()
                    .gap_1p5()
                    .text_xs()
                    .border_l_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_color(theme.foreground)
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(label),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(count(made, "created", true))
                            .child(count(done, "completed", false)),
                    ),
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
    selected: usize,
    return_to: Option<FocusHandle>,
    fallback: FocusHandle,
}

impl EventEmitter<DayTasksEvent> for DayTasks {}

impl DayTasks {
    pub fn new(store: Entity<IssueStore>, fallback: FocusHandle, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self {
            store,
            focus: cx.focus_handle(),
            open: None,
            selected: 0,
            return_to: None,
            fallback,
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open.is_none() {
            self.return_to = window.focused(cx);
        }
        self.open = Some((slug, date));
        self.selected = 0;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.take().is_none() {
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((_, date)) = self.open.clone() else {
            return div().into_any_element();
        };
        let rows = self.rows(cx);
        self.selected = self.selected.min(rows.len().saturating_sub(1));
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
            v_flex()
                .gap_px()
                .children(rows.into_iter().enumerate().map(|(ix, row)| {
                    let selected = ix == self.selected;
                    let word = if row.completed {
                        "Completed"
                    } else {
                        "Created"
                    };
                    h_flex()
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
                        .child(swatch(!row.completed, color))
                        .child(
                            div()
                                .flex_none()
                                .font_family(mono.clone())
                                .child(row.key.clone()),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(row.title))
                        .child(div().flex_none().text_xs().text_color(muted).child(word))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_row(ix, window, cx);
                        }))
                }))
                .into_any_element()
        };
        let root = div()
            .absolute()
            .top_10()
            .left_4()
            .right_4()
            .flex()
            .justify_center()
            .child(
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
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close(window, cx)))
                    .w(px(560.))
                    .max_w_full()
                    .max_h(px(480.))
                    .overflow_y_scroll()
                    .p_1()
                    .gap_1()
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .shadow_lg()
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
                    .child(body),
            );
        crate::shell::motion::enter(root, "day-tasks-enter", cx).into_any_element()
    }
}
