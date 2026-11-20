//! LLM-7: a hover, a live update or a keystroke draws only the views that
//! show the change. Each test counts renders in the style of
//! `cache_tests.rs`.

use std::time::Duration;

use gpui_kit::InputEvent as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, TestAppContext, Window, WindowBounds,
    WindowOptions, px, size,
};

use crate::issues::list::IssueList;
use crate::render_count;
use crate::shell::motion::FillLayer;
use crate::shell::sidebar::Sidebar;
use crate::shell::workspace::Workspace;
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

/// Draws one frame as the platform would: only dirty views render.
fn draw(window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
}

fn wait(ms: u64, cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    cx.run_until_parked();
}

fn press(handle: AnyWindowHandle, keys: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        for key in keys.split(' ') {
            window.press(key, cx);
        }
    })
    .unwrap();
}

/// Opens the window with the sidebar shown and animations on, and lets
/// the panels settle.
fn open_animated(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    let (handle, view) = open(cx);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        if !view.read(cx).sidebar_shown() {
            window.press("cmd-b", cx);
        }
    })
    .unwrap();
    settle(handle, cx);
    (handle, view)
}

/// Draws frames until no animation asks for another.
fn settle(handle: AnyWindowHandle, cx: &mut TestAppContext) {
    for _ in 0..4 {
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        wait(400, cx);
    }
    for _ in 0..4 {
        cx.update_window(handle, |_, window, cx| {
            window.simulate_next_frame(cx);
            draw(window, cx);
        })
        .unwrap();
    }
}

/// Moves the pointer onto `id` and draws the frame, as the platform
/// would. The kit's `hover` draws the whole window first.
fn hover(handle: AnyWindowHandle, id: impl Into<gpui_kit::ElementId>, cx: &mut TestAppContext) {
    let id = id.into();
    cx.update_window(handle, |_, window, cx| {
        let position = window.find(id).bounds().center();
        window.dispatch_event(
            gpui_kit::MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Default::default(),
            }
            .to_platform_input(),
            cx,
        );
        draw(window, cx);
    })
    .unwrap();
}

/// Draws the next animation frame. Returns how many callbacks asked for it.
fn next_frame(handle: AnyWindowHandle, cx: &mut TestAppContext) -> usize {
    cx.update_window(handle, |_, window, cx| {
        let asked = window.simulate_next_frame(cx);
        draw(window, cx);
        asked
    })
    .unwrap()
}

fn painted(layer: &Entity<FillLayer>, cx: &TestAppContext) -> usize {
    cx.read(|cx| layer.read(cx).painted())
}

/// A row's hover fill and its fade out draw only the fill layer under the
/// list, never the list. A task is marked, so every row shows its mark
/// box: that box's own reveal on hover is a GPUI hover style, which draws
/// the list.
#[gpui_kit::test]
fn hovering_a_row_renders_issue_list_0_times(cx: &mut TestAppContext) {
    let (handle, view) = open_animated(cx);
    press(handle, "j x", cx);
    settle(handle, cx);
    let layer = cx.read(|cx| view.read(cx).list().read(cx).fill_layer.clone());
    let at_rest = painted(&layer, cx);
    assert!(at_rest > 0, "the selected and the marked rows have a fill");

    // The pointer comes into the window over the title bar first.
    hover(handle, "project-name", cx);
    settle(handle, cx);

    render_count::reset();
    hover(handle, ("row", 4usize), cx);
    assert_eq!(render_count::of::<IssueList>(), 0);
    assert_eq!(
        painted(&layer, cx),
        at_rest + 1,
        "the hovered row is filled"
    );

    // The pointer moves on: the first row fades out while the next fills.
    render_count::reset();
    hover(handle, ("row", 5usize), cx);
    assert_eq!(painted(&layer, cx), at_rest + 2);
    let mut frames = 0;
    while next_frame(handle, cx) > 0 {
        frames += 1;
        assert!(frames < 100, "the fade ends");
        wait(16, cx);
    }
    assert!(frames > 0, "the fade asks for frames");
    assert_eq!(render_count::of::<IssueList>(), 0);
    assert!(render_count::of::<FillLayer>() > 0);
    assert_eq!(painted(&layer, cx), at_rest + 1, "the fade ended");
}

/// The same for the sidebar's rows.
#[gpui_kit::test]
fn hovering_a_sidebar_row_renders_sidebar_0_times(cx: &mut TestAppContext) {
    let (handle, view) = open_animated(cx);
    let layer = cx.read(|cx| view.read(cx).sidebar().read(cx).fill_layer.clone());
    let at_rest = painted(&layer, cx);
    hover(handle, "project-name", cx);
    settle(handle, cx);
    render_count::reset();
    hover(handle, "sidebar-trash", cx);
    hover(handle, "sidebar-team:LLM", cx);
    assert_eq!(painted(&layer, cx), at_rest + 2);
    while next_frame(handle, cx) > 0 {
        wait(16, cx);
    }
    assert_eq!(painted(&layer, cx), at_rest + 1);
    assert_eq!(render_count::of::<Sidebar>(), 0);
    assert_eq!(render_count::of::<IssueList>(), 0);
    assert_eq!(render_count::of::<TaskView>(), 0);
}

/// Opens a window `height` px tall with the sidebar shown and a task open.
fn open_sized(height: f32, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1400.), px(height)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("open test window")
    });
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
    (handle, view)
}

/// Runs `change`, then draws one frame, and counts renders across both.
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

/// A live change to the text of `id`, which no list filter reads.
fn new_text(view: &Entity<Workspace>, id: llmtodo_core::IssueId, cx: &mut TestAppContext) {
    view.update(cx, |this, cx| {
        let mut issue = this.issue(id, cx).clone();
        issue.description = format!("{}\n\nMore text.", issue.description).into();
        this.receive_issue(issue, cx);
    });
}

#[gpui_kit::test]
fn a_live_update_to_an_off_screen_issue_renders_neither_task_view_nor_list(
    cx: &mut TestAppContext,
) {
    let (handle, view) = open_sized(420., cx);
    let (open, drawn, all) = cx.read(|cx| {
        let this = view.read(cx);
        let open = this.detail(cx).unwrap().issue;
        let drawn = this.list().read(cx).drawn();
        let (issues, _) = this.issues_and_catalog(cx);
        (open, drawn, issues)
    });
    assert!(drawn.contains(&open));
    let parent_key = all.iter().find(|i| i.id == open).unwrap().parent.clone();
    let off = all
        .iter()
        .find(|i| !drawn.contains(&i.id) && i.id != open && Some(&i.key) != parent_key.as_ref())
        .expect("an issue below the fold")
        .id;

    // Opening the task marked its git panel as asked for, without a
    // notify. The first store change after carries that along.
    after(handle, cx, |cx| new_text(&view, off, cx));
    after(handle, cx, |cx| new_text(&view, off, cx));
    assert_eq!(render_count::of::<TaskView>(), 0);
    assert_eq!(render_count::of::<IssueList>(), 0);

    // The open task's own text draws the task view.
    after(handle, cx, |cx| new_text(&view, open, cx));
    assert!(render_count::of::<TaskView>() > 0);
}

#[gpui_kit::test]
fn a_store_change_does_not_rescan_scope_counts_when_nothing_counted_changed(
    cx: &mut TestAppContext,
) {
    let (handle, view) = open_sized(900., cx);
    let sidebar = cx.read(|cx| view.read(cx).sidebar().clone());
    let scans = |cx: &TestAppContext| cx.read(|cx| sidebar.read(cx).scans);
    let before = scans(cx);
    let id = cx.read(|cx| view.read(cx).visible_issue(2, cx).id);

    after(handle, cx, |cx| new_text(&view, id, cx));
    assert_eq!(render_count::of::<Sidebar>(), 0);
    assert_eq!(scans(cx), before, "no count changed");

    // A move to the Trash changes the counts.
    after(handle, cx, |cx| {
        view.update(cx, |this, cx| {
            let mut issue = this.issue(id, cx).clone();
            issue.deleted = true;
            this.receive_issue(issue, cx);
        })
    });
    assert_eq!(render_count::of::<Sidebar>(), 1);
    assert_eq!(scans(cx), before + 1);
}

/// LLM-7: the filter panel is drawn cached. A list change, a hover and a
/// store change that leaves its rows the same do not draw it, and an arrow
/// key draws it without walking the issues again.
#[gpui_kit::test]
fn the_filter_panel_draws_only_for_what_it_shows(cx: &mut TestAppContext) {
    let (handle, view) = open_animated(cx);
    cx.update_window(handle, |_, window, cx| {
        window.click("filter-panel", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    hover(handle, "project-name", cx);
    settle(handle, cx);
    let (panel, list) = cx.read(|cx| {
        let this = view.read(cx);
        (this.filter_panel().clone(), this.list().clone())
    });
    assert!(cx.read(|cx| panel.read(cx).is_open()));
    let builds = |cx: &TestAppContext| cx.read(|cx| panel.read(cx).builds.get());
    type Panel = crate::issues::filter_panel::FilterPanelView;

    // The list draws again: the panel does not.
    after(handle, cx, |cx| list.update(cx, |_, cx| cx.notify()));
    assert_eq!(render_count::of::<IssueList>(), 1);
    assert_eq!(render_count::of::<Panel>(), 0);

    // A hover and its fade draw only the fill layer.
    render_count::reset();
    hover(handle, "filter-row-Label-bug", cx);
    hover(handle, "filter-row-Label-feature", cx);
    while next_frame(handle, cx) > 0 {
        wait(16, cx);
    }
    assert_eq!(render_count::of::<Panel>(), 0);
    assert!(render_count::of::<FillLayer>() > 0);

    // An arrow key moves the highlight from the memo.
    let before = builds(cx);
    after(handle, cx, |cx| {
        cx.update_window(handle, |_, window, cx| {
            window.dispatch_action(Box::new(crate::issues::filter_panel::FilterPanelDown), cx)
        })
        .unwrap()
    });
    assert_eq!(render_count::of::<Panel>(), 1);
    assert_eq!(builds(cx), before, "no walk over the issues");

    // New text on a task changes no row of the panel.
    let id = cx.read(|cx| view.read(cx).visible_issue(2, cx).id);
    after(handle, cx, |cx| new_text(&view, id, cx));
    assert_eq!(render_count::of::<Panel>(), 0);
}

/// LLM-7: each fetch of the open task sends its history again. The same
/// text again neither draws the task view nor prepares its tabs again.
#[gpui_kit::test]
fn the_same_history_again_does_not_prepare_the_tabs_again(cx: &mut TestAppContext) {
    let (handle, view) = open_sized(900., cx);
    let task = cx.read(|cx| view.read(cx).task().clone());
    let id = cx.read(|cx| view.read(cx).detail(cx).unwrap().issue);
    let set = |cx: &mut TestAppContext| {
        task.update(cx, |t, cx| {
            t.set_history(
                id,
                "- created".into(),
                "- a session".into(),
                Default::default(),
                cx,
            )
        })
    };
    after(handle, cx, set);
    assert_eq!(render_count::of::<TaskView>(), 1);
    let builds = cx.read(|cx| task.read(cx).prepared_builds.get());
    after(handle, cx, set);
    assert_eq!(render_count::of::<TaskView>(), 0);
    assert_eq!(cx.read(|cx| task.read(cx).prepared_builds.get()), builds);
}

/// LLM-7: a redraw of the inbox that no store change caused, such as a
/// cursor blink in an answer field, reuses its questions. A store change
/// reads them again.
#[gpui_kit::test]
fn an_inbox_redraw_reads_its_questions_once_per_store_change(cx: &mut TestAppContext) {
    let (handle, view) = open_sized(900., cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            this.set_scope(crate::model::scope::Scope::Inbox, cx)
        });
        window.render_frame(cx);
    })
    .unwrap();
    let inbox = cx.read(|cx| view.read(cx).inbox().clone());
    let builds = |cx: &TestAppContext| cx.read(|cx| inbox.read(cx).builds.get());
    let before = builds(cx);
    for _ in 0..3 {
        after(handle, cx, |cx| inbox.update(cx, |_, cx| cx.notify()));
        assert_eq!(render_count::of::<crate::inbox::InboxView>(), 1);
    }
    assert_eq!(builds(cx), before);
    let id = cx.read(|cx| view.read(cx).issues_and_catalog(cx).0[0].id);
    after(handle, cx, |cx| new_text(&view, id, cx));
    assert_eq!(builds(cx), before + 1);
}
