//! What the cat does, as plain state with no GPUI. The view calls
//! `advance` when a timeline step has run its time, and tells it about
//! agents, the pointer and clicks.
//!
//! 1. An agent works on a task: the cat types on its laptop.
//! 2. Nothing goes on: the cat sleeps.
//! 3. A hover or a click: the cat sits and watches. It keeps sitting while
//!    the pointer stays on it. Then it walks or naps, picked at random, and
//!    goes back to rule 1 or 2.

use std::ops::RangeInclusive;

use super::sprites::{AnimId, Sprites};

/// How long the cat watches after the pointer leaves it.
pub const WATCH_MS: RangeInclusive<u64> = 4_000..=8_000;
/// How long the cat walks or naps after it watched.
pub const ROAM_MS: RangeInclusive<u64> = 6_000..=10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Act {
    /// Rule 1: an agent works.
    Laptop,
    /// Rule 2: nothing goes on.
    Sleep,
    /// Sits and watches the pointer.
    Watch,
    /// Walks after it watched.
    Walk,
    /// Sleeps after it watched, for a while only.
    Nap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub struct Behaviour {
    act: Act,
    step: usize,
    /// Time left in Watch, Walk or Nap. Watch counts down only while the
    /// pointer is off the cat.
    left_ms: u64,
    /// The frame's left edge, in sprite pixels from the strip's left edge.
    x: i32,
    facing: Facing,
    /// The lowest and the highest `x` that keep the cat inside the strip.
    /// It is unknown until the strip has a width.
    range: Option<(i32, i32)>,
    hovered: bool,
    active: bool,
    rng: u64,
}

impl Behaviour {
    /// A cat at the left edge, at work or asleep. `seed` picks the random
    /// choices; any value works.
    pub fn new(active: bool, seed: u64) -> Self {
        Self {
            act: rest(active),
            step: 0,
            left_ms: 0,
            x: 0,
            facing: Facing::Right,
            range: None,
            hovered: false,
            active,
            // Xorshift needs a seed other than zero.
            rng: seed | 1,
        }
    }

    pub fn act(&self) -> Act {
        self.act
    }

    pub fn step(&self) -> usize {
        self.step
    }

    pub fn x(&self) -> i32 {
        self.x
    }

    pub fn facing(&self) -> Facing {
        self.facing
    }

    pub fn left_ms(&self) -> u64 {
        self.left_ms
    }

    pub fn anim(&self) -> AnimId {
        match self.act {
            Act::Laptop => AnimId::Laptop,
            Act::Sleep | Act::Nap => AnimId::Sleep,
            Act::Watch => AnimId::Watch,
            Act::Walk if self.facing == Facing::Left => AnimId::WalkLeft,
            Act::Walk => AnimId::WalkRight,
        }
    }

    /// How long the current step shows.
    pub fn step_ms(&self, sprites: &Sprites) -> u64 {
        let timeline = &sprites.anim(self.anim()).timeline;
        timeline[self.step % timeline.len()].ms
    }

    /// Sets the columns the frame may start at. The first range puts the
    /// cat at the left edge.
    pub fn set_range(&mut self, low: i32, high: i32) {
        let high = high.max(low);
        self.x = match self.range {
            None => low,
            Some(_) => self.x.clamp(low, high),
        };
        self.range = Some((low, high));
    }

    /// Whether an agent works on a task. Returns true when the cat changed.
    pub fn set_active(&mut self, active: bool) -> bool {
        self.active = active;
        if matches!(self.act, Act::Laptop | Act::Sleep) && self.act != rest(active) {
            self.start(rest(active));
            return true;
        }
        false
    }

    /// The pointer moved onto or off the cat. Returns true when the cat
    /// changed.
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        if hovered && self.act != Act::Watch {
            self.start(Act::Watch);
            return true;
        }
        if !hovered && self.act == Act::Watch {
            // The watch time starts when the pointer leaves.
            self.left_ms = self.pick(WATCH_MS);
        }
        false
    }

    /// A click: the cat sits and looks happy, at once.
    pub fn poke(&mut self, sprites: &Sprites) {
        self.start(Act::Watch);
        self.step = sprites.watch_happy;
    }

    /// The current step has run its time: show the next one. A walk moves
    /// the cat one sprite pixel per step, and it turns at the edges.
    pub fn advance(&mut self, sprites: &Sprites) {
        let spent = self.step_ms(sprites);
        if self.act == Act::Walk {
            self.walk();
        }
        let len = sprites.anim(self.anim()).timeline.len();
        self.step = (self.step + 1) % len;
        let counts = match self.act {
            Act::Laptop | Act::Sleep => false,
            Act::Watch => !self.hovered,
            Act::Walk | Act::Nap => true,
        };
        if !counts {
            return;
        }
        self.left_ms = self.left_ms.saturating_sub(spent);
        if self.left_ms > 0 {
            return;
        }
        match self.act {
            Act::Watch => {
                let next = if self.next().is_multiple_of(2) {
                    Act::Walk
                } else {
                    Act::Nap
                };
                self.start(next);
            }
            _ => self.start(rest(self.active)),
        }
    }

    fn range(&self) -> (i32, i32) {
        self.range.unwrap_or((self.x, self.x))
    }

    fn walk(&mut self) {
        let (low, high) = self.range();
        let dx = if self.facing == Facing::Right { 1 } else { -1 };
        if (low..=high).contains(&(self.x + dx)) {
            self.x += dx;
        } else if low < high {
            self.facing = flip(self.facing);
            self.x -= dx;
        }
    }

    fn start(&mut self, act: Act) {
        self.act = act;
        self.step = 0;
        self.left_ms = match act {
            Act::Laptop | Act::Sleep => 0,
            Act::Watch => self.pick(WATCH_MS),
            Act::Walk | Act::Nap => self.pick(ROAM_MS),
        };
        if act == Act::Walk {
            let (low, high) = self.range();
            self.facing = if self.x <= low {
                Facing::Right
            } else if self.x >= high {
                Facing::Left
            } else if self.next().is_multiple_of(2) {
                Facing::Right
            } else {
                Facing::Left
            };
        }
    }

    fn pick(&mut self, range: RangeInclusive<u64>) -> u64 {
        let span = range.end() - range.start() + 1;
        range.start() + self.next() % span
    }

    /// Xorshift64: small, and enough for a cat.
    fn next(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x >> 11
    }
}

fn rest(active: bool) -> Act {
    if active { Act::Laptop } else { Act::Sleep }
}

fn flip(facing: Facing) -> Facing {
    match facing {
        Facing::Left => Facing::Right,
        Facing::Right => Facing::Left,
    }
}
