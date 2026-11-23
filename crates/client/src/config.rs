//! `config.toml`, the settings file a person can read, edit by hand and
//! keep in git with a dotfiles manager. The CLI and the app both use this
//! module, so they read the same file.
//!
//! The folder is the first of these:
//!
//! 1. `LLMTODO_CONFIG_DIR`, when it is not empty.
//! 2. `$XDG_CONFIG_HOME/llmtodo`, when `XDG_CONFIG_HOME` is not empty.
//! 3. `~/.config/llmtodo`, also on macOS.
//!
//! The rules for writing, so the app never fights the person's setup:
//!
//! - A missing file is fine. Nothing here creates one on read.
//! - [`ConfigFile::update`] edits the document in place with `toml_edit`,
//!   so comments, key order and formatting survive.
//! - The write follows a symlink, so a dotfiles manager's link survives.
//! - A read-only file stays as it is: the write fails with
//!   `PermissionDenied`.
//! - A file that does not parse is never overwritten.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use toml_edit;
use toml_edit::DocumentMut;

/// The environment variable that names the config folder.
pub const CONFIG_DIR_ENV: &str = "LLMTODO_CONFIG_DIR";

/// The file name inside the config folder.
pub const CONFIG_FILE: &str = "config.toml";

/// The config folder, see the module docs. `None` only when no rule names
/// a folder, that is with no `HOME`.
pub fn config_dir() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty());
    pick_dir(
        var(CONFIG_DIR_ENV).map(PathBuf::from),
        var("XDG_CONFIG_HOME").map(PathBuf::from),
        var("HOME").map(PathBuf::from),
    )
}

/// The path of `config.toml`, see [`config_dir`].
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(CONFIG_FILE))
}

fn pick_dir(
    explicit: Option<PathBuf>,
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    explicit
        .or_else(|| xdg.map(|x| x.join("llmtodo")))
        .or_else(|| home.map(|h| h.join(".config/llmtodo")))
}

/// Why `config.toml` did not parse, for the Settings view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line of the error, when the parser names one.
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ParseError {}

/// What a read of `config.toml` found.
#[derive(Debug, Clone)]
pub enum ConfigRead {
    Missing,
    Parsed(DocumentMut),
    Invalid(ParseError),
}

/// Why [`ConfigFile::update`] wrote nothing.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("settings file does not parse: {0}")]
    Invalid(ParseError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl WriteError {
    /// True for a file or folder the app may not write.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::Io(e) if e.kind() == io::ErrorKind::PermissionDenied)
    }
}

/// One `config.toml` on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// The file at [`config_path`].
    pub fn default_location() -> Option<Self> {
        config_path().map(Self::at)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True when the file, or the target of its link, exists.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// The modification time of the file, or of the target of its link.
    /// A cheap check for a change made outside the app.
    pub fn modified(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
    }

    pub fn read(&self) -> ConfigRead {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => match parse(&text) {
                Ok(doc) => ConfigRead::Parsed(doc),
                Err(err) => ConfigRead::Invalid(err),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => ConfigRead::Missing,
            Err(err) => ConfigRead::Invalid(ParseError {
                line: None,
                message: err.to_string(),
            }),
        }
    }

    /// Reads the file, lets `edit` change the document, and writes it back
    /// when the text changed. A missing file starts empty. A file that does
    /// not parse is left alone.
    pub fn update(&self, edit: impl FnOnce(&mut DocumentMut)) -> Result<(), WriteError> {
        let (before, mut doc) = match std::fs::read_to_string(&self.path) {
            Ok(text) => {
                let doc = parse(&text).map_err(WriteError::Invalid)?;
                (Some(text), doc)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => (None, DocumentMut::new()),
            Err(err) => return Err(err.into()),
        };
        edit(&mut doc);
        let after = doc.to_string();
        if before.as_deref() == Some(after.as_str()) {
            return Ok(());
        }
        write_through_links(&self.path, after.as_bytes())?;
        Ok(())
    }
}

/// Reads `doc` as `T`, the way serde reads any TOML. A key of the wrong
/// type is an error with its line.
pub fn from_doc<T: serde::de::DeserializeOwned>(doc: &DocumentMut) -> Result<T, ParseError> {
    let text = doc.to_string();
    toml::from_str(&text).map_err(|err| ParseError {
        line: err.span().map(|span| line_of(&text, span.start)),
        message: err.message().to_string(),
    })
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// Sets `key` in `table` to `value`. A value already there keeps its
/// spacing and its trailing comment.
pub fn set_value(table: &mut toml_edit::Table, key: &str, mut value: toml_edit::Value) {
    if let Some(old) = table.get(key).and_then(toml_edit::Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    } else {
        value.decor_mut().clear();
    }
    table[key] = toml_edit::Item::Value(value);
}

/// The `[name]` table of `doc`, created at the end when missing. A key of
/// that name that is not a table is replaced.
pub fn table_mut<'a>(doc: &'a mut DocumentMut, name: &str) -> &'a mut toml_edit::Table {
    if !doc.get(name).is_some_and(toml_edit::Item::is_table) {
        doc[name] = toml_edit::table();
    }
    doc[name].as_table_mut().expect("just made a table")
}

fn parse(text: &str) -> Result<DocumentMut, ParseError> {
    text.parse::<DocumentMut>().map_err(|err| ParseError {
        line: err.span().map(|span| line_of(text, span.start)),
        message: err.message().to_string(),
    })
}

/// The file a write to `path` must change: the target when `path` is a
/// symlink, even one whose target does not exist yet.
fn write_target(path: &Path) -> io::Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(target) => Ok(target),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            match std::fs::read_link(path) {
                // A dangling link: write where it points.
                Ok(link) => Ok(path.parent().unwrap_or(Path::new(".")).join(link)),
                Err(_) => Ok(path.to_path_buf()),
            }
        }
        Err(err) => Err(err),
    }
}

/// Writes a temp file next to the real file and renames it over, so a
/// crash never leaves half a file. The real file keeps its permissions,
/// and a read-only one is refused, because a rename would replace it.
fn write_through_links(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = write_target(path)?;
    let permissions = match std::fs::metadata(&target) {
        Ok(meta) if meta.permissions().readonly() => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is read-only", target.display()),
            ));
        }
        Ok(meta) => Some(meta.permissions()),
        Err(_) => None,
    };
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = target.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    if let Some(permissions) = permissions {
        std::fs::set_permissions(&tmp, permissions)?;
    }
    std::fs::rename(&tmp, &target).inspect_err(|_| {
        std::fs::remove_file(&tmp).ok();
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_rule_that_names_a_folder_wins() {
        let p = |s: &str| Some(PathBuf::from(s));
        assert_eq!(pick_dir(p("/env"), p("/xdg"), p("/home")), p("/env"));
        assert_eq!(pick_dir(None, p("/xdg"), p("/home")), p("/xdg/llmtodo"));
        assert_eq!(pick_dir(None, None, p("/home")), p("/home/.config/llmtodo"));
        assert_eq!(pick_dir(None, None, None), None);
    }

    fn set_theme(file: &ConfigFile, value: &str) -> Result<(), WriteError> {
        file.update(|doc| set_value(table_mut(doc, "app"), "theme", value.into()))
    }

    #[test]
    fn a_missing_file_reads_as_missing_and_a_write_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::at(dir.path().join("sub/config.toml"));
        assert!(matches!(file.read(), ConfigRead::Missing));
        assert!(!file.exists());
        set_theme(&file, "light").unwrap();
        let ConfigRead::Parsed(doc) = file.read() else {
            panic!("parsed")
        };
        assert_eq!(doc["app"]["theme"].as_str(), Some("light"));
    }

    #[test]
    fn an_update_keeps_comments_order_and_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "# My llmtodo setup\ndata_dir = \"/x\"\n\n[app]\n# dark, please\ntheme = \"dark\" # trailing\nzoom = 1.1\nfuture_key = [1, 2]\n";
        std::fs::write(&path, text).unwrap();
        set_theme(&ConfigFile::at(path.clone()), "light").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text.replace("\"dark\" #", "\"light\" #")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_survives_and_its_target_changes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dotfiles/config.toml");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, "[app]\ntheme = \"dark\"\n").unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        set_theme(&ConfigFile::at(link.clone()), "light").unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "[app]\ntheme = \"light\"\n"
        );
    }

    #[test]
    fn a_read_only_file_keeps_its_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[app]\ntheme = \"dark\"\n").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();
        let err = set_theme(&ConfigFile::at(path.clone()), "light").unwrap_err();
        assert!(err.is_read_only(), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[app]\ntheme = \"dark\"\n"
        );
    }

    #[test]
    fn a_file_that_does_not_parse_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[app]\ntheme = \"dark\"\nzoom = = 2\n";
        std::fs::write(&path, text).unwrap();
        let file = ConfigFile::at(path.clone());
        let ConfigRead::Invalid(err) = file.read() else {
            panic!("invalid")
        };
        assert_eq!(err.line, Some(3), "{err}");
        assert!(matches!(
            set_theme(&file, "light"),
            Err(WriteError::Invalid(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
}
