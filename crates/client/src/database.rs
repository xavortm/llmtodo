//! Which database the server opens for a data folder (LLM-461).
//!
//! `config.toml` names the backend in its `[database]` table:
//!
//! ```toml
//! [database]
//! backend = "postgres"   # or "sqlite", the default
//! ```
//!
//! The connection string holds a password, so it never goes in the file,
//! which can live in a dotfiles repo. It comes from `LLMTODO_DATABASE_URL`
//! when set, else from the macOS Keychain, where `llmtodo db set-url`
//! stores it. launchd starts the server without the shell's variables, so
//! the Keychain is the normal place.
//!
//! The `[database]` choice applies only to the main data folder: the one
//! `config.toml` names, or the standard folder. A preview or a test on a
//! folder of its own always gets its own SQLite file, so it can never write
//! to the shared database by mistake.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::config::{ConfigFile, ConfigRead, from_doc, set_value, table_mut};

/// The variable that gives the connection string instead of the Keychain.
/// It never chooses Postgres on its own: `config.toml` does.
pub const DATABASE_URL_ENV: &str = "LLMTODO_DATABASE_URL";

/// The Keychain item that holds the URL: its service and account.
/// `LLMTODO_KEYCHAIN_SERVICE` names another service, for a test that must
/// not touch the real item.
pub const KEYCHAIN_SERVICE: &str = "llmtodo-database";
pub const KEYCHAIN_ACCOUNT: &str = "default";

fn service() -> String {
    std::env::var("LLMTODO_KEYCHAIN_SERVICE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| KEYCHAIN_SERVICE.to_string())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Sqlite,
    Postgres,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Sqlite => "sqlite",
            Backend::Postgres => "postgres",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "sqlite" => Some(Backend::Sqlite),
            "postgres" | "postgresql" => Some(Backend::Postgres),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct DatabaseTable {
    #[serde(default)]
    backend: Backend,
}

#[derive(Debug, Default, Deserialize)]
struct WithDatabase {
    #[serde(default)]
    database: DatabaseTable,
}

/// The backend `config.toml` names. A missing or broken file reads as
/// SQLite.
pub fn configured_backend() -> Backend {
    let Some(file) = ConfigFile::default_location() else {
        return Backend::Sqlite;
    };
    match file.read() {
        ConfigRead::Parsed(doc) => from_doc::<WithDatabase>(&doc)
            .map(|c| c.database.backend)
            .unwrap_or_default(),
        _ => Backend::Sqlite,
    }
}

/// Writes `backend` into the `[database]` table of `config.toml`, keeping
/// the rest of the file as it is.
pub fn set_configured_backend(backend: Backend) -> Result<PathBuf, String> {
    let file = ConfigFile::default_location()
        .ok_or_else(|| "no config folder: set HOME or LLMTODO_CONFIG_DIR".to_string())?;
    file.update(|doc| {
        let table = table_mut(doc, "database");
        set_value(table, "backend", backend.as_str().into());
    })
    .map_err(|e| format!("write {}: {e}", file.path().display()))?;
    Ok(file.path().to_path_buf())
}

/// The data folder that `[database]` applies to: `data_dir` from the config,
/// else the standard folder.
pub fn main_data_dir() -> PathBuf {
    crate::data_dir::global_config()
        .data_dir
        .unwrap_or_else(crate::data_dir::standard_data_dir)
}

fn same_folder(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

/// The database a server for `data_dir` opens.
#[derive(Clone, PartialEq, Eq)]
pub enum Choice {
    Sqlite,
    Postgres { url: String },
}

impl std::fmt::Debug for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Choice::Sqlite => f.write_str("Sqlite"),
            Choice::Postgres { .. } => f.write_str("Postgres(<url>)"),
        }
    }
}

/// Picks the database for a server on `data_dir`. An error means the config
/// asks for Postgres but no URL is stored: the server must not fall back to
/// the SQLite file on its own.
pub fn choose(data_dir: &Path) -> Result<Choice, String> {
    if configured_backend() != Backend::Postgres || !same_folder(data_dir, &main_data_dir()) {
        return Ok(Choice::Sqlite);
    }
    match stored_url()? {
        Some(url) => Ok(Choice::Postgres { url }),
        None => Err(format!(
            "config.toml sets [database] backend = \"postgres\", but no connection string is stored; run `llmtodo db set-url` or set {DATABASE_URL_ENV}"
        )),
    }
}

/// The connection string: `LLMTODO_DATABASE_URL`, else the Keychain item.
pub fn stored_url() -> Result<Option<String>, String> {
    if let Some(url) = std::env::var(DATABASE_URL_ENV)
        .ok()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
    {
        return Ok(Some(url));
    }
    keychain_url()
}

/// True when the `[database]` choice applies to `data_dir`.
pub fn applies_to(data_dir: &Path) -> bool {
    same_folder(data_dir, &main_data_dir())
}

/// The URL stored in the Keychain, if any.
pub fn keychain_url() -> Result<Option<String>, String> {
    let out = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            &service(),
            "-a",
            KEYCHAIN_ACCOUNT,
            "-w",
        ])
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("run `security`: {e}"))?;
    if !out.status.success() {
        // Exit code 44: the item does not exist.
        return Ok(None);
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok((!url.is_empty()).then_some(url))
}

/// Stores `url` in the Keychain, replacing an older one. The URL goes to
/// `security` on stdin, so it never shows in the process list.
pub fn store_keychain_url(url: &str) -> Result<(), String> {
    if url.contains('"') || url.contains('\n') || url.contains('\\') {
        return Err("the connection string holds a quote, a backslash or a line break".into());
    }
    let mut child = Command::new("security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("run `security`: {e}"))?;
    let line = format!(
        "add-generic-password -U -s {} -a {KEYCHAIN_ACCOUNT} -l \"llmtodo shared database\" -w \"{url}\"\n",
        service()
    );
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(line.as_bytes())
        .map_err(|e| format!("write to `security`: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait for `security`: {e}"))?;
    if !out.status.success() || !out.stderr.is_empty() {
        return Err(format!(
            "the Keychain refused the item: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Removes the stored URL. A missing item is fine.
pub fn delete_keychain_url() -> Result<(), String> {
    Command::new("security")
        .args([
            "delete-generic-password",
            "-s",
            &service(),
            "-a",
            KEYCHAIN_ACCOUNT,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("run `security`: {e}"))?;
    Ok(())
}

/// Checks the shape of a Postgres URL before it is stored. The server tests
/// the connection itself.
pub fn check_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
        return Err("a Postgres connection string starts with postgres:// or postgresql://".into());
    }
    if !url.contains('@') {
        return Err(
            "the connection string needs a user and host, as in postgres://user:password@host/db"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backends_parse_in_any_case() {
        assert_eq!(Backend::parse("Postgres"), Some(Backend::Postgres));
        assert_eq!(Backend::parse("postgresql"), Some(Backend::Postgres));
        assert_eq!(Backend::parse("sqlite"), Some(Backend::Sqlite));
        assert_eq!(Backend::parse("mysql"), None);
    }

    #[test]
    fn urls_need_the_postgres_scheme_and_a_host() {
        assert!(check_url("postgres://u:p@ep-x.neon.tech/db?sslmode=require").is_ok());
        assert!(check_url("postgresql://u@localhost/db").is_ok());
        assert!(check_url("mysql://u@h/db").is_err());
        assert!(check_url("postgres://localhost").is_err());
    }

    /// Writes, reads and deletes a Keychain item under a test service. It
    /// touches the login Keychain, so it runs only on request:
    /// `cargo test -p llmtodo-client keychain -- --ignored`.
    #[test]
    #[ignore = "writes to the login Keychain"]
    fn the_keychain_keeps_the_url() {
        // SAFETY: the only test that reads this variable.
        unsafe { std::env::set_var("LLMTODO_KEYCHAIN_SERVICE", "llmtodo-database-test") };
        let url = "postgres://u:p@ep-x.example.com/db?sslmode=require";
        store_keychain_url(url).unwrap();
        assert_eq!(keychain_url().unwrap().as_deref(), Some(url));
        delete_keychain_url().unwrap();
        assert_eq!(keychain_url().unwrap(), None);
    }

    #[test]
    fn a_folder_of_its_own_never_gets_the_shared_database() {
        // A folder other than the main one is SQLite whatever the config
        // and the variable say.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(choose(dir.path()).unwrap(), Choice::Sqlite);
    }
}
