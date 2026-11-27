//! LLM-625: the last full load, saved in the data folder, so the next
//! start shows the sidebar and the list at once. The live load replaces it
//! a moment later.
//!
//! The file holds the snapshot as the server sent it. A file this app
//! cannot read, from an older format or cut short, counts as no file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::backend::Snapshot;

/// The file's name in the data folder.
pub const FILE: &str = "app-cache.json";

/// Goes up when the file's shape changes, so an old file is skipped.
const FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Saved {
    format: u32,
    snapshot: Snapshot,
}

/// Where the cache of the tracker in `data_dir` lives.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// The saved snapshot, if the file exists and this app can read it.
pub fn read(path: &Path) -> Option<Snapshot> {
    let bytes = std::fs::read(path).ok()?;
    let saved: Saved = serde_json::from_slice(&bytes).ok()?;
    (saved.format == FORMAT).then_some(saved.snapshot)
}

/// Saves `snapshot`. It writes a temporary file first and renames it, so a
/// reader never sees half a file.
pub fn write(path: &Path, snapshot: Snapshot) -> std::io::Result<()> {
    let saved = Saved {
        format: FORMAT,
        snapshot,
    };
    let bytes = serde_json::to_vec(&saved).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_snapshot_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = path(dir.path());
        assert!(read(&path).is_none(), "no file yet");
        write(&path, Snapshot::default()).unwrap();
        let back = read(&path).expect("the file reads back");
        assert!(back.issues.is_empty());
        assert!(!dir.path().join("app-cache.json.tmp").exists());
    }

    #[test]
    fn an_unreadable_or_old_file_counts_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = path(dir.path());
        std::fs::write(&path, b"{\"format\":1,\"snap").unwrap();
        assert!(read(&path).is_none(), "a cut file");
        let old = serde_json::json!({ "format": FORMAT + 1, "snapshot": Snapshot::default() });
        std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(read(&path).is_none(), "another format");
    }
}
