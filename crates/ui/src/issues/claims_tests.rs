//! UI integration tests for the claims list and the palette commands that
//! open tasks: `g c`, `:claims`, `:open` and the status bar count.

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

fn claims_open(view: &Entity<Workspace>, cx: &mut TestAppContext) -> bool {
    cx.read(|cx| view.read(cx).claims().read(cx).is_open())
}

/// The keys of the claimed tasks, in the order the list shows them.
fn claim_keys(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        view.read(cx)
            .claims()
            .read(cx)
            .rows(cx)
            .into_iter()
            .map(|r| r.key.to_string())
            .collect()
    })
}

fn open_key(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<String> {
    cx.read(|cx| {
        let this = view.read(cx);
        this.detail(cx)
            .map(|d| this.issue(d.issue, cx).key.to_string())
    })
}

#[gpui_kit::test]
fn g_c_lists_claims_and_enter_opens_the_selected_task(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // The demo has two tasks that agents hold, the longest held first.
    assert_eq!(claim_keys(&view, cx), ["LLM-102", "LLM-103"]);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("g", cx);
        window.press("c", cx);
    })
    .unwrap();
    assert!(claims_open(&view, cx));
    let list_row = cx.read(|cx| view.read(cx).selected(cx));

    // j and Ctrl-N move in the claims list, not in the issue list. The
    // first row starts selected, and the last row stops the move.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).claims().read(cx).selected(), 0);
        window.press("j", cx);
        assert_eq!(view.read(cx).claims().read(cx).selected(), 1);
        window.press("ctrl-n", cx);
        assert_eq!(view.read(cx).claims().read(cx).selected(), 1);
        window.press("ctrl-p", cx);
        window.press("j", cx);
        assert_eq!(view.read(cx).selected(cx), list_row);
        window.press("enter", cx);
    })
    .unwrap();
    assert!(!claims_open(&view, cx));
    assert_eq!(open_key(&view, cx).as_deref(), Some("LLM-103"));
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).selected_issue_key(cx).as_deref(),
            Some("LLM-103")
        );
    });
}

/// LLM-102: a row names the model alone, and the actor name stands in when
/// the agent reports no model.
#[test]
fn a_claim_row_shows_the_model_and_falls_back_to_the_name() {
    use crate::model::data::{Agent, Assignee};
    let agent = |model: Option<&str>| {
        Assignee::Agent(Agent {
            name: "local/qwen".into(),
            harness: Some("ollama".into()),
            model: model.map(|m| m.to_string().into()),
            effort: None,
        })
    };
    assert_eq!(agent(Some("qwen3-coder")).model().as_ref(), "qwen3-coder");
    assert_eq!(agent(None).model().as_ref(), "local/qwen");
    assert_eq!(Assignee::Person("bob".into()).model().as_ref(), "bob");
}

#[gpui_kit::test]
fn esc_closes_the_claims_list_and_a_click_opens_a_row(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "claims", cx);
    assert!(claims_open(&view, cx));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    assert!(!claims_open(&view, cx));
    assert_eq!(open_key(&view, cx), None);

    // The claim count in the status bar opens the list too.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("status-claims", cx);
        window.render_frame(cx);
        window.click("claim-LLM-102", cx);
    })
    .unwrap();
    assert!(!claims_open(&view, cx));
    assert_eq!(open_key(&view, cx).as_deref(), Some("LLM-102"));
}

#[gpui_kit::test]
fn a_release_leaves_the_list_and_the_selection_stays_on_a_row(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "claims", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
    })
    .unwrap();
    // The agent on LLM-103 releases it while the list is open.
    view.update(cx, |this, cx| {
        let mut issue = this.issue_by_key("LLM-103", cx).clone();
        issue.claim = None;
        this.upsert_issue(issue, cx);
    });
    assert_eq!(claim_keys(&view, cx), ["LLM-102"]);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(view.read(cx).claims().read(cx).selected(), 0);
        window.press("enter", cx);
    })
    .unwrap();
    assert_eq!(open_key(&view, cx).as_deref(), Some("LLM-102"));
}

#[gpui_kit::test]
fn open_takes_a_key_in_any_case(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "open llm-105", cx);
    assert_eq!(open_key(&view, cx).as_deref(), Some("LLM-105"));
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).selected_issue_key(cx).as_deref(),
            Some("LLM-105")
        );
    });
    run(handle, &view, "open LLM-999", cx);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "No task LLM-999"));
    assert_eq!(open_key(&view, cx).as_deref(), Some("LLM-105"));
}

#[gpui_kit::test]
fn the_palette_suggests_open_for_part_of_a_key(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press(":", cx);
        window.input("open llm-104", cx);
        window.press("enter", cx);
    })
    .unwrap();
    assert_eq!(open_key(&view, cx).as_deref(), Some("LLM-104"));
}
