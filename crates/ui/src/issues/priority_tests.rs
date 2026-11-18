//! UI integration tests for priority (LLM-301): the column, the menus,
//! the `p` key, `:priority`, sorting, grouping and the filter.
//!
//! The demo tasks cycle through the levels by six: LLM-101 is urgent,
//! LLM-102 high, LLM-103 medium, LLM-104 low, LLM-105 and LLM-106 have
//! none, and LLM-107 starts again at urgent.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

use crate::issues::columns::{self, Column, Demand};
use crate::issues::filter::Filter;
use crate::issues::filter_menu::FilterPanel;
use crate::issues::list::Row;
use crate::model::data::Priority;
use crate::shell::view_events::{IssueAction, ViewEvent};
use crate::shell::workspace::Workspace;

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .unwrap()
    })
}

fn run(handle: AnyWindowHandle, view: &Entity<Workspace>, command: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command(command, window, cx))
    })
    .unwrap();
}

fn press(handle: AnyWindowHandle, keys: &[&str], cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for key in keys {
            window.press(key, cx);
        }
    })
    .unwrap();
}

fn priority_of(view: &Entity<Workspace>, key: &str, cx: &TestAppContext) -> Priority {
    cx.read(|cx| view.read(cx).issue_by_key(key, cx).priority)
}

fn headers(view: &Entity<Workspace>, cx: &TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        view.read(cx)
            .rows(cx)
            .iter()
            .filter_map(|r| match r {
                Row::Header { title, .. } => Some(title.to_string()),
                Row::Issue(_) => None,
            })
            .collect()
    })
}

fn visible_keys(view: &Entity<Workspace>, cx: &TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        let this = view.read(cx);
        (0..this.visible_count(cx))
            .map(|n| this.visible_issue(n, cx).key.to_string())
            .collect()
    })
}

#[gpui_kit::test]
fn the_demo_data_shows_every_level(cx: &mut TestAppContext) {
    let (_, view) = open(cx);
    assert_eq!(priority_of(&view, "LLM-101", cx), Priority::Urgent);
    assert_eq!(priority_of(&view, "LLM-102", cx), Priority::High);
    assert_eq!(priority_of(&view, "LLM-103", cx), Priority::Medium);
    assert_eq!(priority_of(&view, "LLM-104", cx), Priority::Low);
    assert_eq!(priority_of(&view, "LLM-105", cx), Priority::None);
}

#[gpui_kit::test]
fn the_palette_sets_a_priority_on_the_selected_task(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // LLM-101 is selected and urgent.
    run(handle, &view, "priority low", cx);
    assert_eq!(priority_of(&view, "LLM-101", cx), Priority::Low);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "LLM-101 is Low"));
    run(handle, &view, "priority none", cx);
    assert_eq!(priority_of(&view, "LLM-101", cx), Priority::None);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "LLM-101 has no priority"));
    run(handle, &view, "priority soon", cx);
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).message(),
            "Priority is urgent, high, medium, low or none, not soon"
        );
    });
}

#[gpui_kit::test]
fn p_opens_the_palette_on_the_priorities_and_enter_sets_one(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // LLM-104 is low and no agent holds it.
    press(handle, &["j", "j", "j", "p"], cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(view.read(cx).palette_open(cx));
        assert_eq!(view.read(cx).palette_typed(cx), "priority ");
        // The first match, urgent, is selected. Ctrl-N moves to high.
        assert_eq!(view.read(cx).palette_selected(cx), 0);
        window.press("ctrl-n", cx);
        window.press("enter", cx);
    })
    .unwrap();
    assert_eq!(priority_of(&view, "LLM-104", cx), Priority::High);
    cx.read(|cx| assert!(!view.read(cx).palette_open(cx)));
}

#[gpui_kit::test]
fn a_priority_applies_to_every_marked_task(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // LLM-104 to LLM-106. No agent holds them.
    press(handle, &["j", "j", "j", "x", "x", "x"], cx);
    run(handle, &view, "priority urgent", cx);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "3 tasks now urgent"));
    for key in ["LLM-104", "LLM-105", "LLM-106"] {
        assert_eq!(priority_of(&view, key, cx), Priority::Urgent, "{key}");
    }
    assert_eq!(priority_of(&view, "LLM-103", cx), Priority::Medium);
}

#[gpui_kit::test]
fn the_right_click_menu_sets_the_priority_on_the_marked_tasks(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    press(handle, &["j", "j", "j", "x", "x"], cx);
    let id = cx.read(|cx| view.read(cx).issue_by_key("LLM-104", cx).id);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            this.on_view_event(
                ViewEvent::OnIssue(id, IssueAction::SetPriority(Priority::Medium)),
                window,
                cx,
            );
        });
    })
    .unwrap();
    assert_eq!(priority_of(&view, "LLM-104", cx), Priority::Medium);
    assert_eq!(priority_of(&view, "LLM-105", cx), Priority::Medium);
}

#[gpui_kit::test]
fn sort_by_priority_puts_urgent_first_and_none_last(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "sort priority", cx);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Sorted by priority"));
    let keys = visible_keys(&view, cx);
    assert_eq!(&keys[..4], ["LLM-101", "LLM-107", "LLM-113", "LLM-119"]);
    let last = cx.read(|cx| {
        let this = view.read(cx);
        this.visible_issue(this.visible_count(cx) - 1, cx).priority
    });
    assert_eq!(last, Priority::None);
}

#[gpui_kit::test]
fn group_by_priority_lists_the_levels_in_order(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "group priority", cx);
    assert_eq!(
        headers(&view, cx),
        ["Urgent", "High", "Medium", "Low", "No priority"]
    );
    // A new priority moves the task to its group at once.
    run(handle, &view, "open LLM-105", cx);
    run(handle, &view, "priority urgent", cx);
    let urgent = cx.read(|cx| {
        let this = view.read(cx);
        let rows = this.rows(cx);
        let end = rows
            .iter()
            .skip(1)
            .position(|r| matches!(r, Row::Header { .. }))
            .unwrap();
        rows[1..=end]
            .iter()
            .filter_map(|r| match r {
                Row::Issue(id) => Some(this.issue(*id, cx).key.to_string()),
                Row::Header { .. } => None,
            })
            .collect::<Vec<_>>()
    });
    assert!(urgent.contains(&"LLM-105".to_string()), "{urgent:?}");
}

#[gpui_kit::test]
fn priority_filter_words_keep_any_of_their_levels(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "filter priority:urgent", cx);
    assert_eq!(
        visible_keys(&view, cx),
        ["LLM-101", "LLM-107", "LLM-113", "LLM-119"]
    );
    run(handle, &view, "filter priority:urgent priority:low", cx);
    assert_eq!(visible_keys(&view, cx).len(), 7);
    run(handle, &view, "filter priority:none", cx);
    assert_eq!(visible_keys(&view, cx).len(), 6);
    // A word that names no level keeps nothing.
    run(handle, &view, "filter priority:soon", cx);
    assert!(visible_keys(&view, cx).is_empty());
}

#[test]
fn the_filter_panel_toggles_priority_words() {
    let catalog = crate::model::demo::catalog();
    let issues = crate::model::demo::issues(&catalog);
    let panel = FilterPanel::new("fix", &catalog, issues.iter());
    let names: Vec<_> = panel
        .priorities
        .iter()
        .map(|e| e.label.to_string())
        .collect();
    assert_eq!(names, ["Urgent", "High", "Medium", "Low", "No priority"]);
    assert_eq!(panel.priorities[1].text, "fix priority:high");
    assert!(!panel.priorities[1].checked);

    let panel = FilterPanel::new("fix priority:high", &catalog, issues.iter());
    assert!(panel.priorities[1].checked);
    assert_eq!(
        panel.priorities[1].text, "fix",
        "a second click turns it off"
    );
    assert_eq!(
        panel.priorities[0].text,
        "fix priority:high priority:urgent"
    );
    assert_eq!(Filter::parse("priority:high").word_count(), 1);
}

#[test]
fn a_saved_view_keeps_one_priority() {
    let catalog = crate::model::demo::catalog();
    let scope = crate::model::scope::Scope::All;
    let query = Filter::parse("priority:high")
        .to_query(&scope, &catalog)
        .unwrap();
    assert_eq!(query.priority, Some(Priority::High));
    assert!(
        Filter::parse("priority:high priority:low")
            .to_query(&scope, &catalog)
            .is_err()
    );
    assert!(
        Filter::parse("priority:soon")
            .to_query(&scope, &catalog)
            .is_err()
    );
}

#[test]
fn the_priority_column_hides_after_waiting_on_and_before_status() {
    let rem = gpui_kit::px(16.);
    let demand = Demand::titles(30);
    let all = columns::visible(None, rem, &[], &demand);
    let at = |c: Column| all.iter().position(|x| *x == c).unwrap();
    assert!(at(Column::Priority) < at(Column::Status), "{all:?}");
    // Narrow the list until the priority column hides. The waiting on
    // column has hidden before it, and the status still shows.
    let mut width = 1200.;
    let shown = loop {
        let shown = columns::visible(Some(gpui_kit::px(width)), rem, &[], &demand);
        if !shown.contains(&Column::Priority) {
            break shown;
        }
        width -= 4.;
        assert!(width > 0.);
    };
    assert!(!shown.contains(&Column::Waiting));
    assert!(shown.contains(&Column::Status));
    assert!(shown.contains(&Column::Order) || !all.contains(&Column::Order));
    assert_eq!(
        Column::hideable_from_setting("priority"),
        Some(Column::Priority)
    );
}
