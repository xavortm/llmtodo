//! Times the app's main thread on large issue lists. It is ignored in normal
//! runs, and it opens only headless test windows.
//!
//! ```sh
//! cargo test -p llmtodo-ui perf_tests -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Set `LLMTODO_PERF_MAIN_THREAD=1` to build the rows on the main thread at
//! every size, as before LLM-112.
//!
//! The numbers cover state updates, layout and scene building. The test
//! platform has no GPU, so they leave out text rasterizing and presenting.

use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, TestAppContext, Window, WindowBounds,
    WindowOptions, px, size,
};

use crate::issues::list::SortKey;
use crate::model::data::{Assignee, Issue};
use crate::model::store::IssueStore;
use crate::shell::workspace::{MoveDown, MoveUp, Workspace};

const ROUNDS: usize = 15;

/// A description with the shape agents write: prose, then a `+++` section.
fn description(n: usize) -> String {
    let prose = "The list drops frames while an agent writes to many tasks. ".repeat(20);
    format!("## Goal {n}\n\n{prose}\n\n+++ Agent notes\n\n{prose}\n\n+++\n")
}

/// `count` issues built from the demo set. One in twenty is claimed.
fn workload(count: usize) -> (crate::model::data::Catalog, Vec<Issue>) {
    let catalog = crate::model::demo::catalog();
    let demo = crate::model::demo::issues(&catalog);
    let issues = (0..count)
        .map(|n| {
            let mut issue = demo[n % demo.len()].clone();
            issue.id = llmtodo_core::IssueId::from_u128(n as u128 + 1);
            issue.key = format!("LLM-{}", n + 1).into();
            issue.title = format!("{} number {n}", issue.title).into();
            issue.description = description(n).into();
            if !n.is_multiple_of(20) {
                issue.claim = None;
            }
            issue
        })
        .collect();
    (catalog, issues)
}

fn open(cx: &mut TestAppContext, count: usize) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                Default::default(),
                size(px(1400.), px(900.)),
            ))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Workspace::new(window, cx))
        })
        .expect("open test window")
    });
    // `LLMTODO_PERF_MAIN_THREAD=1` keeps every build on the main thread,
    // for a comparison with the background path.
    if std::env::var_os("LLMTODO_PERF_MAIN_THREAD").is_some() {
        view.update(cx, |this, cx| {
            this.list()
                .update(cx, |list, _| list.set_background_from(usize::MAX));
        });
    }
    let (catalog, issues) = workload(count);
    view.update(cx, |this, cx| this.load(catalog, issues, cx));
    cx.run_until_parked();
    (handle, view)
}

/// Runs `step` several times, prints the median and worst time.
fn report(
    name: &str,
    count: usize,
    handle: AnyWindowHandle,
    cx: &mut TestAppContext,
    mut step: impl FnMut(usize, &mut Window, &mut App),
) {
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    // One window update per round, so observers of the store run inside the
    // timed part. They run when the outermost update ends.
    let mut times: Vec<Duration> = (0..ROUNDS)
        .map(|round| {
            let start = Instant::now();
            cx.update_window(handle, |_, window, cx| step(round, window, cx))
                .unwrap();
            start.elapsed()
        })
        .collect();
    times.sort();
    println!(
        "{count:>6} issues  {name:<34} median {:>10.2?}  worst {:>10.2?}",
        times[ROUNDS / 2],
        times[ROUNDS - 1]
    );
}

/// One `j` or `k`, then one frame. `press` would draw two extra frames.
fn step_selection(view: &Entity<Workspace>, round: usize, window: &mut Window, cx: &mut App) {
    view.update(cx, |this, cx| {
        if round.is_multiple_of(2) {
            this.move_down(&MoveDown, window, cx);
        } else {
            this.move_up(&MoveUp, window, cx);
        }
    });
    window.render_frame(cx);
}

fn measure(count: usize, cx: &mut TestAppContext) {
    let (handle, view) = open(cx, count);
    report(
        "frame, nothing changed",
        count,
        handle,
        cx,
        |_, window, cx| {
            window.render_frame(cx);
        },
    );
    let v = view.clone();
    report(
        "move selection, task closed",
        count,
        handle,
        cx,
        move |round, window, cx| {
            step_selection(&v, round, window, cx);
        },
    );
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    report("frame, task open", count, handle, cx, |_, window, cx| {
        window.render_frame(cx);
    });
    let v = view.clone();
    report(
        "move selection, task open",
        count,
        handle,
        cx,
        move |round, window, cx| {
            step_selection(&v, round, window, cx);
        },
    );
    let v = view.clone();
    report(
        "live update, new description",
        count,
        handle,
        cx,
        move |round, window, cx| {
            v.update(cx, |this, cx| {
                let mut issue = this.visible_issue(round + 3, cx).clone();
                issue.description = description(round).into();
                this.receive_issue(issue, cx);
            });
            window.render_frame(cx);
        },
    );
    let v = view.clone();
    report(
        "live update, new title",
        count,
        handle,
        cx,
        move |round, window, cx| {
            v.update(cx, |this, cx| {
                let mut issue = this.visible_issue(round + 3, cx).clone();
                issue.title = format!("Changed in round {round}").into();
                this.receive_issue(issue, cx);
            });
            window.render_frame(cx);
        },
    );
    let v = view.clone();
    report(
        "filter keystroke",
        count,
        handle,
        cx,
        move |round, window, cx| {
            let query = if round.is_multiple_of(2) {
                "number 1"
            } else {
                "number"
            };
            v.update(cx, |this, cx| this.apply_filter(query, cx));
            window.render_frame(cx);
        },
    );
    landed("filter keystroke, result landed", count, handle, &view, cx);
    // The cat: one new frame under the open sidebar, with the task open.
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.apply_filter("", cx));
        window.press("cmd-b", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    report("frame, sidebar open", count, handle, cx, |_, window, cx| {
        window.render_frame(cx);
    });
    // `render_frame` refreshes the whole window, so these frames use a
    // plain draw, as the platform does: only dirty views render.
    let mascot = cx.read(|cx| view.read(cx).mascot().clone());
    report("mascot frame", count, handle, cx, move |_, window, cx| {
        mascot.update(cx, |mascot, cx| mascot.tick(cx));
        window.draw(cx).clear(cx);
    });
    cx.update(|cx| cx.quit());
}

/// Like `report` for a filter keystroke, but the time runs until the
/// result of a background build landed and one frame drew it. The test
/// executor runs the background task on this thread, so this is the whole
/// cost, not what the main thread waits.
fn landed(
    name: &str,
    count: usize,
    handle: AnyWindowHandle,
    view: &Entity<Workspace>,
    cx: &mut TestAppContext,
) {
    let mut times: Vec<Duration> = (0..ROUNDS)
        .map(|round| {
            let query = if round.is_multiple_of(2) {
                "number 1"
            } else {
                "number"
            };
            let start = Instant::now();
            view.update(cx, |this, cx| this.apply_filter(query, cx));
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .unwrap();
            start.elapsed()
        })
        .collect();
    times.sort();
    println!(
        "{count:>6} issues  {name:<34} median {:>10.2?}  worst {:>10.2?}",
        times[ROUNDS / 2],
        times[ROUNDS - 1]
    );
}

/// Times one build of the rows on the calling thread, the step that
/// decides `rows::BACKGROUND_FROM`. It runs a filter that keeps about one
/// issue in ten, then a sort by status under "group by status".
#[test]
#[ignore = "builds large lists and prints timings"]
fn row_build_times() {
    use crate::issues::list::GroupBy;
    use crate::issues::rows::{self, RowsInput};
    use crate::model::scope::Scope;
    for count in [500, 1_000, 2_000, 4_000, 8_000, 10_000, 50_000] {
        let (catalog, issues) = workload(count);
        let statuses = catalog.all_statuses();
        let collapsed = Default::default();
        for (name, text, sort, group) in [
            ("keystroke", "number 1", SortKey::Key, GroupBy::None),
            ("status groups", "", SortKey::Status, GroupBy::Status),
        ] {
            let query = crate::issues::filter::Filter::parse(text).resolve(&catalog);
            let input = RowsInput {
                scope: &Scope::All,
                view: None,
                query: &query,
                cutoff: None,
                now: jiff::Timestamp::now(),
                sort: (sort, false),
                group,
                collapsed: &collapsed,
                statuses: &statuses,
                labels: &catalog.labels,
            };
            let mut times: Vec<Duration> = (0..ROUNDS)
                .map(|_| {
                    let start = Instant::now();
                    std::hint::black_box(rows::build(&input, issues.iter()));
                    start.elapsed()
                })
                .collect();
            times.sort();
            println!(
                "{count:>6} issues  build, {name:<27} median {:>10.2?}  worst {:>10.2?}",
                times[ROUNDS / 2],
                times[ROUNDS - 1]
            );
        }
    }
}

/// A store holding one issue with `description`.
fn store_with(description: &str) -> (IssueStore, llmtodo_core::IssueId) {
    let catalog = crate::model::demo::catalog();
    let mut issue = crate::model::demo::issues(&catalog)[0].clone();
    issue.deleted = false;
    issue.description = description.to_string().into();
    issue.open_questions = llmtodo_core::open_questions(description).len() as u32;
    let id = issue.id;
    let mut store = IssueStore::new();
    store.replace_all(vec![issue]);
    (store, id)
}

const ONE_QUESTION: &str = "+++ Owner questions\n\n### Q1. Ship it?\n\nAnswer:\n\n+++\n";

#[test]
fn the_open_question_count_follows_every_kind_of_write() {
    let (mut store, id) = store_with(ONE_QUESTION);
    let open_status = store.get(id).unwrap().status.clone();
    assert!(!open_status.is_closed());
    let real = |store: &IssueStore| -> usize {
        store
            .iter()
            .filter(|i| !i.deleted && !i.status.is_closed())
            .map(|i| llmtodo_core::open_questions(&i.description).len())
            .sum()
    };
    assert_eq!(real(&store), 1, "the fixture must hold an open question");
    assert_eq!(store.open_question_count(), 1);

    store.modify(id, |i| i.description = "No questions now".into());
    assert_eq!(store.open_question_count(), 0);
    store.modify(id, |i| i.description = ONE_QUESTION.into());
    assert_eq!(store.open_question_count(), 1);

    // A deleted or closed issue stops counting, and comes back unparsed.
    store.modify(id, |i| i.deleted = true);
    assert_eq!(store.open_question_count(), 0);
    store.modify(id, |i| i.deleted = false);
    assert_eq!(store.open_question_count(), 1);

    // A server copy brings its own count.
    let mut copy = store.get(id).unwrap().clone();
    copy.description = "Gone".into();
    copy.open_questions = 0;
    store.upsert(copy.clone());
    assert_eq!(store.open_question_count(), real(&store));
    copy.description = ONE_QUESTION.into();
    copy.open_questions = 1;
    store.upsert(copy.clone());
    assert_eq!(store.open_question_count(), 1);

    store.replace_all(vec![copy.clone()]);
    assert_eq!(store.open_question_count(), 1);
    copy.description = "Snapshot without questions".into();
    copy.open_questions = 0;
    store.replace_all(vec![copy.clone(), copy]);
    assert_eq!(store.open_question_count(), 0);
    assert_eq!(store.iter().count(), 1, "a later copy of an ID replaces it");
}

#[gpui_kit::test]
fn the_comfortable_width_follows_the_filter(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, 40);
    cx.update_window(handle, |_, window, cx| {
        let rem = window.rem_size();
        let width = |view: &Entity<Workspace>, cx: &App| {
            view.read(cx).list().read(cx).comfortable_width(rem, cx)
        };
        let all = width(&view, cx);
        assert_eq!(all, width(&view, cx), "a second read gives the same answer");
        // No title matches, so the width falls to its minimum.
        view.update(cx, |this, cx| this.apply_filter("no such title", cx));
        let none = width(&view, cx);
        assert!(none < all, "{none:?} should be below {all:?}");
        view.update(cx, |this, cx| this.apply_filter("", cx));
        assert_eq!(width(&view, cx), all);
    })
    .unwrap();
}

/// LLM-108: a claim moves a row only while the list filters or sorts by
/// the LLM status.
#[gpui_kit::test]
fn a_claim_causes_a_refilter_only_when_the_list_reads_it(cx: &mut TestAppContext) {
    let (_, view) = open(cx, 2);
    let mut unclaimed = cx.read(|cx| view.read(cx).visible_issue(0, cx).clone());
    unclaimed.claim = None;
    let mut claimed = unclaimed.clone();
    claimed.claim = Some(crate::model::demo::claude_claim(&unclaimed.key));
    let changed = |view: &Entity<Workspace>, cx: &mut TestAppContext| {
        cx.read(|cx| {
            view.read(cx)
                .list()
                .read(cx)
                .needs_refilter(&unclaimed, &claimed)
        })
    };
    assert!(!changed(&view, cx));
    view.update(cx, |this, cx| {
        this.list()
            .update(cx, |list, cx| list.sort_by(SortKey::Llm, cx));
    });
    assert!(changed(&view, cx));
    view.update(cx, |this, cx| {
        this.list().update(cx, |list, cx| {
            list.sort_by(SortKey::Key, cx);
            list.set_query("llm:any", cx);
        });
    });
    assert!(changed(&view, cx));
}

#[gpui_kit::test]
fn only_fields_the_list_reads_cause_a_refilter(cx: &mut TestAppContext) {
    let (_, view) = open(cx, 2);
    cx.read(|cx| {
        let this = view.read(cx);
        let list = this.list().read(cx);
        let old = this.visible_issue(0, cx).clone();
        let check = |change: fn(&mut Issue)| {
            let mut new = old.clone();
            change(&mut new);
            list.needs_refilter(&old, &new)
        };
        assert!(!check(|i| i.description = "New text".into()));
        assert!(!check(|i| i.cost = Some(1.5)));
        assert!(!check(|i| i.depth = 3));
        assert!(!check(|i| i.claim = None));
        assert!(check(|i| i.deleted = true));
        assert!(check(|i| i.key = "LLM-999".into()));
        assert!(check(|i| i.title = "New title".into()));
        assert!(check(|i| i.team = "OTHER".into()));
        assert!(check(|i| i.project = Some("other".into())));
        assert!(check(|i| i.labels = Vec::new()));
        assert!(check(|i| i.status.name = "Other".into()));
        assert!(check(
            |i| i.assignee = Some(Assignee::Person("Someone".into()))
        ));
        assert!(check(|i| i.added.year -= 1));
    });
}

#[gpui_kit::test]
fn a_live_title_change_still_leaves_the_filter(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, 20);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.apply_filter("number 7", cx));
        window.render_frame(cx);
        let id = view.read(cx).visible_issue(0, cx).id;
        assert_eq!(view.read(cx).visible_count(cx), 1);

        // A new description changes nothing about the list.
        view.update(cx, |this, cx| {
            let mut issue = this.issue(id, cx).clone();
            issue.description = "Rewritten by an agent".into();
            this.receive_issue(issue, cx);
        });
        assert_eq!(view.read(cx).visible_count(cx), 1);
        assert_eq!(
            view.read(cx).issue(id, cx).description.as_ref(),
            "Rewritten by an agent"
        );

        view.update(cx, |this, cx| {
            let mut issue = this.issue(id, cx).clone();
            issue.title = "Renamed".into();
            this.receive_issue(issue, cx);
        });
        assert_eq!(view.read(cx).visible_count(cx), 0);
    })
    .unwrap();
}

#[gpui_kit::test]
#[ignore = "builds large lists and prints timings"]
fn frame_times_1k(cx: &mut TestAppContext) {
    measure(1_000, cx);
}

#[gpui_kit::test]
#[ignore = "builds large lists and prints timings"]
fn frame_times_10k(cx: &mut TestAppContext) {
    measure(10_000, cx);
}

#[gpui_kit::test]
#[ignore = "builds large lists and prints timings"]
fn frame_times_50k(cx: &mut TestAppContext) {
    measure(50_000, cx);
}

/// LLM-173: one typed key in a description of about 50 KB, with the task
/// in the editor. The time covers the highlighter update and one frame.
#[gpui_kit::test]
#[ignore = "builds a long description and prints timings"]
fn typing_into_a_long_description(cx: &mut TestAppContext) {
    let (handle, view) = open(cx, 200);
    let long = description(0).repeat(50_000 / description(0).len() + 1);
    let id = view.update(cx, |this, cx| {
        let mut issue = this.visible_issue(1, cx).clone();
        issue.claim = None;
        issue.description = long.clone().into();
        let id = issue.id;
        this.receive_issue(issue, cx);
        id
    });
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |this, cx| this.open_task(id, window, cx));
        window.render_frame(cx);
        window.press("e", cx);
        window.press("i", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let length = cx.read(|cx| view.read(cx).editor(cx).read(cx).value().len());
    assert!(length >= 50_000, "{length}");
    report(
        "type one key, 50 KB description",
        200,
        handle,
        cx,
        |_, window, cx| {
            window.input("x", cx);
            window.draw(cx).clear(cx);
        },
    );
    cx.update(|cx| cx.quit());
}
