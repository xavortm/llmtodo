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
    Animation, AnimationExt as _, AnyElement, App, Bounds, Div, ElementId, Global, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, Pixels, Rgba, Stateful,
    StatefulInteractiveElement as _, Styled, Window, prelude::FluentBuilder as _, px, rems,
};

/// Whether motion is on, as a global so every view reads one value.
struct Motion(bool);

impl Global for Motion {}

/// Whether the Animations switch is on. This is what Settings shows and
/// saves.
pub fn enabled(cx: &App) -> bool {
    cx.try_global::<Motion>().is_some_and(|m| m.0)
}

/// Whether to animate now: the switch is on, and the system's Reduce
/// motion setting is off. GPUI's own animations follow that setting too.
pub fn on(cx: &App) -> bool {
    enabled(cx) && !cx.reduce_motion()
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
/// The shared tab hover background fading in. Short enough to feel
/// instant, long enough to show as a fade. Row fills show at once.
const FILL_IN: Duration = Duration::from_millis(90);
/// A hover, press or selection fill going away.
const FILL_OUT: Duration = Duration::from_millis(240);
/// A group's chevron turning.
const TURN: Duration = Duration::from_millis(180);
/// How many rows a wave spreads over. Rows below come in with the last.
/// More than a full window of rows, so no visible row waits for a jump.
const WAVE_ROWS: f32 = 40.;
/// How far a wave row moves into place.
const WAVE_TRAVEL: f32 = 8.;
/// A group closing: the rows under it slide up over its rows.
pub const CLOSE: Duration = Duration::from_millis(180);
/// How far new content rises into place.
const SWAP_TRAVEL: f32 = 4.;
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

/// The chevron of a group header turning, from 0 for closed to 1 for open.
/// Off, it is the end value at once.
pub fn turn(id: impl Into<ElementId>, open: bool, window: &mut Window, cx: &mut App) -> f32 {
    let target = if open { 1. } else { 0. };
    if !on(cx) {
        return target;
    }
    let id: ElementId = id.into();
    gpui_kit::base::transition(
        id,
        target,
        gpui_kit::base::Transition::new(TURN).ease(ease_out),
        window,
        cx,
    )
}

/// How far a panel such as the sidebar is open, from 0 for closed to 1
/// for open. On, it eases between them both ways, and a change mid-way
/// turns back from where it is. Off, it is the end value at once.
pub fn panel_open(id: impl Into<ElementId>, open: bool, window: &mut Window, cx: &mut App) -> f32 {
    let target = if open { 1. } else { 0. };
    if !on(cx) {
        return target;
    }
    let id: ElementId = id.into();
    gpui_kit::base::transition(
        id,
        target,
        gpui_kit::base::Transition::new(SLIDE).ease(ease_out),
        window,
        cx,
    )
}

/// Rows fading in from top to bottom, each a little after the row above.
/// The front of the wave starts fast and slows down as it reaches the
/// bottom, so it settles like every other animation here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    /// The rows of a group that just opened. They drop into place.
    Open,
    /// Every visible row after a new sort or grouping. They rise into
    /// place, and faster, so the new order reads at once.
    Sort,
}

impl Wave {
    /// How long the front takes to reach the last row.
    fn spread(self) -> Duration {
        match self {
            Self::Open => Duration::from_millis(200),
            Self::Sort => Duration::from_millis(160),
        }
    }

    /// How long one row takes to come in.
    fn row(self) -> Duration {
        match self {
            Self::Open => Duration::from_millis(200),
            Self::Sort => Duration::from_millis(150),
        }
    }

    /// How long the whole wave runs.
    pub fn total(self) -> Duration {
        self.spread() + self.row()
    }

    /// Where the row `index` places from the wave's top is, from 0 to 1,
    /// `elapsed` after the wave starts.
    pub fn step(self, elapsed: Duration, index: usize) -> f32 {
        // The front follows the ease-out curve, so the row at fraction `f`
        // of the way down starts when ease_out(t) = f.
        let f = (index as f32 / WAVE_ROWS).min(1.);
        let start = 1. - (1. - f).cbrt();
        let delay = self.spread().mul_f32(start);
        let t = elapsed.saturating_sub(delay).as_secs_f32() / self.row().as_secs_f32();
        ease_out(t)
    }

    /// Places a row at its step `t`.
    pub fn place<E: Styled>(self, el: E, t: f32) -> E {
        if t >= 1. {
            return el;
        }
        let travel = match self {
            Self::Open => -WAVE_TRAVEL,
            Self::Sort => WAVE_TRAVEL / 2.,
        };
        el.opacity(t).top(px(travel * (1. - t)))
    }
}

/// How much of a closing group is still showing, from 1 at the start to 0,
/// `elapsed` after it starts.
pub fn closing_left(elapsed: Duration) -> f32 {
    1. - ease_out(elapsed.as_secs_f32() / CLOSE.as_secs_f32())
}

/// A list column hiding or coming back as the list gets narrower or wider.
pub const COLUMN: Duration = Duration::from_millis(200);

/// Things that come and go, such as the list's columns. Each eases between
/// hidden, 0, and shown, 1, over `COLUMN`. A change mid-way turns back from
/// where the thing is, and takes only the time for the distance left, so a
/// quick change back and forth never jumps, restarts or stacks.
pub struct Presence<K> {
    items: Vec<(K, Tween)>,
}

impl<K> Default for Presence<K> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug)]
struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    length: Duration,
}

impl Tween {
    fn at(&self, now: Instant) -> f32 {
        if self.length.is_zero() {
            return self.to;
        }
        let t = now.saturating_duration_since(self.start).as_secs_f32() / self.length.as_secs_f32();
        self.from + (self.to - self.from) * ease_out(t)
    }

    fn done(&self, now: Instant) -> bool {
        now >= self.start + self.length
    }
}

impl<K: Copy + PartialEq> Presence<K> {
    /// `key` turns shown or hidden at `now`. It starts from where it is,
    /// or from the other end when it was at rest.
    pub fn change(&mut self, key: K, shown: bool, now: Instant) {
        let to = if shown { 1. } else { 0. };
        let from = self.get(key, now).unwrap_or(1. - to);
        self.items.retain(|(k, _)| *k != key);
        self.items.push((
            key,
            Tween {
                from,
                to,
                start: now,
                length: COLUMN.mul_f32((to - from).abs()),
            },
        ));
    }

    /// How far `key` shows at `now`, or None when it is at rest.
    pub fn get(&self, key: K, now: Instant) -> Option<f32> {
        self.items
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, tween)| tween.at(now))
    }

    /// Every key on its way, with how far it shows at `now`.
    pub fn all(&self, now: Instant) -> impl Iterator<Item = (K, f32)> + '_ {
        self.items.iter().map(move |(k, tween)| (*k, tween.at(now)))
    }

    /// Drops the keys that reached their end by `now`. True while any is
    /// still on its way.
    pub fn settle(&mut self, now: Instant) -> bool {
        self.items.retain(|(_, tween)| !tween.done(now));
        !self.items.is_empty()
    }

    /// Puts every key at its end at once.
    pub fn clear(&mut self) {
        self.items.clear();
    }
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

/// New content in the same place, such as another tab: it fades in and
/// rises a few pixels. The rise is a top margin.
pub fn swap_in<E>(el: E, id: impl Into<ElementId>, cx: &App) -> AnyElement
where
    E: Styled + IntoElement + 'static,
{
    if !on(cx) {
        return el.into_any_element();
    }
    el.with_animation(id, Animation::new(FADE).with_easing(ease_out), |el, t| {
        el.opacity(t).mt(px(SWAP_TRAVEL * (1. - t)))
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

/// A shared hover background moving from one item of a bar to the next,
/// such as a tab. Short, so it keeps up with the pointer.
const GLIDE: Duration = Duration::from_millis(150);

/// One hover background that the items of a bar share, such as tabs
/// (LLM-220). It slides to the item under the pointer and takes its width.
/// Coming from outside the bar, it fades in on that item without a slide.
/// When the pointer leaves the bar, it fades out.
///
/// It knows the items only by their index. Each call passes the items'
/// bounds as they are now, relative to the bar, so a bar that moves or
/// grows keeps its background on the right item.
#[derive(Clone, Copy, Debug, Default)]
pub struct Glide {
    /// The item under the pointer.
    hovered: Option<usize>,
    /// The item the background moves to. It stays on the last item while
    /// the background fades out.
    to: Option<usize>,
    /// Where the slide started.
    from: Option<Bounds<Pixels>>,
    slide_start: Option<Instant>,
    /// The fade: from, to, start and length.
    alpha: (f32, f32, Option<Instant>, Duration),
}

impl Glide {
    /// The item under the pointer.
    pub fn hovered(&self) -> Option<usize> {
        self.hovered
    }

    /// The pointer moved onto `item`, or off the bar for None.
    pub fn hover(&mut self, item: Option<usize>, items: &[Bounds<Pixels>], now: Instant) {
        if item == self.hovered {
            return;
        }
        let (shown, alpha) = match self.sample(items, now).0 {
            Some((rect, alpha)) => (Some(rect), alpha),
            None => (None, 0.),
        };
        self.hovered = item;
        match item {
            Some(ix) => {
                // A background still showing slides from where it is. One
                // that has faded out appears on the new item.
                let from = shown.filter(|_| alpha > 0.);
                self.slide_start = from.map(|_| now);
                self.from = from;
                self.to = Some(ix);
                self.alpha = (alpha, 1., Some(now), FILL_IN);
            }
            None => self.alpha = (alpha, 0., Some(now), FILL_OUT),
        }
    }

    /// Forgets the bar: the background is gone at once.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The background's bounds and opacity now, from 0 to 1, or None when
    /// it does not show. The flag says whether it still moves or fades.
    pub fn sample(
        &self,
        items: &[Bounds<Pixels>],
        now: Instant,
    ) -> (Option<(Bounds<Pixels>, f32)>, bool) {
        let progress = |start: Option<Instant>, length: Duration| {
            let Some(start) = start else {
                return (1., false);
            };
            let elapsed = now.saturating_duration_since(start);
            if length.is_zero() || elapsed >= length {
                return (1., false);
            }
            (ease_out(elapsed.as_secs_f32() / length.as_secs_f32()), true)
        };
        let (from_alpha, to_alpha, start, length) = self.alpha;
        let (t, fading) = progress(start, length);
        let alpha = from_alpha + (to_alpha - from_alpha) * t;
        let Some(to) = self.to.and_then(|ix| items.get(ix)).copied() else {
            return (None, false);
        };
        let (t, sliding) = progress(self.slide_start, GLIDE);
        let from = self.from.unwrap_or(to);
        let lerp = |a: Pixels, b: Pixels| a + (b - a) * t;
        let rect = Bounds {
            origin: gpui_kit::point(
                lerp(from.origin.x, to.origin.x),
                lerp(from.origin.y, to.origin.y),
            ),
            size: gpui_kit::size(
                lerp(from.size.width, to.size.width),
                lerp(from.size.height, to.size.height),
            ),
        };
        let running = fading || (sliding && alpha > 0.);
        ((alpha > 0.).then_some((rect, alpha)), running)
    }
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
/// `bg`, `hover` and `active` styles. On, a fill that turns on shows at
/// once, and one that goes away fades out. A change of the rest fill,
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

/// On is instant, off is soft. A hover shows the moment the mouse enters,
/// a press at the moment of the click, and a selection lands where it
/// moves to at once. Only a fill going away fades.
fn fade_length(from: Level, to: Level) -> Duration {
    if to > from { Duration::ZERO } else { FILL_OUT }
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
    fn a_wave_moves_top_to_bottom_and_ends_on_time() {
        for wave in [Wave::Open, Wave::Sort] {
            assert_eq!(wave.step(Duration::ZERO, 0), 0.);
            let at = Duration::from_millis(60);
            assert!(
                wave.step(at, 0) > wave.step(at, 3),
                "a lower row starts later"
            );
            assert_eq!(
                wave.step(at, 40),
                wave.step(at, 400),
                "the wave ends at row 40"
            );
            for index in [0, 5, 39, 40, 200] {
                assert_eq!(
                    wave.step(wave.total(), index),
                    1.,
                    "row {index} is in place"
                );
            }
        }
    }

    #[test]
    fn the_wave_front_slows_down_toward_the_bottom() {
        // The same number of rows takes longer to cross further down.
        let start = |i: usize| {
            (0..=1000)
                .map(Duration::from_millis)
                .find(|&at| Wave::Open.step(at, i) > 0.)
                .unwrap()
        };
        let top = start(10) - start(0);
        let bottom = start(39) - start(29);
        assert!(bottom > top * 2, "top {top:?} bottom {bottom:?}");
    }

    #[test]
    fn a_closing_group_shrinks_to_nothing_on_time() {
        assert_eq!(closing_left(Duration::ZERO), 1.);
        assert!(closing_left(CLOSE / 2) < 0.5, "it eases out");
        assert_eq!(closing_left(CLOSE), 0.);
    }

    #[test]
    fn a_column_fades_out_and_settles_on_time() {
        let start = Instant::now();
        let mut fades = Presence::default();
        fades.change(1, false, start);
        assert_eq!(fades.get(1, start), Some(1.), "it starts shown");
        let mid = fades.get(1, start + COLUMN / 2).unwrap();
        assert!(mid > 0. && mid < 0.5, "it eases out, got {mid}");
        assert_eq!(fades.get(1, start + COLUMN), Some(0.));
        assert!(fades.settle(start + COLUMN / 2));
        assert!(!fades.settle(start + COLUMN), "it ends on time");
        assert_eq!(fades.get(1, start + COLUMN), None);
    }

    #[test]
    fn a_column_that_turns_back_starts_where_it_was() {
        let start = Instant::now();
        let mut fades = Presence::default();
        fades.change(1, false, start);
        let turn = start + COLUMN / 4;
        let was = fades.get(1, turn).unwrap();
        fades.change(1, true, turn);
        assert_eq!(fades.get(1, turn), Some(was), "no jump");
        // The way back covers only the distance lost, so it ends sooner.
        let back = turn + COLUMN.mul_f32(1. - was);
        assert_eq!(fades.get(1, back), Some(1.));
        assert!(!fades.settle(back));
        // Another key runs on its own clock.
        fades.change(2, true, back);
        assert_eq!(fades.get(2, back), Some(0.), "it starts hidden");
        assert_eq!(fades.all(back).count(), 1);
    }

    #[test]
    fn the_first_draw_shows_the_target_at_once() {
        let mut state = FillState::default();
        let (color, running) = state.sample(fill(), Instant::now());
        assert_eq!(color, None, "no rest fill draws nothing");
        assert!(!running);
    }

    #[test]
    fn a_hover_shows_at_once_and_fades_on_leave() {
        let start = Instant::now();
        let mut state = FillState::default();
        state.sample(fill(), start);
        state.hovered = true;
        let (color, running) = state.sample(fill(), start);
        assert!(!running, "a hover never waits");
        assert!((color.unwrap().a - 0.1).abs() < 0.01);

        state.hovered = false;
        let (color, running) = state.sample(fill(), start);
        assert!(running, "the leave fades");
        assert!(color.is_some(), "it starts at the hover");
        let (color, running) = state.sample(fill(), start + FILL_OUT);
        assert!(!running, "the fade ends on time");
        assert_eq!(color, None, "it ends with no fill");
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

    /// Two tabs side by side: 100 px, then 60 px.
    fn tabs() -> [Bounds<Pixels>; 2] {
        [
            Bounds::new(
                gpui_kit::point(px(0.), px(0.)),
                gpui_kit::size(px(100.), px(32.)),
            ),
            Bounds::new(
                gpui_kit::point(px(100.), px(0.)),
                gpui_kit::size(px(60.), px(32.)),
            ),
        ]
    }

    #[test]
    fn entering_the_bar_fades_in_on_the_tab_without_a_slide() {
        let start = Instant::now();
        let mut glide = Glide::default();
        assert_eq!(glide.sample(&tabs(), start).0, None, "nothing at rest");
        glide.hover(Some(1), &tabs(), start);
        let mid = start + FILL_IN / 2;
        let (shown, running) = glide.sample(&tabs(), mid);
        let (rect, alpha) = shown.unwrap();
        assert!(running);
        assert_eq!(rect, tabs()[1], "it starts on the tab");
        assert!(alpha > 0. && alpha < 1., "it fades in, got {alpha}");
        let (shown, running) = glide.sample(&tabs(), start + FILL_IN);
        assert_eq!(shown, Some((tabs()[1], 1.)));
        assert!(!running, "the fade ends on time");
    }

    #[test]
    fn moving_to_the_next_tab_slides_and_fits_its_width() {
        let start = Instant::now();
        let mut glide = Glide::default();
        glide.hover(Some(0), &tabs(), start);
        let at = start + FILL_IN;
        // The pointer leaves one tab and enters the next in one move.
        glide.hover(None, &tabs(), at);
        glide.hover(Some(1), &tabs(), at);
        let (shown, running) = glide.sample(&tabs(), at + GLIDE / 2);
        let (rect, alpha) = shown.unwrap();
        assert!(running);
        assert_eq!(alpha, 1., "it stays fully shown while it slides");
        assert!(
            rect.origin.x > px(0.) && rect.origin.x < px(100.),
            "{rect:?}"
        );
        assert!(
            rect.size.width < px(100.) && rect.size.width > px(60.),
            "{rect:?}"
        );
        let (shown, running) = glide.sample(&tabs(), at + GLIDE);
        assert_eq!(shown, Some((tabs()[1], 1.)));
        assert!(!running, "the slide ends on time");
    }

    #[test]
    fn leaving_the_bar_fades_out_and_a_later_entry_does_not_slide() {
        let start = Instant::now();
        let mut glide = Glide::default();
        glide.hover(Some(0), &tabs(), start);
        let at = start + FILL_IN;
        glide.hover(None, &tabs(), at);
        let (shown, running) = glide.sample(&tabs(), at + FILL_OUT / 2);
        let (rect, alpha) = shown.unwrap();
        assert!(running);
        assert_eq!(rect, tabs()[0], "it fades where it was");
        assert!(alpha < 1.);
        let (shown, running) = glide.sample(&tabs(), at + FILL_OUT);
        assert_eq!(shown, None, "it is gone");
        assert!(!running);

        // Back in on the other tab: it appears there.
        let later = at + FILL_OUT;
        glide.hover(Some(1), &tabs(), later);
        let (shown, _) = glide.sample(&tabs(), later + FILL_IN / 2);
        assert_eq!(shown.unwrap().0, tabs()[1]);
    }

    #[test]
    fn coming_back_while_it_fades_slides_from_where_it_is() {
        let start = Instant::now();
        let mut glide = Glide::default();
        glide.hover(Some(0), &tabs(), start);
        let at = start + FILL_IN;
        glide.hover(None, &tabs(), at);
        let back = at + FILL_OUT / 4;
        glide.hover(Some(1), &tabs(), back);
        let (shown, _) = glide.sample(&tabs(), back + GLIDE / 2);
        let (rect, alpha) = shown.unwrap();
        assert!(
            rect.origin.x > px(0.) && rect.origin.x < px(100.),
            "{rect:?}"
        );
        assert!(alpha > 0.);
    }

    #[test]
    fn a_hover_that_comes_back_mid_fade_shows_at_once() {
        let start = Instant::now();
        let mut state = FillState::default();
        state.sample(fill(), start);
        state.hovered = true;
        state.sample(fill(), start);
        state.hovered = false;
        state.sample(fill(), start);
        let mid = start + FILL_OUT / 2;
        let (color, _) = state.sample(fill(), mid);
        let a = color.map_or(0., |c| c.a);
        assert!(a > 0. && a < 0.1, "it is partway out, got {a}");
        state.hovered = true;
        let (color, running) = state.sample(fill(), mid);
        assert!(!running, "the hover does not wait for the fade");
        assert!((color.unwrap().a - 0.1).abs() < 0.01);
    }
}
