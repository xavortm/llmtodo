//! The trackers the app can show. A tracker is one data folder with its own
//! server. The app opens `LLMTODO_DATA_DIR`, and the sidebar lists the
//! others, so a preview on demo data can still look up a real ticket.
//!
//! - `LLMTODO_TRACKER_NAME` names the tracker the app opened.
//! - `LLMTODO_TRACKERS` lists more, separated by `:`, each as
//!   `Name=/path` or a bare `/path`.
//! - The owner's real folder, `~/Library/Application Support/llmtodo`, is
//!   always on the list. It is read-only unless the app opened on it, so a
//!   preview can look up a real ticket but cannot change one.

use std::path::{Path, PathBuf};

use gpui_shared_string::SharedString;

/// The name the owner's real folder gets when nothing names it.
pub const REAL_NAME: &str = "Real tracker";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracker {
    pub name: SharedString,
    pub data_dir: PathBuf,
    /// The owner's real data folder. The app marks it, so the owner
    /// never edits real data by mistake.
    pub real: bool,
    /// The app can read this tracker but never writes to it.
    pub read_only: bool,
    /// The team keys this tracker holds, once known.
    pub teams: Option<Vec<SharedString>>,
}

/// The real folder: `~/Library/Application Support/llmtodo`.
pub fn real_dir(home: &Path) -> PathBuf {
    llmtodo_core::data_dir::standard_data_dir_in(home)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The trackers in order: the opened one first, then `extra`, then the
/// real folder. A folder shows once, under its first name.
pub fn known(opened: (Option<String>, PathBuf), extra: &str, home: Option<&Path>) -> Vec<Tracker> {
    let real = home.map(real_dir);
    let is_real = |dir: &Path| real.as_deref().is_some_and(|r| same_dir(r, dir));
    let opened_on_real = is_real(&opened.1);
    let default_name = |dir: &Path| -> SharedString {
        if is_real(dir) {
            return REAL_NAME.into();
        }
        dir.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.display().to_string())
            .into()
    };
    let mut out: Vec<Tracker> = Vec::new();
    let mut add = |name: Option<String>, dir: PathBuf| {
        if dir.as_os_str().is_empty() || out.iter().any(|t| same_dir(&t.data_dir, &dir)) {
            return;
        }
        let name = name
            .filter(|n| !n.trim().is_empty())
            .map(|n| SharedString::from(n.trim().to_string()))
            .unwrap_or_else(|| default_name(&dir));
        out.push(Tracker {
            name,
            real: is_real(&dir),
            read_only: is_real(&dir) && !opened_on_real,
            data_dir: dir,
            teams: None,
        });
    };
    add(opened.0, opened.1);
    for entry in extra.split(':').map(str::trim).filter(|e| !e.is_empty()) {
        match entry.split_once('=') {
            Some((name, path)) if !path.is_empty() => {
                add(Some(name.to_string()), PathBuf::from(path))
            }
            _ => add(None, PathBuf::from(entry)),
        }
    }
    if let Some(real) = real.clone() {
        add(None, real);
    }
    out
}

/// The trackers from the environment, with `data_dir` as the opened one.
pub fn from_env(data_dir: &Path) -> Vec<Tracker> {
    let name = std::env::var("LLMTODO_TRACKER_NAME").ok();
    let extra = std::env::var("LLMTODO_TRACKERS").unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    known((name, data_dir.to_path_buf()), &extra, home.as_deref())
}

/// The team key of an issue key: `LLM` in `LLM-13`.
pub fn team_of(key: &str) -> Option<&str> {
    let (team, number) = key.rsplit_once('-')?;
    (!team.is_empty() && !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
        .then_some(team)
}

/// Which other tracker should open `key`: the one known to hold its team,
/// else the first one whose teams are unknown. `None` when no other
/// tracker could hold it.
pub fn owner_of(trackers: &[Tracker], current: usize, key: &str) -> Option<usize> {
    let team = team_of(key)?;
    let others = || {
        trackers
            .iter()
            .enumerate()
            .filter(move |(ix, _)| *ix != current)
    };
    others()
        .find(|(_, t)| {
            t.teams
                .as_ref()
                .is_some_and(|teams| teams.iter().any(|k| k.eq_ignore_ascii_case(team)))
        })
        .or_else(|| others().find(|(_, t)| t.teams.is_none()))
        .map(|(ix, _)| ix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(name: &str, teams: Option<&[&str]>) -> Tracker {
        Tracker {
            name: name.to_string().into(),
            data_dir: PathBuf::from(format!("/tmp/{name}")),
            real: false,
            read_only: false,
            teams: teams.map(|t| {
                t.iter()
                    .map(|k| SharedString::from(k.to_string()))
                    .collect()
            }),
        }
    }

    #[test]
    fn the_opened_folder_comes_first_and_the_real_one_last() {
        let home = Path::new("/nowhere/home");
        let list = known(
            (Some("Demo".into()), PathBuf::from("/tmp/demo")),
            "Import=/tmp/import:/tmp/other",
            Some(home),
        );
        let names: Vec<_> = list.iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names, ["Demo", "Import", "other", REAL_NAME]);
        assert_eq!(list[3].data_dir, real_dir(home));
        assert!(list[3].real);
        assert!(!list[0].real);
    }

    #[test]
    fn a_folder_listed_twice_shows_once() {
        let home = Path::new("/nowhere/home");
        let real = real_dir(home);
        let extra = format!("Mine={}:/tmp/demo", real.display());
        let list = known((None, PathBuf::from("/tmp/demo")), &extra, Some(home));
        let names: Vec<_> = list.iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names, ["demo", "Mine"]);
        assert!(list[1].real);
    }

    #[test]
    fn opened_on_the_real_folder_lists_only_it() {
        let home = Path::new("/nowhere/home");
        let list = known((None, real_dir(home)), "", Some(home));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, REAL_NAME);
        assert!(!list[0].read_only, "the real app writes to its own data");
    }

    /// LLM-137: every path to the real folder is read-only from a preview.
    #[test]
    fn the_real_folder_is_read_only_when_the_app_opened_elsewhere() {
        let home = Path::new("/nowhere/home");
        let real = real_dir(home);
        let listed = format!("Mine={}", real.display());
        for extra in ["", listed.as_str()] {
            let list = known((None, PathBuf::from("/tmp/demo")), extra, Some(home));
            assert!(!list[0].read_only, "the preview's own data stays editable");
            assert!(list[1].real && list[1].read_only, "{extra}");
        }
    }

    #[test]
    fn team_keys_come_from_issue_keys() {
        assert_eq!(team_of("LLM-13"), Some("LLM"));
        assert_eq!(team_of("MY-TEAM-2"), Some("MY-TEAM"));
        assert_eq!(team_of("LLM-"), None);
        assert_eq!(team_of("LLM-x"), None);
        assert_eq!(team_of("13"), None);
    }

    #[test]
    fn a_key_goes_to_the_tracker_that_holds_its_team() {
        let list = [
            tracker("demo", Some(&["DEMO", "WEB"])),
            tracker("other", None),
            tracker("real", Some(&["LLM"])),
        ];
        assert_eq!(owner_of(&list, 0, "LLM-13"), Some(2));
        assert_eq!(owner_of(&list, 0, "llm-13"), Some(2));
        // No tracker is known to hold it: try one whose teams are unknown.
        assert_eq!(owner_of(&list, 0, "ZZ-1"), Some(1));
        assert_eq!(owner_of(&list, 1, "ZZ-1"), None);
        assert_eq!(owner_of(&list, 0, "nonsense"), None);
    }
}
