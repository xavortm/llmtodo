//! The standard data folder. `llmtodo_client::data_dir` decides which
//! folder a client opens. This module only names the default one, so the
//! server and the app can mark it without depending on the client.

use std::path::{Path, PathBuf};

/// The environment variable that names the data folder.
pub const DATA_DIR_ENV: &str = "LLMTODO_DATA_DIR";

/// The owner's real data folder under `home`:
/// `~/Library/Application Support/llmtodo`.
pub fn standard_data_dir_in(home: &Path) -> PathBuf {
    home.join("Library/Application Support/llmtodo")
}

/// The owner's real data folder under `HOME`, or under `.` when `HOME` is
/// not set.
pub fn standard_data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    standard_data_dir_in(&home)
}
