//! LLM-144: a status change must not scroll the issue list. Approving a
//! ticket in the review panel moved it to the Done group, and the list
//! scrolled down to follow it.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};

use crate::issues::list::GroupBy;
use crate::model::backend::{Backend, Event, WriteKind};
use crate::model::data::{Issue, IssueId, Status, StatusType, WaitingOn};
use crate::shell::workspace::Workspace;

/// 80 tasks: 60 In Progress, 10 in User review and 10 Done. Grouped by
/// status, the groups show in that order, and the list is far taller than
/// the window.
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
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
    let started = catalog.find_status("LLM", "In Progress").unwrap();
    let done = catalog.find_status("LLM", "Done").unwrap();
    let review = Status::new("User review", StatusType::Started, 9).with_review(true);
    let demo = crate::model::demo::issues(&catalog);
    let issues: Vec<Issue> = (0..80)
        .map(|n| {
            let mut issue = demo[n % demo.len()].clone();
            issue.id = llmtodo_core::IssueId::from_u128(n as u128 + 1);
            issue.key = format!("LLM-{}", n + 1).into();
            issue.title = format!("Task number {n}").into();
            issue.claim = None;
            issue.analyzing = None;
            issue.deleted = false;
            issue.status = match n {
                0..60 => started.clone(),
                60..70 => review.clone(),
                _ => done.clone(),
            };
            if (60..70).contains(&n) {
                issue.waiting_on = Some(WaitingOn::Owner);
            }
            issue
        })
        .collect();
    view.update(cx, |this, cx| this.load(catalog, issues, cx));
    let (backend, _requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    // The test keeps no receiver, so sent writes go nowhere.
    view.update(cx, |this, cx| {
        this.list()
            .update(cx, |list, cx| list.group_by(GroupBy::Status, cx));
    });
    cx.run_until_parked();
    (handle, view)
}

fn issue(view: &Entity<Workspace>, key: &str, cx: &mut TestAppContext) -> Issue {
    cx.read(|cx| view.read(cx).store().read(cx).by_key(key).unwrap().clone())
}

fn offset(view: &Entity<Workspace>, cx: &mut TestAppContext) -> gpui_kit::Pixels {
    cx.read(|cx| view.read(cx).list().read(cx).scroll_offset())
}

fn selected(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<IssueId> {
    cx.read(|cx| view.read(cx).list().read(cx).selected_issue())
}

/// Scrolls the list so the User review group is on screen, and selects
/// LLM-65 in it.
fn scroll_to_review(handle: AnyWindowHandle, view: &Entity<Workspace>, cx: &mut TestAppContext) {
    let id = issue(view, "LLM-65", cx).id;
    view.update(cx, |this, cx| {
        this.list().update(cx, |list, cx| {
            let row = list.row_of(id).unwrap();
            list.select(row, cx);
        });
    });
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(
        offset(view, cx) < px(0.),
        "the list scrolled down to LLM-65"
    );
}

/// The server's copy of a ticket arrives with a new status.
fn server_moves(view: &Entity<Workspace>, key: &str, status: &str, cx: &mut TestAppContext) {
    let mut moved = issue(view, key, cx);
    moved.status = match status {
        "User review" => Status::new("User review", StatusType::Started, 9).with_review(true),
        name => cx.read(|cx| {
            view.read(cx)
                .store()
                .read(cx)
                .catalog
                .find_status("LLM", name)
                .unwrap()
        }),
    };
    moved.waiting_on = None;
    view.update(cx, |this, cx| this.receive_issue(moved, cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn approving_a_ticket_keeps_the_list_scroll(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    scroll_to_review(handle, &view, cx);
    let before = offset(&view, cx);
    let id = issue(&view, "LLM-65", cx).id;

    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command("open llm-65", window, cx));
        window.render_frame(cx);
        window.click("review-approve", cx);
    })
    .unwrap();
    cx.run_until_parked();
    // The server moves the ticket to Done, below in the list, and answers
    // the review write.
    server_moves(&view, "LLM-65", "Done", cx);
    let version = issue(&view, "LLM-65", cx).version;
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::WriteDone {
                key: "LLM-65".into(),
                kind: WriteKind::Review,
                version,
            },
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(issue(&view, "LLM-65", cx).status.name, "Done");
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();

    assert_eq!(offset(&view, cx), before, "the list did not scroll");
    // LLM-155: the first ticket still in review takes the selection.
    assert_ne!(selected(&view, cx), Some(id));
    assert_eq!(selected(&view, cx), Some(issue(&view, "LLM-61", cx).id));
}

#[gpui_kit::test]
fn other_status_changes_keep_the_list_scroll(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    scroll_to_review(handle, &view, cx);
    let before = offset(&view, cx);

    // Up to In Progress, back to User review, down to Done, and to Todo.
    for status in ["In Progress", "User review", "Done", "Todo"] {
        server_moves(&view, "LLM-65", status, cx);
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        assert_eq!(
            offset(&view, cx),
            before,
            "moving to {status} kept the scroll"
        );
    }
}

#[gpui_kit::test]
fn a_ticket_that_leaves_the_list_keeps_the_scroll(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    view.update(cx, |this, cx| {
        this.list()
            .update(cx, |list, cx| list.set_query("status:open", cx));
    });
    let visible = cx.read(|cx| view.read(cx).list().read(cx).visible().len());
    assert_eq!(visible, 70, "the Done tasks are hidden");
    scroll_to_review(handle, &view, cx);
    let before = offset(&view, cx);
    let row = cx.read(|cx| view.read(cx).list().read(cx).selected());

    // Done leaves the filter. The task that moves up takes its row.
    server_moves(&view, "LLM-65", "Done", cx);
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert_eq!(offset(&view, cx), before, "the list did not scroll");
    let after = cx.read(|cx| view.read(cx).list().read(cx).selected());
    assert_eq!(after, row, "the selection stayed on the same row");
    assert_eq!(selected(&view, cx), Some(issue(&view, "LLM-66", cx).id));
}

// LLM-155: Approve or Done on a review ticket opens the next ticket to
// review: the first row still in a review status.

fn run(handle: AnyWindowHandle, view: &Entity<Workspace>, command: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command(command, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

fn open_task(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<IssueId> {
    cx.read(|cx| view.read(cx).task().read(cx).detail().map(|d| d.issue))
}

#[gpui_kit::test]
fn done_on_a_review_ticket_opens_the_next_one(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    scroll_to_review(handle, &view, cx);
    let before = offset(&view, cx);

    run(handle, &view, "status Done", cx);

    let next = issue(&view, "LLM-61", cx).id;
    assert_eq!(selected(&view, cx), Some(next));
    assert_eq!(open_task(&view, cx), Some(next), "the task panel shows it");
    assert_eq!(offset(&view, cx), before, "LLM-61 was in view: no scroll");
}

#[gpui_kit::test]
fn the_next_review_ticket_out_of_view_scrolls_into_view(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    scroll_to_review(handle, &view, cx);
    // Every review ticket but LLM-65 and LLM-70 goes to Done.
    for n in (61..70).filter(|n| *n != 65) {
        server_moves(&view, &format!("LLM-{n}"), "Done", cx);
    }
    // The list goes back to the top, far above the review group.
    view.update(cx, |this, cx| {
        this.list().update(cx, |list, cx| list.select(0, cx));
    });
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let top = offset(&view, cx);
    assert_eq!(top, px(0.), "the list is back at the top");

    // Done on LLM-65 from its row menu, while the list shows the top.
    let id = issue(&view, "LLM-65", cx).id;
    let done = cx.read(|cx| {
        view.read(cx)
            .store()
            .read(cx)
            .catalog
            .find_status("LLM", "Done")
            .unwrap()
    });
    view.update(cx, |this, cx| this.set_status(id, done, cx));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();

    assert_eq!(selected(&view, cx), Some(issue(&view, "LLM-70", cx).id));
    assert!(offset(&view, cx) < top, "the list scrolled to LLM-70");
}

#[gpui_kit::test]
fn other_status_changes_on_a_review_ticket_keep_the_selection(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    scroll_to_review(handle, &view, cx);
    let id = issue(&view, "LLM-65", cx).id;

    run(handle, &view, "status In Progress", cx);

    assert_eq!(selected(&view, cx), Some(id));
}

#[gpui_kit::test]
fn done_on_the_last_review_ticket_keeps_the_selection(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    for n in (61..71).filter(|n| *n != 65) {
        server_moves(&view, &format!("LLM-{n}"), "Done", cx);
    }
    scroll_to_review(handle, &view, cx);
    let id = issue(&view, "LLM-65", cx).id;

    run(handle, &view, "status Done", cx);

    assert_eq!(selected(&view, cx), Some(id), "no review ticket is left");
}

#[gpui_kit::test]
fn done_on_marked_review_tickets_skips_all_of_them(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    scroll_to_review(handle, &view, cx);
    let (a, b) = (issue(&view, "LLM-61", cx).id, issue(&view, "LLM-62", cx).id);
    view.update(cx, |this, cx| {
        this.list().update(cx, |list, _| {
            list.toggle_marked(a);
            list.toggle_marked(b);
        });
    });

    run(handle, &view, "status Done", cx);

    assert_eq!(issue(&view, "LLM-61", cx).status.name, "Done");
    assert_eq!(issue(&view, "LLM-62", cx).status.name, "Done");
    assert_eq!(selected(&view, cx), Some(issue(&view, "LLM-63", cx).id));
}
