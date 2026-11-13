//! LLM-99: the dependency icon in the list, the Dependencies section of the
//! task view and the palette commands.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};
use llmtodo_client::types::Relation;
use serde_json::json;

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
    cx.run_until_parked();
}

fn message(view: &Entity<Workspace>, cx: &mut TestAppContext) -> String {
    cx.read(|cx| view.read(cx).message().to_string())
}

fn relation(direction: &str, key: &str, status: &str, kind: &str) -> Relation {
    serde_json::from_value(json!({
        "id": "01a1c453-1260-7779-be1c-619faeb9d424",
        "direction": direction,
        "type": "blocks",
        "created_at": "2026-11-02T10:00:00Z",
        "issue": {
            "key": key, "title": format!("Title of {key}"), "deleted": false,
            "status": {
                "id": "01a1c453-1260-7779-be1c-619faeb9d421",
                "name": status, "type": kind, "color": "#fff"
            }
        }
    }))
    .unwrap()
}

#[gpui_kit::test]
fn the_list_shows_the_icon_only_on_tasks_with_open_blockers(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.run_until_parked();
    let id = |key: &str, cx: &mut TestAppContext| {
        cx.read(|cx| view.read(cx).store().read(cx).by_key(key).unwrap().id)
    };
    let (blocked, free) = (id("LLM-104", cx), id("LLM-101", cx));
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find(format!("deps-{blocked}")).is_some());
        assert!(window.try_find(format!("deps-{free}")).is_none());
    })
    .unwrap();
}

/// Gives the task `key` one dependency in each direction, and opens it.
fn open_with_relations(
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    key: &str,
    cx: &mut TestAppContext,
) {
    run(handle, view, &format!("open {}", key.to_lowercase()), cx);
    let id = cx.read(|cx| view.read(cx).store().read(cx).by_key(key).unwrap().id);
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, cx| {
            store.set_relations(
                id,
                vec![
                    relation("blocked-by", "LLM-101", "Done", "completed"),
                    relation("blocks", "LLM-106", "Todo", "unstarted"),
                ],
            );
            cx.notify();
        });
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_task_view_lists_both_directions(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.run_until_parked();
    open_with_relations(handle, &view, "LLM-105", cx);
    cx.update_window(handle, |_, window, _| {
        window.find("deps-panel");
        window.find("dep-open-undepend-LLM-101");
        window.find("dep-open-unblock-LLM-106");
        window.find("dep-remove-undepend-LLM-101");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_locked_task_offers_no_way_to_remove_a_dependency(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.run_until_parked();
    // The demo data has an agent holding LLM-103.
    open_with_relations(handle, &view, "LLM-103", cx);
    cx.update_window(handle, |_, window, _| {
        window.find("dep-open-undepend-LLM-101");
        assert!(window.try_find("dep-remove-undepend-LLM-101").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn palette_commands_check_their_task(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.run_until_parked();
    // LLM-105 has no claim in the demo data.
    run(handle, &view, "open llm-105", cx);
    run(handle, &view, "depends", cx);
    assert_eq!(message(&view, cx), "Name a task, such as :depends LLM-9");
    run(handle, &view, "depends llm-999", cx);
    assert_eq!(message(&view, cx), "Unknown task: LLM-999");
    run(handle, &view, "depends llm-105", cx);
    assert_eq!(message(&view, cx), "LLM-105 cannot depend on itself");
    run(handle, &view, "depends llm-101", cx);
    assert_eq!(message(&view, cx), "LLM-105 now depends on LLM-101");
    run(handle, &view, "unblock llm-101", cx);
    assert_eq!(message(&view, cx), "LLM-105 no longer blocks LLM-101");
}

#[gpui_kit::test]
fn the_palette_can_open_with_a_command_typed(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            this.open_palette_with("depends ", window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(view.read(cx).palette_open(cx));
    });
    let typed = cx.read(|cx| view.read(cx).palette_typed(cx));
    assert_eq!(typed, "depends ");
}

/// LLM-208: order edges show as one chip each way, such as "Order: step 2
/// of LLM-101", not one chip per task. `:undepend` points to `:step`.
#[gpui_kit::test]
fn order_edges_show_as_one_line_per_direction(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.run_until_parked();
    run(handle, &view, "open llm-105", cx);
    let id = cx.read(|cx| view.read(cx).store().read(cx).by_key("LLM-105").unwrap().id);
    let order = |direction: &str, key: &str| {
        let mut r = relation(direction, key, "Todo", "unstarted");
        r.source = llmtodo_api_types::RelationSource::Order;
        r
    };
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, cx| {
            store.modify(id, |issue| {
                issue.parent = Some("LLM-101".into());
                issue.order_step = Some(2);
            });
            store.set_relations(
                id,
                vec![
                    order("blocked-by", "LLM-102"),
                    order("blocked-by", "LLM-103"),
                    order("blocks", "LLM-106"),
                    relation("blocks", "LLM-104", "Todo", "unstarted"),
                ],
            );
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.update_window(handle, |_, window, _| {
        window.find("dep-order-undepend");
        window.find("dep-order-unblock");
        // The manual edge keeps its own chip. The order edges have none.
        window.find("dep-open-unblock-LLM-104");
        for key in ["LLM-102", "LLM-103"] {
            assert!(
                window
                    .try_find(format!("dep-open-undepend-{key}"))
                    .is_none()
            );
        }
        assert!(window.try_find("dep-open-unblock-LLM-106").is_none());
    })
    .unwrap();
    // A click on the chip opens the parent.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("dep-order-undepend", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let open = cx.read(|cx| view.read(cx).detail(cx).map(|d| d.issue));
    let parent = cx.read(|cx| view.read(cx).store().read(cx).by_key("LLM-101").unwrap().id);
    assert_eq!(open, Some(parent));

    run(handle, &view, "open llm-105", cx);
    run(handle, &view, "undepend llm-102", cx);
    assert_eq!(
        message(&view, cx),
        "LLM-105 and LLM-102 are in their parent's order. :step none takes LLM-105 out"
    );
}
