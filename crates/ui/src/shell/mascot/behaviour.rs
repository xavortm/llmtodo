//! What the cat does, as plain state with no GPUI. The view calls
//! `advance` when a timeline step has run its time, and tells it about
//! agents, the pointer and clicks.
//!
//! While an agent works on a task, the cat works on its laptop:
//!
//! - A hover or a single click changes nothing.
//! - A double click walks it to the other side of the sidebar, where it
//!   works again.
//! - Every click during any walk is ignored, so clicks never stop or turn
//!   a walk.
//! - When the work ends away from the left edge, it walks back left. There
//!   it lies down to sleep, or sits and watches for a while and then
//!   sleeps, picked at random.
//!
//! While nothing goes on, the cat sleeps. A hover or a click makes it sit
//! and watch. It keeps sitting while the pointer stays on it. Then it walks
//! or naps, picked at random, and goes back to work or to sleep.
//!
//! Each time the sidebar opens, the cat waits out of view for 1 to 3 s.
//! Then it walks in from the left edge to its spot: the left edge, or the
//! right edge when it works there. Hover and clicks do nothing meanwhile.

use std::ops::RangeInclusive;

use super::sprites::{AnimId, Sprites};

/// How long the cat watches after the pointer leaves it.
pub const WATCH_MS: RangeInclusive<u64> = 4_000..=8_000;
/// How long the cat walks or naps after it watched.
pub const ROAM_MS: RangeInclusive<u64> = 6_000..=10_000;
/// How long the cat waits out of view after the sidebar opens.
pub const ENTER_DELAY_MS: RangeInclusive<u64> = 1_000..=3_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Act {
    /// An agent works: the cat types on its laptop.
    Laptop,
    /// Nothing goes on.
    Sleep,
    /// Sits and watches.
    Watch,
    /// Walks after it watched, turning at the edges.
    Walk,
    /// Sleeps after it watched, for a while only.
    Nap,
    /// Walks to one edge of the sidebar, then works or rests there.
    Commute,
    /// Waits out of view after the sidebar opened.
    Away,
    /// Walks in from out of view to its spot.
    Enter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Facing {
    Left,
    Right,
}

/// What follows a watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterWatch {
    /// A walk or a nap: the cat watched because of a hover or a click.
    Roam,
    /// Sleep, or work if an agent started meanwhile: the cat sat down
    /// after it walked home.
    Rest,
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
    /// The edge a commute walks to.
    goal: Facing,
    /// The edge the cat's spot is at: where it goes after a walk-in.
    home: Facing,
    /// An `x` that puts the whole cat past the left edge, out of view.
    away: i32,
    after_watch: AfterWatch,
    /// The lowest and the highest `x` that keep the whole cat inside the
    /// strip. It is unknown until the strip has a width.
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
            goal: Facing::Left,
            home: Facing::Left,
            away: 0,
            after_watch: AfterWatch::Roam,
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

    /// The `x` of the left and the right edge.
    pub fn edges(&self) -> (i32, i32) {
        self.range.unwrap_or((self.x, self.x))
    }

    /// Whether the cat is at work: it types, or walks to the side where it
    /// will type.
    fn working(&self) -> bool {
        self.active && matches!(self.act, Act::Laptop | Act::Commute)
    }

    /// Whether the cat waits for, or walks in, after the sidebar opened.
    fn entering(&self) -> bool {
        matches!(self.act, Act::Away | Act::Enter)
    }

    pub fn anim(&self) -> AnimId {
        match self.act {
            Act::Laptop => AnimId::Laptop,
            Act::Sleep | Act::Nap => AnimId::Sleep,
            Act::Watch => AnimId::Watch,
            Act::Walk | Act::Commute if self.facing == Facing::Left => AnimId::WalkLeft,
            Act::Walk | Act::Commute | Act::Away | Act::Enter => AnimId::WalkRight,
        }
    }

    /// Whether the cat is on screen. It is not while it waits out of view.
    pub fn visible(&self) -> bool {
        self.act != Act::Away
    }

    /// How long the current step shows. Out of view, the wait is one step.
    pub fn step_ms(&self, sprites: &Sprites) -> u64 {
        if self.act == Act::Away {
            return self.left_ms.max(1);
        }
        let timeline = &sprites.anim(self.anim()).timeline;
        timeline[self.step % timeline.len()].ms
    }

    /// Sets the columns the frame may start at, and `away`, a column that
    /// puts the whole cat out of view past the left edge. The first range
    /// puts the cat at the left edge, or out of view before a walk-in.
    pub fn set_range(&mut self, low: i32, high: i32, away: i32) {
        let high = high.max(low);
        self.away = away;
        self.x = match self.act {
            Act::Away => away,
            Act::Enter => self.x.clamp(away, high),
            _ if self.range.is_none() => low,
            _ => self.x.clamp(low, high),
        };
        self.range = Some((low, high));
    }

    /// The sidebar opened: the cat waits out of view, then walks in. A new
    /// open starts this again, so a walk-in cut off by a close starts over.
    pub fn enter(&mut self) {
        self.act = Act::Away;
        self.step = 0;
        self.x = self.away;
        self.facing = Facing::Right;
        self.after_watch = AfterWatch::Roam;
        self.left_ms = self.pick(ENTER_DELAY_MS);
    }

    /// Whether an agent works on a task. Returns true when the cat changed.
    pub fn set_active(&mut self, active: bool) -> bool {
        let was = self.active;
        self.active = active;
        if self.entering() {
            // The walk-in ends at work or asleep, as `active` says then.
            return false;
        }
        match self.act {
            Act::Sleep if active => self.start(Act::Laptop),
            Act::Laptop if !active => {
                if self.x > self.edges().0 {
                    self.commute(Facing::Left);
                } else {
                    self.start(Act::Sleep);
                }
            }
            // The work ended on the way to the right: turn for home.
            Act::Commute if was && !active && self.goal == Facing::Right => {
                self.commute(Facing::Left);
            }
            _ => return false,
        }
        true
    }

    /// The pointer moved onto or off the cat. Returns true when the cat
    /// changed.
    pub fn set_hovered(&mut self, hovered: bool) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.hovered = hovered;
        if self.entering() || self.act == Act::Commute || self.working() {
            return false;
        }
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

    /// A click, the `count`th in a row. Returns true when the cat changed.
    ///
    /// During any walk, every click is ignored. At work, a double click
    /// walks the cat to the other side and a single click does nothing.
    /// Otherwise any click makes it sit and look happy.
    pub fn click(&mut self, count: usize, sprites: &Sprites) -> bool {
        if matches!(self.act, Act::Away | Act::Enter | Act::Commute | Act::Walk) {
            return false;
        }
        if self.act == Act::Laptop {
            if count != 2 {
                return false;
            }
            let side = if self.x >= self.edges().1 {
                Facing::Left
            } else {
                Facing::Right
            };
            self.commute(side);
            return true;
        }
        self.start(Act::Watch);
        self.step = sprites.watch_happy;
        true
    }

    /// The current step has run its time: show the next one. Walking moves
    /// the cat one sprite pixel per step.
    pub fn advance(&mut self, sprites: &Sprites) {
        let spent = self.step_ms(sprites);
        match self.act {
            Act::Away => {
                self.act = Act::Enter;
                self.step = 0;
                self.x = self.away;
                return;
            }
            Act::Enter => {
                let spot = self.edge_x(self.home);
                self.x = (self.x + 1).min(spot);
                if self.x >= spot {
                    self.start(rest(self.active));
                    return;
                }
            }
            Act::Walk => self.walk(),
            Act::Commute => {
                let goal = self.goal_x();
                self.x += (goal - self.x).signum();
                if self.x == goal {
                    self.arrive();
                    return;
                }
            }
            _ => {}
        }
        let len = sprites.anim(self.anim()).timeline.len();
        self.step = (self.step + 1) % len;
        let counts = match self.act {
            Act::Laptop | Act::Sleep | Act::Commute | Act::Away | Act::Enter => false,
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
            Act::Watch if self.after_watch == AfterWatch::Roam => {
                let next = if self.coin() { Act::Walk } else { Act::Nap };
                self.start(next);
            }
            _ => self.start(rest(self.active)),
        }
    }

    fn edge_x(&self, side: Facing) -> i32 {
        let (low, high) = self.edges();
        if side == Facing::Left { low } else { high }
    }

    fn goal_x(&self) -> i32 {
        self.edge_x(self.goal)
    }

    /// Starts a walk to one edge. A cat already there arrives at once.
    fn commute(&mut self, goal: Facing) {
        self.act = Act::Commute;
        self.step = 0;
        self.left_ms = 0;
        self.goal = goal;
        self.home = goal;
        self.facing = goal;
        if self.x == self.goal_x() {
            self.arrive();
        }
    }

    /// The commute reached its edge. At work, the cat types there. Home
    /// after work, it sleeps or sits and watches first.
    fn arrive(&mut self) {
        if self.active {
            self.start(Act::Laptop);
        } else if self.coin() {
            self.start(Act::Sleep);
        } else {
            self.start(Act::Watch);
            self.after_watch = AfterWatch::Rest;
        }
    }

    fn walk(&mut self) {
        let (low, high) = self.edges();
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
        self.after_watch = AfterWatch::Roam;
        self.left_ms = match act {
            Act::Laptop | Act::Sleep | Act::Commute | Act::Away | Act::Enter => 0,
            Act::Watch => self.pick(WATCH_MS),
            Act::Walk | Act::Nap => self.pick(ROAM_MS),
        };
        if act == Act::Walk {
            let (low, high) = self.edges();
            self.facing = if self.x <= low {
                Facing::Right
            } else if self.x >= high {
                Facing::Left
            } else if self.coin() {
                Facing::Right
            } else {
                Facing::Left
            };
        }
    }

    fn coin(&mut self) -> bool {
        self.next().is_multiple_of(2)
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
