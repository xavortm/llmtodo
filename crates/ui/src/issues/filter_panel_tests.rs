//! UI integration tests for the filter bar's filter panel and saved filter
//! button (LLM-247).

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

use crate::model::scope::Scope;
use crate::settings::{SettingsStore, UiSettings};
use crate::shell::workspace::Workspace;

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    let (handle, view) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| {
                let store = SettingsStore::memory();
                let mut this = Workspace::with_settings(UiSettings::default(), store, window, cx);
                this.load_demo(cx);
                this
            })
        })
        .unwrap()
    });
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
    (handle, view)
}

fn click(handle: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        window.click(id, cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

fn press(handle: AnyWindowHandle, keys: &[&'static str], cx: &mut TestAppContext) {
    for key in keys {
        cx.update_window(handle, |_, window, cx| {
            window.press(key, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}

fn command(handle: AnyWindowHandle, view: &Entity<Workspace>, text: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command(text, window, cx));
        window.render_frame(cx);
    })
    .unwrap();
}

fn filter(view: &Entity<Workspace>, cx: &TestAppContext) -> String {
    cx.read(|cx| view.read(cx).filter_text(cx).to_string())
}

fn panel_open(view: &Entity<Workspace>, cx: &TestAppContext) -> bool {
    cx.read(|cx| view.read(cx).filter_panel_open(cx))
}

fn panel_row(view: &Entity<Workspace>, cx: &TestAppContext) -> Option<String> {
    cx.read(|cx| view.read(cx).filter_panel_row(cx).map(|s| s.to_string()))
}

#[gpui_kit::test]
fn the_filter_button_opens_and_closes_the_panel(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    assert!(!panel_open(&view, cx));
    click(handle, "filter-panel", cx);
    assert!(panel_open(&view, cx));
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find("filter-side-panel").is_some());
        // The list stays in the window beside the panel.
        assert!(window.try_find("filter-bar").is_some());
    })
    .unwrap();
    click(handle, "filter-panel", cx);
    assert!(!panel_open(&view, cx));
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find("filter-side-panel").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn each_click_filters_the_list_and_the_panel_stays_open(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let all = cx.read(|cx| view.read(cx).visible_count(cx));
    command(handle, &view, "filter Crash", cx);
    click(handle, "filter-panel", cx);
    click(handle, "filter-row-Label-bug", cx);
    assert_eq!(filter(&view, cx), "Crash label:bug");
    assert!(panel_open(&view, cx));
    // A second click in the same open panel adds a second word.
    click(handle, "filter-row-Status-Open", cx);
    assert_eq!(filter(&view, cx), "Crash label:bug status:open");
    assert!(panel_open(&view, cx));
    // The list follows each click.
    let shown = cx.read(|cx| view.read(cx).visible_count(cx));
    assert!(shown < all, "{shown} of {all}");
    // A click on a checked row turns its word off.
    click(handle, "filter-row-Label-bug", cx);
    assert_eq!(filter(&view, cx), "Crash status:open");
    click(handle, "clear-filters", cx);
    assert_eq!(filter(&view, cx), "Crash");
    assert!(panel_open(&view, cx));
}

#[gpui_kit::test]
fn the_list_takes_clicks_while_the_panel_is_open(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    click(handle, "filter-panel", cx);
    cx.update_window(handle, |_, window, cx| {
        window.click(("row", 3usize), cx);
        window.render_frame(cx);
        assert!(view.read(cx).detail(cx).is_some());
    })
    .unwrap();
    assert!(panel_open(&view, cx));
}

#[gpui_kit::test]
fn keys_move_and_toggle_and_esc_closes(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    click(handle, "filter-panel", cx);
    assert_eq!(panel_row(&view, cx).as_deref(), Some("Open"));
    // Up from the first row wraps to the last.
    press(handle, &["ctrl-p"], cx);
    assert_eq!(panel_row(&view, cx).as_deref(), Some("Analyzing"));
    press(handle, &["ctrl-n", "enter"], cx);
    assert_eq!(filter(&view, cx), "status:open");
    press(handle, &["j", "space"], cx);
    assert_ne!(panel_row(&view, cx).as_deref(), Some("Open"));
    assert!(filter(&view, cx).starts_with("status:open status:"));
    press(handle, &["escape"], cx);
    assert!(!panel_open(&view, cx));
    // The filter stays after the panel closes.
    assert!(filter(&view, cx).starts_with("status:open"));
}

#[gpui_kit::test]
fn esc_closes_the_panel_from_the_list_too(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    command(handle, &view, "filters", cx);
    assert!(panel_open(&view, cx));
    // A click on a task moves focus to the list.
    cx.update_window(handle, |_, window, cx| {
        window.click(("row", 3usize), cx);
        window.render_frame(cx);
    })
    .unwrap();
    press(handle, &["escape"], cx);
    assert!(!panel_open(&view, cx));
}

#[gpui_kit::test]
fn the_saved_filter_button_opens_and_leaves_a_view(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    click(handle, "saved-filters", cx);
    press(handle, &["ctrl-n", "enter"], cx);
    cx.read(|cx| assert_eq!(view.read(cx).scope(cx), Scope::View("Open bugs".into())));
    // The last item leaves the view.
    click(handle, "saved-filters", cx);
    press(handle, &["ctrl-p", "enter"], cx);
    cx.read(|cx| assert_eq!(view.read(cx).scope(cx), Scope::All));
}

#[gpui_kit::test]
fn no_saved_view_hides_the_saved_filter_button(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        assert!(window.try_find("saved-filters").is_some());
        let store = view.read(cx).store().clone();
        store.update(cx, |store, cx| {
            store.catalog.views.clear();
            cx.notify();
        });
        view.update(cx, |_, cx| cx.notify());
        window.render_frame(cx);
        assert!(window.try_find("saved-filters").is_none());
        assert!(window.try_find("filter-panel").is_some());
    })
    .unwrap();
}
