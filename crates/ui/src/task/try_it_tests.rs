//! LLM-591: Try it in the review panel opens the ticket's preview build.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};
use llmtodo_api_types::PreviewBuild;

use crate::model::backend::Backend;
use crate::model::data::{IssueId, Status, StatusType, WaitingOn};
use crate::model::try_it::RunState;
use crate::shell::workspace::Workspace;

const SECTION: &str =
    "+++ Review\n### Steps to reproduce\n1. Click Try it.\n\n### What to check\n- It opens.\n+++\n";

/// Opens the app with LLM-105 in User review, and opens LLM-105.
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>, IssueId) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .unwrap()
    });
    let (backend, _requests) = Backend::detached();
    view.update(cx, |this, _| this.attach_backend(backend));
    cx.run_until_parked();
    let id = cx.read(|cx| view.read(cx).store().read(cx).by_key("LLM-105").unwrap().id);
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, _| {
            store.modify(id, |i| {
                i.status = Status::new("User review", StatusType::Started, 9).with_review(true);
                i.description = SECTION.into();
                i.waiting_on = Some(WaitingOn::Owner);
                i.claim = None;
            });
        });
    });
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command("open llm-105", window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    (handle, view, id)
}

/// A checkout under `root` with a preview build of LLM-105, whose
/// `scripts/preview` is `script`. Returns the build's app path.
fn checkout(root: &Path, script: &str) -> PathBuf {
    let dir = root.join("target/preview/LLM-105");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(dir.join("llmtodo-app"), "").unwrap();
    std::fs::write(dir.join("llmtodo"), "").unwrap();
    let path = root.join("scripts/preview");
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir.join("llmtodo-app")
}

fn record(view: &Entity<Workspace>, id: IssueId, app: &Path, stale: bool, cx: &mut TestAppContext) {
    let build = PreviewBuild {
        path: app.to_string_lossy().into_owned(),
        commit: "a".repeat(40),
        host: None,
        built_at: jiff::Timestamp::UNIX_EPOCH,
        stale,
    };
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, cx| {
            store.set_preview_build(id, Some(build));
            cx.notify();
        });
    });
    cx.run_until_parked();
}

fn state(view: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<RunState> {
    cx.read(|cx| view.read(cx).task_view().read(cx).previews.state("LLM-105"))
}

/// Waits on the preview's own thread, which the test executor does not run.
fn wait_for(
    view: &Entity<Workspace>,
    cx: &mut TestAppContext,
    what: &str,
    done: impl Fn(Option<RunState>) -> bool,
) {
    let start = Instant::now();
    loop {
        cx.run_until_parked();
        if done(state(view, cx)) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui_kit::test]
fn no_preview_build_shows_no_try_it(cx: &mut TestAppContext) {
    let (handle, _view, _) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find("review-panel");
        assert!(window.try_find("review-try-it").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_stale_or_missing_build_turns_try_it_off_with_a_reason(cx: &mut TestAppContext) {
    let (handle, view, id) = open(cx);
    let tmp = tempfile::tempdir().unwrap();
    let app = checkout(tmp.path(), "#!/bin/sh\nexit 1\n");
    record(&view, id, &app, true, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find("review-try-it");
        window.find("review-try-it-note");
        // A click on the button that is off starts nothing.
        window.click("review-try-it", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(state(&view, cx), None);
    let text = crate::task::review::TryIt::Off(crate::model::try_it::STALE);
    let shown = cx.read(|cx| {
        let store = view.read(cx).store().read(cx);
        let issue = store.get(id).unwrap();
        crate::task::review::try_it_state(
            issue,
            store,
            &view.read(cx).task_view().read(cx).previews,
        )
    });
    assert_eq!(shown, Some(text));

    record(&view, id, &tmp.path().join("gone/llmtodo-app"), false, cx);
    let shown = cx.read(|cx| {
        let store = view.read(cx).store().read(cx);
        let issue = store.get(id).unwrap();
        crate::task::review::try_it_state(
            issue,
            store,
            &view.read(cx).task_view().read(cx).previews,
        )
    });
    assert_eq!(
        shown,
        Some(crate::task::review::TryIt::Off(
            crate::model::try_it::MISSING
        ))
    );
}

#[gpui_kit::test]
fn try_it_starts_the_preview_once_and_it_ends_when_the_app_quits(cx: &mut TestAppContext) {
    let (handle, view, id) = open(cx);
    let tmp = tempfile::tempdir().unwrap();
    let gate = tmp.path().join("quit");
    let count = tmp.path().join("runs");
    // Stands in for scripts/preview: it counts its runs, says the app is
    // open, and waits until the test quits the app.
    let script = format!(
        "#!/bin/sh\necho run >> '{}'\necho PREVIEW_DATA=$TMPDIR/x\n\
         while [ ! -f '{}' ]; do sleep 0.05; done\n",
        count.display(),
        gate.display()
    );
    let app = checkout(tmp.path(), &script);
    record(&view, id, &app, false, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("review-try-it-note").is_none());
        window.click("review-try-it", cx);
    })
    .unwrap();
    wait_for(&view, cx, "open", |s| s == Some(RunState::Running));
    // A second click brings the open preview forward and starts no other.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("review-try-it", cx);
    })
    .unwrap();
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(std::fs::read_to_string(&count).unwrap(), "run\n");
    std::fs::write(&gate, "").unwrap();
    wait_for(&view, cx, "ended", |s| s.is_none());
}

#[gpui_kit::test]
fn a_failed_start_shows_why_under_try_it(cx: &mut TestAppContext) {
    let (handle, view, id) = open(cx);
    let tmp = tempfile::tempdir().unwrap();
    let app = checkout(
        tmp.path(),
        "#!/bin/sh\necho 'demo seed failed' >&2\nexit 1\n",
    );
    record(&view, id, &app, false, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("review-try-it", cx);
    })
    .unwrap();
    wait_for(&view, cx, "failed", |s| {
        matches!(s, Some(RunState::Failed(_)))
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.find("review-try-it-note");
    })
    .unwrap();
}
