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
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
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
    _store: Subscription,
}

impl Sidebar {
    pub fn new(store: Entity<IssueStore>, events: ViewEvents, cx: &mut Context<Self>) -> Self {
        // The workspace draws the sidebar as a cached view, so it redraws
        // only when it is notified. Everything it shows comes from the store.
        let subscription = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            _store: subscription,
            store,
            events,
            focus: cx.focus_handle(),
            active: Scope::All,
            cursor: 0,
        }
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
        // Borrowed for the whole render: the store does not change during it.
        let store = self.store.read(cx);
        let catalog = &store.catalog;
        let rows = scopes(catalog);
        let counts = ScopeCounts::of(store);
        let active_row = self.active_row(&rows);
        let sidebar = cx.entity().downgrade();
        let focused = self.focus.is_focused(window);
        let theme = cx.theme();
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
        let (trackers, open) = (store.trackers.clone(), store.tracker);
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
                column = column.child(
                    h_flex()
                        .id(ElementId::Name(format!("sidebar-tracker-{ix}").into()))
                        .test_support()
                        .w_full()
                        .h_7()
                        .flex_none()
                        .px_2()
                        .gap_2()
                        .text_sm()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .when(active, |this| this.bg(active_bg))
                        .hover(move |s| s.bg(if active { active_hover } else { hover }))
                        .active(move |s| s.bg(pressed))
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
        for (ix, row) in rows.into_iter().enumerate() {
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
            let count = counts.get(&scope);
            let locked = counts.locked(&scope);
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
            let row = h_flex()
                .id(ElementId::Name(id.into()))
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
                .when(active, |this| this.bg(active_bg))
                .hover(move |s| s.bg(if active { active_hover } else { hover }))
                .active(move |s| s.bg(pressed))
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
            // A project row has a menu with the project colors.
            column = column.child(match &scope {
                Scope::Project(slug) => row
                    .context_menu(color_menu(
                        self.store.clone(),
                        self.events.clone(),
                        slug.clone(),
                    ))
                    .into_any_element(),
                _ => row.into_any_element(),
            });
        }
        column
    }
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
