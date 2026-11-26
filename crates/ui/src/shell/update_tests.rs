//! Restart to update: the button shows for a new installed build, and a
//! click keeps the open editor's text before it hands over to the helper.

use std::sync::Mutex;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};

use crate::model::update::{Restart, UpdateWatch};
use crate::shell::workspace::Workspace;

static LAUNCHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(restart: &Restart) -> std::io::Result<()> {
    LAUNCHED
        .lock()
        .unwrap()
        .push(restart.bundle.display().to_string());
    Ok(())
}

static MENU_LAUNCHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record_menu(restart: &Restart) -> std::io::Result<()> {
    MENU_LAUNCHED
        .lock()
        .unwrap()
        .push(restart.bundle.display().to_string());
    Ok(())
}

fn install(bundle: &std::path::Path, commit: &str) {
    std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
    std::fs::write(bundle.join(llmtodo_core::bundle::COMMIT_FILE), commit).unwrap();
}

#[gpui_kit::test]
fn a_new_build_shows_the_button_and_a_click_keeps_the_text(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("llmtodo.app");
    install(&bundle, "own");
    cx.update(crate::init);
    let (handle, view) = cx
        .update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(window, cx))
            })
        })
        .unwrap();
    let watch = UpdateWatch {
        bundle: bundle.clone(),
        own: "own".into(),
    };
    view.update(cx, |this, cx| {
        this.watch_for_updates(watch, dir.path().to_path_buf(), None, cx);
        this.set_update_launcher(record);
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| view.read(cx).update_ready().map(String::from)),
        None
    );

    // A publish installs another commit. The next check finds it, and
    // the watch ends there (LLM-344).
    assert!(cx.read(|cx| view.read(cx).update_watching()));
    install(&bundle, "newer");
    cx.executor()
        .advance_clock(crate::shell::workspace::CHECK_EVERY / 2);
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| view.read(cx).update_ready().map(String::from)),
        None,
        "the check runs only every CHECK_EVERY"
    );
    cx.executor()
        .advance_clock(crate::shell::workspace::CHECK_EVERY);
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| view.read(cx).update_ready().map(String::from))
            .as_deref(),
        Some("newer")
    );
    assert!(!cx.read(|cx| view.read(cx).update_watching()));

    // LLM-104 is free to edit. The owner types, then clicks the button.
    let key = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            // The button is solid gold at rest, not a faint tint.
            let bounds = window
                .find("restart-update")
                .bounds()
                .scale(window.scale_factor());
            let fill = window
                .painted_quads()
                .iter()
                .find(|q| q.bounds == bounds)
                .and_then(|q| q.background.as_solid())
                .expect("a filled quad at the button's bounds");
            assert_eq!(fill, crate::shell::workspace::UPDATE_FILL);
            window.press("j", cx);
            window.press("j", cx);
            window.press("j", cx);
            window.press("enter", cx);
            window.press("e", cx);
            let editor = view.read(cx).editor(cx).clone();
            editor.update(cx, |s, cx| s.set_value("Unsaved owner text.\n", window, cx));
            view.read(cx).open_task_key(cx).unwrap()
        })
        .unwrap();
    view.update(cx, |this, cx| this.restart_to_update(cx));
    cx.run_until_parked();

    let kept = cx.read(|cx| view.read(cx).drafts().get(&key).cloned());
    assert_eq!(kept.as_deref(), Some("Unsaved owner text.\n"));
    let launched = LAUNCHED.lock().unwrap().clone();
    assert_eq!(launched, [bundle.display().to_string()]);
}

/// The X hides the button until the next start. Check for Updates in the
/// app menu still restarts onto the new build (LLM-564).
#[gpui_kit::test]
fn the_x_hides_the_button_and_the_menu_still_restarts(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("llmtodo.app");
    install(&bundle, "own");
    cx.update(crate::init);
    let (handle, view) = cx
        .update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| Workspace::new(window, cx))
            })
        })
        .unwrap();
    let watch = UpdateWatch {
        bundle: bundle.clone(),
        own: "own".into(),
    };
    view.update(cx, |this, cx| {
        this.watch_for_updates(watch, dir.path().to_path_buf(), None, cx);
        this.set_update_launcher(record_menu);
    });
    cx.run_until_parked();

    // No new build: the menu says so and nothing restarts.
    cx.update_window(handle, |_, window, cx| {
        window.dispatch_action(Box::new(crate::shell::app_menus::MenuCheckForUpdates), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| view.read(cx).message().to_string()),
        "llmtodo is up to date"
    );
    assert!(!cx.read(|cx| view.read(cx).update_restarting()));

    install(&bundle, "newer");
    cx.executor()
        .advance_clock(crate::shell::workspace::CHECK_EVERY * 2);
    cx.run_until_parked();
    assert!(cx.read(|cx| view.read(cx).update_button_shown()));

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // One split button: two gold halves of one height, with a 1px gap
        // that shows the title bar between them.
        let restart = window.find("restart-update").bounds();
        let close = window.find("dismiss-update").bounds();
        assert_eq!(close.left() - restart.right(), gpui_kit::px(1.));
        assert_eq!(close.size.height, restart.size.height);
        assert_eq!(close.top(), restart.top());
        let scale = window.scale_factor();
        let fill = |b: gpui_kit::Bounds<gpui_kit::Pixels>| {
            let b = b.scale(scale);
            window
                .painted_quads()
                .iter()
                .find(|q| q.bounds == b)
                .and_then(|q| q.background.as_solid())
        };
        assert_eq!(fill(close), Some(crate::shell::workspace::UPDATE_FILL));
        assert_eq!(fill(restart), Some(crate::shell::workspace::UPDATE_FILL));
        window.click("dismiss-update", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!cx.read(|cx| view.read(cx).update_button_shown()));
    assert!(!cx.read(|cx| view.read(cx).update_restarting()));

    cx.update_window(handle, |_, window, cx| {
        window.dispatch_action(Box::new(crate::shell::app_menus::MenuCheckForUpdates), cx)
    })
    .unwrap();
    cx.run_until_parked();
    let launched = MENU_LAUNCHED.lock().unwrap().clone();
    assert_eq!(launched, [bundle.display().to_string()]);
}
