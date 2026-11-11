//! UI integration tests for `c`, the form that creates an issue.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};
use llmtodo_client::types::{CreateIssue, Priority};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::model::backend::{Backend, Event, Request};
use crate::shell::workspace::Workspace;
use crate::task::task_view::DetailMode;

fn open(
    cx: &mut TestAppContext,
) -> (
    AnyWindowHandle,
    Entity<Workspace>,
    UnboundedReceiver<Request>,
) {
    open_with(cx, WindowOptions::default())
}

fn open_sized(
    cx: &mut TestAppContext,
    width: f32,
    height: f32,
) -> (
    AnyWindowHandle,
    Entity<Workspace>,
    UnboundedReceiver<Request>,
) {
    open_with(
        cx,
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                Default::default(),
                size(px(width), px(height)),
            ))),
            ..Default::default()
        },
    )
}

fn open_with(
    cx: &mut TestAppContext,
    options: WindowOptions,
) -> (
    AnyWindowHandle,
    Entity<Workspace>,
    UnboundedReceiver<Request>,
) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .unwrap()
    });
    let (backend, requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    (handle, view, requests)
}

fn run(handle: AnyWindowHandle, view: &Entity<Workspace>, command: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command(command, window, cx))
    })
    .unwrap();
}

fn created(requests: &mut UnboundedReceiver<Request>) -> Option<CreateIssue> {
    match requests.try_recv() {
        Ok(Request::Create(c)) => Some(*c),
        _ => None,
    }
}

/// The server's copy of LLM-121 with this title and description.
fn issue_json(
    title: &str,
    description: &str,
    project: Option<&str>,
) -> llmtodo_client::types::Issue {
    serde_json::from_value(serde_json::json!({
        "id": "01a1c453-1260-7779-be1c-619faeb9d999",
        "key": "LLM-121", "team": "LLM", "title": title,
        "status": {"id": "01a1c453-1260-7779-be1c-619faeb9d421", "name": "Todo", "type": "unstarted", "color": "#fff"},
        "priority": "high", "assignee": null, "labels": ["docs"], "project": project,
        "milestone": null, "parent": null, "wanted_executor": {}, "description": description,
        "description_version": 1, "claim": null, "created_by": null,
        "created_at": "2026-11-10T10:00:00Z", "updated_at": "2026-11-10T10:00:00Z",
        "completed_at": null, "deleted_at": null
    }))
    .unwrap()
}

/// The server's answer for a new issue LLM-121 with this title.
fn answer(view: &Entity<Workspace>, title: &str, project: Option<&str>, cx: &mut TestAppContext) {
    let issue = issue_json(title, "", project);
    let id = issue.id;
    view.update(cx, |this, cx| {
        this.apply_event(Event::Issue(Box::new(issue)), cx);
        this.apply_event(
            Event::Created {
                id,
                key: "LLM-121".into(),
            },
            cx,
        );
    });
}

#[gpui_kit::test]
fn c_opens_the_form_and_enter_creates_the_issue(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        window.render_frame(cx);
        assert!(view.read(cx).create_open(cx));
        // The form takes the keys, so j is text, not a move.
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
        window.input("Write the docs label:docs priority:high", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.read(|cx| assert!(!view.read(cx).create_open(cx)));
    let create = created(&mut requests).expect("a create request");
    assert_eq!(create.title, "Write the docs");
    assert_eq!(create.team.as_deref(), Some("LLM"));
    assert_eq!(create.labels, ["docs"]);
    assert_eq!(create.priority, Some(Priority::High));
    assert_eq!(create.project, None);

    // The new issue is selected and opens in the task view.
    answer(&view, "Write the docs", None, cx);
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.selected_issue_key(cx).as_deref(), Some("LLM-121"));
        let open = this.detail(cx).expect("the new issue opens");
        assert_eq!(this.issue(open.issue, cx).key.as_ref(), "LLM-121");
        assert_eq!(this.message(), "Created LLM-121");
    });
}

#[gpui_kit::test]
fn esc_and_ctrl_c_close_the_form_without_creating(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    for key in ["escape", "ctrl-c"] {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("c", cx);
            window.input("Half a thought", cx);
            window.press(key, cx);
            assert!(!view.read(cx).create_open(cx), "{key} closes the form");
            assert_eq!(view.read(cx).vim_mode(window, cx), "normal");
        })
        .unwrap();
    }
    assert!(created(&mut requests).is_none());
}

#[gpui_kit::test]
fn new_goes_to_the_sidebar_project_and_checks_its_words(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    run(handle, &view, "project server", cx);
    run(
        handle,
        &view,
        "new Ship it status:in-review assignee:me",
        cx,
    );
    let create = created(&mut requests).expect("a create request");
    assert_eq!(create.title, "Ship it");
    assert_eq!(create.project.as_deref(), Some("server"));
    assert_eq!(create.status.as_deref(), Some("In Review"));
    assert_eq!(create.assignee.as_deref(), Some("me"));

    // Unknown words stop the create, so nothing goes out half right.
    for (command, message) in [
        ("new Ship label:nope", "Unknown label: nope"),
        ("new Ship status:later", "Unknown status: later"),
        ("new Ship priority:soon", "Unknown priority: soon"),
        ("new label:docs", "Type a title for the new issue"),
    ] {
        run(handle, &view, command, cx);
        cx.read(|cx| assert_eq!(view.read(cx).message(), message));
        assert!(created(&mut requests).is_none(), "{command}");
    }

    // A new issue outside the list's filter is created but not shown.
    run(handle, &view, "project app", cx);
    answer(&view, "Elsewhere", Some("server"), cx);
    cx.read(|cx| {
        assert!(
            view.read(cx)
                .message()
                .starts_with("Created LLM-121. The list hides it")
        );
    });
}

/// The server's activity for LLM-121: the one event a create records.
fn send_activity(view: &Entity<Workspace>, cx: &mut TestAppContext) {
    let history = serde_json::from_value(serde_json::json!({
        "revisions": [],
        "activity": [{
            "seq": 7, "entity_type": "issue", "entity_id": "01a1c453-1260-7779-be1c-619faeb9d999",
            "issue": "LLM-121", "field": "title", "old": null, "new": "Write the docs",
            "actor": {"id": "01a1c453-1260-7779-be1c-619faeb9d402", "kind": "human", "name": "owner", "is_owner": true},
            "session_id": null, "created_at": "2026-11-10T12:03:00Z"
        }]
    }))
    .unwrap();
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::History {
                key: "LLM-121".into(),
                history: Box::new(history),
                sessions: Vec::new(),
            },
            cx,
        )
    });
}

/// Whether the editor opened on LLM-121 after pressing `e` or clicking Edit.
fn starts_editing(
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    how: &str,
    cx: &mut TestAppContext,
) -> bool {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        if how == "key" {
            window.press("e", cx);
        } else {
            window.click("mode-edit", cx);
        }
        window.render_frame(cx);
        view.read(cx)
            .detail(cx)
            .is_some_and(|d| d.mode == DetailMode::Editing)
    })
    .unwrap()
}

/// LLM-131: a new ticket has an empty description and one Activity entry.
#[gpui_kit::test]
fn a_new_ticket_opens_its_empty_description_by_key(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    answer(&view, "Write the docs", None, cx);
    send_activity(&view, cx);
    assert!(starts_editing(handle, &view, "key", cx));
}

#[gpui_kit::test]
fn a_new_ticket_opens_its_empty_description_by_the_button(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    answer(&view, "Write the docs", None, cx);
    send_activity(&view, cx);
    assert!(starts_editing(handle, &view, "button", cx));
}

/// The bug needs an empty description, not a new ticket: an older ticket
/// that arrives without a create fails the same way.
#[gpui_kit::test]
fn an_older_ticket_with_an_empty_description_opens_too(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    let issue = issue_json("Older ticket", "", None);
    let id = issue.id;
    view.update(cx, |this, cx| {
        this.apply_event(Event::Issue(Box::new(issue)), cx);
        this.open_for_test(id, cx);
    });
    send_activity(&view, cx);
    assert!(starts_editing(handle, &view, "key", cx));
}

/// Before its activity arrives, the task has no read-only tab to land on.
#[gpui_kit::test]
fn an_empty_description_opens_before_its_activity_arrives(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    answer(&view, "Write the docs", None, cx);
    assert!(starts_editing(handle, &view, "key", cx));
}

/// A new ticket with text in its description has a Description tab already.
#[gpui_kit::test]
fn a_description_with_text_opens_with_activity_loaded(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    let issue = issue_json("Older ticket", "Some text", None);
    let id = issue.id;
    view.update(cx, |this, cx| {
        this.apply_event(Event::Issue(Box::new(issue)), cx);
        this.open_for_test(id, cx);
    });
    send_activity(&view, cx);
    assert!(starts_editing(handle, &view, "key", cx));
}

/// An explicit Activity tab stays read only on a ticket with no description.
#[gpui_kit::test]
fn the_activity_tab_stays_read_only_on_an_empty_ticket(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    answer(&view, "Write the docs", None, cx);
    send_activity(&view, cx);
    run(handle, &view, "tab activity", cx);
    assert!(!starts_editing(handle, &view, "key", cx));
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Activity is read only"));
}

/// LLM-132: upper-case C does nothing now.
#[gpui_kit::test]
fn upper_case_c_no_longer_opens_the_form(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("shift-c", cx);
        assert!(!view.read(cx).create_open(cx));
    })
    .unwrap();
}

/// The key that opens the form is not typed into the title.
#[gpui_kit::test]
fn c_is_not_typed_into_the_title(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        assert_eq!(view.read(cx).create_title(cx), "");
    })
    .unwrap();
}

/// The chords that start with another key still reach their commands.
#[gpui_kit::test]
fn g_c_still_shows_the_claims_list(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("g", cx);
        window.press("c", cx);
        assert!(!view.read(cx).create_open(cx));
        assert!(view.read(cx).claims().read(cx).is_open());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_description_goes_with_the_create_request(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        window.render_frame(cx);
        window.input("Write the docs", cx);
        window.press("tab", cx);
        window.input("First line", cx);
        // Enter is a new line in the description, not a submit.
        window.press("enter", cx);
        assert!(view.read(cx).create_open(cx));
        window.input("Second line", cx);
        window.press("cmd-enter", cx);
    })
    .unwrap();
    cx.read(|cx| assert!(!view.read(cx).create_open(cx)));
    let create = created(&mut requests).expect("a create request");
    assert_eq!(create.title, "Write the docs");
    assert_eq!(create.description, "First line\nSecond line");
}

#[gpui_kit::test]
fn shift_tab_returns_to_the_title(cx: &mut TestAppContext) {
    let (handle, _view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        window.render_frame(cx);
        window.press("tab", cx);
        window.press("shift-tab", cx);
        window.input("Back in the title", cx);
        window.press("enter", cx);
    })
    .unwrap();
    let create = created(&mut requests).expect("a create request");
    assert_eq!(create.title, "Back in the title");
    assert_eq!(create.description, "");
}

#[gpui_kit::test]
fn the_create_button_creates_and_cancel_closes(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        window.render_frame(cx);
        window.input("Half a thought", cx);
        window.render_frame(cx);
        window.click("create-cancel", cx);
        assert!(!view.read(cx).create_open(cx));
    })
    .unwrap();
    assert!(created(&mut requests).is_none());

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).create_title(cx), "", "the form opens empty");
        window.input("By mouse", cx);
        window.render_frame(cx);
        window.click("create-submit", cx);
    })
    .unwrap();
    cx.read(|cx| assert!(!view.read(cx).create_open(cx)));
    assert_eq!(created(&mut requests).expect("a create").title, "By mouse");
}

#[gpui_kit::test]
fn esc_and_ctrl_c_close_the_form_from_the_description(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    for key in ["escape", "ctrl-c"] {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press("c", cx);
            window.render_frame(cx);
            window.press("tab", cx);
            window.input("Half a thought", cx);
            window.press(key, cx);
            assert!(!view.read(cx).create_open(cx), "{key} closes the form");
            assert_eq!(view.read(cx).vim_mode(window, cx), "normal");
        })
        .unwrap();
    }
    assert!(created(&mut requests).is_none());
}

/// A refused title keeps the form open, so the description is not lost.
#[gpui_kit::test]
fn a_refused_create_keeps_the_typed_text(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        window.render_frame(cx);
        window.input("Ship it label:nope", cx);
        window.press("tab", cx);
        window.input("Long text", cx);
        window.press("cmd-enter", cx);
    })
    .unwrap();
    cx.read(|cx| {
        assert!(view.read(cx).create_open(cx));
        assert_eq!(
            view.read(cx).create_notice(cx).as_deref(),
            Some("Unknown label: nope")
        );
        assert_eq!(view.read(cx).create_description(cx), "Long text");
    });
    assert!(created(&mut requests).is_none());
}

/// An empty title says so in the form and sends nothing.
#[gpui_kit::test]
fn an_empty_title_shows_a_notice(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        window.render_frame(cx);
        window.click("create-submit", cx);
        assert!(view.read(cx).create_open(cx));
        assert_eq!(
            view.read(cx).create_notice(cx).as_deref(),
            Some("Type a title for the new issue")
        );
    })
    .unwrap();
    assert!(created(&mut requests).is_none());
}

/// The form is clearly larger than the old one, which was 384 pixels wide.
#[gpui_kit::test]
fn the_form_is_wide_and_tall(cx: &mut TestAppContext) {
    let (handle, _view, _requests) = open_sized(cx, 1200., 760.);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        window.render_frame(cx);
        let form = window.find("create-form").bounds();
        assert!(form.size.width >= px(700.), "{:?}", form.size);
        assert!(form.size.height >= px(450.), "{:?}", form.size);
    })
    .unwrap();
}

/// In a short, narrow window the form stays inside and Create stays visible.
#[gpui_kit::test]
fn the_form_fits_a_small_window(cx: &mut TestAppContext) {
    let (handle, _view, _requests) = open_sized(cx, 520., 360.);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("c", cx);
        window.render_frame(cx);
        window.render_frame(cx);
        let form = window.find("create-form").bounds();
        let submit = window.find("create-submit").bounds();
        let screen = window.viewport_size();
        assert!(form.right() <= screen.width, "{form:?}");
        assert!(form.bottom() <= screen.height, "{form:?}");
        assert!(submit.bottom() <= form.bottom(), "{submit:?} in {form:?}");
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_add_ticket_button_opens_the_form(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // The button is first in the bar, left of the filter input, and
        // the input sits between it and the grouping menu.
        let bar = window.find("filter-bar").bounds();
        let button = window.within("filter-bar").find("add-ticket").bounds();
        let group = window.within("filter-bar").find("group").bounds();
        assert!(button.left() - bar.left() <= gpui_kit::px(8.), "{button:?}");
        assert!(
            group.left() - button.right() > gpui_kit::px(100.),
            "{group:?}"
        );
        window.click("add-ticket", cx);
        window.render_frame(cx);
        assert!(view.read(cx).create_open(cx));
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
        window.input("From the button", cx);
        window.press("enter", cx);
    })
    .unwrap();
    let create = created(&mut requests).expect("a create request");
    assert_eq!(create.title, "From the button");
}
