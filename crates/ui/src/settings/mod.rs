//! UI settings that survive a restart. They live in two files:
//!
//! - **Preferences** go to `config.toml`, in its `[app]` table: theme,
//!   color theme, Vim keys, the cat, animations, the line through done
//!   tasks, hidden columns, hiding closed tasks, opening the last view and
//!   the zoom. A person can edit the file by hand and keep it in git.
//!   `llmtodo_client::config` finds the file and writes it in place.
//! - **State** goes to `ui-state.json`: the window frame, pane sizes, open
//!   tabs, the last task, the sidebar, sort and grouping. It changes every
//!   few seconds, so it stays out of git. It sits in `LLMTODO_CONFIG_DIR`
//!   when that is set, as for previews, and in the standard data folder,
//!   `~/Library/Application Support/llmtodo`, otherwise.
//!
//! The rules for `config.toml`:
//!
//! - No file is fine. The app does not create it at start, and writes only
//!   the keys the person changes.
//! - A save changes only the keys that changed, and keeps comments, key
//!   order, unknown keys and symlinks.
//! - A read-only file stays as it is. The change lasts until quit, and
//!   Settings says so once.
//! - A hand edit applies while the app runs: the workspace checks the
//!   file's modification time when the window gets focus. A file that does
//!   not parse keeps the last good settings, Settings shows the error, and
//!   the app never writes over it.
//!
//! The first start after an upgrade moves an old `ui.json` over: its state
//! to `ui-state.json`, and its non-default preferences to `config.toml`
//! when that file has no `[app]` table yet. `ui.json` stays, so an older build still
//! starts.
//!
//! The app saves after every change, so a crash loses nothing. The window
//! frame waits until the window stops moving, and goes out at quit.
//! Unknown or missing fields fall back to defaults, so old files keep
//! loading.

pub mod view;

#[cfg(test)]
mod settings_tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use llmtodo_client::config::{self, ConfigFile, ConfigRead, ParseError, WriteError, toml_edit};
use llmtodo_client::data_dir::standard_data_dir;
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
    /// LLM-560: the strip of tasks created and completed per day under a
    /// project's list. Shown unless the owner hides it.
    pub activity_strip: bool,
    /// LLM-575: background images show. The owner hides them with View >
    /// Show Background Images, `g b` or `:background`, as for a screen
    /// share. It never changes `config.toml`.
    pub background_images: bool,
    /// What the sidebar picked: `all`, `team:<key>` or `project:<slug>`.
    pub sidebar_scope: String,
    /// The teams whose groups the sidebar shows closed, by team key.
    pub sidebar_collapsed: Vec<String>,
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
    /// Hide closed tasks once they are `hide_closed_days` old. 0 hides them
    /// at once. A closed task that an open task waits for stays.
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
    /// Short animations for actions: panels, overlays, hover and press
    /// (LLM-195). Off, the app changes state at once.
    pub animations: bool,
    /// LLM-566: background images, `[app.background.<slot>]` in
    /// `config.toml`. A settings save never writes them; the Background
    /// section writes one key at a time (LLM-567).
    #[serde(skip)]
    pub background: crate::shell::backdrop::BackgroundsConfig,
}

/// A value Settings writes to a background table (LLM-567).
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundValue {
    Text(String),
    Number(f32),
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
            activity_strip: true,
            background_images: true,
            sidebar_scope: "all".into(),
            sidebar_collapsed: Vec::new(),
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
            animations: false,
            background: Default::default(),
        }
    }
}

/// The preferences part of [`UiSettings`], the `[app]` table of
/// `config.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    theme: String,
    color_theme: String,
    vim_editor: bool,
    mascot: bool,
    animations: bool,
    strike_done: bool,
    hidden_columns: Vec<String>,
    hide_closed: bool,
    hide_closed_days: u32,
    open_last_view: bool,
    zoom: f32,
    /// Read only: a save never writes it, see `changed_since`.
    #[serde(skip_serializing)]
    background: crate::shell::backdrop::BackgroundsConfig,
}

/// The state part of [`UiSettings`], `ui-state.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct UiState {
    window: Option<[f32; 4]>,
    split_side: Option<f32>,
    split_stacked: Option<f32>,
    panel_heights: BTreeMap<String, f32>,
    open_tabs: BTreeMap<String, usize>,
    last_task: Option<String>,
    sidebar_open: bool,
    activity_strip: bool,
    background_images: bool,
    sidebar_scope: String,
    sidebar_collapsed: Vec<String>,
    sort: String,
    sort_reversed: bool,
    group: String,
    collapsed_groups: Vec<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self::of(&UiSettings::default())
    }
}

impl Default for UiState {
    fn default() -> Self {
        Self::of(&UiSettings::default())
    }
}

impl Preferences {
    fn of(s: &UiSettings) -> Self {
        Self {
            theme: s.theme.clone(),
            color_theme: s.color_theme.clone(),
            vim_editor: s.vim_editor,
            mascot: s.mascot,
            animations: s.animations,
            strike_done: s.strike_done,
            hidden_columns: s.hidden_columns.clone(),
            hide_closed: s.hide_closed,
            hide_closed_days: s.hide_closed_days,
            open_last_view: s.open_last_view,
            zoom: s.zoom,
            background: s.background.clone(),
        }
    }

    fn apply_to(self, s: &mut UiSettings) {
        s.theme = self.theme;
        s.color_theme = self.color_theme;
        s.vim_editor = self.vim_editor;
        s.mascot = self.mascot;
        s.animations = self.animations;
        s.strike_done = self.strike_done;
        s.hidden_columns = self.hidden_columns;
        s.hide_closed = self.hide_closed;
        s.hide_closed_days = self.hide_closed_days;
        s.open_last_view = self.open_last_view;
        s.zoom = self.zoom;
        s.background = self.background;
    }

    /// The keys, as TOML values, in field order.
    fn values(&self) -> toml_edit::Table {
        let text = toml::to_string(self).expect("preferences serialize");
        let doc: toml_edit::DocumentMut = text.parse().expect("toml reads its own output");
        doc.as_table().clone()
    }

    /// The keys whose value differs from `other`, with the value in `self`.
    fn changed_since(&self, other: &Self) -> Vec<(String, toml_edit::Value)> {
        let (new, old) = (self.values(), other.values());
        new.iter()
            .filter_map(|(key, item)| {
                let value = item.as_value()?;
                let before = old.get(key).and_then(|i| i.as_value());
                let same = before.is_some_and(|b| b.to_string().trim() == value.to_string().trim());
                (!same).then(|| (key.to_string(), value.clone()))
            })
            .collect()
    }
}

impl UiState {
    fn of(s: &UiSettings) -> Self {
        Self {
            window: s.window,
            split_side: s.split_side,
            split_stacked: s.split_stacked,
            panel_heights: s.panel_heights.clone(),
            open_tabs: s.open_tabs.clone(),
            last_task: s.last_task.clone(),
            sidebar_open: s.sidebar_open,
            activity_strip: s.activity_strip,
            background_images: s.background_images,
            sidebar_scope: s.sidebar_scope.clone(),
            sidebar_collapsed: s.sidebar_collapsed.clone(),
            sort: s.sort.clone(),
            sort_reversed: s.sort_reversed,
            group: s.group.clone(),
            collapsed_groups: s.collapsed_groups.clone(),
        }
    }

    fn apply_to(self, s: &mut UiSettings) {
        s.window = self.window;
        s.split_side = self.split_side;
        s.split_stacked = self.split_stacked;
        s.panel_heights = self.panel_heights;
        s.open_tabs = self.open_tabs;
        s.last_task = self.last_task;
        s.sidebar_open = self.sidebar_open;
        s.activity_strip = self.activity_strip;
        s.background_images = self.background_images;
        s.sidebar_scope = self.sidebar_scope;
        s.sidebar_collapsed = self.sidebar_collapsed;
        s.sort = self.sort;
        s.sort_reversed = self.sort_reversed;
        s.group = self.group;
        s.collapsed_groups = self.collapsed_groups;
    }
}

/// The shape of `config.toml` that the app reads.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AppFile {
    app: Preferences,
}

/// The table of `config.toml` that holds the app's preferences.
const APP_TABLE: &str = "app";

/// Why a preference change only lasts until quit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigProblem {
    ReadOnly,
    Invalid(ParseError),
}

impl ConfigProblem {
    /// The quiet note Settings shows.
    pub fn note(&self) -> String {
        match self {
            Self::ReadOnly => "Settings file is read-only. Changes last until you quit.".into(),
            Self::Invalid(err) => format!(
                "Settings file does not parse, {err}. The app keeps the last good settings and does not write the file."
            ),
        }
    }
}

/// Where settings are read and written. `memory` keeps them in memory
/// only, which tests use.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    config: Option<ConfigFile>,
    state: Option<PathBuf>,
    /// The old `ui.json`, moved over at the first start.
    legacy: Option<PathBuf>,
    /// Set after a write failed or the file did not parse. Cleared when
    /// the file changes on disk.
    problem: Option<ConfigProblem>,
    /// The file's modification time at the last read or write.
    seen: Option<SystemTime>,
    /// False until the first `check_file`.
    checked: bool,
}

impl SettingsStore {
    pub fn memory() -> Self {
        Self {
            config: None,
            state: None,
            legacy: None,
            problem: None,
            seen: None,
            checked: false,
        }
    }

    /// State in `path`, and `config.toml` beside it. Tests use it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn at(path: PathBuf) -> Self {
        Self {
            config: Some(ConfigFile::at(path.with_file_name(config::CONFIG_FILE))),
            state: Some(path),
            ..Self::memory()
        }
    }

    /// The default locations, see the module docs.
    pub fn default_location() -> Self {
        let env = std::env::var_os(config::CONFIG_DIR_ENV)
            .filter(|d| !d.is_empty())
            .map(PathBuf::from);
        let state_dir = env.clone().unwrap_or_else(standard_data_dir);
        Self {
            config: ConfigFile::default_location(),
            state: Some(state_dir.join("ui-state.json")),
            legacy: env.or_else(legacy_dir).map(|d| d.join("ui.json")),
            problem: None,
            seen: None,
            checked: false,
        }
    }

    /// The path of `config.toml`, if the store has one.
    pub fn config_path(&self) -> Option<&Path> {
        self.config.as_ref().map(ConfigFile::path)
    }

    /// Why changes do not reach `config.toml` right now, if they don't.
    pub fn problem(&self) -> Option<&ConfigProblem> {
        self.problem.as_ref()
    }

    /// Reads the settings. A missing or broken file gives the defaults, so
    /// the app always starts. The first start moves `ui.json` over.
    pub fn load(&self) -> UiSettings {
        self.migrate();
        let mut settings = UiSettings::default();
        if let Some(state) = self
            .state
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<UiState>(&text).ok())
        {
            state.apply_to(&mut settings);
        }
        if let Some(Ok(prefs)) = self.read_preferences() {
            prefs.apply_to(&mut settings);
        }
        settings
    }

    /// `None` without a file, `Err` for a file that does not parse.
    fn read_preferences(&self) -> Option<Result<Preferences, ParseError>> {
        match self.config.as_ref()?.read() {
            ConfigRead::Missing => None,
            ConfigRead::Parsed(doc) => Some(config::from_doc::<AppFile>(&doc).map(|f| f.app)),
            ConfigRead::Invalid(err) => Some(Err(err)),
        }
    }

    /// Checks `config.toml` for a change made outside the app since the
    /// last check. Returns the preferences when they changed and parse.
    /// A file that does not parse returns `None` and sets the problem.
    /// The first call only notes the file's time and any parse error.
    pub fn check_file(&mut self) -> Option<UiPreferences> {
        let config = self.config.as_ref()?;
        let modified = config.modified();
        let first = !self.checked;
        self.checked = true;
        if !first && modified == self.seen {
            return None;
        }
        self.seen = modified;
        self.problem = None;
        let prefs = match self.read_preferences() {
            None => Preferences::default(),
            Some(Ok(prefs)) => prefs,
            Some(Err(err)) => {
                self.problem = Some(ConfigProblem::Invalid(err));
                return None;
            }
        };
        (!first).then_some(UiPreferences(prefs))
    }

    /// Writes what changed from `old` to `new`: state to `ui-state.json`,
    /// and each changed preference to `config.toml`. It writes a temp file
    /// and renames it, so a crash mid-write never leaves a half file.
    pub fn save(&mut self, old: &UiSettings, new: &UiSettings) -> std::io::Result<()> {
        let state = UiState::of(new);
        if state != UiState::of(old)
            && let Some(path) = &self.state
        {
            write_json(path, &state)?;
        }
        let changed = Preferences::of(new).changed_since(&Preferences::of(old));
        let Some(config) = &self.config else {
            return Ok(());
        };
        if changed.is_empty() || self.problem.is_some() {
            return Ok(());
        }
        let result = config.update(|doc| {
            let app = config::table_mut(doc, APP_TABLE);
            for (key, value) in changed {
                config::set_value(app, &key, value);
            }
        });
        match result {
            Ok(()) => {
                self.seen = config.modified();
                Ok(())
            }
            Err(WriteError::Invalid(err)) => {
                self.problem = Some(ConfigProblem::Invalid(err));
                Ok(())
            }
            Err(err) if err.is_read_only() => {
                self.problem = Some(ConfigProblem::ReadOnly);
                Ok(())
            }
            Err(WriteError::Io(err)) => Err(err),
        }
    }

    /// LLM-567: writes one key of `[app.background.<slot>]`, or takes it
    /// out with `None`. Taking out `image` takes out the whole table. The
    /// rest of the file, comments too, stays as it is. Returns the
    /// background tables as the file now holds them.
    pub fn set_background(
        &mut self,
        slot: &str,
        key: &str,
        value: Option<BackgroundValue>,
    ) -> std::io::Result<Option<crate::shell::backdrop::BackgroundsConfig>> {
        let Some(config) = &self.config else {
            return Ok(None);
        };
        if self.problem.is_some() {
            return Ok(None);
        }
        let result = config.update(|doc| {
            let app = config::table_mut(doc, APP_TABLE);
            if !app.get("background").is_some_and(toml_edit::Item::is_table) {
                let mut table = toml_edit::Table::new();
                // `[app.background.window]` needs no `[app.background]`.
                table.set_implicit(true);
                app["background"] = toml_edit::Item::Table(table);
            }
            let background = app["background"].as_table_mut().expect("a table");
            match value {
                None if key == "image" => {
                    background.remove(slot);
                }
                None => {
                    if let Some(t) = background.get_mut(slot).and_then(|t| t.as_table_mut()) {
                        t.remove(key);
                    }
                }
                Some(value) => {
                    if !background.get(slot).is_some_and(toml_edit::Item::is_table) {
                        background[slot] = toml_edit::table();
                    }
                    let table = background[slot].as_table_mut().expect("a table");
                    let value = match value {
                        BackgroundValue::Text(t) => toml_edit::Value::from(t),
                        // f32 to f64 adds noise: 0.35 would read 0.3499999940395355.
                        BackgroundValue::Number(n) => {
                            toml_edit::Value::from((f64::from(n) * 1e4).round() / 1e4)
                        }
                    };
                    config::set_value(table, key, value);
                }
            }
            if background.is_empty() {
                app.remove("background");
            }
        });
        match result {
            Ok(()) => {
                self.seen = config.modified();
                Ok(match self.read_preferences() {
                    Some(Ok(prefs)) => Some(prefs.background),
                    _ => Some(Default::default()),
                })
            }
            Err(WriteError::Invalid(err)) => {
                self.problem = Some(ConfigProblem::Invalid(err));
                Ok(None)
            }
            Err(err) if err.is_read_only() => {
                self.problem = Some(ConfigProblem::ReadOnly);
                Ok(None)
            }
            Err(WriteError::Io(err)) => Err(err),
        }
    }

    /// Moves `ui.json` over when `ui-state.json` does not exist yet.
    fn migrate(&self) {
        let (Some(state), Some(legacy)) = (&self.state, &self.legacy) else {
            return;
        };
        if state.exists() {
            return;
        }
        let Some(old) = std::fs::read_to_string(legacy)
            .ok()
            .and_then(|text| serde_json::from_str::<UiSettings>(&text).ok())
        else {
            return;
        };
        if let Err(err) = write_json(state, &UiState::of(&old)) {
            eprintln!(
                "llmtodo: could not move UI state to {}: {err}",
                state.display()
            );
            return;
        }
        // Only into a file with no preferences yet: missing, or holding
        // other keys such as `data_dir`.
        let Some(config) = self.config.as_ref().filter(|c| match c.read() {
            ConfigRead::Missing => true,
            ConfigRead::Parsed(doc) => doc.get(APP_TABLE).is_none(),
            ConfigRead::Invalid(_) => false,
        }) else {
            return;
        };
        let changed = Preferences::of(&old).changed_since(&Preferences::default());
        if changed.is_empty() {
            return;
        }
        let result = config.update(|doc| {
            let app = config::table_mut(doc, APP_TABLE);
            for (key, value) in changed {
                config::set_value(app, &key, value);
            }
        });
        if let Err(err) = result {
            eprintln!(
                "llmtodo: could not move preferences to {}: {err}",
                config.path().display()
            );
        }
    }
}

/// Preferences read from `config.toml` after a hand edit. The workspace
/// applies them with [`UiPreferences::apply_to`].
#[derive(Debug, Clone, PartialEq)]
pub struct UiPreferences(Preferences);

impl UiPreferences {
    /// `settings` with these preferences, and its state unchanged.
    pub fn apply_to(self, settings: &UiSettings) -> UiSettings {
        let mut out = settings.clone();
        self.0.apply_to(&mut out);
        out
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(tmp, path)
}

/// Where `ui.json` lived before LLM-449.
fn legacy_dir() -> Option<PathBuf> {
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
mod tests;
