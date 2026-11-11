//! Every API route is called by the CLI command that `routes.rs` names for
//! it. The test runs each command once against a server that records the
//! routes it sees, so a command that stops calling its route fails here.

use std::collections::{BTreeMap, BTreeSet};
use std::io::BufRead as _;
use std::time::{Duration, Instant};

use assert_cmd::cargo::CommandCargoExt as _;
use serde_json::Value;

/// Routes this test cannot call without a real outside service.
const EXEMPT: &[(&str, &str, &str)] = &[(
    "POST",
    "/v1/import/linear",
    "needs a Linear API key and network access",
)];

struct Harness {
    dir: tempfile::TempDir,
    sync: tempfile::TempDir,
    running: Option<llmtodo_server::Running>,
    /// Routes seen while each command ran, by its `cli` name.
    seen: BTreeMap<String, BTreeSet<String>>,
    /// The stderr of each command, for failure messages.
    stderr: BTreeMap<String, String>,
}

impl Harness {
    fn command(&self, args: &[&str]) -> std::process::Command {
        let mut c = std::process::Command::cargo_bin("llmtodo").unwrap();
        c.env_clear()
            .env("HOME", self.dir.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LLMTODO_DATA_DIR", self.dir.path())
            .env("LLMTODO_SESSION", "s1")
            .env("LLMTODO_ACTOR", "agent:test-harness/test-model/high")
            .current_dir(self.dir.path())
            .args(["--json", "--no-input", "-y"])
            .args(args);
        c
    }

    fn running(&self) -> &llmtodo_server::Running {
        self.running.as_ref().unwrap()
    }

    /// Runs one command and files the routes it called under `cli`.
    fn run(&mut self, cli: &str, args: &[&str]) -> Value {
        let out = self.command(args).output().unwrap();
        let routes = self.running().take_routes();
        self.seen.entry(cli.to_string()).or_default().extend(routes);
        self.stderr.insert(
            cli.to_string(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    /// Runs `activity --follow` until the server sees it open the WebSocket.
    fn run_follow(&mut self) {
        let mut child = self
            .command(&["activity", "--follow"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut routes = BTreeSet::new();
        while Instant::now() < deadline && !routes.contains("GET /v1/ws") {
            std::thread::sleep(Duration::from_millis(50));
            routes.extend(self.running().take_routes());
        }
        child.kill().unwrap();
        child.wait().unwrap();
        self.seen
            .entry("activity --follow".into())
            .or_default()
            .extend(routes);
    }
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap_or("missing-id").to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_route_is_called_by_its_command() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    running.record_routes();
    let mut h = Harness {
        dir,
        sync: tempfile::tempdir().unwrap(),
        running: Some(running),
        seen: BTreeMap::new(),
        stderr: BTreeMap::new(),
    };
    let sync = h.sync.path().display().to_string();

    h.run("server status", &["server", "status"]);
    h.run("settings show", &["settings", "show"]);
    h.run(
        "settings set",
        &["settings", "set", "--workspace-name", "W"],
    );
    h.run("actor list", &["actor", "list"]);
    h.run("actor me", &["actor", "me"]);

    h.run("team create", &["team", "create", "LLM", "llmtodo"]);
    h.run("team list", &["team", "list"]);
    h.run("team view", &["team", "view", "LLM"]);
    h.run(
        "team update",
        &["team", "update", "LLM", "--name", "Tracker"],
    );
    let status = h.run(
        "status create",
        &["status", "create", "LLM", "Review", "--type", "started"],
    );
    let status = id(&status);
    h.run(
        "status update",
        &["status", "update", &status, "--name", "QA"],
    );
    h.run("status delete", &["status", "delete", &status]);

    h.run("label create", &["label", "create", "bug"]);
    h.run("label list", &["label", "list"]);
    h.run(
        "label update",
        &["label", "update", "bug", "--color", "#ff0000"],
    );
    h.run("label delete", &["label", "delete", "bug"]);

    h.run(
        "project create",
        &[
            "project", "create", "Alpha", "--slug", "alpha", "--team", "LLM",
        ],
    );
    h.run("project list", &["project", "list"]);
    h.run("project view", &["project", "view", "alpha"]);
    h.run(
        "project update",
        &["project", "update", "alpha", "--description", "d"],
    );
    // Before a repo is linked, so the sync makes no GitHub request.
    h.run("github status", &["github", "status"]);
    h.run("github sync", &["github", "sync"]);
    h.run(
        "project link-repo",
        &["project", "link-repo", "acme/widgets", "--project", "alpha"],
    );
    h.run(
        "project unlink-repo",
        &[
            "project",
            "unlink-repo",
            "acme/widgets",
            "--project",
            "alpha",
        ],
    );

    h.run(
        "milestone list",
        &["milestone", "list", "--project", "alpha"],
    );
    let m = h.run(
        "milestone create",
        &["milestone", "create", "M1", "--project", "alpha"],
    );
    let m = id(&m);
    h.run(
        "milestone update",
        &["milestone", "update", &m, "--name", "M2"],
    );
    h.run("milestone delete", &["milestone", "delete", &m]);

    h.run(
        "doc create",
        &[
            "doc",
            "create",
            "Plan",
            "--project",
            "alpha",
            "--slug",
            "plan",
            "--body",
            "one",
        ],
    );
    h.run("doc list", &["doc", "list"]);
    h.run("doc view", &["doc", "view", "plan"]);
    h.run(
        "doc update",
        &["doc", "update", "plan", "--body", "two", "--force"],
    );
    h.run("doc history", &["doc", "history", "plan"]);
    h.run(
        "doc restore",
        &["doc", "restore", "plan", "--revision", "1"],
    );
    h.run("doc delete", &["doc", "delete", "plan"]);
    h.run("project delete", &["project", "delete", "alpha"]);

    h.run(
        "issue create",
        &[
            "issue",
            "create",
            "-t",
            "Map",
            "--estimate-minutes",
            "30",
            "--estimate-cost",
            "1.5",
        ],
    );
    h.run(
        "issue create",
        &["issue", "create", "-t", "Child", "--parent", "LLM-1"],
    );
    h.run("issue query", &["issue", "query"]);
    h.run("issue frontier", &["issue", "frontier", "LLM-1"]);
    h.run("issue view", &["issue", "view", "LLM-1"]);
    h.run(
        "issue update",
        &["issue", "update", "LLM-1", "--title", "Map 2"],
    );
    h.run("issue history", &["issue", "history", "LLM-1"]);
    h.run("issue claim", &["issue", "claim", "LLM-1"]);
    h.run("issue analyze", &["issue", "analyze", "LLM-1"]);
    h.run("issue release", &["issue", "release", "LLM-1"]);
    h.command(&["issue", "ask", "LLM-1", "Name?"])
        .output()
        .unwrap();
    h.command(&["issue", "answer", "LLM-1", "Q1", "lt"])
        .output()
        .unwrap();
    h.running().take_routes();
    h.run(
        "issue undo-answer",
        &["issue", "undo-answer", "LLM-1", "Q1"],
    );
    h.run("issue questions", &["issue", "questions", "LLM-1"]);
    h.run("issue review show", &["issue", "review", "show", "LLM-1"]);
    // LLM-1 is not in User review, so the server refuses, after the call.
    h.run(
        "issue review answer",
        &["issue", "review", "answer", "LLM-1", "approve"],
    );
    let c = h.run(
        "issue comment add",
        &["issue", "comment", "add", "LLM-1", "--body", "hi"],
    );
    let c = id(&c);
    h.run(
        "issue comment update",
        &["issue", "comment", "update", &c, "--body", "hello"],
    );
    h.run("issue comment delete", &["issue", "comment", "delete", &c]);
    h.run(
        "issue relation add",
        &["issue", "relation", "add", "LLM-1", "blocks", "LLM-2"],
    );
    h.run("issue deps", &["issue", "deps", "LLM-1"]);
    h.run(
        "issue relation remove",
        &["issue", "relation", "remove", "LLM-1", "blocks", "LLM-2"],
    );
    let a = h.run(
        "issue attach",
        &["issue", "attach", "LLM-1", "https://example.com/x"],
    );
    let a = id(&a);
    h.run("issue detach", &["issue", "detach", &a]);
    h.run("issue delete", &["issue", "delete", "LLM-2"]);
    h.run("issue restore", &["issue", "restore", "LLM-2"]);

    h.run(
        "session note",
        &["session", "note", "a note", "--issue", "LLM-1"],
    );
    h.run("session show", &["session", "show", "s1"]);
    h.run("usage", &["usage"]);
    h.run("activity", &["activity"]);
    h.run_follow();

    h.run("view create", &["view", "create", "Open", "--open"]);
    h.run("view list", &["view", "list"]);
    h.run(
        "view update",
        &["view", "update", "Open", "--name", "Open 2"],
    );
    h.run("view delete", &["view", "delete", "Open 2"]);

    h.run("file get", &["file", "get", "0123abcd"]);
    h.run("prune", &["prune", "--before", "2999-01-01"]);
    h.run("server backup", &["server", "backup"]);
    h.run("sync status", &["sync", "status"]);
    h.run("sync push", &["sync", "push", "--folder", &sync]);
    h.run("sync pull", &["sync", "pull", "--folder", &sync, "--force"]);
    h.run("server stop", &["server", "stop"]);

    let mut missing = Vec::new();
    for r in llmtodo_api_types::routes::ROUTES {
        let route = format!("{} {}", r.method.as_str(), r.path);
        if EXEMPT
            .iter()
            .any(|(m, p, _)| *m == r.method.as_str() && *p == r.path)
        {
            continue;
        }
        let ran = h.seen.get(r.cli);
        if !ran.is_some_and(|s| s.contains(&route)) {
            missing.push(format!(
                "`{}` did not call {route}{}",
                r.cli,
                match (ran, h.stderr.get(r.cli)) {
                    (None, _) => " (the test does not run it)".to_string(),
                    (Some(_), Some(e)) if !e.trim().is_empty() => format!("; stderr: {}", e.trim()),
                    _ => String::new(),
                }
            ));
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));

    // The server stopped on `server stop`; wait for its task to end.
    let _ = h.running.take().unwrap().stop().await;
}

/// `activity --follow` survives a server restart, such as another CLI's
/// version upgrade: it reconnects and prints the events it missed.
#[tokio::test(flavor = "multi_thread")]
async fn follow_resumes_after_a_server_restart() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let h = Harness {
        dir,
        sync: tempfile::tempdir().unwrap(),
        running: None,
        seen: BTreeMap::new(),
        stderr: BTreeMap::new(),
    };
    let ok = |args: &[&str]| {
        let out = h.command(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    ok(&["team", "create", "LLM", "llmtodo"]);

    let mut child = h
        .command(&["activity", "--follow"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if let Ok(v) = serde_json::from_str(&line) {
                let _ = tx.send(v);
            }
        }
    });
    let wait_for = |title: &str| {
        let deadline = Instant::now() + Duration::from_secs(20);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(v) if v.to_string().contains(title) => return,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        panic!("`activity --follow` never printed an event for {title:?}");
    };

    // Give the follower time to connect before the first write.
    std::thread::sleep(Duration::from_millis(500));
    ok(&["issue", "create", "-t", "before-restart"]);
    wait_for("before-restart");

    // The in-process server stops. The follower's reconnect starts a server
    // process for the same folder, which the next command also finds.
    running.stop().await.unwrap();
    std::thread::sleep(Duration::from_millis(300));
    ok(&["issue", "create", "-t", "after-restart"]);
    wait_for("after-restart");
    assert!(
        child.try_wait().unwrap().is_none(),
        "the follower exited after the restart"
    );

    child.kill().unwrap();
    child.wait().unwrap();
    ok(&["server", "stop"]);
}
