//! UI integration tests for the macOS shortcuts, zoom and the menu bar.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, ClipboardItem, Entity, Keystroke, MenuItem, TestAppContext,
    WindowOptions,
};

use crate::settings::{SettingsStore, UiSettings};
use crate::shell::app_menus;
use crate::shell::workspace::Workspace;
use crate::task::vim_editor::VimMode;

fn open_with(
    settings: UiSettings,
    store: SettingsStore,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
            cx.new(|cx| {
                let mut this = Workspace::with_settings(settings, store, window, cx);
                this.load_demo(cx);
                this
            })
        })
        .unwrap()
    })
}

fn open(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
    open_with(UiSettings::default(), SettingsStore::memory(), cx)
}

fn zoom(cx: &TestAppContext) -> f32 {
    cx.read(crate::shell::theme::zoom)
}

#[gpui_kit::test]
fn cmd_and_ctrl_keys_zoom_and_the_level_is_saved(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.json");
    cx.update(crate::init);
    let (handle, view) = open_with(UiSettings::default(), SettingsStore::at(path.clone()), cx);
    assert_eq!(zoom(cx), 1.0);

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("cmd-=", cx);
        assert_eq!(crate::shell::theme::zoom(cx), 1.1);
        // Zoom scales the font sizes, and GPUI Kit sets rem size from them.
        assert_eq!(cx.theme().font_size, gpui_kit::px(16. * 1.1));
        assert_eq!(cx.theme().mono_font_size, gpui_kit::px(15. * 1.1));
        assert_eq!(view.read(cx).message(), "Zoom 110%");
        window.press("ctrl-=", cx);
        assert_eq!(crate::shell::theme::zoom(cx), 1.25);
        window.press("cmd--", cx);
        window.press("ctrl--", cx);
        window.press("cmd--", cx);
        assert_eq!(crate::shell::theme::zoom(cx), 0.9);
        window.press("cmd-0", cx);
        assert_eq!(crate::shell::theme::zoom(cx), 1.0);
        window.press("cmd-=", cx);
        window.press("cmd-=", cx);
        window.press("ctrl-0", cx);
        assert_eq!(crate::shell::theme::zoom(cx), 1.0);
        window.press("ctrl-=", cx);
    })
    .unwrap();
    assert_eq!(zoom(cx), 1.1);
    let saved = SettingsStore::at(path.clone()).load();
    assert_eq!(saved.zoom, 1.1, "saved on change");

    // A theme change keeps the zoom.
    cx.update(|cx| crate::shell::theme::set_mode(gpui_kit::component::ThemeMode::Light, cx));
    assert_eq!(cx.read(|cx| cx.theme().font_size), gpui_kit::px(16. * 1.1));

    // A new window starts at the saved zoom.
    cx.update(|cx| crate::shell::theme::set_zoom(1.0, cx));
    let (_, view) = open_with(saved, SettingsStore::at(path), cx);
    assert_eq!(zoom(cx), 1.1);
    cx.read(|cx| assert_eq!(view.read(cx).current_settings(cx).zoom, 1.1));
}

#[gpui_kit::test]
fn zoom_stops_at_the_ends(cx: &mut TestAppContext) {
    let (handle, _) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        for _ in 0..20 {
            window.press("cmd-=", cx);
        }
        assert_eq!(crate::shell::theme::zoom(cx), 2.0);
        for _ in 0..20 {
            window.press("cmd--", cx);
        }
        assert_eq!(crate::shell::theme::zoom(cx), 0.5);
    })
    .unwrap();
}

#[gpui_kit::test]
fn cmd_w_closes_the_window(cx: &mut TestAppContext) {
    let (handle, _) = open(cx);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("cmd-w", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.update(|cx| cx.windows().is_empty()));
}

#[gpui_kit::test]
fn cmd_comma_opens_the_settings_view(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("cmd-,", cx);
        assert!(view.read(cx).settings_shown(cx));
    })
    .unwrap();
}

/// Each standard key reaches its action, and no other binding in the app
/// or GPUI Kit uses the same key.
#[gpui_kit::test]
fn standard_keys_bind_once_and_to_the_right_action(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let expected: &[(&str, &str)] = &[
        ("cmd-q", "app::Quit"),
        ("cmd-w", "app::CloseWindow"),
        ("cmd-m", "app::Minimize"),
        ("cmd-ctrl-f", "app::ToggleFullScreen"),
        ("cmd-h", "app::Hide"),
        ("cmd-alt-h", "app::HideOthers"),
        ("cmd-,", "app::OpenSettings"),
        ("cmd-=", "app::ZoomIn"),
        ("cmd-+", "app::ZoomIn"),
        ("cmd--", "app::ZoomOut"),
        ("cmd-0", "app::ZoomReset"),
        ("ctrl-=", "app::ZoomIn"),
        ("ctrl-+", "app::ZoomIn"),
        ("ctrl--", "app::ZoomOut"),
        ("ctrl-0", "app::ZoomReset"),
    ];
    cx.update(|cx| {
        for (key, action) in expected {
            let keystroke = Keystroke::parse(key).unwrap();
            let bindings = cx.all_bindings_for_input(&[keystroke]);
            let names: Vec<_> = bindings.iter().map(|b| b.action().name()).collect();
            assert_eq!(names, [*action], "{key}");
        }
    });
}

#[gpui_kit::test]
fn menu_bar_lists_the_menus_and_never_shows_a_plain_key(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let menus = app_menus::menus();
    let names: Vec<_> = menus.iter().map(|m| m.name.to_string()).collect();
    assert_eq!(names, ["llmtodo", "Edit", "Issue", "View", "Window"]);

    // macOS shows an action's first binding as its menu key. A plain key
    // there could fire while the owner types, so every menu key must use
    // Cmd or Ctrl.
    cx.update(|cx| {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let mut checked = 0;
        for menu in &menus {
            for item in &menu.items {
                let MenuItem::Action { name, action, .. } = item else {
                    continue;
                };
                if let Some(binding) = keymap.bindings_for_action(action.as_ref()).next() {
                    let m = binding.keystrokes()[0].modifiers();
                    assert!(m.platform || m.control, "{name} shows a plain key");
                    checked += 1;
                }
            }
        }
        assert!(checked >= 15, "most menu items show a key, got {checked}");
    });
}

/// Cmd-V and Cmd-X work in the editor in vim normal mode too, as in any
/// macOS text field. Before LLM-84, normal mode swallowed them.
#[gpui_kit::test]
fn cmd_v_and_cmd_x_work_in_the_editor_in_normal_mode(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let text = |cx: &mut gpui_kit::App| view.read(cx).editor(cx).read(cx).value().to_string();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.press("e", cx);
        assert_eq!(view.read(cx).editor_vim(cx), VimMode::Normal);
        let before = text(cx);

        cx.write_to_clipboard(ClipboardItem::new_string("pasted ".into()));
        window.press("cmd-v", cx);
        assert_eq!(text(cx), format!("pasted {before}"));
        assert_eq!(view.read(cx).editor_vim(cx), VimMode::Normal);

        window.press("cmd-a", cx);
        window.press("cmd-x", cx);
        assert_eq!(text(cx), "");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()),
            Some(format!("pasted {before}"))
        );

        // Undo is Cmd-Z and redo Cmd-Shift-Z; vim's u still works too.
        window.press("cmd-z", cx);
        assert_eq!(text(cx), format!("pasted {before}"));
        window.press("cmd-shift-z", cx);
        assert_eq!(text(cx), "");
        window.press("u", cx);
        assert_eq!(text(cx), format!("pasted {before}"));
    })
    .unwrap();
}

/// LLM-212: zoom in, zoom out and reset never change the title bar. The
/// rest of the app still zooms.
#[gpui_kit::test]
fn zoom_does_not_change_the_title_bar(cx: &mut TestAppContext) {
    let (handle, _view) = open(cx);
    // The bar's height, the sidebar button, and the folder text, whose
    // size follows its font size. The window's rem size shows the zoom.
    let measure = |key: &str, cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
            window.render_frame(cx);
            (
                window.find("title-bar").bounds().size,
                window.find("toggle-sidebar").bounds().size,
                window.find("project-path").bounds().size,
                window.rem_size(),
            )
        })
        .unwrap()
    };
    let (bar, button, path, rem) = measure("escape", cx);
    assert!(bar.height > gpui_kit::px(0.) && path.width > gpui_kit::px(0.));
    for key in [
        "cmd-=", "cmd-=", "cmd-=", "cmd-0", "cmd--", "cmd--", "cmd-0",
    ] {
        let (zoomed_bar, zoomed_button, zoomed_path, zoomed_rem) = measure(key, cx);
        let level = zoom(cx);
        assert_eq!(zoomed_rem, rem * level, "the app zooms after {key}");
        assert_eq!(zoomed_bar, bar, "title bar after {key}");
        assert_eq!(zoomed_button, button, "sidebar button after {key}");
        assert_eq!(zoomed_path, path, "folder text after {key}");
    }
    assert_eq!(zoom(cx), 1.0);
}
