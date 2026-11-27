//! LLM-591: Try it opens a ticket's preview build on demo data.
//!
//! `scripts/preview-build` leaves the build at
//! `<checkout>/target/preview/<KEY>/llmtodo-app`, with its own `llmtodo` CLI
//! beside it (LLM-590). Try it runs that checkout's `scripts/preview` with
//! the recorded build in place of a fresh compile, so the steps stay in one
//! place: a fresh temporary folder, demo data from `scripts/seed-demo`, a
//! server on a free port, then the app on that folder. The preview marks
//! itself as the ticket's preview (LLM-592), and the real tracker in it is
//! read-only (LLM-137).
//!
//! Each run gets a folder of its own, passed as `TMPDIR`, so the script's
//! temporary folder and its logs land inside it. A small shell line around
//! the script deletes the folder once the script ends, which stops the
//! server first. So the cleanup happens even when this app quits before the
//! preview does. The run sits in its own process group, so a Ctrl-C on this
//! app's terminal leaves the preview alone.
//!
//! One preview runs per ticket. A click on a ticket whose preview runs
//! brings that window forward instead of starting another.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use llmtodo_api_types::PreviewBuild;

/// How to run one ticket's preview build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    /// The ticket key, such as `LLM-591`.
    pub key: String,
    /// The recorded app binary.
    pub app: PathBuf,
    /// The CLI built with it, which seeds the demo data and runs the server.
    pub cli: PathBuf,
    /// `scripts/preview` in the checkout that built it.
    pub script: PathBuf,
}

/// Why Try it is off, in a few words.
pub const ANOTHER_COMPUTER: &str = "Built on another computer";
pub const MISSING: &str = "Build is missing";
pub const STALE: &str = "Build is out of date";

/// How to run `build`, or why it cannot run on this machine. `host` is this
/// machine's name, as the CLI stores it with the build.
pub fn check(key: &str, build: &PreviewBuild, host: Option<&str>) -> Result<Launch, &'static str> {
    if build.host.is_some() && build.host.as_deref() != host {
        return Err(ANOTHER_COMPUTER);
    }
    let app = PathBuf::from(&build.path);
    let dir = app.parent().ok_or(MISSING)?;
    let cli = dir.join("llmtodo");
    // `<checkout>/target/preview/<KEY>/llmtodo-app`.
    let checkout = dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or(MISSING)?;
    let script = checkout.join("scripts").join("preview");
    if !app.is_file() || !cli.is_file() || !script.is_file() {
        return Err(MISSING);
    }
    if build.stale {
        return Err(STALE);
    }
    Ok(Launch {
        key: key.to_string(),
        app,
        cli,
        script,
    })
}

/// Where one ticket's preview is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    /// The script seeds the demo data and starts the server.
    Starting,
    /// The preview app is open.
    Running,
    /// The last run failed, with a short reason. A new click tries again.
    Failed(String),
}

/// What a click did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// A new preview starts.
    Started,
    /// The preview is still starting, so the click does nothing.
    Starting,
    /// The open preview came to the front.
    Forward,
}

struct Run {
    state: RunState,
}

type Changed = Arc<dyn Fn() + Send + Sync>;

/// The previews this app started, one per ticket. Cheap to clone: clones
/// share the list.
#[derive(Clone)]
pub struct Previews {
    runs: Arc<Mutex<HashMap<String, Run>>>,
    changed: Changed,
}

impl Default for Previews {
    fn default() -> Self {
        Self::new(|| {})
    }
}

impl std::fmt::Debug for Previews {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Previews").finish_non_exhaustive()
    }
}

/// The shell line around `scripts/preview`: `$1` is the script, `$2` the
/// run's folder. The script stops its server when it ends, and then the
/// folder goes. The script writes to a file, never to this app's pipe: a
/// script left writing to the pipe of an app that quit could hang before
/// it stops its server. The shell line hands back the file's last lines.
const WRAPPER: &str = r#""$1" >"$2/preview.out" 2>&1; code=$?; tail -n 5 "$2/preview.out" 2>/dev/null; rm -rf "$2"; exit "$code""#;

/// How often a starting run's output is read for the line that says the
/// app opened.
const START_POLL: std::time::Duration = std::time::Duration::from_millis(100);

impl Previews {
    /// `changed` runs on a background thread each time a preview starts,
    /// opens or ends, so the caller can draw again.
    pub fn new(changed: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            runs: Arc::default(),
            changed: Arc::new(changed),
        }
    }

    /// Where the preview of `key` is, or None when none runs.
    pub fn state(&self, key: &str) -> Option<RunState> {
        lock(&self.runs).get(key).map(|r| r.state.clone())
    }

    /// Starts the preview, or brings an open one forward. An open preview
    /// counts even when another run of this app started it.
    pub fn open(&self, launch: &Launch) -> Result<Opened, String> {
        let mut runs = lock(&self.runs);
        match runs.get(&launch.key).map(|r| &r.state) {
            Some(RunState::Starting) => return Ok(Opened::Starting),
            // Its script still runs, so it never starts a second one.
            Some(RunState::Running) => {
                bring_forward(&launch.app);
                return Ok(Opened::Forward);
            }
            Some(RunState::Failed(_)) | None => {}
        }
        if bring_forward(&launch.app) {
            return Ok(Opened::Forward);
        }
        match self.spawn(launch) {
            Ok(()) => {
                runs.insert(
                    launch.key.clone(),
                    Run {
                        state: RunState::Starting,
                    },
                );
                Ok(Opened::Started)
            }
            Err(e) => {
                let text = format!("The preview did not start: {e}");
                runs.insert(
                    launch.key.clone(),
                    Run {
                        state: RunState::Failed(text.clone()),
                    },
                );
                Err(text)
            }
        }
    }

    fn spawn(&self, launch: &Launch) -> std::io::Result<()> {
        let folder = run_folder(&launch.key)?;
        let mut child = match command(launch, &folder).spawn() {
            Ok(child) => child,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&folder);
                return Err(e);
            }
        };
        let stdout = child.stdout.take();
        let runs = self.runs.clone();
        let changed = self.changed.clone();
        let key = launch.key.clone();
        std::thread::Builder::new()
            .name(format!("try-it-{key}"))
            .spawn(move || {
                // Until the app opens, read the script's output for the
                // line that says so. After that, only its end matters.
                let out = folder.join("preview.out");
                let mut opened = false;
                loop {
                    if std::fs::read_to_string(&out).is_ok_and(|t| t.contains("PREVIEW_DATA=")) {
                        opened = true;
                        set_state(&runs, &key, RunState::Running);
                        changed();
                        break;
                    }
                    match child.try_wait() {
                        Ok(None) => std::thread::sleep(START_POLL),
                        Ok(Some(_)) | Err(_) => break,
                    }
                }
                let status = child.wait();
                let mut text = String::new();
                if let Some(mut stdout) = stdout {
                    let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
                }
                let tail: VecDeque<&str> = text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();
                // The shell line deletes it too. This covers a shell that
                // was killed.
                let _ = std::fs::remove_dir_all(&folder);
                let ok = status.as_ref().is_ok_and(|s| s.success());
                if ok {
                    lock(&runs).remove(&key);
                } else {
                    let reason = if opened {
                        "The preview quit with an error".to_string()
                    } else {
                        let last = tail
                            .iter()
                            .rev()
                            .find(|l| !l.starts_with("PREVIEW_"))
                            .map_or_else(|| "no output".to_string(), |l| l.to_string());
                        format!("The preview did not start: {last}")
                    };
                    set_state(&runs, &key, RunState::Failed(reason));
                }
                changed();
            })?;
        Ok(())
    }
}

fn lock(runs: &Mutex<HashMap<String, Run>>) -> std::sync::MutexGuard<'_, HashMap<String, Run>> {
    runs.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn set_state(runs: &Mutex<HashMap<String, Run>>, key: &str, state: RunState) {
    lock(runs).insert(key.to_string(), Run { state });
}

/// A new, empty folder for one run, inside the user's temporary folder.
fn run_folder(key: &str) -> std::io::Result<PathBuf> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let parent = std::env::temp_dir().join("llmtodo-try-it");
    std::fs::create_dir_all(&parent)?;
    let safe: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    loop {
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!("{safe}-{}-{n}-{nanos}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Wraps `s` in single quotes for a POSIX shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The command for one run. `scripts/preview` skips its build when both
/// `LLMTODO_PREVIEW_APP` and `LLMTODO_BIN` are set, and runs the app
/// command with `sh -c`.
pub fn command(launch: &Launch, folder: &Path) -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(WRAPPER)
        .arg("sh")
        .arg(&launch.script)
        .arg(folder)
        .current_dir(folder)
        .env("TMPDIR", folder)
        .env(
            "LLMTODO_PREVIEW_APP",
            format!("exec {}", shell_quote(&launch.app.to_string_lossy())),
        )
        .env("LLMTODO_BIN", &launch.cli)
        .env(crate::model::preview::TICKET_VAR, &launch.key)
        // The script picks these for the preview. Values from this app's
        // own start must not leak into it.
        .env_remove("LLMTODO_PORT")
        .env_remove("LLMTODO_DATA_DIR")
        .env_remove("LLMTODO_CONFIG_DIR")
        .env_remove("LLMTODO_TRACKER_NAME")
        .env_remove("LLMTODO_PROJECT_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    cmd
}

/// The processes that run the binary at `app`, from `ps`. A preview build
/// starts with its path as its command, as `exec` in `scripts/preview` runs
/// it.
fn pids_running(app: &Path) -> Vec<i32> {
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let want = canonical(app);
    let Ok(out) = Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,comm="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, comm) = line.trim_start().split_once(' ')?;
            let comm = Path::new(comm.trim());
            (comm == app || canonical(comm) == want)
                .then(|| pid.parse().ok())
                .flatten()
        })
        .collect()
}

/// Brings the running app at `app` to the front. False when none runs.
///
/// It looks the process up by its ID each time. `NSWorkspace`'s list of
/// running apps changes only while the main run loop runs, so it can miss
/// a preview that just opened.
#[cfg(target_os = "macos")]
pub fn bring_forward(app: &Path) -> bool {
    use objc2::MainThreadMarker;
    use objc2::runtime::NSObjectProtocol as _;
    use objc2::sel;
    use objc2_app_kit::{NSApplication, NSApplicationActivationOptions, NSRunningApplication};

    let Some(found) = pids_running(app)
        .into_iter()
        .find_map(NSRunningApplication::runningApplicationWithProcessIdentifier)
    else {
        return false;
    };
    // Since macOS 14 an app comes forward only when the active app yields
    // to it.
    if let Some(mtm) = MainThreadMarker::new() {
        let me = NSApplication::sharedApplication(mtm);
        if me.respondsToSelector(sel!(yieldActivationToApplication:)) {
            me.yieldActivationToApplication(&found);
        }
    }
    found.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows)
}

#[cfg(not(target_os = "macos"))]
pub fn bring_forward(app: &Path) -> bool {
    !pids_running(app).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn build(path: &Path, stale: bool, host: Option<&str>) -> PreviewBuild {
        PreviewBuild {
            path: path.to_string_lossy().into_owned(),
            commit: "a".repeat(40),
            host: host.map(str::to_string),
            built_at: jiff::Timestamp::UNIX_EPOCH,
            stale,
        }
    }

    /// A checkout with a preview build of `key`, and `script` as its
    /// `scripts/preview`.
    fn checkout(root: &Path, key: &str, script: &str) -> PathBuf {
        let dir = root.join("target/preview").join(key);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(dir.join("llmtodo-app"), "").unwrap();
        std::fs::write(dir.join("llmtodo"), "").unwrap();
        let path = root.join("scripts/preview");
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir.join("llmtodo-app")
    }

    #[test]
    fn a_build_runs_only_when_it_is_here_and_current() {
        let tmp = tempfile::tempdir().unwrap();
        let app = checkout(tmp.path(), "LLM-1", "#!/bin/sh\n");
        let me = Some("me");
        let launch = check("LLM-1", &build(&app, false, me), me).unwrap();
        assert_eq!(launch.app, app);
        assert_eq!(launch.cli, app.with_file_name("llmtodo"));
        assert_eq!(launch.script, tmp.path().join("scripts/preview"));
        assert!(check("LLM-1", &build(&app, false, None), me).is_ok());

        assert_eq!(
            check("LLM-1", &build(&app, false, Some("other")), me),
            Err(ANOTHER_COMPUTER)
        );
        assert_eq!(check("LLM-1", &build(&app, true, me), me), Err(STALE));
        let gone = tmp.path().join("target/preview/LLM-2/llmtodo-app");
        assert_eq!(check("LLM-2", &build(&gone, false, me), me), Err(MISSING));
        std::fs::remove_file(app.with_file_name("llmtodo")).unwrap();
        assert_eq!(check("LLM-1", &build(&app, false, me), me), Err(MISSING));
        assert_eq!(
            check("LLM-1", &build(Path::new("/"), false, me), me),
            Err(MISSING)
        );
    }

    #[test]
    fn the_command_hands_the_build_to_scripts_preview() {
        let launch = Launch {
            key: "LLM-7".into(),
            app: PathBuf::from("/a b/it's/llmtodo-app"),
            cli: PathBuf::from("/a b/llmtodo"),
            script: PathBuf::from("/repo/scripts/preview"),
        };
        let cmd = command(&launch, Path::new("/tmp/run"));
        let env: HashMap<_, _> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_owned())))
            .collect();
        let get = |k: &str| env.get(k).cloned().flatten();
        assert_eq!(
            get("LLMTODO_PREVIEW_APP").unwrap(),
            r"exec '/a b/it'\''s/llmtodo-app'"
        );
        assert_eq!(get("LLMTODO_BIN").unwrap(), "/a b/llmtodo");
        assert_eq!(get("LLMTODO_PREVIEW_TICKET").unwrap(), "LLM-7");
        assert_eq!(get("TMPDIR").unwrap(), "/tmp/run");
        assert_eq!(env.get("LLMTODO_DATA_DIR"), Some(&None));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args[3], "/repo/scripts/preview");
        assert_eq!(args[4], "/tmp/run");
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out: {what}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_run_starts_opens_and_cleans_up_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let gate = tmp.path().join("quit");
        // Stands in for scripts/preview: it writes into TMPDIR, says the
        // app is open, and waits until the test lets the app quit.
        let script = format!(
            "#!/bin/sh\necho \"$LLMTODO_PREVIEW_TICKET $LLMTODO_PREVIEW_APP\" > \"$TMPDIR/seen\"\n\
             echo PREVIEW_DATA=$TMPDIR/x\nwhile [ ! -f '{}' ]; do sleep 0.05; done\necho PREVIEW_EXIT=0\n",
            gate.display()
        );
        let app = checkout(tmp.path(), "LLM-3", &script);
        let launch = check("LLM-3", &build(&app, false, None), None).unwrap();
        let previews = Previews::default();
        assert_eq!(previews.state("LLM-3"), None);
        assert_eq!(previews.open(&launch), Ok(Opened::Started));
        wait_for("running", || {
            previews.state("LLM-3") == Some(RunState::Running)
        });
        let folders = || {
            std::fs::read_dir(std::env::temp_dir().join("llmtodo-try-it"))
                .map(|d| {
                    d.filter_map(Result::ok)
                        .filter(|e| e.file_name().to_string_lossy().starts_with("LLM-3-"))
                        .map(|e| e.path())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let mine = folders();
        let seen: Vec<_> = mine.iter().filter(|f| f.join("seen").is_file()).collect();
        assert_eq!(seen.len(), 1, "{mine:?}");
        let folder = seen[0].clone();
        let text = std::fs::read_to_string(folder.join("seen")).unwrap();
        assert!(text.starts_with("LLM-3 exec '"), "{text}");
        std::fs::write(&gate, "").unwrap();
        wait_for("ended", || previews.state("LLM-3").is_none());
        assert!(!folder.exists(), "the run's folder stays: {folder:?}");
    }

    #[test]
    fn a_failed_start_shows_the_script_s_last_line() {
        let tmp = tempfile::tempdir().unwrap();
        let script = "#!/bin/sh\necho 'demo seed failed; see the log' >&2\nexit 1\n";
        let app = checkout(tmp.path(), "LLM-4", script);
        let launch = check("LLM-4", &build(&app, false, None), None).unwrap();
        let previews = Previews::default();
        assert_eq!(previews.open(&launch), Ok(Opened::Started));
        wait_for("failed", || {
            matches!(previews.state("LLM-4"), Some(RunState::Failed(_)))
        });
        assert_eq!(
            previews.state("LLM-4"),
            Some(RunState::Failed(
                "The preview did not start: demo seed failed; see the log".into()
            ))
        );
    }

    #[test]
    fn a_second_click_while_starting_does_not_start_another() {
        let tmp = tempfile::tempdir().unwrap();
        let gate = tmp.path().join("go");
        let script = format!(
            "#!/bin/sh\nwhile [ ! -f '{}' ]; do sleep 0.05; done\n",
            gate.display()
        );
        let app = checkout(tmp.path(), "LLM-5", &script);
        let launch = check("LLM-5", &build(&app, false, None), None).unwrap();
        let previews = Previews::default();
        assert_eq!(previews.open(&launch), Ok(Opened::Started));
        assert_eq!(previews.state("LLM-5"), Some(RunState::Starting));
        assert_eq!(previews.open(&launch), Ok(Opened::Starting));
        std::fs::write(&gate, "").unwrap();
        wait_for("ended", || previews.state("LLM-5").is_none());
    }
}
