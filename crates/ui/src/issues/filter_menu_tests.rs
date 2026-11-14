//! UI integration tests for the filter bar's filter button and saved
//! filter button (LLM-247).

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

/// Opens a menu button, then presses each key in turn.
fn pick(
    handle: AnyWindowHandle,
    button: &'static str,
    keys: &[&'static str],
    cx: &mut TestAppContext,
) {
    cx.update_window(handle, |_, window, cx| {
        window.click(button, cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    for key in keys {
        cx.update_window(handle, |_, window, cx| {
            window.press(key, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}

fn filter(view: &Entity<Workspace>, cx: &TestAppContext) -> String {
    cx.read(|cx| view.read(cx).filter_text(cx).to_string())
}

#[gpui_kit::test]
fn the_panel_adds_a_word_to_the_typed_filter(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command("filter Crash", window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    // Enter opens Status with its first entry, Open, selected.
    pick(handle, "filter-panel", &["ctrl-n", "enter", "enter"], cx);
    assert_eq!(filter(&view, cx), "Crash status:open");
    // The same entry again turns the word off.
    pick(handle, "filter-panel", &["ctrl-n", "enter", "enter"], cx);
    assert_eq!(filter(&view, cx), "Crash");
}

#[gpui_kit::test]
fn clear_keeps_the_free_text(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            this.run_command("filter fix label:bug assignee:me", window, cx)
        });
        window.render_frame(cx);
    })
    .unwrap();
    // Up from the top wraps to the last items: Save filter, then Clear.
    pick(handle, "filter-panel", &["ctrl-p", "ctrl-p", "enter"], cx);
    assert_eq!(filter(&view, cx), "fix");
}

#[gpui_kit::test]
fn the_saved_filter_button_opens_and_leaves_a_view(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    pick(handle, "saved-filters", &["ctrl-n", "enter"], cx);
    cx.read(|cx| assert_eq!(view.read(cx).scope(cx), Scope::View("Open bugs".into())));
    // The last item leaves the view.
    pick(handle, "saved-filters", &["ctrl-p", "enter"], cx);
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
