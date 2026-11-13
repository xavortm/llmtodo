//! Optional motion (LLM-195). The Animations switch in Settings turns it
//! on, and it is off by default. Off, every helper here returns the plain
//! element: the app looks and behaves as it did before, with no delay.
//!
//! The rules, so every animation feels the same:
//!
//! - Motion never makes the owner wait. A state that turns on shows at once
//!   or nearly so: a press, a selection, a hover. Only the way back fades.
//! - An entrance is short and settles with a soft ease-out, as on macOS.
//! - An animation redraws only the views it moves, and only while it runs
//!   (LLM-169). A panel that slides in draws again for those frames, then
//!   stays cached.

use std::time::{Duration, Instant};

use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Div, ElementId, Global, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, Rgba, Stateful,
    StatefulInteractiveElement as _, Styled, Window, prelude::FluentBuilder as _, px, rems,
};

/// Whether motion is on, as a global so every view reads one value.
struct Motion(bool);

impl Global for Motion {}

/// Whether the Animations switch is on.
pub fn on(cx: &App) -> bool {
    cx.try_global::<Motion>().is_some_and(|m| m.0)
}

pub fn set(on: bool, cx: &mut App) {
    cx.set_global(Motion(on));
}

/// A menu, the palette or a dialog appearing.
const ENTER: Duration = Duration::from_millis(200);
/// A tooltip appearing. GPUI already waits before it shows one.
const FADE: Duration = Duration::from_millis(150);
/// A panel sliding in: the task view or the sidebar.
const SLIDE: Duration = Duration::from_millis(220);
/// A hover fill coming in. Short enough to feel instant, long enough to
/// show as a fade.
const FILL_IN: Duration = Duration::from_millis(90);
/// A hover, press or selection fill going away.
const FILL_OUT: Duration = Duration::from_millis(240);
/// How far an overlay drops into place.
const ENTER_TRAVEL: f32 = 10.;

/// Ease-out cubic: quick at first, then a soft settle. A steeper curve ends
/// its visible change in the first two or three frames.
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}

/// An overlay card entering: it fades in and drops a few pixels into
/// place. The drop is a top margin, so it works on an absolute card and
/// must not go on one that sets its own. Keyed by `id`, so it plays each time the card mounts and never
/// while it stays open.
pub fn enter<E>(el: E, id: impl Into<ElementId>, cx: &App) -> AnyElement
where
    E: Styled + IntoElement + 'static,
{
    if !on(cx) {
        return el.into_any_element();
    }
    el.with_animation(id, Animation::new(ENTER).with_easing(ease_out), |el, t| {
        el.opacity(t).mt(px(-ENTER_TRAVEL * (1. - t)))
    })
    .into_any_element()
}

/// A small card, such as a tooltip, fading in where it stands.
pub fn fade_in<E>(el: E, id: impl Into<ElementId>, cx: &App) -> AnyElement
where
    E: Styled + IntoElement + 'static,
{
    if !on(cx) {
        return el.into_any_element();
    }
    el.with_animation(id, Animation::new(FADE).with_easing(ease_out), |el, t| {
        el.opacity(t)
    })
    .into_any_element()
}

/// A panel sliding in when it opens. It starts `from` rem to the side,
/// negative for the left, and settles in place. With `fade`, it also fades
/// in. Keyed by `id`, so it plays each time the panel mounts. It moves by a
/// `left` offset, so the panel must not set its own.
pub fn slide_in<E>(el: E, id: impl Into<ElementId>, from: f32, fade: bool, cx: &App) -> AnyElement
where
    E: Styled + IntoElement + 'static,
{
    if !on(cx) {
        return el.into_any_element();
    }
    el.with_animation(
        id,
        Animation::new(SLIDE).with_easing(ease_out),
        move |el, t| {
            el.relative()
                .left(rems(from * (1. - t)))
                .when(fade, |el| el.opacity(t))
        },
    )
    .into_any_element()
}

/// The fills of a clickable element: at rest, under the pointer and while
/// pressed. `rest` is None for no fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    pub rest: Option<Hsla>,
    pub hover: Hsla,
    pub pressed: Hsla,
}

/// Gives `el` its rest, hover and pressed fills. Off, this is the plain
/// `bg`, `hover` and `active` styles. On, the fill fades between them: in
/// at once or nearly, out over a short fade. A change of the rest fill,
/// such as a selection that moves away, fades the same way.
///
/// `key` names the state for this element. Use one that follows the data,
/// such as the issue, so a row that scrolls or moves keeps its own fade.
pub fn fill(
    el: Stateful<Div>,
    key: impl Into<ElementId>,
    fill: Fill,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    if !on(cx) {
        let Fill {
            rest,
            hover,
            pressed,
        } = fill;
        return el
            .when_some(rest, |el, rest| el.bg(rest))
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed));
    }
    let state = window.use_keyed_state(key, cx, |_, _| FillState::default());
    let now = cx.background_executor().now();
    let (color, running) = state.update(cx, |state, _| state.sample(fill, now));
    if running {
        window.request_animation_frame();
    }
    let hover_state = state.clone();
    let down_state = state.clone();
    let up_state = state.clone();
    el.when_some(color, |el, color| el.bg(color))
        .on_hover(move |hovered, _, cx| {
            hover_state.update(cx, |s, cx| {
                s.hovered = *hovered;
                if !*hovered {
                    s.pressed = false;
                }
                cx.notify();
            });
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            down_state.update(cx, |s, cx| {
                s.pressed = true;
                cx.notify();
            });
        })
        .on_mouse_up(MouseButton::Left, {
            let state = up_state.clone();
            move |_, _, cx| release(&state, cx)
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| release(&up_state, cx))
}

fn release(state: &gpui_kit::Entity<FillState>, cx: &mut App) {
    state.update(cx, |s, cx| {
        if s.pressed {
            s.pressed = false;
            cx.notify();
        }
    });
}

/// Where an element's fill is, and where it goes.
#[derive(Default)]
struct FillState {
    hovered: bool,
    pressed: bool,
    /// The fade: from, to, start and length. None before the first draw.
    fade: Option<Fade>,
}

#[derive(Clone, Copy)]
struct Fade {
    from: Rgba,
    to: Rgba,
    /// What `to` shows, so a later change knows its direction.
    level: Level,
    start: Instant,
    length: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Level {
    Clear,
    Rest,
    Hover,
    Pressed,
}

impl FillState {
    /// The fill to draw now, and whether the fade still runs.
    fn sample(&mut self, fill: Fill, now: Instant) -> (Option<Hsla>, bool) {
        let (level, target) = if self.pressed {
            (Level::Pressed, fill.pressed)
        } else if self.hovered {
            (Level::Hover, fill.hover)
        } else if let Some(rest) = fill.rest {
            (Level::Rest, rest)
        } else {
            // No fill: the hover color, clear, so a fade keeps its hue.
            (Level::Clear, fill.hover.opacity(0.))
        };
        let target = target.to_rgb();
        let fade = match self.fade {
            None => Fade {
                from: target,
                to: target,
                level,
                start: now,
                length: Duration::ZERO,
            },
            Some(fade) if fade.to == target => Fade { level, ..fade },
            Some(fade) => Fade {
                from: fade.at(now),
                to: target,
                level,
                start: now,
                length: fade_length(fade.level, level),
            },
        };
        self.fade = Some(fade);
        let color = fade.at(now);
        let running = now < fade.start + fade.length;
        let color: Hsla = color.into();
        ((color.a > 0.).then_some(color), running)
    }
}

/// On is fast, off is soft. A press shows at the moment of the click, and
/// a selection lands where it moves to at once.
fn fade_length(from: Level, to: Level) -> Duration {
    match (from, to) {
        (_, Level::Pressed) => Duration::ZERO,
        (Level::Clear, Level::Rest) => Duration::ZERO,
        (from, to) if to > from => FILL_IN,
        _ => FILL_OUT,
    }
}

impl Fade {
    fn at(&self, now: Instant) -> Rgba {
        if self.length.is_zero() {
            return self.to;
        }
        let t = now.saturating_duration_since(self.start).as_secs_f32() / self.length.as_secs_f32();
        let t = ease_out(t);
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        Rgba {
            r: lerp(self.from.r, self.to.r),
            g: lerp(self.from.g, self.to.g),
            b: lerp(self.from.b, self.to.b),
            a: lerp(self.from.a, self.to.a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill() -> Fill {
        Fill {
            rest: None,
            hover: gpui_kit::hsla(0., 0., 0.5, 0.1),
            pressed: gpui_kit::hsla(0., 0., 0.5, 0.2),
        }
    }

    #[test]
    fn the_first_draw_shows_the_target_at_once() {
        let mut state = FillState::default();
        let (color, running) = state.sample(fill(), Instant::now());
        assert_eq!(color, None, "no rest fill draws nothing");
        assert!(!running);
    }

    #[test]
    fn a_press_shows_at_once_and_the_release_fades() {
        let start = Instant::now();
        let mut state = FillState::default();
        state.sample(fill(), start);
        state.hovered = true;
        state.pressed = true;
        let (color, running) = state.sample(fill(), start);
        assert!(!running, "a press never waits");
        assert!((color.unwrap().a - 0.2).abs() < 0.01);

        state.pressed = false;
        let (color, running) = state.sample(fill(), start);
        assert!(running, "the release fades");
        assert!(
            (color.unwrap().a - 0.2).abs() < 0.01,
            "it starts at the press"
        );
        let (color, running) = state.sample(fill(), start + FILL_OUT);
        assert!(!running, "the fade ends on time");
        assert!(
            (color.unwrap().a - 0.1).abs() < 0.01,
            "it ends at the hover"
        );
    }

    #[test]
    fn a_selection_lands_at_once_and_leaves_with_a_fade() {
        let start = Instant::now();
        let mut state = FillState::default();
        state.sample(fill(), start);
        let selected = Fill {
            rest: Some(gpui_kit::hsla(0., 0., 0.5, 0.3)),
            ..fill()
        };
        let (_, running) = state.sample(selected, start);
        assert!(!running, "the new selection shows at once");
        let (color, running) = state.sample(fill(), start);
        assert!(running, "the old selection fades");
        assert!(color.is_some());
        let (color, _) = state.sample(fill(), start + FILL_OUT);
        assert_eq!(color, None, "it ends with no fill");
    }

    #[test]
    fn a_reversed_fade_starts_where_the_last_one_was() {
        let start = Instant::now();
        let mut state = FillState::default();
        state.sample(fill(), start);
        state.hovered = true;
        state.sample(fill(), start);
        state.hovered = false;
        let mid = start + FILL_IN / 2;
        let (color, _) = state.sample(fill(), mid);
        let a = color.map_or(0., |c| c.a);
        assert!(
            a > 0. && a < 0.1,
            "it leaves from the partway fill, got {a}"
        );
    }
}
