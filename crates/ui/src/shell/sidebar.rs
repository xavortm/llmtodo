//! The sidebar: every issue, each team with its projects under it, each
//! saved view and the Trash. A row picks the scope the issue list shows. Only rows that
//! work are drawn.
//!
//! `Cmd-B` or the title bar button shows and hides it. With the sidebar
//! open, `h` in the list moves focus into it. There `j` / `k` and
//! `Ctrl-N` / `Ctrl-P` move between rows and show that scope at once, and
//! `l`, `Enter` or `Esc` go back to the list.

use std::collections::{HashMap, HashSet};

use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, h_flex,
    menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::{
    App, Context, ElementId, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _,
};

use crate::model::data::Catalog;
use crate::model::scope::Scope;
use crate::model::store::IssueStore;
use crate::shell::icons::HasIcon as _;
use crate::shell::view_events::{ViewEvent, ViewEvents};

pub const CONTEXT: &str = "Sidebar";

/// One sidebar row: the scope it picks and the heading drawn above it.
/// An empty heading draws a divider. A nested row is a project inside its
/// team, drawn indented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub heading: Option<&'static str>,
    pub scope: Scope,
    pub nested: bool,
}

impl Row {
    fn new(heading: Option<&'static str>, scope: Scope) -> Self {
        Self {
            heading,
            scope,
            nested: false,
        }
    }
}

/// The rows in order. Each team is a group: its row shows the whole team,
/// and its projects follow it, indented. A project linked to several teams
/// shows under each of them. Projects with no known team come after the
/// teams, under a Projects heading.
pub fn scopes(catalog: &Catalog) -> Vec<Row> {
    let mut out = vec![Row::new(None, Scope::All), Row::new(None, Scope::Inbox)];
    let teams_of = |slug: &SharedString| catalog.project_teams.get(slug).map(Vec::as_slice);
    for team in &catalog.team_names {
        out.push(Row::new(None, Scope::Team(team.key.clone())));
        for project in &catalog.projects {
            if teams_of(&project.key).is_some_and(|t| t.contains(&team.key)) {
                out.push(Row {
                    heading: None,
                    scope: Scope::Project(project.key.clone()),
                    nested: true,
                });
            }
        }
    }
    let loose = catalog.projects.iter().filter(|project| {
        !teams_of(&project.key)
            .unwrap_or_default()
            .iter()
            .any(|key| catalog.team_names.iter().any(|t| t.key == *key))
    });
    for (ix, project) in loose.enumerate() {
        out.push(Row::new(
            (ix == 0).then_some("Projects"),
            Scope::Project(project.key.clone()),
        ));
    }
    for (ix, view) in catalog.views.iter().enumerate() {
        out.push(Row::new(
            (ix == 0).then_some("Views"),
            Scope::View(view.key.clone()),
        ));
    }
    for (ix, doc) in catalog.documents.iter().enumerate() {
        out.push(Row::new(
            (ix == 0).then_some("Documents"),
            Scope::Document(doc.id.to_string().into()),
        ));
    }
    out.push(Row::new(Some(""), Scope::Settings));
    out.push(Row::new(None, Scope::Trash));
    out
}

pub struct Sidebar {
    store: Entity<IssueStore>,
    events: ViewEvents,
    pub focus: FocusHandle,
    /// The scope the list shows. The workspace sets it.
    pub active: Scope,
    /// The row the owner last picked. A project can show under several
    /// teams, so the scope alone does not say which row is active.
    cursor: usize,
    /// The rows' hover fills, drawn under this view (LLM-345).
    pub fill_layer: Entity<crate::shell::motion::FillLayer>,
    /// What the rows show, from the store, until a change to what they
    /// count. None draws it again from the store (LLM-345).
    data: Option<std::rc::Rc<SidebarData>>,
    /// What each issue adds to the counts, as of the last scan, and the
    /// store revision the sidebar last looked at.
    counted: HashMap<llmtodo_core::IssueId, Counted>,
    store_seen: u64,
    /// How often the rows counted every issue, for tests.
    #[cfg(test)]
    pub scans: usize,
    _store: Subscription,
}

/// What the sidebar draws from the store: a copy of the catalog, its
/// rows, and each row's count and lock.
struct SidebarData {
    catalog: Catalog,
    rows: Vec<Row>,
    tallies: Vec<(usize, bool)>,
    trackers: Vec<crate::model::trackers::Tracker>,
    open_tracker: usize,
}

/// The fields of one issue that [`ScopeCounts`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Counted {
    deleted: bool,
    closed: bool,
    team: SharedString,
    project: Option<SharedString>,
    claimed: bool,
}

impl Counted {
    fn of(issue: &crate::model::data::Issue) -> Self {
        Self {
            deleted: issue.deleted,
            closed: issue.status.is_closed(),
            team: issue.team.clone(),
            project: issue.project.clone(),
            claimed: issue.claim.is_some(),
        }
    }
}

impl Sidebar {
    pub fn new(store: Entity<IssueStore>, events: ViewEvents, cx: &mut Context<Self>) -> Self {
        // The workspace draws the sidebar as a cached view, so it redraws
        // only when it is notified. Everything it shows comes from the store.
        let subscription = cx.observe(&store, |this: &mut Self, store, cx| {
            if this.store_changed(&store, cx) {
                this.data = None;
                cx.notify();
            }
        });
        Self {
            _store: subscription,
            store,
            events,
            focus: cx.focus_handle(),
            active: Scope::All,
            cursor: 0,
            fill_layer: gpui_kit::AppContext::new(cx, |_| Default::default()),
            data: None,
            counted: HashMap::new(),
            store_seen: 0,
            #[cfg(test)]
            scans: 0,
        }
    }

    /// Whether a store change may change a row: anything but a change to
    /// issues whose counted fields stayed the same.
    fn store_changed(&mut self, store: &Entity<IssueStore>, cx: &App) -> bool {
        let store = store.read(cx);
        let change = store.changes_since(self.store_seen);
        self.store_seen = store.revision();
        let crate::model::store::StoreChange::Issues(ids) = change else {
            return true;
        };
        if self.data.is_none() {
            return true;
        }
        let questions = crate::inbox::open_count(store);
        let counted_questions = self.data.as_ref().and_then(|d| {
            d.rows
                .iter()
                .position(|r| r.scope == Scope::Inbox)
                .map(|ix| d.tallies[ix].0)
        });
        if counted_questions.is_some_and(|n| n != questions) {
            return true;
        }
        ids.into_iter()
            .any(|id| self.counted.get(&id) != store.get(id).map(Counted::of).as_ref())
    }

    /// What the rows show, counted again only after a change to it.
    fn data(&mut self, cx: &App) -> std::rc::Rc<SidebarData> {
        if let Some(data) = &self.data {
            return data.clone();
        }
        #[cfg(test)]
        {
            self.scans += 1;
        }
        let store = self.store.read(cx);
        let catalog = store.catalog.clone();
        let rows = scopes(&catalog);
        let counts = ScopeCounts::of(store);
        let tallies = rows
            .iter()
            .map(|row| (counts.get(&row.scope), counts.locked(&row.scope)))
            .collect();
        self.counted = store.iter().map(|i| (i.id, Counted::of(i))).collect();
        self.store_seen = store.revision();
        let data = std::rc::Rc::new(SidebarData {
            catalog,
            rows,
            tallies,
            trackers: store.trackers.clone(),
            open_tracker: store.tracker,
        });
        self.data = Some(data.clone());
        data
    }

    /// The active row: the picked row while it still shows the active
    /// scope, or else the first row that does.
    fn active_row(&self, rows: &[Row]) -> Option<usize> {
        if rows
            .get(self.cursor)
            .is_some_and(|r| r.scope == self.active)
        {
            return Some(self.cursor);
        }
        rows.iter().position(|r| r.scope == self.active)
    }

    /// Moves `step` rows down, or up for a negative step, and returns the
    /// scope of the new row. It stops at the first and the last row.
    pub fn step(&mut self, step: isize, cx: &App) -> Scope {
        let rows = scopes(&self.store.read(cx).catalog);
        let at = self.active_row(&rows).unwrap_or(0);
        self.cursor = at.saturating_add_signed(step).min(rows.len() - 1);
        rows[self.cursor].scope.clone()
    }

    /// Moves to the last row and returns its scope.
    pub fn last(&mut self, cx: &App) -> Scope {
        let rows = scopes(&self.store.read(cx).catalog);
        self.cursor = rows.len().saturating_sub(1);
        rows.last().map_or(Scope::All, |r| r.scope.clone())
    }
}

/// The counts on the right of the sidebar rows: open issues per scope,
/// every issue in the Trash, and open questions for the inbox. One pass
/// over the issues finds them all. A saved view counts only once the
/// server sent its issues, so only the active view has a count.
#[derive(Debug, Default)]
pub struct ScopeCounts<'a> {
    all: usize,
    trash: usize,
    teams: HashMap<&'a str, usize>,
    projects: HashMap<&'a str, usize>,
    /// The projects with an open task that an agent holds a claim on.
    locked: HashSet<&'a str>,
    /// The view the server sent, and its open issues.
    view: Option<(&'a str, usize)>,
    questions: usize,
}

impl<'a> ScopeCounts<'a> {
    pub fn of(store: &'a IssueStore) -> Self {
        let view = store.view.as_ref();
        let mut counts = Self {
            view: view.map(|v| (v.name.as_ref(), 0)),
            questions: crate::inbox::open_count(store),
            ..Self::default()
        };
        for issue in store.iter() {
            if issue.deleted {
                counts.trash += 1;
                continue;
            }
            if issue.status.is_closed() {
                continue;
            }
            counts.all += 1;
            *counts.teams.entry(issue.team.as_ref()).or_default() += 1;
            if let Some(project) = &issue.project {
                *counts.projects.entry(project.as_ref()).or_default() += 1;
                if issue.claim.is_some() {
                    counts.locked.insert(project.as_ref());
                }
            }
            if let (Some(members), Some((_, n))) = (view, counts.view.as_mut())
                && members.ids.contains(&issue.id)
            {
                *n += 1;
            }
        }
        counts
    }

    /// Whether a row of `scope` shows the yellow dot: a project in which
    /// an agent holds a claim on an open task.
    pub fn locked(&self, scope: &Scope) -> bool {
        match scope {
            Scope::Project(slug) => self.locked.contains(slug.as_ref()),
            _ => false,
        }
    }

    /// The count a row of `scope` shows. Zero shows nothing.
    pub fn get(&self, scope: &Scope) -> usize {
        match scope {
            Scope::All => self.all,
            Scope::Trash => self.trash,
            Scope::Inbox => self.questions,
            Scope::Team(key) => self.teams.get(key.as_ref()).copied().unwrap_or(0),
            Scope::Project(slug) => self.projects.get(slug.as_ref()).copied().unwrap_or(0),
            Scope::View(name) => match self.view {
                Some((view, n)) if view == name.as_ref() => n,
                _ => 0,
            },
            Scope::Document(_) | Scope::Settings => 0,
        }
    }
}

/// The dot left of a project's icon while an agent works in it. It sits in
/// the row's left padding: 1.5 rem on a project under a team, 0.5 rem on
/// one without.
fn locked_dot(nested: bool, fill: gpui_kit::Hsla) -> impl IntoElement {
    const DOT: f32 = 0.375;
    let left = if nested {
        1.5 - DOT - 0.25
    } else {
        0.5 - DOT - 0.0625
    };
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(gpui_kit::rems(left))
        .flex()
        .items_center()
        .child(
            div()
                .id("locked-dot")
                .test_support()
                .size(gpui_kit::rems(DOT))
                .rounded_full()
                .bg(fill)
                .tooltip(
                    crate::shell::tooltip::Card::text("An agent holds a task in this project")
                        .builder(),
                ),
        )
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::render_count::bump::<Self>();
        self.fill_layer.update(cx, |layer, _| layer.start_frame());
        let fill_layer = self.fill_layer.clone();
        // Kept apart from the store, so a row's motion can take `cx`, and
        // counted again only after a change to what it counts (LLM-345).
        let data = self.data(cx);
        let catalog = &data.catalog;
        let rows = data.rows.clone();
        let tallies = data.tallies.clone();
        let active_row = self.active_row(&rows);
        let sidebar = cx.entity().downgrade();
        let focused = self.focus.is_focused(window);
        // A copy, so a row's motion can take `cx` while the colors stay
        // in reach.
        let theme = &cx.theme().clone();
        let hover = theme.list_hover;
        let pressed = crate::shell::theme::row_pressed(theme);
        let active_hover = crate::shell::theme::selection_hover(theme);
        // Like the list: the active row is a filled background, drawn
        // fainter while focus is elsewhere. No left border: the owner's rule.
        let active_bg = crate::shell::theme::selection_bg(theme, focused);
        // The Locked badge's yellow, so both mean an agent works there.
        let (locked_fill, _) = crate::shell::theme::locked_badge(theme);
        let mut column = v_flex()
            .id("sidebar")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .p_1()
            .gap_px()
            .overflow_y_scroll()
            .text_color(theme.sidebar_foreground);
        // The trackers, when the app knows more than one. A click opens
        // one. They are not scopes, so j / k never switch the data.
        let (trackers, open) = (data.trackers.clone(), data.open_tracker);
        if trackers.len() > 1 {
            column = column.child(
                div()
                    .px_2()
                    .pt_1()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Trackers"),
            );
            for (ix, tracker) in trackers.into_iter().enumerate() {
                let active = ix == open;
                let events = self.events.clone();
                let marker = if tracker.read_only {
                    "read-only"
                } else {
                    "real"
                };
                let frame = crate::shell::motion::fill_under(
                    h_flex().id(ElementId::Name(format!("sidebar-tracker-{ix}").into())),
                    ElementId::Name(format!("sidebar-tracker-fill-{ix}").into()),
                    crate::shell::motion::Fill {
                        rest: active.then_some(active_bg),
                        hover: if active { active_hover } else { hover },
                        pressed,
                    },
                    theme.radius,
                    Some(&fill_layer),
                    window,
                    cx,
                );
                column = column.child(
                    frame
                        .test_support()
                        .w_full()
                        .h_7()
                        .flex_none()
                        .px_2()
                        .gap_2()
                        .text_sm()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::Database)
                                .small()
                                .text_color(theme.muted_foreground),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(tracker.name))
                        .when(tracker.real, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(marker),
                            )
                        })
                        .when(active, |this| {
                            this.child(crate::shell::tooltip::with_card(
                                "sidebar-tracker-open",
                                Icon::new(IconName::Check)
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                                crate::shell::tooltip::Card::text("The open tracker"),
                            ))
                        })
                        .on_click(move |_, window, cx| {
                            events.emit(ViewEvent::SwitchTracker(ix), window, cx);
                        }),
                );
            }
            column = column.child(div().my_2().mx_2().h_px().bg(theme.sidebar_border));
        }
        let mut seen: HashSet<String> = HashSet::new();
        for ((ix, row), (count, locked)) in rows.into_iter().enumerate().zip(tallies) {
            let Row {
                heading,
                scope,
                nested,
            } = row;
            if heading == Some("") {
                column = column.child(div().my_2().mx_2().h_px().bg(theme.sidebar_border));
            } else if let Some(heading) = heading {
                column = column.child(
                    div()
                        .px_2()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(heading),
                );
            }
            let team = matches!(scope, Scope::Team(_));
            let active = active_row == Some(ix);
            let name = scope.name(catalog);
            let events = self.events.clone();
            let sidebar = sidebar.clone();
            let target = scope.clone();
            // A project under a second team gets the row number in its ID,
            // so each row keeps its own hover and press state.
            let setting = scope.to_setting();
            let id = if seen.insert(setting.clone()) {
                format!("sidebar-{setting}")
            } else {
                format!("sidebar-{setting}-{ix}")
            };
            let fill_key = ElementId::Name(format!("{id}-fill").into());
            let row = crate::shell::motion::fill_under(
                h_flex().id(ElementId::Name(id.into())),
                fill_key,
                crate::shell::motion::Fill {
                    rest: active.then_some(active_bg),
                    hover: if active { active_hover } else { hover },
                    pressed,
                },
                theme.radius,
                Some(&fill_layer),
                window,
                cx,
            )
            .test_support()
            .w_full()
            .h_7()
            .flex_none()
            .when(team, |this| this.mt_3())
            .px_2()
            .when(nested, |this| this.pl_6())
            .gap_2()
            .text_sm()
            .when(team, |this| this.font_weight(gpui_kit::FontWeight::MEDIUM))
            .rounded(theme.radius)
            .cursor_pointer()
            // The dot sits in the left padding, so the icon and the
            // name stay where they are.
            .when(locked, |this| {
                this.relative().child(locked_dot(nested, locked_fill))
            })
            .child(match &scope {
                Scope::Project(slug) => crate::shell::icons::project_icon(
                    &catalog.project_style(slug),
                    theme.muted_foreground,
                    cx,
                )
                .small(),
                _ => Icon::new(scope.icon())
                    .small()
                    .text_color(theme.muted_foreground),
            })
            .child(div().flex_1().min_w_0().truncate().child(name))
            .when(count > 0, |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(count.to_string()),
                )
            })
            .on_click(move |_, window, cx| {
                sidebar.update(cx, |sidebar, _| sidebar.cursor = ix).ok();
                events.emit(ViewEvent::PickScope(target.clone()), window, cx);
            });
            // A project row has a menu with the project colors, and drags
            // onto another project row to take its place (LLM-524).
            column = column.child(match &scope {
                Scope::Project(slug) => {
                    with_project_drag(row, slug, catalog, self.events.clone(), cx)
                        .context_menu(color_menu(
                            self.store.clone(),
                            self.events.clone(),
                            slug.clone(),
                        ))
                        .into_any_element()
                }
                _ => row.into_any_element(),
            });
        }
        column
    }
}

/// A project being dragged in the sidebar. It renders as the small card
/// that follows the pointer.
#[derive(Clone)]
pub struct DraggedProject {
    pub slug: SharedString,
    pub name: SharedString,
    /// Its place in the project order, to tell up from down.
    pub ix: usize,
}

impl Render for DraggedProject {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .px_2()
            .py_1()
            .text_sm()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_md()
            .child(self.name.clone())
    }
}

/// Lets a project row be dragged, and take drops of other projects. While
/// a project hovers it, a line on the row's top edge, or its bottom edge
/// when the project comes from above, shows where it lands.
fn with_project_drag<E: StatefulInteractiveElement>(
    row: E,
    slug: &SharedString,
    catalog: &Catalog,
    events: ViewEvents,
    cx: &App,
) -> E {
    let Some(ix) = catalog.projects.iter().position(|p| p.key == *slug) else {
        return row;
    };
    let dragged = DraggedProject {
        slug: slug.clone(),
        name: catalog.projects[ix].name.clone(),
        ix,
    };
    let line = cx.theme().foreground;
    let onto = slug.clone();
    row.on_drag(dragged, |dragged, _, _, cx| {
        gpui_kit::AppContext::new(cx, |_| dragged.clone())
    })
    .drag_over::<DraggedProject>(move |s, dragged, _, _| {
        use std::cmp::Ordering::*;
        match dragged.ix.cmp(&ix) {
            Greater => s.border_t_2().border_color(line),
            Less => s.border_b_2().border_color(line),
            Equal => s,
        }
    })
    .on_drop(move |dragged: &DraggedProject, window, cx| {
        if dragged.slug == onto {
            return;
        }
        events.emit(
            ViewEvent::MoveProject {
                slug: dragged.slug.clone(),
                onto: onto.clone(),
            },
            window,
            cx,
        );
    })
}

/// The right-click menu of a project row: None and the preset colors.
/// The current one has a check. The first item is selected when the menu
/// opens, so Enter picks it. `:project-color` does the same from the
/// palette.
pub fn color_menu(
    store: Entity<IssueStore>,
    events: ViewEvents,
    slug: SharedString,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, window, cx| {
        let current = store.read(cx).catalog.project_style(&slug).color;
        let mut choices: Vec<(SharedString, Option<&'static str>)> = vec![("None".into(), None)];
        choices.extend(
            crate::shell::icons::PROJECT_COLORS
                .iter()
                .map(|(name, hex)| {
                    let mut label = name.to_string();
                    label[..1].make_ascii_uppercase();
                    (label.into(), Some(*hex))
                }),
        );
        let mut menu = menu.label("Project color");
        for (label, hex) in choices {
            let checked = current.as_deref() == hex;
            let events = events.clone();
            let slug = slug.clone();
            menu = menu.item(
                PopupMenuItem::new(label)
                    .icon(crate::shell::icons::swatch_icon(hex, cx))
                    .checked(checked)
                    .on_click(move |_, window, cx| {
                        events.emit(
                            ViewEvent::SetProjectColor {
                                slug: slug.to_string(),
                                color: hex.map(str::to_string),
                            },
                            window,
                            cx,
                        );
                    }),
            );
        }
        // The menu takes focus after this builder runs, and handles keys
        // once it is drawn. Select the first swatch after its first frame,
        // as every list in the app starts on its first item.
        window.on_next_frame(|window, cx| {
            window.dispatch_action(Box::new(gpui_kit::base::actions::SelectDown), cx);
        });
        menu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_round_trip_through_settings() {
        for scope in [
            Scope::All,
            Scope::Team("LLM".into()),
            Scope::Project("demo-app".into()),
            Scope::View("Open bugs".into()),
            Scope::Trash,
        ] {
            assert_eq!(Scope::from_setting(&scope.to_setting()), Some(scope));
        }
        assert_eq!(Scope::from_setting("team:"), None);
        assert_eq!(Scope::from_setting("nope"), None);
    }

    fn rows(catalog: &Catalog) -> Vec<(Option<&'static str>, String, bool)> {
        scopes(catalog)
            .into_iter()
            .map(|r| (r.heading, r.scope.to_setting(), r.nested))
            .collect()
    }

    /// The demo catalog with a second team, WEB. App belongs to both
    /// teams, Server to LLM only, and Docs to no team.
    fn two_teams() -> Catalog {
        let mut catalog = crate::model::demo::catalog();
        let mut web = catalog.team_names[0].clone();
        web.key = "WEB".into();
        web.name = "Website".into();
        catalog.team_names.push(web);
        catalog
            .project_teams
            .insert("app".into(), vec!["LLM".into(), "WEB".into()]);
        catalog.projects.push(crate::model::data::Place {
            key: "docs".into(),
            name: "Docs".into(),
        });
        catalog
    }

    #[test]
    fn each_team_lists_its_projects_under_it() {
        let names = rows(&two_teams());
        assert_eq!(
            names[..8],
            [
                (None, "all".to_string(), false),
                (None, "inbox".into(), false),
                (None, "team:LLM".into(), false),
                (None, "project:app".into(), true),
                (None, "project:server".into(), true),
                (None, "team:WEB".into(), false),
                (None, "project:app".into(), true),
                (Some("Projects"), "project:docs".into(), false),
            ]
        );
        assert!(!names.iter().any(|(h, _, _)| *h == Some("Teams")));
    }

    #[test]
    fn rows_put_the_lone_team_above_its_projects() {
        let names = rows(&crate::model::demo::catalog());
        assert_eq!(
            names,
            vec![
                (None, "all".to_string(), false),
                (None, "inbox".into(), false),
                (None, "team:LLM".into(), false),
                (None, "project:app".into(), true),
                (None, "project:server".into(), true),
                (Some("Views"), "view:Open bugs".into(), false),
                (
                    Some("Documents"),
                    format!("doc:{}", uuid::Uuid::from_u128(901)),
                    false,
                ),
                (Some(""), "settings".into(), false),
                (None, "trash".into(), false),
            ]
        );
    }
}
