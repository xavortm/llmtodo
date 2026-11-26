//! The shortcuts every macOS app has, and the native menu bar that lists
//! them.
//!
//! The keys bind with no context, so they work wherever focus is. Text
//! fields and the editor already bind the edit keys, Cmd-C, Cmd-X, Cmd-V,
//! Cmd-A, Cmd-Z and Cmd-Shift-Z, under GPUI Kit's `Input` context. The Edit
//! menu sends those same actions to the focused field.
//!
//! App actions, such as Quit and Hide, run here as global handlers. Window
//! actions and zoom need the workspace, so `Workspace` handles them: zoom
//! saves to the settings, and Close Window saves before the window goes.
//!
//! macOS menus show the first binding of each action as its key. The app's
//! own commands bind plain vim keys, such as `c` for a new issue, and
//! a plain key in the menu could fire while the owner types. So the menu
//! runs them through `Menu*` actions that have no binding.

use gpui_kit::component::input;
use gpui_kit::{App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, actions};

use crate::shell::workspace;

actions!(
    app,
    [
        Quit,
        CloseWindow,
        Minimize,
        ToggleFullScreen,
        Hide,
        HideOthers,
        ShowAll,
        OpenSettings,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        MenuNewIssue,
        MenuPalette,
        MenuFilter,
        MenuClaims,
        MenuActivityStrip,
        MenuBackgrounds,
        MenuEditBackgrounds,
        MenuHelp,
        MenuCheckForUpdates,
    ]
);

pub const APP_NAME: &str = "llmtodo";

/// Every app-wide binding. Zoom also takes Ctrl, as the owner asked.
pub fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-ctrl-f", ToggleFullScreen, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-=", ZoomIn, None),
        KeyBinding::new("cmd-+", ZoomIn, None),
        KeyBinding::new("cmd--", ZoomOut, None),
        KeyBinding::new("cmd-0", ZoomReset, None),
        // LLM-582: hide or show background images from anywhere, as for a
        // screen share. The View menu shows it.
        KeyBinding::new("cmd-shift-b", MenuBackgrounds, None),
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-0", ZoomReset, None),
    ]
}

/// Binds the keys and registers the app actions. `main::init` calls it, so
/// tests get the same keys.
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    // A single-window app quits when its window closes, by key or by the
    // red button.
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
}

/// What the menu bar shows that changes while the app runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuState {
    /// LLM-575: `config.toml` names a background image.
    pub backgrounds_set: bool,
    /// The owner shows background images.
    pub backgrounds_shown: bool,
}

impl gpui_kit::Global for MenuState {}

/// The menu bar: App, Edit, Issue, View and Window.
pub fn menus() -> Vec<Menu> {
    menus_for(MenuState::default())
}

/// The menu bar for `state`.
pub fn menus_for(state: MenuState) -> Vec<Menu> {
    vec![
        Menu::new(APP_NAME).items([
            MenuItem::action("Check for Updates…", MenuCheckForUpdates),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(format!("Hide {APP_NAME}"), Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action(format!("Quit {APP_NAME}"), Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Issue").items([
            MenuItem::action("New Issue", MenuNewIssue),
            MenuItem::action("Save", workspace::Save),
            MenuItem::separator(),
            MenuItem::action("Filter Issues", MenuFilter),
            MenuItem::action("Command Palette", MenuPalette),
            MenuItem::action("Claims", MenuClaims),
        ]),
        Menu::new("View").items([
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Actual Size", ZoomReset),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", workspace::ToggleSidebar),
            MenuItem::action("Toggle Activity Strip", MenuActivityStrip),
            MenuItem::action("Show Background Images", MenuBackgrounds)
                .checked(state.backgrounds_set && state.backgrounds_shown)
                .disabled(!state.backgrounds_set),
            // LLM-581: the floating Backgrounds panel. Always on: it is
            // also how a first image gets set.
            MenuItem::action("Edit Backgrounds…", MenuEditBackgrounds),
            MenuItem::action("Keyboard Shortcuts", MenuHelp),
            MenuItem::separator(),
            MenuItem::action("Toggle Full Screen", ToggleFullScreen),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Close Window", CloseWindow),
        ]),
    ]
}

/// Puts the menu bar up. Call it after `init`, so the menus show the keys.
pub fn set_menus(cx: &mut App) {
    let state = cx.try_global::<MenuState>().copied().unwrap_or_default();
    cx.set_menus(menus_for(state));
}

/// Puts the menu bar up again when `state` changed.
pub fn update_state(state: MenuState, cx: &mut App) {
    if cx.try_global::<MenuState>() == Some(&state) {
        return;
    }
    cx.set_global(state);
    set_menus(cx);
}
