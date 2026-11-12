//! LLM-169: the list, the task view and the other main views are cached.
//! A cat frame or an update-button frame must not draw them again, and
//! every change they show must still draw them again.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, TestAppContext, Window, WindowBounds,
    WindowOptions, WindowVisibility, px, size,
};

use crate::docs::DocView;
use crate::inbox::InboxView;
use crate::issues::list::IssueList;
use crate::model::update::UpdateWatch;
use crate::render_count;
use crate::settings::view::SettingsView;
use crate::shell::mascot::Mascot;
use crate::shell::sidebar::Sidebar;
use crate::shell::workspace::{UpdateButton, Workspace};
use crate::task::task_view::TaskView;

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1400.), px(900.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("open test window")
    })
}

/// Opens the window with the sidebar, the cat and a task open, and draws
/// it once.
fn open_with_task(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        if !view.read(cx).sidebar_shown() {
            window.press("cmd-b", cx);
        }
        window.press("j", cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| {
        assert!(view.read(cx).sidebar_shown());
        assert!(view.read(cx).detail(cx).is_some());
    });
    (handle, view)
}

/// Draws one frame as the platform would: only dirty views render.
/// `render_frame` refreshes the whole window first, so every view renders.
fn draw(window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
}

/// Draws one frame and returns how often `T` rendered in it.
fn frame_renders<T>(handle: AnyWindowHandle, cx: &mut TestAppContext) -> usize {
    render_count::reset();
    cx.update_window(handle, |_, window, cx| draw(window, cx))
        .unwrap();
    render_count::of::<T>()
}

#[gpui_kit::test]
fn a_cat_frame_draws_neither_the_list_nor_the_task_view(cx: &mut TestAppContext) {
    let (handle, view) = open_with_task(cx);
    let mascot = cx.read(|cx| view.read(cx).mascot().clone());
    for _ in 0..3 {
        render_count::reset();
        cx.update_window(handle, |_, window, cx| {
            mascot.update(cx, |mascot, cx| mascot.tick(cx));
            draw(window, cx);
        })
        .unwrap();
        assert_eq!(render_count::of::<Mascot>(), 1);
        assert_eq!(render_count::of::<Sidebar>(), 0);
        assert_eq!(render_count::of::<IssueList>(), 0);
        assert_eq!(render_count::of::<TaskView>(), 0);
    }
}

#[gpui_kit::test]
fn an_update_button_frame_draws_neither_the_list_nor_the_task_view(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("llmtodo.app");
    std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
    std::fs::write(bundle.join(llmtodo_core::bundle::COMMIT_FILE), "newer").unwrap();
    let (handle, view) = open_with_task(cx);
    let watch = UpdateWatch {
        bundle,
        own: "own".into(),
    };
    view.update(cx, |this, cx| {
        this.watch_for_updates(watch, dir.path().to_path_buf(), None, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| view.read(cx).update_ready().map(String::from))
            .as_deref(),
        Some("newer")
    );
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    for _ in 0..3 {
        render_count::reset();
        cx.update_window(handle, |_, window, cx| {
            // The gradient asked for this frame.
            assert!(window.simulate_next_frame(cx) > 0);
            draw(window, cx);
        })
        .unwrap();
        assert_eq!(render_count::of::<UpdateButton>(), 1);
        assert_eq!(render_count::of::<IssueList>(), 0);
        assert_eq!(render_count::of::<TaskView>(), 0);
    }

    // The button still follows its state: a click says it restarts.
    view.update(cx, |this, cx| {
        this.set_update_launcher(|_| Err(std::io::Error::other("test")));
        cx.notify();
    });
    render_count::reset();
    view.update(cx, |this, cx| this.restart_to_update(cx));
    assert!(cx.read(|cx| view.read(cx).update_restarting()));
    assert_eq!(frame_renders::<UpdateButton>(handle, cx), 1);
}

#[gpui_kit::test]
fn the_cat_rests_while_the_window_is_hidden(cx: &mut TestAppContext) {
    let (handle, view) = open_with_task(cx);
    let mascot = cx.read(|cx| view.read(cx).mascot().clone());
    assert!(cx.read(|cx| mascot.read(cx).ticking()));
    cx.simulate_window_visibility_change(handle, WindowVisibility::Hidden);
    cx.run_until_parked();
    assert!(!cx.read(|cx| mascot.read(cx).ticking()));
    let before = cx.read(|cx| mascot.read(cx).behaviour().step());
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| mascot.read(cx).behaviour().step()), before);

    cx.simulate_window_visibility_change(handle, WindowVisibility::Visible);
    cx.run_until_parked();
    assert!(cx.read(|cx| mascot.read(cx).ticking()));
}

/// Runs `change`, then draws one frame, and counts renders across both. The
/// test platform draws a dirty window when an update ends, so the count must
/// start before the change.
fn after(
    handle: AnyWindowHandle,
    cx: &mut TestAppContext,
    change: impl FnOnce(&mut TestAppContext),
) {
    render_count::reset();
    change(cx);
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| draw(window, cx))
        .unwrap();
}

fn press(handle: AnyWindowHandle, keys: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        for key in keys.split(' ') {
            window.press(key, cx);
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_list_and_the_task_view_draw_again_for_what_they_show(cx: &mut TestAppContext) {
    let (handle, view) = open_with_task(cx);

    // Nothing changed: neither draws.
    assert_eq!(frame_renders::<IssueList>(handle, cx), 0);
    assert_eq!(frame_renders::<TaskView>(handle, cx), 0);

    // A live title change on the open task: both show it.
    after(handle, cx, |cx| {
        view.update(cx, |this, cx| {
            let id = this.detail(cx).unwrap().issue;
            let mut issue = this.issue(id, cx).clone();
            issue.title = "A title from the server".into();
            this.receive_issue(issue, cx);
        })
    });
    assert!(render_count::of::<IssueList>() > 0);
    assert!(render_count::of::<TaskView>() > 0);

    // A change to an issue the list shows, but not the open one.
    after(handle, cx, |cx| {
        view.update(cx, |this, cx| {
            let mut issue = this.visible_issue(3, cx).clone();
            issue.title = "Another server title".into();
            this.receive_issue(issue, cx);
        })
    });
    assert!(render_count::of::<IssueList>() > 0);

    // The workspace changes the list: a mark, the grouping, the sort.
    press(handle, "escape", cx);
    after(handle, cx, |cx| press(handle, "x", cx));
    assert!(render_count::of::<IssueList>() > 0);
    assert_eq!(cx.read(|cx| view.read(cx).marked(cx).len()), 1);
    after(handle, cx, |cx| {
        view.update(cx, |this, cx| {
            this.group_by(crate::issues::list::GroupBy::Status, cx)
        })
    });
    assert!(render_count::of::<IssueList>() > 0);
    after(handle, cx, |cx| {
        view.update(cx, |this, cx| {
            this.sort_by(crate::issues::list::SortKey::Added, cx)
        })
    });
    assert!(render_count::of::<IssueList>() > 0);
    after(handle, cx, |cx| {
        view.update(cx, |this, cx| {
            this.set_scope(crate::model::scope::Scope::All, cx)
        })
    });
    assert!(render_count::of::<IssueList>() > 0);

    // The workspace changes the task view: its tab, and the vim switch.
    press(handle, "enter", cx);
    after(handle, cx, |cx| {
        view.update(cx, |this, cx| this.set_tab(1, cx))
    });
    assert!(render_count::of::<TaskView>() > 0);
    after(handle, cx, |cx| {
        view.update(cx, |this, cx| this.set_vim_editor(false, cx))
    });
    assert!(render_count::of::<TaskView>() > 0);
}

#[gpui_kit::test]
fn the_inbox_documents_and_settings_draw_again_for_the_store(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| {
            this.set_scope(crate::model::scope::Scope::Inbox, cx)
        });
        window.render_frame(cx);
    })
    .unwrap();
    assert!(cx.read(|cx| view.read(cx).inbox_shown(cx)));
    assert_fills(handle, "inbox-view", cx);
    assert_eq!(frame_renders::<InboxView>(handle, cx), 0);
    after(handle, cx, |cx| touch_store(&view, cx));
    assert!(render_count::of::<InboxView>() > 0);

    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.show_settings(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    assert!(cx.read(|cx| view.read(cx).settings_shown(cx)));
    assert_fills(handle, "settings-view", cx);
    assert_eq!(frame_renders::<SettingsView>(handle, cx), 0);
    after(handle, cx, |cx| touch_store(&view, cx));
    assert!(render_count::of::<SettingsView>() > 0);

    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command("doc release", window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    assert!(cx.read(|cx| view.read(cx).doc_shown(cx)));
    assert_fills(handle, "doc-view", cx);
    assert_eq!(frame_renders::<DocView>(handle, cx), 0);
    after(handle, cx, |cx| touch_store(&view, cx));
    assert!(render_count::of::<DocView>() > 0);
}

/// A cached view is laid out as its own root. It must still fill the main
/// area beside the sidebar, which is most of the 1400 px window.
fn assert_fills(handle: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, _| {
        let width = window.find(id).bounds().size.width;
        assert!(width > px(1100.), "{id} is only {width:?} wide");
    })
    .unwrap();
}

/// A live change to one issue, as the server sends it.
fn touch_store(view: &Entity<Workspace>, cx: &mut TestAppContext) {
    view.update(cx, |this, cx| {
        let (issues, _) = this.issues_and_catalog(cx);
        let mut issue = issues[0].clone();
        issue.title = format!("{} again", issue.title).into();
        this.receive_issue(issue, cx);
    });
}
