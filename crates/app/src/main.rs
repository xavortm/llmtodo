//! llmtodo desktop app. It opens the window on the views and the model in
//! `llmtodo-ui`, which talks to the local API server through
//! `model/backend.rs`. SPIKE.md records how the GPUI parts work.

use gpui_kit::assets::AllAssets;
use gpui_kit::component::{ThemeMode, TitleBar};
use gpui_kit::{AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};

use llmtodo_ui::model::{backend, drafts, trackers, update};
use llmtodo_ui::settings;
use llmtodo_ui::shell::workspace::{self, Workspace};
use llmtodo_ui::shell::{app_icon, app_menus, theme};

fn main() {
    // AllAssets bundles every Lucide icon, so the app can draw any project
    // icon an agent picks. It adds 0.9 MiB to the release binary (LLM-196).
    gpui_kit::application().with_assets(AllAssets).run(|cx| {
        llmtodo_ui::init(cx);
        // GPUI loads remote images in the Markdown preview through this.
        match reqwest_client::ReqwestClient::user_agent("llmtodo") {
            Ok(client) => cx.set_http_client(std::sync::Arc::new(client)),
            Err(err) => eprintln!("llmtodo: no HTTP client for images: {err}"),
        }
        app_icon::set_dock_icon();
        let store = settings::SettingsStore::default_location();
        let saved = store.load();
        let mode = if saved.theme == "light" {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        let color_theme = theme::ColorTheme::from_id(&saved.color_theme).unwrap_or_default();
        theme::set_theme(color_theme, mode, cx);
        app_menus::set_menus(cx);
        // Reopen where the window was, at its last size.
        let bounds = match saved.window {
            Some([x, y, w, h]) if w >= 480. && h >= 320. => {
                Bounds::new(gpui_kit::point(px(x), px(y)), size(px(w), px(h)))
            }
            _ => Bounds::centered(None, size(px(1200.), px(760.)), cx),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..TitleBar::window_options()
        };
        // The API runs on its own tokio thread; see backend.rs.
        let target = backend::Target::from_env();
        let trackers = trackers::from_env(&target.data_dir);
        let drafts = drafts::DraftStore::open(&target.data_dir);
        let update = update::UpdateWatch::from_env(&target.data_dir);
        let update_dir = target.data_dir.clone();
        let update_cli = target.spawn.clone();
        let (backend, events) = backend::Backend::start(target.clone());
        // Another tracker gets its own API thread, which starts its server
        // with the same binary when none answers. A read-only tracker never
        // starts one.
        let starter: workspace::Starter = std::sync::Arc::new(move |tracker| {
            backend::Backend::start(backend::Target {
                data_dir: tracker.data_dir.clone(),
                spawn: target.spawn.clone(),
                read_only: tracker.read_only,
            })
        });
        gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| {
                let mut workspace = Workspace::with_settings(saved, store, window, cx);
                workspace.set_trackers(trackers, starter, cx);
                workspace.set_drafts(drafts);
                workspace.connect(backend, events, cx);
                if let Some(watch) = update {
                    workspace.watch_for_updates(watch, update_dir, update_cli, cx);
                }
                workspace
            })
        })
        .expect("open main window");
        cx.activate(true);
    });
}
