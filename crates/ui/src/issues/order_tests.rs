//! LLM-208: "group by parent" shows each parent's sub-issues in their
//! order, with a line and one dot per step in a narrow column.

use std::collections::HashSet;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};

use crate::issues::columns::Column;
use crate::issues::list::{GroupBy, Row, SortKey};
use crate::issues::rows::{self, BuiltRows, OrderMark, RowsInput};
use crate::model::backend::{Backend, Request, Write};
use crate::model::data::{Catalog, Issue, IssueId};
use crate::model::scope::Scope;
use crate::shell::workspace::Workspace;

/// `(number, parent, step, done)` for each task, as LLM-<number>.
///
/// LLM-1 is an epic. Step 1 is LLM-2, done. Step 2 is LLM-3, LLM-4 and
/// LLM-5, with LLM-4 done. Step 3 is LLM-7 and step 4 is LLM-6, so the
/// steps do not follow the keys. LLM-8 has no step. LLM-9 is a second
/// parent with one sub-issue, and LLM-11 stands alone.
const TASKS: &[(u32, Option<u32>, Option<u32>, bool)] = &[
    (1, None, None, false),
    (2, Some(1), Some(1), true),
    (3, Some(1), Some(2), false),
    (4, Some(1), Some(2), true),
    (5, Some(1), Some(2), false),
    (6, Some(1), Some(4), false),
    (7, Some(1), Some(3), false),
    (8, Some(1), None, false),
    (9, None, None, false),
    (10, Some(9), None, false),
    (11, None, None, false),
];

fn issues(catalog: &Catalog) -> Vec<Issue> {
    let demo = crate::model::demo::issues(catalog);
    let open = catalog.find_status("LLM", "Todo").unwrap();
    let done = catalog.find_status("LLM", "Done").unwrap();
    TASKS
        .iter()
        .map(|&(n, parent, step, closed)| {
            let mut issue = demo[0].clone();
            issue.id = IssueId::from_u128(n as u128);
            issue.key = format!("LLM-{n}").into();
            issue.title = format!("Task {n}").into();
            issue.parent = parent.map(|p| format!("LLM-{p}").into());
            issue.order_step = step;
            issue.status = if closed { done.clone() } else { open.clone() };
            issue.closed_at = None;
            issue.claim = None;
            issue.analyzing = None;
            issue.deleted = false;
            issue
        })
        .collect()
}

fn build_with(filter: &str, sort: (SortKey, bool), collapsed: &[&str]) -> BuiltRows {
    let catalog = crate::model::demo::catalog();
    let issues = issues(&catalog);
    let statuses = catalog.all_statuses();
    let collapsed: HashSet<String> = collapsed.iter().map(|c| c.to_string()).collect();
    let query = crate::issues::filter::Filter::parse(filter).resolve(&catalog);
    let input = RowsInput {
        scope: &Scope::All,
        view: None,
        query: &query,
        cutoff: None,
        now: jiff::Timestamp::now(),
        sort,
        group: GroupBy::Parent,
        collapsed: &collapsed,
        statuses: &statuses,
        labels: &catalog.labels,
    };
    rows::build(&input, issues.iter())
}

/// The rows as text: `# LLM-1 Task 1` for a header, `LLM-2` for a task.
fn names(built: &BuiltRows) -> Vec<String> {
    built
        .rows
        .iter()
        .map(|r| match r {
            Row::Header {
                title, subtitle, ..
            } => match subtitle {
                Some(s) => format!("# {title} {s}"),
                None => format!("# {title}"),
            },
            Row::Issue(id) => format!("LLM-{}", id.0.as_u128()),
        })
        .collect()
}

fn mark(dot: Option<bool>, up: bool, down: bool) -> Option<OrderMark> {
    Some(OrderMark { dot, up, down })
}

#[test]
fn each_parent_is_a_group_with_its_ordered_tasks_first() {
    let built = build_with("", (SortKey::Key, false), &[]);
    assert_eq!(
        names(&built),
        [
            "# LLM-1 Task 1",
            "LLM-2",
            "LLM-3",
            "LLM-4",
            "LLM-5",
            "LLM-7",
            "LLM-6",
            "LLM-8",
            "# LLM-9 Task 9",
            "LLM-10",
            "# No parent",
            "LLM-1",
            "LLM-9",
            "LLM-11",
        ]
    );
}

#[test]
fn a_dot_starts_each_step_and_the_line_runs_from_first_to_last() {
    let built = build_with("", (SortKey::Key, false), &[]);
    assert_eq!(built.order.len(), built.rows.len());
    assert_eq!(
        built.order[..9],
        [
            None,
            // Step 1 is done: a filled dot, and the line starts here.
            mark(Some(true), false, true),
            // Step 2 has open tasks: a hollow dot, then two rows on the line.
            mark(Some(false), true, true),
            mark(None, true, true),
            mark(None, true, true),
            mark(Some(false), true, true),
            // The last ordered row ends the line.
            mark(Some(false), true, false),
            // No step: nothing.
            None,
            None,
        ]
    );
    // Groups with no steps draw nothing.
    assert!(built.order[9..].iter().all(Option::is_none));
}

#[test]
fn a_reversed_sort_keeps_the_steps_in_order() {
    let built = build_with("", (SortKey::Key, true), &[]);
    assert_eq!(
        names(&built)[..8],
        [
            "# LLM-1 Task 1",
            "LLM-2",
            "LLM-5",
            "LLM-4",
            "LLM-3",
            "LLM-7",
            "LLM-6",
            "LLM-8",
        ]
    );
}

#[test]
fn a_dot_counts_the_tasks_the_filter_hides() {
    // Only the done tasks show. LLM-4 now starts step 2 on the list, but
    // LLM-3 and LLM-5 are still open, so its dot stays hollow.
    let built = build_with("status:done", (SortKey::Key, false), &[]);
    assert_eq!(names(&built), ["# LLM-1 Task 1", "LLM-2", "LLM-4"]);
    assert_eq!(
        built.order,
        [
            None,
            mark(Some(true), false, true),
            mark(Some(false), true, false)
        ]
    );
}

#[test]
fn a_collapsed_parent_group_keeps_its_header() {
    let built = build_with("", (SortKey::Key, false), &["parent:LLM-1"]);
    assert_eq!(
        names(&built)[..3],
        ["# LLM-1 Task 1", "# LLM-9 Task 9", "LLM-10"]
    );
    assert!(matches!(
        built.rows[0],
        Row::Header {
            collapsed: true,
            count: 7,
            ..
        }
    ));
    assert_eq!(built.order.len(), built.rows.len());
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>, Requests) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                Default::default(),
                size(px(1400.), px(760.)),
            ))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .expect("open test window")
    });
    let catalog = crate::model::demo::catalog();
    let issues = issues(&catalog);
    view.update(cx, |this, cx| this.load(catalog, issues, cx));
    let (backend, requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    view.update(cx, |this, cx| {
        this.list()
            .update(cx, |list, cx| list.group_by(GroupBy::Parent, cx));
    });
    cx.run_until_parked();
    (handle, view, requests)
}

type Requests = tokio::sync::mpsc::UnboundedReceiver<Request>;

fn row_of(view: &Entity<Workspace>, n: u128, cx: &mut TestAppContext) -> usize {
    cx.read(|cx| {
        view.read(cx)
            .list()
            .read(cx)
            .row_of(IssueId::from_u128(n))
            .unwrap()
    })
}

fn header_row(view: &Entity<Workspace>, title: &str, cx: &mut TestAppContext) -> usize {
    cx.read(|cx| {
        view.read(cx)
            .rows(cx)
            .iter()
            .position(|r| matches!(r, Row::Header { title: t, .. } if t.as_ref() == title))
            .unwrap()
    })
}

#[gpui_kit::test]
fn the_order_column_shows_only_under_group_by_parent(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(
            window
                .within("column-header")
                .try_find(Column::Order.id())
                .is_some()
        );
        // LLM-2 is the first row of the done step: a filled dot.
        let first = window
            .within(("row", 1usize))
            .find(Column::Order.id())
            .bounds();
        let dot = window
            .within(("row", 1usize))
            .find("order-dot-done")
            .bounds();
        assert!(first.contains(&dot.center()), "the dot sits in its cell");
        // LLM-3 starts step 2, and LLM-4 sits on the line with no dot.
        assert!(
            window
                .within(("row", 2usize))
                .try_find("order-dot")
                .is_some()
        );
        assert!(
            window
                .within(("row", 3usize))
                .try_find("order-mark")
                .is_some()
        );
        assert!(
            window
                .within(("row", 3usize))
                .try_find("order-dot")
                .is_none()
        );
        // LLM-8 has no step.
        assert!(
            window
                .within(("row", 7usize))
                .try_find("order-mark")
                .is_none()
        );
        // Rows keep their height.
        let task = window.find(("row", 1usize)).bounds().size.height;
        let header = window.find(("row", 0usize)).bounds().size.height;
        assert_eq!(task, header);
    })
    .unwrap();
    view.update(cx, |this, cx| {
        this.list()
            .update(cx, |list, cx| list.group_by(GroupBy::Status, cx));
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .within("column-header")
                .try_find(Column::Order.id())
                .is_none()
        );
        assert!(window.try_find("order-mark").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn keys_move_through_parent_groups_and_close_them(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("g", cx);
        window.press("g", cx);
        assert_eq!(view.read(cx).selected(cx), 0);
        // Enter closes the first parent group, and j moves to the next one.
        window.press("enter", cx);
        window.render_frame(cx);
        window.press("j", cx);
        let rows = view.read(cx).rows(cx).to_vec();
        assert!(matches!(
            &rows[view.read(cx).selected(cx)],
            Row::Header { title, .. } if title.as_ref() == "LLM-9"
        ));
        window.press("j", cx);
        assert_eq!(
            view.read(cx).list().read(cx).selected_issue(),
            Some(IssueId::from_u128(10))
        );
    })
    .unwrap();
    let saved = cx.read(|cx| view.read(cx).current_settings(cx));
    assert_eq!(saved.group, "parent");
    assert_eq!(saved.collapsed_groups, ["parent:LLM-1"]);
}

/// The parent write the workspace sent for `key`, if any.
fn parent_writes(requests: &mut Requests) -> Vec<(String, llmtodo_client::types::Change<String>)> {
    let mut out = Vec::new();
    while let Ok(request) = requests.try_recv() {
        if let Request::Write {
            key,
            write: Write::Update(update),
            ..
        } = request
        {
            out.push((key, update.parent.clone()));
        }
    }
    out
}

#[gpui_kit::test]
fn a_drop_on_a_parent_group_sets_that_parent(cx: &mut TestAppContext) {
    use llmtodo_client::types::Change;
    let (handle, view, mut requests) = open(cx);
    // LLM-11 has no parent. Drop it on the LLM-9 header.
    let (from, to) = (row_of(&view, 11, cx), header_row(&view, "LLM-9", cx));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.drag_to(("row", from), ("row", to), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let parent = cx.read(|cx| view.read(cx).issue_by_key("LLM-11", cx).parent.clone());
    assert_eq!(parent.as_deref(), Some("LLM-9"));
    assert_eq!(
        parent_writes(&mut requests),
        [("LLM-11".to_string(), Change::Set("LLM-9".to_string()))]
    );
    let message = cx.read(|cx| view.read(cx).message().to_string());
    assert_eq!(message, "LLM-11 is now under LLM-9");

    // LLM-3 leaves the epic: a drop on "No parent" clears its parent and
    // its step.
    let (from, to) = (row_of(&view, 3, cx), header_row(&view, rows::NO_PARENT, cx));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.drag_to(("row", from), ("row", to), cx);
    })
    .unwrap();
    cx.run_until_parked();
    let moved = cx.read(|cx| view.read(cx).issue_by_key("LLM-3", cx).clone());
    assert_eq!((moved.parent, moved.order_step), (None, None));
    assert_eq!(
        parent_writes(&mut requests),
        [("LLM-3".to_string(), Change::Clear)]
    );
}

// LLM-209: `:step` moves the selected or marked tasks between steps.

fn run(handle: AnyWindowHandle, view: &Entity<Workspace>, command: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command(command, window, cx))
    })
    .unwrap();
    cx.run_until_parked();
}

fn select(view: &Entity<Workspace>, n: u128, cx: &mut TestAppContext) {
    let row = row_of(view, n, cx);
    view.update(cx, |this, cx| {
        this.list().update(cx, |list, cx| list.select(row, cx));
    });
}

fn mark_tasks(view: &Entity<Workspace>, numbers: &[u128], cx: &mut TestAppContext) {
    view.update(cx, |this, cx| {
        this.list().update(cx, |list, cx| {
            for n in numbers {
                list.marked.insert(IssueId::from_u128(*n));
            }
            cx.notify();
        });
    });
}

fn message(view: &Entity<Workspace>, cx: &mut TestAppContext) -> String {
    cx.read(|cx| view.read(cx).message().to_string())
}

/// The steps of LLM-1's sub-issues in the app, as `(number, step)`.
fn steps(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Vec<(u128, Option<u32>)> {
    cx.read(|cx| {
        (2..=8)
            .map(|n| {
                let issue = view.read(cx).issue_by_key(&format!("LLM-{n}"), cx);
                (n, issue.order_step)
            })
            .collect()
    })
}

/// The order writes the workspace sent: the parent and its steps.
fn order_writes(requests: &mut Requests) -> Vec<(String, Vec<Vec<String>>)> {
    let mut out = Vec::new();
    while let Ok(request) = requests.try_recv() {
        if let Request::Write {
            key,
            write: Write::SetOrder(order),
            ..
        } = request
        {
            let steps = order
                .steps
                .iter()
                .map(|s| s.iter().map(ToString::to_string).collect())
                .collect();
            out.push((key, steps));
        }
    }
    out
}

fn strings(steps: &[&[&str]]) -> Vec<Vec<String>> {
    steps
        .iter()
        .map(|s| s.iter().map(|k| k.to_string()).collect())
        .collect()
}

#[gpui_kit::test]
fn step_moves_the_selected_task_and_sends_the_whole_order(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    // LLM-8 has no step. Put it in step 2.
    select(&view, 8, cx);
    run(handle, &view, "step 2", cx);
    assert_eq!(message(&view, cx), "LLM-8 is in step 2 of LLM-1");
    assert_eq!(
        order_writes(&mut requests),
        [(
            "LLM-1".to_string(),
            strings(&[
                &["LLM-2"],
                &["LLM-3", "LLM-4", "LLM-5", "LLM-8"],
                &["LLM-7"],
                &["LLM-6"]
            ])
        )]
    );
    assert_eq!(
        steps(&view, cx),
        [
            (2, Some(1)),
            (3, Some(2)),
            (4, Some(2)),
            (5, Some(2)),
            (6, Some(4)),
            (7, Some(3)),
            (8, Some(2)),
        ]
    );
    // The list follows at once: LLM-8 now sits in step 2, on the line.
    cx.read(|cx| {
        let list = view.read(cx).list().read(cx);
        let row = list.row_of(IssueId::from_u128(8)).unwrap();
        assert_eq!(row, 5, "after LLM-3, LLM-4 and LLM-5");
    });

    // The same step again changes nothing and says so.
    run(handle, &view, "step 2", cx);
    assert_eq!(
        message(&view, cx),
        "Already in step 2 of LLM-1: nothing changed"
    );
    assert!(order_writes(&mut requests).is_empty());

    // :step none takes it out again.
    select(&view, 8, cx);
    run(handle, &view, "step none", cx);
    assert_eq!(message(&view, cx), "LLM-8 left the order of LLM-1");
    assert_eq!(steps(&view, cx)[6], (8, None));
    assert_eq!(order_writes(&mut requests).len(), 1);
}

#[gpui_kit::test]
fn step_new_puts_the_marked_tasks_in_one_new_last_step(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    mark_tasks(&view, &[2, 8], cx);
    run(handle, &view, "step new", cx);
    // LLM-2 leaves step 1 empty, so the steps move up by one.
    assert_eq!(
        message(&view, cx),
        "2 tasks are in step 4 of LLM-1, a new last step"
    );
    assert_eq!(
        order_writes(&mut requests),
        [(
            "LLM-1".to_string(),
            strings(&[
                &["LLM-3", "LLM-4", "LLM-5"],
                &["LLM-7"],
                &["LLM-6"],
                &["LLM-2", "LLM-8"]
            ])
        )]
    );
    assert_eq!(steps(&view, cx)[0], (2, Some(4)));
    assert_eq!(steps(&view, cx)[6], (8, Some(4)));
}

#[gpui_kit::test]
fn step_refuses_tasks_of_different_parents_or_none(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    let before = steps(&view, cx);
    mark_tasks(&view, &[8, 10], cx);
    run(handle, &view, "step 1", cx);
    assert_eq!(
        message(&view, cx),
        "LLM-8 and LLM-10 have different parents. Pick tasks of one parent"
    );
    view.update(cx, |this, cx| {
        this.list().update(cx, |list, _| list.marked.clear());
    });
    select(&view, 11, cx);
    run(handle, &view, "step 1", cx);
    assert_eq!(
        message(&view, cx),
        "LLM-11 has no parent. Only sub-issues have a step"
    );
    select(&view, 8, cx);
    run(handle, &view, "step 9", cx);
    assert_eq!(
        message(&view, cx),
        "LLM-1 has 4 steps. Use :step 1 to :step 5, or :step new"
    );
    run(handle, &view, "step soon", cx);
    assert_eq!(message(&view, cx), "Use :step <n>, :step new or :step none");
    assert_eq!(steps(&view, cx), before, "nothing changed");
    assert!(order_writes(&mut requests).is_empty());
}

#[gpui_kit::test]
fn the_palette_suggests_the_step_commands(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    select(&view, 8, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press(":", cx);
        window.input("step n", cx);
        window.render_frame(cx);
    })
    .unwrap();
    let typed = cx.read(|cx| view.read(cx).palette_typed(cx));
    assert_eq!(typed, "step n");
    // The first match is selected, so Enter runs it.
    cx.update_window(handle, |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        message(&view, cx),
        "LLM-8 is in step 5 of LLM-1, a new last step"
    );
}
