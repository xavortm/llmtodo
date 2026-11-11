//! Which data folder a client opens. The CLI and the app both call
//! [`resolve`], so they open the same folder.
//!
//! The folder is always an input to the rest of the client. Nothing here
//! caches it, so one process can open several folders, as the app's
//! trackers do.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub use llmtodo_core::data_dir::{DATA_DIR_ENV, standard_data_dir, standard_data_dir_in};

/// `~/.config/llmtodo/config.toml`, the settings shared by every client on
/// this machine.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct GlobalConfig {
    pub data_dir: Option<PathBuf>,
}

/// The path of the global config file under `home`.
pub fn global_config_path(home: &Path) -> PathBuf {
    home.join(".config/llmtodo/config.toml")
}

/// Reads the global config. A missing or broken file reads as empty.
pub fn global_config() -> GlobalConfig {
    std::env::var_os("HOME")
        .map(|h| read_global_config(&global_config_path(Path::new(&h))))
        .unwrap_or_default()
}

fn read_global_config(path: &Path) -> GlobalConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// The data folder to open. The first of these wins:
///
/// 1. `explicit`, such as the CLI's `--data-dir` flag.
/// 2. `LLMTODO_DATA_DIR`, when it is not empty.
/// 3. `data_dir` in the global config.
/// 4. The standard folder, `~/Library/Application Support/llmtodo`.
pub fn resolve(explicit: Option<&Path>) -> PathBuf {
    let env = std::env::var_os(DATA_DIR_ENV)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from);
    pick(explicit, env, || global_config().data_dir)
}

fn pick(
    explicit: Option<&Path>,
    env: Option<PathBuf>,
    config: impl FnOnce() -> Option<PathBuf>,
) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .filter(|p| !p.as_os_str().is_empty())
        .or(env)
        .or_else(config)
        .unwrap_or_else(standard_data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_source_that_names_a_folder_wins() {
        let p = PathBuf::from;
        let config = || Some(p("/config"));
        assert_eq!(
            pick(Some(Path::new("/flag")), Some(p("/env")), config),
            p("/flag")
        );
        assert_eq!(pick(None, Some(p("/env")), config), p("/env"));
        assert_eq!(pick(None, None, config), p("/config"));
        assert_eq!(pick(Some(Path::new("")), None, config), p("/config"));
        assert_eq!(pick(None, None, || None), standard_data_dir());
    }

    #[test]
    fn the_global_config_names_the_data_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = global_config_path(dir.path());
        assert_eq!(read_global_config(&path).data_dir, None);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "data_dir = \"/somewhere/else\"\n").unwrap();
        assert_eq!(
            read_global_config(&path).data_dir,
            Some(PathBuf::from("/somewhere/else"))
        );
        std::fs::write(&path, "not toml [").unwrap();
        assert_eq!(read_global_config(&path).data_dir, None);
    }
}
