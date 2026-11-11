//! LLM-103: the token cost of a task in the list and the task view.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};
use llmtodo_client::types::WorkSession;
use serde_json::json;

use crate::model::data::format_cost;
use crate::shell::tooltip::Card;
use crate::shell::workspace::Workspace;
use crate::task::history::cost_breakdown;
use crate::task::task_view::estimate_text;

fn session(n: u8, cost: Option<f64>, unpriced: &[&str]) -> WorkSession {
    serde_json::from_value(json!({
        "id": "01a1c453-1260-7779-be1c-619faeb9d424",
        "session_id": format!("s{n}"),
        "issue": "LLM-1",
        "actor": {
            "id": "01a1c453-1260-7779-be1c-619faeb9d403", "kind": "agent",
            "name": format!("agent-{n}"), "is_owner": false
        },
        "started_at": "2026-11-10T12:00:00Z",
        "last_seen_at": "2026-11-10T12:05:00Z",
        "notes": [],
        "usage": {
            "by_model": {}, "total_tokens": 1_500_000, "requests": 3,
            "cost_usd": cost, "unpriced_models": unpriced
        }
    }))
    .unwrap()
}

#[test]
fn costs_show_cents_and_stay_short() {
    assert_eq!(format_cost(0.004), "<$0.01");
    assert_eq!(format_cost(3.4), "$3.40");
    assert_eq!(format_cost(999.5), "$999.50");
    assert_eq!(format_cost(1234.5), "$1234");
}

#[test]
fn the_breakdown_lists_sessions_with_a_cost_most_expensive_first() {
    let b = cost_breakdown(&[
        session(1, Some(0.5), &[]),
        session(2, None, &["local-model"]),
        session(3, Some(2.0), &["local-model"]),
    ]);
    let order: Vec<f64> = b.sessions.iter().map(|s| s.cost).collect();
    assert_eq!(order, [2.0, 0.5]);
    assert!(b.sessions[0].detail.starts_with("agent-3 · "));
    assert!(b.sessions[0].detail.ends_with(" · 1.5M tokens"));
    assert_eq!(b.unpriced, ["local-model"]);
}

#[test]
fn the_card_says_whether_the_cost_is_final() {
    let open = Card::cost(1.0, false, None);
    assert_eq!(open.heading.as_deref(), Some("Cost so far: $1.00"));
    assert!(open.rows.is_empty());
    assert_eq!(
        Card::cost(1.0, true, None).heading.as_deref(),
        Some("Final cost: $1.00")
    );
}

#[test]
fn the_card_caps_its_sessions_and_names_models_with_no_price() {
    let sessions: Vec<WorkSession> = (0..10).map(|n| session(n, Some(1.0), &["x"])).collect();
    let card = Card::cost(10.0, false, Some(&cost_breakdown(&sessions)));
    assert_eq!(card.rows.len(), 9);
    assert_eq!(card.rows[8].1.as_ref(), "and 2 more");
    assert_eq!(
        card.body.as_deref(),
        Some("Leaves out models with no price: x.")
    );
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

#[test]
fn the_estimate_shows_time_and_cost_with_a_dash_for_each_missing_value() {
    assert_eq!(estimate_text(Some(90), Some(4.0)), "1h 30m · $4.00");
    assert_eq!(estimate_text(Some(45), None), "45m · —");
    assert_eq!(estimate_text(None, Some(4.0)), "— · $4.00");
    assert_eq!(estimate_text(None, None), "—");
}

/// LLM-139: the line shows Cost, Time and Estimate on every task, with a
/// dash for a missing value.
#[gpui_kit::test]
fn every_task_has_a_cost_line(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.run_until_parked();
    let run = |command: &str, cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |this, cx| this.run_command(command, window, cx))
        })
        .unwrap();
        cx.run_until_parked();
    };
    // The demo data has usage on LLM-101 and none on LLM-105.
    run("open llm-101", cx);
    cx.update_window(handle, |_, window, _| {
        window.find("cost-line");
    })
    .unwrap();
    run("open llm-105", cx);
    cx.update_window(handle, |_, window, _| {
        window.find("cost-line");
    })
    .unwrap();
}
