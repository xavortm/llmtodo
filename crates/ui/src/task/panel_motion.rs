//! LLM-224: with Animations on, the task view's panels grow or shrink to
//! their new height when the owner moves to another task. The content
//! swaps at once. Only the panel's box changes size.
//!
//! A panel at rest lays out as usual. On a move to another task, it lays
//! its content out on its own to learn the new height, then draws a box
//! that eases from the height on screen to that height. A growing panel
//! clips its content. A shrinking one shows the new content at the top of
//! the box. Another move mid-way starts again from the height on screen.
//!
//! A change inside the same task, such as typing, never animates. A panel
//! that only one of the two tasks has grows from nothing or shrinks to
//! nothing as an empty box.
//!
//! Measuring the content on its own follows `MotionReveal` in GPUI Kit's
//! `gpui-base` crate (`crates/base/src/motion/reveal.rs`), Apache-2.0.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Display, Element, Empty, FlexDirection,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, Overflow, Pixels, Style, Window,
    px, relative, rems, size,
};

use crate::model::data::IssueId;
use crate::shell::motion;

/// The panels' heights and their changes, by panel name. The task view
/// holds one and gives it to every panel it draws.
#[derive(Default)]
pub struct PanelMotion {
    panels: HashMap<&'static str, PanelState>,
    /// The panels' width in the last frame. All panels share the column.
    width: Option<Pixels>,
}

impl PanelMotion {
    /// Forgets every panel, so the next task shows without a change.
    pub fn clear(&mut self) {
        self.panels.clear();
    }

    /// Whether any panel still changes height.
    #[cfg(test)]
    pub fn running(&self) -> bool {
        self.panels.values().any(|p| p.change.is_some())
    }
}

/// One panel: the task it showed, the height on screen and the change on
/// its way.
#[derive(Default, Debug)]
struct PanelState {
    key: Option<IssueId>,
    /// The height drawn in the last frame.
    shown: Pixels,
    /// The height the change started from, and when.
    change: Option<(Pixels, Instant)>,
}

impl PanelState {
    /// The panel draws for task `key` at `now`. A new task starts a change
    /// from the height on screen. Returns where the change starts and how
    /// far it has come, or None at rest.
    fn frame(&mut self, key: IssueId, now: Instant) -> Option<(Pixels, f32)> {
        if self.key != Some(key) {
            if self.key.is_some() {
                self.change = Some((self.shown, now));
            }
            self.key = Some(key);
        }
        let (from, start) = self.change?;
        match motion::resize_step(now.saturating_duration_since(start)) {
            Some(t) => Some((from, t)),
            None => {
                self.change = None;
                None
            }
        }
    }

    /// The panel is gone from task `key`. It returns whether an empty box
    /// must still draw, so the panel can shrink to nothing.
    fn absent(&mut self, key: IssueId) -> bool {
        if self.key == Some(key) && self.change.is_none() {
            // Gone within the same task, or already shrunk: at once.
            self.shown = px(0.);
            return false;
        }
        self.key != Some(key) && self.shown > px(0.) || self.change.is_some()
    }
}

/// Wraps one task view panel, named `name`, as it shows for task `key`.
/// `panel` is None when this task has no such panel. `shrink` says the
/// owner dragged the panel to a height, so it gives up room first on a
/// short window, as `panel_resize` sets up.
///
/// Off, this is the panel as it is.
pub fn panel(
    name: &'static str,
    key: IssueId,
    panel: Option<AnyElement>,
    shrink: bool,
    motion: &Rc<RefCell<PanelMotion>>,
    cx: &App,
) -> Option<AnyElement> {
    if !motion::on(cx) {
        return panel;
    }
    let absent = panel.is_none();
    let child = match panel {
        Some(panel) => panel,
        None => {
            let mut store = motion.borrow_mut();
            let state = store.panels.entry(name).or_default();
            if !state.absent(key) {
                state.key = Some(key);
                return None;
            }
            Empty.into_any_element()
        }
    };
    Some(
        PanelHeight {
            name,
            key,
            shrink,
            absent,
            motion: motion.clone(),
            child,
        }
        .into_any_element(),
    )
}

/// The element behind [`panel`].
struct PanelHeight {
    name: &'static str,
    key: IssueId,
    shrink: bool,
    /// The task has no such panel: this is the empty box shrinking away.
    absent: bool,
    motion: Rc<RefCell<PanelMotion>>,
    child: AnyElement,
}

impl IntoElement for PanelHeight {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// How a panel lays out in this frame.
enum Mode {
    /// As usual: the content is a child of the box.
    Rest,
    /// The box has the height of the change. The content laid out on its
    /// own, at `width`.
    Changing { width: Pixels },
}

impl Element for PanelHeight {
    type RequestLayoutState = Mode;
    type PrepaintState = ();

    fn id(&self) -> Option<gpui_kit::ElementId> {
        // No ID, so the panel's own elements keep their IDs and state.
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Mode) {
        let now = cx.background_executor().now();
        let (change, width) = {
            let mut store = self.motion.borrow_mut();
            let width = store.width;
            let state = store.panels.entry(self.name).or_default();
            (state.frame(self.key, now), width)
        };
        let mut style = Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            ..Style::default()
        };
        style.size.width = relative(1.).into();
        style.min_size.width = px(0.).into();
        // A change needs the width of the last frame to measure the content.
        let (Some((from, t)), Some(width)) = (change, width) else {
            if self.absent {
                // The shrink has ended. The box stays at nothing until the
                // next render drops it, so it never takes the minimum
                // height of a dragged panel.
                style.flex_shrink = 0.;
                style.size.height = px(0.).into();
            } else if self.shrink {
                style.flex_shrink = 1.;
                style.min_size.height = rems(super::panel_resize::MIN_REM).into();
                style.overflow.y = Overflow::Hidden;
            } else {
                style.flex_shrink = 0.;
            }
            let child = self.child.request_layout(window, cx);
            return (window.request_layout(style, [child], cx), Mode::Rest);
        };
        let target = measure(&mut self.child, width, window, cx);
        let height = if (target - from).abs() < px(0.5) {
            // Nothing to animate: end the change now.
            if let Some(state) = self.motion.borrow_mut().panels.get_mut(self.name) {
                state.change = None;
            }
            target
        } else {
            window.request_animation_frame();
            from + (target - from) * t
        };
        style.flex_shrink = 0.;
        style.size.height = height.into();
        style.overflow.y = Overflow::Hidden;
        (
            window.request_layout(style, None, cx),
            Mode::Changing { width },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        mode: &mut Mode,
        window: &mut Window,
        cx: &mut App,
    ) {
        {
            let mut store = self.motion.borrow_mut();
            store.width = Some(bounds.size.width);
            if let Some(state) = store.panels.get_mut(self.name) {
                state.shown = bounds.size.height;
            }
        }
        match mode {
            Mode::Rest => {
                self.child.prepaint(window, cx);
            }
            Mode::Changing { width } => {
                // The column changed width since the last frame.
                if *width != bounds.size.width {
                    measure(&mut self.child, bounds.size.width, window, cx);
                }
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    self.child.prepaint_at(bounds.origin, window, cx);
                });
            }
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        mode: &mut Mode,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        match mode {
            Mode::Rest => self.child.paint(window, cx),
            Mode::Changing { .. } => {
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    self.child.paint(window, cx);
                });
            }
        }
    }
}

/// Lays `child` out on its own at `width`, and returns its height.
fn measure(child: &mut AnyElement, width: Pixels, window: &mut Window, cx: &mut App) -> Pixels {
    child
        .layout_as_root(
            size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
            window,
            cx,
        )
        .height
}

/// How long a panel's height change runs, for tests.
#[cfg(test)]
pub const LENGTH: std::time::Duration = motion::RESIZE;

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> IssueId {
        IssueId::from_u128(n)
    }

    #[test]
    fn the_first_task_and_the_same_task_never_animate() {
        let now = Instant::now();
        let mut state = PanelState::default();
        assert_eq!(
            state.frame(id(1), now),
            None,
            "the first task shows at once"
        );
        state.shown = px(40.);
        assert_eq!(state.frame(id(1), now), None, "an edit does not animate");
    }

    #[test]
    fn a_new_task_eases_from_the_height_on_screen() {
        let now = Instant::now();
        let mut state = PanelState::default();
        state.frame(id(1), now);
        state.shown = px(40.);
        let (from, t) = state.frame(id(2), now).unwrap();
        assert_eq!(from, px(40.));
        assert_eq!(t, 0.);
        let (_, mid) = state.frame(id(2), now + LENGTH / 2).unwrap();
        assert!(mid > 0.5 && mid < 1., "it eases out, got {mid}");
        assert_eq!(state.frame(id(2), now + LENGTH), None, "it ends on time");
    }

    #[test]
    fn another_move_mid_way_starts_from_where_the_panel_is() {
        let now = Instant::now();
        let mut state = PanelState::default();
        state.frame(id(1), now);
        state.shown = px(40.);
        state.frame(id(2), now);
        // Mid-way, the box is 70 px tall on screen.
        let later = now + LENGTH / 3;
        state.shown = px(70.);
        let (from, t) = state.frame(id(3), later).unwrap();
        assert_eq!(from, px(70.));
        assert_eq!(t, 0., "the new change starts now, without a wait");
    }

    #[test]
    fn a_panel_the_next_task_lacks_shrinks_as_an_empty_box() {
        let now = Instant::now();
        let mut state = PanelState::default();
        state.frame(id(1), now);
        state.shown = px(40.);
        assert!(state.absent(id(2)), "it still draws, to shrink");
        state.frame(id(2), now);
        state.shown = px(20.);
        assert!(state.absent(id(2)), "it draws while it shrinks");
        assert_eq!(state.frame(id(2), now + LENGTH), None);
        state.shown = px(0.);
        assert!(!state.absent(id(2)), "then it is gone");
    }

    #[test]
    fn a_panel_gone_within_the_same_task_goes_at_once() {
        let now = Instant::now();
        let mut state = PanelState::default();
        state.frame(id(1), now);
        state.shown = px(40.);
        assert!(!state.absent(id(1)));
        assert_eq!(state.shown, px(0.));
        // A later task with the panel grows it from nothing.
        let (from, _) = state.frame(id(2), now).unwrap();
        assert_eq!(from, px(0.));
    }
}
