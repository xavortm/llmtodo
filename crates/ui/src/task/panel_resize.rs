//! LLM-146: the owner drags the bottom edge of a task view panel to make it
//! taller or shorter. The review panel and the git panel have the handle.
//!
//! A panel's height follows its content until the owner drags it. From then
//! on it keeps the dragged height, in rem, so a zoom keeps its proportions.
//! A double click on the edge goes back to the content height. The heights
//! live in the UI settings.
//!
//! GPUI Kit's panel groups give every panel a fixed size, so they do not
//! fit panels that follow their content. The kit's standalone resize handle
//! does fit: it draws the edge and starts the drag, and this module sets the
//! height.

use gpui_kit::component::InteractiveElementExt as _;
use gpui_kit::component::resizable::{HandleEdge, resize_handle};
use gpui_kit::{
    AnyElement, App, AppContext as _, Axis, DragMoveEvent, Empty, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Styled as _, WeakEntity, Window, div,
    prelude::FluentBuilder as _, rems,
};

use crate::shell::view_events::{ViewEvent, ViewEvents};
use crate::task::task_view::TaskView;

/// The review panel's name in the settings.
pub const REVIEW: &str = "review";
/// The git panel's name in the settings.
pub const GIT: &str = "git";

/// The shortest a dragged panel gets, in rem: its title and one line.
pub const MIN_REM: f32 = 3.;
/// The tallest a dragged panel gets, in rem. The description below keeps
/// [`BODY_MIN_REM`] in any case, because a dragged panel shrinks first.
pub const MAX_REM: f32 = 80.;
/// The room the description keeps under dragged panels, in rem.
pub const BODY_MIN_REM: f32 = 6.;

/// The drag of one panel's bottom edge, by panel name.
#[derive(Clone)]
pub struct ResizePanel(pub &'static str);

impl Render for ResizePanel {
    fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// A dragged height in rem, from the pointer's distance below the panel's
/// top, kept within the limits.
pub fn height_rem(distance: gpui_kit::Pixels, rem: gpui_kit::Pixels) -> f32 {
    (f32::from(distance) / f32::from(rem)).clamp(MIN_REM, MAX_REM)
}

/// Wraps `panel` with a drag handle on its bottom edge. `height` is the
/// dragged height in rem, or `None` to follow the content. With a height,
/// the panel must fill the wrapper.
pub fn resizable(
    name: &'static str,
    height: Option<f32>,
    panel: AnyElement,
    view: WeakEntity<TaskView>,
    events: ViewEvents,
) -> AnyElement {
    let set = move |height: Option<f32>, window: &mut Window, cx: &mut App| {
        let Some(view) = view.upgrade() else {
            return;
        };
        let changed = update_height(&view, name, height, cx);
        // Events reach the workspace at once, and it reads the task view,
        // so the event goes out after the update above has ended.
        if changed {
            events.emit(ViewEvent::PanelResized, window, cx);
        }
    };
    let set_on_double_click = set.clone();
    div()
        .id(("panel-resize", name_ix(name)))
        .relative()
        .w_full()
        .min_w_0()
        .map(|this| match height {
            Some(h) => this
                .flex_shrink(1.)
                .h(rems(h))
                .min_h(rems(MIN_REM))
                .overflow_hidden(),
            None => this.flex_none(),
        })
        .on_drag_move::<ResizePanel>(move |e: &DragMoveEvent<ResizePanel>, window, cx| {
            if e.drag(cx).0 != name {
                return;
            }
            let h = height_rem(e.event.position.y - e.bounds.top(), window.rem_size());
            set(Some(h), window, cx);
        })
        .child(panel)
        .child(
            resize_handle::<ResizePanel, ResizePanel>(
                ("panel-resize-handle", name_ix(name)),
                Axis::Vertical,
            )
            .inside(HandleEdge::Trailing)
            .on_drag(ResizePanel(name), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_double_click(move |_, window, cx| set_on_double_click(None, window, cx)),
        )
        .into_any_element()
}

/// Sets a panel's dragged height, or clears it. Returns whether it changed.
fn update_height(view: &Entity<TaskView>, name: &str, height: Option<f32>, cx: &mut App) -> bool {
    view.update(cx, |this, cx| {
        let before = this.panel_heights.get(name).copied();
        if before == height {
            return false;
        }
        match height {
            Some(h) => this.panel_heights.insert(name.to_string(), h),
            None => this.panel_heights.remove(name),
        };
        cx.notify();
        true
    })
}

/// A number per panel name, for element IDs.
fn name_ix(name: &str) -> usize {
    match name {
        REVIEW => 0,
        GIT => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::px;

    #[test]
    fn the_height_is_in_rem_and_kept_within_limits() {
        assert_eq!(height_rem(px(160.), px(16.)), 10.);
        assert_eq!(height_rem(px(320.), px(32.)), 10.);
        assert_eq!(height_rem(px(-50.), px(16.)), MIN_REM);
        assert_eq!(height_rem(px(10_000.), px(16.)), MAX_REM);
    }
}
