//! LLM-219: with animations on, a list column that hides or comes back
//! fades out or in while its width shrinks or grows. The header and the
//! rows stay in step, and the list stops drawing when the fade ends.

use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ElementId, Entity, Pixels, TestAppContext,
    WindowBounds, WindowOptions, px, size,
};

use crate::issues::columns::Column;
use crate::shell::motion::COLUMN;
use crate::shell::workspace::Workspace;

fn open(cx: &mut TestAppContext, width: f32) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(width), px(760.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("open test window")
    })
}

/// Draws a frame and says how many views asked for the next one.
fn frames_asked(handle: AnyWindowHandle, cx: &mut TestAppContext) -> usize {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.simulate_next_frame(cx)
    })
    .unwrap()
}

/// Lets every animation of the start end.
fn settle(handle: AnyWindowHandle, cx: &mut TestAppContext) {
    frames_asked(handle, cx);
    cx.executor().advance_clock(Duration::from_secs(1));
    frames_asked(handle, cx);
    frames_asked(handle, cx);
}

/// The left edge and width of `column` in the header, and in the first
/// issue row. None when the header does not draw it.
fn cells(
    handle: AnyWindowHandle,
    column: Column,
    cx: &mut TestAppContext,
) -> Option<((Pixels, Pixels), (Pixels, Pixels))> {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let head = window.within("column-header").try_find(column.id())?;
        let head = head.bounds();
        let row = (0..12usize)
            .map(|row| ElementId::from(("row", row)))
            .find(|id| {
                window
                    .within(id.clone())
                    .try_find(Column::Key.id())
                    .is_some()
            })
            .expect("an issue row");
        let cell = window.within(row).find(column.id()).bounds();
        Some((
            (head.left(), head.size.width),
            (cell.left(), cell.size.width),
        ))
    })
    .unwrap()
}

fn width(handle: AnyWindowHandle, column: Column, cx: &mut TestAppContext) -> Option<f32> {
    let (head, row) = cells(handle, column, cx)?;
    assert_eq!(head, row, "the header and the rows stay in step");
    Some(f32::from(head.1))
}

fn toggle(view: &Entity<Workspace>, column: Column, cx: &mut TestAppContext) {
    let list = cx.read(|cx| view.read(cx).list().clone());
    cx.update(|cx| list.update(cx, |list, cx| list.toggle_column(column, cx)));
}

#[gpui_kit::test]
fn a_column_the_owner_hides_fades_out_and_back_in(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, 1600.);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    settle(handle, cx);
    let full = width(handle, Column::Added, cx).expect("a wide list shows the date");
    let title = width(handle, Column::Title, cx).unwrap();

    toggle(&view, Column::Added, cx);
    assert_eq!(
        width(handle, Column::Added, cx),
        Some(full),
        "it starts whole"
    );
    cx.executor().advance_clock(COLUMN / 4);
    let mid = width(handle, Column::Added, cx).expect("it still shows while it fades");
    assert!(mid > 0. && mid < full, "it shrinks: {mid} of {full}");
    let grown = width(handle, Column::Title, cx).unwrap();
    assert!(grown > title, "the title takes up the room");
    assert!(frames_asked(handle, cx) > 0, "the fade draws each frame");

    // Back before it is gone: it grows from where it was, with no jump.
    toggle(&view, Column::Added, cx);
    assert_eq!(width(handle, Column::Added, cx), Some(mid));
    cx.executor().advance_clock(COLUMN);
    frames_asked(handle, cx);
    assert_eq!(width(handle, Column::Added, cx), Some(full));
    assert_eq!(width(handle, Column::Title, cx), Some(title));
    frames_asked(handle, cx);
    assert_eq!(frames_asked(handle, cx), 0, "it stops when done");

    toggle(&view, Column::Added, cx);
    cx.executor().advance_clock(COLUMN);
    frames_asked(handle, cx);
    assert_eq!(width(handle, Column::Added, cx), None, "it goes at the end");
    frames_asked(handle, cx);
    assert_eq!(frames_asked(handle, cx), 0);

    // Off: it comes back at once.
    cx.update(|cx| crate::shell::motion::set(false, cx));
    toggle(&view, Column::Added, cx);
    assert_eq!(width(handle, Column::Added, cx), Some(full));
    assert_eq!(frames_asked(handle, cx), 0, "off: nothing animates");
}

#[gpui_kit::test]
fn a_narrower_window_fades_columns_out(cx: &mut TestAppContext) {
    let (handle, _view) = open(cx, 1600.);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    settle(handle, cx);
    let full = width(handle, Column::Time, cx).expect("a wide list shows the time");

    cx.simulate_window_resize(handle, size(px(700.), px(760.)));
    // A frame lays out the new width, and the next plans the columns.
    for _ in 0..3 {
        frames_asked(handle, cx);
    }
    assert_eq!(
        width(handle, Column::Time, cx),
        Some(full),
        "the time fades out from its full width"
    );
    let title = width(handle, Column::Title, cx).unwrap();
    assert_eq!(title, 160., "the title keeps its minimum");
    cx.executor().advance_clock(COLUMN / 2);
    let mid = width(handle, Column::Time, cx).unwrap();
    assert!(mid > 0. && mid < full, "{mid} of {full}");
    cx.executor().advance_clock(COLUMN);
    frames_asked(handle, cx);
    assert_eq!(width(handle, Column::Time, cx), None);
    frames_asked(handle, cx);
    assert_eq!(frames_asked(handle, cx), 0, "the list stops drawing");
}
