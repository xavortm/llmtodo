//! `scripts/seed-demo` wipes its target folder. These tests check that its
//! guard refuses the real data folder in every spelling, and any folder it did
//! not make. A fake HOME stands in for the owner's home.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/seed-demo")
}

fn seed(home: &Path, target: &str) -> Output {
    Command::new("sh")
        .arg(script())
        .arg(target)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LLMTODO_BIN", "/nonexistent/llmtodo")
        .output()
        .unwrap()
}

/// A fake home with a real-looking data folder that holds a database.
fn home_with_real_folder() -> (tempfile::TempDir, PathBuf) {
    let home = tempfile::tempdir().unwrap();
    let real = home.path().join("Library/Application Support/llmtodo");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("llmtodo.db"), "owner data").unwrap();
    (home, real)
}

fn assert_refused(out: &Output, db: &Path) {
    assert!(!out.status.success(), "seed-demo did not refuse");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("refusing"), "{err}");
    assert_eq!(std::fs::read_to_string(db).unwrap(), "owner data");
}

#[test]
fn refuses_real_folder_with_trailing_slash() {
    let (home, real) = home_with_real_folder();
    let out = seed(home.path(), &format!("{}/", real.display()));
    assert_refused(&out, &real.join("llmtodo.db"));
}

#[test]
fn refuses_real_folder_with_dot() {
    let (home, real) = home_with_real_folder();
    let out = seed(home.path(), &format!("{}/.", real.display()));
    assert_refused(&out, &real.join("llmtodo.db"));
}

#[test]
fn refuses_symlink_to_real_folder() {
    let (home, real) = home_with_real_folder();
    let link = home.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let out = seed(home.path(), link.to_str().unwrap());
    assert_refused(&out, &real.join("llmtodo.db"));
}

#[test]
fn refuses_custom_data_folder_without_marker() {
    let home = tempfile::tempdir().unwrap();
    let custom = home.path().join("custom-data");
    std::fs::create_dir_all(&custom).unwrap();
    std::fs::write(custom.join("llmtodo.db"), "owner data").unwrap();
    let out = seed(home.path(), custom.to_str().unwrap());
    assert_refused(&out, &custom.join("llmtodo.db"));
}

#[test]
fn seeds_empty_folder_and_marks_it() {
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("demo");
    std::fs::create_dir_all(&target).unwrap();
    // The missing binary makes the seed fail after the guard. The guard
    // passed if the marker exists.
    let _ = seed(home.path(), target.to_str().unwrap());
    assert!(target.join(".llmtodo-demo").is_file());
    // A marked folder may be wiped again.
    std::fs::write(target.join("llmtodo.db"), "demo").unwrap();
    let out = seed(home.path(), target.to_str().unwrap());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("refusing"));
    assert!(!target.join("llmtodo.db").exists());
}

// Tests that run the seed for real, and check that no server stays behind.

const CLI: &str = env!("CARGO_BIN_EXE_llmtodo");

fn run_seed(home: &Path, target: &Path, bin: &str) -> Output {
    Command::new("sh")
        .arg(script())
        .arg(target)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LLMTODO_BIN", bin)
        .output()
        .unwrap()
}

/// The PIDs that hold the data folder's server lock.
fn lock_holders(data: &Path) -> String {
    let out = Command::new("/usr/sbin/lsof")
        .arg("-t")
        .arg(data.join("server.lock"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_finished_seed_leaves_no_server() {
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("demo");
    let out = run_seed(home.path(), &data, CLI);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(data.join("server.lock").exists(), "no server ever ran");
    // Checked at once: the seed must wait until its server has exited.
    assert_eq!(lock_holders(&data), "");
}

#[test]
fn a_failed_seed_leaves_no_server() {
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("demo");
    let wrapper = home.path().join("fails-midway");
    std::fs::write(
        &wrapper,
        format!("#!/bin/sh\ncase \"$*\" in *\"label create bug\"*) exit 7 ;; esac\nexec '{CLI}' \"$@\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(
        &wrapper,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    let out = run_seed(home.path(), &data, wrapper.to_str().unwrap());
    assert!(!out.status.success(), "the seed should have failed");
    assert!(data.join("server.lock").exists(), "no server ever ran");
    assert_eq!(lock_holders(&data), "");
}

/// LLM-137: a preview opens on its temporary folder, keeps its settings out
/// of the real data folder, and moves settings that an older preview left there.
#[test]
fn preview_stays_out_of_the_real_folder() {
    let home = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let real = home.path().join("Library/Application Support/llmtodo");
    let old = real.join("preview-config");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("ui.json"), r#"{"zoom":1.5}"#).unwrap();
    std::fs::write(real.join("ui.json"), r#"{"zoom":2.0}"#).unwrap();
    let out = Command::new("sh")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/preview"))
        .arg("empty")
        .env_clear()
        .env("HOME", home.path())
        .env("TMPDIR", tmp.path())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LLMTODO_BIN", CLI)
        .env(
            "LLMTODO_PREVIEW_APP",
            "echo \"data=$LLMTODO_DATA_DIR\"; echo \"config=$LLMTODO_CONFIG_DIR\"",
        )
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    let data = PathBuf::from(
        stdout
            .lines()
            .find_map(|l| l.strip_prefix("PREVIEW_DATA="))
            .expect("no PREVIEW_DATA line"),
    );
    let log = std::fs::read_to_string(data.join("app.log")).unwrap();
    let value = |name: &str| {
        log.lines()
            .find_map(|l| l.strip_prefix(name))
            .unwrap_or_else(|| panic!("no {name} in {log}"))
            .to_string()
    };
    assert_eq!(PathBuf::from(value("data=")), data);
    assert!(data.starts_with(tmp.path()), "{}", data.display());
    let config = PathBuf::from(value("config="));
    assert!(!config.starts_with(&real), "{}", config.display());
    // The older preview's settings moved, and the real ones stayed.
    assert_eq!(
        std::fs::read_to_string(config.join("ui.json")).unwrap(),
        r#"{"zoom":1.5}"#
    );
    assert!(!old.exists());
    assert_eq!(
        std::fs::read_to_string(real.join("ui.json")).unwrap(),
        r#"{"zoom":2.0}"#
    );
    assert!(
        !real.join("llmtodo.db").exists(),
        "no server ran on real data"
    );
}

/// Runs `scripts/preview` with a command in place of the app. The command is a
/// client, so it starts a server when it finds none, as the app does.
#[test]
fn preview_leaves_no_server() {
    let home = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new("sh")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/preview"))
        .env_clear()
        .env("HOME", home.path())
        .env("TMPDIR", tmp.path())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LLMTODO_BIN", CLI)
        .env(
            "LLMTODO_PREVIEW_APP",
            "sleep 0.3; \"$LLMTODO_BIN\" --json --no-input team list",
        )
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(!stderr.contains("LEFTOVER"), "{stderr}");
    let data = PathBuf::from(
        stdout
            .lines()
            .find_map(|l| l.strip_prefix("PREVIEW_DATA="))
            .expect("no PREVIEW_DATA line"),
    );
    assert_eq!(lock_holders(&data), "");
    let server_log = std::fs::read_to_string(data.join("server.out")).unwrap();
    assert!(!server_log.contains("already runs"), "{server_log}");
    let app_log = std::fs::read_to_string(data.join("app.log")).unwrap();
    assert!(app_log.contains("DEMO"), "{app_log}");
}
