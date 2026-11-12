//! Tests for the cat: the art file, the rules in `behaviour.rs`, and the
//! view in a real workspace on the test clock.

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::test::{ClickOptions, TestEventExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions, point, px,
};

use super::behaviour::{Act, Behaviour, ENTER_DELAY_MS, Facing, ROAM_MS, WATCH_MS};
use super::sprites::{AnimId, sprites};
use super::{CAT_ID, Mascot, quads};
use crate::model::data::{Analyzing, Catalog, Issue};
use crate::settings::{SettingsStore, UiSettings};
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

#[test]
fn at_full_size_every_sprite_pixel_is_whole_device_pixels() {
    let runs = sprites().frame(AnimId::Laptop, 0);
    // 16 px a rem, 2 device pixels per logical pixel: 4 device pixels.
    for [x0, y0, x1, y1] in quads(runs, 16., 2., (10., 100.), 1.) {
        for v in [x0, y0, x1, y1] {
            assert_eq!((v * 2.).fract(), 0.);
        }
        assert_eq!(y1 - y0, 2.);
    }
}

#[test]
fn pressed_the_cat_shrinks_around_its_feet_with_no_gaps() {
    let runs = sprites().frame(AnimId::Laptop, 0);
    let full = quads(runs, 16., 2., (10., 100.), 1.);
    let small = quads(runs, 16., 2., (10., 100.), 0.98);
    let bottom = |q: &[[f32; 4]]| q.iter().map(|r| r[3]).fold(f32::MIN, f32::max);
    let span = |q: &[[f32; 4]]| {
        let l = q.iter().map(|r| r[0]).fold(f32::MAX, f32::min);
        let r = q.iter().map(|r| r[2]).fold(f32::MIN, f32::max);
        (l, r)
    };
    assert_eq!(bottom(&small), bottom(&full), "the feet stay on the ground");
    let (fl, fr) = span(&full);
    let (sl, sr) = span(&small);
    assert!(sl > fl && sr < fr, "it shrinks from both sides");
    assert!(((sl - fl) - (fr - sr)).abs() <= 0.5, "around its centre");
    for (ix, run) in runs.iter().enumerate() {
        let q = small[ix];
        // Every edge sits on a device pixel, so nothing blurs.
        for v in q {
            assert_eq!((v * 2.).fract(), 0.);
        }
        // Runs that touch in a row still touch.
        if let Some(next) = runs.get(ix + 1)
            && next.y == run.y
            && next.x == run.x + run.len
        {
            assert_eq!(small[ix + 1][0], q[2]);
        }
    }
}

// The rules ------------------------------------------------------------------

/// The strip in these tests: the cat's spots are -1 and 20, and -31 is out
/// of view.
const LOW: i32 = -1;
const HIGH: i32 = 20;
const AWAY: i32 = -31;

/// A cat on the test strip, at the left edge.
fn cat(active: bool, seed: u64) -> Behaviour {
    let mut cat = Behaviour::new(active, seed);
    cat.set_range(LOW, HIGH, AWAY);
    cat
}

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

/// A cat that watched and then started to walk.
fn walking_cat() -> Behaviour {
    for seed in 0.. {
        let mut cat = cat(false, seed);
        cat.set_hovered(true);
        cat.set_hovered(false);
        run_to_next_act(&mut cat);
        if cat.act() == Act::Walk {
            return cat;
        }
    }
    unreachable!()
}

/// A working cat at the right edge, after a double click.
fn working_on_the_right(seed: u64) -> Behaviour {
    let mut cat = cat(true, seed);
    assert!(cat.click(2, sprites()));
    run_to_next_act(&mut cat);
    assert_eq!((cat.act(), cat.x()), (Act::Laptop, HIGH));
    cat
}

#[test]
fn an_agent_at_work_means_the_laptop_and_none_means_sleep() {
    let mut cat = cat(true, 7);
    assert_eq!(cat.act(), Act::Laptop);
    assert_eq!(cat.anim(), AnimId::Laptop);
    run_for(&mut cat, 60_000);
    assert_eq!(cat.act(), Act::Laptop, "the laptop has no end");

    // At the left edge, the end of the work means sleep at once.
    assert!(cat.set_active(false));
    assert_eq!(cat.act(), Act::Sleep);
    assert_eq!(cat.step(), 0);
    run_for(&mut cat, 60_000);
    assert_eq!(cat.act(), Act::Sleep);
    // A new session on the left: it works where it is.
    assert!(cat.set_active(true));
    assert_eq!((cat.act(), cat.x()), (Act::Laptop, LOW));
}

#[test]
fn at_work_a_hover_or_a_single_click_changes_nothing() {
    let mut cat = cat(true, 7);
    assert!(!cat.set_hovered(true));
    assert_eq!(cat.act(), Act::Laptop);
    run_for(&mut cat, 10_000);
    assert_eq!(cat.act(), Act::Laptop);
    assert!(!cat.set_hovered(false));
    assert!(!cat.click(1, sprites()));
    assert!(!cat.click(3, sprites()), "only the second click counts");
    assert_eq!((cat.act(), cat.x()), (Act::Laptop, LOW));
}

#[test]
fn a_double_click_at_work_walks_to_the_other_side_and_works_there() {
    let mut cat = cat(true, 7);
    assert!(cat.click(2, sprites()));
    assert_eq!(cat.act(), Act::Commute);
    assert_eq!(cat.anim(), AnimId::WalkRight);
    let mut last = cat.x();
    while cat.act() == Act::Commute {
        cat.advance(sprites());
        assert_eq!(cat.x(), last + 1, "one pixel a step");
        last = cat.x();
    }
    assert_eq!((cat.act(), cat.x()), (Act::Laptop, HIGH));
    // Hover still changes nothing on the right.
    assert!(!cat.set_hovered(true));

    // A second double click walks it back.
    assert!(cat.click(2, sprites()));
    assert_eq!(cat.anim(), AnimId::WalkLeft);
    run_to_next_act(&mut cat);
    assert_eq!((cat.act(), cat.x()), (Act::Laptop, LOW));
}

#[test]
fn click_spam_during_a_walk_neither_stops_nor_turns_it() {
    let mut cat = cat(true, 7);
    assert!(cat.click(2, sprites()));
    let mut clicks = 0;
    while cat.act() == Act::Commute {
        // A burst of clicks every step: singles, doubles and longer runs.
        for count in 1..=5 {
            assert!(!cat.click(count, sprites()));
            clicks += 1;
        }
        assert_eq!(cat.facing(), Facing::Right);
        cat.advance(sprites());
    }
    assert!(clicks > 50);
    assert_eq!(
        (cat.act(), cat.x()),
        (Act::Laptop, HIGH),
        "it reaches the far side"
    );
    // Once it works at its spot, a double click starts the next walk.
    assert!(cat.click(2, sprites()));
    assert_eq!((cat.act(), cat.facing()), (Act::Commute, Facing::Left));
}

#[test]
fn clicks_during_the_walk_home_and_a_roaming_walk_are_ignored() {
    let mut cat = working_on_the_right(7);
    cat.set_active(false);
    for count in 1..=3 {
        assert!(!cat.click(count, sprites()));
    }
    run_to_next_act(&mut cat);
    assert_eq!(cat.x(), LOW);

    let mut cat = walking_cat();
    for count in 1..=3 {
        assert!(!cat.click(count, sprites()));
    }
    assert_eq!(cat.act(), Act::Walk);
}

#[test]
fn when_work_ends_on_the_right_it_walks_home_then_sleeps_or_sits() {
    let mut endings = HashSet::new();
    for seed in 0..40 {
        let mut cat = working_on_the_right(seed);
        assert!(cat.set_active(false));
        assert_eq!((cat.act(), cat.facing()), (Act::Commute, Facing::Left));
        run_to_next_act(&mut cat);
        assert_eq!(cat.x(), LOW, "it walks all the way left");
        endings.insert(cat.act());
        match cat.act() {
            Act::Sleep => {}
            Act::Watch => {
                // It sits for a while, then falls asleep: no walk, no nap.
                let sat = run_to_next_act(&mut cat);
                assert!(WATCH_MS.contains(&sat), "{sat}");
                assert_eq!(cat.act(), Act::Sleep);
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(endings, [Act::Sleep, Act::Watch].into_iter().collect());
}

#[test]
fn work_that_ends_on_the_way_right_sends_the_cat_home() {
    let mut cat = cat(true, 7);
    cat.click(2, sprites());
    run_for(&mut cat, 900);
    assert!(cat.set_active(false));
    assert_eq!(cat.facing(), Facing::Left);
    run_to_next_act(&mut cat);
    assert_eq!(cat.x(), LOW);
    assert!(matches!(cat.act(), Act::Sleep | Act::Watch));
}

#[test]
fn a_session_that_starts_on_the_way_home_ends_at_work_on_the_left() {
    let mut cat = working_on_the_right(7);
    cat.set_active(false);
    run_for(&mut cat, 900);
    // The cat walks on home and works there.
    assert!(!cat.set_active(true));
    run_to_next_act(&mut cat);
    assert_eq!((cat.act(), cat.x()), (Act::Laptop, LOW));
}

#[test]
fn idle_a_hover_makes_it_watch_for_as_long_as_the_pointer_stays() {
    let mut cat = cat(false, 7);
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
    assert_eq!(cat.act(), Act::Laptop, "back to work");
}

#[test]
fn after_watching_it_sometimes_walks_and_sometimes_naps() {
    let mut picks = HashSet::new();
    for seed in 0..40 {
        let mut cat = cat(false, seed);
        cat.set_hovered(true);
        cat.set_hovered(false);
        run_to_next_act(&mut cat);
        picks.insert(cat.act());
        run_to_next_act(&mut cat);
        assert_eq!(cat.act(), Act::Sleep);
    }
    assert_eq!(picks, [Act::Walk, Act::Nap].into_iter().collect());
}

#[test]
fn a_walk_moves_one_pixel_a_step_and_turns_at_the_edges() {
    let mut cat = walking_cat();
    assert_eq!(cat.x(), LOW, "it starts at the left edge");
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
        assert!((LOW..=HIGH).contains(&cat.x()));
        assert_eq!((cat.x() - last.0).abs(), 1);
        if cat.facing() != last.1 {
            turns += 1;
            assert!(
                cat.x() == HIGH - 1 || cat.x() == LOW + 1,
                "it turns at an edge"
            );
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
fn idle_a_click_jumps_to_the_happy_eyes() {
    let mut cat = cat(false, 1);
    assert!(cat.click(1, sprites()));
    assert_eq!(cat.act(), Act::Watch);
    assert_eq!(cat.step(), sprites().watch_happy);
    assert!(cat.step() > 0);
}

#[test]
fn an_open_waits_out_of_view_then_walks_in_to_its_spot() {
    for active in [false, true] {
        let mut cat = cat(active, 5);
        cat.enter();
        assert_eq!(cat.act(), Act::Away);
        assert!(!cat.visible());
        assert_eq!(cat.x(), AWAY);
        // Hover and clicks do nothing while it waits.
        assert!(!cat.set_hovered(true));
        assert!(!cat.click(1, sprites()));
        assert!(!cat.click(2, sprites()));
        let waited = run_to_next_act(&mut cat);
        assert!(ENTER_DELAY_MS.contains(&waited), "{waited}");

        assert_eq!(cat.act(), Act::Enter);
        assert!(cat.visible());
        assert_eq!(cat.anim(), AnimId::WalkRight);
        assert!(!cat.click(1, sprites()), "clicks do nothing on the way in");
        assert!(!cat.set_hovered(false));
        let mut last = cat.x();
        while cat.act() == Act::Enter {
            cat.advance(sprites());
            assert_eq!(cat.x(), last + 1);
            last = cat.x();
        }
        assert_eq!(cat.x(), LOW);
        let rest = if active { Act::Laptop } else { Act::Sleep };
        assert_eq!(cat.act(), rest);
    }
}

#[test]
fn each_open_picks_a_new_delay() {
    let mut cat = cat(false, 9);
    let delays: HashSet<u64> = (0..10)
        .map(|_| {
            cat.enter();
            cat.left_ms()
        })
        .collect();
    assert!(delays.len() > 5, "{delays:?}");
    assert!(delays.iter().all(|d| ENTER_DELAY_MS.contains(d)));
}

#[test]
fn a_cat_that_works_on_the_right_walks_in_all_the_way_there() {
    let mut cat = working_on_the_right(3);
    cat.enter();
    run_to_next_act(&mut cat);
    run_to_next_act(&mut cat);
    assert_eq!((cat.act(), cat.x()), (Act::Laptop, HIGH));
}

#[test]
fn a_new_open_during_the_walk_in_starts_it_over() {
    let mut cat = cat(false, 4);
    cat.enter();
    run_to_next_act(&mut cat);
    run_for(&mut cat, 900);
    assert_eq!(cat.act(), Act::Enter);
    assert!(cat.x() > AWAY);
    // The sidebar closed and opened again.
    cat.enter();
    assert_eq!((cat.act(), cat.x()), (Act::Away, AWAY));
}

#[test]
fn an_agent_that_starts_during_the_walk_in_means_work_on_arrival() {
    let mut cat = cat(false, 4);
    cat.enter();
    assert!(!cat.set_active(true));
    run_to_next_act(&mut cat);
    run_to_next_act(&mut cat);
    assert_eq!(cat.act(), Act::Laptop);
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

fn window_with(
    settings: UiSettings,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| Workspace::with_settings(settings, SettingsStore::memory(), window, cx))
        })
        .unwrap()
    })
}

/// A workspace on idle demo data with the sidebar just opened. The cat
/// waits out of view.
fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    let (handle, view) = window_with(UiSettings::default(), cx);
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

/// Like `open`, then waits until the cat walked in.
fn open_settled(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    let (handle, view) = open(cx);
    settle(handle, &view, cx);
    (handle, view)
}

/// Waits out the delay and the walk-in, drawing frames so the strip has a
/// width.
fn settle(handle: AnyWindowHandle, view: &Entity<Workspace>, cx: &mut TestAppContext) {
    for _ in 0..200 {
        if !matches!(act(view, cx), Act::Away | Act::Enter) {
            return;
        }
        wait(cx, 90);
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    panic!("the walk-in never ended");
}

fn mascot(view: &Entity<Workspace>, cx: &TestAppContext) -> Entity<Mascot> {
    cx.read(|cx| view.read(cx).mascot().clone())
}

fn behaviour(view: &Entity<Workspace>, cx: &TestAppContext) -> Behaviour {
    let cat = mascot(view, cx);
    cx.read(|cx| cat.read(cx).behaviour().clone())
}

fn act(view: &Entity<Workspace>, cx: &TestAppContext) -> Act {
    behaviour(view, cx).act()
}

fn step(view: &Entity<Workspace>, cx: &TestAppContext) -> usize {
    behaviour(view, cx).step()
}

fn ticking(view: &Entity<Workspace>, cx: &TestAppContext) -> bool {
    let cat = mascot(view, cx);
    cx.read(|cx| cat.read(cx).ticking())
}

fn wait(cx: &mut TestAppContext, ms: u64) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    cx.run_until_parked();
}

fn press(handle: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| window.press(key, cx))
        .unwrap();
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

fn demo_claim() -> crate::model::data::Claim {
    crate::model::demo::issues(&crate::model::demo::catalog())
        .into_iter()
        .find_map(|i| i.claim)
        .expect("the demo data has a claim")
}

#[gpui_kit::test]
fn the_cat_follows_claims_as_they_start_and_end(cx: &mut TestAppContext) {
    let (_, view) = open_settled(cx);
    assert_eq!(act(&view, cx), Act::Sleep);

    change_first(&view, cx, |issue| issue.claim = Some(demo_claim()));
    assert_eq!(act(&view, cx), Act::Laptop);

    change_first(&view, cx, |issue| issue.claim = None);
    assert_eq!(act(&view, cx), Act::Sleep);
}

#[gpui_kit::test]
fn a_live_analyzing_mark_counts_and_the_trash_does_not(cx: &mut TestAppContext) {
    let (_, view) = open_settled(cx);
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
    let (handle, view) = open_settled(cx);
    assert!(ticking(&view, cx));
    let first = step(&view, cx);
    // The sleep animation shows each frame for 160 ms.
    wait(cx, 170);
    assert_eq!(step(&view, cx), first + 1);

    press(handle, "cmd-b", cx);
    assert!(!ticking(&view, cx));
    let hidden = step(&view, cx);
    wait(cx, 2_000);
    assert_eq!(step(&view, cx), hidden, "no frames while hidden");

    press(handle, "cmd-b", cx);
    assert!(ticking(&view, cx));
}

#[gpui_kit::test]
fn the_sidebar_opening_starts_a_walk_in_and_hiding_it_cancels(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let cat = behaviour(&view, cx);
    assert_eq!(cat.act(), Act::Away);
    assert!(!cat.visible());
    // The cat draws nothing while it waits, so there is nothing to hover.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_all(CAT_ID).is_empty());
    })
    .unwrap();

    wait(cx, *ENTER_DELAY_MS.end() + 10);
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    let cat = behaviour(&view, cx);
    assert_eq!(cat.act(), Act::Enter);
    assert!(cat.x() < cat.edges().0, "it is still partly out of view");

    // Hidden in the middle of the walk-in: nothing moves.
    press(handle, "cmd-b", cx);
    let frozen = behaviour(&view, cx);
    wait(cx, 5_000);
    let still = behaviour(&view, cx);
    assert_eq!((still.act(), still.x()), (frozen.act(), frozen.x()));
    assert!(!ticking(&view, cx));

    // The next open starts again, out of view.
    press(handle, "cmd-b", cx);
    let cat = behaviour(&view, cx);
    assert_eq!(cat.act(), Act::Away);
    settle(handle, &view, cx);
    let cat = behaviour(&view, cx);
    assert_eq!((cat.act(), cat.x()), (Act::Sleep, cat.edges().0));
}

#[gpui_kit::test]
fn a_sidebar_open_at_app_start_runs_the_walk_in(cx: &mut TestAppContext) {
    let settings = UiSettings {
        sidebar_open: true,
        ..UiSettings::default()
    };
    let (_, view) = window_with(settings, cx);
    assert_eq!(act(&view, cx), Act::Away);
    assert!(ticking(&view, cx));
}

#[gpui_kit::test]
fn hover_and_clicks_do_nothing_during_the_walk_in(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    wait(cx, *ENTER_DELAY_MS.end() + 10);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.hover(CAT_ID, cx);
        window.click(CAT_ID, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(act(&view, cx), Act::Enter);
}

#[gpui_kit::test]
fn idle_hover_watch_then_walk_or_nap_then_back(cx: &mut TestAppContext) {
    let (handle, view) = open_settled(cx);
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
fn idle_a_click_shows_the_happy_eyes_at_once(cx: &mut TestAppContext) {
    let (handle, view) = open_settled(cx);
    cx.update_window(handle, |_, window, cx| window.click(CAT_ID, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(act(&view, cx), Act::Watch);
    assert_eq!(step(&view, cx), sprites().watch_happy);
}

#[gpui_kit::test]
fn at_work_hover_does_nothing_and_a_double_click_walks_right(cx: &mut TestAppContext) {
    let (handle, view) = open_settled(cx);
    change_first(&view, cx, |issue| issue.claim = Some(demo_claim()));
    assert_eq!(act(&view, cx), Act::Laptop);

    cx.update_window(handle, |_, window, cx| {
        window.hover(CAT_ID, cx);
        window.click(CAT_ID, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(act(&view, cx), Act::Laptop);

    cx.update_window(handle, |_, window, cx| {
        window.click_with_options(CAT_ID, ClickOptions::new().with_count(2), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(act(&view, cx), Act::Commute);
    wait(cx, 20_000);
    let cat = behaviour(&view, cx);
    assert_eq!((cat.act(), cat.x()), (Act::Laptop, cat.edges().1));

    // The work ends: it walks home.
    change_first(&view, cx, |issue| issue.claim = None);
    assert_eq!(act(&view, cx), Act::Commute);
    wait(cx, 20_000);
    let cat = behaviour(&view, cx);
    assert_eq!(cat.x(), cat.edges().0);
}

#[gpui_kit::test]
fn click_spam_during_a_walk_still_reaches_the_far_side(cx: &mut TestAppContext) {
    let (handle, view) = open_settled(cx);
    change_first(&view, cx, |issue| issue.claim = Some(demo_claim()));
    let double = || ClickOptions::new().with_count(2);
    cx.update_window(handle, |_, window, cx| {
        window.click_with_options(CAT_ID, double(), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(act(&view, cx), Act::Commute);
    // Double clicks on the moving cat, every frame of the walk.
    for _ in 0..400 {
        if act(&view, cx) != Act::Commute {
            break;
        }
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click_with_options(CAT_ID, double(), cx);
        })
        .unwrap();
        assert_eq!(behaviour(&view, cx).facing(), Facing::Right);
        wait(cx, 90);
    }
    let cat = behaviour(&view, cx);
    assert_eq!((cat.act(), cat.x()), (Act::Laptop, cat.edges().1));
}

#[gpui_kit::test]
fn the_cat_shows_pressed_while_the_button_is_down(cx: &mut TestAppContext) {
    let (handle, view) = open_settled(cx);
    let cat = mascot(&view, cx);
    let centre = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.find(CAT_ID).bounds().center()
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        window.pointer_down(centre, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    assert!(cx.read(|cx| cat.read(cx).pressed()));
    cx.update_window(handle, |_, window, cx| {
        window.pointer_up(centre, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    assert!(!cx.read(|cx| cat.read(cx).pressed()));

    // A release away from the cat ends the press too.
    cx.update_window(handle, |_, window, cx| {
        window.pointer_down(centre, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(point(px(700.), px(300.)), gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    assert!(!cx.read(|cx| cat.read(cx).pressed()));
}

#[gpui_kit::test]
fn the_cat_ends_its_walk_in_at_the_left_edge(cx: &mut TestAppContext) {
    let (_, view) = open_settled(cx);
    let cat = behaviour(&view, cx);
    assert_eq!(cat.x(), cat.edges().0);
    assert_eq!(cat.edges().0, 2 - i32::from(sprites().cols.0));
}
