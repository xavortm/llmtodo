//! LLM-112: on large lists the filter, the sort and the rows run on a
//! background task. These tests lower the size limit, so a small list takes
//! the background path too.

use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};

use crate::issues::list::{GroupBy, Row, SortKey};
use crate::model::data::{Issue, IssueId};
use crate::shell::workspace::Workspace;

const COUNT: usize = 30;

fn open(cx: &mut TestAppContext, background: bool) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                Default::default(),
                size(px(1200.), px(760.)),
            ))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .expect("open test window")
    });
    let catalog = crate::model::demo::catalog();
    let demo = crate::model::demo::issues(&catalog);
    let issues: Vec<Issue> = (0..COUNT)
        .map(|n| {
            let mut issue = demo[n % demo.len()].clone();
            issue.id = llmtodo_core::IssueId::from_u128(n as u128 + 1);
            issue.key = format!("LLM-{}", n + 1).into();
            issue.title = format!("Task number {n}").into();
            issue.deleted = false;
            issue
        })
        .collect();
    if background {
        view.update(cx, |this, cx| {
            this.list()
                .update(cx, |list, _| list.set_background_from(0));
        });
    }
    view.update(cx, |this, cx| this.load(catalog, issues, cx));
    cx.run_until_parked();
    (handle, view)
}

fn rows(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Vec<Row> {
    cx.read(|cx| view.read(cx).list().read(cx).rows().to_vec())
}

fn selected(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<IssueId> {
    cx.read(|cx| view.read(cx).list().read(cx).selected_issue())
}

fn building(view: &Entity<Workspace>, cx: &mut TestAppContext) -> bool {
    cx.read(|cx| view.read(cx).list().read(cx).building())
}

fn list_do(
    view: &Entity<Workspace>,
    cx: &mut TestAppContext,
    f: impl FnOnce(
        &mut crate::issues::list::IssueList,
        &mut gpui_kit::Context<crate::issues::list::IssueList>,
    ),
) {
    view.update(cx, |this, cx| this.list().update(cx, f));
}

#[gpui_kit::test]
fn both_paths_build_the_same_rows(cx: &mut TestAppContext) {
    let (_, here) = open(cx, false);
    let (_, away) = open(cx, true);
    type Step = fn(
        &mut crate::issues::list::IssueList,
        &mut gpui_kit::Context<crate::issues::list::IssueList>,
    );
    let steps: [Step; 6] = [
        |list, cx| list.sort_by(SortKey::Status, cx),
        |list, cx| list.group_by(GroupBy::Label, cx),
        |list, cx| list.sort_by(SortKey::Status, cx),
        |list, cx| list.set_query("number 1", cx),
        |list, cx| list.group_by(GroupBy::Assignee, cx),
        |list, cx| list.sort_by(SortKey::Added, cx),
    ];
    for step in steps {
        list_do(&here, cx, step);
        list_do(&away, cx, step);
        assert!(building(&away, cx), "the build runs on a background task");
        cx.run_until_parked();
        assert!(!building(&away, cx));
        assert_eq!(rows(&here, cx), rows(&away, cx));
        assert_eq!(selected(&here, cx), selected(&away, cx));
    }
}

#[gpui_kit::test]
fn the_old_rows_stay_until_the_result_lands(cx: &mut TestAppContext) {
    let (_, view) = open(cx, true);
    let before = rows(&view, cx);
    list_do(&view, cx, |list, cx| list.set_query("number 7", cx));
    assert_eq!(rows(&view, cx), before);
    cx.run_until_parked();
    assert_eq!(rows(&view, cx).len(), 1);
}

#[gpui_kit::test]
fn a_newer_keystroke_cancels_an_older_result(cx: &mut TestAppContext) {
    let (_, view) = open(cx, true);
    list_do(&view, cx, |list, cx| list.set_query("number 1", cx));
    list_do(&view, cx, |list, cx| list.set_query("number 2", cx));
    cx.run_until_parked();
    let titles: Vec<String> = cx.read(|cx| {
        let this = view.read(cx);
        (0..this.visible_count(cx))
            .map(|n| this.visible_issue(n, cx).title.to_string())
            .collect()
    });
    assert!(!titles.is_empty());
    assert!(
        titles.iter().all(|t| t.contains("number 2")),
        "only the newest filter lands: {titles:?}"
    );
}

#[gpui_kit::test]
fn the_selection_follows_its_issue_into_the_new_rows(cx: &mut TestAppContext) {
    let (_, view) = open(cx, true);
    list_do(&view, cx, |list, cx| list.select(5, cx));
    let id = selected(&view, cx).unwrap();
    list_do(&view, cx, |list, cx| list.sort_by(SortKey::Key, cx));
    cx.run_until_parked();
    assert_eq!(selected(&view, cx), Some(id));
    let row = cx.read(|cx| view.read(cx).list().read(cx).selected());
    assert_ne!(row, 5, "the reversed order moved the issue");
}

#[gpui_kit::test]
fn a_move_while_the_result_is_on_its_way_counts(cx: &mut TestAppContext) {
    let (_, view) = open(cx, true);
    list_do(&view, cx, |list, cx| list.select(2, cx));
    list_do(&view, cx, |list, cx| list.sort_by(SortKey::Key, cx));
    // The owner presses j on the old rows before the result lands.
    list_do(&view, cx, |list, cx| list.move_down(cx));
    let id = selected(&view, cx).unwrap();
    cx.run_until_parked();
    assert_eq!(selected(&view, cx), Some(id));
}

#[gpui_kit::test]
fn a_live_update_during_a_keystroke_still_selects_the_first_match(cx: &mut TestAppContext) {
    let (_, view) = open(cx, true);
    list_do(&view, cx, |list, cx| list.select(10, cx));
    list_do(&view, cx, |list, cx| list.set_query("number", cx));
    // A live update asks to keep the current row, and cancels the
    // keystroke's build.
    list_do(&view, cx, |list, cx| list.refilter(cx));
    cx.run_until_parked();
    let (row, first) = cx.read(|cx| {
        let list = view.read(cx).list().read(cx);
        let first = list.rows().iter().position(|r| matches!(r, Row::Issue(_)));
        (list.selected(), first)
    });
    assert_eq!(Some(row), first);
}

#[gpui_kit::test]
fn a_burst_of_changes_runs_one_more_build_not_one_each(cx: &mut TestAppContext) {
    let (_, view) = open(cx, true);
    let started =
        |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).list().read(cx).background_builds);
    let before = started(cx);
    for query in ["n", "nu", "num", "numb", "number 2"] {
        list_do(&view, cx, |list, cx| list.set_query(query, cx));
    }
    assert_eq!(started(cx), before + 1, "one build at a time");
    cx.run_until_parked();
    assert_eq!(
        started(cx),
        before + 2,
        "one more for the changes meanwhile"
    );
    assert!(!building(&view, cx));
    let titles: Vec<String> = cx.read(|cx| {
        let this = view.read(cx);
        (0..this.visible_count(cx))
            .map(|n| this.visible_issue(n, cx).title.to_string())
            .collect()
    });
    assert!(!titles.is_empty());
    assert!(titles.iter().all(|t| t.contains("number 2")), "{titles:?}");
}
