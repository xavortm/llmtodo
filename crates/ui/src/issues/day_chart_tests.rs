//! LLM-560: the per-day strip under a project's list, and the day's tasks.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};
use jiff::civil::date;

use crate::issues::day_chart::days;
use crate::model::data::{Date, Issue, IssueId, Status, StatusType};
use crate::model::scope::Scope;
use crate::shell::workspace::Workspace;

fn issue(n: u128, project: &str, added: (i8, i8), done: Option<&str>) -> Issue {
    let mut issue = crate::model::demo::issues(&crate::model::demo::catalog())
        .into_iter()
        .next()
        .unwrap();
    issue.id = IssueId::from_u128(n);
    issue.project = Some(project.into());
    issue.added = Date {
        year: 2026,
        month: added.0,
        day: added.1,
        hour: 12,
        minute: 0,
    };
    issue.status = Status::new("Todo", StatusType::Unstarted, 0);
    if let Some(at) = done {
        issue.status = Status::new("Done", StatusType::Completed, 0);
        issue.closed_at = Some(
            date(2026, 11, at.parse().unwrap())
                .at(12, 0, 0, 0)
                .to_zoned(jiff::tz::TimeZone::system())
                .unwrap()
                .timestamp(),
        );
    }
    issue
}

#[test]
fn days_count_created_and_completed_from_the_first_task_to_today() {
    let mut canceled = issue(4, "app", (11, 21), Some("22"));
    canceled.status = Status::new("Canceled", StatusType::Canceled, 0);
    let mut trashed = issue(5, "app", (11, 20), None);
    trashed.deleted = true;
    let issues = [
        issue(1, "app", (11, 21), Some("23")),
        issue(2, "app", (11, 21), None),
        issue(3, "server", (11, 1), None),
        canceled,
        trashed,
    ];
    let out = days(&issues, "app", date(2026, 11, 24));
    let shape: Vec<_> = out
        .iter()
        .map(|d| (d.date.day(), d.created.len(), d.completed.len()))
        .collect();
    // From Nov 21, the first App task. A canceled task is not completed,
    // and the trashed one and the Server one count nowhere.
    assert_eq!(shape, [(21, 3, 0), (22, 0, 0), (23, 0, 1), (24, 0, 0)]);
    assert_eq!(out[2].completed, [IssueId::from_u128(1)]);
    assert!(days(&issues, "nothing", date(2026, 11, 24)).is_empty());
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .unwrap()
    })
}

#[gpui_kit::test]
fn a_project_shows_the_strip_and_a_day_click_lists_its_tasks(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("day-chart").is_none(),
            "All issues has no strip"
        );
        view.update(cx, |this, cx| {
            this.set_scope(Scope::Project("app".into()), cx)
        });
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(window.try_find("day-chart").is_some(), "a project has one");

        // The demo adds LLM-101, in App, on Nov 10.
        window.click("day-2026-11-10", cx);
    })
    .unwrap();
    // The click's event reaches the workspace when the update ends.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let tasks = view.read(cx).day_tasks().clone();
        assert!(tasks.read(cx).is_open());
        let keys: Vec<String> = tasks
            .read(cx)
            .rows(cx)
            .into_iter()
            .map(|r| r.key.to_string())
            .collect();
        assert_eq!(keys, ["LLM-101"]);

        window.press("enter", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(!view.read(cx).day_tasks().read(cx).is_open());
        let open = view
            .read(cx)
            .detail(cx)
            .map(|d| view.read(cx).issue(d.issue, cx).key.to_string());
        assert_eq!(open.as_deref(), Some("LLM-101"));
    })
    .unwrap();
}
