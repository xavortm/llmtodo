//! LLM-224: with animations on, a move to another task eases the task
//! view's panels to their new height. Off, they jump as before.

use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Pixels, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};

use crate::model::data::IssueId;
use crate::shell::workspace::Workspace;
use crate::task::panel_motion::LENGTH;

/// A workspace on demo data with LLM-101 open. LLM-101 has no claim, and
/// LLM-102 has one, so the claim bar comes and goes between them.
fn open_task(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    open_task_with(Default::default(), cx)
}

fn open_task_with(
    panel_heights: std::collections::BTreeMap<String, f32>,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    let settings = crate::settings::UiSettings {
        split_side: Some(560.),
        panel_heights,
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
    show(&view, 101, cx);
    (handle, view)
}

fn show(view: &Entity<Workspace>, n: u128, cx: &mut TestAppContext) {
    let task = cx.read(|cx| view.read(cx).task().clone());
    cx.update(|cx| task.update(cx, |task, cx| task.show(IssueId::from_u128(n), cx)));
}

fn wait(ms: u64, cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    cx.run_until_parked();
}

/// Draws a frame and returns the top of the description's first tab, which
/// sits under every panel. Also says how many views asked for the next
/// frame.
fn body_top(handle: AnyWindowHandle, cx: &mut TestAppContext) -> (Pixels, usize) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let top = window.find(("detail-tab", 0usize)).bounds().top();
        (top, window.simulate_next_frame(cx))
    })
    .unwrap()
}

/// Lets every animation end and returns where the body rests.
fn settle(handle: AnyWindowHandle, cx: &mut TestAppContext) -> Pixels {
    for _ in 0..3 {
        body_top(handle, cx);
        wait(400, cx);
    }
    body_top(handle, cx);
    let (top, asked) = body_top(handle, cx);
    assert_eq!(asked, 0, "nothing draws at rest");
    top
}

#[gpui_kit::test]
fn panels_ease_to_the_next_tasks_height(cx: &mut TestAppContext) {
    let (handle, view) = open_task(cx);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    let without_claim = settle(handle, cx);

    show(&view, 102, cx);
    let (first, asked) = body_top(handle, cx);
    assert_eq!(first, without_claim, "it starts from the height on screen");
    assert!(asked > 0, "the change draws each frame");
    wait(LENGTH.as_millis() as u64 / 3, cx);
    let (mid, _) = body_top(handle, cx);
    assert!(mid > without_claim, "the claim bar grows: {mid:?}");
    let with_claim = settle(handle, cx);
    assert!(mid < with_claim, "{mid:?} is on its way to {with_claim:?}");

    // Back mid-way: the bar shrinks from where it is, with no jump.
    show(&view, 101, cx);
    body_top(handle, cx);
    wait(LENGTH.as_millis() as u64 / 3, cx);
    let (shrinking, _) = body_top(handle, cx);
    assert!(shrinking < with_claim && shrinking > without_claim);
    show(&view, 102, cx);
    let (turned, _) = body_top(handle, cx);
    assert_eq!(turned, shrinking, "a new move starts from the screen");
    assert_eq!(settle(handle, cx), with_claim);
    show(&view, 101, cx);
    assert_eq!(
        settle(handle, cx),
        without_claim,
        "an empty bar shrinks away"
    );
}

#[gpui_kit::test]
fn off_the_panels_jump_at_once(cx: &mut TestAppContext) {
    let (handle, view) = open_task(cx);
    let without_claim = settle(handle, cx);
    show(&view, 102, cx);
    let (with_claim, asked) = body_top(handle, cx);
    assert!(with_claim > without_claim, "the claim bar shows at once");
    assert_eq!(asked, 0, "nothing animates");
    show(&view, 101, cx);
    assert_eq!(body_top(handle, cx), (without_claim, 0));
}

/// Puts LLM-101 in User review, so it shows the review panel and LLM-102
/// does not.
fn review_on_101(view: &Entity<Workspace>, cx: &mut TestAppContext) {
    use crate::model::data::{Status, StatusType};
    const SECTION: &str = "Context.\n\n+++ Review\n### Steps to reproduce\n1. Open it.\n\n### What to check\n- It works.\n+++\n";
    view.update(cx, |this, cx| {
        this.store().update(cx, |store, _| {
            store.modify(IssueId::from_u128(101), |i| {
                i.status = Status::new("User review", StatusType::Started, 9).with_review(true);
                i.description = SECTION.into();
                i.claim = None;
            });
        });
    });
    cx.run_until_parked();
}

/// The owner found this on LLM-225 to LLM-226: a review panel the owner
/// dragged to a height, on a task the next one lacks, shrank away and then
/// kept its minimum height as an empty box. It pushed the panels below it
/// down until the next redraw.
#[gpui_kit::test]
fn a_dragged_panel_the_next_task_lacks_shrinks_to_nothing(cx: &mut TestAppContext) {
    let heights = [(crate::task::panel_resize::REVIEW.to_string(), 12.)].into();
    let (handle, view) = open_task_with(heights, cx);
    review_on_101(&view, cx);
    show(&view, 102, cx);
    let at_rest = settle(handle, cx);

    cx.update(|cx| crate::shell::motion::set(true, cx));
    show(&view, 101, cx);
    settle(handle, cx);
    show(&view, 102, cx);
    body_top(handle, cx);
    wait(LENGTH.as_millis() as u64 + 50, cx);
    // The change has ended. This frame must already be at rest.
    let (top, _) = body_top(handle, cx);
    assert_eq!(top, at_rest, "no empty box is left behind");
    assert_eq!(settle(handle, cx), at_rest);
}
