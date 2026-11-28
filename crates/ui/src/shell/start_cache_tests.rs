//! LLM-625: the app shows the last run's data at start. The rows are
//! dimmed under a sweep, and edits wait, until the live load replaces them.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use llmtodo_api_types::Priority;
use llmtodo_client::types::{Issue, Team};

use crate::issues::list::Stale;
use crate::model::backend::{Backend, CatalogData, Event, Request, Snapshot};
use crate::settings::{SettingsStore, UiSettings};
use crate::shell::workspace::Workspace;

/// A workspace with no data, as at start, and the requests it sends.
fn open(
    cx: &mut TestAppContext,
) -> (
    AnyWindowHandle,
    Entity<Workspace>,
    tokio::sync::mpsc::UnboundedReceiver<Request>,
) {
    open_with(UiSettings::default(), cx)
}

/// The same, from saved settings.
fn open_with(
    settings: UiSettings,
    cx: &mut TestAppContext,
) -> (
    AnyWindowHandle,
    Entity<Workspace>,
    tokio::sync::mpsc::UnboundedReceiver<Request>,
) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::with_settings(settings, SettingsStore::memory(), window, cx))
        })
        .expect("open test window")
    });
    // The settings set motion as the window opens.
    cx.update(|cx| crate::shell::motion::set(true, cx));
    let (backend, requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    (handle, view, requests)
}

fn team() -> Team {
    team_named("ABC", 0)
}

/// Team `key`. Each `n` gives it its own IDs.
fn team_named(key: &str, n: u32) -> Team {
    serde_json::from_value(serde_json::json!({
        "id": format!("01a1c453-1260-7779-be1c-619faeb9d4{n}0"), "key": key, "name": key.to_lowercase(),
        "created_at": "2026-11-01T10:00:00Z",
        "statuses": [
            {"id": format!("01a1c453-1260-7779-be1c-619faeb9d4{n}1"), "team": key, "name": "Todo", "type": "unstarted", "color": "#fff", "position": 1}
        ]
    }))
    .unwrap()
}

fn issue(n: u32, title: &str) -> Issue {
    serde_json::from_value(serde_json::json!({
        "id": format!("01a1c453-1260-7779-be1c-619faeb9d5{n:02}"),
        "key": format!("ABC-{n}"), "team": "ABC", "title": title,
        "status": {"id": "01a1c453-1260-7779-be1c-619faeb9d421", "name": "Todo", "type": "unstarted", "color": "#fff"},
        "priority": "none", "assignee": null, "labels": [], "project": null,
        "milestone": null, "parent": null, "wanted_executor": {}, "description": null,
        "description_version": 1, "claim": null, "created_by": null,
        "created_at": "2026-11-02T10:00:00Z", "updated_at": "2026-11-02T10:00:00Z",
        "completed_at": null, "deleted_at": null
    }))
    .unwrap()
}

fn snapshot(issues: Vec<Issue>) -> Snapshot {
    Snapshot {
        catalog: CatalogData {
            teams: vec![team()],
            ..Default::default()
        },
        issues,
        questions: Vec::new(),
    }
}

/// Draws a frame, and says what the list drew and whether the sweep shows.
fn frame(
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    cx: &mut TestAppContext,
) -> (Stale, bool) {
    let sweep = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("list-sweep").is_some()
        })
        .unwrap();
    let stale = cx.read(|cx| view.read(cx).list().read(cx).stale_shown);
    (stale, sweep)
}

fn title(view: &Entity<Workspace>, cx: &mut TestAppContext) -> String {
    cx.read(|cx| view.read(cx).issue_by_key("ABC-1", cx).title.to_string())
}

#[gpui_kit::test]
fn the_cache_shows_dimmed_until_the_live_load_replaces_it(cx: &mut TestAppContext) {
    let (handle, view, mut requests) = open(cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Cached(Box::new(snapshot(vec![issue(1, "Old")]).load())),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(title(&view, cx), "Old", "the cache fills the list at once");
    cx.read(|cx| {
        let store = view.read(cx).store().read(cx);
        assert!(store.cached);
        assert_eq!(
            store.catalog.team_names[0].key.as_ref(),
            "ABC",
            "the sidebar has its teams"
        );
    });
    let (stale, sweep) = frame(handle, &view, cx);
    assert!(stale.opacity() < 1., "the cached rows are dimmed");
    assert!(sweep, "the sweep shows while the cache does");

    // An edit waits for the live data.
    while requests.try_recv().is_ok() {}
    view.update(cx, |this, cx| {
        let id = this.issue_by_key("ABC-1", cx).id;
        this.set_priority(id, Priority::Urgent, cx);
    });
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(
            this.issue_by_key("ABC-1", cx).priority,
            crate::model::data::Priority::None
        );
        assert!(
            this.message().starts_with("Still loading"),
            "{}",
            this.message()
        );
    });
    assert!(
        !std::iter::from_fn(|| requests.try_recv().ok())
            .any(|r| matches!(r, Request::Write { .. })),
        "no write goes out"
    );

    // The live load replaces the cache, and the rows fade back in.
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Snapshot(Box::new(snapshot(vec![issue(1, "New")]).load())),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(title(&view, cx), "New");
    cx.read(|cx| assert!(!view.read(cx).store().read(cx).cached));
    let (stale, sweep) = frame(handle, &view, cx);
    assert!(
        stale.dim > 0. && sweep,
        "the fade starts from the dimmed rows"
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    let (stale, sweep) = frame(handle, &view, cx);
    assert_eq!(stale, Stale::default(), "the rows are at full strength");
    assert!(!sweep);

    // Edits work again.
    view.update(cx, |this, cx| {
        let id = this.issue_by_key("ABC-1", cx).id;
        this.set_priority(id, Priority::Urgent, cx);
    });
    assert!(
        std::iter::from_fn(|| requests.try_recv().ok()).any(|r| matches!(r, Request::Write { .. })),
        "the edit goes out"
    );
}

#[gpui_kit::test]
fn a_cache_that_comes_after_the_live_load_is_dropped(cx: &mut TestAppContext) {
    let (_, view, _requests) = open(cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Snapshot(Box::new(snapshot(vec![issue(1, "New")]).load())),
            cx,
        );
        this.apply_event(
            Event::Cached(Box::new(snapshot(vec![issue(1, "Old")]).load())),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(title(&view, cx), "New");
    cx.read(|cx| assert!(!view.read(cx).store().read(cx).cached));
}

#[gpui_kit::test]
fn with_animations_off_the_cache_is_dimmed_without_a_sweep(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    cx.update(|cx| crate::shell::motion::set(false, cx));
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Cached(Box::new(snapshot(vec![issue(1, "Old")]).load())),
            cx,
        );
    });
    let (stale, sweep) = frame(handle, &view, cx);
    assert!(stale.opacity() < 1. && !sweep);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Snapshot(Box::new(snapshot(vec![issue(1, "New")]).load())),
            cx,
        );
    });
    let (stale, _) = frame(handle, &view, cx);
    assert_eq!(stale, Stale::default(), "the rows come back at once");
}

/// The cache may be older than the server. The saved view and the last
/// task, made since, stay saved and open once the live load has them.
#[gpui_kit::test]
fn a_cache_without_the_saved_view_and_task_keeps_them_for_the_live_load(cx: &mut TestAppContext) {
    use crate::model::scope::Scope;
    let settings = UiSettings {
        open_last_view: true,
        sidebar_scope: "team:NEW".into(),
        last_task: Some("NEW-1".into()),
        ..Default::default()
    };
    let (_, view, _requests) = open_with(settings, cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Cached(Box::new(snapshot(vec![issue(1, "Old")]).load())),
            cx,
        );
    });
    cx.run_until_parked();
    let scope = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).list().read(cx).scope.clone());
    assert_eq!(
        scope(cx),
        Scope::Team("NEW".into()),
        "the cache resets no view"
    );

    let mut new: Issue = issue(2, "Made since");
    new.key = "NEW-1".parse().unwrap();
    new.team = "NEW".into();
    new.status.id = team_named("NEW", 1).statuses[0].id;
    let mut live = snapshot(vec![issue(1, "Old"), new]);
    live.catalog.teams.push(team_named("NEW", 1));
    view.update(cx, |this, cx| {
        this.apply_event(Event::Snapshot(Box::new(live.load())), cx)
    });
    cx.run_until_parked();
    assert_eq!(scope(cx), Scope::Team("NEW".into()));
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).open_task_key(cx).as_deref(),
            Some("NEW-1"),
            "the last task opens from the live load"
        );
    });
}

/// Moving or coloring a project waits for the live data too.
#[gpui_kit::test]
fn project_edits_wait_while_the_cache_shows(cx: &mut TestAppContext) {
    let (_, view, mut requests) = open(cx);
    let project = |slug: &str, n: u32| -> llmtodo_client::types::Project {
        serde_json::from_value(serde_json::json!({
            "id": format!("01a1c453-1260-7779-be1c-619faeb9d6{n:02}"), "slug": slug, "name": slug,
            "description": "", "state": "started", "teams": ["ABC"], "repos": [],
            "position": n, "created_at": "2026-11-01T10:00:00Z",
            "updated_at": "2026-11-01T10:00:00Z", "deleted_at": null
        }))
        .unwrap()
    };
    let mut cached = snapshot(vec![issue(1, "Old")]);
    cached.catalog.projects = vec![project("one", 1), project("two", 2)];
    view.update(cx, |this, cx| {
        this.apply_event(Event::Cached(Box::new(cached.load())), cx);
    });
    while requests.try_recv().is_ok() {}
    view.update(cx, |this, cx| {
        let moved = this.move_project("one", "two", cx);
        assert!(moved.starts_with("Still loading"), "{moved}");
        let colored = this.set_project_color("one", Some("#eb5757".into()), cx);
        assert!(colored.starts_with("Still loading"), "{colored}");
        let projects = &this.store().read(cx).catalog.projects;
        assert_eq!(projects[0].key.as_ref(), "one", "the order stays");
    });
    assert!(
        std::iter::from_fn(|| requests.try_recv().ok())
            .next()
            .is_none(),
        "nothing goes out"
    );
}

/// A tracker switch while the cache shows plays no fade on the new one.
#[gpui_kit::test]
fn a_new_connection_plays_no_fade(cx: &mut TestAppContext) {
    let (handle, view, _requests) = open(cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Cached(Box::new(snapshot(vec![issue(1, "Old")]).load())),
            cx,
        );
        let (backend, _) = Backend::detached();
        let (_, events) = futures::channel::mpsc::unbounded();
        this.connect(backend, events, cx);
    });
    let (stale, sweep) = frame(handle, &view, cx);
    assert_eq!(stale, Stale::default());
    assert!(!sweep);
}
