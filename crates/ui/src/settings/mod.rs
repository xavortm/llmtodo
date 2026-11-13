//! UI settings that survive a restart: theme, sort, grouping, collapsed
//! groups, sidebar, pane sizes, open tabs, the window frame and the zoom.
//!
//! They live in `ui.json` under `LLMTODO_CONFIG_DIR`, or the platform config
//! folder: `~/Library/Application Support/llmtodo` on macOS,
//! `$XDG_CONFIG_HOME/llmtodo` or `~/.config/llmtodo` elsewhere. The app saves
//! after every change, so a crash loses nothing. The window frame waits until
//! the window stops moving, and goes out at quit. Unknown or missing fields
//! fall back to defaults, so old files keep loading.

pub mod view;

#[cfg(test)]
mod settings_tests;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// `dark` or `light`.
    pub theme: String,
    /// The color theme's id, such as `default` or `gruvbox`. See
    /// `shell::theme::ColorTheme`.
    pub color_theme: String,
    /// `key`, `status` or `added`.
    pub sort: String,
    pub sort_reversed: bool,
    /// `none`, `status`, `label` or `assignee`.
    pub group: String,
    /// Collapsed groups, as `grouping:title`, for example `status:Done`.
    pub collapsed_groups: Vec<String>,
    pub sidebar_open: bool,
    /// What the sidebar picked: `all`, `team:<key>` or `project:<slug>`.
    pub sidebar_scope: String,
    /// Task view width beside the list, in pixels.
    pub split_side: Option<f32>,
    /// Task view height under the list, in pixels.
    pub split_stacked: Option<f32>,
    /// Open tab per task key.
    pub open_tabs: BTreeMap<String, usize>,
    /// Task view panels the owner dragged to a height, in rem, by panel
    /// name: `review` or `git`. A missing panel follows its content.
    pub panel_heights: BTreeMap<String, f32>,
    /// Window origin and size: x, y, width, height.
    pub window: Option<[f32; 4]>,
    /// UI zoom, 1.0 for actual size.
    pub zoom: f32,
    /// Hide closed tasks once they are `hide_closed_days` old.
    pub hide_closed: bool,
    pub hide_closed_days: u32,
    /// Vim keys in the description editor.
    pub vim_editor: bool,
    /// Reopen the last view and task at start, or always All issues.
    pub open_last_view: bool,
    /// The key of the task open when the app last ran.
    pub last_task: Option<String>,
    /// Columns the owner hid in the issue list, by `Column::setting` name.
    pub hidden_columns: Vec<String>,
    /// Sudo, the pixel cat under the sidebar.
    pub mascot: bool,
    /// Experimental: a light line through the titles of Done tasks in the
    /// task list.
    pub strike_done: bool,
}

/// The largest value the "Hide after" field takes, ten years.
pub const MAX_HIDE_DAYS: u32 = 3650;

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            color_theme: "default".into(),
            sort: "key".into(),
            sort_reversed: false,
            group: "none".into(),
            collapsed_groups: Vec::new(),
            sidebar_open: false,
            sidebar_scope: "all".into(),
            split_side: None,
            split_stacked: None,
            open_tabs: BTreeMap::new(),
            panel_heights: BTreeMap::new(),
            window: None,
            zoom: 1.0,
            hide_closed: false,
            hide_closed_days: 14,
            vim_editor: true,
            open_last_view: true,
            last_task: None,
            hidden_columns: Vec::new(),
            mascot: true,
            strike_done: true,
        }
    }
}

/// Where settings are read and written. `None` keeps them in memory only,
/// which tests use.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: Option<PathBuf>,
}

impl SettingsStore {
    pub fn memory() -> Self {
        Self { path: None }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn at(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// The default location, see the module docs.
    pub fn default_location() -> Self {
        let dir = std::env::var_os("LLMTODO_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(platform_config_dir);
        Self {
            path: dir.map(|d| d.join("ui.json")),
        }
    }

    /// Reads the settings. A missing or broken file gives the defaults, so
    /// the app always starts.
    pub fn load(&self) -> UiSettings {
        self.path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the settings. It writes a temp file and renames it, so a crash
    /// mid-write never leaves a half file.
    pub fn save(&self, settings: &UiSettings) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
        std::fs::rename(tmp, path)
    }
}

fn platform_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    if cfg!(target_os = "macos") {
        return Some(home.join("Library/Application Support/llmtodo"));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    Some(base.join("llmtodo"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_survives_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::at(dir.path().join("ui.json"));
        assert_eq!(store.load(), UiSettings::default(), "missing file");

        let settings = UiSettings {
            group: "status".into(),
            collapsed_groups: vec!["status:Done".into()],
            split_side: Some(612.),
            hidden_columns: vec!["labels".into(), "cost".into()],
            color_theme: "gruvbox".into(),
            ..Default::default()
        };
        store.save(&settings).unwrap();
        assert_eq!(store.load(), settings);

        std::fs::write(dir.path().join("ui.json"), "{ not json").unwrap();
        assert_eq!(store.load(), UiSettings::default(), "broken file");

        // Old files without newer fields still load.
        std::fs::write(dir.path().join("ui.json"), r#"{"group":"label"}"#).unwrap();
        assert_eq!(store.load().group, "label");
        assert_eq!(store.load().theme, "dark");
        assert_eq!(store.load().color_theme, "default");
        assert!(!store.load().hide_closed);
        assert_eq!(store.load().hide_closed_days, 14);
        assert!(store.load().vim_editor);
        assert!(store.load().open_last_view);
        assert!(store.load().hidden_columns.is_empty());
        assert!(store.load().mascot, "the cat is on for old files");
        assert!(store.load().strike_done, "the line is on for old files");
    }

    #[test]
    fn the_cat_is_on_by_default() {
        assert!(UiSettings::default().mascot);
    }
}
