//! `db migrate` holds the server lock while it copies (LLM-477). The app
//! starts a server as soon as the old one stops. Without the lock, that
//! server read the old config and kept serving SQLite after the move.
//!
//! Needs Postgres, as the server tests do:
//! `docker compose up -d --wait postgres`, then
//! LLMTODO_TEST_POSTGRES_URL=postgres://llmtodo:llmtodo@127.0.0.1:54329/llmtodo \
//!   cargo test -p llmtodo-cli --test db_migrate_restart

use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use assert_cmd::cargo::cargo_bin;
use serde_json::Value;

struct Home {
    dir: tempfile::TempDir,
    url: String,
}

impl Home {
    fn new(url: String) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            format!(
                "data_dir = {:?}\n",
                dir.path().join("data").display().to_string()
            ),
        )
        .unwrap();
        Self { dir, url }
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn command(&self, bin: &Path) -> StdCommand {
        let mut cmd = StdCommand::new(bin);
        cmd.env_clear()
            .env("HOME", self.dir.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LLMTODO_CONFIG_DIR", self.dir.path().join("config"))
            .env("LLMTODO_DATA_DIR", self.data())
            .env(
                "LLMTODO_KEYCHAIN_SERVICE",
                format!("llmtodo-test-none-{}", std::process::id()),
            )
            .env("LLMTODO_DATABASE_URL", &self.url)
            .env("LLMTODO_ACTOR", "human")
            .env("LLMTODO_NO_GITHUB", "1")
            .env("LLMTODO_PORT", "0")
            .current_dir(self.dir.path());
        cmd
    }

    fn run(&self, args: &[&str]) -> Value {
        let out = self
            .command(&cargo_bin("llmtodo"))
            .args(["--json", "--no-input"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "llmtodo {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }
}

/// Stops the folder's server, even after a failed assert.
impl Drop for Home {
    fn drop(&mut self) {
        let _ = self
            .command(&cargo_bin("llmtodo"))
            .args(["--json", "--no-input", "server", "stop"])
            .output();
    }
}

/// Starts `llmtodo server run` every 50 ms until dropped, then kills every
/// server it started.
struct Starter {
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<Child>>>,
}

impl Starter {
    fn new(home: Arc<Home>) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let thread = std::thread::spawn(move || {
            let bin = cargo_bin("llmtodo");
            let mut started = Vec::new();
            while !flag.load(Ordering::Relaxed) {
                if let Ok(c) = home
                    .command(&bin)
                    .args(["server", "run"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    started.push(c);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            started
        });
        Self {
            done,
            thread: Some(thread),
        }
    }
}

impl Drop for Starter {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(Ok(started)) = self.thread.take().map(|t| t.join()) {
            for mut c in started {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_started_during_the_copy_does_not_keep_the_old_database() {
    let Some(admin) = std::env::var("LLMTODO_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())
    else {
        eprintln!("skipped: set LLMTODO_TEST_POSTGRES_URL");
        return;
    };
    let url = llmtodo_server::db::create_test_database(&admin)
        .await
        .unwrap();
    let home = Arc::new(Home::new(url));
    home.run(&["team", "create", "ABC", "Alpha"]);
    home.run(&[
        "issue",
        "create",
        "--team",
        "ABC",
        "-t",
        "Moves to Postgres",
    ]);

    // Play the app: start a server again and again, faster than its quickest
    // retry, so the race shows up on most runs.
    let starter = Starter::new(home.clone());
    let migrated = home.run(&["db", "migrate", "--to", "postgres", "-y"]);
    // Ask while the started servers still run, as the app's would.
    let status = home.run(&["db", "status"]);
    drop(starter);
    assert_eq!(migrated["to"], "postgres", "{migrated}");
    assert_eq!(status["server"]["backend"], "postgres", "{status}");
    let issues = home.run(&["issue", "query", "--team", "ABC", "--fields", "title"]);
    assert_eq!(
        issues.to_string(),
        r#"[{"title":"Moves to Postgres"}]"#,
        "{issues}"
    );
}
