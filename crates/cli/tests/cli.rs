//! Runs the real binary against a server started inside the test process.

use assert_cmd::Command;
use serde_json::Value;

struct T {
    dir: tempfile::TempDir,
    running: Option<llmtodo_server::Running>,
}

impl T {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let running = llmtodo_server::start(llmtodo_server::Config {
            data_dir: dir.path().to_path_buf(),
            port: 0,
            host: None,
            github_poll: false,
        })
        .await
        .unwrap();
        T {
            dir,
            running: Some(running),
        }
    }

    /// Runs the binary. New issues get estimates, so that tests can claim.
    fn cmd(&self, session: &str, args: &[&str]) -> Command {
        let mut c = self.bare_cmd(session, args);
        if args.starts_with(&["issue", "create"]) {
            c.args(["--estimate-minutes", "30", "--estimate-cost", "1.5"]);
        }
        c
    }

    fn bare_cmd(&self, session: &str, args: &[&str]) -> Command {
        let mut c = Command::cargo_bin("llmtodo").unwrap();
        c.env_clear()
            .env("HOME", self.dir.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LLMTODO_DATA_DIR", self.dir.path())
            .env("LLMTODO_SESSION", session)
            .env("LLMTODO_ACTOR", "agent:test-harness/test-model/high")
            .current_dir(self.dir.path())
            .args(args);
        c
    }

    fn json(&self, session: &str, args: &[&str]) -> Value {
        let out = self.cmd(session, args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn code(&self, session: &str, args: &[&str]) -> i32 {
        self.cmd(session, args)
            .output()
            .unwrap()
            .status
            .code()
            .unwrap()
    }

    async fn stop(mut self) {
        self.running.take().unwrap().stop().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_workflow_and_exit_codes() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "LLM", "llmtodo"]);
    let v = t.json(
        "s1",
        &[
            "issue",
            "create",
            "-t",
            "First",
            "-d",
            "## Plan\n\nold\n",
            "--fields",
            "key,status.name",
        ],
    );
    assert_eq!(
        v,
        serde_json::json!({ "key": "LLM-1", "status": { "name": "Todo" } })
    );
    t.json(
        "s1",
        &[
            "issue",
            "create",
            "-t",
            "Second",
            "--parent",
            "LLM-1",
            "--blocked-by",
            "LLM-1",
        ],
    );

    let q = t.json("s1", &["issue", "query", "--open", "--fields", "key"]);
    assert_eq!(
        q,
        serde_json::json!([{ "key": "LLM-2" }, { "key": "LLM-1" }]),
        "no priority, so newest update first"
    );

    // Frontier of the map skips the blocked child.
    let f = t.json("s1", &["issue", "frontier", "LLM-1"]);
    assert_eq!(f.as_array().unwrap().len(), 0);

    let c = t.json("s1", &["issue", "claim", "LLM-1"]);
    assert_eq!(
        c["issue"]["assignee"]["name"],
        "test-harness/test-model/high"
    );
    assert_eq!(t.code("s2", &["issue", "claim", "LLM-1"]), 4);

    let v = t.json(
        "s1",
        &[
            "issue",
            "update",
            "LLM-1",
            "--section",
            "Plan",
            "--body",
            "\nnew\n",
        ],
    );
    assert!(v["description"].as_str().unwrap().contains("new"));
    assert_eq!(
        t.code(
            "s1",
            &[
                "issue",
                "update",
                "LLM-1",
                "--replace",
                "missing",
                "--with",
                "x"
            ]
        ),
        10
    );
    assert_eq!(
        t.code(
            "s1",
            &["issue", "update", "LLM-1", "-d", "x", "--base-version", "1"]
        ),
        5
    );
    assert_eq!(t.code("s1", &["issue", "view", "LLM-99"]), 3);
    // Delete asks first; without a terminal and -y it needs input.
    assert_eq!(t.code("s1", &["issue", "delete", "LLM-2", "--no-input"]), 8);
    t.json("s1", &["issue", "delete", "LLM-2", "-y"]);
    assert_eq!(t.code("s1", &["prune", "--before", "2999-01-01", "-y"]), 11);

    let s = t.json("s1", &["session", "show"]);
    assert_eq!(s["issues"][0]["issue"], "LLM-1");

    let schema = t.json("s1", &["schema"]);
    assert!(
        schema["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["command"] == "issue claim")
    );
    t.stop().await;
}

/// The repo's default project must not hide duplicates in other projects.
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_search_spans_all_projects() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "ALX", "Alex"]);
    t.json(
        "s1",
        &[
            "project", "create", "Alpha", "--slug", "alpha", "--team", "ALX",
        ],
    );
    t.json(
        "s1",
        &[
            "project", "create", "Beta", "--slug", "beta", "--team", "ALX",
        ],
    );
    t.json(
        "s1",
        &[
            "issue",
            "create",
            "-t",
            "Login page crashes on submit",
            "--project",
            "beta",
        ],
    );
    std::fs::write(t.dir.path().join(".llmtodo.toml"), "project = \"alpha\"\n").unwrap();
    let keys = |args: &[&str]| -> Vec<String> {
        let mut a = vec!["issue", "query", "--fields", "key"];
        a.extend_from_slice(args);
        t.json("s1", &a)
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["key"].as_str().unwrap().to_string())
            .collect()
    };
    // The default project still applies to a plain query.
    assert!(keys(&[]).is_empty());
    assert_eq!(keys(&["--similar", "login page crash"]), ["ALX-1"]);
    assert_eq!(keys(&["-q", "login", "--all-projects"]), ["ALX-1"]);
    assert_eq!(
        keys(&["--team", "ALX", "-q", "login", "--all-projects"]),
        ["ALX-1"]
    );
    // A text search limited by the default project says so on stderr.
    let out = t
        .cmd("s1", &["issue", "query", "-q", "login"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("alpha"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    t.stop().await;
}

/// A repo file that names the human owner must not turn a detected agent
/// into the owner. Only `LLMTODO_ACTOR` overrides the detection.
#[tokio::test(flavor = "multi_thread")]
async fn repo_human_actor_does_not_hide_an_agent() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "ALX", "Alex"]);
    t.json("s1", &["issue", "create", "-t", "Work"]);
    std::fs::write(t.dir.path().join(".llmtodo.toml"), "actor = \"human\"\n").unwrap();
    let out = t
        .cmd("agent-s", &["--json", "issue", "claim", "ALX-1"])
        .env_remove("LLMTODO_ACTOR")
        .env("AI_AGENT", "claude-code_2-1-322_agent")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["issue"]["assignee"]["kind"], "agent", "{v}");
    // Without agent detection, the repo setting still applies.
    t.json("s1", &["issue", "create", "-t", "Owner work"]);
    let out = t
        .cmd("human-s", &["--json", "issue", "claim", "ALX-2"])
        .env_remove("LLMTODO_ACTOR")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["issue"]["assignee"]["kind"], "human", "{v}");
    t.stop().await;
}

/// Subagents can share one session. A repeated claim then succeeds without
/// a change, so the CLI must say so on stderr, where JSON readers see it.
#[tokio::test(flavor = "multi_thread")]
async fn repeated_claim_warns_on_stderr() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "ALX", "Alex"]);
    t.json("s1", &["issue", "create", "-t", "Work"]);
    let claim = || {
        t.cmd("same", &["--json", "issue", "claim", "ALX-1"])
            .output()
            .unwrap()
    };
    let first = claim();
    assert!(first.status.success());
    assert!(
        first.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let again = claim();
    assert!(again.status.success());
    let v: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(v["changed"], false);
    let err = String::from_utf8_lossy(&again.stderr);
    assert!(err.contains("already holds"), "{err}");
    t.stop().await;
}

/// A label named for a workflow state is refused with exit code 6 and
/// names the field to use. `--force` creates it anyway (LLM-161).
#[tokio::test(flavor = "multi_thread")]
async fn label_create_refuses_workflow_names() {
    let t = T::new().await;
    let out = t
        .cmd("s1", &["--json", "label", "create", "ready-for-agent"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "validation");
    let msg = err["error"]["message"].as_str().unwrap();
    assert!(msg.contains("-w agent") && msg.contains("--force"), "{msg}");
    assert_eq!(t.json("s1", &["label", "list"]), serde_json::json!([]));

    t.json("s1", &["label", "create", "cli"]);
    assert_eq!(
        t.code("s1", &["label", "update", "cli", "--name", "needs-owner"]),
        6
    );
    t.json("s1", &["label", "create", "blocked", "--force"]);
    let names: Vec<Value> = t
        .json("s1", &["label", "list"])
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].clone())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    t.stop().await;
}

/// `--with` without `--replace`, or no change at all, must fail instead of
/// exiting 0 with nothing changed.
#[tokio::test(flavor = "multi_thread")]
async fn update_without_a_change_fails() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "ALX", "Alex"]);
    t.json("s1", &["issue", "create", "-t", "Work"]);
    assert_eq!(
        t.code("s1", &["issue", "update", "ALX-1", "--with", "Use comrak."]),
        2
    );
    assert_eq!(t.code("s1", &["issue", "update", "ALX-1"]), 2);
    assert_eq!(t.code("s1", &["issue", "update", "ALX-1", "--force"]), 2);
    assert_eq!(
        t.code("s1", &["doc", "update", "some-doc", "--with", "x"]),
        2
    );
    assert_eq!(t.code("s1", &["doc", "update", "some-doc"]), 2);
    let v = t.json(
        "s1",
        &["issue", "view", "ALX-1", "--fields", "description_version"],
    );
    assert_eq!(v["description_version"], 1);
    // A real change still works.
    t.json("s1", &["issue", "update", "ALX-1", "-p", "high"]);
    t.stop().await;
}

/// Every error prints the JSON error body with a code that matches the
/// exit code, including errors that do not come from the server's handlers.
#[tokio::test(flavor = "multi_thread")]
async fn json_error_contract() {
    let t = T::new().await;
    let err = |args: &[&str]| -> (i32, Value) {
        let out = t.cmd("s1", args).output().unwrap();
        let body: Value = serde_json::from_slice(&out.stderr).unwrap_or_else(|e| {
            panic!(
                "{args:?}: stderr is not JSON ({e}): {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap(), body)
    };

    // axum rejects the body before the handler runs.
    let (code, body) = err(&[
        "--json",
        "api",
        "POST",
        "/v1/issues",
        "--body",
        r#"{"bad":1}"#,
    ]);
    assert_eq!(
        (code, body["error"]["code"].as_str()),
        (2, Some("bad_request"))
    );

    // 405 from the router.
    let (code, body) = err(&["--json", "api", "PUT", "/v1/teams"]);
    assert_eq!(
        (code, body["error"]["code"].as_str()),
        (2, Some("bad_request"))
    );

    // Clap errors print JSON too.
    let (code, body) = err(&["--json", "issue", "query", "--priority", "bogus"]);
    assert_eq!(
        (code, body["error"]["code"].as_str()),
        (2, Some("bad_request"))
    );
    // Stdout is not a terminal here, so JSON is the default.
    let (code, body) = err(&["issue", "nosuchcommand"]);
    assert_eq!(
        (code, body["error"]["code"].as_str()),
        (2, Some("bad_request"))
    );

    t.json("s1", &["team", "create", "LLM", "llmtodo"]);
    t.json("s1", &["issue", "create", "-t", "x"]);
    let (code, body) = err(&["--json", "issue", "delete", "LLM-1", "--no-input"]);
    assert_eq!(
        (code, body["error"]["code"].as_str()),
        (8, Some("input_needed"))
    );

    // Help and version still print text and exit 0.
    let out = t.cmd("s1", &["--version"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("llmtodo "));

    // The schema lists every error code with its exit code.
    let schema = t.json("s1", &["schema"]);
    assert_eq!(schema["error_codes"]["input_needed"], 8);
    assert_eq!(schema["error_codes"]["unreachable"], 7);
    assert_eq!(schema["error_codes"]["bad_request"], 2);
    t.stop().await;
}

/// Reads never parse the transcript. The first write parses it, keeps its
/// state in the data folder, and sends the usage.
#[tokio::test(flavor = "multi_thread")]
async fn usage_is_read_only_for_writes() {
    let t = T::new().await;
    let projects = t.dir.path().join(".claude/projects/some-project");
    std::fs::create_dir_all(&projects).unwrap();
    let line = serde_json::json!({
        "requestId": "req-1",
        "message": { "model": "claude-opus-5-5", "usage": { "input_tokens": 100, "output_tokens": 50 } }
    });
    std::fs::write(projects.join("sess-u.jsonl"), format!("{line}\n")).unwrap();
    let cache = t.dir.path().join("usage-cache");
    let run = |args: &[&str]| {
        let mut c = t.cmd("work", args);
        c.env("CLAUDE_CODE_SESSION_ID", "sess-u");
        let out = c.output().unwrap();
        assert!(
            out.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap_or(Value::Null)
    };

    run(&["team", "create", "LLM", "llmtodo"]);
    assert!(cache.join("sess-u.json").is_file(), "a write reads usage");
    std::fs::remove_dir_all(&cache).unwrap();

    run(&["schema"]);
    run(&["issue", "query"]);
    assert!(!cache.exists(), "reads must not parse the transcript");

    run(&["issue", "create", "-t", "x"]);
    assert!(cache.join("sess-u.json").is_file());
    t.stop().await;
}

/// `issue ask` writes the question and waits on the owner, `issue questions`
/// lists it, and the last `issue answer` hands the issue back.
#[tokio::test(flavor = "multi_thread")]
async fn ask_and_answer_owner_questions() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "ALX", "Alex"]);
    t.json("s1", &["issue", "create", "-t", "Decide"]);
    let v = t.json(
        "s1",
        &[
            "issue",
            "ask",
            "ALX-1",
            "Fork or PR?",
            "-o",
            "a=Fork",
            "-o",
            "Upstream PR",
        ],
    );
    assert_eq!(v["question"]["id"], "Q1");
    assert_eq!(v["question"]["options"][1]["letter"], "b");
    assert_eq!(v["waiting_on"], "owner");
    let v = t.json(
        "s1",
        &["issue", "ask", "ALX-1", "Name?", "--context", "Short."],
    );
    assert_eq!(v["question"]["id"], "Q2");
    assert_eq!(v["question"]["context"], "Short.");

    let v = t.json("s1", &["issue", "questions"]);
    let ids: Vec<&str> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["Q1", "Q2"]);
    assert_eq!(v[0]["key"], "ALX-1");

    let v = t.json("s1", &["issue", "answer", "ALX-1", "Q1", "b"]);
    assert_eq!(v["question"]["answer"], "b) Upstream PR");
    assert_eq!(v["open"], 1);
    assert_eq!(v["waiting_on"], "owner");
    let v = t.json("s1", &["issue", "answer", "ALX-1", "2", "lt"]);
    assert_eq!(v["open"], 0);
    assert_eq!(v["waiting_on"], "agent");
    let v = t.json("s1", &["issue", "questions"]);
    assert_eq!(v.as_array().unwrap().len(), 0);
    let v = t.json("s1", &["issue", "questions", "ALX-1", "--all"]);
    assert_eq!(v.as_array().unwrap().len(), 2);

    // Bad input: one option, an unknown question.
    assert_eq!(
        t.code("s1", &["issue", "ask", "ALX-1", "Only?", "-o", "x"]),
        2
    );
    assert_eq!(t.code("s1", &["issue", "answer", "ALX-1", "Q9", "x"]), 6);

    // The actor who answered can take the last answer back.
    let v = t.json("s1", &["issue", "undo-answer", "ALX-1", "q2"]);
    assert_eq!(v["question"]["id"], "Q2");
    assert_eq!(v["question"]["answer"], serde_json::Value::Null);
    assert_eq!(v["open"], 1);
    assert_eq!(v["waiting_on"], "owner");
    // A second undo finds nothing to take back.
    assert_eq!(t.code("s1", &["issue", "undo-answer", "ALX-1", "Q2"]), 6);
    t.stop().await;
}

/// LLM-108: agents see the same set the app shows as Locked or Analyzing.
#[tokio::test(flavor = "multi_thread")]
async fn query_narrows_to_claimed_and_analyzing() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "ALX", "Alex"]);
    for title in ["Locked", "Reading", "Idle"] {
        t.json("s1", &["issue", "create", "-t", title]);
    }
    t.json("s1", &["issue", "claim", "ALX-1"]);
    t.json("s2", &["issue", "analyze", "ALX-2"]);
    let keys = |flags: &[&str]| {
        let mut args = vec!["issue", "query", "--fields", "key"];
        args.extend(flags);
        let mut keys: Vec<String> = t
            .json("s3", &args)
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap().to_string())
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(keys(&["--claimed"]), ["ALX-1"]);
    assert_eq!(keys(&["--analyzing"]), ["ALX-2"]);
    assert_eq!(keys(&["--claimed", "--analyzing"]), ["ALX-1", "ALX-2"]);
    assert_eq!(keys(&[]).len(), 3);
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn estimates_gate_claims_and_stay_frozen() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "LLM", "llmtodo"]);
    let created = t
        .bare_cmd("s1", &["--json", "issue", "create", "-t", "No estimates"])
        .output()
        .unwrap();
    assert!(created.status.success());

    let claim = ["--json", "issue", "claim", "LLM-1"];
    let refused = t.bare_cmd("s1", &claim).output().unwrap();
    assert_eq!(refused.status.code(), Some(6));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    let stdout = String::from_utf8_lossy(&refused.stdout);
    assert!(
        format!("{stdout}{stderr}").contains("--estimate-minutes"),
        "the error says how to set them: {stdout}{stderr}"
    );

    t.json(
        "s1",
        &[
            "issue",
            "update",
            "LLM-1",
            "--estimate-minutes",
            "20",
            "--estimate-cost",
            "0.75",
        ],
    );
    t.json("s1", &claim[1..]);
    let v = t.json("s1", &["issue", "view", "LLM-1"]);
    assert_eq!(v["estimate_minutes"], 20);
    assert_eq!(v["estimate_cost_usd"], 0.75);

    let change = ["issue", "update", "LLM-1", "--estimate-minutes", "40"];
    assert_eq!(t.code("s1", &change), 6);
    let mut with_reason = change.to_vec();
    with_reason.extend(["--estimate-reason", "The spec grew."]);
    t.json("s1", &with_reason);
    assert_eq!(
        t.json("s1", &["issue", "view", "LLM-1"])["estimate_minutes"],
        40
    );
    t.stop().await;
}

/// The review flow from the CLI: the section, the gate, an answer, and a
/// wait that hears it.
#[tokio::test(flavor = "multi_thread")]
async fn review_set_answer_and_wait() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "LLM", "llmtodo"]);
    t.json(
        "s1",
        &[
            "status",
            "create",
            "LLM",
            "User review",
            "--type",
            "started",
            "--review",
        ],
    );
    t.json("s1", &["issue", "create", "-t", "Panel", "--team", "LLM"]);
    assert_eq!(
        t.code(
            "s1",
            &[
                "--no-input",
                "issue",
                "update",
                "LLM-1",
                "--status",
                "User review"
            ]
        ),
        6
    );
    let r = t.json(
        "s1",
        &[
            "--json",
            "issue",
            "review",
            "set",
            "LLM-1",
            "--step",
            "Open the app.",
            "--step",
            "Press c.",
            "--check",
            "A window opens.",
        ],
    );
    assert_eq!(r["section"]["steps"][1], "Press c.");
    let v = t.json(
        "s1",
        &[
            "--json",
            "issue",
            "update",
            "LLM-1",
            "--status",
            "User review",
        ],
    );
    assert_eq!(v["waiting_on"], "owner");

    // Start from the newest seq, so the wait cannot miss the answer.
    let seq = t.json("s1", &["--json", "activity", "--limit", "1"])["last_seq"]
        .as_i64()
        .unwrap()
        .to_string();
    let mut wait = t.bare_cmd(
        "s2",
        &[
            "--json",
            "issue",
            "review",
            "wait",
            "--after-seq",
            &seq,
            "--timeout",
            "30",
        ],
    );
    let waiter = std::thread::spawn(move || wait.output().unwrap());
    let a = t.json(
        "s1",
        &[
            "--json",
            "issue",
            "review",
            "answer",
            "LLM-1",
            "request-changes",
            "-m",
            "Make it bigger.",
        ],
    );
    assert_eq!(a["status"], "Todo");
    let out = waiter.join().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let heard: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(heard["key"], "LLM-1");
    assert_eq!(heard["verdict"], "changes_requested");
    assert_eq!(heard["feedback"], "Make it bigger.");
    assert_eq!(heard["round"], 1);
    assert_eq!(heard["status"], "Todo");
    assert_eq!(heard["waiting_on"], "agent");
    t.stop().await;
}

/// `llmtodo app` opens the installed bundle, and says so clearly when there
/// is none. `LLMTODO_OPEN_COMMAND` stands in for `open`.
#[test]
fn app_opens_the_installed_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("llmtodo.app");
    let run = || {
        let mut c = Command::cargo_bin("llmtodo").unwrap();
        c.env_clear()
            .env("HOME", dir.path())
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("LLMTODO_DATA_DIR", dir.path())
            .env("LLMTODO_APP_BUNDLE", &bundle)
            .env("LLMTODO_OPEN_COMMAND", "echo")
            .current_dir(dir.path())
            .args(["app", "--json"]);
        c.output().unwrap()
    };
    let out = run();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no app is installed") && err.contains("scripts/publish"),
        "{err}"
    );

    std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
    std::fs::write(bundle.join("Contents/Info.plist"), "<plist/>").unwrap();
    std::fs::write(bundle.join("Contents/Resources/build-commit"), "abc123\n").unwrap();
    let out = run();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["commit"], "abc123");
}

/// `llmtodo init` writes `.llmtodo.toml` at the git root, keeps an existing
/// file, and installs the skill in a temporary HOME.
#[tokio::test(flavor = "multi_thread")]
async fn init_writes_config_and_installs_skill() {
    let t = T::new().await;
    let repo = t.dir.path().join("my-repo");
    let sub = repo.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    let init = |args: &[&str]| {
        let mut all = vec!["init", "--no-input"];
        all.extend_from_slice(args);
        let mut c = t.bare_cmd("s1", &all);
        c.current_dir(&sub);
        c.output().unwrap()
    };
    let ok = |out: std::process::Output| -> Value {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    };

    // No team yet: init asks for flags instead of a prompt.
    assert_eq!(init(&[]).status.code(), Some(8));
    // A missing team needs --create.
    assert_eq!(init(&["--team", "ABC"]).status.code(), Some(2));

    let v = ok(init(&[
        "--team",
        "abc",
        "--team-name",
        "Alpha",
        "--project",
        "my-repo",
        "--create",
    ]));
    assert_eq!(v["team"], "ABC");
    assert_eq!(v["project"], "my-repo");
    assert_eq!(v["created_team"], true);
    assert_eq!(v["created_project"], true);
    assert_eq!(v["changed"], true);
    let toml = std::fs::read_to_string(repo.join(".llmtodo.toml")).unwrap();
    assert!(
        toml.contains("team = \"ABC\"") && toml.contains("project = \"my-repo\""),
        "{toml}"
    );
    let skill = t.dir.path().join(".claude/skills/llmtodo");
    assert!(skill.join("SKILL.md").is_file() && skill.join("guide.md").is_file());
    let team = t.json("s1", &["team", "view", "ABC"]);
    assert_eq!(team["name"], "Alpha");

    // Again with the same values: nothing changes.
    let v = ok(init(&["--team", "ABC", "--project", "my-repo"]));
    assert_eq!(v["changed"], false);
    let v = ok(init(&[]));
    assert_eq!(v["changed"], false);
    assert_eq!(v["team"], "ABC");

    // Other values need --force.
    t.json("s1", &["team", "create", "XYZ", "Other"]);
    assert_eq!(init(&["--team", "XYZ"]).status.code(), Some(2));
    let v = ok(init(&["--team", "XYZ", "--force", "--no-skill"]));
    assert_eq!(v["team"], "XYZ");
    assert_eq!(v["project"], Value::Null);
    assert_eq!(v["skill"], Value::Null);

    // A skill file someone else wrote is kept, and --force backs it up.
    std::fs::write(skill.join("SKILL.md"), "mine\n").unwrap();
    let out = t.bare_cmd("s1", &["skill", "install"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        "mine\n"
    );
    let v = t.json("s1", &["skill", "status"]);
    assert_eq!(v["files"][0]["state"], "foreign");
    let v = t.json("s1", &["skill", "install", "--force"]);
    assert_eq!(v["files"][0]["action"], "replaced");
    let backup = v["files"][0]["backup"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(backup).unwrap(), "mine\n");
    let v = t.json("s1", &["skill", "status"]);
    assert_eq!(v["files"][0]["state"], "current");
    t.stop().await;
}

/// The skill in this repo is the one the binary installs.
#[test]
fn repo_skill_matches_the_installed_skill() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for file in ["SKILL.md", "guide.md"] {
        let out = Command::cargo_bin("llmtodo")
            .unwrap()
            .args(["skill", "print", file])
            .output()
            .unwrap();
        let repo = std::fs::read(root.join(".claude/skills/llmtodo").join(file)).unwrap();
        assert_eq!(out.stdout, repo, "{file} differs");
        let text = String::from_utf8(repo).unwrap();
        assert!(
            !text.contains("docs/agents"),
            "{file} points at repo-only docs"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn order_set_show_step_and_clear() {
    let t = T::new().await;
    t.json("s1", &["team", "create", "LLM", "llmtodo"]);
    t.json("s1", &["issue", "create", "-t", "Epic"]);
    for name in ["a", "b", "c", "d"] {
        t.json("s1", &["issue", "create", "-t", name, "--parent", "LLM-1"]);
    }
    let steps = |v: &Value| -> Vec<Vec<String>> {
        v["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                s["issues"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| l["key"].as_str().unwrap().to_string())
                    .collect()
            })
            .collect()
    };
    // Bare numbers take the parent's team, and a comma list is one step.
    let v = t.json("s1", &["issue", "order", "set", "LLM-1", "2", "3,LLM-4"]);
    assert_eq!(
        steps(&v),
        [
            vec!["LLM-2".to_string()],
            vec!["LLM-3".into(), "LLM-4".into()]
        ]
    );
    assert_eq!(v["unordered"][0]["key"], "LLM-5");
    let v = t.json("s1", &["issue", "order", "show", "LLM-1"]);
    assert_eq!(steps(&v).len(), 2);
    let i = t.json("s1", &["issue", "view", "LLM-4"]);
    assert_eq!(i["order_step"], 2);
    assert_eq!(i["depth"], 1);
    // The frontier holds the first step and the task with no step.
    let f = t.json("s1", &["issue", "frontier", "LLM-1", "--fields", "key"]);
    let keys: Vec<&str> = f
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys.len(), 2, "{f}");
    assert!(keys.contains(&"LLM-2") && keys.contains(&"LLM-5"));

    // A key that is not a sub-issue is a validation error.
    t.json("s1", &["issue", "create", "-t", "Other"]);
    assert_eq!(
        t.code("s1", &["issue", "order", "set", "LLM-1", "2", "6"]),
        6
    );
    // An order edge is not removed as a relation.
    assert_eq!(
        t.code(
            "s1",
            &[
                "issue",
                "relation",
                "remove",
                "LLM-3",
                "blocked-by",
                "LLM-2"
            ]
        ),
        6
    );

    // --order-step moves one task.
    let i = t.json("s1", &["issue", "update", "LLM-5", "--order-step", "3"]);
    assert_eq!(i["order_step"], 3);
    let i = t.json("s1", &["issue", "update", "LLM-3", "--order-step", "none"]);
    assert_eq!(i["order_step"], Value::Null);
    let v = t.json("s1", &["issue", "order", "show", "LLM-1"]);
    assert_eq!(
        steps(&v),
        [
            vec!["LLM-2".to_string()],
            vec!["LLM-4".into()],
            vec!["LLM-5".into()]
        ]
    );
    assert_eq!(
        t.code("s1", &["issue", "update", "LLM-3", "--order-step", "x"]),
        2
    );

    let v = t.json("s1", &["issue", "order", "clear", "LLM-1"]);
    assert!(steps(&v).is_empty());
    assert_eq!(t.json("s1", &["issue", "view", "LLM-5"])["depth"], 0);
    t.stop().await;
}

/// LLM-213: the project's local folder. `init` saves the repo's main
/// checkout, a call from a worktree fills it when it is empty, and
/// `project update --local-path` sets and clears it.
#[tokio::test(flavor = "multi_thread")]
async fn projects_remember_their_local_folder() {
    let t = T::new().await;
    let repo = t.dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "base"]);
    let main = repo.canonicalize().unwrap().to_string_lossy().into_owned();
    let path = |t: &T| t.json("s1", &["project", "view", "app"])["local_path"].clone();

    let mut c = t.bare_cmd(
        "s1",
        &[
            "init",
            "--no-input",
            "--no-skill",
            "--team",
            "ABC",
            "--project",
            "app",
            "--create",
        ],
    );
    assert!(c.current_dir(&repo).output().unwrap().status.success());
    assert_eq!(path(&t), Value::from(main.clone()));

    t.json("s1", &["project", "update", "app", "--local-path", ""]);
    assert_eq!(path(&t), Value::Null);

    // The server already runs, so this call does not start it. A call
    // from a worktree saves the main checkout, not the worktree.
    git(&["add", ".llmtodo.toml"]);
    git(&["commit", "-q", "-m", "init"]);
    git(&["worktree", "add", "-q", "-b", "side", "wt"]);
    let mut c = t.bare_cmd("s1", &["team", "list"]);
    assert!(
        c.current_dir(repo.join("wt"))
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(path(&t), Value::from(main.clone()));

    // A set folder stays: a call from elsewhere does not replace it.
    t.json(
        "s1",
        &["project", "update", "app", "--local-path", "/tmp/other"],
    );
    let mut c = t.bare_cmd("s1", &["team", "list"]);
    assert!(c.current_dir(&repo).output().unwrap().status.success());
    assert_eq!(path(&t), Value::from("/tmp/other"));

    // A relative folder becomes absolute.
    t.json("s1", &["project", "update", "app", "--local-path", "sub"]);
    let want = t.dir.path().canonicalize().unwrap().join("sub");
    let want = want.to_string_lossy().into_owned();
    assert_eq!(path(&t), Value::from(want));
    t.stop().await;
}
