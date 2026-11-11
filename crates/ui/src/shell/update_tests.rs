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

    // A publish installs another commit. The next check finds it.
    install(&bundle, "newer");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(6));
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| view.read(cx).update_ready().map(String::from))
            .as_deref(),
        Some("newer")
    );

    // LLM-104 is free to edit. The owner types, then clicks the button.
    let key = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
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
