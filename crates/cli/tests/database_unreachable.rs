//! A server that cannot reach its shared database says so in plain words,
//! with exit code 7, the code for "try again later" (LLM-470).

use assert_cmd::Command;
use serde_json::Value;

/// A home folder whose config asks for Postgres in its main data folder.
/// The Keychain service is one that holds no item.
struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let config = dir.path().join("config");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            format!(
                "data_dir = {:?}\n\n[database]\nbackend = \"postgres\"\n",
                data.display().to_string()
            ),
        )
        .unwrap();
        Self { dir }
    }

    fn run(&self, url: Option<&str>, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::cargo_bin("llmtodo").unwrap();
        cmd.env_clear()
            .env("HOME", self.dir.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LLMTODO_CONFIG_DIR", self.dir.path().join("config"))
            .env("LLMTODO_DATA_DIR", self.dir.path().join("data"))
            .env(
                "LLMTODO_KEYCHAIN_SERVICE",
                format!("llmtodo-test-none-{}", std::process::id()),
            )
            .env("LLMTODO_ACTOR", "human")
            .env("LLMTODO_NO_GITHUB", "1")
            .env("LLMTODO_PORT", "0")
            .current_dir(self.dir.path())
            .args(["--json", "--no-input"])
            .args(args);
        if let Some(url) = url {
            cmd.env("LLMTODO_DATABASE_URL", url);
        }
        cmd.output().unwrap()
    }
}

fn error(out: &std::process::Output) -> Value {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().last().unwrap_or_default();
    serde_json::from_str(line).unwrap_or_else(|_| panic!("not a JSON error: {stderr}"))
}

#[test]
fn no_stored_url_is_an_unreachable_database() {
    let home = Home::new();
    let out = home.run(None, &["team", "list"]);
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    let e = error(&out);
    assert_eq!(e["error"]["code"], "database_unreachable");
    let message = e["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("Shared database unreachable"),
        "{message}"
    );
    assert!(message.contains("llmtodo db set-url"), "{message}");
}

#[test]
fn a_dead_host_is_an_unreachable_database_with_its_host() {
    // A port nothing listens on: the connection is refused at once.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let home = Home::new();
    let url = format!("postgres://llmtodo:secret@127.0.0.1:{port}/llmtodo");
    let out = home.run(Some(&url), &["team", "list"]);
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    let e = error(&out);
    assert_eq!(e["error"]["code"], "database_unreachable");
    let host = format!("127.0.0.1:{port}/llmtodo");
    assert_eq!(e["error"]["details"]["database_host"], host.as_str());
    let message = e["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with(&format!("Shared database unreachable at {host}")),
        "{message}"
    );
    assert!(
        !message.contains("secret"),
        "the password stays out: {message}"
    );
}
