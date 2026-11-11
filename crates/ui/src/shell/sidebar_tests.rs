//! UI integration tests for the sidebar: Cmd-B, keyboard moves, clicks,
//! palette commands and saved settings.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

use crate::model::scope::Scope;
use crate::settings::{SettingsStore, UiSettings};
use crate::shell::workspace::Workspace;

/// A workspace on demo data that saves its settings to `store`.
fn open_with(
    settings: UiSettings,
    store: SettingsStore,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| {
                let mut this = Workspace::with_settings(settings, store, window, cx);
                this.load_demo(cx);
                this
            })
        })
        .unwrap()
    })
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    open_with(UiSettings::default(), SettingsStore::memory(), cx)
}

fn run(handle: AnyWindowHandle, view: &Entity<Workspace>, command: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command(command, window, cx))
    })
    .unwrap();
}

/// The keys of every issue the list shows.
fn shown(view: &Entity<Workspace>, cx: &TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        let this = view.read(cx);
        (0..this.visible_count(cx))
            .map(|n| this.visible_issue(n, cx).key.to_string())
            .collect()
    })
}

/// In the demo data, issue `ix` belongs to App when it is even, Server when
/// it is odd, and to no project every fifth row.
fn demo_keys_in(project: &str) -> Vec<String> {
    (0..20)
        .filter(|ix| ix % 5 != 4)
        .filter(|ix| (ix % 2 == 0) == (project == "app"))
        .map(|ix| format!("LLM-{}", 101 + ix))
        .collect()
}

#[gpui_kit::test]
fn cmd_b_shows_the_sidebar_and_h_moves_into_it(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(!view.read(cx).sidebar_open());
        // Closed, h stays in the list.
        window.press("h", cx);
        assert!(!view.read(cx).sidebar_focused(window, cx));

        window.press("cmd-b", cx);
        window.render_frame(cx);
        assert!(view.read(cx).sidebar_open());
        window.press("h", cx);
        assert!(view.read(cx).sidebar_focused(window, cx));

        // j shows the next row at once: the inbox, the team, its first
        // project, then the next.
        window.press("j", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Inbox);
        window.press("j", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Team("LLM".into()));
        window.press("j", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Project("app".into()));
        window.press("ctrl-n", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Project("server".into()));
        window.press("k", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Project("app".into()));
        // The Trash is the last row, below the saved views.
        window.press("shift-g", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Trash);
        // The last row stays the last row.
        window.press("j", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Trash);
        // Above it: Settings, the documents, then the saved views.
        window.press("ctrl-p", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Settings);
        window.press("ctrl-p", cx);
        assert!(matches!(view.read(cx).scope(cx), Scope::Document(_)));
        window.press("ctrl-p", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::View("Open bugs".into()));
        window.press("k", cx);
        window.press("k", cx);
        assert_eq!(view.read(cx).scope(cx), Scope::Project("app".into()));

        // l goes back to the list, where j moves through the issues again.
        window.press("l", cx);
        assert!(!view.read(cx).sidebar_focused(window, cx));
        assert_eq!(view.read(cx).selected(cx), 0);
        window.press("j", cx);
        assert_eq!(view.read(cx).selected(cx), 1);

        // Cmd-B from inside the sidebar hides it and gives focus back.
        window.press("h", cx);
        assert!(view.read(cx).sidebar_focused(window, cx));
        window.press("cmd-b", cx);
        assert!(!view.read(cx).sidebar_open());
        assert!(!view.read(cx).sidebar_focused(window, cx));
    })
    .unwrap();
    assert_eq!(shown(&view, cx), demo_keys_in("app"));
}

#[gpui_kit::test]
fn a_click_on_a_project_row_shows_its_issues(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "sidebar", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("sidebar-project:server", cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).message(), "Showing Server");
        // Focus goes to the list, so j moves through the issues shown.
        assert!(!view.read(cx).sidebar_focused(window, cx));
    })
    .unwrap();
    assert_eq!(shown(&view, cx), demo_keys_in("server"));
    cx.update_window(handle, |_, window, cx| {
        window.click("sidebar-all", cx);
    })
    .unwrap();
    assert_eq!(shown(&view, cx).len(), 20);
}

#[gpui_kit::test]
fn palette_commands_pick_a_team_or_project(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "project Server", cx);
    assert_eq!(shown(&view, cx), demo_keys_in("server"));
    run(handle, &view, "team llm", cx);
    assert_eq!(shown(&view, cx).len(), 20);
    run(handle, &view, "project app", cx);
    assert_eq!(shown(&view, cx), demo_keys_in("app"));

    run(handle, &view, "project nope", cx);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Unknown project: nope"));
    assert_eq!(shown(&view, cx), demo_keys_in("app"), "unchanged");

    run(handle, &view, "all", cx);
    assert_eq!(shown(&view, cx).len(), 20);
}

#[gpui_kit::test]
fn the_sidebar_and_its_scope_survive_a_restart(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.json");
    let store = SettingsStore::at(path.clone());
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    run(handle, &view, "sidebar", cx);
    run(handle, &view, "project server", cx);
    let saved = store.load();
    assert!(saved.sidebar_open);
    assert_eq!(saved.sidebar_scope, "project:server");

    let (_, view) = open_with(store.load(), store, cx);
    cx.read(|cx| {
        assert!(view.read(cx).sidebar_open());
        assert_eq!(view.read(cx).scope(cx), Scope::Project("server".into()));
    });
    assert_eq!(shown(&view, cx), demo_keys_in("server"));
}

#[gpui_kit::test]
fn a_project_that_goes_away_falls_back_to_all_issues(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "project server", cx);
    view.update(cx, |this, cx| {
        let (issues, mut catalog) = this.issues_and_catalog(cx);
        catalog.projects.retain(|p| p.key.as_ref() != "server");
        this.load(catalog, issues, cx);
        assert_eq!(this.scope(cx), Scope::All);
        assert_eq!(this.message(), "server is gone. Showing all issues");
    });
    assert_eq!(shown(&view, cx).len(), 20);
}

#[gpui_kit::test]
fn the_trash_shows_trashed_issues_and_restores_them(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "sidebar", cx);
    // The first row is LLM-101. Trash it, then LLM-104: agents hold
    // LLM-102 and LLM-103.
    run(handle, &view, "delete", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("j", cx);
    })
    .unwrap();
    run(handle, &view, "delete", cx);
    assert_eq!(shown(&view, cx).len(), 18);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("sidebar-trash", cx);
    })
    .unwrap();
    assert_eq!(shown(&view, cx), ["LLM-101", "LLM-104"]);
    cx.read(|cx| assert_eq!(view.read(cx).scope(cx), Scope::Trash));

    // :restore brings back the selected task, not only the last one
    // trashed, and the next one takes the selection.
    run(handle, &view, "delete", cx);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "LLM-101 is already in the Trash"));
    run(handle, &view, "restore", cx);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Restored LLM-101"));
    assert_eq!(shown(&view, cx), ["LLM-104"]);
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).selected_issue_key(cx).as_deref(),
            Some("LLM-104")
        )
    });
    run(handle, &view, "restore", cx);
    assert!(shown(&view, cx).is_empty());

    run(handle, &view, "all", cx);
    assert_eq!(shown(&view, cx).len(), 20);
}

#[gpui_kit::test]
fn the_right_click_menu_restores_a_trashed_issue(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "delete", cx);
    run(handle, &view, "show trash", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.right_click(("row", 0usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // Restore takes the place of Move to Trash.
        window
            .within("popup-menu")
            .click(crate::shell::tests::TRASH_ITEM, cx);
    })
    .unwrap();
    assert!(shown(&view, cx).is_empty());
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Restored LLM-101"));
}

#[gpui_kit::test]
fn filter_words_narrow_the_list(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("/", cx);
        window.input("status:done label:bug", cx);
    })
    .unwrap();
    assert_eq!(shown(&view, cx), ["LLM-119"]);
    // The first match is selected, so Enter opens it.
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).selected_issue_key(cx).as_deref(),
            Some("LLM-119")
        )
    });
    // The project narrows it further.
    run(handle, &view, "project server", cx);
    assert!(shown(&view, cx).is_empty());
}

#[gpui_kit::test]
fn a_saved_view_shows_the_issues_the_server_sends(cx: &mut TestAppContext) {
    use crate::model::backend::{Backend, Event, Request};

    let (handle, view) = open(cx);
    let (backend, mut requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));

    run(handle, &view, "view open bugs", cx);
    assert!(matches!(
        requests.try_recv(),
        Ok(Request::ViewIssues { name }) if name == "Open bugs"
    ));
    // Nothing shows until the server answers.
    assert!(shown(&view, cx).is_empty());

    let ids = |keys: &[&str], cx: &TestAppContext| -> Vec<llmtodo_core::IssueId> {
        cx.read(|cx| {
            keys.iter()
                .map(|k| view.read(cx).issue_by_key(k, cx).id)
                .collect()
        })
    };
    let bugs = ids(&["LLM-116", "LLM-119"], cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::ViewIssues {
                name: "Open bugs".into(),
                ids: bugs,
            },
            cx,
        )
    });
    assert_eq!(shown(&view, cx), ["LLM-116", "LLM-119"]);

    // A changed issue asks the server again, once the changes settle.
    view.update(cx, |this, cx| {
        let mut issue = this.issue_by_key("LLM-116", cx).clone();
        issue.title = "Renamed".into();
        this.upsert_issue(issue.clone(), cx);
        this.upsert_issue(issue, cx);
    });
    assert!(requests.try_recv().is_err(), "waits for changes to settle");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    assert!(matches!(
        requests.try_recv(),
        Ok(Request::ViewIssues { name }) if name == "Open bugs"
    ));
    assert!(requests.try_recv().is_err(), "one request for both changes");

    // An answer for another view changes nothing.
    let other = ids(&["LLM-101"], cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::ViewIssues {
                name: "Mine".into(),
                ids: other,
            },
            cx,
        )
    });
    assert_eq!(shown(&view, cx), ["LLM-116", "LLM-119"]);
}

#[gpui_kit::test]
fn view_save_sends_the_scope_and_filter_and_selects_the_view(cx: &mut TestAppContext) {
    use crate::model::backend::{Backend, Event, Request};

    let (handle, view) = open(cx);
    let (backend, mut requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    run(handle, &view, "project app", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("/", cx);
        window.input("status:todo assignee:me", cx);
    })
    .unwrap();
    run(handle, &view, "view save My todo", cx);
    let Ok(Request::CreateView(create)) = requests.try_recv() else {
        panic!("expected a CreateView request");
    };
    assert_eq!(create.name, "My todo");
    assert_eq!(create.query.projects, ["app"]);
    assert_eq!(create.query.filter.statuses, ["Todo"]);
    assert_eq!(create.query.filter.assignee.as_deref(), Some("owner"));

    view.update(cx, |this, cx| {
        this.apply_event(
            Event::ViewSaved {
                name: "My todo".into(),
            },
            cx,
        );
        assert_eq!(this.scope(cx), Scope::View("My todo".into()));
        assert_eq!(this.message(), "Saved the view My todo");
    });
    assert!(matches!(
        requests.try_recv(),
        Ok(Request::ViewIssues { name }) if name == "My todo"
    ));

    // The Trash is no base for a view.
    run(handle, &view, "show trash", cx);
    run(handle, &view, "view save Nope", cx);
    cx.read(|cx| {
        assert!(view.read(cx).message().starts_with("Pick All issues"));
    });

    run(handle, &view, "view delete my todo", cx);
    assert!(matches!(
        requests.try_recv(),
        Ok(Request::DeleteView { name }) if name == "My todo"
    ));
    run(handle, &view, "view my todo", cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::ViewDeleted {
                name: "My todo".into(),
            },
            cx,
        );
        assert_eq!(this.scope(cx), Scope::All);
        assert_eq!(this.message(), "Deleted the view My todo");
    });
}

#[gpui_kit::test]
fn a_banner_shows_while_this_machine_is_not_the_primary(cx: &mut TestAppContext) {
    use crate::model::backend::{Backend, Event, Request};

    let (handle, view) = open(cx);
    let (backend, mut requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    let banner = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            gpui_kit::base::test_support::find(window, &[], &"not-primary".into()).is_some()
        })
        .unwrap()
    };
    assert!(!banner(cx));

    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Primary {
                primary: false,
                holder: Some("laptop-2".into()),
            },
            cx,
        );
        assert_eq!(this.not_primary(), Some(&Some("laptop-2".into())));
    });
    assert!(banner(cx));

    // Check again asks the server.
    cx.update_window(handle, |_, window, cx| window.click("check-primary", cx))
        .unwrap();
    assert!(matches!(requests.try_recv(), Ok(Request::SyncStatus)));

    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Primary {
                primary: true,
                holder: None,
            },
            cx,
        );
        assert_eq!(this.message(), "This machine is the primary copy again");
    });
    assert!(!banner(cx));
}

/// Server history with one status change, for the Activity tab.
fn history_for_status_change() -> Box<llmtodo_client::types::History> {
    Box::new(
        serde_json::from_value(serde_json::json!({
            "revisions": [],
            "activity": [{
                "seq": 7, "entity_type": "issue", "entity_id": "01a1c453-1260-7779-be1c-619faeb9d401",
                "issue": "LLM-104", "field": "status", "old": "Todo", "new": "In Progress",
                "actor": {"id": "01a1c453-1260-7779-be1c-619faeb9d402", "kind": "human", "name": "owner", "is_owner": true},
                "session_id": null, "created_at": "2026-11-10T12:03:00Z"
            }]
        }))
        .unwrap(),
    )
}

#[gpui_kit::test]
fn an_open_task_asks_for_its_history_and_shows_an_activity_tab(cx: &mut TestAppContext) {
    use crate::model::backend::{Backend, Event, Request};

    let (handle, view) = open(cx);
    let (backend, mut requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    let history_asked = |requests: &mut tokio::sync::mpsc::UnboundedReceiver<Request>| {
        let mut keys = Vec::new();
        while let Ok(r) = requests.try_recv() {
            if let Request::History { key } = r {
                keys.push(key);
            }
        }
        keys
    };
    let tab_titles = |cx: &mut TestAppContext| -> Vec<String> {
        cx.read(|cx| {
            view.read(cx)
                .open_tab(cx)
                .map(|(_, tabs)| tabs.into_iter().map(|t| t.title).collect())
                .unwrap_or_default()
        })
    };

    // LLM-104 is the fourth row. Opening it asks once.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("enter", cx);
    })
    .unwrap();
    assert_eq!(history_asked(&mut requests), ["LLM-104"]);
    let before = tab_titles(cx);
    assert!(!before.contains(&"Activity".to_string()));

    view.update(cx, |this, cx| {
        this.apply_event(
            Event::History {
                key: "LLM-104".into(),
                history: history_for_status_change(),
                sessions: Vec::new(),
            },
            cx,
        )
    });
    let after = tab_titles(cx);
    assert_eq!(after.last().map(String::as_str), Some("Activity"));
    assert_eq!(after.len(), before.len() + 1);
    run(handle, &view, "tab activity", cx);
    cx.read(|cx| {
        let (ix, tabs) = view.read(cx).open_tab(cx).unwrap();
        assert_eq!(tabs[ix].title, "Activity");
        assert!(tabs[ix].body.contains("**You** status: Todo → In Progress"));
    });

    // The next task asks for its own history, and an answer for the old
    // one shows nowhere.
    cx.update_window(handle, |_, window, cx| {
        window.press("j", cx);
    })
    .unwrap();
    assert_eq!(history_asked(&mut requests), ["LLM-105"]);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::History {
                key: "LLM-104".into(),
                history: history_for_status_change(),
                sessions: Vec::new(),
            },
            cx,
        )
    });
    assert!(!tab_titles(cx).contains(&"Activity".to_string()));
}

/// LLM-83: the Activity tab is marked read only. Its tab has an eye and a
/// card that says why, its body starts with a caption, and `e`, `E` and
/// `:w` say it is read only instead of doing nothing.
#[gpui_kit::test]
fn the_activity_tab_is_marked_read_only(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // LLM-104 is the fourth row.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("enter", cx);
    })
    .unwrap();
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::History {
                key: "LLM-104".into(),
                history: history_for_status_change(),
                sessions: Vec::new(),
            },
            cx,
        )
    });
    let activity = cx.read(|cx| {
        let (_, tabs) = view.read(cx).open_tab(cx).unwrap();
        assert!(tabs[..tabs.len() - 1].iter().all(|t| t.read_only.is_none()));
        tabs.len() - 1
    });

    // The description's own tab has no mark and no caption.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("tab-read-only", activity)).is_some());
        assert!(window.try_find(("tab-read-only", 0usize)).is_none());
        assert!(window.try_find(("read-only-caption", 0usize)).is_none());
    })
    .unwrap();

    run(handle, &view, "tab activity", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("read-only-caption", activity)).is_some());
        window.press("e", cx);
        assert_ne!(
            view.read(cx).detail(cx).map(|d| d.mode),
            Some(crate::task::task_view::DetailMode::Editing)
        );
        assert_eq!(view.read(cx).message(), "Activity is read only");
    })
    .unwrap();
    view.update(cx, |this, _| this.set_message(String::new()));
    run(handle, &view, "w", cx);
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Activity is read only"));
    view.update(cx, |this, _| this.set_message(String::new()));
    cx.update_window(handle, |_, window, cx| window.press("shift-e", cx))
        .unwrap();
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Activity is read only"));

    // The tab's card says why.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.hover(("tab-read-only", activity), cx);
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("tooltip-card").is_some());
    })
    .unwrap();
}

/// Resizes the test window and draws a frame.
fn resize(handle: AnyWindowHandle, width: f32, cx: &mut TestAppContext) {
    use gpui_kit::{px, size};
    cx.simulate_window_resize(handle, size(px(width), px(760.)));
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

#[gpui_kit::test]
fn a_narrow_window_hides_the_sidebar_and_keeps_the_owners_choice(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("cmd-b", cx);
        window.press("h", cx);
        assert!(view.read(cx).sidebar_focused(window, cx));
    })
    .unwrap();

    // Too narrow: the sidebar hides and takes focus with it. The owner's
    // choice stays "open", so the saved setting does not change.
    resize(handle, 700., cx);
    cx.update_window(handle, |_, window, cx| {
        assert!(!view.read(cx).sidebar_shown());
        assert!(view.read(cx).sidebar_open());
        assert!(!view.read(cx).sidebar_focused(window, cx));
        assert!(view.read(cx).current_settings(cx).sidebar_open);
    })
    .unwrap();

    // Wide again: it comes back.
    resize(handle, 1200., cx);
    cx.read(|cx| assert!(view.read(cx).sidebar_shown()));
}

#[gpui_kit::test]
fn a_sidebar_the_owner_hid_stays_hidden_when_the_window_widens(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(cx.read(|cx| !view.read(cx).sidebar_open()));
    resize(handle, 700., cx);
    resize(handle, 1200., cx);
    cx.read(|cx| assert!(!view.read(cx).sidebar_shown()));
}

#[gpui_kit::test]
fn cmd_b_shows_the_sidebar_in_a_narrow_window_until_it_widens(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    resize(handle, 700., cx);
    cx.update_window(handle, |_, window, cx| {
        window.press("cmd-b", cx);
        window.render_frame(cx);
        assert!(view.read(cx).sidebar_shown());
    })
    .unwrap();

    // Widening keeps it, and the next narrow window hides it again.
    resize(handle, 1200., cx);
    cx.read(|cx| assert!(view.read(cx).sidebar_shown()));
    resize(handle, 700., cx);
    cx.read(|cx| assert!(!view.read(cx).sidebar_shown()));

    // Cmd-B in a narrow window with the sidebar hidden shows it, it never
    // flips the owner's choice to "closed".
    cx.update_window(handle, |_, window, cx| {
        window.press("cmd-b", cx);
        assert!(view.read(cx).sidebar_shown());
        window.press("cmd-b", cx);
        assert!(!view.read(cx).sidebar_shown());
        assert!(!view.read(cx).sidebar_open());
    })
    .unwrap();
}

/// The key of the task the task panel shows.
fn open_task(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| view.read(cx).open_task_key(cx).map(|k| k.to_string()))
}

fn open_key(key: &str, view: &Entity<Workspace>, cx: &mut TestAppContext) {
    cx.update(|cx| view.update(cx, |this, cx| this.open_key(key, cx)));
    cx.run_until_parked();
}

fn press(handle: AnyWindowHandle, keys: &[&str], cx: &mut TestAppContext) {
    for key in keys {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}

/// LLM-149: in the demo data LLM-101 is in App and LLM-102 in Server.
#[gpui_kit::test]
fn a_project_switch_closes_a_task_from_another_project(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    open_key("LLM-102", &view, cx);
    run(handle, &view, "project app", cx);
    assert_eq!(open_task(&view, cx), None, "the palette closes it");

    // A task the new project holds stays open.
    open_key("LLM-101", &view, cx);
    run(handle, &view, "team LLM", cx);
    assert_eq!(
        cx.read(|cx| view.read(cx).scope(cx)),
        Scope::Team("LLM".into())
    );
    assert_eq!(open_task(&view, cx).as_deref(), Some("LLM-101"));
    run(handle, &view, "show all", cx);
    assert_eq!(cx.read(|cx| view.read(cx).scope(cx)), Scope::All);
    assert_eq!(open_task(&view, cx).as_deref(), Some("LLM-101"));

    // The sidebar keys close it too.
    run(handle, &view, "project server", cx);
    assert_eq!(open_task(&view, cx), None);
    open_key("LLM-101", &view, cx);
    press(handle, &["cmd-b", "h", "k"], cx);
    assert_eq!(
        cx.read(|cx| view.read(cx).scope(cx)),
        Scope::Project("app".into())
    );
    assert_eq!(open_task(&view, cx).as_deref(), Some("LLM-101"));
    press(handle, &["j"], cx);
    assert_eq!(open_task(&view, cx), None, "the sidebar closes it");
}

#[gpui_kit::test]
fn a_saved_view_closes_a_task_it_does_not_hold(cx: &mut TestAppContext) {
    use crate::model::backend::{Backend, Event};

    let (handle, view) = open(cx);
    let (backend, _requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    let ids = |keys: &[&str], cx: &TestAppContext| -> Vec<llmtodo_core::IssueId> {
        cx.read(|cx| {
            keys.iter()
                .map(|k| view.read(cx).issue_by_key(k, cx).id)
                .collect()
        })
    };
    let bugs = ids(&["LLM-116", "LLM-119"], cx);
    let answer = |cx: &mut TestAppContext| {
        view.update(cx, |this, cx| {
            this.apply_event(
                Event::ViewIssues {
                    name: "Open bugs".into(),
                    ids: bugs.clone(),
                },
                cx,
            )
        })
    };

    open_key("LLM-101", &view, cx);
    run(handle, &view, "view open bugs", cx);
    assert_eq!(
        open_task(&view, cx).as_deref(),
        Some("LLM-101"),
        "it waits for the view's issues"
    );
    answer(cx);
    assert_eq!(open_task(&view, cx), None);

    // A task the view holds stays, and a later answer leaves it open.
    open_key("LLM-116", &view, cx);
    answer(cx);
    assert_eq!(open_task(&view, cx).as_deref(), Some("LLM-116"));
}

#[gpui_kit::test]
fn back_does_not_reopen_a_task_from_another_project(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "project app", cx);
    // The owner opens a Server task by key while App shows.
    open_key("LLM-102", &view, cx);
    run(handle, &view, "project server", cx);
    assert_eq!(open_task(&view, cx).as_deref(), Some("LLM-102"));
    run(handle, &view, "project app", cx);
    assert_eq!(open_task(&view, cx), None);

    press(handle, &["ctrl-o"], cx);
    assert_eq!(
        cx.read(|cx| view.read(cx).scope(cx)),
        Scope::Project("server".into())
    );
    assert_eq!(open_task(&view, cx).as_deref(), Some("LLM-102"));
    press(handle, &["ctrl-o"], cx);
    assert_eq!(
        cx.read(|cx| view.read(cx).scope(cx)),
        Scope::Project("app".into())
    );
    assert_eq!(open_task(&view, cx), None, "App never showed it by itself");
}

/// LLM-148: the task view's tabs grow with the zoom, like their text.
#[gpui_kit::test]
fn the_task_tabs_grow_with_the_zoom(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // LLM-104 is the fourth row.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("j", cx);
        window.press("enter", cx);
    })
    .unwrap();
    view.update(cx, |this, cx| {
        this.apply_event(
            crate::model::backend::Event::History {
                key: "LLM-104".into(),
                history: history_for_status_change(),
                sessions: Vec::new(),
            },
            cx,
        )
    });
    let height = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.find(("detail-tab", 0usize)).bounds().size.height
        })
        .unwrap()
    };
    let actual = height(cx);
    cx.update(|cx| crate::shell::theme::set_zoom(2.0, cx));
    let zoomed = height(cx);
    cx.update(|cx| crate::shell::theme::set_zoom(1.0, cx));
    // A click on a tab still opens it.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(("detail-tab", 1usize), cx);
    })
    .unwrap();
    cx.read(|cx| assert_eq!(view.read(cx).open_tab(cx).unwrap().0, 1));
    assert!(actual > gpui_kit::px(0.));
    // The bar's 1px border does not scale, so the tab is within a pixel of
    // twice its height.
    assert!(
        zoomed >= actual * 2. - gpui_kit::px(1.) && zoomed <= actual * 2. + gpui_kit::px(1.),
        "{actual:?} -> {zoomed:?}"
    );
}

/// LLM-156: adds the team WEB and links the project App to it too, so App
/// shows under both teams. No demo issue belongs to WEB.
fn add_web_team(view: &Entity<Workspace>, cx: &mut TestAppContext) {
    cx.update(|cx| {
        let store = view.read(cx).store().clone();
        store.update(cx, |store, cx| {
            let catalog = &mut store.catalog;
            let mut web = catalog.team_names[0].clone();
            web.key = "WEB".into();
            web.name = "Website".into();
            catalog.team_names.push(web);
            catalog
                .project_teams
                .insert("app".into(), vec!["LLM".into(), "WEB".into()]);
            cx.notify();
        });
    });
}

fn scope(view: &Entity<Workspace>, cx: &TestAppContext) -> Scope {
    cx.read(|cx| view.read(cx).scope(cx))
}

#[gpui_kit::test]
fn a_click_on_a_team_shows_all_its_issues(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "project server", cx);
    run(handle, &view, "sidebar", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("sidebar-team:LLM", cx);
        window.render_frame(cx);
        assert_eq!(view.read(cx).message(), "Showing llmtodo");
    })
    .unwrap();
    assert_eq!(scope(&view, cx), Scope::Team("LLM".into()));
    assert_eq!(shown(&view, cx).len(), 20);
}

#[gpui_kit::test]
fn the_keys_walk_a_project_under_its_second_team(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    add_web_team(&view, cx);
    // All, Inbox, LLM, App, Server, WEB, App again, then the saved view.
    press(handle, &["cmd-b", "h", "j", "j", "j", "j", "j"], cx);
    assert_eq!(scope(&view, cx), Scope::Team("WEB".into()));
    press(handle, &["j"], cx);
    assert_eq!(scope(&view, cx), Scope::Project("app".into()));
    // j goes on from the second App row, not back to Server.
    press(handle, &["j"], cx);
    assert_eq!(scope(&view, cx), Scope::View("Open bugs".into()));
    press(handle, &["k", "k"], cx);
    assert_eq!(scope(&view, cx), Scope::Team("WEB".into()));
}

#[gpui_kit::test]
fn a_click_on_the_second_app_row_moves_on_from_there(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    add_web_team(&view, cx);
    run(handle, &view, "sidebar", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // The second App row is row 6, after All, Inbox, LLM, App, Server
        // and WEB.
        window.click("sidebar-project:app-6", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(scope(&view, cx), Scope::Project("app".into()));
    press(handle, &["h", "k"], cx);
    assert_eq!(scope(&view, cx), Scope::Team("WEB".into()));
}

#[gpui_kit::test]
fn a_team_click_closes_a_task_from_another_team(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    add_web_team(&view, cx);
    run(handle, &view, "sidebar", cx);
    open_key("LLM-101", &view, cx);
    let click = |id: &'static str, cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    // The task's own team keeps it open.
    click("sidebar-team:LLM", cx);
    assert_eq!(open_task(&view, cx).as_deref(), Some("LLM-101"));
    click("sidebar-team:WEB", cx);
    assert_eq!(scope(&view, cx), Scope::Team("WEB".into()));
    assert_eq!(open_task(&view, cx), None);
}
