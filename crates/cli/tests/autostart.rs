//! The CLI starts a server for a data folder that has none. For a folder other
//! than the default one, that server must not need the default port, because
//! the owner's own server usually holds it.

use assert_cmd::Command;
use llmtodo_api_types::DEFAULT_PORT;

#[test]
fn autostart_works_while_default_port_is_taken() {
    // Hold the default port, unless another server holds it already.
    let _held = std::net::TcpListener::bind(("127.0.0.1", DEFAULT_PORT));
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::cargo_bin("llmtodo")
            .unwrap()
            .env_clear()
            .env("HOME", dir.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LLMTODO_DATA_DIR", dir.path())
            .env("LLMTODO_ACTOR", "human")
            .env("LLMTODO_NO_GITHUB", "1")
            .current_dir(dir.path())
            .args(["--json", "--no-input"])
            .args(args)
            .output()
            .unwrap()
    };
    let out = run(&["team", "list"]);
    let stopped = run(&["server", "stop"]);
    assert!(
        out.status.success(),
        "auto-start failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stopped.status.success());
}
