//! LLM-85: long descriptions wrap to the task view's width. No text in the
//! preview runs past the panel's right edge. Code blocks and wide tables
//! may scroll sideways, but only inside their own box.

use gpui_kit::base::test_support::texts;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ElementId, Entity, Pixels, TestAppContext,
    WindowBounds, WindowOptions, px, size,
};

use crate::shell::workspace::Workspace;

/// The demo issue whose description holds every overflow case.
const WRAP_KEY: &str = "LLM-114";

/// A workspace on demo data, with the task view `detail_width` pixels wide.
fn open(
    width: f32,
    detail_width: f32,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    let settings = crate::settings::UiSettings {
        split_side: Some(detail_width),
        ..Default::default()
    };
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(width), px(900.)),
                ))),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    let store = crate::settings::SettingsStore::memory();
                    let mut this = Workspace::with_settings(settings, store, window, cx);
                    this.load_demo(cx);
                    this
                })
            },
        )
        .expect("open test window")
    });
    view.update(cx, |this, cx| this.open_key(WRAP_KEY, cx));
    // Two frames: the first measures the preview, the second may switch
    // between tabs and columns.
    for _ in 0..2 {
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.run_until_parked();
    }
    (handle, view)
}

/// Whether a path runs through a box that scrolls sideways on its own: a
/// code block or a table.
fn in_scroll_box(path: &[ElementId]) -> bool {
    path.iter().any(|id| {
        let s = format!("{id:?}");
        s.contains("\"scroll\"") || s.contains("table")
    })
}

/// Every painted text in the preview that ends past `right`.
fn overflowing(handle: AnyWindowHandle, cx: &mut TestAppContext) -> (Pixels, Vec<String>) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let preview = window.find("preview").bounds();
        let preview_id = ElementId::from("preview");
        let painted = texts(window);
        let inside: Vec<_> = painted
            .iter()
            .filter(|t| t.path().contains(&preview_id))
            .collect();
        assert!(
            inside.iter().any(|t| t.text().contains("Every case")),
            "the wrap description is not in the preview: {:?}",
            inside.iter().map(|t| t.text()).collect::<Vec<_>>()
        );
        let right = preview.right();
        // In columns, each text stays inside its own column.
        let edge = |path: &[ElementId]| {
            (0..4usize)
                .map(|ix| ElementId::from(("preview-column", ix)))
                .find(|id| path.contains(id))
                .map_or(right, |id| window.find(id).bounds().right())
        };
        let bad = inside
            .iter()
            .filter(|t| !in_scroll_box(t.path()))
            .filter(|t| t.bounds().right() > edge(t.path()) + px(0.5))
            .map(|t| {
                format!(
                    "{:?} ends at {:?}, path {:?}",
                    t.text(),
                    t.bounds().right(),
                    t.path()
                )
            })
            .collect();
        (right, bad)
    })
    .unwrap()
}

/// The task view at its narrowest beside the list: 22 rem.
#[gpui_kit::test]
fn the_preview_wraps_at_a_narrow_width(cx: &mut TestAppContext) {
    let (handle, _) = open(1200., 352., cx);
    let (right, bad) = overflowing(handle, cx);
    assert!(bad.is_empty(), "text past {right:?}:\n{}", bad.join("\n"));
}

/// A narrow window puts the task view under the list, at full width.
#[gpui_kit::test]
fn the_preview_wraps_in_a_narrow_window(cx: &mut TestAppContext) {
    let (handle, _) = open(420., 352., cx);
    let (right, bad) = overflowing(handle, cx);
    assert!(bad.is_empty(), "text past {right:?}:\n{}", bad.join("\n"));
}

#[gpui_kit::test]
fn the_preview_wraps_in_columns(cx: &mut TestAppContext) {
    // Two tabs at 30 rem each fit in 1100 px, so the tabs show as columns.
    let (handle, _) = open(1600., 1100., cx);
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find(("preview-column", 1usize)).is_some());
    })
    .unwrap();
    let (right, bad) = overflowing(handle, cx);
    assert!(bad.is_empty(), "text past {right:?}:\n{}", bad.join("\n"));
}

/// The text wraps again when the task view gets narrower: a smaller
/// window squeezes the task view, then stacks it under the list.
#[gpui_kit::test]
fn the_preview_wraps_again_after_a_resize(cx: &mut TestAppContext) {
    let (handle, _) = open(2400., 900., cx);
    overflowing(handle, cx);
    for width in [1100., 420.] {
        cx.simulate_window_resize(handle, size(px(width), px(900.)));
        for _ in 0..2 {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .unwrap();
            cx.run_until_parked();
        }
        let (right, bad) = overflowing(handle, cx);
        assert!(
            bad.is_empty(),
            "at {width} px, text past {right:?}:\n{}",
            bad.join("\n")
        );
    }
}

/// Diagnostic: lays out every `.md` file in `$LLMTODO_WRAP_SAMPLES` at the
/// narrowest task view and lists the text that runs past the edge.
#[gpui_kit::test]
#[ignore]
fn wrap_samples(cx: &mut TestAppContext) {
    let Ok(dir) = std::env::var("LLMTODO_WRAP_SAMPLES") else {
        return;
    };
    let (handle, view) = open(1200., 352., cx);
    let mut report = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let body = std::fs::read_to_string(&path).unwrap();
        view.update(cx, |this, cx| {
            let mut issue = this.issue_by_key(WRAP_KEY, cx).clone();
            issue.description = format!("Every case\n\n{body}").into();
            this.upsert_issue(issue, cx);
        });
        for _ in 0..2 {
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .unwrap();
            cx.run_until_parked();
        }
        let (_, bad) = overflowing(handle, cx);
        for b in bad {
            report.push(format!("{}: {b}", path.display()));
        }
    }
    eprintln!("WRAP {} overflows\n{}", report.len(), report.join("\n"));
}
