//! UI integration tests for more than one tracker: the sidebar rows, the
//! title bar badge, and keys that live in another tracker.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

use crate::model::backend::{Backend, Event, Snapshot};
use crate::model::trackers::Tracker;
use crate::shell::workspace::{Starter, Workspace};

fn tracker(name: &str, real: bool, teams: Option<&[&str]>) -> Tracker {
    Tracker {
        name: name.to_string().into(),
        data_dir: PathBuf::from(format!("/nowhere/{name}")),
        real,
        read_only: false,
        teams: teams.map(|t| t.iter().map(|k| k.to_string().into()).collect()),
    }
}

/// A workspace on demo data with a second, real tracker. The starter
/// records each tracker it starts and hands back a detached backend.
fn open(
    cx: &mut TestAppContext,
    real_teams: Option<&[&str]>,
) -> (AnyWindowHandle, Entity<Workspace>, Arc<Mutex<Vec<PathBuf>>>) {
    cx.update(crate::init);
    let started = Arc::new(Mutex::new(Vec::new()));
    let starter: Starter = {
        let started = started.clone();
        Arc::new(move |t: &Tracker| {
            started.lock().unwrap().push(t.data_dir.clone());
            let (backend, _requests) = Backend::detached();
            let (_tx, events) = futures::channel::mpsc::unbounded();
            (backend, events)
        })
    };
    let list = vec![
        tracker("Demo", false, None),
        tracker("Real tracker", true, real_teams),
    ];
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| {
                let mut this = Workspace::new(window, cx);
                this.set_trackers(list, starter, cx);
                this
            })
        })
        .unwrap()
    });
    (handle, view, started)
}

/// The real tracker's snapshot: one team, REAL, with REAL-13.
fn real_snapshot() -> Snapshot {
    use llmtodo_client::types::{Issue, Team};
    let team: Team = serde_json::from_value(serde_json::json!({
        "id": "01a1c453-1260-7779-be1c-619faeb9d420", "key": "REAL", "name": "Real",
        "created_at": "2026-11-01T10:00:00Z",
        "statuses": [
            {"id": "01a1c453-1260-7779-be1c-619faeb9d421", "team": "REAL", "name": "Todo", "type": "unstarted", "color": "#fff", "position": 1}
        ]
    }))
    .unwrap();
    let issue: Issue = serde_json::from_value(serde_json::json!({
        "id": "01a1c453-1260-7779-be1c-619faeb9d513",
        "key": "REAL-13", "team": "REAL", "title": "A real ticket",
        "status": {"id": "01a1c453-1260-7779-be1c-619faeb9d421", "name": "Todo", "type": "unstarted", "color": "#fff"},
        "priority": "none", "assignee": null, "labels": [], "project": null,
        "milestone": null, "parent": null, "wanted_executor": {}, "description": "Body",
        "description_version": 1, "claim": null, "created_by": null,
        "created_at": "2026-11-02T10:00:00Z", "updated_at": "2026-11-02T10:00:00Z",
        "completed_at": null, "deleted_at": null
    }))
    .unwrap();
    Snapshot {
        catalog: crate::model::backend::CatalogData {
            teams: vec![team],
            ..Default::default()
        },
        issues: vec![issue],
        questions: Vec::new(),
    }
}

#[gpui_kit::test]
fn a_click_on_a_tracker_row_opens_that_tracker(cx: &mut TestAppContext) {
    let (handle, view, started) = open(cx, None);
    cx.update_window(handle, |_, window, cx| {
        window.press("cmd-b", cx);
        window.render_frame(cx);
        // The open tracker is badged in the title bar.
        assert!(window.try_find("open-tracker").is_some());
        window.click("sidebar-tracker-1", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        *started.lock().unwrap(),
        [PathBuf::from("/nowhere/Real tracker")]
    );
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.trackers(cx).1, 1);
        // Nothing from the demo stays on screen under the real name.
        assert_eq!(this.visible_count(cx), 0);
        assert!(this.message().contains("real data"), "{}", this.message());
    });
}

/// LLM-111: kept text stays in the folder of its tracker, and the next
/// tracker does not show it.
#[gpui_kit::test]
fn a_switch_leaves_kept_text_with_its_tracker(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut drafts = crate::model::drafts::DraftStore::open(dir.path());
    drafts.insert("LLM-104".into(), "Owner text.\n".into());
    let (_, view, _) = open(cx, None);
    view.update(cx, |this, cx| {
        this.set_drafts(drafts);
        assert!(this.switch_tracker(1, cx));
        assert!(!this.drafts().contains_key("LLM-104"));
    });
    let kept = crate::model::drafts::DraftStore::open(dir.path());
    assert_eq!(kept.get("LLM-104").unwrap(), "Owner text.\n");
}

#[gpui_kit::test]
fn a_key_from_another_tracker_opens_there(cx: &mut TestAppContext) {
    let (_, view, started) = open(cx, Some(&["REAL"]));
    view.update(cx, |this, cx| this.open_key("REAL-13", cx));
    assert_eq!(started.lock().unwrap().len(), 1);
    view.update(cx, |this, cx| {
        assert_eq!(this.trackers(cx).1, 1);
        this.apply_event(Event::Snapshot(Box::new(real_snapshot())), cx);
    });
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(this.open_task_key(cx).as_deref(), Some("REAL-13"));
        // The snapshot taught the app the real tracker's teams.
        assert_eq!(
            this.trackers(cx).0[1].teams.as_deref(),
            Some(&["REAL".into()][..])
        );
    });
}

#[gpui_kit::test]
fn a_missing_key_of_a_local_team_stays_put(cx: &mut TestAppContext) {
    let (_, view, started) = open(cx, Some(&["REAL"]));
    // The demo has team LLM, so LLM-999 is just missing here.
    view.update(cx, |this, cx| this.open_key("LLM-999", cx));
    assert!(started.lock().unwrap().is_empty());
    cx.read(|cx| assert_eq!(view.read(cx).trackers(cx).1, 0));
}

#[gpui_kit::test]
fn probed_teams_route_keys(cx: &mut TestAppContext) {
    let (_, view, started) = open(cx, Some(&[]));
    // The real tracker holds no ZZ team, so ZZ-1 goes nowhere.
    view.update(cx, |this, cx| this.open_key("ZZ-1", cx));
    assert!(started.lock().unwrap().is_empty());
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Teams {
                data_dir: PathBuf::from("/nowhere/Real tracker"),
                teams: Some(vec!["ZZ".into()]),
            },
            cx,
        );
        this.open_key("ZZ-1", cx);
    });
    assert_eq!(started.lock().unwrap().len(), 1);
}

#[gpui_kit::test]
fn the_tracker_command_switches(cx: &mut TestAppContext) {
    let (handle, view, started) = open(cx, None);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            this.run_command("tracker real", window, cx);
            assert_eq!(this.trackers(cx).1, 1);
            this.run_command("tracker demo", window, cx);
            assert_eq!(this.trackers(cx).1, 0);
            this.run_command("tracker nope", window, cx);
            assert!(this.message().contains("Unknown tracker"));
        })
    })
    .unwrap();
    assert_eq!(started.lock().unwrap().len(), 2);
}

/// LLM-137: every edit on a read-only tracker stops in the app. Nothing goes
/// to the server, and the message says why.
#[gpui_kit::test]
fn a_read_only_tracker_sends_no_writes(cx: &mut TestAppContext) {
    use crate::model::backend::Request;
    cx.update(crate::init);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let starter: Starter = {
        let requests = requests.clone();
        Arc::new(move |_: &Tracker| {
            let (backend, rx) = Backend::detached();
            requests.lock().unwrap().push(rx);
            let (_tx, events) = futures::channel::mpsc::unbounded();
            (backend, events)
        })
    };
    let mut real = tracker("Real tracker", true, Some(&["REAL"]));
    real.read_only = true;
    let list = vec![tracker("Demo", false, None), real];
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| {
                let mut this = Workspace::new(window, cx);
                this.set_trackers(list, starter, cx);
                this
            })
        })
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            assert!(this.switch_tracker(1, cx));
            assert!(this.message().contains("read-only"), "{}", this.message());
            this.apply_event(Event::Snapshot(Box::new(real_snapshot())), cx);
            this.open_key("REAL-13", cx);
            assert_eq!(this.open_task_key(cx).as_deref(), Some("REAL-13"));
            for command in [
                "status Todo",
                "assign me",
                "unassign",
                "restore",
                "release",
                "view save Mine",
                "view delete Mine",
            ] {
                this.run_command(command, window, cx);
                assert!(
                    this.message().contains("read-only"),
                    "{command}: {}",
                    this.message()
                );
            }
            assert!(!this.create_issue("New issue", "", window, cx));
        })
    })
    .unwrap();
    let mut requests = requests.lock().unwrap();
    while let Ok(request) = requests[0].try_recv() {
        assert!(
            !matches!(
                request,
                Request::Write { .. }
                    | Request::Create(_)
                    | Request::CreateView(_)
                    | Request::DeleteView { .. }
                    | Request::SaveDocument { .. }
            ),
            "{request:?}"
        );
    }
}

/// LLM-137: a new window opens on its own data, whichever tracker the last
/// one showed.
#[gpui_kit::test]
fn a_new_window_opens_on_its_own_tracker(cx: &mut TestAppContext) {
    use crate::settings::{SettingsStore, UiSettings};
    cx.update(crate::init);
    let dir = tempfile::tempdir().unwrap();
    let store = SettingsStore::at(dir.path().join("ui.json"));
    let starter: Starter = Arc::new(|_: &Tracker| {
        let (backend, _requests) = Backend::detached();
        let (_tx, events) = futures::channel::mpsc::unbounded();
        (backend, events)
    });
    let list = vec![
        tracker("Demo", false, None),
        tracker("Real tracker", true, None),
    ];
    let open_window = |saved: UiSettings, cx: &mut TestAppContext| {
        let (store, starter, list) = (store.clone(), starter.clone(), list.clone());
        cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
                cx.new(|cx| {
                    let mut this = Workspace::with_settings(saved, store, window, cx);
                    this.set_trackers(list, starter, cx);
                    this
                })
            })
            .unwrap()
        })
    };
    let (handle, view) = open_window(UiSettings::default(), cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            assert!(this.switch_tracker(1, cx));
            this.run_command("sort status", window, cx);
        })
    })
    .unwrap();
    let saved = store.load();
    assert_eq!(saved.sort, "status", "the settings were saved");
    let (_, view) = open_window(saved, cx);
    cx.read(|cx| assert_eq!(view.read(cx).trackers(cx).1, 0));
}

#[gpui_kit::test]
fn one_tracker_shows_no_tracker_rows(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (handle, _) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
        window.press("cmd-b", cx);
        window.render_frame(cx);
        assert!(!window.try_find("sidebar-tracker-0").is_some());
        assert!(!window.try_find("open-tracker").is_some());
    })
    .unwrap();
}
