//! The pixel cat at the bottom of the sidebar. It types on a laptop while
//! an agent works on a task, and a double click walks it to the other side.
//! It sleeps when nothing goes on, and a hover or a click makes it sit and
//! watch, then walk or nap. `behaviour.rs` holds those rules, and
//! `sprites.rs` the art.
//!
//! The cat has no hover or pressed style. The owner asked for that, as the
//! one exception to the rule that every clickable element shows both.
//!
//! The mascot is its own entity, so a new frame notifies only this view.
//! Its timer runs only while the sidebar is on screen: the workspace calls
//! `set_shown`.

pub mod behaviour;
pub mod sprites;

#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    Bounds, ClickEvent, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, StatefulInteractiveElement as _, Styled as _, Task, Window,
    canvas, div, point, prelude::FluentBuilder as _, px, rems, size,
};

use crate::model::store::IssueStore;
use behaviour::Behaviour;
use sprites::{Run, sprites};

/// One sprite pixel, in rems: 2 px at the default zoom, so the 32-pixel
/// frame is 64 px.
const PIXEL_REMS: f32 = 0.125;

/// The gap between the cat and the sidebar's edges, in sprite pixels.
const INSET: i32 = 2;

pub const CAT_ID: &str = "mascot-cat";

/// Whether an agent works on any task not in the Trash: it holds a claim,
/// or an Analyzing mark that has not expired at `now`.
pub fn agent_active(store: &IssueStore, now: jiff::Timestamp) -> bool {
    store.iter().any(|issue| {
        !issue.deleted
            && (issue.claim.is_some() || issue.analyzing.as_ref().is_some_and(|a| a.is_live(now)))
    })
}

pub struct Mascot {
    store: Entity<IssueStore>,
    behaviour: Behaviour,
    shown: bool,
    /// Shows each timeline step for its time. None while hidden.
    ticker: Option<Task<()>>,
    /// The strip's width and one sprite pixel, from the last layout.
    measured: Rc<Cell<Option<(Pixels, Pixels)>>>,
    _store: gpui_kit::Subscription,
}

impl Mascot {
    pub fn new(store: Entity<IssueStore>, cx: &mut Context<Self>) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        let active = agent_active(store.read(cx), jiff::Timestamp::now());
        let subscription = cx.observe(&store, |this, _, cx| this.refresh_active(cx));
        Self {
            store,
            behaviour: Behaviour::new(active, seed),
            shown: false,
            ticker: None,
            measured: Rc::new(Cell::new(None)),
            _store: subscription,
        }
    }

    #[cfg(test)]
    pub fn behaviour(&self) -> &Behaviour {
        &self.behaviour
    }

    /// One timeline step, as the timer would run it.
    #[cfg(test)]
    pub fn tick(&mut self, cx: &mut Context<Self>) {
        self.behaviour.advance(sprites());
        cx.notify();
    }

    #[cfg(test)]
    pub fn ticking(&self) -> bool {
        self.ticker.is_some()
    }

    /// The sidebar showed or hid. The timer runs only while it shows.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        if shown {
            // Claims may have changed while the cat was off screen.
            self.behaviour
                .set_active(agent_active(self.store.read(cx), jiff::Timestamp::now()));
            // Every open, the start of the app too, begins with a walk-in.
            self.behaviour.enter();
        }
        self.restart(cx);
    }

    fn refresh_active(&mut self, cx: &mut Context<Self>) {
        let active = agent_active(self.store.read(cx), jiff::Timestamp::now());
        if self.behaviour.set_active(active) {
            self.restart(cx);
        }
    }

    fn set_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.behaviour.set_hovered(hovered) {
            self.restart(cx);
        }
    }

    fn click(&mut self, count: usize, cx: &mut Context<Self>) {
        if self.behaviour.click(count, sprites()) {
            self.restart(cx);
        }
    }

    /// Draws the current step now and times the next one from now.
    fn restart(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if !self.shown {
            self.ticker = None;
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                let Ok(ms) = this.update(cx, |this, _| this.behaviour.step_ms(sprites())) else {
                    break;
                };
                cx.background_executor()
                    .timer(Duration::from_millis(ms))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    this.behaviour.advance(sprites());
                    // An Analyzing mark ends without a server event, so the
                    // cat looks again each time an animation starts over.
                    if this.behaviour.step() == 0 {
                        let active = agent_active(this.store.read(cx), jiff::Timestamp::now());
                        this.behaviour.set_active(active);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    /// Fits the walk to the strip's width from the last layout. The strip
    /// is as wide as the sidebar, so it clips the cat at the sidebar's edge.
    fn sync_range(&mut self) {
        let Some((width, pixel)) = self.measured.get() else {
            return;
        };
        let art = sprites();
        let columns = (width / pixel).floor() as i32;
        let low = INSET - i32::from(art.cols.0);
        let high = columns - 1 - INSET - i32::from(art.cols.1);
        let away = -1 - i32::from(art.cols.1);
        self.behaviour.set_range(low, high, away);
    }
}

impl Render for Mascot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_range();
        let art = sprites();
        let runs = art.frame(self.behaviour.anim(), self.behaviour.step());
        let theme = cx.theme();
        let palette = art.palette(theme.is_dark());
        let frame = rems(PIXEL_REMS * f32::from(art.size));
        let measured = self.measured.clone();
        let entity = cx.entity_id();
        let visible = self.behaviour.visible();
        div()
            .id("mascot")
            .relative()
            .w_full()
            .h(frame)
            .flex_none()
            .overflow_hidden()
            // Notes the strip's width, so the walk turns at its edges.
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let now = Some((bounds.size.width, window.rem_size() * PIXEL_REMS));
                        if measured.replace(now) != now {
                            cx.notify(entity);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            // Out of view, the cat draws nothing, so it takes no hover.
            .when(visible, |this| {
                this.child(
                    div()
                        .id(CAT_ID)
                        .test_support()
                        .absolute()
                        .top_0()
                        .left(rems(PIXEL_REMS * self.behaviour.x() as f32))
                        .size(frame)
                        .cursor_pointer()
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            this.set_hovered(*hovered, cx);
                        }))
                        .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                            this.click(event.click_count(), cx);
                        }))
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |bounds, _, window, _| {
                                    paint_frame(runs, palette, bounds, window)
                                },
                            )
                            .size_full(),
                        ),
                )
            })
    }
}

/// Paints one frame, one quad per run. Each sprite pixel covers a whole
/// number of device pixels, so the edges stay sharp at any zoom. The frame
/// sits on the bottom edge of `bounds`.
fn paint_frame(runs: &[Run], palette: &[u32], bounds: Bounds<Pixels>, window: &mut Window) {
    let scale = window.scale_factor();
    let unit = (f32::from(window.rem_size() * PIXEL_REMS) * scale)
        .round()
        .max(1.)
        / scale;
    let snap = |v: Pixels| (f32::from(v) * scale).round() / scale;
    let left = snap(bounds.origin.x);
    let top = snap(bounds.origin.y + bounds.size.height) - unit * f32::from(sprites().size);
    for run in runs {
        let colour: Hsla = gpui_kit::rgb(palette[usize::from(run.key)]).into();
        let origin = point(
            px(left + unit * f32::from(run.x)),
            px(top + unit * f32::from(run.y)),
        );
        let extent = size(px(unit * f32::from(run.len)), px(unit));
        window.paint_quad(gpui_kit::fill(Bounds::new(origin, extent), colour));
    }
}
