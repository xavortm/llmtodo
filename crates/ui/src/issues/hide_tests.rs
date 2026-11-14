//! LLM-214: "Hide after" takes 0, and a closed task that an open task waits
//! for stays in the list at every value.

use std::collections::HashSet;

use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowOptions};

use crate::issues::list::{GroupBy, SortKey};
use crate::issues::rows::{self, RowsInput};
use crate::model::backend::Event;
use crate::model::data::{Issue, IssueId, hide_cutoff};
use crate::model::scope::Scope;
use crate::shell::workspace::Workspace;

const DAY: jiff::SignedDuration = jiff::SignedDuration::from_hours(24);

/// The demo tasks, all open, and the first Done status.
fn open_tasks() -> (
    crate::model::data::Catalog,
    Vec<Issue>,
    crate::model::data::Status,
) {
    let catalog = crate::model::demo::catalog();
    let mut issues = crate::model::demo::issues(&catalog);
    let statuses = catalog.all_statuses();
    let todo = statuses.iter().find(|s| !s.is_closed()).unwrap().clone();
    let done = statuses.iter().find(|s| s.is_done()).unwrap().clone();
    for issue in &mut issues {
        issue.status = todo.clone();
        issue.closed_at = None;
        issue.deleted = false;
        issue.has_open_dependents = false;
    }
    (catalog, issues, done)
}

/// The keys the list keeps with "Hide after" at `days`, and the next
/// moment a shown task hides.
fn kept(
    catalog: &crate::model::data::Catalog,
    issues: &[Issue],
    days: u32,
    now: jiff::Timestamp,
) -> (Vec<IssueId>, Option<jiff::Timestamp>) {
    let statuses = catalog.all_statuses();
    let collapsed = HashSet::new();
    let query = crate::issues::filter::Filter::parse("").resolve(catalog);
    let input = RowsInput {
        scope: &Scope::All,
        view: None,
        query: &query,
        cutoff: Some(hide_cutoff(days, now)),
        now,
        sort: (SortKey::Key, false),
        group: GroupBy::None,
        collapsed: &collapsed,
        statuses: &statuses,
        labels: &catalog.labels,
        projects: &catalog.projects,
    };
    let built = rows::build(&input, issues.iter());
    (built.visible, built.next_hide)
}

#[test]
fn zero_days_hides_every_closed_task_at_once() {
    let (catalog, mut issues, done) = open_tasks();
    let now = jiff::Timestamp::now();
    // Closed a second ago, closed with no close time, and closed in a
    // moment that the app's clock has not reached yet.
    issues[0].status = done.clone();
    issues[0].closed_at = Some(now - jiff::SignedDuration::from_secs(1));
    issues[1].status = done.clone();
    issues[2].status = done;
    issues[2].closed_at = Some(now + jiff::SignedDuration::from_secs(5));
    let (visible, next) = kept(&catalog, &issues, 0, now);
    for issue in &issues[..3] {
        assert!(!visible.contains(&issue.id), "{} shows", issue.key);
    }
    assert_eq!(visible.len(), issues.len() - 3);
    assert_eq!(next, None, "nothing is left to hide later");
}

#[test]
fn a_closed_task_an_open_task_waits_for_stays_at_every_value() {
    let (catalog, mut issues, done) = open_tasks();
    let now = jiff::Timestamp::now();
    // A done step that a later, open step waits for, closed long ago.
    issues[0].status = done.clone();
    issues[0].closed_at = Some(now - DAY * 30);
    issues[0].has_open_dependents = true;
    // A done task that only closed tasks wait for: the server leaves the
    // flag off, so it hides like any closed task.
    issues[1].status = done;
    issues[1].closed_at = Some(now - DAY * 30);
    for days in [0, 7] {
        let (visible, next) = kept(&catalog, &issues, days, now);
        assert!(visible.contains(&issues[0].id), "held at {days}");
        assert!(!visible.contains(&issues[1].id), "hidden at {days}");
        // A held task sets no timer: only a new flag can hide it.
        assert_eq!(next, None, "no timer at {days}");
    }
    // When the chain closes, the server clears the flag and the task hides.
    issues[0].has_open_dependents = false;
    let (visible, _) = kept(&catalog, &issues, 0, now);
    assert!(!visible.contains(&issues[0].id));
}

fn open(cx: &mut TestAppContext) -> Entity<Workspace> {
    cx.update(crate::init);
    let (_, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    view
}

fn shows(view: &Entity<Workspace>, id: IssueId, cx: &TestAppContext) -> bool {
    cx.read(|cx| view.read(cx).list().read(cx).visible().contains(&id))
}

/// The server's copy of a closed task, with only the fields the
/// dependents refresh reads.
fn dependent(id: IssueId, key: &str, held: bool) -> llmtodo_client::types::Issue {
    serde_json::from_value(serde_json::json!({
        "id": id, "key": key, "team": "LLM", "title": "Step",
        "status": {"id": "01a1c453-1260-7779-be1c-619faeb9d421", "name": "Done", "type": "completed", "color": "#fff"},
        "priority": "none", "assignee": null, "labels": [], "project": null,
        "milestone": null, "parent": null, "wanted_executor": {},
        "description_version": 1, "claim": null, "created_by": null,
        "created_at": "2026-11-10T10:00:00Z", "updated_at": "2026-11-10T10:00:00Z",
        "completed_at": "2026-11-10T10:00:00Z", "deleted_at": null,
        "has_open_dependents": held
    }))
    .unwrap()
}

#[gpui_kit::test]
fn a_live_refresh_of_the_flag_hides_and_shows_the_task(cx: &mut TestAppContext) {
    let view = open(cx);
    view.update(cx, |this, cx| {
        this.set_hide_closed(true, cx);
        this.set_hide_closed_days(0, cx);
    });
    // A done step that an open step waits for.
    let (id, key) = view.update(cx, |this, cx| {
        let mut issue = this.visible_issue(0, cx).clone();
        let done = this.store().read(cx).catalog.all_statuses();
        issue.status = done.into_iter().find(|s| s.is_done()).unwrap();
        issue.closed_at = Some(jiff::Timestamp::now() - DAY);
        issue.has_open_dependents = true;
        let out = (issue.id, issue.key.to_string());
        this.upsert_issue(issue, cx);
        out
    });
    cx.run_until_parked();
    assert!(shows(&view, id, cx), "an open step still waits for it");

    // The last open step closes: the refresh clears the flag.
    view.update(cx, |this, cx| {
        this.apply_event(Event::Dependents(vec![dependent(id, &key, false)]), cx)
    });
    cx.run_until_parked();
    assert!(!shows(&view, id, cx), "the whole chain is closed");

    // A step opens again: the refresh sets the flag.
    view.update(cx, |this, cx| {
        this.apply_event(Event::Dependents(vec![dependent(id, &key, true)]), cx)
    });
    cx.run_until_parked();
    assert!(shows(&view, id, cx), "an open step waits for it again");
}
