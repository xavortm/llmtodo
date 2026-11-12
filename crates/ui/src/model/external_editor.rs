//! `Shift+E`: edit a description in `$EDITOR`, such as nvim.
//!
//! The app writes the text to one file per task in a temporary folder and
//! opens the editor on it. Inside herdr, the editor runs in a new pane next
//! to the app's pane. Outside herdr, it runs in a new Terminal window. The
//! workspace polls the file and saves the text each time the editor writes
//! it, with the same version check as the built-in editor.
//!
//! The launcher gives no handle on the editor process: Terminal and herdr
//! run the command on their own. So the shell line touches a marker file
//! after the editor exits, and the next poll ends the session. A session
//! also ends when its file is gone, or when the workspace says its task
//! closed or went to the Trash. An ended session deletes both files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use gpui_shared_string::SharedString;

use crate::model::data::IssueId;

/// The editor command: `$VISUAL`, then `$EDITOR`, then `vi`.
pub fn editor_command() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// The file for one task. Keys are letters, digits and `-`; anything else
/// becomes `_`, so a key never leaves the folder.
pub fn file_for(dir: &Path, key: &str) -> PathBuf {
    let name: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{name}.md"))
}

/// The file the shell line touches once the editor exits.
pub fn done_marker(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".done");
    PathBuf::from(name)
}

/// An editor that returns this soon after the open did not wait for its
/// window to close, such as `code` without `--wait`. Its session stays
/// open until its task closes.
pub const QUICK_EXIT: Duration = Duration::from_secs(3);

/// The folder for the files: `llmtodo-edit` in the user's temporary folder.
pub fn default_dir() -> PathBuf {
    std::env::temp_dir().join("llmtodo-edit")
}

/// Wraps `s` in single quotes for a POSIX shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The shell line that opens `path` in `editor`, then touches the done
/// marker when the editor exits. The editor command may carry its own
/// arguments, such as `code --wait`, so it stays unquoted.
pub fn shell_line(editor: &str, path: &Path) -> String {
    format!(
        "{editor} {}; touch {}",
        shell_quote(&path.to_string_lossy()),
        shell_quote(&done_marker(path).to_string_lossy())
    )
}

/// Where the editor runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A new herdr pane, split from the app's pane.
    Herdr { bin: String, pane: String },
    /// A new Terminal window.
    Terminal,
}

impl Place {
    /// herdr when the app runs inside a herdr pane, else Terminal.
    pub fn from_env() -> Self {
        let herdr = std::env::var("HERDR_ENV").is_ok_and(|v| v == "1");
        match std::env::var("HERDR_PANE_ID") {
            Ok(pane) if herdr && !pane.is_empty() => Place::Herdr {
                bin: std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into()),
                pane,
            },
            _ => Place::Terminal,
        }
    }
}

/// Escapes `s` for a double-quoted AppleScript string.
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

/// The `osascript` call that runs `line` in a new Terminal window.
pub fn terminal_command(line: &str) -> Command {
    let mut command = Command::new("osascript");
    command
        .arg("-e")
        .arg("tell application \"Terminal\" to activate")
        .arg("-e")
        .arg(format!(
            "tell application \"Terminal\" to do script {}",
            applescript_string(line)
        ));
    command
}

/// The pane ID in the JSON that `herdr pane split` prints.
pub fn split_pane_id(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let pane = value.pointer("/result/pane")?;
    pane.get("pane_id")
        .or_else(|| pane.get("id"))?
        .as_str()
        .map(str::to_string)
}

/// Opens `path` in `editor` at `place`. It blocks until the terminal or
/// the pane has the command, not until the editor exits, so callers run it
/// off the UI thread.
pub fn launch(place: &Place, editor: &str, path: &Path) -> Result<(), String> {
    let line = shell_line(editor, path);
    match place {
        Place::Terminal => run(terminal_command(&line)).map(|_| ()),
        Place::Herdr { bin, pane } => {
            let cwd = path.parent().unwrap_or(Path::new("/"));
            let mut split = Command::new(bin);
            split
                .args(["pane", "split", pane, "--direction", "right", "--focus"])
                .arg("--cwd")
                .arg(cwd);
            let out = run(split)?;
            let new_pane = split_pane_id(&out)
                .ok_or_else(|| format!("herdr pane split printed no pane ID: {out}"))?;
            let mut command = Command::new(bin);
            command.args(["pane", "run", &new_pane, &line]);
            run(command).map(|_| ())
        }
    }
}

fn run(mut command: Command) -> Result<String, String> {
    let out = command.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The file's last change time, or None when it is gone.
pub fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Opens a file in `$EDITOR`. Tests swap it for one that opens nothing.
pub type Launcher = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// A description open in `$EDITOR`: its file, and what the app last saw.
struct Session {
    id: IssueId,
    path: PathBuf,
    /// The description version the next save builds on.
    base: i64,
    /// The file's change time when the app last read or wrote it.
    modified: Option<SystemTime>,
    /// The text the app last read or wrote, so a write that changes
    /// nothing saves nothing.
    text: String,
    /// When the app wrote the file and started the editor.
    opened: SystemTime,
    /// The editor returned at once, so its exit does not end the session.
    detached: bool,
}

/// What one poll found.
#[derive(Default)]
pub struct Poll {
    pub saves: Vec<ExternalSave>,
    /// The sessions that ended, by task key.
    pub ended: Vec<SharedString>,
    /// The sessions whose editor returned at once, by task key.
    pub detached: Vec<SharedString>,
}

impl Poll {
    /// Whether the poll found nothing, so nothing needs a redraw.
    pub fn is_empty(&self) -> bool {
        self.saves.is_empty() && self.ended.is_empty() && self.detached.is_empty()
    }
}

/// A save the editor asked for by writing its file.
pub struct ExternalSave {
    pub key: SharedString,
    pub id: IssueId,
    pub text: String,
    pub base: i64,
}

/// The descriptions open in `$EDITOR`, by task key.
pub struct ExternalEdits {
    sessions: HashMap<SharedString, Session>,
    /// The folder for the `$EDITOR` files.
    dir: PathBuf,
    launcher: Launcher,
}

impl ExternalEdits {
    /// Sessions in the default folder, opened where the app runs.
    pub fn from_env() -> Self {
        let place = Place::from_env();
        Self::new(
            default_dir(),
            Arc::new(move |path: &Path| launch(&place, &editor_command(), path)),
        )
    }

    pub fn new(dir: PathBuf, launcher: Launcher) -> Self {
        Self {
            sessions: HashMap::new(),
            dir,
            launcher,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn launcher(&self) -> Launcher {
        self.launcher.clone()
    }

    /// Writes `text` to the task's file and starts a session on it.
    pub fn open(
        &mut self,
        key: SharedString,
        id: IssueId,
        text: String,
        version: i64,
    ) -> Result<PathBuf, String> {
        let path = file_for(&self.dir, &key);
        // A marker left by an earlier session would end this one at once.
        let _ = std::fs::remove_file(done_marker(&path));
        std::fs::create_dir_all(&self.dir)
            .and_then(|_| std::fs::write(&path, &text))
            .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
        let session = Session {
            id,
            path: path.clone(),
            base: version,
            modified: modified(&path),
            text,
            opened: SystemTime::now(),
            detached: false,
        };
        self.sessions.insert(key, session);
        Ok(path)
    }

    /// The saves for each file the editor wrote since the last check, and
    /// the sessions that ended. A session ends when its file is gone, when
    /// its editor exited, or when `keep` refuses its task. One `stat` per
    /// session, and one more for the done marker.
    pub fn poll(&mut self, keep: impl Fn(IssueId) -> bool) -> Poll {
        let mut poll = Poll::default();
        self.sessions.retain(|key, session| {
            let Some(now) = modified(&session.path) else {
                poll.ended.push(key.clone());
                end(session);
                return false;
            };
            if !keep(session.id) {
                poll.ended.push(key.clone());
                end(session);
                return false;
            }
            if Some(now) != session.modified {
                session.modified = Some(now);
                if let Ok(text) = std::fs::read_to_string(&session.path)
                    && text != session.text
                {
                    session.text = text.clone();
                    poll.saves.push(ExternalSave {
                        key: key.clone(),
                        id: session.id,
                        text,
                        base: session.base,
                    });
                }
            }
            // The editor writes before it exits, so the read above holds
            // its last text.
            let marker = done_marker(&session.path);
            if let Some(done) = modified(&marker) {
                let quick = done
                    .duration_since(session.opened)
                    .is_ok_and(|d| d < QUICK_EXIT);
                if quick && !session.detached {
                    session.detached = true;
                    let _ = std::fs::remove_file(&marker);
                    poll.detached.push(key.clone());
                } else if !session.detached {
                    poll.ended.push(key.clone());
                    end(session);
                    return false;
                }
            }
            true
        });
        poll
    }

    /// A description save reached the server. A save of the `$EDITOR` text
    /// moves its base on. A save from the built-in editor does not, so a
    /// later write from `$EDITOR` cannot overwrite it unseen.
    pub fn saved(&mut self, key: &str, sent: Option<&str>, version: i64) {
        if let Some(session) = self.sessions.get_mut(key)
            && sent == Some(session.text.as_str())
        {
            session.base = version;
        }
    }

    /// Puts the server's version in the open file, so the next write from
    /// the editor builds on it.
    pub fn take_server_text(&mut self, key: &str, text: String, version: i64) {
        if let Some(session) = self.sessions.get_mut(key)
            && std::fs::write(&session.path, &text).is_ok()
        {
            session.modified = modified(&session.path);
            session.text = text;
            session.base = version;
        }
    }
}

/// Deletes an ended session's file and its done marker.
fn end(session: &Session) {
    let _ = std::fs::remove_file(&session.path);
    let _ = std::fs::remove_file(done_marker(&session.path));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_stay_in_the_folder() {
        let dir = Path::new("/tmp/x");
        assert_eq!(file_for(dir, "LLM-7"), Path::new("/tmp/x/LLM-7.md"));
        assert_eq!(file_for(dir, "../a b"), Path::new("/tmp/x/___a_b.md"));
    }

    #[test]
    fn the_shell_line_quotes_the_path_only() {
        let line = shell_line("code --wait", Path::new("/tmp/it's here/LLM-7.md"));
        assert_eq!(
            line,
            r"code --wait '/tmp/it'\''s here/LLM-7.md'; touch '/tmp/it'\''s here/LLM-7.md.done'"
        );
    }

    fn edits(dir: &Path) -> ExternalEdits {
        ExternalEdits::new(dir.to_path_buf(), Arc::new(|_: &Path| Ok(())))
    }

    /// Writes `text` with a change time `secs` after now, as an editor would.
    fn write_later(path: &Path, text: &str, secs: u64) {
        std::fs::write(path, text).unwrap();
        let later = SystemTime::now() + Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(later)
            .unwrap();
    }

    const ID: IssueId = IssueId::from_u128(7);

    #[test]
    fn an_idle_poll_finds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut edits = edits(dir.path());
        edits.open("LLM-7".into(), ID, "Text\n".into(), 1).unwrap();
        assert!(edits.poll(|_| true).is_empty());
        assert!(!edits.is_empty());
    }

    #[test]
    fn the_editor_exit_saves_its_last_write_and_ends_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut edits = edits(dir.path());
        let path = edits.open("LLM-7".into(), ID, "Text\n".into(), 1).unwrap();
        // The last write and the exit land between two polls.
        write_later(&path, "Last words\n", 1);
        write_later(&done_marker(&path), "", 10);
        let poll = edits.poll(|_| true);
        assert_eq!(poll.saves.len(), 1);
        assert_eq!(poll.saves[0].text, "Last words\n");
        assert_eq!(poll.ended, ["LLM-7"]);
        assert!(edits.is_empty());
        assert!(!path.exists());
        assert!(!done_marker(&path).exists());
    }

    #[test]
    fn an_editor_that_returns_at_once_keeps_its_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut edits = edits(dir.path());
        let path = edits.open("LLM-7".into(), ID, "Text\n".into(), 1).unwrap();
        std::fs::write(done_marker(&path), "").unwrap();
        let poll = edits.poll(|_| true);
        assert_eq!(poll.detached, ["LLM-7"]);
        assert!(poll.ended.is_empty());
        // Its writes still save, and its marker no longer counts.
        write_later(&path, "From the GUI editor\n", 1);
        write_later(&done_marker(&path), "", 20);
        let poll = edits.poll(|_| true);
        assert_eq!(poll.saves.len(), 1);
        assert!(poll.ended.is_empty() && poll.detached.is_empty());
        assert!(!edits.is_empty());
    }

    #[test]
    fn a_session_ends_when_its_task_goes_or_its_file_does() {
        let dir = tempfile::tempdir().unwrap();
        let mut edits = edits(dir.path());
        let path = edits.open("LLM-7".into(), ID, "Text\n".into(), 1).unwrap();
        let poll = edits.poll(|id| id != ID);
        assert_eq!(poll.ended, ["LLM-7"]);
        assert!(!path.exists());

        let path = edits.open("LLM-7".into(), ID, "Text\n".into(), 1).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(edits.poll(|_| true).ended, ["LLM-7"]);
        assert!(edits.is_empty());
    }

    #[test]
    fn a_new_open_clears_an_old_marker() {
        let dir = tempfile::tempdir().unwrap();
        let mut edits = edits(dir.path());
        let path = file_for(dir.path(), "LLM-7");
        write_later(&done_marker(&path), "", 30);
        edits.open("LLM-7".into(), ID, "Text\n".into(), 1).unwrap();
        assert!(edits.poll(|_| true).is_empty());
    }

    #[test]
    fn terminal_gets_an_escaped_script() {
        let command = terminal_command(r#"nvim '/tmp/a "b"\c.md'"#);
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(command.get_program(), "osascript");
        assert_eq!(
            args[3],
            r#"tell application "Terminal" to do script "nvim '/tmp/a \"b\"\\c.md'""#
        );
    }

    #[test]
    fn reads_the_new_pane_from_herdr() {
        let json =
            r#"{"id":"cli:pane:split","result":{"pane":{"pane_id":"wF:p9"},"type":"pane_split"}}"#;
        assert_eq!(split_pane_id(json).as_deref(), Some("wF:p9"));
        assert_eq!(split_pane_id("not json"), None);
    }
}
