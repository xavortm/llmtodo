//! Restart to update. The app knows the commit it was built from, and
//! `scripts/publish` writes the commit of the installed bundle into it. When
//! the two differ, the title bar offers a restart onto the installed bundle.
//!
//! The restart runs a small shell helper that outlives the app. It waits
//! until the app has quit, stops the server so the next start uses the new
//! binary, and opens the bundle. The app cannot stop the server itself: its
//! API thread would start the old binary again before the app quits.

use std::path::{Path, PathBuf};
use std::process::Command;

use llmtodo_core::bundle;

/// The commit this app was built from, from `build.rs`.
pub const BUILD_COMMIT: &str = env!("LLMTODO_BUILD_COMMIT");

/// What the app compares to find a new build.
#[derive(Debug, Clone)]
pub struct UpdateWatch {
    pub bundle: PathBuf,
    pub own: String,
}

impl UpdateWatch {
    /// The watch for this process, or `None` for a preview or demo window.
    pub fn from_env(data_dir: &Path) -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let exe = std::env::current_exe().ok();
        let bundle = bundle::installed_bundle();
        enabled(data_dir, home.as_deref(), exe.as_deref(), &bundle).then(|| Self {
            bundle,
            own: BUILD_COMMIT.to_string(),
        })
    }

    /// The installed bundle's commit, when it differs from this build.
    pub fn newer(&self) -> Option<String> {
        bundle::bundle_commit(&self.bundle).filter(|c| *c != self.own)
    }
}

/// The button shows only for the owner's real app: one on the real data
/// folder, or one that runs from the installed bundle.
pub fn enabled(data_dir: &Path, home: Option<&Path>, exe: Option<&Path>, bundle: &Path) -> bool {
    let real = home.is_some_and(|h| {
        let real = crate::model::trackers::real_dir(h);
        match (real.canonicalize(), data_dir.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => real == data_dir,
        }
    });
    real || exe.is_some_and(|e| bundle::runs_from(e, bundle))
}

/// The helper that finishes a restart after the app quits.
#[derive(Debug, Clone)]
pub struct Restart {
    /// The app's process. The helper waits until it is gone.
    pub pid: u32,
    pub data_dir: PathBuf,
    pub bundle: PathBuf,
    /// The CLI that stops the server. The bundle's own copy when it has one.
    pub cli: Option<PathBuf>,
    /// `open` on macOS. Tests pass another command.
    pub open: String,
}

/// Waits for the app, stops the folder's server, waits until the server
/// lets go of its lock, then opens the bundle.
const HELPER: &str = r#"
pid=$1 data=$2 bundle=$3 cli=$4 open=$5
while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
if [ -n "$cli" ]; then
  LLMTODO_DATA_DIR="$data" "$cli" server stop --no-input --json || true
  i=0
  while /usr/sbin/lsof -t "$data/server.lock" >/dev/null 2>&1 && [ "$i" -lt 100 ]; do
    sleep 0.1
    i=$((i + 1))
  done
fi
$open "$bundle"
"#;

impl Restart {
    pub fn new(data_dir: &Path, bundle: &Path, fallback_cli: Option<PathBuf>) -> Self {
        let inside = bundle.join("Contents/MacOS/llmtodo");
        Self {
            pid: std::process::id(),
            data_dir: data_dir.to_path_buf(),
            bundle: bundle.to_path_buf(),
            cli: Some(inside).filter(|p| p.is_file()).or(fallback_cli),
            open: std::env::var("LLMTODO_OPEN_COMMAND").unwrap_or_else(|_| "open".into()),
        }
    }

    /// The helper command. Its output goes to `restart.log` in the data
    /// folder. It runs in its own process group, so a Ctrl-C in the
    /// terminal of a `cargo run` app cannot stop it halfway.
    pub fn command(&self) -> Command {
        let mut c = Command::new("/bin/sh");
        c.arg("-c")
            .arg(HELPER)
            .arg("llmtodo-restart")
            .arg(self.pid.to_string())
            .arg(&self.data_dir)
            .arg(&self.bundle)
            .arg(self.cli.as_deref().unwrap_or(Path::new("")))
            .arg(&self.open)
            .stdin(std::process::Stdio::null());
        if let Ok(log) = std::fs::File::create(self.data_dir.join("restart.log"))
            && let Ok(err) = log.try_clone()
        {
            c.stdout(log).stderr(err);
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_real_folder_or_the_bundle_offers_a_restart() {
        let home = tempfile::tempdir().unwrap();
        let real = crate::model::trackers::real_dir(home.path());
        std::fs::create_dir_all(&real).unwrap();
        let bundle = home.path().join("Applications/llmtodo.app");
        let in_bundle = bundle.join("Contents/MacOS/llmtodo-app");
        std::fs::create_dir_all(in_bundle.parent().unwrap()).unwrap();
        std::fs::write(&in_bundle, "").unwrap();
        let preview = tempfile::tempdir().unwrap();
        let debug = Path::new("/repo/target/debug/llmtodo-app");

        assert!(enabled(&real, Some(home.path()), Some(debug), &bundle));
        assert!(enabled(
            preview.path(),
            Some(home.path()),
            Some(&in_bundle),
            &bundle
        ));
        assert!(!enabled(
            preview.path(),
            Some(home.path()),
            Some(debug),
            &bundle
        ));
        assert!(!enabled(preview.path(), None, None, &bundle));
    }

    #[test]
    fn a_different_installed_commit_is_newer() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("llmtodo.app");
        let watch = UpdateWatch {
            bundle: bundle.clone(),
            own: "aaa".into(),
        };
        assert_eq!(watch.newer(), None);
        std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
        std::fs::write(bundle.join(bundle::COMMIT_FILE), "aaa\n").unwrap();
        assert_eq!(watch.newer(), None);
        std::fs::write(bundle.join(bundle::COMMIT_FILE), "bbb\n").unwrap();
        assert_eq!(watch.newer().as_deref(), Some("bbb"));
    }

    /// The helper waits for the app, stops the server with the CLI and
    /// opens the bundle, in that order.
    #[test]
    fn the_helper_waits_then_stops_then_opens() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("steps");
        let fake = |name: &str, line: &str| {
            let path = dir.path().join(name);
            std::fs::write(
                &path,
                format!("#!/bin/sh\necho \"{line} $*\" >> '{}'\n", log.display()),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
            path
        };
        let cli = fake("cli", "stop");
        let open = fake("open", "open");
        let mut app = Command::new("/bin/sleep").arg("0.3").spawn().unwrap();
        let restart = Restart {
            pid: app.id(),
            data_dir: dir.path().to_path_buf(),
            bundle: dir.path().join("llmtodo.app"),
            cli: Some(cli),
            open: open.display().to_string(),
        };
        let mut helper = restart.command().spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!log.exists(), "the helper ran before the app quit");
        app.wait().unwrap();
        assert!(helper.wait().unwrap().success());
        let steps = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = steps.lines().collect();
        assert_eq!(lines.len(), 2, "{steps}");
        assert!(lines[0].starts_with("stop server stop"), "{steps}");
        assert!(
            lines[1].starts_with("open ") && lines[1].ends_with("llmtodo.app"),
            "{steps}"
        );
    }
}
