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
