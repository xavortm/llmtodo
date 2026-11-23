//! LLM-470: the saving, loading and database-unreachable states. Every
//! wait shows only after its delay, so the local SQLite file, which answers
//! in milliseconds, never flashes one.

use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
use llmtodo_api_types::Priority;

use crate::model::activity::{SHOW_AFTER, Wait};
use crate::model::backend::{Backend, Event, WriteKind};
use crate::model::store::Connection;
use crate::shell::workspace::Workspace;

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .expect("open test window")
    });
    // A server the test plays: requests go nowhere, answers come as events.
    let (backend, requests) = Backend::detached();
    std::mem::forget(requests);
    view.update(cx, |this, cx| {
        this.attach_backend(backend);
        this.apply_event(
            Event::Connected {
                url: "http://127.0.0.1:4567".into(),
                database: None,
            },
            cx,
        );
    });
    (handle, view)
}

fn has(handle: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) -> bool {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
    .unwrap()
}

fn wait(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<Wait> {
    cx.read(|cx| view.read(cx).wait_shown(cx))
}

fn advance(by: Duration, cx: &mut TestAppContext) {
    cx.executor().advance_clock(by);
    cx.run_until_parked();
}

/// Changes LLM-101's priority, which sends one write. Returns its key.
fn write(view: &Entity<Workspace>, cx: &mut TestAppContext) -> String {
    view.update(cx, |this, cx| {
        let id = this.issue_by_key("LLM-101", cx).id;
        this.set_priority(id, Priority::Urgent, cx);
    });
    "LLM-101".into()
}

fn done(view: &Entity<Workspace>, key: &str, cx: &mut TestAppContext) {
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::WriteDone {
                key: key.into(),
                kind: WriteKind::Priority,
                version: 1,
            },
            cx,
        )
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_quick_save_never_shows_saving(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let key = write(&view, cx);
    assert!(!has(handle, "status-wait", cx));
    advance(Duration::from_millis(20), cx);
    done(&view, &key, cx);
    advance(Duration::from_secs(2), cx);
    assert_eq!(wait(&view, cx), None);
    assert!(!has(handle, "status-wait", cx), "nothing flashed");
    assert!(!has(handle, "row-saving", cx));
}

#[gpui_kit::test]
fn a_slow_save_shows_on_its_row_and_in_the_status_bar(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let key = write(&view, cx);
    advance(SHOW_AFTER - Duration::from_millis(50), cx);
    assert!(!has(handle, "status-wait", cx), "not before the delay");
    assert!(!has(handle, "row-saving", cx));
    advance(Duration::from_millis(100), cx);
    assert_eq!(wait(&view, cx), Some(Wait::Saving));
    assert!(has(handle, "status-wait", cx));
    assert!(has(handle, "row-saving", cx), "the changed row shows it");
    done(&view, &key, cx);
    assert_eq!(wait(&view, cx), None);
    assert!(!has(handle, "status-wait", cx));
    assert!(!has(handle, "row-saving", cx));
}

#[gpui_kit::test]
fn a_failed_slow_save_clears_the_saving_state(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let key = write(&view, cx);
    advance(SHOW_AFTER * 2, cx);
    assert!(has(handle, "row-saving", cx));
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::WriteFailed {
                key,
                kind: WriteKind::Priority,
                message: "refused".into(),
                stale: false,
                summary: None,
            },
            cx,
        )
    });
    assert!(!has(handle, "row-saving", cx));
    assert!(!has(handle, "status-wait", cx));
    cx.read(|cx| assert_eq!(view.read(cx).message(), "Could not save LLM-101: refused"));
}

#[gpui_kit::test]
fn a_lost_database_reads_apart_from_an_offline_server(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::DatabaseDown {
                host: Some("ep-calm.neon.tech/neondb".into()),
                reason: "timed out".into(),
            },
            cx,
        )
    });
    cx.read(|cx| {
        let this = view.read(cx);
        assert_eq!(
            this.message(),
            "Shared database unreachable at ep-calm.neon.tech/neondb: timed out. Retrying"
        );
        assert!(matches!(
            this.store().read(cx).connection,
            Connection::DatabaseDown { .. }
        ));
    });
    assert!(has(handle, "status-database-down", cx));
    // An empty list says so too.
    view.update(cx, |this, cx| {
        this.load(Default::default(), Vec::new(), cx);
    });
    cx.read(|cx| {
        let text = view.read(cx).list().read(cx).empty_text(cx);
        assert_eq!(
            text.as_deref(),
            Some("Shared database unreachable at ep-calm.neon.tech/neondb. Retrying")
        );
    });
    // Back online, the message goes.
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Connected {
                url: "http://127.0.0.1:4567".into(),
                database: None,
            },
            cx,
        )
    });
    cx.read(|cx| assert_eq!(view.read(cx).message(), ""));
    assert!(!has(handle, "status-database-down", cx));
}

#[gpui_kit::test]
fn the_first_load_counts_its_pages(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    view.update(cx, |this, cx| {
        this.load(Default::default(), Vec::new(), cx);
        this.apply_event(
            Event::Loading {
                loaded: 0,
                total: None,
            },
            cx,
        );
        this.apply_event(
            Event::Loading {
                loaded: 15_000,
                total: Some(50_000),
            },
            cx,
        );
    });
    let empty = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            view.read(cx)
                .list()
                .read(cx)
                .empty_text(cx)
                .map(|t| t.to_string())
        })
    };
    // The empty list says it at once: it stands in for the rows.
    assert_eq!(
        empty(cx).as_deref(),
        Some("Loading issues… 15,000 of 50,000")
    );
    // The status bar only after the delay.
    assert!(!has(handle, "status-wait", cx));
    advance(SHOW_AFTER, cx);
    assert_eq!(
        wait(&view, cx),
        Some(Wait::Loading {
            loaded: 15_000,
            total: Some(50_000)
        })
    );
    assert!(has(handle, "status-wait", cx));
}

#[gpui_kit::test]
fn the_first_connect_to_a_shared_database_says_it_is_waking(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .expect("open test window")
    });
    let (backend, requests) = Backend::detached();
    std::mem::forget(requests);
    let (_tx, events) = futures::channel::mpsc::unbounded();
    view.update(cx, |this, cx| {
        this.set_shared_dir(Some("/shared".into()), cx);
        this.connect(backend, events, cx);
    });
    advance(Duration::from_millis(100), cx);
    assert_eq!(wait(&view, cx), None, "a quick connect shows nothing");
    advance(SHOW_AFTER, cx);
    assert_eq!(wait(&view, cx), Some(Wait::Waking));
    assert!(has(handle, "status-wait", cx));
    view.update(cx, |this, cx| {
        this.apply_event(
            Event::Connected {
                url: "http://127.0.0.1:4567".into(),
                database: None,
            },
            cx,
        )
    });
    assert_eq!(wait(&view, cx), None);
}
