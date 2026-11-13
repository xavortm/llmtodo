//! The issue list: the filter, sorting, grouping, the selection and the
//! rows. It reads issues from the shared `IssueStore` and holds only their
//! IDs, so a snapshot that reorders the store cannot shift a row onto
//! another issue.
//!
//! The list changes its state only when the workspace asks. Clicks, drops
//! and menu items emit a `ViewEvent`, never through `cx.listener` on the
//! list. The workspace then updates the list, the
//! store and the task view in one pass, and no entity is borrowed twice.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{self as ed, InputState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder as _, uniform_list,
};

use crate::issues::columns::{self, Column, cell_with};
use crate::issues::filter::Filter;
use crate::issues::rows::{self, BuiltRows, OwnedRowsInput, RowsInput};
use crate::model::data::{Assignee, Issue, IssueId, Label, LlmStatus, Status, hide_cutoff};
use crate::model::scope::Scope;
use crate::model::store::{Connection, IssueStore};
use crate::shell::icons::HasIcon as _;
use crate::shell::tooltip::{Card, with_card};
use crate::shell::view_events::{IssueAction, ViewEvent, ViewEvents};
use crate::shell::widgets::{self, DraggedIssue, label_color};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Key,
    Status,
    Added,
    /// Locked, then Analyzing, then the rest.
    Llm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    None,
    Status,
    Label,
    Assignee,
    /// One group per parent task. Its sub-issues show in their order.
    Parent,
}

impl GroupBy {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" | "off" => Some(Self::None),
            "status" => Some(Self::Status),
            "label" | "labels" => Some(Self::Label),
            "assignee" | "owner" => Some(Self::Assignee),
            "parent" | "epic" => Some(Self::Parent),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Status => "Status",
            Self::Label => "Label",
            Self::Assignee => "Assignee",
            Self::Parent => "Parent",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::None => Self::Status,
            Self::Status => Self::Label,
            Self::Label => Self::Assignee,
            Self::Assignee => Self::Parent,
            Self::Parent => Self::None,
        }
    }
}

/// One line of the issue list: a group header or an issue.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Header {
        /// Names the group. Under "group by parent", the parent's key.
        title: SharedString,
        /// Under "group by parent", the parent's title.
        subtitle: Option<SharedString>,
        count: usize,
        label: Option<Label>,
        /// Set under "group by status", where a drop moves a task here.
        status: Option<Status>,
        collapsed: bool,
    },
    Issue(IssueId),
}

/// How a click on a task row acts, from the keys held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowClick {
    /// Clears the marks, selects the task and opens it.
    Plain,
    /// Shift: makes the range from the anchor to this task the marked range.
    Range,
    /// Cmd or the row's box: marks this task, or clears its mark.
    Toggle,
}

/// The hover group of one task row, for its mark box.
const ROW_GROUP: &str = "issue-row";
/// The hover and press group of a mark box's click target.
const CHECK_GROUP: &str = "issue-row-check";

pub struct IssueList {
    store: Entity<IssueStore>,
    events: ViewEvents,
    /// The workspace's focus, which is the list's focus.
    focus: FocusHandle,
    pub filter: Entity<InputState>,
    /// The filter text, parsed.
    query: Filter,
    /// The team or project the sidebar picked.
    pub scope: Scope,
    /// Hide closed tasks this many days old. None shows them all.
    pub hide_closed_after: Option<u32>,
    /// Experimental: draw a light line through the titles of Done tasks.
    pub strike_done: bool,
    /// How many tasks the scope and filter would show but that setting hides.
    hidden_closed: usize,
    /// The issues that pass the filter, in sort order.
    visible: Vec<IssueId>,
    /// What the list shows: `visible`, split into groups.
    rows: Vec<Row>,
    /// Under "group by parent", the order line of each row.
    order: Vec<Option<rows::OrderMark>>,
    /// Index into `rows`.
    selected: usize,
    pub group: GroupBy,
    /// Sort key, and whether the order is reversed.
    pub sort: (SortKey, bool),
    /// Collapsed groups, as `grouping:title`.
    pub collapsed: HashSet<String>,
    /// Issues marked with `x` for a bulk action.
    pub marked: HashSet<IssueId>,
    /// Where a shift-click range starts: the last task toggled with `x`,
    /// cmd-click or its box. When None, the next range starts at the
    /// selected row. It stays put across shift-clicks, so a later shift-click
    /// can grow or shrink the same range.
    pub anchor: Option<IssueId>,
    /// The marks outside the current range. A shift-click replaces the range
    /// and keeps these.
    range_base: HashSet<IssueId>,
    scroll: UniformListScrollHandle,
    /// The list's own width after the last layout.
    width: Rc<Cell<Option<Pixels>>>,
    /// The columns that fit that width and the labels width, set at each
    /// render.
    /// Shared with every row, so a row does not copy the column list.
    layout: std::rc::Rc<columns::Layout>,
    /// The columns the owner hid from the column heading menu.
    pub hidden_columns: Vec<Column>,
    /// What the titles and labels of `visible` ask of the columns. The
    /// workspace asks on each frame, and the answer changes only when
    /// `visible` is computed again.
    demand: Cell<Option<columns::Demand>>,
    /// From this many issues, builds run on a background task.
    background_from: usize,
    /// The build on a background task, if one is on its way. Dropping it
    /// cancels the build.
    build_task: Option<Task<()>>,
    /// The selection rule for the build on its way.
    pending_keep: Option<Keep>,
    /// Builds the rows again when the next task gets old enough to hide.
    hide_timer: Option<Task<()>>,
    /// Set when the rows changed while a background build was on its way.
    /// Its result is then thrown away, and one more build starts.
    stale: bool,
    /// How many background builds started, for tests.
    #[cfg(test)]
    pub(crate) background_builds: usize,
}

/// Where the selection goes once new rows land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keep {
    /// On the first issue row, as after a filter keystroke.
    First,
    /// On this issue, or on the first issue row when it left the list.
    Issue(Option<IssueId>),
    /// On the issue or header selected when the rows land. With `scroll`
    /// off, the list keeps its scroll, and a selected issue that left the
    /// list passes the selection to the row that moves into its place.
    Current { scroll: bool },
}

impl Keep {
    /// The rule for one build that replaces a cancelled build with rule
    /// `self` and its own rule `next`. A rule that names a place wins over
    /// keeping the current row, so a live update during a filter keystroke
    /// still selects the first match.
    fn then(self, next: Keep) -> Keep {
        match (self, next) {
            (Keep::Current { scroll: a }, Keep::Current { scroll: b }) => {
                Keep::Current { scroll: a || b }
            }
            (placed, Keep::Current { .. }) => placed,
            (_, placed) => placed,
        }
    }
}

impl IssueList {
    pub fn new(
        store: Entity<IssueStore>,
        events: ViewEvents,
        focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter: text, status:todo, label:bug, assignee:me")
        });
        // The list is cached, and its rows draw each issue from the store.
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self {
            store,
            events,
            focus,
            filter,
            query: Filter::default(),
            scope: Scope::All,
            hide_closed_after: None,
            strike_done: true,
            hidden_closed: 0,
            visible: Vec::new(),
            rows: Vec::new(),
            order: Vec::new(),
            selected: 0,
            group: GroupBy::None,
            sort: (SortKey::Key, false),
            collapsed: HashSet::new(),
            marked: HashSet::new(),
            range_base: HashSet::new(),
            anchor: None,
            scroll: UniformListScrollHandle::new(),
            width: Rc::default(),
            layout: std::rc::Rc::new(columns::plan(
                None,
                gpui_kit::px(16.),
                &[],
                &columns::Demand::default(),
            )),
            hidden_columns: Vec::new(),
            demand: Cell::new(None),
            background_from: rows::BACKGROUND_FROM,
            build_task: None,
            pending_keep: None,
            hide_timer: None,
            stale: false,
            #[cfg(test)]
            background_builds: 0,
        }
    }

    /// How far the list is scrolled. It is zero or less: below zero means
    /// scrolled down.
    pub fn scroll_offset(&self) -> Pixels {
        self.scroll.0.borrow().base_handle.offset().y
    }

    /// The scroll handle of the list. The benchmarks scroll through it.
    pub fn scroll_handle(&self) -> &UniformListScrollHandle {
        &self.scroll
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn visible(&self) -> &[IssueId] {
        &self.visible
    }

    /// The issue on the selected row, if it is an issue row.
    pub fn selected_issue(&self) -> Option<IssueId> {
        match self.rows.get(self.selected) {
            Some(Row::Issue(id)) => Some(*id),
            _ => None,
        }
    }

    pub fn row_of(&self, id: IssueId) -> Option<usize> {
        self.rows.iter().position(|r| *r == Row::Issue(id))
    }

    /// Filters, sorts and groups the store's issues again, and puts the
    /// selection on `keep`, or on the first issue.
    pub fn reload(&mut self, keep: Option<IssueId>, cx: &mut Context<Self>) {
        self.recompute(Keep::Issue(keep), cx);
    }

    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        self.query = Filter::parse(query);
        // The first match is always selected, so Enter picks it.
        self.recompute(Keep::First, cx);
    }

    /// Builds the rows again and then places the selection by `keep`. With
    /// fewer than `background_from` issues the build runs here, at once.
    /// With more, it runs on a background task on a snapshot of the store,
    /// and the list shows the old rows until the result lands.
    ///
    /// One background build runs at a time. A call while one is on its way
    /// marks the rows stale. The build's result is then thrown away, and one
    /// more build starts on a new snapshot, however many calls came. The
    /// selection rule of the skipped calls carries over when it is the
    /// stronger one: see `Keep::then`. The selection is read when the
    /// result lands, by stable ID, so a move made meanwhile counts.
    fn recompute(&mut self, keep: Keep, cx: &mut Context<Self>) {
        let keep = match self.pending_keep.take() {
            Some(pending) => pending.then(keep),
            None => keep,
        };
        let store = self.store.read(cx);
        if self.build_task.is_some() && store.len() >= self.background_from {
            self.pending_keep = Some(keep);
            self.stale = true;
            return;
        }
        self.stale = false;
        let now = jiff::Timestamp::now();
        let cutoff = self.hide_closed_after.map(|days| hide_cutoff(days, now));
        let query = self.query.resolve(&store.catalog);
        let statuses = store.catalog.all_statuses();
        if store.len() < self.background_from {
            self.build_task = None;
            let input = RowsInput {
                scope: &self.scope,
                view: store.view.as_ref(),
                query: &query,
                cutoff,
                now,
                sort: self.sort,
                group: self.group,
                collapsed: &self.collapsed,
                statuses: &statuses,
                labels: &store.catalog.labels,
            };
            let built = rows::build(&input, store.iter());
            self.apply(built, keep, cx);
            return;
        }
        let input = OwnedRowsInput {
            scope: self.scope.clone(),
            view: store.view.clone(),
            query,
            cutoff,
            now,
            sort: self.sort,
            group: self.group,
            collapsed: self.collapsed.clone(),
            statuses,
            labels: store.catalog.labels.clone(),
        };
        let issues = store.snapshot();
        self.pending_keep = Some(keep);
        #[cfg(test)]
        {
            self.background_builds += 1;
        }
        self.build_task = Some(cx.spawn(async move |this, cx| {
            let built = cx
                .background_spawn(async move {
                    rows::build(&input.borrow(), issues.iter().map(|issue| &**issue))
                })
                .await;
            this.update(cx, |list, cx| {
                list.build_task = None;
                let keep = list
                    .pending_keep
                    .take()
                    .unwrap_or(Keep::Current { scroll: false });
                if list.stale {
                    list.recompute(keep, cx);
                } else {
                    list.apply(built, keep, cx);
                }
            })
            .ok();
        }));
    }

    /// Builds the rows again when the next shown task gets old enough for
    /// "Hide old closed tasks", so it hides without any write. A new build
    /// replaces the timer.
    fn schedule_hide(&mut self, at: Option<jiff::Timestamp>, cx: &mut Context<Self>) {
        self.hide_timer = at.map(|at| {
            let wait: std::time::Duration = jiff::Timestamp::now()
                .duration_until(at)
                .try_into()
                .unwrap_or_default();
            // A little late, so the task is past the cutoff when it runs.
            let wait = wait + std::time::Duration::from_secs(1);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                this.update(cx, |list, cx| list.refilter(cx)).ok();
            })
        });
    }

    /// Puts in a finished build and places the selection.
    fn apply(&mut self, built: BuiltRows, keep: Keep, cx: &mut Context<Self>) {
        let held = self.rows.get(self.selected).cloned();
        let old_row = self.selected;
        self.visible = built.visible;
        self.hidden_closed = built.hidden_closed;
        self.schedule_hide(built.next_hide, cx);
        self.rows = built.rows;
        self.order = built.order;
        self.demand.set(None);
        let kept_row = |list: &Self, cx: &App| match &held {
            Some(Row::Issue(id)) => list.row_of(*id).or_else(|| list.header_of_issue(*id, cx)),
            Some(Row::Header { title, .. }) => list.header_row(title),
            None => None,
        };
        match keep {
            Keep::First => self.select_first(cx),
            Keep::Issue(id) => match id.and_then(|id| self.row_of(id)) {
                Some(row) => self.select(row, cx),
                None => self.select_first(cx),
            },
            // The list keeps its scroll, so a status change that moves the
            // issue to another group does not scroll the list after it.
            Keep::Current { scroll: false } => {
                let row = kept_row(self, cx);
                if !self.rows.is_empty() {
                    self.selected = row.unwrap_or(old_row).min(self.rows.len() - 1);
                }
                cx.notify();
            }
            Keep::Current { scroll: true } => match kept_row(self, cx) {
                Some(row) => self.select(row, cx),
                None => self.select_first(cx),
            },
        }
    }

    /// Whether a build runs on a background task and its result has not
    /// landed yet.
    #[cfg(test)]
    pub fn building(&self) -> bool {
        self.build_task.is_some()
    }

    /// Sets the size from which builds run on a background task.
    #[cfg(test)]
    pub fn set_background_from(&mut self, count: usize) {
        self.background_from = count;
    }

    /// How many tasks the "Hide old closed tasks" setting hides here.
    pub fn hidden_closed(&self) -> usize {
        self.hidden_closed
    }

    /// How many issues pass the filter.
    #[cfg(test)]
    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    /// Whether the rows depend on the LLM status, through the filter or
    /// the sort.
    pub fn reads_llm_status(&self) -> bool {
        self.query.llm.is_some() || self.sort.0 == SortKey::Llm
    }

    /// Whether a change from `old` to `new` can change the rows. Filters,
    /// scopes, sorting and grouping read only the fields compared here, so
    /// a new description or cost needs no refilter. A claim or a mark counts
    /// only while the list reads the LLM status. With "Hide old closed
    /// tasks" on, a change counts when it hides or shows the task. A timer
    /// builds the rows again when the next task gets old enough to hide.
    pub fn needs_refilter(&self, old: &Issue, new: &Issue) -> bool {
        let now = jiff::Timestamp::now();
        let cutoff = self.hide_closed_after.map(|days| hide_cutoff(days, now));
        // A new close time moves the hide timer too.
        cutoff
            .is_some_and(|c| old.hidden_by(c) != new.hidden_by(c) || old.closed_at != new.closed_at)
            || (self.reads_llm_status() && old.llm_rank(now) != new.llm_rank(now))
            || old.deleted != new.deleted
            || old.key != new.key
            || old.title != new.title
            || old.team != new.team
            || old.project != new.project
            || old.parent != new.parent
            || old.order_step != new.order_step
            || old.status != new.status
            || old.labels != new.labels
            || old.assignee != new.assignee
            || old.added != new.added
    }

    /// Filters again after one issue changed: drops issues that went to the
    /// Trash and picks up new ones. The selection stays on its issue, and
    /// the list keeps its scroll, so a status change that moves the issue
    /// to another group does not scroll the list after it. When the issue
    /// left the list, the row that moves up into its place takes the
    /// selection.
    pub fn refilter(&mut self, cx: &mut Context<Self>) {
        self.recompute(Keep::Current { scroll: false }, cx);
    }

    /// The key that names a group in `collapsed`, such as `status:Done`.
    pub fn group_key(&self, title: &str) -> String {
        rows::group_key(self.group, title)
    }

    /// Re-sorts or re-groups and keeps the selected row selected: the same
    /// issue, or the same group header. When the issue's group is now
    /// collapsed, its header takes the selection.
    pub fn refresh_keeping_selection(&mut self, cx: &mut Context<Self>) {
        self.recompute(Keep::Current { scroll: true }, cx);
    }

    fn header_row(&self, title: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r, Row::Header { title: t, .. } if t == title))
    }

    /// The header of the collapsed group that holds `id`, if any.
    fn header_of_issue(&self, id: IssueId, cx: &App) -> Option<usize> {
        let i = self.store.read(cx).get(id)?;
        let title: SharedString = match self.group {
            GroupBy::None => return None,
            GroupBy::Status => i.status.name.clone(),
            GroupBy::Label => i
                .labels
                .first()
                .map_or("No label".into(), |l| l.name.clone()),
            GroupBy::Assignee => i
                .assignee
                .as_ref()
                .map_or("Unassigned".into(), Assignee::name),
            GroupBy::Parent => i.parent.clone().unwrap_or_else(|| rows::NO_PARENT.into()),
        };
        self.header_row(&title)
    }

    /// The header row of the group that row `row` belongs to.
    fn group_header_above(&self, row: usize) -> Option<usize> {
        (0..=row.min(self.rows.len().saturating_sub(1)))
            .rev()
            .find(|&ix| matches!(self.rows[ix], Row::Header { .. }))
    }

    /// Sorts by `key`. The same key again reverses the order.
    pub fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.sort = if self.sort.0 == key {
            (key, !self.sort.1)
        } else {
            (key, false)
        };
        self.refresh_keeping_selection(cx);
    }

    pub fn group_by(&mut self, group: GroupBy, cx: &mut Context<Self>) {
        self.group = group;
        self.refresh_keeping_selection(cx);
    }

    /// Opens or closes the group with header `title`.
    pub fn set_group_collapsed(&mut self, title: &str, collapsed: bool, cx: &mut Context<Self>) {
        let key = self.group_key(title);
        if collapsed {
            self.collapsed.insert(key);
        } else {
            self.collapsed.remove(&key);
        }
        // Keep the header selected while its group opens or closes.
        if let Some(row) = self.header_row(title) {
            self.selected = row;
        }
        self.refresh_keeping_selection(cx);
    }

    /// The group of the selected row: its header title and whether it is
    /// collapsed.
    pub fn current_group(&self) -> Option<(SharedString, bool)> {
        let row = self.group_header_above(self.selected)?;
        match &self.rows[row] {
            Row::Header {
                title, collapsed, ..
            } => Some((title.clone(), *collapsed)),
            Row::Issue(_) => None,
        }
    }

    /// The header on the selected row, if the selection is on one.
    pub fn selected_header(&self) -> Option<(SharedString, bool)> {
        match self.rows.get(self.selected) {
            Some(Row::Header {
                title, collapsed, ..
            }) => Some((title.clone(), *collapsed)),
            _ => None,
        }
    }

    pub fn close_all_groups(&mut self, cx: &mut Context<Self>) {
        let titles: Vec<_> = self
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Header { title, .. } => Some(title.clone()),
                Row::Issue(_) => None,
            })
            .collect();
        for title in titles {
            self.collapsed.insert(self.group_key(&title));
        }
        self.selected = 0;
        self.refresh_keeping_selection(cx);
    }

    pub fn open_all_groups(&mut self, cx: &mut Context<Self>) {
        let prefix = format!("{}:", self.group.label().to_lowercase());
        self.collapsed.retain(|k| !k.starts_with(&prefix));
        self.refresh_keeping_selection(cx);
    }

    /// Whether any group in the list is open.
    fn any_group_open(&self) -> bool {
        self.rows.iter().any(|r| {
            matches!(
                r,
                Row::Header {
                    collapsed: false,
                    ..
                }
            )
        })
    }

    /// Selects the first issue row, or the first row when every group is
    /// closed. Filtering uses this, so Enter picks the first match.
    pub fn select_first(&mut self, cx: &mut Context<Self>) {
        let row = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Issue(_)))
            .unwrap_or(0);
        self.select(row, cx);
    }

    /// Selects `row`. Group headers can be selected too, so the keyboard can
    /// open and close groups.
    pub fn select(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let row = row.min(self.rows.len() - 1);
        self.selected = row;
        // Show the group header above the first issue too.
        let scroll_to = if row == 1 && matches!(self.rows[0], Row::Header { .. }) {
            0
        } else {
            row
        };
        self.scroll
            .scroll_to_item(scroll_to, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// The width at which this list shows every column and fits most of
    /// its titles. The task view beside it grows only past this width.
    pub fn comfortable_width(&self, rem_size: Pixels, cx: &App) -> Pixels {
        columns::comfortable_width_for(&self.demand(cx), rem_size, &self.layout_hidden())
    }

    /// What the titles and labels on the list ask of the columns.
    pub(crate) fn demand(&self, cx: &App) -> columns::Demand {
        self.demand.get().unwrap_or_else(|| {
            let store = self.store.read(cx);
            let rows: Vec<(usize, Vec<usize>)> = self
                .visible
                .iter()
                .filter_map(|id| store.get(*id))
                .map(|issue| {
                    let names = issue.labels.iter().map(|l| l.name.chars().count());
                    (issue.title.chars().count(), names.collect())
                })
                .collect();
            let demand =
                columns::Demand::measure(rows.iter().map(|(t, names)| (*t, names.as_slice())));
            self.demand.set(Some(demand));
            demand
        })
    }

    /// Marks `id`, or clears its mark, and makes it the range anchor.
    pub fn toggle_marked(&mut self, id: IssueId) {
        if !self.marked.remove(&id) {
            self.marked.insert(id);
        }
        self.anchor = Some(id);
        self.range_base = self.marked.clone();
    }

    /// Clears every mark and the range anchor.
    pub fn clear_marks(&mut self) {
        self.marked.clear();
        self.range_base.clear();
        self.anchor = None;
    }

    /// Makes the range from the anchor to `id` the marked range, both ends
    /// included. The anchor stays, so a nearer shift-click shrinks the range
    /// and one past the anchor flips it. Marks made outside the range stay.
    /// Without an anchor in the list, the range starts at the selected row.
    /// Group headers in the range are skipped.
    pub fn mark_range(&mut self, id: IssueId) {
        let Some(to) = self.row_of(id) else {
            return;
        };
        let anchored = self.anchor.and_then(|a| self.row_of(a));
        if anchored.is_none() {
            self.anchor = Some(self.selected_issue().unwrap_or(id));
            self.range_base = self.marked.clone();
        }
        let from = anchored.unwrap_or(self.selected.min(self.rows.len().saturating_sub(1)));
        let (lo, hi) = (from.min(to), from.max(to));
        self.marked = self.range_base.clone();
        for row in &self.rows[lo..=hi] {
            if let Row::Issue(id) = row {
                self.marked.insert(*id);
            }
        }
    }

    pub fn move_down(&mut self, cx: &mut Context<Self>) {
        if self.selected + 1 < self.rows.len() {
            self.select(self.selected + 1, cx);
        }
    }

    pub fn move_up(&mut self, cx: &mut Context<Self>) {
        if self.selected > 0 {
            self.select(self.selected - 1, cx);
        }
    }

    /// What the list says when it shows no rows.
    fn empty_text(&self, cx: &App) -> Option<SharedString> {
        if !self.rows.is_empty() {
            return None;
        }
        let store = self.store.read(cx);
        Some(match (&store.connection, &self.scope) {
            (Connection::Connecting, _) if store.is_empty() => "Loading issues…".into(),
            (Connection::Offline(reason), _) if store.is_empty() => {
                format!("Cannot reach the llmtodo server: {reason}").into()
            }
            (_, Scope::Trash) if self.query.is_empty() => "The Trash is empty".into(),
            (_, Scope::View(name)) if store.view.as_ref().is_none_or(|v| v.name != *name) => {
                format!("Loading {name}…").into()
            }
            _ if store.iter().all(|i| i.deleted) => "No issues yet".into(),
            _ if self.query.is_empty() => {
                format!("No issues in {}", self.scope.name(&store.catalog)).into()
            }
            _ => "No issue matches the filter".into(),
        })
    }

    /// Collapse all, or expand all when every group is closed, like the
    /// button in VS Code's explorer. Only shown while the list is grouped.
    /// Whether the list is wide enough to show `column`.
    /// The columns the layout leaves out: the owner's hidden columns, and
    /// the status while the groups already name it. The status comes back
    /// on its own under another grouping, and the owner's setting stays.
    fn layout_hidden(&self) -> Vec<Column> {
        let mut hidden = self.hidden_columns.clone();
        if self.group == GroupBy::Status && !hidden.contains(&Column::Status) {
            hidden.push(Column::Status);
        }
        // The order line belongs to the parent groups.
        if self.group != GroupBy::Parent && !hidden.contains(&Column::Order) {
            hidden.push(Column::Order);
        }
        hidden
    }

    fn shows(&self, column: Column) -> bool {
        self.layout.columns.contains(&column)
    }

    /// Hides `column`, or shows it again. The title always shows.
    pub fn toggle_column(&mut self, column: Column, cx: &mut Context<Self>) {
        if column == Column::Title {
            return;
        }
        match self.hidden_columns.iter().position(|c| *c == column) {
            Some(ix) => {
                self.hidden_columns.remove(ix);
            }
            None => self.hidden_columns.push(column),
        }
        cx.notify();
    }

    fn collapse_all_button(&self) -> Option<AnyElement> {
        if self.group == GroupBy::None {
            return None;
        }
        let open = self.any_group_open();
        let events = self.events.clone();
        let card = Card::text(if open {
            "Collapse all groups (zM)"
        } else {
            "Expand all groups (zR)"
        });
        Some(with_card(
            "collapse-all-tip",
            Button::new("collapse-all")
                .ghost()
                .xsmall()
                .icon(if open {
                    IconName::ChevronsDownUp
                } else {
                    IconName::ChevronsUpDown
                })
                .on_click(move |_, window, cx| {
                    events.emit(
                        if open {
                            ViewEvent::CloseAllGroups
                        } else {
                            ViewEvent::OpenAllGroups
                        },
                        window,
                        cx,
                    );
                }),
            card,
        ))
    }

    /// The badge in the LLM status column: `Locked` while an agent holds a
    /// claim, `Analyzing` while one reads or fills in the task. It grows in
    /// when the state starts and shrinks away when it ends. A spring drives
    /// both ways, so a quick change mid-animation reverses smoothly.
    fn render_llm_badge(
        issue: &Issue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let now = jiff::Timestamp::now();
        let state = issue.llm_status(now);
        let spring = cx.theme().motion_tokens().spring_control;
        // Keyed by ID, so a team move does not restart the animation.
        let target = if state.is_some() { 1.0_f32 } else { 0.0 };
        let progress: f32 = gpui_kit::base::spring(
            widgets::issue_part("llm", issue.id),
            target,
            spring,
            window,
            cx,
        )
        .clamp(0., 1.);
        if progress <= 0.001 {
            return None;
        }
        let theme = cx.theme();
        // The workspace clock redraws the list each minute, so the age
        // counts up and an expired mark goes away.
        let (icon, age, card, bg, fg, kind) = match &state {
            // The state just ended and the badge shrinks away. It keeps a
            // neutral fill and no content for those few frames.
            None => (
                None,
                String::new(),
                crate::shell::tooltip::Card::default(),
                theme.muted,
                theme.muted_foreground,
                "llm-ending",
            ),
            Some(LlmStatus::Locked(claim)) => {
                let (bg, fg) = crate::shell::theme::locked_badge(theme);
                (
                    Some(IconName::Lock),
                    claim.badge(now),
                    crate::shell::tooltip::Card::claim(claim, now),
                    bg,
                    fg,
                    "llm-locked",
                )
            }
            // Analyzing stays monochrome: only Locked has the owner's color.
            Some(LlmStatus::Analyzing(mark)) => (
                Some(IconName::Eye),
                mark.age(now),
                crate::shell::tooltip::Card::analyzing(mark, now),
                theme.muted,
                theme.muted_foreground,
                "llm-analyzing",
            ),
        };
        let age: SharedString = age.into();
        Some(
            div()
                .id(widgets::issue_part(kind, issue.id))
                .test_support()
                .h_5()
                .flex()
                .items_center()
                .overflow_hidden()
                // The full badge is as wide as the column.
                .w(gpui_kit::rems(columns::LLM_BADGE_REM * progress))
                .opacity(progress)
                .child(
                    h_flex()
                        .flex_none()
                        .w(gpui_kit::rems(columns::LLM_BADGE_REM))
                        .h_5()
                        .gap_1()
                        .px_1p5()
                        .rounded(theme.radius)
                        .text_xs()
                        .bg(bg)
                        .text_color(fg)
                        .children(icon.map(|icon| Icon::new(icon).xsmall()))
                        .child(
                            div()
                                .id(widgets::issue_part("llm-age", issue.id))
                                .test_support()
                                .min_w_0()
                                .truncate()
                                .child(age),
                        ),
                )
                .tooltip(card.builder())
                .into_any_element(),
        )
    }

    /// A group header. A click or Enter opens and closes the group. Under
    /// "group by status", dropping a task here moves it to this status.
    #[allow(clippy::too_many_arguments)]
    fn render_header_row(
        &self,
        row: usize,
        title: SharedString,
        subtitle: Option<SharedString>,
        count: usize,
        label: Option<Label>,
        status: Option<Status>,
        collapsed: bool,
        list_focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = label.map(|l| label_color(l.color, cx));
        let theme = cx.theme();
        let selected = row == self.selected;
        let rest = theme.muted.opacity(0.5);
        let hover = theme.muted;
        let pressed = crate::shell::theme::row_pressed(theme);
        let selected_hover = crate::shell::theme::selection_hover(theme);
        let drop = theme.foreground.opacity(0.16);
        let selected_bg = crate::shell::theme::selection_bg(theme, list_focused);
        let toggle_title = title.clone();
        let events = self.events.clone();
        let drop_events = self.events.clone();
        h_flex()
            .id(("row", row))
            .test_support()
            .w_full()
            .h_7()
            .px_2()
            .gap_2()
            .text_xs()
            .cursor_pointer()
            .bg(if selected { selected_bg } else { rest })
            .hover(move |s| s.bg(if selected { selected_hover } else { hover }))
            .active(move |s| s.bg(pressed))
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .small()
                .text_color(theme.muted_foreground),
            )
            .when_some(color, |this, color| {
                this.child(div().size_2().rounded_full().bg(color))
            })
            .when_some(status.clone(), |this, status| {
                this.child(
                    Icon::new(status.icon())
                        .small()
                        .text_color(if status.is_done() {
                            theme.success
                        } else {
                            theme.muted_foreground
                        }),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_color(theme.foreground)
                    .when(subtitle.is_some(), |this| {
                        this.font_family(theme.mono_font_family.clone())
                    })
                    .child(title.clone()),
            )
            .when_some(subtitle, |this, subtitle| {
                this.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.foreground)
                        .child(subtitle),
                )
            })
            .child(
                div()
                    .flex_none()
                    .text_color(theme.muted_foreground)
                    .child(count.to_string()),
            )
            .on_click(move |_, window, cx| {
                events.emit(
                    ViewEvent::ClickHeader {
                        row,
                        title: toggle_title.to_string(),
                        collapsed,
                    },
                    window,
                    cx,
                );
            })
            .when(self.group == GroupBy::Parent, |this| {
                let parent = (title.as_ref() != rows::NO_PARENT).then(|| title.clone());
                let events = self.events.clone();
                this.drag_over::<DraggedIssue>(move |s, _, _, _| s.bg(drop))
                    .on_drop(move |dragged: &DraggedIssue, window, cx| {
                        events.emit(
                            ViewEvent::OnIssue(
                                dragged.issue,
                                IssueAction::SetParent(parent.clone()),
                            ),
                            window,
                            cx,
                        );
                    })
            })
            .when_some(
                status.filter(|_| self.group == GroupBy::Status),
                |this, status| {
                    this.drag_over::<DraggedIssue>(move |s, _, _, _| s.bg(drop))
                        .on_drop(move |dragged: &DraggedIssue, window, cx| {
                            drop_events.emit(
                                ViewEvent::OnIssue(
                                    dragged.issue,
                                    IssueAction::SetStatus(status.clone()),
                                ),
                                window,
                                cx,
                            );
                        })
                },
            )
            .into_any_element()
    }

    fn render_row(
        &self,
        row: usize,
        list_focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match &self.rows[row] {
            Row::Issue(id) => *id,
            Row::Header {
                title,
                subtitle,
                count,
                label,
                status,
                collapsed,
            } => {
                return self.render_header_row(
                    row,
                    title.clone(),
                    subtitle.clone(),
                    *count,
                    label.clone(),
                    status.clone(),
                    *collapsed,
                    list_focused,
                    cx,
                );
            }
        };
        self.render_issue_row(row, id, list_focused, window, cx)
    }

    /// The box that marks a task. It shows on the hovered row, and on every
    /// row while any task is marked. A click toggles the mark and leaves the
    /// open task alone; shift-click marks a range.
    fn mark_box(
        &self,
        row: usize,
        id: IssueId,
        marked: bool,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (fg, page, muted) = (theme.foreground, theme.background, theme.muted_foreground);
        let always = !self.marked.is_empty();
        let events = self.events.clone();
        let square = div()
            .id(("row-check-box", row))
            .size_3p5()
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .map(|this| {
                if marked {
                    this.bg(fg)
                        .group_hover(CHECK_GROUP, move |s| s.bg(fg.opacity(0.8)))
                        .group_active(CHECK_GROUP, move |s| s.bg(fg.opacity(0.6)))
                        .child(Icon::new(IconName::Check).xsmall().text_color(page))
                } else {
                    this.border_1()
                        .border_color(muted)
                        .group_hover(CHECK_GROUP, move |s| {
                            s.border_color(fg).bg(fg.opacity(0.12))
                        })
                        .group_active(CHECK_GROUP, move |s| {
                            s.border_color(fg).bg(fg.opacity(0.28))
                        })
                }
            });
        let target = h_flex()
            .id(("row-check", row))
            .test_support()
            .group(CHECK_GROUP)
            .size_full()
            .justify_center()
            .cursor_pointer()
            .when(!always, |this| {
                this.opacity(0.).group_hover(ROW_GROUP, |s| s.opacity(1.))
            })
            .on_click(move |event, window, cx| {
                cx.stop_propagation();
                let click = if event.modifiers().shift {
                    RowClick::Range
                } else {
                    RowClick::Toggle
                };
                events.emit(ViewEvent::ClickRow { id, click }, window, cx);
            })
            .child(square)
            .tooltip(
                Card::text(if marked {
                    "Unselect this task (x)"
                } else {
                    "Select this task (x)"
                })
                .builder(),
            );
        // On the outside, so the box still sees the press but the row does
        // not: the row neither shows as pressed nor starts a drag.
        div()
            .size_full()
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation()
            })
            .child(target)
    }

    /// One task's row.
    fn render_issue_row(
        &self,
        row: usize,
        id: IssueId,
        list_focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A shared copy: one counter bump, not a copy of the issue.
        let Some(issue) = self.store.read(cx).get_shared(id) else {
            return div().id(("row", row)).h_7().into_any_element();
        };
        let selected = row == self.selected;
        let closed = issue.status.is_closed();
        let struck = self.strike_done && issue.status.is_done();
        let layout = self.layout.clone();
        let llm = Self::render_llm_badge(&issue, window, cx);
        let marked = self.marked.contains(&issue.id);
        let theme = cx.theme();
        let hover = theme.list_hover;
        let pressed = crate::shell::theme::row_pressed(theme);
        let selected_hover = crate::shell::theme::selection_hover(theme);
        // A selection in a list without focus is drawn fainter.
        let selected_bg = crate::shell::theme::selection_bg(theme, list_focused);
        // A claimed row has no tint of its own: the Locked badge in the
        // LLM column marks it.
        let muted = theme.muted_foreground;
        let key = widgets::issue_key(
            ("row-key", row).into(),
            &issue,
            self.events.clone(),
            muted,
            cx,
        )
        .into_any_element();
        let deps = widgets::dependency_badge(&issue, muted, cx);
        // Every marked row is highlighted, also when the list has no focus.
        // The cursor row stays visible inside a run of marked rows.
        let marked_bg = theme.list_active;
        let cursor_on_marked = theme.foreground.opacity(0.13);
        let cursor_on_marked_hover = theme.foreground.opacity(0.16);
        let filled = selected || marked;
        let hover = match (selected && marked, filled) {
            (true, _) => cursor_on_marked_hover,
            (false, true) => selected_hover,
            (false, false) => hover,
        };
        let check = self.mark_box(row, id, marked, cx);
        let click_events = self.events.clone();
        columns::row_frame()
            .id(("row", row))
            .test_support()
            .group(ROW_GROUP)
            .relative()
            .text_sm()
            .cursor_pointer()
            .when(marked, |this| this.bg(marked_bg))
            .when(selected, |this| this.bg(selected_bg))
            .when(selected && marked, |this| this.bg(cursor_on_marked))
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .when(self.shows(Column::Check), |this| {
                this.child(cell_with(Column::Check, &layout, check))
            })
            .when(self.shows(Column::Llm), |this| {
                this.child(cell_with(Column::Llm, &layout, div().children(llm)))
            })
            .when(self.shows(Column::Order), |this| {
                let mark = self.order.get(row).copied().flatten();
                this.child(cell_with(Column::Order, &layout, order_cell(mark, cx)))
            })
            .when(self.shows(Column::Status), |this| {
                this.child(cell_with(
                    Column::Status,
                    &layout,
                    status_cell(&issue, muted, cx),
                ))
            })
            .when(self.shows(Column::Key), |this| {
                this.child(cell_with(Column::Key, &layout, key))
            })
            .child(cell_with(
                Column::Title,
                &layout,
                div()
                    .id(if struck { "title-struck" } else { "title" })
                    .test_support()
                    .w_full()
                    .truncate()
                    .when(closed, |this| this.text_color(muted))
                    .when(struck, |mut this| {
                        this.text_style().strikethrough = Some(gpui_kit::StrikethroughStyle {
                            thickness: gpui_kit::px(1.),
                            color: Some(muted.opacity(0.6)),
                        });
                        this
                    })
                    .child(issue.title.clone()),
            ))
            // As many tags as fit, then "+N" for the rest. The last tag
            // shown is cut off if it must be.
            .when(self.shows(Column::Labels), |this| {
                let fit = columns::labels_that_fit(
                    issue.labels.iter().map(|label| label.name.chars().count()),
                    layout.labels,
                );
                let rest = issue.labels.len() - fit;
                let tags = issue.labels.iter().take(fit).map(|l| widgets::tag(l, cx));
                this.child(cell_with(
                    Column::Labels,
                    &layout,
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_1()
                        .children(tags.enumerate().map(|(ix, tag)| {
                            if ix + 1 == fit {
                                tag.min_w_0().truncate()
                            } else {
                                tag.flex_none()
                            }
                        }))
                        .when(rest > 0, |this| {
                            this.child(
                                div()
                                    .id("labels-more")
                                    .test_support()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!("+{rest}")),
                            )
                        }),
                ))
            })
            .when(self.shows(Column::Deps), |this| {
                this.child(cell_with(Column::Deps, &layout, div().children(deps)))
            })
            .when(self.shows(Column::Waiting), |this| {
                this.child(cell_with(
                    Column::Waiting,
                    &layout,
                    h_flex()
                        .w_full()
                        .justify_center()
                        .children(widgets::waiting_on(
                            widgets::issue_part("waiting", issue.id),
                            issue.waiting_on,
                            cx,
                        )),
                ))
            })
            .when(self.shows(Column::Assignee), |this| {
                this.child(cell_with(
                    Column::Assignee,
                    &layout,
                    assignee_cell(&issue, muted),
                ))
            })
            .when(self.shows(Column::Cost), |this| {
                this.child(cell_with(
                    Column::Cost,
                    &layout,
                    right_mono_text(muted, cx)
                        .children(issue.cost.map(crate::model::data::format_cost)),
                ))
            })
            .when(self.shows(Column::Time), |this| {
                this.child(cell_with(
                    Column::Time,
                    &layout,
                    right_mono_text(muted, cx).children(
                        issue
                            .active_seconds
                            .map(llmtodo_client::types::format_active_time),
                    ),
                ))
            })
            .when(self.shows(Column::Added), |this| {
                this.child(cell_with(
                    Column::Added,
                    &layout,
                    right_mono_text(muted, cx).child(issue.added.short()),
                ))
            })
            // A click selects the task and opens it to the side. Focus stays
            // in the list, so j / k keep working.
            // Shift-click marks a range; cmd-click marks one task.
            .on_click(move |event, window, cx| {
                let click = row_click(event.modifiers(), cx);
                click_events.emit(ViewEvent::ClickRow { id, click }, window, cx);
            })
            // Under "group by status", drag a task onto another group, or
            // onto any task in it, to change its status.
            .when(self.group == GroupBy::Status, |this| {
                self.with_status_drag(this, &issue, cx)
            })
            // Under "group by parent", drag a task onto another group, or
            // onto any task in it, to give it that parent.
            .when(self.group == GroupBy::Parent, |this| {
                self.with_parent_drag(this, &issue, cx)
            })
            .context_menu(crate::shell::issue_menu::issue_menu(
                self.store.clone(),
                self.events.clone(),
                issue.id,
            ))
            .into_any_element()
    }

    /// A canvas that records the list's width after layout.
    fn measure_width(
        &self,
        rem_size: gpui_kit::Pixels,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        // Records the list's width after layout. When the new width fits
        // other columns, the list draws again with them.
        let width = self.width.clone();
        let entity = cx.entity_id();
        let hidden = self.layout_hidden();
        let demand = self.demand(cx);
        gpui_kit::canvas(
            move |bounds, _, cx| {
                let old = width.replace(Some(bounds.size.width));
                if columns::plan(old, rem_size, &hidden, &demand)
                    != columns::plan(Some(bounds.size.width), rem_size, &hidden, &demand)
                {
                    cx.notify(entity);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }

    /// The filter field, the grouping menu and the button that folds groups.
    fn render_filter_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let collapse_all = self.collapse_all_button();
        let border = cx.theme().border;
        let (down_events, up_events) = (self.events.clone(), self.events.clone());
        let group_events = self.events.clone();
        let new_events = self.events.clone();
        let list = cx.entity();
        h_flex()
            .key_context("Filter")
            // The filter is a single line, so Up and Down move the
            // list selection instead of the caret.
            .capture_action(move |_: &ed::MoveDown, window, cx| {
                cx.stop_propagation();
                down_events.emit(ViewEvent::MoveDown, window, cx);
            })
            .capture_action(move |_: &ed::MoveUp, window, cx| {
                cx.stop_propagation();
                up_events.emit(ViewEvent::MoveUp, window, cx);
            })
            .id("filter-bar")
            .test_support()
            // The task view's header lines up with this bar.
            .h(crate::shell::widgets::top_bar_height(window))
            .flex_none()
            .items_center()
            .px_2()
            .gap_2()
            .border_b_1()
            .border_color(border)
            .child(with_card(
                "add-ticket-tip",
                Button::new("add-ticket")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label("Add ticket")
                    .on_click(move |_, window, cx| {
                        new_events.emit(ViewEvent::NewIssue, window, cx);
                    }),
                Card::text("Add a ticket (c)"),
            ))
            .child(
                crate::shell::widgets::text_input(&self.filter)
                    .prefix(Icon::new(IconName::Search).small()),
            )
            .child(with_card(
                "group-tip",
                Button::new("group")
                    .ghost()
                    .small()
                    .icon(IconName::LayoutList)
                    .label(format!("Group: {}", self.group.label()))
                    .selected(self.group != GroupBy::None)
                    .dropdown_menu(move |menu, _, cx| {
                        let current = list.read(cx).group;
                        [
                            GroupBy::None,
                            GroupBy::Status,
                            GroupBy::Label,
                            GroupBy::Assignee,
                            GroupBy::Parent,
                        ]
                        .into_iter()
                        .fold(menu, |menu, group| {
                            let events = group_events.clone();
                            menu.item(
                                PopupMenuItem::new(group.label())
                                    .checked(group == current)
                                    .on_click(move |_, window, cx| {
                                        events.emit(ViewEvent::GroupBy(group), window, cx);
                                    }),
                            )
                        })
                    }),
                Card::text("Change grouping (:group)"),
            ))
            // Next to the grouping it acts on. In the column header
            // it would push the cells out of line with the rows.
            .when_some(collapse_all, |this, button| this.child(button))
    }

    /// The column titles. The sortable ones are buttons.
    fn render_column_header(&self, cx: &Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let border = theme.border;
        let sort_header = |id: &'static str, label: &'static str, name: &str, key: SortKey| {
            let active = self.sort.0 == key;
            let events = self.events.clone();
            let button = Button::new(id)
                .ghost()
                .xsmall()
                .label(label)
                .when(active, |b| {
                    b.icon(if self.sort.1 {
                        IconName::ArrowDown
                    } else {
                        IconName::ArrowUp
                    })
                })
                .selected(active)
                .on_click(move |_, window, cx| {
                    events.emit(ViewEvent::SortBy(key), window, cx);
                });
            with_card(
                format!("{id}-tip"),
                button,
                Card::text(format!("Sort by {name}")),
            )
        };
        let muted = theme.muted_foreground;
        let layout = self.layout.clone();
        let label = move |text: &'static str| div().px_1().text_color(muted).child(text);
        // The same frame and cells as an issue row. The bottom line sits on
        // a wrapper.
        let cells = columns::row_frame()
            .h_full()
            .text_xs()
            .when(self.shows(Column::Check), |this| {
                this.child(cell_with(Column::Check, &layout, div()))
            })
            .when(self.shows(Column::Llm), |this| {
                this.child(cell_with(
                    Column::Llm,
                    &layout,
                    sort_header("sort-llm", "LLM", "LLM status", SortKey::Llm),
                ))
            })
            .when(self.shows(Column::Order), |this| {
                this.child(cell_with(Column::Order, &layout, div()))
            })
            .when(self.shows(Column::Status), |this| {
                this.child(cell_with(
                    Column::Status,
                    &layout,
                    sort_header("sort-status", "Status", "status", SortKey::Status),
                ))
            })
            .when(self.shows(Column::Key), |this| {
                this.child(cell_with(
                    Column::Key,
                    &layout,
                    sort_header("sort-key", "Key", "key", SortKey::Key),
                ))
            })
            .child(cell_with(Column::Title, &layout, label("Title")))
            .when(self.shows(Column::Labels), |this| {
                this.child(cell_with(Column::Labels, &layout, label("Labels")))
            })
            .when(self.shows(Column::Deps), |this| {
                this.child(cell_with(Column::Deps, &layout, div()))
            })
            .when(self.shows(Column::Waiting), |this| {
                this.child(cell_with(Column::Waiting, &layout, div()))
            })
            .when(self.shows(Column::Assignee), |this| {
                this.child(cell_with(Column::Assignee, &layout, div()))
            })
            .when(self.shows(Column::Cost), |this| {
                this.child(cell_with(
                    Column::Cost,
                    &layout,
                    h_flex().w_full().justify_end().child(label("Cost")),
                ))
            })
            .when(self.shows(Column::Time), |this| {
                this.child(cell_with(
                    Column::Time,
                    &layout,
                    h_flex().w_full().justify_end().child(label("Time")),
                ))
            })
            .when(self.shows(Column::Added), |this| {
                this.child(cell_with(
                    Column::Added,
                    &layout,
                    h_flex().w_full().justify_end().child(sort_header(
                        "sort-added",
                        "Added",
                        "added",
                        SortKey::Added,
                    )),
                ))
            });
        let list = cx.weak_entity();
        div()
            .id("column-header")
            .test_support()
            .flex_none()
            .h_7()
            .border_b_1()
            .border_color(border)
            .child(cells)
            // A checklist of the columns. It reads the list each time it
            // opens, so the checks are always current.
            .context_menu(move |menu, _, cx| {
                let (hidden, group) = list
                    .upgrade()
                    .map(|list| {
                        let list = list.read(cx);
                        (list.hidden_columns.clone(), list.group)
                    })
                    .unwrap_or((Vec::new(), GroupBy::None));
                Column::ALL.into_iter().fold(menu, |menu, column| {
                    menu.menu_with_check_and_disabled(
                        column.label(),
                        !hidden.contains(&column),
                        Box::new(columns::ToggleColumn(column)),
                        // The order line shows only under "group by parent".
                        column == Column::Title
                            || (column == Column::Order && group != GroupBy::Parent),
                    )
                })
            })
    }

    /// Lets a row be dragged onto another group, or onto a task in it, to
    /// change its status.
    fn with_status_drag<E: gpui_kit::StatefulInteractiveElement>(
        &self,
        this: E,
        issue: &Issue,
        cx: &App,
    ) -> E {
        let theme = cx.theme();
        let dragged = DraggedIssue {
            issue: issue.id,
            key: issue.key.clone(),
            title: issue.title.clone(),
        };
        let status = issue.status.clone();
        let drop = theme.foreground.opacity(0.16);
        let events = self.events.clone();
        let store = self.store.clone();
        this.on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedIssue>(move |s, _, _, _| s.bg(drop))
            .on_drop(move |dragged: &DraggedIssue, window, cx| {
                let same = store
                    .read(cx)
                    .get(dragged.issue)
                    .is_some_and(|i| i.status == status);
                if same {
                    return;
                }
                events.emit(
                    ViewEvent::OnIssue(dragged.issue, IssueAction::SetStatus(status.clone())),
                    window,
                    cx,
                );
            })
    }
}

impl IssueList {
    /// Lets a row be dragged onto another parent group, or onto a task in
    /// it, to give it that parent.
    fn with_parent_drag<E: gpui_kit::StatefulInteractiveElement>(
        &self,
        this: E,
        issue: &Issue,
        cx: &App,
    ) -> E {
        let dragged = DraggedIssue {
            issue: issue.id,
            key: issue.key.clone(),
            title: issue.title.clone(),
        };
        let parent = issue.parent.clone();
        let drop = cx.theme().foreground.opacity(0.16);
        let events = self.events.clone();
        let store = self.store.clone();
        this.on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedIssue>(move |s, _, _, _| s.bg(drop))
            .on_drop(move |dragged: &DraggedIssue, window, cx| {
                let same = store
                    .read(cx)
                    .get(dragged.issue)
                    .is_some_and(|i| i.parent == parent);
                if same {
                    return;
                }
                events.emit(
                    ViewEvent::OnIssue(dragged.issue, IssueAction::SetParent(parent.clone())),
                    window,
                    cx,
                );
            })
    }
}

/// The order line in one row: a line through the row, cut at the first
/// and last ordered rows, and a dot on the first row of each step. A
/// filled dot means every task of the step is done or canceled. When the
/// order has more than one step, the dot is larger and shows the step
/// number. A row with no step draws nothing.
fn order_cell(mark: Option<rows::OrderMark>, cx: &App) -> AnyElement {
    let Some(mark) = mark else {
        return div().into_any_element();
    };
    let theme = cx.theme();
    let line = theme.muted_foreground.opacity(0.6);
    let fg = theme.muted_foreground;
    let page = theme.background;
    let segment = |show: bool| {
        div()
            .flex_1()
            .w(gpui_kit::px(1.5))
            .when(show, |this| this.bg(line))
    };
    v_flex()
        .id("order-mark")
        .test_support()
        .relative()
        .size_full()
        .items_center()
        .child(segment(mark.up))
        .child(segment(mark.down))
        .children(mark.dot.map(|filled| {
            // Fits two digits and stays inside the row.
            let size = if mark.number.is_some() { 14. } else { 8. };
            div()
                .id(if filled {
                    "order-dot-done"
                } else {
                    "order-dot"
                })
                .test_support()
                .absolute()
                .top(gpui_kit::relative(0.5))
                .mt(gpui_kit::px(-size / 2.))
                .size(gpui_kit::px(size))
                .rounded_full()
                .border_1()
                .border_color(fg)
                .bg(if filled { fg } else { page })
                .flex()
                .items_center()
                .justify_center()
                .when_some(mark.number, |this, number| {
                    // The number takes the colour the dot is not filled
                    // with, so it reads on both kinds in every theme.
                    this.child(
                        div()
                            .id("order-number")
                            .test_support()
                            .text_size(gpui_kit::px(8.5))
                            .line_height(gpui_kit::px(8.5))
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .text_color(if filled { page } else { fg })
                            .child(number.to_string()),
                    )
                })
        }))
        .into_any_element()
}

impl Render for IssueList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        let rem_size = window.rem_size();
        let demand = self.demand(cx);
        let layout = columns::plan(self.width.get(), rem_size, &self.layout_hidden(), &demand);
        if *self.layout != layout {
            self.layout = std::rc::Rc::new(layout);
        }
        let measure = self.measure_width(rem_size, cx);
        let filter_bar = self.render_filter_bar(window, cx);
        let header = self.render_column_header(cx);
        v_flex()
            .relative()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            // The workspace draws this view cached, so GPUI lays it out as
            // its own root, where `flex_1` does nothing. It fills by size.
            .size_full()
            .child(measure)
            .child(filter_bar)
            .child(header)
            .when_some(self.empty_text(cx), |this, text| {
                this.child(
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(text),
                )
            })
            .child(
                uniform_list(
                    "issues",
                    self.rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                        let list_focused = this.focus.is_focused(window);
                        range
                            .map(|row| this.render_row(row, list_focused, window, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1(),
            )
    }
}

/// The status cell of a row.
fn status_cell(issue: &Issue, muted: gpui_kit::Hsla, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let done = issue.status.is_done();
    h_flex()
        .w_full()
        .min_w_0()
        .gap_1p5()
        .text_xs()
        .text_color(muted)
        .child(Icon::new(issue.status.icon()).small().text_color(if done {
            theme.success
        } else {
            muted
        }))
        .child(div().truncate().child(issue.status.name.clone()))
}

/// The assignee cell of a row: an agent's icon or a person's name.
fn assignee_cell(issue: &Issue, muted: gpui_kit::Hsla) -> impl IntoElement + use<> {
    h_flex()
        .w_full()
        .justify_center()
        .text_xs()
        .text_color(muted)
        .map(|this| match &issue.assignee {
            Some(Assignee::Agent(agent)) => this.child(with_card(
                widgets::issue_part("assignee", issue.id),
                Icon::new(agent.icon()).small().text_color(muted),
                Card::text(format!("Assigned to {}", agent.label())),
            )),
            Some(Assignee::Person(name)) => this.child(name.clone()),
            None => this,
        })
}

/// Small right-aligned monospace text, for the cost and date columns.
fn right_mono_text(muted: gpui_kit::Hsla, cx: &App) -> gpui_kit::Div {
    div()
        .w_full()
        .text_right()
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(muted)
}

/// What a click on a row does, from the keys held during the click.
fn row_click(modifiers: gpui_kit::Modifiers, cx: &App) -> RowClick {
    if modifiers.shift {
        RowClick::Range
    } else if modifiers.secondary(cx) {
        RowClick::Toggle
    } else {
        RowClick::Plain
    }
}
