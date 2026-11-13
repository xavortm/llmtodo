//! UI integration tests for the Settings view: opening and leaving it, the
//! switches and number fields, hiding old closed tasks, and what `ui.json`
//! keeps.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, WindowOptions};

use crate::model::scope::Scope;
use crate::settings::view::Row;
use crate::settings::{SettingsStore, UiSettings};
use crate::shell::workspace::Workspace;
use crate::task::task_view::DetailMode;
use crate::task::vim_editor::VimMode;

fn open_with(
    settings: UiSettings,
    store: SettingsStore,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(crate::init);
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
    open_with(UiSettings::default(), SettingsStore::memory(), cx)
}

fn run(handle: AnyWindowHandle, view: &Entity<Workspace>, command: &str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.run_command(command, window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn press(handle: AnyWindowHandle, keys: &[&str], cx: &mut TestAppContext) {
    for key in keys {
        cx.update_window(handle, |_, window, cx| {
            window.press(key, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}

fn click(handle: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn shown(view: &Entity<Workspace>, cx: &TestAppContext) -> bool {
    cx.read(|cx| view.read(cx).settings_shown(cx))
}

fn scope(view: &Entity<Workspace>, cx: &TestAppContext) -> Scope {
    cx.read(|cx| view.read(cx).list().read(cx).scope.clone())
}

fn saved_store() -> (tempfile::TempDir, SettingsStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = SettingsStore::at(dir.path().join("ui.json"));
    (dir, store)
}

#[gpui_kit::test]
fn esc_leaves_settings_and_restores_the_scope_and_selection(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "show trash", cx);
    run(handle, &view, "all", cx);
    press(handle, &["j", "j"], cx);
    let before = cx.read(|cx| view.read(cx).selected(cx));
    assert_eq!(before, 2);

    run(handle, &view, "settings", cx);
    assert!(shown(&view, cx));

    press(handle, &["escape"], cx);
    assert!(!shown(&view, cx));
    assert_eq!(scope(&view, cx), Scope::All);
    assert_eq!(cx.read(|cx| view.read(cx).selected(cx)), before);
}

#[gpui_kit::test]
fn the_mouse_back_button_leaves_settings_like_esc(cx: &mut TestAppContext) {
    use gpui_kit::{MouseButton, MouseDownEvent, NavigationDirection};
    let (handle, view) = open(cx);
    run(handle, &view, "settings", cx);
    cx.update_window(handle, |_, window, cx| {
        let position = window.find("settings-view").bounds().center();
        crate::shell::tests::mouse(
            window,
            MouseDownEvent {
                button: MouseButton::Navigate(NavigationDirection::Back),
                position,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            },
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!shown(&view, cx));
}

#[gpui_kit::test]
fn esc_leaves_settings_from_a_number_field(cx: &mut TestAppContext) {
    let settings = UiSettings {
        hide_closed: true,
        ..Default::default()
    };
    let (handle, view) = open_with(settings, SettingsStore::memory(), cx);
    run(handle, &view, "settings", cx);
    press(handle, &["j", "enter"], cx);
    cx.update_window(handle, |_, window, cx| {
        assert_eq!(view.read(cx).vim_mode(window, cx), "insert");
    })
    .unwrap();
    press(handle, &["escape"], cx);
    assert!(!shown(&view, cx));
}

#[gpui_kit::test]
fn the_sidebar_link_and_cmd_comma_open_settings(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    press(handle, &["cmd-,"], cx);
    assert!(shown(&view, cx));
    press(handle, &["escape"], cx);
    assert!(!shown(&view, cx));

    run(handle, &view, "sidebar", cx);
    click(handle, "sidebar-settings", cx);
    assert!(shown(&view, cx));
    // Focus is in the view, so j moves its rows and not the list.
    press(handle, &["j"], cx);
    assert_eq!(
        cx.read(|cx| view.read(cx).settings_view().read(cx).selected_row()),
        Row::HideClosedDays
    );
}

#[gpui_kit::test]
fn a_click_on_a_switch_row_applies_and_saves_at_once(cx: &mut TestAppContext) {
    let (_dir, store) = saved_store();
    let (handle, view) = open_with(UiSettings::default(), store.clone(), cx);
    run(handle, &view, "settings", cx);

    click(handle, "settings-row-vim", cx);
    assert!(!store.load().vim_editor);
    click(handle, "settings-row-vim", cx);
    assert!(store.load().vim_editor);

    click(handle, "settings-row-hide-closed", cx);
    assert!(store.load().hide_closed);

    click(handle, "settings-row-open-last", cx);
    assert!(!store.load().open_last_view);

    click(handle, "settings-row-mascot", cx);
    assert!(!store.load().mascot);
    click(handle, "settings-row-mascot", cx);
    assert!(store.load().mascot);

    assert_eq!(
        cx.read(|cx| view.read(cx).current_settings(cx).theme),
        "dark"
    );
    click(handle, "settings-row-dark", cx);
    assert_eq!(store.load().theme, "light");
    assert!(shown(&view, cx), "a switch does not leave the view");
}

#[gpui_kit::test]
fn the_keyboard_moves_flips_and_steps(cx: &mut TestAppContext) {
    let (_dir, store) = saved_store();
    let (handle, view) = open_with(UiSettings::default(), store.clone(), cx);
    run(handle, &view, "settings", cx);
    let selected =
        |cx: &TestAppContext| cx.read(|cx| view.read(cx).settings_view().read(cx).selected_row());

    press(handle, &["ctrl-n", "j"], cx);
    assert_eq!(selected(cx), Row::Vim);
    press(handle, &["space"], cx);
    assert!(!store.load().vim_editor);

    press(handle, &["j"], cx);
    assert_eq!(selected(cx), Row::Theme);
    press(handle, &["l"], cx);
    assert_eq!(store.load().color_theme, "gruvbox");
    press(handle, &["h"], cx);
    assert_eq!(store.load().color_theme, "default");

    press(handle, &["j", "j"], cx);
    assert_eq!(selected(cx), Row::Zoom);
    press(handle, &["l"], cx);
    assert!((store.load().zoom - 1.1).abs() < 0.001);
    let text = cx.read(|cx| {
        view.read(cx)
            .settings_view()
            .read(cx)
            .zoom_input()
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(text, "110");

    press(handle, &["ctrl-p"], cx);
    assert_eq!(selected(cx), Row::DarkMode);
    press(handle, &["ctrl-p", "ctrl-p", "ctrl-p"], cx);
    assert_eq!(selected(cx), Row::HideClosedDays);
    // The days row is off until the setting is on.
    press(handle, &["l"], cx);
    assert_eq!(store.load().hide_closed_days, 14);
}

#[gpui_kit::test]
fn a_click_on_a_theme_preview_picks_it_and_keeps_the_mode(cx: &mut TestAppContext) {
    use crate::shell::theme::{self, ColorTheme};
    use gpui_kit::component::ActiveTheme as _;

    let (_dir, store) = saved_store();
    let (handle, view) = open_with(UiSettings::default(), store.clone(), cx);
    run(handle, &view, "settings", cx);

    click(handle, "settings-theme-gruvbox", cx);
    assert_eq!(cx.read(theme::color_theme), ColorTheme::Gruvbox);
    assert_eq!(store.load().color_theme, "gruvbox");
    assert_eq!(
        cx.read(|cx| cx.theme().theme_name().to_string()),
        "Gruvbox Dark"
    );

    click(handle, "settings-row-dark", cx);
    assert_eq!(store.load().theme, "light");
    assert_eq!(
        store.load().color_theme,
        "gruvbox",
        "the mode keeps the theme"
    );
    assert_eq!(
        cx.read(|cx| cx.theme().theme_name().to_string()),
        "Gruvbox Light"
    );

    click(handle, "settings-theme-default", cx);
    assert_eq!(store.load().color_theme, "default");
    assert_eq!(store.load().theme, "light", "the theme keeps the mode");
    assert_eq!(
        cx.read(|cx| view.read(cx).settings_view().read(cx).selected_row()),
        Row::Theme
    );

    run(handle, &view, "theme gruvbox", cx);
    assert_eq!(store.load().color_theme, "gruvbox");

    click(handle, "settings-theme-macos", cx);
    assert_eq!(store.load().color_theme, "macos");
    assert_eq!(
        cx.read(|cx| cx.theme().theme_name().to_string()),
        "macOS Light"
    );
    assert_eq!(
        cx.read(|cx| cx.theme().font_family.to_string()),
        ".SystemUIFont"
    );
}

#[gpui_kit::test]
fn the_zoom_field_follows_a_zoom_from_elsewhere(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    run(handle, &view, "settings", cx);
    press(handle, &["cmd-="], cx);
    let text = cx.read(|cx| {
        view.read(cx)
            .settings_view()
            .read(cx)
            .zoom_input()
            .read(cx)
            .value()
            .to_string()
    });
    assert_eq!(text, "110");
}

#[gpui_kit::test]
fn typing_days_applies_at_once(cx: &mut TestAppContext) {
    let settings = UiSettings {
        hide_closed: true,
        ..Default::default()
    };
    let (_dir, store) = saved_store();
    let (handle, view) = open_with(settings, store.clone(), cx);
    run(handle, &view, "settings", cx);
    press(handle, &["j", "enter"], cx);
    cx.update_window(handle, |_, window, cx| {
        window.press("cmd-a", cx);
        window.input("30", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(store.load().hide_closed_days, 30);
}

/// Marks the first Done demo issue as closed `days` days ago.
fn close_first_done(view: &Entity<Workspace>, days: i64, cx: &mut TestAppContext) -> String {
    view.update(cx, |this, cx| {
        let closed_at = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(days * 24);
        this.store().update(cx, |store, _| {
            let id = store
                .iter()
                .find(|i| i.status.is_closed())
                .map(|i| i.id)
                .unwrap();
            store
                .modify(id, |issue| {
                    issue.closed_at = Some(closed_at);
                    issue.key.to_string()
                })
                .unwrap()
        })
    })
}

fn visible_keys(view: &Entity<Workspace>, cx: &TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        let list = view.read(cx).list().read(cx);
        let store = view.read(cx).store().read(cx);
        list.visible()
            .iter()
            .filter_map(|id| store.get(*id).map(|i| i.key.to_string()))
            .collect()
    })
}

#[gpui_kit::test]
fn old_closed_tasks_hide_and_the_list_counts_them(cx: &mut TestAppContext) {
    let (_handle, view) = open(cx);
    let old = close_first_done(&view, 30, cx);
    assert!(visible_keys(&view, cx).contains(&old));

    view.update(cx, |this, cx| this.set_hide_closed(true, cx));
    assert!(!visible_keys(&view, cx).contains(&old));
    assert_eq!(
        cx.read(|cx| view.read(cx).list().read(cx).hidden_closed()),
        1
    );

    // A longer limit shows it again.
    view.update(cx, |this, cx| this.set_hide_closed_days(60, cx));
    assert!(visible_keys(&view, cx).contains(&old));
    assert_eq!(
        cx.read(|cx| view.read(cx).list().read(cx).hidden_closed()),
        0
    );
}

#[gpui_kit::test]
fn a_recently_closed_task_stays_visible(cx: &mut TestAppContext) {
    let (_handle, view) = open(cx);
    let recent = close_first_done(&view, 3, cx);
    view.update(cx, |this, cx| this.set_hide_closed(true, cx));
    assert!(visible_keys(&view, cx).contains(&recent));
}

#[gpui_kit::test]
fn with_vim_off_the_editor_types_and_esc_saves(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    view.update(cx, |this, cx| this.set_vim_editor(false, cx));
    press(handle, &["enter", "e"], cx);
    assert_eq!(
        cx.read(|cx| view.read(cx).detail(cx).map(|d| d.mode)),
        Some(DetailMode::Editing)
    );
    assert_eq!(cx.read(|cx| view.read(cx).editor_vim(cx)), VimMode::Insert);

    press(handle, &["escape"], cx);
    assert_eq!(
        cx.read(|cx| view.read(cx).detail(cx).map(|d| d.mode)),
        Some(DetailMode::Preview)
    );
}

#[gpui_kit::test]
fn settings_is_never_saved_as_the_last_view(cx: &mut TestAppContext) {
    let (_dir, store) = saved_store();
    let (handle, view) = open_with(UiSettings::default(), store.clone(), cx);
    run(handle, &view, "show trash", cx);
    run(handle, &view, "settings", cx);
    assert_eq!(store.load().sidebar_scope, "trash");
    press(handle, &["escape"], cx);
    assert_eq!(scope(&view, cx), Scope::Trash);
}

#[gpui_kit::test]
fn the_last_view_and_task_reopen_at_start(cx: &mut TestAppContext) {
    let settings = UiSettings {
        sidebar_scope: "project:server".into(),
        last_task: Some("LLM-102".into()),
        ..Default::default()
    };
    let (_handle, view) = open_with(settings, SettingsStore::memory(), cx);
    assert_eq!(scope(&view, cx), Scope::Project("server".into()));
    view.update(cx, |this, cx| this.restore_last_task(cx));
    let open = cx.read(|cx| {
        let this = view.read(cx);
        this.detail(cx)
            .and_then(|d| this.store().read(cx).get(d.issue))
            .map(|i| i.key.to_string())
    });
    assert_eq!(open.as_deref(), Some("LLM-102"));
}

#[gpui_kit::test]
fn with_open_last_off_the_app_starts_on_all_issues(cx: &mut TestAppContext) {
    let settings = UiSettings {
        sidebar_scope: "project:server".into(),
        last_task: Some("LLM-102".into()),
        open_last_view: false,
        ..Default::default()
    };
    let (_handle, view) = open_with(settings, SettingsStore::memory(), cx);
    assert_eq!(scope(&view, cx), Scope::All);
    view.update(cx, |this, cx| this.restore_last_task(cx));
    assert!(cx.read(|cx| view.read(cx).detail(cx)).is_none());
}

#[gpui_kit::test]
fn commands_do_not_act_on_the_task_behind_settings(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    press(handle, &["enter"], cx);
    let before = cx.read(|cx| {
        view.read(cx).issues_and_catalog(cx).0[0]
            .status
            .name
            .clone()
    });
    run(handle, &view, "settings", cx);
    run(handle, &view, "status done", cx);
    let after = cx.read(|cx| {
        view.read(cx).issues_and_catalog(cx).0[0]
            .status
            .name
            .clone()
    });
    assert_eq!(before, after);
}
