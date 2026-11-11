//! A stale `server.json` can name a port that a server for another data
//! folder holds now. `server stop` and `server status` must leave that server
//! alone.

use assert_cmd::Command;
use serde_json::Value;
use std::path::PathBuf;

/// Runs the CLI against `dir` off the async runtime, so the in-process server
/// keeps serving.
async fn run(dir: PathBuf, args: &[&'static str]) -> Value {
    let args = args.to_vec();
    tokio::task::spawn_blocking(move || {
        let out = Command::cargo_bin("llmtodo")
            .unwrap()
            .env_clear()
            .env("HOME", &dir)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LLMTODO_DATA_DIR", &dir)
            .env("LLMTODO_ACTOR", "human")
            .current_dir(&dir)
            .args(["--json", "--no-input"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_server_json_does_not_reach_other_server() {
    let other = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: other.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    // A crashed server for `stale` left a server.json with the other port.
    let stale = tempfile::tempdir().unwrap();
    std::fs::copy(
        other.path().join("server.json"),
        stale.path().join("server.json"),
    )
    .unwrap();

    let status = run(stale.path().to_path_buf(), &["server", "status"]).await;
    assert_eq!(status["running"], false, "{status}");
    let stop = run(stale.path().to_path_buf(), &["server", "stop"]).await;
    assert_eq!(stop["running"], false, "{stop}");

    // The other folder's server still answers.
    let c = llmtodo_client::Client::new(format!("http://{}", running.addr), &Default::default());
    assert!(
        c.health().await.is_ok(),
        "server stop reached the other server"
    );
    running.stop().await.unwrap();
}
