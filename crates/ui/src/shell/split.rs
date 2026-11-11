//! How the window width splits between the issue list and the task view
//! beside it, until the owner drags the divider.
//!
//! The list gets width first, up to the width at which its columns stop
//! cutting most of their content. After that point, the task view takes
//! every extra pixel. Both keep their minimum widths.

use gpui_kit::Pixels;

/// The task view's width beside the list.
///
/// `room` is the width of the list and the task view together. `list_want`
/// is the width at which the list stops cropping most content, from
/// `columns::comfortable_width`. When the room is too small for both
/// minimums, the task view keeps its minimum and the list gets the rest.
pub fn detail_width(
    room: Pixels,
    list_want: Pixels,
    list_min: Pixels,
    detail_min: Pixels,
) -> Pixels {
    let most = (room - list_min).max(detail_min);
    (room - list_want.max(list_min)).max(detail_min).min(most)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::px;

    /// A list that wants 800 px, a list minimum of 480 and a task view
    /// minimum of 352, as at 16 px per rem.
    fn split(room: f32) -> f32 {
        f32::from(detail_width(px(room), px(800.), px(480.), px(352.)))
    }

    #[test]
    fn the_list_grows_first_then_the_task_view() {
        // Both minimums: the list gets everything over the task view's.
        assert_eq!(split(900.), 352.);
        assert_eq!(split(1100.), 352.);
        // The list reaches 800 at 1152; the task view grows after that.
        assert_eq!(split(1152.), 352.);
        assert_eq!(split(1200.), 400.);
        assert_eq!(split(2000.), 1200.);
    }

    #[test]
    fn a_small_room_keeps_the_task_view_minimum() {
        // Less than both minimums: the list gives up its minimum first.
        assert_eq!(split(600.), 352.);
    }

    #[test]
    fn a_list_that_wants_less_than_its_minimum_keeps_the_minimum() {
        let d = detail_width(px(1000.), px(200.), px(480.), px(352.));
        assert_eq!(f32::from(d), 520.);
    }
}
