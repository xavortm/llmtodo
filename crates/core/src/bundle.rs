//! The installed app bundle. `scripts/publish` writes it, `llmtodo app`
//! opens it, and the running app compares its own build with it.
//!
//! The bundle is `~/Applications/llmtodo.app`. `LLMTODO_APP_BUNDLE` names
//! another path, so tests can publish into a temporary folder.

use std::path::{Path, PathBuf};

/// The file inside the bundle that holds the git commit it was built from.
pub const COMMIT_FILE: &str = "Contents/Resources/build-commit";

/// The marker after a commit when the tree had changes at build time.
pub const DIRTY_SUFFIX: &str = "-dirty";

/// The bundle path: `LLMTODO_APP_BUNDLE`, else `~/Applications/llmtodo.app`.
pub fn installed_bundle() -> PathBuf {
    if let Some(path) = std::env::var_os("LLMTODO_APP_BUNDLE").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Applications/llmtodo.app")
}

/// The commit the bundle at `bundle` was built from. `None` when no bundle
/// is there, or its commit file is missing or empty.
pub fn bundle_commit(bundle: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(bundle.join(COMMIT_FILE)).ok()?;
    let commit = raw.trim();
    (!commit.is_empty()).then(|| commit.to_string())
}

/// Whether `exe` runs from inside the bundle at `bundle`.
pub fn runs_from(exe: &Path, bundle: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    canon(exe).starts_with(canon(bundle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_commit_and_ignores_whitespace() {
        let dir = std::env::temp_dir().join(format!("llmtodo-bundle-{}", std::process::id()));
        let bundle = dir.join("llmtodo.app");
        assert_eq!(bundle_commit(&bundle), None);
        std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        std::fs::write(bundle.join(COMMIT_FILE), "  \n").unwrap();
        assert_eq!(bundle_commit(&bundle), None);
        std::fs::write(bundle.join(COMMIT_FILE), "abc123\n").unwrap();
        assert_eq!(bundle_commit(&bundle).as_deref(), Some("abc123"));
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(bundle.join("Contents/MacOS/llmtodo-app"), "").unwrap();
        assert!(runs_from(
            &bundle.join("Contents/MacOS/llmtodo-app"),
            &bundle
        ));
        assert!(!runs_from(&dir.join("other"), &bundle));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
