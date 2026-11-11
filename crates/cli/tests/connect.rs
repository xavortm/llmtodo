//! How the CLI finds its server: auto-start in a fresh folder, a stale
//! `server.json`, a server for another folder on the recorded port, and the
//! restart of an older server. Each test uses its own temp data folder, and
//! stops any server the CLI started.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use assert_cmd::Command;
use serde_json::{Value, json};

/// Runs the CLI for `dir` off the async runtime, so in-process servers keep
/// serving. Fails unless it exits 0, and returns the JSON output.
async fn cli(dir: &Path, args: &[&str]) -> Value {
    let dir = dir.to_path_buf();
    let args: Vec<String> = args.iter().map(std::string::ToString::to_string).collect();
    tokio::task::spawn_blocking(move || {
        let out = Command::cargo_bin("llmtodo")
            .unwrap()
            .env_clear()
            .env("HOME", &dir)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LLMTODO_DATA_DIR", &dir)
            .env("LLMTODO_ACTOR", "human")
            .env("LLMTODO_NO_GITHUB", "1")
            .current_dir(&dir)
            .args(["--json", "--no-input"])
            .args(&args)
            .output()
            .unwrap();
        let code = out.status.code().unwrap_or(-1);
        assert!(
            code == 0,
            "{args:?} exited {code}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    })
    .await
    .unwrap()
}

fn server_json(dir: &Path) -> Option<Value> {
    let raw = std::fs::read(dir.join("server.json")).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Stops the server the CLI started for `dir`, if one runs.
struct StopOnDrop(PathBuf);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = std::process::Command::new(assert_cmd::cargo::cargo_bin("llmtodo"))
            .env_clear()
            .env("HOME", &self.0)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LLMTODO_DATA_DIR", &self.0)
            .env("LLMTODO_ACTOR", "human")
            .args(["--json", "--no-input", "server", "stop"])
            .output();
    }
}

fn team_keys(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t["key"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A fresh folder gets its own server, and the next call reuses it.
#[tokio::test(flavor = "multi_thread")]
async fn fresh_folder_starts_a_server_and_reuses_it() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = StopOnDrop(dir.path().to_path_buf());
    cli(dir.path(), &["team", "create", "LLM", "llmtodo"]).await;
    let first = server_json(dir.path()).expect("the CLI wrote no server.json");
    assert_ne!(first["pid"], json!(std::process::id()));
    assert!(has_db(dir.path()), "no database in the data folder");

    let teams = cli(dir.path(), &["team", "list"]).await;
    assert_eq!(team_keys(&teams), ["LLM"]);
    assert_eq!(
        server_json(dir.path()).unwrap()["pid"],
        first["pid"],
        "the second call started another server"
    );
}

fn has_db(dir: &Path) -> bool {
    std::fs::read_dir(dir).unwrap().flatten().any(|e| {
        e.path()
            .extension()
            .is_some_and(|x| x == "db" || x == "sqlite")
    })
}

/// A crashed server leaves `server.json` behind with a dead port and pid.
#[tokio::test(flavor = "multi_thread")]
async fn stale_server_json_starts_a_new_server() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = StopOnDrop(dir.path().to_path_buf());
    // A port that nothing listens on now.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(
        dir.path().join("server.json"),
        json!({ "port": port, "pid": 999_999, "version": "0.0.1", "started_at": "2026-01-01T00:00:00Z" })
            .to_string(),
    )
    .unwrap();

    let teams = cli(dir.path(), &["team", "list"]).await;
    assert_eq!(teams, json!([]));
    let info = server_json(dir.path()).unwrap();
    assert_ne!(info["pid"], json!(999_999), "server.json was not replaced");
}

/// `server.json` names a port that a server for another folder holds now.
/// The CLI must start its own server and leave the other one alone.
#[tokio::test(flavor = "multi_thread")]
async fn another_folders_server_on_the_port_is_not_used() {
    let other = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: other.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _stop = StopOnDrop(dir.path().to_path_buf());
    std::fs::copy(
        other.path().join("server.json"),
        dir.path().join("server.json"),
    )
    .unwrap();

    cli(dir.path(), &["team", "create", "MINE", "mine"]).await;
    let info = server_json(dir.path()).unwrap();
    assert_ne!(info["port"], json!(running.addr.port()));

    let c = llmtodo_client::Client::new(running.url(), &Default::default());
    let teams = c.teams().await.unwrap();
    assert!(
        teams.iter().all(|t| t.key != "MINE"),
        "the write went to the other folder's server"
    );
    running.stop().await.unwrap();
}

/// A server older than the CLI is asked to stop, and the CLI starts a new
/// one in its place.
#[tokio::test(flavor = "multi_thread")]
async fn an_older_server_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let _stop = StopOnDrop(dir.path().to_path_buf());
    let stopped = Arc::new(AtomicBool::new(false));
    let fake = fake_old_server(dir.path(), stopped.clone()).await;

    let teams = cli(dir.path(), &["team", "list"]).await;
    assert_eq!(teams, json!([]));
    assert!(
        stopped.load(Ordering::SeqCst),
        "the CLI never asked the old server to stop"
    );
    let info = server_json(dir.path()).unwrap();
    assert_eq!(info["version"], json!(llmtodo_api_types::VERSION));
    assert_ne!(info["port"], json!(fake));
}

/// Serves `/v1/health` with version 0.0.1 for `dir`, under this test's pid,
/// as an old server would. `/v1/server/stop` removes its `server.json`, as
/// the real server does when it stops. Returns the port.
async fn fake_old_server(dir: &Path, stopped: Arc<AtomicBool>) -> u16 {
    use axum::routing::{get, post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let data_dir = dir.to_path_buf();
    let health = json!({
        "version": "0.0.1",
        "pid": std::process::id(),
        "data_dir": data_dir.display().to_string(),
        "schema_version": 1,
        "is_primary": true,
        "started_at": "2026-01-01T00:00:00Z",
    });
    let json_path = data_dir.join("server.json");
    let app = axum::Router::new()
        .route(
            "/v1/health",
            get(move || {
                let h = health.clone();
                async move { axum::Json(h) }
            }),
        )
        .route(
            "/v1/server/stop",
            post(move || {
                let stopped = stopped.clone();
                let json_path = json_path.clone();
                async move {
                    stopped.store(true, Ordering::SeqCst);
                    let _ = std::fs::remove_file(&json_path);
                    axum::Json(json!({ "stopping": true }))
                }
            }),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    std::fs::write(
        dir.join("server.json"),
        json!({ "port": port, "pid": std::process::id(), "version": "0.0.1", "started_at": "2026-01-01T00:00:00Z" })
            .to_string(),
    )
    .unwrap();
    port
}
