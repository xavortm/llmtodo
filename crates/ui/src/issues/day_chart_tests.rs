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
        // It opens on the strip's top edge, not over the strip.
        window.render_frame(cx);
        let list = window.find("day-tasks").bounds();
        let strip = window.find("day-chart").bounds();
        assert_eq!(list.bottom(), strip.top(), "the list touches the strip");
        let day = window.find("day-2026-11-10").bounds();
        assert!(
            (list.center().x - day.center().x).abs() < gpui_kit::px(10.)
                || list.left() == gpui_kit::px(8.),
            "centered on the click, or kept inside the window"
        );
        let keys: Vec<String> = tasks
            .read(cx)
            .rows(cx)
            .into_iter()
            .map(|r| r.key.to_string())
            .collect();
        assert_eq!(keys, ["LLM-101"]);
        // Only created tasks that day, so no line between two sets.
        assert!(window.try_find("day-tasks-divider").is_none());

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

#[test]
fn the_readout_shows_the_hovered_day_or_the_last_seven_days() {
    use crate::issues::day_chart::readout;
    let issues: Vec<Issue> = (1..=9)
        .map(|n| issue(n, "app", (11, 10 + n as i8), Some("20")))
        .collect();
    let out = days(&issues, "app", date(2026, 11, 20));
    // Nov 11 to Nov 20. The last seven days are Nov 14 to Nov 20: six
    // created, and all nine completed on Nov 20.
    assert_eq!(readout(&out, None), ("Last 7 days".to_string(), 6, 9));
    assert_eq!(readout(&out, Some(0)), ("Wed Nov 11".to_string(), 1, 0));
}

/// The dots inside a created bar keep even gaps on the pixel grid: the
/// 5 px inside a 7 px bar at 1x, 1.5x (8 device px) and 2x.
#[test]
fn dots_split_the_room_evenly_at_each_scale() {
    use crate::issues::day_chart::dot_run;
    // (room, dot) -> (first dot's offset, count)
    assert_eq!(dot_run(5, 1), (1, 2), "1x: gap, dot, gap, dot, gap");
    assert_eq!(dot_run(10, 2), (2, 2), "2x: the same, doubled");
    assert_eq!(dot_run(8, 2), (3, 1), "1.5x: one centered dot");
    assert_eq!(dot_run(2, 1), (0, 0), "no room, no dots");
    // A tall bar: the gap above the last row and below the first match.
    let (gap, rows) = dot_run(31, 1);
    assert_eq!((gap, rows), (1, 15));
    assert_eq!(31 - (gap + (rows - 1) * 2 + 1), gap);
}

/// A day with both kinds lists created tasks, a line, then completed ones.
#[gpui_kit::test]
fn a_line_splits_created_from_completed(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        // LLM-103, in App and added on Nov 8, closes on Nov 10, the day
        // LLM-101 was added.
        view.update(cx, |this, cx| {
            this.store().update(cx, |store, cx| {
                let done = issue(103, "app", (11, 8), Some("10"));
                store.modify(IssueId::from_u128(103), |i| {
                    i.status = done.status.clone();
                    i.closed_at = done.closed_at;
                });
                cx.notify();
            });
            this.set_scope(Scope::Project("app".into()), cx)
        });
        window.render_frame(cx);
        window.click("day-2026-11-10", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let rows: Vec<(String, bool)> = view
            .read(cx)
            .day_tasks()
            .read(cx)
            .rows(cx)
            .into_iter()
            .map(|r| (r.key.to_string(), r.completed))
            .collect();
        assert_eq!(
            rows,
            [
                ("LLM-101".to_string(), false),
                ("LLM-103".to_string(), true)
            ]
        );
        let line = window.find("day-tasks-divider").bounds();
        let created = window.find("day-task-LLM-101-0").bounds();
        let completed = window.find("day-task-LLM-103-1").bounds();
        assert!(created.bottom() <= line.top() && line.bottom() <= completed.top());
    })
    .unwrap();
}

/// At 100% the strip keeps its designed sizes: a 10 px slot and 88 px in
/// all. Every zoom step rounds each size to whole px.
#[test]
fn the_strip_sizes_follow_the_zoom_in_whole_px() {
    use crate::issues::day_chart::Metrics;
    let m = Metrics::at(1.);
    assert_eq!((m.slot, m.bar, m.inset, m.height), (10., 7., 1., 88.));
    for &zoom in crate::shell::theme::ZOOM_STEPS {
        let m = Metrics::at(zoom);
        for v in [m.slot, m.bar, m.inset, m.half, m.height] {
            assert_eq!(v, v.round(), "{v} at {zoom}");
        }
        assert!(m.bar < m.slot, "a gap between bars at {zoom}");
    }
    assert_eq!(Metrics::at(2.).height, 175.);
}

/// Zoomed in, the strip grows with its text, so the readout keeps all
/// three lines inside it, and the day list grows too.
#[gpui_kit::test]
fn zoom_grows_the_strip_and_the_day_list(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let measure = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            let strip = window.find("day-chart").bounds();
            let last = window.find("completed").bounds();
            assert!(
                last.bottom() <= strip.bottom(),
                "the readout's last line fits: {last:?} in {strip:?}"
            );
            strip.size.height
        })
        .unwrap()
    };
    cx.update_window(handle, |_, _, cx| {
        view.update(cx, |this, cx| {
            this.set_scope(Scope::Project("app".into()), cx)
        });
    })
    .unwrap();
    let small = measure(cx);
    cx.update(|cx| crate::shell::theme::set_zoom(2., cx));
    let big = measure(cx);
    assert!(big > small * 1.9, "{small:?} grows to {big:?}");

    cx.update_window(handle, |_, window, cx| {
        window.click("day-2026-11-10", cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let list = window.find("day-tasks").bounds();
        assert_eq!(list.size.width, window.rem_size() * 30.);
    })
    .unwrap();
}

/// The strip comes back after any trip away from a project.
#[gpui_kit::test]
fn the_strip_comes_back_after_leaving_the_project(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    for (scope, shown) in [
        (Scope::Project("app".into()), true),
        (Scope::All, false),
        (Scope::Project("app".into()), true),
        (Scope::Project("server".into()), true),
        (Scope::Team("LLM".into()), false),
        (Scope::Project("server".into()), true),
    ] {
        cx.update_window(handle, |_, _, cx| {
            view.update(cx, |this, cx| this.set_scope(scope.clone(), cx));
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let found = window.try_find("day-chart").is_some();
            assert_eq!(found, shown, "{scope:?}");
            if shown {
                let strip = window.find("day-chart").bounds();
                assert!(
                    strip.size.height > gpui_kit::px(40.),
                    "{scope:?}: {strip:?}"
                );
            }
        })
        .unwrap();
    }
}
