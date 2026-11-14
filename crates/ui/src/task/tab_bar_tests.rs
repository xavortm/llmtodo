//! LLM-197: the task view's tab bar shows the tabs that fit and puts the
//! rest under a "More (N)" button.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};

use crate::render_count;
use crate::shell::workspace::Workspace;
use crate::task::tab_bar::tabs_that_fit;
use crate::task::task_view::TaskView;

#[test]
fn every_tab_shows_when_all_fit() {
    let tabs = [px(100.), px(100.), px(100.)];
    assert_eq!(tabs_that_fit(&tabs, px(300.), |_| px(80.)), 3);
    assert_eq!(tabs_that_fit(&tabs, px(1000.), |_| px(80.)), 3);
}

#[test]
fn the_more_button_takes_room_from_the_tabs() {
    let tabs = [px(100.), px(100.), px(100.)];
    // 299 is too narrow for all three. Two tabs and the button need 280.
    assert_eq!(tabs_that_fit(&tabs, px(299.), |_| px(80.)), 2);
    assert_eq!(tabs_that_fit(&tabs, px(279.), |_| px(80.)), 1);
    assert_eq!(tabs_that_fit(&tabs, px(179.), |_| px(80.)), 0);
    // The button's width depends on how many tabs it holds.
    assert_eq!(
        tabs_that_fit(&tabs, px(290.), |hidden| px(80.) + px(hidden as f32 * 10.)),
        2
    );
    assert_eq!(
        tabs_that_fit(&tabs, px(289.), |hidden| px(90.) + px(hidden as f32)),
        1
    );
}

#[test]
fn an_unmeasured_bar_shows_every_tab() {
    let tabs = [px(100.), px(100.)];
    assert_eq!(tabs_that_fit(&tabs, px(0.), |_| px(80.)), 2);
}

/// A workspace on demo data with the task view `detail_width` pixels wide,
/// and its first task open. LLM-101 has three tabs: Description, Agent
/// notes and Work log.
fn open_task(detail_width: f32, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    let settings = crate::settings::UiSettings {
        split_side: Some(detail_width),
        ..Default::default()
    };
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1200.), px(760.)),
                ))),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    let store = crate::settings::SettingsStore::memory();
                    let mut this = Workspace::with_settings(settings, store, window, cx);
                    this.load_demo(cx);
                    this
                })
            },
        )
        .expect("open test window")
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    // The second frame uses the room the first one measured.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    (handle, view)
}

fn active_tab(view: &Entity<Workspace>, cx: &TestAppContext) -> Option<(usize, usize)> {
    cx.read(|cx| {
        view.read(cx)
            .open_tab(cx)
            .map(|(ix, tabs)| (ix, tabs.len()))
    })
}

#[gpui_kit::test]
fn a_wide_bar_shows_every_tab_and_no_more_button(cx: &mut TestAppContext) {
    let (handle, view) = open_task(560., cx);
    assert_eq!(active_tab(&view, cx), Some((0, 3)));
    cx.update_window(handle, |_, window, _| {
        for ix in 0..3usize {
            assert!(window.try_find(("detail-tab", ix)).is_some(), "tab {ix}");
        }
        assert!(window.try_find("detail-tabs-more").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_narrow_bar_puts_the_rest_under_more(cx: &mut TestAppContext) {
    let (handle, view) = open_task(300., cx);
    let shown = cx
        .update_window(handle, |_, window, _| {
            let shown = (0..3usize)
                .filter(|ix| window.try_find(("detail-tab", *ix)).is_some())
                .count();
            assert!(shown < 3, "every tab shows in a narrow bar");
            // The shown tabs are the first ones, and the button sits after
            // them, inside the bar.
            for ix in 0..shown {
                assert!(window.try_find(("detail-tab", ix)).is_some());
            }
            let room = window.find("detail-tabs-room").bounds();
            let more = window.find("detail-tabs-more").bounds();
            assert!(
                more.right() <= room.right() + px(0.5),
                "{more:?} in {room:?}"
            );
            if shown > 0 {
                let last = window.find(("detail-tab", shown - 1)).bounds();
                assert!(last.right() <= more.left(), "{last:?} overlaps {more:?}");
            }
            shown
        })
        .unwrap();

    // `gt` reaches a hidden tab. The More button stays.
    cx.update_window(handle, |_, window, cx| {
        for _ in 0..2 {
            window.press("g", cx);
            window.press("t", cx);
        }
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(active_tab(&view, cx), Some((2, 3)));
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find("detail-tabs-more").is_some());
        assert!(window.try_find(("detail-tab", 2usize)).is_none());
    })
    .unwrap();

    // The menu lists the hidden tabs. ctrl-n picks the first one.
    cx.update_window(handle, |_, window, cx| {
        window.click("detail-tabs-more", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.press("ctrl-n", cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(active_tab(&view, cx), Some((shown, 3)));
}

/// The bar measures itself without drawing the task view on every frame.
#[gpui_kit::test]
fn a_settled_bar_does_not_draw_again(cx: &mut TestAppContext) {
    let (handle, _) = open_task(300., cx);
    for _ in 0..3 {
        render_count::reset();
        cx.update_window(handle, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        assert_eq!(render_count::of::<TaskView>(), 0);
    }
}

/// Where the shared hover background is, in window coordinates, and its
/// opacity.
fn hover_shown(
    view: &Entity<Workspace>,
    cx: &TestAppContext,
) -> Option<(gpui_kit::Bounds<gpui_kit::Pixels>, f32)> {
    cx.read(|cx| {
        let task = view.read(cx).task().read(cx);
        task.tab_hover.read(cx).shown(cx)
    })
}

fn tab_bounds(
    handle: AnyWindowHandle,
    ix: usize,
    cx: &mut TestAppContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update_window(handle, |_, window, _| {
        window.find(("detail-tab", ix)).bounds()
    })
    .unwrap()
}

fn wait(ms: u64, cx: &mut TestAppContext) {
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(ms));
    cx.run_until_parked();
}

/// LLM-220: with animations on, one hover background slides from tab to
/// tab and fits each one. Its frames draw it, not the task view.
#[gpui_kit::test]
fn the_hover_background_slides_between_tabs(cx: &mut TestAppContext) {
    let (handle, view) = open_task(560., cx);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    // The task view slides in once animations are on. Let it settle.
    for _ in 0..2 {
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        wait(400, cx);
    }
    assert_eq!(hover_shown(&view, cx), None, "nothing shows at rest");

    cx.update_window(handle, |_, window, cx| {
        window.hover(("detail-tab", 0usize), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    wait(400, cx);
    // The first frames after animations turn on may draw the task view.
    for _ in 0..2 {
        cx.update_window(handle, |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
    }
    let first = tab_bounds(handle, 0, cx);
    let second = tab_bounds(handle, 1, cx);
    assert_eq!(
        hover_shown(&view, cx),
        Some((first, 1.)),
        "it fades in on the tab"
    );

    // The pointer's move draws the task view once, for the label's hover
    // color, as before LLM-220.
    cx.update_window(handle, |_, window, cx| {
        window.hover(("detail-tab", 1usize), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    // The slide's frames draw only the hover background.
    for _ in 0..3 {
        render_count::reset();
        cx.update_window(handle, |_, window, cx| {
            assert!(
                window.simulate_next_frame(cx) > 0,
                "the slide asks for frames"
            );
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert_eq!(render_count::of::<crate::task::tab_bar::TabHover>(), 1);
        assert_eq!(render_count::of::<TaskView>(), 0);
    }
    wait(60, cx);
    let (mid, alpha) = hover_shown(&view, cx).unwrap();
    assert_eq!(alpha, 1.);
    assert!(
        mid.origin.x > first.origin.x && mid.origin.x < second.origin.x,
        "it is between the tabs: {mid:?}"
    );
    wait(400, cx);
    assert_eq!(
        hover_shown(&view, cx),
        Some((second, 1.)),
        "it fits the tab"
    );

    // Settled: no more frames.
    cx.update_window(handle, |_, window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
        assert_eq!(window.simulate_next_frame(cx), 0);
    })
    .unwrap();
}

/// With animations off, each tab keeps its own hover fill, as before, and
/// no frame waits on an animation.
#[gpui_kit::test]
fn with_animations_off_no_hover_background_shows(cx: &mut TestAppContext) {
    let (handle, view) = open_task(560., cx);
    cx.update_window(handle, |_, window, cx| {
        window.hover(("detail-tab", 0usize), cx)
    })
    .unwrap();
    wait(400, cx);
    cx.update_window(handle, |_, window, cx| {
        window.hover(("detail-tab", 1usize), cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.update_window(handle, |_, window, cx| {
        assert_eq!(window.simulate_next_frame(cx), 0, "no animation runs");
    })
    .unwrap();
    assert_eq!(hover_shown(&view, cx), None);
}

/// The More button shares the hover background with the tabs.
#[gpui_kit::test]
fn the_hover_background_reaches_the_more_button(cx: &mut TestAppContext) {
    let (handle, view) = open_task(300., cx);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    for _ in 0..2 {
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        wait(400, cx);
    }
    cx.update_window(handle, |_, window, cx| {
        window.hover("detail-tabs-more", cx);
        window.draw(cx).clear(cx);
    })
    .unwrap();
    wait(400, cx);
    let more = cx
        .update_window(handle, |_, window, _| {
            window.find("detail-tabs-more").bounds()
        })
        .unwrap();
    assert_eq!(hover_shown(&view, cx), Some((more, 1.)));
}
