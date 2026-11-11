//! `llmtodo skill`: installs the agent skill in `~/.claude/skills/llmtodo`.
//!
//! The skill files live in this repo in `.claude/skills/llmtodo`. The binary
//! embeds them, so the repo skill and the installed skill come from one
//! source. A manifest next to them records a hash of each file this command
//! wrote. A file that does not match its recorded hash is not ours, so the
//! command keeps it, or backs it up first with `--force`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::{App, CliError, CliResult};

/// The skill files, embedded from the repo skill.
pub const FILES: &[(&str, &str)] = &[
    (
        "SKILL.md",
        include_str!("../../../../.claude/skills/llmtodo/SKILL.md"),
    ),
    (
        "guide.md",
        include_str!("../../../../.claude/skills/llmtodo/guide.md"),
    ),
];

const MANIFEST: &str = ".llmtodo-skill.json";

#[derive(Subcommand)]
pub enum SkillCmd {
    /// Install or update the skill in ~/.claude/skills/llmtodo. Keeps files
    /// that llmtodo did not write, unless --force.
    Install {
        /// Back up files that llmtodo did not write, then replace them.
        #[arg(long)]
        force: bool,
    },
    /// Show where the skill goes and the state of each file.
    Status,
    /// Print one skill file, such as SKILL.md.
    Print {
        #[arg(default_value = "SKILL.md")]
        file: String,
    },
}

pub async fn run(app: &App, cmd: SkillCmd) -> CliResult {
    match cmd {
        SkillCmd::Install { force } => {
            let report = install(&skill_dir()?, force)?;
            app.out.ser(&report);
            if report.kept() > 0 {
                return Err(CliError::Other(report.kept_message()));
            }
            Ok(())
        }
        SkillCmd::Status => {
            app.out.ser(&status(&skill_dir()?));
            Ok(())
        }
        SkillCmd::Print { file } => {
            let (_, text) = FILES
                .iter()
                .find(|(name, _)| *name == file)
                .ok_or_else(|| {
                    let names: Vec<&str> = FILES.iter().map(|(n, _)| *n).collect();
                    CliError::Usage(format!(
                        "no skill file {file:?}; files: {}",
                        names.join(", ")
                    ))
                })?;
            print!("{text}");
            Ok(())
        }
    }
}

/// `$CLAUDE_CONFIG_DIR/skills/llmtodo`, or `~/.claude/skills/llmtodo`.
pub fn skill_dir() -> CliResult<PathBuf> {
    let base = match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(
            std::env::var_os("HOME").ok_or_else(|| CliError::Other("HOME is not set".into()))?,
        )
        .join(".claude"),
    };
    Ok(base.join("skills").join("llmtodo"))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    version: String,
    /// File name to the hash of the content this command wrote.
    #[serde(default)]
    files: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct FileReport {
    pub file: String,
    /// `installed`, `updated`, `unchanged`, `replaced` or `kept`.
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct InstallReport {
    pub dir: PathBuf,
    pub files: Vec<FileReport>,
}

impl InstallReport {
    pub fn kept(&self) -> usize {
        self.files.iter().filter(|f| f.action == "kept").count()
    }

    pub fn kept_message(&self) -> String {
        format!(
            "kept {} file(s) in {} that llmtodo did not write; pass --force to back them up and replace them",
            self.kept(),
            self.dir.display()
        )
    }
}

/// FNV-1a, 64 bit. Stable across Rust versions, unlike `DefaultHasher`.
fn hash(text: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn read_manifest(dir: &Path) -> Manifest {
    std::fs::read_to_string(dir.join(MANIFEST))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn is_symlink(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// A free backup name next to `path`: `<name>.bak-<seconds>`, plus a
/// counter when that name is taken.
fn backup_path(path: &Path) -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut candidate = path.with_file_name(format!("{name}.bak-{secs}"));
    let mut n = 1;
    while std::fs::symlink_metadata(&candidate).is_ok() {
        candidate = path.with_file_name(format!("{name}.bak-{secs}-{n}"));
        n += 1;
    }
    candidate
}

/// Writes the skill into `dir`. Never overwrites a file it did not write,
/// unless `force`, and then it renames the old file to a backup first.
pub fn install(dir: &Path, force: bool) -> CliResult<InstallReport> {
    let mut files = Vec::new();
    // A linked skill folder belongs to someone else. Writing through the
    // link would change files outside this folder.
    if is_symlink(dir) {
        if !force {
            return Ok(InstallReport {
                dir: dir.to_path_buf(),
                files: FILES
                    .iter()
                    .map(|(f, _)| FileReport {
                        file: (*f).to_string(),
                        action: "kept",
                        backup: None,
                    })
                    .collect(),
            });
        }
        let backup = backup_path(dir);
        std::fs::rename(dir, &backup)?;
        files.push(FileReport {
            file: ".".into(),
            action: "replaced",
            backup: Some(backup),
        });
    }
    std::fs::create_dir_all(dir)?;
    let mut manifest = read_manifest(dir);
    for (name, text) in FILES {
        let path = dir.join(name);
        let new_hash = hash(text.as_bytes());
        let link = is_symlink(&path);
        let existing = if link {
            None
        } else {
            std::fs::read(&path).ok()
        };
        let action = match &existing {
            None if !link => "installed",
            Some(old) if old == text.as_bytes() => "unchanged",
            Some(old) if manifest.files.get(*name) == Some(&hash(old)) => "updated",
            _ if force => "replaced",
            _ => "kept",
        };
        let mut backup = None;
        match action {
            "kept" => {}
            "unchanged" => {
                manifest.files.insert((*name).to_string(), new_hash);
            }
            _ => {
                if action == "replaced" {
                    let b = backup_path(&path);
                    std::fs::rename(&path, &b)?;
                    backup = Some(b);
                }
                std::fs::write(&path, text)?;
                manifest.files.insert((*name).to_string(), new_hash);
            }
        }
        files.push(FileReport {
            file: (*name).to_string(),
            action,
            backup,
        });
    }
    manifest.version = llmtodo_api_types::VERSION.to_string();
    std::fs::write(
        dir.join(MANIFEST),
        serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n",
    )?;
    Ok(InstallReport {
        dir: dir.to_path_buf(),
        files,
    })
}

#[derive(Serialize)]
struct FileStatus {
    file: String,
    /// `missing`, `current`, `outdated` (ours, older) or `foreign`.
    state: &'static str,
}

#[derive(Serialize)]
struct Status {
    dir: PathBuf,
    files: Vec<FileStatus>,
}

fn status(dir: &Path) -> Status {
    let manifest = read_manifest(dir);
    let linked = is_symlink(dir);
    let files = FILES
        .iter()
        .map(|(name, text)| {
            let path = dir.join(name);
            let state = if linked || is_symlink(&path) {
                if path.exists() { "foreign" } else { "missing" }
            } else {
                match std::fs::read(&path) {
                    Err(_) => "missing",
                    Ok(old) if old == text.as_bytes() => "current",
                    Ok(old) if manifest.files.get(*name) == Some(&hash(&old)) => "outdated",
                    Ok(_) => "foreign",
                }
            };
            FileStatus {
                file: (*name).to_string(),
                state,
            }
        })
        .collect();
    Status {
        dir: dir.to_path_buf(),
        files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(dir: &Path, f: &str) -> String {
        std::fs::read_to_string(dir.join(f)).unwrap()
    }

    #[test]
    fn installs_updates_and_keeps_foreign_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills/llmtodo");

        let r = install(&dir, false).unwrap();
        assert!(r.files.iter().all(|f| f.action == "installed"));
        assert_eq!(read(&dir, "SKILL.md"), FILES[0].1);

        let r = install(&dir, false).unwrap();
        assert!(r.files.iter().all(|f| f.action == "unchanged"));

        // An older version that llmtodo wrote is updated in place.
        let old = "old skill\n";
        std::fs::write(dir.join("SKILL.md"), old).unwrap();
        let mut m = read_manifest(&dir);
        m.files.insert("SKILL.md".into(), hash(old.as_bytes()));
        std::fs::write(dir.join(MANIFEST), serde_json::to_string(&m).unwrap()).unwrap();
        let r = install(&dir, false).unwrap();
        assert_eq!(r.files[0].action, "updated");
        assert!(r.files[0].backup.is_none());

        // A file someone edited is kept.
        std::fs::write(dir.join("SKILL.md"), "my own notes\n").unwrap();
        let r = install(&dir, false).unwrap();
        assert_eq!(r.files[0].action, "kept");
        assert_eq!(r.kept(), 1);
        assert_eq!(read(&dir, "SKILL.md"), "my own notes\n");

        // --force backs it up, then replaces it.
        let r = install(&dir, true).unwrap();
        assert_eq!(r.files[0].action, "replaced");
        let backup = r.files[0].backup.clone().unwrap();
        assert_eq!(std::fs::read_to_string(backup).unwrap(), "my own notes\n");
        assert_eq!(read(&dir, "SKILL.md"), FILES[0].1);
    }

    #[test]
    fn a_file_without_a_manifest_entry_is_foreign() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("llmtodo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "someone else's skill\n").unwrap();
        let r = install(&dir, false).unwrap();
        assert_eq!(r.files[0].action, "kept");
        assert_eq!(r.files[1].action, "installed");
        assert_eq!(read(&dir, "SKILL.md"), "someone else's skill\n");
    }

    #[test]
    fn a_linked_folder_is_not_written_through() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let dir = tmp.path().join("llmtodo");
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        let r = install(&dir, false).unwrap();
        assert_eq!(r.kept(), FILES.len());
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);

        let r = install(&dir, true).unwrap();
        assert_eq!(r.files[0].action, "replaced");
        assert!(!is_symlink(&dir));
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    }
}
