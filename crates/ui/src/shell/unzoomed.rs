//! An element that draws its child at 100% zoom (LLM-212).
//!
//! Zoom scales the window's rem size, and the app sizes almost everything in
//! rems. The title bar must keep one size, so it draws with the rem size of
//! 100% zoom. Every rem inside it, for text, gaps, buttons and icons, then
//! keeps its actual size.

use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

/// Draws `child` at 100% zoom, whatever the zoom level.
pub fn unzoomed(child: impl IntoElement) -> Unzoomed {
    Unzoomed {
        child: child.into_any_element(),
    }
}

pub struct Unzoomed {
    child: AnyElement,
}

impl Element for Unzoomed {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let rem = crate::shell::theme::unzoomed_rem_size();
        let layout_id =
            window.with_rem_size(Some(rem), |window| self.child.request_layout(window, cx));
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let rem = crate::shell::theme::unzoomed_rem_size();
        window.with_rem_size(Some(rem), |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let rem = crate::shell::theme::unzoomed_rem_size();
        window.with_rem_size(Some(rem), |window| self.child.paint(window, cx));
    }
}

impl IntoElement for Unzoomed {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
