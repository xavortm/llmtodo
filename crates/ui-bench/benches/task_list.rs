//! GPU frame times for the task list at 1,000, 10,000 and 50,000 issues.
//! GPUI draws each frame through headless Metal: it shapes and rasterizes
//! the text, builds the scene and submits it to the GPU.
//!
//! ```sh
//! cargo bench -p llmtodo-ui-bench --bench task_list
//! ```
//!
//! Each benchmark prints a GPUI bench report with draw and present times.
//! The window is 1400 by 900 points, the size the shell perf tests use.
//!
//! The benchmark app has no asset source, so the icons draw nothing. The
//! numbers leave out the cost of the icons.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::{
    AnyWindowHandle, AppContext as _, BenchAppContext, Context, Entity, IntoElement,
    ParentElement as _, Render, Styled as _, Window, div, point, px, size,
};
use llmtodo_ui::settings::{SettingsStore, UiSettings};
use llmtodo_ui::shell::workspace::Workspace;
use llmtodo_ui_bench::{SIZES, workload};

fn sizes() -> Vec<usize> {
    SIZES.to_vec()
}

/// Opens a window with a workspace that holds `count` issues, and waits
/// until the list rows are built. The rows of a large list build on a
/// background task, so this runs outside the timed part.
fn open(count: usize, cx: &mut BenchAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    let mut window = cx.add_empty_window();
    window.simulate_resize(size(px(1400.), px(900.)));
    let (catalog, issues) = workload(count);
    let workspace = window.update(|window, cx| {
        window.replace_root(cx, |window, cx| {
            let mut workspace = Workspace::with_settings(
                UiSettings::default(),
                SettingsStore::memory(),
                window,
                cx,
            );
            workspace.load(catalog, issues, cx);
            workspace
        })
    });
    window.run_until_idle();
    let rows = window.update(|_, cx| workspace.read(cx).list().read(cx).rows().len());
    assert!(
        rows >= count,
        "the list shows {rows} rows for {count} issues"
    );
    (window.window_handle(), workspace)
}

/// The window root for the first draw. It shows nothing until the
/// benchmark hands it a workspace whose rows are already built.
struct Host {
    workspace: Option<Entity<Workspace>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().children(self.workspace.clone())
    }
}

/// The first frame of the list in a new window, with every row built. Each
/// iteration opens a fresh window, so the glyph atlas starts cold.
#[gpui::bench(inputs = sizes(), input_name = "issues", group = "first draw", sample_size = 10)]
fn first_draw(count: &usize, cx: &mut BenchAppContext) {
    cx.update(llmtodo_ui::init);
    let count = *count;
    let (catalog, issues) = workload(count);
    cx.bench_renderer_session(
        std::time::Duration::from_secs(60),
        |cx| {
            // Close the window of the last iteration, so they do not pile up.
            cx.update(|cx| {
                for window in cx.windows() {
                    window
                        .update(cx, |_, window, _| window.remove_window())
                        .ok();
                }
            });
            let mut window = cx.add_empty_window();
            window.simulate_resize(size(px(1400.), px(900.)));
            let (catalog, issues) = (catalog.clone(), issues.clone());
            let (host, workspace) = window.update(|window, cx| {
                let workspace = cx.new(|cx| {
                    let mut workspace = Workspace::with_settings(
                        UiSettings::default(),
                        SettingsStore::memory(),
                        window,
                        cx,
                    );
                    workspace.load(catalog, issues, cx);
                    workspace
                });
                let host = window.replace_root(cx, |_, _| Host { workspace: None });
                (host, workspace)
            });
            window.run_until_idle();
            let stop = Arc::new(AtomicBool::new(false));
            (
                (host, Some(workspace), stop.clone()),
                window.window_handle(),
                stop,
            )
        },
        |(host, workspace, stop), frame, _, cx| {
            if frame == 0 {
                let workspace = workspace.take();
                host.update(cx, |host, cx| {
                    host.workspace = workspace;
                    cx.notify();
                });
                stop.store(true, Ordering::Release);
            }
        },
    );
}

/// One filter keystroke and the frame after it. The query switches between
/// "number 1" and "number", so each keystroke changes the rows. A large
/// list builds its rows on a background task, and the result lands in a
/// later frame, as in the app.
#[gpui::bench(inputs = sizes(), input_name = "issues", group = "filter keystroke", sample_size = 10)]
fn filter_keystroke(count: &usize, cx: &mut BenchAppContext) {
    cx.update(llmtodo_ui::init);
    let (_, workspace) = open(*count, cx);
    let mut round = 0usize;
    cx.bench_renderer(workspace, move |workspace, _, cx| {
        round += 1;
        let query = if round.is_multiple_of(2) {
            "number 1"
        } else {
            "number"
        };
        workspace.apply_filter(query, cx);
    });
}

/// A steady scroll of 40 points a frame, back to the top at the end. Every
/// frame moves the visible rows.
#[gpui::bench(inputs = sizes(), input_name = "issues", group = "scroll", sample_size = 10)]
fn scroll(count: &usize, cx: &mut BenchAppContext) {
    cx.update(llmtodo_ui::init);
    let (_, workspace) = open(*count, cx);
    let mut offset = px(0.);
    cx.bench_renderer(workspace, move |workspace, _, cx| {
        workspace.list().update(cx, |list, cx| {
            let handle = list.scroll_handle().0.borrow().base_handle.clone();
            offset += px(40.);
            if offset > handle.max_offset().y {
                offset = px(0.);
            }
            handle.set_offset(point(px(0.), -offset));
            cx.notify();
        });
    });
}

gpui::bench_group!(benches, first_draw, filter_keystroke, scroll);
gpui::bench_main!(benches);
