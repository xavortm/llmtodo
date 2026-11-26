//! The settings store: the split between `config.toml` and
//! `ui-state.json`, the rules for the person's file, and the move from
//! `ui.json`. Every test works in a temporary folder.

use super::*;

struct Dir {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

impl Dir {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        Self { _tmp: tmp, root }
    }

    fn store(&self) -> SettingsStore {
        SettingsStore::at(self.state())
    }

    fn state(&self) -> PathBuf {
        self.root.join("ui-state.json")
    }

    fn config(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    fn read_config(&self) -> String {
        std::fs::read_to_string(self.config()).unwrap()
    }
}

#[test]
fn round_trips_and_survives_bad_files() {
    let dir = Dir::new();
    let mut store = dir.store();
    assert_eq!(store.load(), UiSettings::default(), "missing files");

    let settings = UiSettings {
        group: "status".into(),
        collapsed_groups: vec!["status:Done".into()],
        split_side: Some(612.),
        hidden_columns: vec!["labels".into(), "cost".into()],
        color_theme: "gruvbox".into(),
        zoom: 1.1,
        ..Default::default()
    };
    store.save(&UiSettings::default(), &settings).unwrap();
    assert_eq!(store.load(), settings);

    std::fs::write(dir.state(), "{ not json").unwrap();
    std::fs::write(dir.config(), "not toml [").unwrap();
    assert_eq!(store.load(), UiSettings::default(), "broken files");

    // Old files without newer fields still load.
    std::fs::write(dir.state(), r#"{"group":"label"}"#).unwrap();
    std::fs::write(dir.config(), "[app]\nmascot = false\n").unwrap();
    let loaded = store.load();
    assert_eq!(loaded.group, "label");
    assert!(!loaded.mascot);
    assert_eq!(loaded.theme, "dark");
    assert_eq!(loaded.hide_closed_days, 14);
    assert!(loaded.strike_done, "the line is on for old files");
}

#[test]
fn the_cat_is_on_by_default() {
    assert!(UiSettings::default().mascot);
}

#[test]
fn no_change_writes_no_file() {
    let dir = Dir::new();
    let mut store = dir.store();
    let settings = store.load();
    assert!(store.check_file().is_none());
    store.save(&settings, &settings.clone()).unwrap();
    assert!(!dir.config().exists());
    assert!(!dir.state().exists());
}

#[test]
fn a_preference_change_writes_only_that_key() {
    let dir = Dir::new();
    let text = "# mine\ndata_dir = \"/x\"\n\n[app]\n# keep\nzoom = 1.2 # big\nfuture = \"kept\"\n";
    std::fs::write(dir.config(), text).unwrap();
    let mut store = dir.store();
    let old = store.load();
    assert_eq!(old.zoom, 1.2);
    let new = UiSettings {
        vim_editor: false,
        ..old.clone()
    };
    store.save(&old, &new).unwrap();
    assert_eq!(dir.read_config(), format!("{text}vim_editor = false\n"));
    assert!(!dir.state().exists(), "no state changed");
}

#[test]
fn a_window_move_changes_only_the_state_file() {
    let dir = Dir::new();
    std::fs::write(dir.config(), "[app]\ntheme = \"light\"\n").unwrap();
    let mut store = dir.store();
    let old = store.load();
    let new = UiSettings {
        window: Some([10., 20., 900., 700.]),
        split_side: Some(500.),
        ..old.clone()
    };
    store.save(&old, &new).unwrap();
    assert_eq!(dir.read_config(), "[app]\ntheme = \"light\"\n");
    let state = std::fs::read_to_string(dir.state()).unwrap();
    assert!(state.contains("900"), "{state}");
    assert!(!state.contains("theme"), "{state}");
}

#[test]
fn a_read_only_file_keeps_its_content_and_says_so_once() {
    let dir = Dir::new();
    std::fs::write(dir.config(), "[app]\ntheme = \"dark\"\n").unwrap();
    let mut perms = std::fs::metadata(dir.config()).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(dir.config(), perms).unwrap();
    let mut store = dir.store();
    let old = store.load();
    store.check_file();
    let new = UiSettings {
        theme: "light".into(),
        ..old.clone()
    };
    store.save(&old, &new).unwrap();
    assert_eq!(store.problem(), Some(&ConfigProblem::ReadOnly));
    assert_eq!(
        store.problem().unwrap().note(),
        "Settings file is read-only. Changes last until you quit."
    );
    store.save(&new, &UiSettings::default()).unwrap();
    assert_eq!(dir.read_config(), "[app]\ntheme = \"dark\"\n");
}

#[test]
fn a_hand_edit_applies_and_a_broken_one_keeps_the_last_good_settings() {
    let dir = Dir::new();
    let mut store = dir.store();
    let settings = store.load();
    assert!(store.check_file().is_none(), "the first check only looks");
    assert!(store.check_file().is_none(), "nothing changed");

    std::fs::write(dir.config(), "[app]\ntheme = \"light\"\nmascot = false\n").unwrap();
    let edited = store.check_file().expect("the edit").apply_to(&settings);
    assert_eq!(edited.theme, "light");
    assert!(!edited.mascot);
    assert!(store.check_file().is_none(), "seen once");

    // A save from the app does not come back as an edit.
    let next = UiSettings {
        zoom: 1.5,
        ..edited.clone()
    };
    store.save(&edited, &next).unwrap();
    assert!(store.check_file().is_none(), "the app's own write");

    std::fs::write(dir.config(), "[app]\ntheme = \"light\"\nzoom = = 2\n").unwrap();
    assert!(
        store.check_file().is_none(),
        "a broken file applies nothing"
    );
    let Some(ConfigProblem::Invalid(err)) = store.problem() else {
        panic!("a parse error, got {:?}", store.problem());
    };
    assert_eq!(err.line, Some(3));
    let broken = dir.read_config();
    store.save(&next, &UiSettings::default()).unwrap();
    assert_eq!(dir.read_config(), broken, "never written over");

    // A key of the wrong type is a parse error too.
    std::fs::write(dir.config(), "[app]\nzoom = \"big\"\n").unwrap();
    assert!(store.check_file().is_none());
    assert!(matches!(store.problem(), Some(ConfigProblem::Invalid(_))));

    std::fs::write(dir.config(), "[app]\ntheme = \"dark\"\n").unwrap();
    assert_eq!(
        store.check_file().map(|p| p.apply_to(&next).theme),
        Some("dark".into())
    );
    assert_eq!(store.problem(), None, "fixed");
}

#[test]
fn ui_json_moves_over_at_the_first_start() {
    let dir = Dir::new();
    let legacy = dir.root.join("ui.json");
    let old = UiSettings {
        theme: "light".into(),
        window: Some([1., 2., 800., 600.]),
        group: "status".into(),
        ..Default::default()
    };
    std::fs::write(&legacy, serde_json::to_string(&old).unwrap()).unwrap();
    let store = SettingsStore {
        legacy: Some(legacy.clone()),
        ..dir.store()
    };
    assert_eq!(store.load(), old);
    assert_eq!(
        dir.read_config(),
        "[app]\ntheme = \"light\"\n",
        "only non-defaults"
    );
    assert!(dir.state().exists());
    assert!(legacy.exists(), "ui.json stays for older builds");

    // Later starts read the new files, not ui.json.
    std::fs::write(&legacy, "{}").unwrap();
    assert_eq!(store.load(), old);
}

#[test]
fn the_move_leaves_an_existing_config_alone() {
    let dir = Dir::new();
    let legacy = dir.root.join("ui.json");
    std::fs::write(&legacy, r#"{"theme":"light","group":"label"}"#).unwrap();
    std::fs::write(dir.config(), "# mine\n[app]\nzoom = 1.0\n").unwrap();
    let store = SettingsStore {
        legacy: Some(legacy),
        ..dir.store()
    };
    let loaded = store.load();
    assert_eq!(loaded.group, "label");
    assert_eq!(loaded.theme, "dark");
    assert_eq!(dir.read_config(), "# mine\n[app]\nzoom = 1.0\n");
}

#[test]
fn the_move_fills_a_config_that_holds_only_the_data_folder() {
    let dir = Dir::new();
    let legacy = dir.root.join("ui.json");
    std::fs::write(&legacy, r#"{"theme":"light"}"#).unwrap();
    std::fs::write(dir.config(), "data_dir = \"/x\"\n").unwrap();
    let store = SettingsStore {
        legacy: Some(legacy),
        ..dir.store()
    };
    assert_eq!(store.load().theme, "light");
    assert_eq!(
        dir.read_config(),
        "data_dir = \"/x\"\n\n[app]\ntheme = \"light\"\n"
    );
}

#[test]
fn the_readme_example_reads() {
    let readme = include_str!("../../../../README.md");
    let start = readme.find("```toml\n").unwrap() + "```toml\n".len();
    let example = &readme[start..start + readme[start..].find("```").unwrap()];
    let dir = Dir::new();
    std::fs::write(dir.config(), example).unwrap();
    let loaded = dir.store().load();
    assert_eq!(loaded.theme, "auto");
    assert_eq!(loaded.color_theme, "catppuccin");
    assert_eq!(loaded.zoom, 1.1);
    assert!(loaded.hide_closed);
}

/// LLM-560: the activity strip shows by default, also for a state file
/// from before it existed, and a hidden strip stays hidden.
#[test]
fn the_activity_strip_is_on_by_default_and_saves() {
    assert!(UiSettings::default().activity_strip);
    let dir = Dir::new();
    let mut store = dir.store();
    std::fs::write(dir.state(), r#"{"group":"label"}"#).unwrap();
    assert!(store.load().activity_strip, "an old file shows it");
    let hidden = UiSettings {
        activity_strip: false,
        ..Default::default()
    };
    store.save(&UiSettings::default(), &hidden).unwrap();
    assert!(!store.load().activity_strip);
}
