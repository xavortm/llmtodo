//! UI integration tests for the Settings view: opening and leaving it, the
//! switches and number fields, hiding old closed tasks, and what the settings
//! files keep.

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
    click(handle, "settings-appearance-light", cx);
    assert_eq!(store.load().theme, "light");
    assert!(shown(&view, cx), "a switch does not leave the view");
}

#[gpui_kit::test]
fn the_appearance_control_picks_dark_auto_or_light(cx: &mut TestAppContext) {
    use gpui_kit::component::ActiveTheme as _;
    let is_dark = |cx: &TestAppContext| cx.read(|cx| cx.theme().is_dark());

    let (_dir, store) = saved_store();
    let (handle, view) = open_with(UiSettings::default(), store.clone(), cx);
    run(handle, &view, "settings", cx);
    assert!(is_dark(cx));

    // The test window reports a light macOS appearance.
    click(handle, "settings-appearance-auto", cx);
    assert_eq!(store.load().theme, "auto");
    assert!(!is_dark(cx), "auto follows macOS");
    assert_eq!(
        cx.read(|cx| view.read(cx).settings_view().read(cx).selected_row()),
        Row::Appearance
    );

    click(handle, "settings-appearance-dark", cx);
    assert_eq!(store.load().theme, "dark");
    assert!(is_dark(cx));

    // h and l step along the control and stop at its ends.
    press(handle, &["l"], cx);
    assert_eq!(store.load().theme, "auto");
    press(handle, &["l", "l"], cx);
    assert_eq!(store.load().theme, "light");
    press(handle, &["enter"], cx);
    assert_eq!(store.load().theme, "dark", "enter wraps around");

    run(handle, &view, "theme auto", cx);
    assert_eq!(store.load().theme, "auto");
    run(handle, &view, "theme dark", cx);
    assert_eq!(store.load().theme, "dark");
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
    assert_eq!(selected(cx), Row::Appearance);
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

    click(handle, "settings-appearance-light", cx);
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

    click(handle, "settings-theme-catppuccin", cx);
    assert_eq!(store.load().color_theme, "catppuccin");
    assert_eq!(
        cx.read(|cx| cx.theme().theme_name().to_string()),
        "Catppuccin Latte"
    );
    click(handle, "settings-appearance-dark", cx);
    assert_eq!(
        cx.read(|cx| cx.theme().theme_name().to_string()),
        "Catppuccin Mocha"
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

/// LLM-214: 0 in the field saves, and a task closed a moment ago hides.
#[gpui_kit::test]
fn zero_days_saves_and_hides_a_task_closed_just_now(cx: &mut TestAppContext) {
    let settings = UiSettings {
        hide_closed: true,
        ..Default::default()
    };
    let (_dir, store) = saved_store();
    let (handle, view) = open_with(settings, store.clone(), cx);
    let recent = close_first_done(&view, 0, cx);
    run(handle, &view, "settings", cx);
    press(handle, &["j", "enter"], cx);
    cx.update_window(handle, |_, window, cx| {
        window.press("cmd-a", cx);
        window.input("0", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(store.load().hide_closed_days, 0);
    assert!(!visible_keys(&view, cx).contains(&recent));
    // The stepper stays at 0 below it.
    view.update(cx, |this, cx| this.set_hide_closed_days(0, cx));
    assert_eq!(store.load().hide_closed_days, 0);
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

/// The first list row whose task status passes `pick`.
fn row_where(
    view: &Entity<Workspace>,
    pick: impl Fn(&crate::model::data::Issue) -> bool,
    cx: &TestAppContext,
) -> usize {
    cx.read(|cx| {
        let this = view.read(cx);
        let store = this.store().read(cx);
        this.rows(cx)
            .iter()
            .position(|row| match row {
                crate::issues::list::Row::Issue(id) => store.get(*id).is_some_and(&pick),
                _ => false,
            })
            .expect("a row of that kind")
    })
}

fn struck(handle: AnyWindowHandle, row: usize, cx: &mut TestAppContext) -> bool {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window
            .within(("row", row))
            .try_find("title-struck")
            .is_some()
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_line_through_done_tasks_is_on_by_default_and_a_switch_turns_it_off(cx: &mut TestAppContext) {
    let (_dir, store) = saved_store();
    let (handle, view) = open_with(UiSettings::default(), store.clone(), cx);
    let done = row_where(&view, |i| i.status.is_done(), cx);
    let open = row_where(&view, |i| !i.status.is_closed(), cx);
    assert!(struck(handle, done, cx), "a Done title has the line");
    assert!(!struck(handle, open, cx), "an open title has none");

    run(handle, &view, "settings", cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .within("settings-row-strike-done")
                .try_find("settings-row-strike-done-experimental")
                .is_some(),
            "the row says it is experimental"
        );
    })
    .unwrap();
    click(handle, "settings-row-strike-done", cx);
    assert!(!store.load().strike_done);
    press(handle, &["escape"], cx);
    assert!(!struck(handle, done, cx), "off: no line");

    // The choice survives a restart.
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    let done = row_where(&view, |i| i.status.is_done(), cx);
    assert!(!struck(handle, done, cx));
}

/// Renders a frame, then counts the next-frame requests it left, which is
/// what a running animation asks for.
fn frames_asked(handle: AnyWindowHandle, cx: &mut TestAppContext) -> usize {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.simulate_next_frame(cx)
    })
    .unwrap()
}

fn open_palette(handle: AnyWindowHandle, view: &Entity<Workspace>, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            this.open_palette(&crate::shell::workspace::OpenPalette, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
}

/// LLM-195: animations are off by default, and then the palette opens
/// with no animation frames. The switch turns them on and saves at once.
/// On, the palette's entrance asks for frames only while it runs.
#[gpui_kit::test]
fn animations_are_off_by_default_and_a_switch_turns_them_on(cx: &mut TestAppContext) {
    let (_dir, store) = saved_store();
    let (handle, view) = open_with(UiSettings::default(), store.clone(), cx);
    assert!(!cx.read(crate::shell::motion::enabled));
    open_palette(handle, &view, cx);
    // The first frame after the open may ask for one more, for focus.
    frames_asked(handle, cx);
    assert_eq!(frames_asked(handle, cx), 0, "off: nothing animates");
    press(handle, &["escape"], cx);

    run(handle, &view, "settings", cx);
    click(handle, "settings-row-animations", cx);
    assert!(store.load().animations);
    assert!(cx.read(crate::shell::motion::enabled));
    press(handle, &["escape"], cx);

    open_palette(handle, &view, cx);
    frames_asked(handle, cx);
    // The test clock stands still, so the entrance is still running.
    assert!(frames_asked(handle, cx) > 0, "on: the palette animates in");
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    frames_asked(handle, cx);
    assert_eq!(frames_asked(handle, cx), 0, "the entrance stops when done");
    press(handle, &["escape"], cx);

    // The choice survives a restart.
    cx.update(|cx| crate::shell::motion::set(false, cx));
    let (_handle, _view) = open_with(store.load(), store.clone(), cx);
    assert!(cx.read(crate::shell::motion::enabled));
}

/// LLM-195: with animations on, an opened group's rows cascade in. The
/// list asks for frames only until the last row lands.
#[gpui_kit::test]
fn an_opened_group_cascades_in_and_then_stops(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    let list = cx.read(|cx| view.read(cx).list().clone());
    let title = cx.update(|cx| {
        list.update(cx, |list, cx| {
            list.group_by(crate::issues::list::GroupBy::Status, cx);
            list.rows()
                .iter()
                .find_map(|row| match row {
                    crate::issues::list::Row::Header { title, .. } => Some(title.to_string()),
                    _ => None,
                })
                .expect("a group")
        })
    });
    cx.update(|cx| list.update(cx, |list, cx| list.set_group_collapsed(&title, true, cx)));
    // The chevron turns and the open task slides in. The test clock
    // stands still, so move it past them.
    frames_asked(handle, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    frames_asked(handle, cx);
    assert_eq!(
        frames_asked(handle, cx),
        0,
        "closing leaves nothing running"
    );

    cx.update(|cx| list.update(cx, |list, cx| list.set_group_collapsed(&title, false, cx)));
    frames_asked(handle, cx);
    assert!(frames_asked(handle, cx) > 0, "the rows cascade in");
    cx.executor().advance_clock(
        crate::shell::motion::Wave::Open.total() + std::time::Duration::from_millis(50),
    );
    frames_asked(handle, cx);
    frames_asked(handle, cx);
    assert_eq!(frames_asked(handle, cx), 0, "the cascade stops");
}

/// LLM-195: with animations on, a closing group's rows stay drawn under
/// its header for a moment, cropped from the bottom, and then go.
#[gpui_kit::test]
fn a_closing_group_crops_its_rows_and_then_drops_them(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    let list = cx.read(|cx| view.read(cx).list().clone());
    let title = cx.update(|cx| {
        list.update(cx, |list, cx| {
            list.group_by(crate::issues::list::GroupBy::Status, cx);
            list.rows()
                .iter()
                .find_map(|row| match row {
                    crate::issues::list::Row::Header {
                        title,
                        collapsed: false,
                        count,
                        ..
                    } if *count > 0 => Some(title.to_string()),
                    _ => None,
                })
                .expect("an open group")
        })
    });
    frames_asked(handle, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    frames_asked(handle, cx);

    cx.update(|cx| list.update(cx, |list, cx| list.set_group_collapsed(&title, true, cx)));
    let ghost = ("row", usize::MAX / 2);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(ghost).is_some(),
            "the closing rows still show"
        );
    })
    .unwrap();
    assert!(frames_asked(handle, cx) > 0, "the close animates");

    cx.executor()
        .advance_clock(crate::shell::motion::CLOSE + std::time::Duration::from_millis(50));
    frames_asked(handle, cx);
    frames_asked(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(ghost).is_none(), "they go when it ends");
    })
    .unwrap();
}

/// LLM-218: with animations on, Close all groups folds every open group
/// at once, as one close does, and then stops. Off, it closes at once.
#[gpui_kit::test]
fn close_all_groups_folds_every_group_and_then_stops(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    let list = cx.read(|cx| view.read(cx).list().clone());
    cx.update(|cx| {
        list.update(cx, |list, cx| {
            list.group_by(crate::issues::list::GroupBy::Status, cx)
        })
    });
    frames_asked(handle, cx);
    // The first kept row of each group, in the fold.
    let ghosts = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            (0..20).any(|group| {
                window
                    .try_find(("row", usize::MAX / 2 + group * 40))
                    .is_some()
            })
        })
        .unwrap()
    };

    // Off: no fold, and nothing runs.
    cx.update(|cx| list.update(cx, |list, cx| list.close_all_groups(cx)));
    assert!(!ghosts(cx), "off, the groups close at once");
    assert_eq!(frames_asked(handle, cx), 0);
    cx.update(|cx| list.update(cx, |list, cx| list.open_all_groups(cx)));
    frames_asked(handle, cx);

    cx.update(|cx| crate::shell::motion::set(true, cx));
    frames_asked(handle, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    frames_asked(handle, cx);
    assert_eq!(frames_asked(handle, cx), 0);

    // The first task row, and its kept copy in the fold.
    let (first, ghost) = cx.read(|cx| {
        let rows = list.read(cx).rows();
        let first = rows
            .iter()
            .position(|row| matches!(row, crate::issues::list::Row::Issue(_)))
            .expect("a task row");
        let group = rows[..first]
            .iter()
            .filter(|row| matches!(row, crate::issues::list::Row::Header { .. }))
            .count()
            - 1;
        (first, usize::MAX / 2 + group * 40)
    });
    let before = cx
        .update_window(handle, |_, window, _| window.find(("row", first)).bounds())
        .unwrap();
    cx.update(|cx| list.update(cx, |list, cx| list.close_all_groups(cx)));
    assert!(ghosts(cx), "the groups' rows still show");
    let after = cx
        .update_window(handle, |_, window, _| window.find(("row", ghost)).bounds())
        .unwrap();
    assert_eq!(after, before, "the fold starts where the list was");
    assert!(frames_asked(handle, cx) > 0, "the close animates");
    assert_eq!(
        cx.read(|cx| list.read(cx).scroll_offset()),
        gpui_kit::px(0.),
        "the list sits at its top under the fold"
    );

    cx.executor()
        .advance_clock(crate::shell::motion::CLOSE + std::time::Duration::from_millis(50));
    frames_asked(handle, cx);
    frames_asked(handle, cx);
    assert!(!ghosts(cx), "they go when it ends");
    assert_eq!(frames_asked(handle, cx), 0, "the fold stops");
    assert!(cx.read(|cx| {
        list.read(cx).rows().iter().all(|row| {
            matches!(
                row,
                crate::issues::list::Row::Header {
                    collapsed: true,
                    ..
                }
            )
        })
    }));
}

/// LLM-195: a new sort sends a wave down the list, which then stops.
#[gpui_kit::test]
fn a_new_sort_sends_a_wave_and_then_stops(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    frames_asked(handle, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    frames_asked(handle, cx);
    assert_eq!(frames_asked(handle, cx), 0);

    let list = cx.read(|cx| view.read(cx).list().clone());
    cx.update(|cx| {
        list.update(cx, |list, cx| {
            list.sort_by(crate::issues::list::SortKey::Status, cx)
        })
    });
    frames_asked(handle, cx);
    assert!(frames_asked(handle, cx) > 0, "the wave runs");
    cx.executor().advance_clock(
        crate::shell::motion::Wave::Sort.total() + std::time::Duration::from_millis(50),
    );
    frames_asked(handle, cx);
    frames_asked(handle, cx);
    assert_eq!(frames_asked(handle, cx), 0, "the wave stops");
}

/// LLM-195: with animations on, the sidebar's column shrinks away when it
/// closes, so the view beside it moves smoothly, and then it goes.
#[gpui_kit::test]
fn the_sidebar_closes_smoothly_and_then_goes(cx: &mut TestAppContext) {
    let settings = UiSettings {
        sidebar_open: true,
        ..UiSettings::default()
    };
    let (handle, view) = open_with(settings, SettingsStore::memory(), cx);
    cx.update(|cx| crate::shell::motion::set(true, cx));
    frames_asked(handle, cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    frames_asked(handle, cx);
    // Where the sidebar's left edge is, or None when it is gone.
    let left = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("sidebar-all").map(|el| el.bounds().left())
        })
        .unwrap()
    };
    let at_rest = left(cx).expect("the sidebar shows");

    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| {
            this.toggle_sidebar(&crate::shell::workspace::ToggleSidebar, window, cx)
        })
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(60));
    let mid = left(cx).expect("the sidebar still shows while it closes");
    assert!(mid < at_rest, "it slides out to the left");

    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    frames_asked(handle, cx);
    frames_asked(handle, cx);
    assert_eq!(left(cx), None, "it goes when closed");
}

#[gpui_kit::test]
fn the_four_theme_cards_share_one_row_at_any_width(cx: &mut TestAppContext) {
    use crate::shell::theme::ColorTheme;
    let (handle, view) = open(cx);
    run(handle, &view, "settings", cx);

    for width in [1200., 700.] {
        cx.simulate_window_resize(
            handle,
            gpui_kit::size(gpui_kit::px(width), gpui_kit::px(800.)),
        );
        cx.run_until_parked();
        let cards: Vec<_> = cx
            .update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                ColorTheme::ALL
                    .map(|name| {
                        let id = format!("settings-theme-{}", name.id());
                        gpui_kit::base::test_support::find(window, &[], &id.into())
                            .expect("a card per theme")
                            .bounds()
                    })
                    .to_vec()
            })
            .unwrap();
        assert_eq!(cards.len(), 4);
        let first = cards[0];
        for (ix, card) in cards.iter().enumerate() {
            assert_eq!(
                card.origin.y, first.origin.y,
                "{width}: card {ix} is on the first row"
            );
            assert!(
                (card.size.width - first.size.width).abs() < gpui_kit::px(0.5),
                "{width}: card {ix} has the same width"
            );
            assert!(
                card.size.width > gpui_kit::px(40.),
                "{width}: card {ix} is wide enough"
            );
        }
        for pair in cards.windows(2) {
            assert!(
                pair[0].right() <= pair[1].origin.x,
                "{width}: cards do not overlap"
            );
        }
    }
}

/// LLM-344: j and k open a task each. `ui.json` keeps the open task once
/// the keys rest, not on every key.
#[gpui_kit::test]
fn the_open_task_saves_once_the_keys_rest(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui.json");
    let saved = || SettingsStore::at(path.clone()).load().last_task;
    let (handle, view) = open_with(UiSettings::default(), SettingsStore::at(path.clone()), cx);
    let rest = |cx: &mut TestAppContext| {
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
    };
    press(handle, &["enter"], cx);
    rest(cx);
    let first = cx.read(|cx| view.read(cx).open_task_key(cx)).unwrap();
    assert_eq!(saved().as_deref(), Some(&*first));

    press(handle, &["j", "j", "j"], cx);
    let last = cx.read(|cx| view.read(cx).open_task_key(cx)).unwrap();
    assert_ne!(first, last, "j opens the next task");
    assert_eq!(saved().as_deref(), Some(&*first), "no save on each key");
    rest(cx);
    assert_eq!(saved().as_deref(), Some(&*last));
}

/// Comes back to the window, as after a hand edit in another app.
fn refocus(handle: AnyWindowHandle, view: &Entity<Workspace>, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |this, cx| this.reload_config_file(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn config_note(view: &Entity<Workspace>, cx: &TestAppContext) -> Option<String> {
    cx.read(|cx| {
        view.read(cx)
            .settings_view()
            .read(cx)
            .config_note()
            .map(|n| n.to_string())
    })
}

#[gpui_kit::test]
fn a_read_only_config_keeps_the_change_for_this_run_and_shows_a_note(cx: &mut TestAppContext) {
    let (dir, store) = saved_store();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[app]\nvim_editor = true\n").unwrap();
    let mut perms = std::fs::metadata(&config).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&config, perms).unwrap();
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    run(handle, &view, "settings", cx);
    assert_eq!(config_note(&view, cx), None);

    click(handle, "settings-row-vim", cx);
    assert!(!cx.read(|cx| view.read(cx).current_settings(cx).vim_editor));
    assert_eq!(
        config_note(&view, cx).as_deref(),
        Some("Settings file is read-only. Changes last until you quit.")
    );
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "[app]\nvim_editor = true\n"
    );
}

#[gpui_kit::test]
fn a_hand_edit_applies_when_the_window_gets_focus(cx: &mut TestAppContext) {
    let (dir, store) = saved_store();
    let config = dir.path().join("config.toml");
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    run(handle, &view, "settings", cx);
    assert!(!config.exists(), "no file at start");

    std::fs::write(
        &config,
        "# by hand\n[app]\nmascot = false\nstrike_done = false\n",
    )
    .unwrap();
    refocus(handle, &view, cx);
    let now = cx.read(|cx| view.read(cx).current_settings(cx));
    assert!(!now.mascot);
    assert!(!now.strike_done);
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "# by hand\n[app]\nmascot = false\nstrike_done = false\n",
        "nothing written back"
    );

    std::fs::write(&config, "[app]\nmascot = true\nzoom = \n").unwrap();
    refocus(handle, &view, cx);
    assert!(
        !cx.read(|cx| view.read(cx).current_settings(cx).mascot),
        "last good"
    );
    let note = config_note(&view, cx).expect("a parse error");
    assert!(note.contains("line 3"), "{note}");
}

/// LLM-566: `[app.background.<slot>]` loads, a hand edit applies, and a
/// save of another key keeps the tables as they are.
#[gpui_kit::test]
fn background_tables_load_apply_and_are_never_written(cx: &mut TestAppContext) {
    use crate::shell::backdrop::BgSize;
    let (dir, store) = saved_store();
    let config = dir.path().join("config.toml");
    let text = "[app.background.window]\nimage = \"wall.png\"\nopacity = 0.3\n";
    std::fs::write(&config, text).unwrap();
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    let shown = |cx: &TestAppContext| cx.read(crate::shell::backdrop::shown);
    let window = shown(cx).window.expect("window image");
    assert_eq!(window.image, dir.path().join("wall.png"));
    assert_eq!(window.opacity, 0.3);

    run(handle, &view, "settings", cx);
    click(handle, "settings-row-mascot", cx);
    let after = std::fs::read_to_string(&config).unwrap();
    assert!(after.contains(text), "{after}");
    assert!(after.contains("mascot = false"), "{after}");

    std::fs::write(
        &config,
        "[app.background.task]\nimage = \"/t.png\"\nsize = \"auto\"\nposition = \"nowhere\"\n",
    )
    .unwrap();
    refocus(handle, &view, cx);
    let b = shown(cx);
    assert!(b.window.is_none());
    assert_eq!(b.task.unwrap().layout.size, BgSize::Scale(1.));
    let message = cx.read(|cx| view.read(cx).message().to_string());
    assert!(
        message.contains("unknown position \"nowhere\""),
        "{message}"
    );
}

/// LLM-568: a background file that changes on disk loads again on focus,
/// and a missing one is named once in the status bar.
#[gpui_kit::test]
fn a_changed_background_file_reloads_and_a_missing_one_is_named_once(cx: &mut TestAppContext) {
    let (dir, store) = saved_store();
    let wall = dir.path().join("wall.png");
    std::fs::write(&wall, b"one").unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[app.background.window]\nimage = \"wall.png\"\n[app.background.list]\nimage = \"gone.png\"\n",
    )
    .unwrap();
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    let shown = |cx: &TestAppContext| cx.read(crate::shell::backdrop::shown);
    let message = |cx: &TestAppContext| cx.read(|cx| view.read(cx).message().to_string());
    let before = shown(cx).window.unwrap().stamp;
    assert!(before.is_some());
    assert!(message(cx).contains("no file at"), "{}", message(cx));

    // The file changes; config.toml does not.
    let file = std::fs::File::options().write(true).open(&wall).unwrap();
    file.set_modified(before.unwrap() + std::time::Duration::from_secs(9))
        .unwrap();
    view.update(cx, |w, _| w.set_message("other".into()));
    refocus(handle, &view, cx);
    assert_ne!(shown(cx).window.unwrap().stamp, before);
    // Still missing: not named again.
    assert_eq!(message(cx), "other");

    std::fs::remove_file(&wall).unwrap();
    refocus(handle, &view, cx);
    assert!(shown(cx).window.unwrap().stamp.is_none());
    assert!(
        message(cx).contains("background.window: no file"),
        "{}",
        message(cx)
    );
}

/// LLM-571 and LLM-574: the image follows the mode, and the sidebar's
/// project picks its own table.
#[gpui_kit::test]
fn the_background_follows_the_mode_and_the_project(cx: &mut TestAppContext) {
    let (dir, store) = saved_store();
    std::fs::write(
        dir.path().join("config.toml"),
        "[app.background.window]\nimage = \"day.png\"\nimage_dark = \"night.png\"\n\
         [app.background.project.server.window]\nimage = \"server.png\"\n",
    )
    .unwrap();
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    let image = |cx: &TestAppContext| {
        cx.read(crate::shell::backdrop::shown)
            .window
            .map(|b| b.image.file_name().unwrap().to_string_lossy().into_owned())
    };
    run(handle, &view, "settings", cx);
    click(handle, "settings-appearance-light", cx);
    assert_eq!(image(cx).as_deref(), Some("day.png"));
    click(handle, "settings-appearance-dark", cx);
    assert_eq!(image(cx).as_deref(), Some("night.png"));

    view.update(cx, |w, cx| w.set_scope(Scope::Project("server".into()), cx));
    assert_eq!(image(cx).as_deref(), Some("server.png"));
    view.update(cx, |w, cx| w.set_scope(Scope::All, cx));
    assert_eq!(image(cx).as_deref(), Some("night.png"));
}

/// LLM-575: `:background` and `g b` hide and show the images, the View
/// menu follows, and the choice goes to ui-state.json, not config.toml.
#[gpui_kit::test]
fn background_images_hide_and_show_without_touching_config(cx: &mut TestAppContext) {
    use crate::shell::app_menus::MenuState;
    let (dir, store) = saved_store();
    let config = dir.path().join("config.toml");
    let text = "[app.background.window]\nimage = \"wall.png\"\n";
    std::fs::write(&config, text).unwrap();
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    let shown = |cx: &TestAppContext| cx.read(crate::shell::backdrop::shown).window.is_some();
    let menu = |cx: &TestAppContext| cx.read(|cx| *cx.global::<MenuState>());
    assert!(shown(cx));
    assert_eq!(
        menu(cx),
        MenuState {
            backgrounds_set: true,
            backgrounds_shown: true
        }
    );

    run(handle, &view, "background", cx);
    assert!(!shown(cx));
    assert!(!menu(cx).backgrounds_shown);
    assert!(!store.load().background_images);
    assert_eq!(std::fs::read_to_string(&config).unwrap(), text);

    press(handle, &["g", "b"], cx);
    assert!(shown(cx));
    assert!(store.load().background_images);

    // With no image set, the switch says so and stays.
    std::fs::write(&config, "").unwrap();
    refocus(handle, &view, cx);
    assert!(!menu(cx).backgrounds_set);
    run(handle, &view, "background", cx);
    assert!(store.load().background_images);
    let message = cx.read(|cx| view.read(cx).message().to_string());
    assert!(message.contains("No background image"), "{message}");
}

/// LLM-567: the Background section writes `[app.background.<slot>]` and
/// keeps the rest of the file as it was.
#[gpui_kit::test]
fn the_background_section_writes_each_slot(cx: &mut TestAppContext) {
    use crate::settings::BackgroundValue;
    use crate::shell::backdrop::Slot;
    use crate::shell::view_events::ViewEvent;
    let (dir, store) = saved_store();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "# my settings\n[app]\nmascot = true # keep\n").unwrap();
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    run(handle, &view, "settings", cx);
    let shown = |cx: &TestAppContext| cx.read(crate::shell::backdrop::shown);

    // Choose image… ends in this event once a file is picked.
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |w, cx| {
            w.on_view_event(
                ViewEvent::SetBackground {
                    slot: Slot::List,
                    key: "image",
                    value: Some(BackgroundValue::Text("/pics/a.png".into())),
                },
                window,
                cx,
            )
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(shown(cx).list.map(|b| b.image), Some("/pics/a.png".into()));

    let end = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            view.read(cx).settings_view().read(cx).scroll_to_end();
            window.refresh();
            window.render_frame(cx);
        })
        .unwrap();
    };
    end(cx);
    // Opacity: typed, then Enter.
    let settings = cx.read(|cx| view.read(cx).settings_view().clone());
    let editor = cx.read(|cx| settings.read(cx).backgrounds().clone());
    let field = cx.read(|cx| editor.read(cx).number(Slot::List, "opacity"));
    cx.update_window(handle, |_, window, cx| {
        field.update(cx, |f, cx| {
            f.set_value("50", window, cx);
            cx.emit(gpui_kit::component::input::InputEvent::PressEnter {
                secondary: false,
                shift: false,
            });
        });
    })
    .unwrap();
    cx.run_until_parked();
    // The dropdowns: a confirmed choice writes its value.
    for (key, value) in [
        ("repeat", "repeat"),
        ("size", "auto"),
        ("position", "left top"),
    ] {
        let select = cx.read(|cx| editor.read(cx).select(Slot::List, key));
        cx.update_window(handle, |_, _, cx| {
            select.update(cx, |_, cx| {
                cx.emit(gpui_kit::component::select::SelectEvent::Confirm(Some(
                    value,
                )))
            });
        })
        .unwrap();
        cx.run_until_parked();
    }
    end(cx);
    // The dropdown shows the saved choice: `left top` is "Top left".
    let position = cx.read(|cx| editor.read(cx).select(Slot::List, "position"));
    assert_eq!(
        cx.read(|cx| position.read(cx).selected_value().copied()),
        Some("top left")
    );
    assert_eq!(cx.read(|cx| field.read(cx).value().to_string()), "50");
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(
        text.starts_with("# my settings\n[app]\nmascot = true # keep\n"),
        "{text}"
    );
    assert!(text.contains("[app.background.list]"), "{text}");
    assert!(!text.contains("[app.background]\n"), "{text}");
    assert!(text.contains("position = \"left top\""), "{text}");
    let list = shown(cx).list.unwrap();
    assert_eq!(list.opacity, 0.5);
    assert!(list.layout.repeat.x && list.layout.repeat.y);
    assert_eq!(list.layout.size, crate::shell::backdrop::BgSize::Scale(1.));

    // + steps by 5.
    cx.update_window(handle, |_, _, cx| {
        field.update(cx, |_, cx| {
            cx.emit(gpui_kit::component::input::NumberInputEvent::Step(
                gpui_kit::component::input::StepAction::Increment,
            ))
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(shown(cx).list.unwrap().opacity, 0.55);
    let text = std::fs::read_to_string(&config).unwrap();
    // The app is in dark mode: the opacity is dark mode's (LLM-580).
    assert!(text.contains("opacity_dark = 0.55\n"), "{text}");
    assert_eq!(cx.read(|cx| field.read(cx).value().to_string()), "55");

    // A hand-written size with no entry leaves the dropdown on none.
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, text.replace("size = \"auto\"", "size = \"40%\"")).unwrap();
    refocus(handle, &view, cx);
    let size = cx.read(|cx| editor.read(cx).select(Slot::List, "size"));
    assert_eq!(cx.read(|cx| size.read(cx).selected_value().copied()), None);

    end(cx);
    click(handle, "settings-bg-list-remove", cx);
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(!text.contains("background"), "{text}");
    assert!(shown(cx).list.is_none());
}

/// LLM-578: a slot runs a shader; a broken one says its line, keeps the
/// last good program, and `:shader copy` writes a built-in out to edit.
#[gpui_kit::test]
fn shader_backgrounds_compile_report_errors_and_copy_out(cx: &mut TestAppContext) {
    use crate::shell::shader::{Programs, Source};
    let (dir, store) = saved_store();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        "[app.background.list]\nshader = \"builtin:waves\"\nfps = 500\n\
         [app.background.task]\nshader = \"mine.wgsl\"\n",
    )
    .unwrap();
    let mine = dir.path().join("mine.wgsl");
    std::fs::write(
        &mine,
        "fn main_image(f: vec2<f32>) -> vec4<f32> {\n  return u.accent;\n}\n",
    )
    .unwrap();
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    let shown = cx.read(crate::shell::backdrop::shown);
    let list = shown.list.expect("list shader");
    assert_eq!(list.shader.as_ref().unwrap().fps, 120.);
    let compiled = |source: Source, cx: &TestAppContext| {
        cx.read(|cx| cx.try_global::<Programs>().and_then(|p| p.get(&source)))
    };
    assert!(compiled(Source::Builtin("waves".into()), cx).is_some());
    let good = compiled(Source::File(mine.clone()), cx).expect("mine compiles");

    // A broken edit: the line shows, the old program stays.
    std::fs::write(
        &mine,
        "fn main_image(f: vec2<f32>) -> vec4<f32> {\n  return nope;\n}\n",
    )
    .unwrap();
    let file = std::fs::File::options().write(true).open(&mine).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    refocus(handle, &view, cx);
    let message = cx.read(|cx| view.read(cx).message().to_string());
    assert!(message.contains("background.task: shader"), "{message}");
    assert!(message.contains("line 2"), "{message}");
    let kept = compiled(Source::File(mine.clone()), cx).unwrap();
    assert!(std::sync::Arc::ptr_eq(&kept, &good));

    run(handle, &view, "shader copy aurora", cx);
    let copied = dir.path().join("shaders/aurora.wgsl");
    assert!(
        std::fs::read_to_string(&copied)
            .unwrap()
            .contains("main_image")
    );
    let message = cx.read(|cx| view.read(cx).message().to_string());
    assert!(message.contains("shaders/aurora.wgsl"), "{message}");
}

/// LLM-580: Edit live… leaves Settings and opens the floating panel; it
/// edits one slot at a time with the extra controls, and Esc closes it.
#[gpui_kit::test]
fn the_background_panel_edits_live_over_the_app(cx: &mut TestAppContext) {
    use crate::settings::BackgroundValue;
    use crate::shell::backdrop::Slot;
    use crate::shell::view_events::ViewEvent;
    let (dir, store) = saved_store();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[app.background.task]\nimage = \"/p/a.png\"\n").unwrap();
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    let shown = |cx: &TestAppContext| cx.read(crate::shell::backdrop::shown);
    let open = |cx: &TestAppContext| cx.read(|cx| view.read(cx).background_panel_open());

    run(handle, &view, "settings", cx);
    let settings = cx.read(|cx| view.read(cx).settings_view().clone());
    cx.update_window(handle, |_, window, cx| {
        settings.read(cx).scroll_to_end();
        window.refresh();
        window.render_frame(cx);
    })
    .unwrap();
    click(handle, "settings-bg-edit-live", cx);
    assert!(open(cx));
    assert!(
        !cx.read(|cx| view.read(cx).settings_shown(cx)),
        "Settings closed"
    );

    // It opens centred over the dimmed app.
    let (panel_box, view_size) = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("bg-panel-scrim").is_some(), "dimmed");
            (window.find("bg-panel").bounds(), window.viewport_size())
        })
        .unwrap();
    let off = panel_box.center() - gpui_kit::point(view_size.width / 2., view_size.height / 2.);
    assert!(
        f32::from(off.x).abs() < 2. && f32::from(off.y).abs() < 2.,
        "{off:?}"
    );
    assert_eq!(cx.read(|cx| view.read(cx).background_panel_at()), None);

    // A drag by the header moves it, and the dimming goes.
    let header = cx
        .update_window(handle, |_, window, _| {
            window.find("bg-panel-header").bounds()
        })
        .unwrap();
    let from = header.center();
    let to = from - gpui_kit::point(gpui_kit::px(150.), gpui_kit::px(80.));
    cx.update_window(handle, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    cx.run_until_parked();
    let moved = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("bg-panel-scrim").is_none(),
                "no longer dimmed"
            );
            window.find("bg-panel").bounds()
        })
        .unwrap();
    let shift = moved.origin - panel_box.origin;
    assert!(
        (f32::from(shift.x) + 150.).abs() < 2. && (f32::from(shift.y) + 80.).abs() < 2.,
        "{shift:?}"
    );

    // The slot picker, then a panel-only control.
    click(handle, "bg-panel-slot-task", cx);
    let panel = cx.read(|cx| view.read(cx).background_panel().clone());
    assert_eq!(cx.read(|cx| panel.read(cx).slot()), Slot::Task);
    let blur = cx.read(|cx| panel.read(cx).number(Slot::Task, "blur"));
    cx.update_window(handle, |_, _, cx| {
        blur.update(cx, |_, cx| {
            cx.emit(gpui_kit::component::input::NumberInputEvent::Step(
                gpui_kit::component::input::StepAction::Increment,
            ))
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(shown(cx).task.unwrap().effects.blur, 2.);
    // With the field focused, as after a click on +, a step still shows.
    cx.update_window(handle, |_, window, cx| {
        let focus = gpui_kit::Focusable::focus_handle(blur.read(cx), cx);
        window.focus(&focus, cx);
        blur.update(cx, |_, cx| {
            cx.emit(gpui_kit::component::input::NumberInputEvent::Step(
                gpui_kit::component::input::StepAction::Increment,
            ))
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(shown(cx).task.unwrap().effects.blur, 4.);
    assert_eq!(cx.read(|cx| blur.read(cx).value().to_string()), "4");
    click(handle, "settings-bg-task-grayscale", cx);
    assert!(shown(cx).task.unwrap().grayscale);
    let tint = cx.read(|cx| panel.read(cx).select(Slot::Task, "tint"));
    cx.update_window(handle, |_, _, cx| {
        tint.update(cx, |_, cx| {
            cx.emit(gpui_kit::component::select::SelectEvent::Confirm(Some(
                "#000000",
            )))
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(shown(cx).task.unwrap().tint.is_some());
    // The scroll choice shows for the task panel.
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |w, cx| {
            w.on_view_event(
                ViewEvent::SetBackground {
                    slot: Slot::Task,
                    key: "attachment",
                    value: Some(BackgroundValue::Text("scroll".into())),
                },
                window,
                cx,
            )
        })
    })
    .unwrap();
    assert!(shown(cx).task.unwrap().scroll);
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("grayscale = true"), "{text}");
    assert!(text.contains("tint = \"#000000\""), "{text}");

    press(handle, &["escape"], cx);
    assert!(!open(cx));
    run(handle, &view, "background edit", cx);
    assert!(open(cx));
    assert_eq!(
        cx.read(|cx| view.read(cx).background_panel_at()),
        None,
        "centred again"
    );
    // The first change lifts the dimming, and the panel stays put.
    let before = cx
        .update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("bg-panel-scrim").is_some());
            window.find("bg-panel").bounds()
        })
        .unwrap();
    cx.update_window(handle, |_, window, cx| {
        view.update(cx, |w, cx| {
            w.on_view_event(
                ViewEvent::SetBackground {
                    slot: Slot::Task,
                    key: "repeat",
                    value: Some(BackgroundValue::Text("repeat".into())),
                },
                window,
                cx,
            )
        })
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("bg-panel-scrim").is_none(),
            "no longer dimmed"
        );
        assert_eq!(window.find("bg-panel").bounds().origin, before.origin);
    })
    .unwrap();
    click(handle, "bg-panel-close", cx);
    assert!(!open(cx));
}

/// LLM-580: an image set in one mode belongs to that mode. The other mode
/// keeps its own, or none, and can get another. A plain `image` from
/// before stays with the other mode.
#[gpui_kit::test]
fn each_mode_keeps_its_own_background(cx: &mut TestAppContext) {
    use crate::settings::BackgroundValue;
    use crate::shell::backdrop::Slot;
    use crate::shell::view_events::ViewEvent;
    let (dir, store) = saved_store();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        "[app.background.window]\nimage = \"/p/both.png\"\n",
    )
    .unwrap();
    let (handle, view) = open_with(store.load(), store.clone(), cx);
    let image = |cx: &TestAppContext| {
        cx.read(crate::shell::backdrop::shown)
            .window
            .map(|b| b.image.to_string_lossy().into_owned())
    };
    let set = |value: Option<&str>, cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |w, cx| {
                w.on_view_event(
                    ViewEvent::SetBackground {
                        slot: Slot::Window,
                        key: "image",
                        value: value.map(|v| BackgroundValue::Text(v.into())),
                    },
                    window,
                    cx,
                )
            })
        })
        .unwrap();
        cx.run_until_parked();
    };
    run(handle, &view, "settings", cx);
    click(handle, "settings-appearance-dark", cx);
    set(Some("/p/night.png"), cx);
    assert_eq!(image(cx).as_deref(), Some("/p/night.png"));
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(text.contains("image_dark = \"/p/night.png\""), "{text}");
    assert!(text.contains("image_light = \"/p/both.png\""), "{text}");
    assert!(!text.contains("\nimage ="), "{text}");

    // Light mode kept the old image, and takes its own.
    click(handle, "settings-appearance-light", cx);
    assert_eq!(image(cx).as_deref(), Some("/p/both.png"));
    set(Some("/p/day.png"), cx);
    assert_eq!(image(cx).as_deref(), Some("/p/day.png"));
    // Remove in light mode: light shows none, dark keeps its image.
    set(None, cx);
    assert_eq!(image(cx), None);
    click(handle, "settings-appearance-dark", cx);
    assert_eq!(image(cx).as_deref(), Some("/p/night.png"));
    // Remove in dark too: the table goes.
    set(None, cx);
    let text = std::fs::read_to_string(&config).unwrap();
    assert!(!text.contains("background"), "{text}");
}
