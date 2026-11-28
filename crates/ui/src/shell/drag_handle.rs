//! LLM-646: one look for every edge the owner can drag. The divider between
//! the list and the task view and the bottom edges of the task view's
//! review and git panels use it.
//!
//! A plain border is a line. An edge you can drag is a line with a short
//! grip in its middle, even at rest, so the owner sees which lines move
//! before hovering them. Hovering darkens the line and grows the grip, and
//! a drag shows them strongest. GPUI Kit's base handle owns the grab band,
//! the resize cursor and the drag. This module only paints.

use std::rc::Rc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::base::{HandleEdge, ResizeHandleContext, ResizeHandleRenderer, ResizeHandleState};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyElement, App, Axis, InteractiveElement as _, IntoElement as _, ParentElement as _, Pixels,
    Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

/// How thick the grip is, across the line.
pub const GRIP_THICKNESS: Pixels = px(3.);

/// How long the grip is, and how solid, in each state.
pub fn grip(state: ResizeHandleState) -> (Pixels, f32) {
    match state {
        ResizeHandleState::Idle => (px(16.), 0.45),
        ResizeHandleState::Hovered => (px(28.), 0.75),
        ResizeHandleState::Pressed | ResizeHandleState::Dragging => (px(40.), 1.),
    }
}

/// The renderer for a draggable edge whose line is `line` thick.
pub fn appearance(line: Pixels) -> ResizeHandleRenderer {
    Rc::new(move |handle, window, cx| Some(render(handle, line, window, cx)))
}

fn render(handle: &ResizeHandleContext, line: Pixels, _: &mut Window, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let state = handle.state();
    let color = match state {
        ResizeHandleState::Idle => theme.border,
        ResizeHandleState::Hovered => theme.muted_foreground.opacity(0.6),
        ResizeHandleState::Pressed | ResizeHandleState::Dragging => theme.foreground.opacity(0.7),
    };
    let (length, opacity) = grip(state);
    // Across the line, the grip is thicker than the line. A divider between
    // two panels centres it on the line. An edge of its own container keeps
    // it inside, so the container's clip does not cut it.
    let overhang = GRIP_THICKNESS - line;
    let offset = match handle.edge() {
        None => overhang * -0.5,
        Some(HandleEdge::Trailing) => -overhang,
        Some(HandleEdge::Leading) => px(0.),
    };
    let pill = div()
        .id("drag-handle-grip")
        .flex_none()
        .rounded(theme.radius_full())
        .bg(theme.muted_foreground.opacity(opacity));
    div()
        .flex_none()
        .flex()
        .bg(color)
        .map(|this| match handle.axis() {
            Axis::Horizontal => this
                .w(line)
                .h_full()
                .items_center()
                .child(pill.w(GRIP_THICKNESS).h(length).ml(offset).test_support()),
            _ => this
                .h(line)
                .w_full()
                .items_start()
                .justify_center()
                .child(pill.h(GRIP_THICKNESS).w(length).mt(offset).test_support()),
        })
        .into_any_element()
}
