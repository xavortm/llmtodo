//! Tests for the cat: the art file, the rules in `behaviour.rs`, and the
//! view in a real workspace on the test clock.

use std::time::Duration;

use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions, point, px,
};

use super::behaviour::{Act, Behaviour, Facing, ROAM_MS, WATCH_MS};
use super::sprites::{AnimId, sprites};
use super::{CAT_ID, Mascot};
use crate::model::data::{Analyzing, Catalog, Issue};
use crate::shell::workspace::Workspace;

// The art ------------------------------------------------------------------

#[test]
fn the_art_file_parses_with_every_animation() {
    let art = sprites();
    assert_eq!(art.size, 32);
    for id in [
        AnimId::Watch,
        AnimId::Laptop,
        AnimId::Sleep,
        AnimId::WalkRight,
        AnimId::WalkLeft,
    ] {
        let anim = art.anim(id);
        assert!(!anim.frames.is_empty(), "{id:?}");
        assert!(anim.frames.iter().all(|f| !f.is_empty()), "{id:?}");
    }
    // Both palettes colour every key, and they differ: a black cat and a
    // white one.
    assert_eq!(art.palette(false).len(), art.palette(true).len());
    assert_ne!(art.palette(false), art.palette(true));
    assert!(art.cols.0 <= art.cols.1 && art.cols.1 < 32);
}

#[test]
fn runs_merge_a_row_of_one_colour_and_cover_no_pixel_twice() {
    let art = sprites();
    for id in [AnimId::Laptop, AnimId::WalkLeft] {
        for frame in &art.anim(id).frames {
            let mut seen = [[false; 32]; 32];
            for (ix, run) in frame.iter().enumerate() {
                for x in run.x..run.x + run.len {
                    let cell = &mut seen[usize::from(run.y)][usize::from(x)];
                    assert!(!*cell, "{id:?} paints a pixel twice");
                    *cell = true;
                }
                // The next run in the same row starts a new colour.
                if let Some(next) = frame.get(ix + 1)
                    && next.y == run.y
                    && next.x == run.x + run.len
                {
                    assert_ne!(next.key, run.key);
                }
            }
        }
    }
}

#[test]
fn a_bad_file_is_refused() {
    assert!(super::sprites::Sprites::parse("{}").is_err());
}

// The rules ------------------------------------------------------------------

/// Runs steps until `ms` of animation time has passed.
fn run_for(cat: &mut Behaviour, ms: u64) {
    let mut spent = 0;
    while spent < ms {
        spent += cat.step_ms(sprites());
        cat.advance(sprites());
    }
}

/// Runs steps until the act changes, and returns the time that took.
fn run_to_next_act(cat: &mut Behaviour) -> u64 {
    let (from, mut spent) = (cat.act(), 0);
    while cat.act() == from {
        spent += cat.step_ms(sprites());
        cat.advance(sprites());
        assert!(spent < 60_000, "{from:?} never ends");
    }
    spent
}

/// A cat on a strip from -1 to 20 that watched and then started to walk.
fn walking_cat() -> Behaviour {
    for seed in 0.. {
        let mut cat = Behaviour::new(false, seed);
        cat.set_range(-1, 20);
        cat.set_hovered(true);
        cat.set_hovered(false);
        run_to_next_act(&mut cat);
        if cat.act() == Act::Walk {
            return cat;
        }
    }
    unreachable!()
}

#[test]
fn an_agent_at_work_means_the_laptop_and_none_means_sleep() {
    let mut cat = Behaviour::new(true, 7);
    assert_eq!(cat.act(), Act::Laptop);
    assert_eq!(cat.anim(), AnimId::Laptop);
    run_for(&mut cat, 60_000);
    assert_eq!(cat.act(), Act::Laptop, "the laptop has no end");

    assert!(cat.set_active(false));
    assert_eq!(cat.act(), Act::Sleep);
    assert_eq!(cat.step(), 0);
    run_for(&mut cat, 60_000);
    assert_eq!(cat.act(), Act::Sleep);
    assert!(cat.set_active(true));
    assert_eq!(cat.act(), Act::Laptop);
}

#[test]
fn a_hover_makes_it_watch_for_as_long_as_the_pointer_stays() {
    let mut cat = Behaviour::new(false, 7);
    assert!(cat.set_hovered(true));
    assert_eq!(cat.act(), Act::Watch);
    run_for(&mut cat, 60_000);
    assert_eq!(cat.act(), Act::Watch);
    // Agents come and go while it watches: it keeps watching.
    assert!(!cat.set_active(true));
    assert_eq!(cat.act(), Act::Watch);

    cat.set_hovered(false);
    assert!(WATCH_MS.contains(&cat.left_ms()));
    let watched = run_to_next_act(&mut cat);
    assert!(WATCH_MS.contains(&watched), "{watched}");
    assert!(matches!(cat.act(), Act::Walk | Act::Nap), "{:?}", cat.act());
    assert!(ROAM_MS.contains(&cat.left_ms()));
    let roamed = run_to_next_act(&mut cat);
    assert!(roamed >= *ROAM_MS.start(), "{roamed}");
    assert_eq!(cat.act(), Act::Laptop, "back to rule 1");
}

#[test]
fn after_watching_it_sometimes_walks_and_sometimes_naps() {
    let mut picks = std::collections::HashSet::new();
    for seed in 0..40 {
        let mut cat = Behaviour::new(false, seed);
        cat.set_hovered(true);
        cat.set_hovered(false);
        run_to_next_act(&mut cat);
        picks.insert(cat.act());
        // Rule 2 after a nap or a walk.
        run_to_next_act(&mut cat);
        assert_eq!(cat.act(), Act::Sleep);
    }
    assert_eq!(picks, [Act::Walk, Act::Nap].into_iter().collect());
}

#[test]
fn a_walk_moves_one_pixel_a_step_and_turns_at_the_edges() {
    let mut cat = walking_cat();
    assert_eq!(cat.x(), -1, "it starts at the left edge");
    assert_eq!(
        cat.facing(),
        Facing::Right,
        "from the left edge it walks right"
    );
    let mut turns = 0;
    let mut last = (cat.x(), cat.facing());
    loop {
        cat.advance(sprites());
        if cat.act() != Act::Walk {
            break;
        }
        assert!((-1..=20).contains(&cat.x()));
        assert_eq!((cat.x() - last.0).abs(), 1);
        if cat.facing() != last.1 {
            turns += 1;
            assert!(cat.x() == 19 || cat.x() == 0, "it turns at an edge");
        }
        let walk = match cat.facing() {
            Facing::Left => AnimId::WalkLeft,
            Facing::Right => AnimId::WalkRight,
        };
        assert_eq!(cat.anim(), walk);
        last = (cat.x(), cat.facing());
    }
    assert!(turns >= 1, "a 6 s walk crosses a 22 px strip");
}

#[test]
fn a_click_jumps_to_the_happy_eyes() {
    let mut cat = Behaviour::new(true, 1);
    cat.poke(sprites());
    assert_eq!(cat.act(), Act::Watch);
    assert_eq!(cat.step(), sprites().watch_happy);
    assert!(cat.step() > 0);
}

// The view, in a workspace ----------------------------------------------

/// Demo data with every claim and Analyzing mark taken off.
fn idle_demo() -> (Catalog, Vec<Issue>) {
    let catalog = crate::model::demo::catalog();
    let mut issues = crate::model::demo::issues(&catalog);
    for issue in &mut issues {
        issue.claim = None;
        issue.analyzing = None;
    }
    (catalog, issues)
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .unwrap()
    });
    let (catalog, issues) = idle_demo();
    view.update(cx, |this, cx| this.load(catalog, issues, cx));
    cx.update_window(handle, |_, window, cx| {
        window.press("cmd-b", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    (handle, view)
}

fn mascot(view: &Entity<Workspace>, cx: &TestAppContext) -> Entity<Mascot> {
    cx.read(|cx| view.read(cx).mascot().clone())
}

fn act(view: &Entity<Workspace>, cx: &TestAppContext) -> Act {
    let cat = mascot(view, cx);
    cx.read(|cx| cat.read(cx).behaviour().act())
}

fn step(view: &Entity<Workspace>, cx: &TestAppContext) -> usize {
    let cat = mascot(view, cx);
    cx.read(|cx| cat.read(cx).behaviour().step())
}

fn ticking(view: &Entity<Workspace>, cx: &TestAppContext) -> bool {
    let cat = mascot(view, cx);
    cx.read(|cx| cat.read(cx).ticking())
}

fn wait(cx: &mut TestAppContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    cx.run_until_parked();
}

/// The first issue in the list, changed by `change`, as a live update.
fn change_first(
    view: &Entity<Workspace>,
    cx: &mut TestAppContext,
    change: impl FnOnce(&mut Issue),
) {
    view.update(cx, |this, cx| {
        let mut issue = this.visible_issue(0, cx).clone();
        change(&mut issue);
        this.receive_issue(issue, cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_cat_follows_claims_as_they_start_and_end(cx: &mut TestAppContext) {
    let (_, view) = open(cx);
    assert_eq!(act(&view, cx), Act::Sleep);

    let claim = crate::model::demo::issues(&crate::model::demo::catalog())
        .into_iter()
        .find_map(|i| i.claim)
        .expect("the demo data has a claim");
    change_first(&view, cx, |issue| issue.claim = Some(claim));
    assert_eq!(act(&view, cx), Act::Laptop);

    change_first(&view, cx, |issue| issue.claim = None);
    assert_eq!(act(&view, cx), Act::Sleep);
}

#[gpui_kit::test]
fn a_live_analyzing_mark_counts_and_the_trash_does_not(cx: &mut TestAppContext) {
    let (_, view) = open(cx);
    let now = jiff::Timestamp::now();
    let mark = |expires_at| Analyzing {
        actor: crate::model::data::Assignee::Person("Agent".into()),
        session: "s".into(),
        started_at: now,
        expires_at,
    };
    change_first(&view, cx, |issue| {
        issue.analyzing = Some(mark(now + jiff::SignedDuration::from_mins(15)));
    });
    assert_eq!(act(&view, cx), Act::Laptop);

    change_first(&view, cx, |issue| issue.deleted = true);
    assert_eq!(
        act(&view, cx),
        Act::Sleep,
        "a task in the Trash does not count"
    );

    // The trashed issue left the list, so the first row is another one.
    change_first(&view, cx, |issue| {
        issue.analyzing = Some(mark(now - jiff::SignedDuration::from_mins(1)));
    });
    assert_eq!(act(&view, cx), Act::Sleep, "an expired mark does not count");
}

#[gpui_kit::test]
fn frames_advance_on_the_clock_only_while_the_sidebar_shows(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    assert!(ticking(&view, cx));
    let first = step(&view, cx);
    // The sleep animation shows each frame for 160 ms.
    wait(cx, 170);
    assert_eq!(step(&view, cx), first + 1);

    cx.update_window(handle, |_, window, cx| window.press("cmd-b", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(!ticking(&view, cx));
    let hidden = step(&view, cx);
    wait(cx, 2_000);
    assert_eq!(step(&view, cx), hidden, "no frames while hidden");

    cx.update_window(handle, |_, window, cx| window.press("cmd-b", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(ticking(&view, cx));
}

#[gpui_kit::test]
fn hover_watch_then_walk_or_nap_then_back(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| window.hover(CAT_ID, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(act(&view, cx), Act::Watch);
    wait(cx, 30_000);
    assert_eq!(
        act(&view, cx),
        Act::Watch,
        "it sits while the pointer stays"
    );

    // The pointer leaves for the list.
    cx.update_window(handle, |_, window, cx| {
        window.pointer_move(point(px(700.), px(300.)), None, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(act(&view, cx), Act::Watch);
    wait(cx, *WATCH_MS.end() + 200);
    let roam = act(&view, cx);
    assert!(matches!(roam, Act::Walk | Act::Nap), "{roam:?}");
    wait(cx, *ROAM_MS.end() + 200);
    assert_eq!(act(&view, cx), Act::Sleep);
}

#[gpui_kit::test]
fn a_click_shows_the_happy_eyes_at_once(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| window.click(CAT_ID, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(act(&view, cx), Act::Watch);
    assert_eq!(step(&view, cx), sprites().watch_happy);
}

#[gpui_kit::test]
fn the_cat_starts_at_the_left_edge_of_the_sidebar(cx: &mut TestAppContext) {
    let (_, view) = open(cx);
    let cat = mascot(&view, cx);
    let x = cx.read(|cx| cat.read(cx).behaviour().x());
    assert_eq!(x, -i32::from(sprites().cols.0));
}
