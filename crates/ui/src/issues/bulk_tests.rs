//! UI integration tests for `x` marks and bulk actions.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

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

fn marked_keys(view: &Entity<Workspace>, cx: &TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        let this = view.read(cx);
        this.marked(cx)
            .into_iter()
            .map(|id| this.issue(id, cx).key.to_string())
            .collect()
    })
}

fn status_of(view: &Entity<Workspace>, key: &str, cx: &TestAppContext) -> String {
    cx.read(|cx| view.read(cx).issue_by_key(key, cx).status.name.to_string())
}

#[gpui_kit::test]
fn x_marks_a_run_of_tasks_and_a_status_applies_to_all(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // LLM-101 to LLM-104. Agents hold LLM-102 and LLM-103.
    press(handle, &["x", "x", "x", "x"], cx);
    assert_eq!(
        marked_keys(&view, cx),
        ["LLM-101", "LLM-102", "LLM-103", "LLM-104"]
    );
    cx.read(|cx| assert_eq!(view.read(cx).selected(cx), 4, "x moves down"));

    // x on a marked task clears its mark.
    press(handle, &["k", "x"], cx);
    assert_eq!(marked_keys(&view, cx), ["LLM-101", "LLM-102", "LLM-103"]);
    press(handle, &["k", "x"], cx);

    run(handle, &view, "status done", cx);
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).message(),
            "2 tasks now Done. Skipped 2 tasks agents hold"
        );
    });
    assert_eq!(status_of(&view, "LLM-101", cx), "Done");
    assert_eq!(status_of(&view, "LLM-104", cx), "Done");
    assert_eq!(status_of(&view, "LLM-102", cx), "In Progress");

    // Esc clears the marks, and commands act on one task again.
    press(handle, &["escape"], cx);
    assert!(marked_keys(&view, cx).is_empty());
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Cleared the marks"));
}

#[gpui_kit::test]
fn a_bulk_label_adds_until_all_have_it_then_removes(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // LLM-105 has infra, LLM-104 does not.
    press(handle, &["j", "j", "j", "x", "x"], cx);
    let has = |key: &str, cx: &TestAppContext| {
        cx.read(|cx| {
            view.read(cx)
                .issue_by_key(key, cx)
                .labels
                .iter()
                .any(|l| l.name.as_ref() == "infra")
        })
    };
    assert!(!has("LLM-104", cx) && has("LLM-105", cx));
    run(handle, &view, "label infra", cx);
    assert!(
        has("LLM-104", cx) && has("LLM-105", cx),
        "added where missing"
    );
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Added infra to 2 tasks"));
    run(handle, &view, "label infra", cx);
    assert!(!has("LLM-104", cx) && !has("LLM-105", cx));
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Removed infra from 2 tasks"));
}

#[gpui_kit::test]
fn marked_tasks_go_to_the_trash_and_come_back_together(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    press(handle, &["j", "j", "j", "x", "x", "x"], cx);
    run(handle, &view, "delete", cx);
    let count = cx.read(|cx| view.read(cx).visible_count(cx));
    assert_eq!(count, 17);
    run(handle, &view, "restore", cx);
    cx.read(|cx| {
        assert_eq!(view.read(cx).message(), "Restored 3 tasks");
        assert_eq!(view.read(cx).visible_count(cx), 20);
    });
    run(handle, &view, "unassign", cx);
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.message(), "Unassigned 3 tasks");
        for key in ["LLM-104", "LLM-105", "LLM-106"] {
            assert!(this.issue_by_key(key, cx).assignee.is_none(), "{key}");
        }
    });
}

fn click(
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    key: &str,
    click: crate::issues::list::RowClick,
    cx: &mut TestAppContext,
) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            let id = this.issue_by_key(key, cx).id;
            this.click_row(id, click, window, cx)
        })
    })
    .unwrap();
}

#[gpui_kit::test]
fn shift_click_marks_a_range_and_cmd_click_toggles_one(cx: &mut TestAppContext) {
    use crate::issues::list::RowClick::*;
    let (handle, view) = open(cx);
    // Nothing is marked: the range starts at the selected row, LLM-101.
    click(handle, &view, "LLM-103", Range, cx);
    assert_eq!(marked_keys(&view, cx), ["LLM-101", "LLM-102", "LLM-103"]);
    cx.read(|cx| assert_eq!(view.read(cx).selected(cx), 2, "the clicked row is selected"));

    // Cmd-click clears one mark, and its row becomes the anchor.
    click(handle, &view, "LLM-102", Toggle, cx);
    assert_eq!(marked_keys(&view, cx), ["LLM-101", "LLM-103"]);
    click(handle, &view, "LLM-102", Toggle, cx);
    click(handle, &view, "LLM-105", Range, cx);
    assert_eq!(
        marked_keys(&view, cx),
        ["LLM-101", "LLM-102", "LLM-103", "LLM-104", "LLM-105"]
    );

    // A plain click clears the marks, and the next range starts there.
    click(handle, &view, "LLM-104", Plain, cx);
    assert!(marked_keys(&view, cx).is_empty());
    click(handle, &view, "LLM-102", Range, cx);
    assert_eq!(marked_keys(&view, cx), ["LLM-102", "LLM-103", "LLM-104"]);

    // Bulk actions apply to the range. Agents hold LLM-102 and LLM-103.
    run(handle, &view, "status done", cx);
    assert_eq!(status_of(&view, "LLM-104", cx), "Done");
    assert_eq!(status_of(&view, "LLM-102", cx), "In Progress");
}

#[gpui_kit::test]
fn a_later_shift_click_moves_the_range_end(cx: &mut TestAppContext) {
    use crate::issues::list::RowClick::*;
    let (handle, view) = open(cx);
    click(handle, &view, "LLM-102", Plain, cx);
    click(handle, &view, "LLM-105", Range, cx);
    assert_eq!(
        marked_keys(&view, cx),
        ["LLM-102", "LLM-103", "LLM-104", "LLM-105"]
    );
    // A nearer row shrinks the range.
    click(handle, &view, "LLM-103", Range, cx);
    assert_eq!(marked_keys(&view, cx), ["LLM-102", "LLM-103"]);
    // A row past the anchor flips it.
    click(handle, &view, "LLM-101", Range, cx);
    assert_eq!(marked_keys(&view, cx), ["LLM-101", "LLM-102"]);
}

#[gpui_kit::test]
fn a_range_keeps_marks_made_outside_it(cx: &mut TestAppContext) {
    use crate::issues::list::RowClick::*;
    let (handle, view) = open(cx);
    click(handle, &view, "LLM-101", Toggle, cx);
    click(handle, &view, "LLM-103", Toggle, cx);
    click(handle, &view, "LLM-105", Range, cx);
    click(handle, &view, "LLM-104", Range, cx);
    assert_eq!(marked_keys(&view, cx), ["LLM-101", "LLM-103", "LLM-104"]);
}

#[gpui_kit::test]
fn the_box_marks_a_row_and_a_second_click_clears_it(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let click_box = |row: usize, cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            // The box shows when the row is hovered.
            window.hover(("row", row), cx);
            window.render_frame(cx);
            window.click(("row-check", row), cx);
            window.render_frame(cx);
        })
        .unwrap();
    };
    click_box(1, cx);
    click_box(3, cx);
    assert_eq!(marked_keys(&view, cx), ["LLM-102", "LLM-104"]);
    click_box(1, cx);
    assert_eq!(marked_keys(&view, cx), ["LLM-104"]);
}

/// LLM-152: a bare status name such as `:done` sets that status, on the
/// open task and on every marked task.
#[gpui_kit::test]
fn a_bare_status_name_sets_the_status(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, ":done", cx);
    assert_eq!(status_of(&view, "LLM-101", cx), "Done");

    press(handle, &["j", "j", "j", "x", "x"], cx);
    run(handle, &view, "done", cx);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "2 tasks now Done"));
    assert_eq!(status_of(&view, "LLM-104", cx), "Done");
    assert_eq!(status_of(&view, "LLM-105", cx), "Done");

    run(handle, &view, "in-progress", cx);
    assert_eq!(status_of(&view, "LLM-104", cx), "In Progress");
    run(handle, &view, "dne", cx);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Unknown command: dne"));
}

fn on_issue(
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    key: &str,
    action: impl FnOnce(&Workspace, &gpui_kit::App) -> crate::shell::view_events::IssueAction,
    cx: &mut TestAppContext,
) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            let id = this.issue_by_key(key, cx).id;
            let action = action(this, cx);
            let event = crate::shell::view_events::ViewEvent::OnIssue(id, action);
            this.on_view_event(event, window, cx);
        })
    })
    .unwrap();
}

/// LLM-153: the row menu and a drop set the status of every marked task
/// when the clicked task is one of them.
#[gpui_kit::test]
fn the_row_menu_sets_the_status_of_every_marked_task(cx: &mut TestAppContext) {
    use crate::issues::list::RowClick::*;
    use crate::shell::view_events::IssueAction;
    let (handle, view) = open(cx);
    click(handle, &view, "LLM-104", Plain, cx);
    click(handle, &view, "LLM-106", Range, cx);
    let done = |this: &Workspace, cx: &gpui_kit::App| {
        let issue = this.issue_by_key("LLM-105", cx);
        let status = this
            .store()
            .read(cx)
            .catalog
            .find_status(&issue.team, "done");
        IssueAction::SetStatus(status.unwrap())
    };
    on_issue(handle, &view, "LLM-105", done, cx);
    for key in ["LLM-104", "LLM-105", "LLM-106"] {
        assert_eq!(status_of(&view, key, cx), "Done", "{key}");
    }
    cx.read(|cx| assert_eq!(view.read(cx).message(), "3 tasks now Done"));

    // A task outside the marks changes alone.
    let todo = |this: &Workspace, cx: &gpui_kit::App| {
        let issue = this.issue_by_key("LLM-101", cx);
        let status = this
            .store()
            .read(cx)
            .catalog
            .find_status(&issue.team, "todo");
        IssueAction::SetStatus(status.unwrap())
    };
    on_issue(handle, &view, "LLM-101", todo, cx);
    assert_eq!(status_of(&view, "LLM-101", cx), "Todo");
    assert_eq!(status_of(&view, "LLM-104", cx), "Done");
}
