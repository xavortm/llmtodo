//! Server integration tests against a temporary data folder.

use base64::Engine;
use llmtodo_api_types::*;
use serde_json::{Value, json};

struct T {
    url: String,
    http: reqwest::Client,
    running: Option<llmtodo_server::Running>,
    _dir: tempfile::TempDir,
}

#[derive(Clone)]
struct Who(ClientContext);

fn owner() -> Who {
    Who(ClientContext {
        session_id: Some("owner-session".into()),
        ..Default::default()
    })
}

fn agent(session: &str) -> Who {
    Who(ClientContext {
        actor: ActorInput {
            kind: ActorKind::Agent,
            name: None,
            harness: Some("claude-code".into()),
            model: Some("opus-5.5".into()),
            effort: Some("high".into()),
        },
        session_id: Some(session.into()),
        host: Some("test-host".into()),
        branch: Some(format!("{session}-branch")),
        head: Some("abc1234".into()),
        ..Default::default()
    })
}

impl T {
    async fn new() -> Self {
        let t = Self::bare(None).await;
        t.ok(
            &owner(),
            "POST",
            "/v1/teams",
            json!({ "key": "ALX", "name": "Alex" }),
        )
        .await;
        t
    }

    /// A server with no team, as on a fresh machine.
    async fn bare(host: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let running = llmtodo_server::start(llmtodo_server::Config {
            data_dir: dir.path().to_path_buf(),
            port: 0,
            host: host.map(str::to_string),
            github_poll: false,
        })
        .await
        .unwrap();
        let url = running.url();
        T {
            url,
            http: reqwest::Client::new(),
            running: Some(running),
            _dir: dir,
        }
    }

    async fn call(&self, who: &Who, method: &str, path: &str, body: Value) -> (u16, Value) {
        let ctx = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&who.0).unwrap());
        let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut req = self
            .http
            .request(m, format!("{}{path}", self.url))
            .header(CONTEXT_HEADER, ctx);
        if method != "GET" {
            req = req.json(&body);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let v = res.json().await.unwrap_or(Value::Null);
        (status, v)
    }

    async fn ok(&self, who: &Who, method: &str, path: &str, body: Value) -> Value {
        let (s, v) = self.call(who, method, path, body).await;
        assert!(s < 300, "{method} {path} -> {s}: {v}");
        v
    }

    async fn err(&self, who: &Who, method: &str, path: &str, body: Value) -> String {
        let (s, v) = self.call(who, method, path, body).await;
        assert!(s >= 400, "{method} {path} should fail: {v}");
        v["error"]["code"].as_str().unwrap().to_string()
    }

    async fn issue(&self, title: &str, extra: Value) -> String {
        // Tests that claim need estimates, so tasks get them by default.
        // Pass nulls in `extra` for a task without estimates.
        let mut body = json!({ "title": title, "estimate_minutes": 30, "estimate_cost_usd": 1.5 });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        self.ok(&owner(), "POST", "/v1/issues", body).await["key"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn stop(mut self) {
        self.running.take().unwrap().stop().await.unwrap();
    }
}

#[tokio::test]
async fn create_view_query_update() {
    let t = T::new().await;
    let a = t.issue("Fix the login page", json!({ "description": "Users cannot log in.\n\n+++ Agent notes\nnone\n+++\n", "priority": "high" })).await;
    assert_eq!(a, "ALX-1");
    let b = t.issue("Add dark mode", json!({ "parent": "ALX-1" })).await;
    assert_eq!(b, "ALX-2");

    let v = t.ok(&owner(), "GET", "/v1/issues/alx-1", Value::Null).await;
    assert_eq!(v["status"]["name"], "Todo");
    assert_eq!(v["children"][0]["key"], "ALX-2");
    assert_eq!(v["description_version"], 1);

    let page = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "text": "login" }),
        )
        .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert!(page["items"][0]["score"].as_f64().is_some());

    // Default sort: priority first, then no priority.
    let page = t.ok(&owner(), "POST", "/v1/issues/query", json!({})).await;
    let keys: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["ALX-1", "ALX-2"]);

    // Patch a section without a version.
    let v = t
        .ok(&owner(), "PATCH", "/v1/issues/ALX-1", json!({ "patches": [{ "op": "replace_section", "section": "Agent notes", "body": "plan: rewrite" }] }))
        .await;
    assert!(
        v["description"]
            .as_str()
            .unwrap()
            .contains("+++ Agent notes\nplan: rewrite\n+++")
    );
    assert_eq!(v["description_version"], 2);

    // A full replace needs the current version.
    assert_eq!(
        t.err(
            &owner(),
            "PATCH",
            "/v1/issues/ALX-1",
            json!({ "body": "x" })
        )
        .await,
        "bad_request"
    );
    assert_eq!(
        t.err(
            &owner(),
            "PATCH",
            "/v1/issues/ALX-1",
            json!({ "body": "x", "base_version": 1 })
        )
        .await,
        "stale_version"
    );
    t.ok(
        &owner(),
        "PATCH",
        "/v1/issues/ALX-1",
        json!({ "body": "x", "base_version": 2 }),
    )
    .await;
    assert_eq!(
        t.err(
            &owner(),
            "PATCH",
            "/v1/issues/ALX-1",
            json!({ "patches": [{ "op": "replace_exact", "old": "nope", "new": "y" }] })
        )
        .await,
        "patch_mismatch"
    );

    // History and restore of a revision.
    let h = t
        .ok(&owner(), "GET", "/v1/issues/ALX-1/history", Value::Null)
        .await;
    assert_eq!(h["revisions"].as_array().unwrap().len(), 3);
    // The list leaves out each version's text unless asked.
    assert!(h["revisions"][0].get("body").is_none());
    let full = t
        .ok(
            &owner(),
            "GET",
            "/v1/issues/ALX-1/history?bodies=true",
            Value::Null,
        )
        .await;
    assert!(full["revisions"][0]["body"].is_string());
    let v = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/ALX-1/restore",
            json!({ "revision": 1 }),
        )
        .await;
    assert!(
        v["description"]
            .as_str()
            .unwrap()
            .starts_with("Users cannot log in.")
    );

    // Parent cycle.
    assert_eq!(
        t.err(
            &owner(),
            "PATCH",
            "/v1/issues/ALX-1",
            json!({ "parent": "ALX-2" })
        )
        .await,
        "validation"
    );

    // Trash and restore.
    t.ok(&owner(), "DELETE", "/v1/issues/ALX-2", Value::Null)
        .await;
    assert_eq!(
        t.err(
            &owner(),
            "PATCH",
            "/v1/issues/ALX-2",
            json!({ "title": "y" })
        )
        .await,
        "not_found"
    );
    let trash = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "trashed": true }),
        )
        .await;
    assert_eq!(trash["items"][0]["key"], "ALX-2");
    t.ok(&owner(), "POST", "/v1/issues/ALX-2/restore", json!({}))
        .await;
    t.stop().await;
}

#[tokio::test]
async fn labels_and_groups() {
    let t = T::new().await;
    t.ok(
        &owner(),
        "POST",
        "/v1/labels",
        json!({ "name": "Area", "is_group": true }),
    )
    .await;
    t.ok(
        &owner(),
        "POST",
        "/v1/labels",
        json!({ "name": "fe", "group": "Area" }),
    )
    .await;
    t.ok(
        &owner(),
        "POST",
        "/v1/labels",
        json!({ "name": "be", "group": "Area" }),
    )
    .await;
    t.ok(
        &owner(),
        "POST",
        "/v1/labels",
        json!({ "name": "needs-triage" }),
    )
    .await;
    let k = t
        .issue("Thing", json!({ "labels": ["fe", "needs-triage"] }))
        .await;
    assert_eq!(
        t.err(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "add_labels": ["Area/be"] })
        )
        .await,
        "validation"
    );
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "remove_labels": ["fe"], "add_labels": ["be"] }),
        )
        .await;
    assert_eq!(v["labels"], json!(["Area/be", "needs-triage"]));
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "remove_labels": ["needs-triage"] }),
        )
        .await;
    assert_eq!(v["labels"], json!(["Area/be"]));
    let q = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "labels": ["be"] }),
        )
        .await;
    assert_eq!(q["items"].as_array().unwrap().len(), 1);
    t.stop().await;
}

#[tokio::test]
async fn claims_and_work_log() {
    let t = T::new().await;
    let k = t.issue("Work", json!({})).await;
    let blocker = t.issue("Blocker", json!({})).await;
    t.ok(
        &owner(),
        "POST",
        &format!("/v1/issues/{blocker}/relations"),
        json!({ "type": "blocks", "issue": k }),
    )
    .await;

    let r = t
        .ok(
            &agent("s1"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({}),
        )
        .await;
    assert_eq!(r["changed"], true);
    assert_eq!(r["issue"]["status"]["name"], "In Progress");
    assert_eq!(r["issue"]["assignee"]["name"], "claude-code/opus-5.5/high");
    assert_eq!(r["open_blockers"][0]["key"], blocker);
    assert!(!r["warnings"].as_array().unwrap().is_empty());

    // Same session again changes nothing.
    let r = t
        .ok(
            &agent("s1"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({}),
        )
        .await;
    assert_eq!(r["changed"], false);
    // Another session fails, unless it steals.
    assert_eq!(
        t.err(
            &agent("s2"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({})
        )
        .await,
        "claim_held"
    );
    let r = t
        .ok(
            &agent("s2"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({ "steal": true }),
        )
        .await;
    assert_eq!(r["stolen_from"]["session_id"], "s1");

    // Release frees it and moves it back to Todo.
    assert_eq!(
        t.err(
            &agent("s1"),
            "POST",
            &format!("/v1/issues/{k}/release"),
            json!({})
        )
        .await,
        "claim_held"
    );
    let r = t
        .ok(
            &agent("s2"),
            "POST",
            &format!("/v1/issues/{k}/release"),
            json!({}),
        )
        .await;
    assert_eq!(r["status"]["name"], "Todo");
    assert!(r["assignee"].is_null());

    // Completing ends the claim; a completed issue cannot be claimed.
    t.ok(
        &agent("s3"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    let v = t
        .ok(
            &agent("s3"),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "status": "Done" }),
        )
        .await;
    assert!(v["claim"].is_null());
    assert!(v["completed_at"].is_string());
    assert_eq!(
        t.err(
            &agent("s4"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({})
        )
        .await,
        "validation"
    );

    // The work log lists the sessions that wrote.
    let v = t
        .ok(&owner(), "GET", &format!("/v1/issues/{k}"), Value::Null)
        .await;
    let sessions: Vec<&str> = v["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["session_id"].as_str().unwrap())
        .collect();
    assert_eq!(sessions, ["s1", "s2", "s3"]);
    assert_eq!(v["sessions"][0]["first_head"], "abc1234");

    t.ok(
        &agent("s3"),
        "POST",
        "/v1/sessions/s3/notes",
        json!({ "body": "plan in /tmp/plan.md" }),
    )
    .await;
    let s = t.ok(&owner(), "GET", "/v1/sessions/s3", Value::Null).await;
    assert_eq!(s["issues"][0]["issue"], k);
    assert_eq!(s["notes"][0]["body"], "plan in /tmp/plan.md");
    t.stop().await;
}

#[tokio::test]
async fn twenty_claimers_one_owner_each() {
    let t = T::new().await;
    let map = t.issue("Map", json!({})).await;
    for i in 0..10 {
        t.issue(&format!("Task {i}"), json!({ "parent": map }))
            .await;
    }
    let t = std::sync::Arc::new(t);
    let mut tasks = Vec::new();
    for n in 0..20 {
        let t = t.clone();
        let map = map.clone();
        tasks.push(tokio::spawn(async move {
            let me = agent(&format!("claimer-{n}"));
            let mut won = Vec::new();
            loop {
                let (_, front) = t
                    .call(&me, "POST", "/v1/issues/frontier", json!({ "issue": map }))
                    .await;
                let Some(first) = front.as_array().and_then(|a| a.first()).cloned() else {
                    break;
                };
                let key = first["key"].as_str().unwrap().to_string();
                let (s, _) = t
                    .call(&me, "POST", &format!("/v1/issues/{key}/claim"), json!({}))
                    .await;
                if s < 300 {
                    won.push(key);
                }
            }
            won
        }));
    }
    let mut all = Vec::new();
    for task in tasks {
        all.extend(task.await.unwrap());
    }
    all.sort();
    let before = all.len();
    all.dedup();
    assert_eq!(before, all.len(), "an issue was won twice");
    assert_eq!(all.len(), 10);
    let t = std::sync::Arc::into_inner(t).unwrap();
    t.stop().await;
}

#[tokio::test]
async fn dependencies_depth_tree_and_cycles() {
    let t = T::new().await;
    let c = t.issue("C", json!({})).await;
    let b = t
        .issue("B", json!({ "relations": { "blocked-by": [c.clone()] } }))
        .await;
    let a = t
        .issue("A", json!({ "relations": { "blocked-by": [b.clone()] } }))
        .await;
    // D waits for A and also straight for C: the longest way counts.
    let d = t
        .issue(
            "D",
            json!({ "relations": { "blocked-by": [c.clone(), a.clone()] } }),
        )
        .await;

    let depth = async |key: &str| -> u64 {
        t.ok(&owner(), "GET", &format!("/v1/issues/{key}"), Value::Null)
            .await["depth"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(depth(&c).await, 0);
    assert_eq!(depth(&b).await, 1);
    assert_eq!(depth(&a).await, 2);
    assert_eq!(depth(&d).await, 3);

    // The list carries the same numbers, with the direct open blockers.
    let page = t.ok(&owner(), "POST", "/v1/issues/query", json!({})).await;
    let row = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["key"] == d)
        .unwrap()
        .clone();
    assert_eq!(row["depth"], 3);
    let keys: Vec<&str> = row["open_blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, [c.as_str(), a.as_str()]);

    // A cycle is refused, and the error names it.
    let (status, v) = t
        .call(
            &owner(),
            "POST",
            &format!("/v1/issues/{c}/relations"),
            json!({ "type": "blocked-by", "issue": a }),
        )
        .await;
    assert_eq!(status, 422, "{v}");
    assert_eq!(v["error"]["code"], "validation");
    let cycle: Vec<&str> = v["error"]["details"]["cycle"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    assert_eq!(cycle, [a.as_str(), c.as_str(), b.as_str(), a.as_str()]);
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&format!("{a} blocks {c} blocks {b} blocks {a}")),
        "{message}"
    );

    // The tree goes both ways, with depth and status on each node.
    let tree = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{b}/deps"),
            Value::Null,
        )
        .await;
    assert_eq!(tree["key"], b);
    assert_eq!(tree["depth"], 1);
    assert_eq!(tree["depends_on"][0]["key"], c);
    assert_eq!(tree["depends_on"][0]["status"]["name"], "Todo");
    assert_eq!(tree["blocks"][0]["key"], a);
    assert_eq!(tree["blocks"][0]["depth"], 2);
    assert_eq!(tree["blocks"][0]["blocks"][0]["key"], d);

    // A claim names the chain.
    let claim = t
        .ok(
            &agent("deps"),
            "POST",
            &format!("/v1/issues/{d}/claim"),
            json!({}),
        )
        .await;
    let warning = claim["warnings"][0].as_str().unwrap();
    assert!(
        warning.contains(&format!("{a} waits for {b} waits for {c}")),
        "{warning}"
    );

    // Dependents of an issue, through the chain.
    let found = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "depends_on": c }),
        )
        .await;
    let mut keys: Vec<&str> = found["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, [b.as_str(), a.as_str(), d.as_str()]);

    // Closing a blocker shortens every chain through it.
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{c}"),
        json!({ "status": "Done" }),
    )
    .await;
    assert_eq!(depth(&b).await, 0);
    assert_eq!(depth(&a).await, 1);
    assert_eq!(depth(&d).await, 2);

    // `open` hides the closed issue. Without it, the tree shows it.
    let open = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{b}/deps?open=true"),
            Value::Null,
        )
        .await;
    assert!(open.get("depends_on").is_none(), "{open}");
    let all = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{b}/deps"),
            Value::Null,
        )
        .await;
    assert_eq!(all["depends_on"][0]["status"]["name"], "Done");

    // A closed issue has no depth, even with open blockers.
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{d}"),
        json!({ "status": "Canceled" }),
    )
    .await;
    assert_eq!(depth(&d).await, 0);

    // A trashed blocker stops counting.
    t.ok(&owner(), "DELETE", &format!("/v1/issues/{b}"), json!({}))
        .await;
    assert_eq!(depth(&a).await, 0);
    t.stop().await;
}

/// LLM-216: the server keeps the dependency graph in memory. Every write
/// that changes it must show in the next read.
#[tokio::test]
async fn the_dependency_graph_follows_every_write() {
    let t = T::new().await;
    let a = t.issue("A", json!({})).await;
    let b = t.issue("B", json!({})).await;
    let get = async |key: &str| -> Value {
        t.ok(&owner(), "GET", &format!("/v1/issues/{key}"), Value::Null)
            .await
    };
    let status = async |key: &str, name: &str| {
        t.ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{key}"),
            json!({ "status": name }),
        )
        .await;
    };
    assert_eq!(get(&b).await["depth"], 0);

    // A blocks B.
    t.ok(
        &owner(),
        "POST",
        &format!("/v1/issues/{a}/relations"),
        json!({ "type": "blocks", "issue": b }),
    )
    .await;
    assert_eq!(get(&b).await["depth"], 1);
    assert_eq!(get(&b).await["open_blockers"][0]["key"], a.as_str());

    // Closing A ends the chain, and A is held by the open B.
    status(&a, "Done").await;
    assert_eq!(get(&b).await["depth"], 0);
    assert_eq!(get(&a).await["has_open_dependents"], true);
    // Closing B frees A.
    status(&b, "Done").await;
    assert_eq!(get(&a).await["has_open_dependents"], false);
    status(&b, "Todo").await;
    status(&a, "Todo").await;
    assert_eq!(get(&b).await["depth"], 1);

    // Trash and restore.
    t.ok(&owner(), "DELETE", &format!("/v1/issues/{a}"), json!({}))
        .await;
    assert_eq!(get(&b).await["depth"], 0);
    t.ok(
        &owner(),
        "POST",
        &format!("/v1/issues/{a}/restore"),
        json!({}),
    )
    .await;
    assert_eq!(get(&b).await["depth"], 1);

    // Removing the relation.
    t.ok(
        &owner(),
        "POST",
        &format!("/v1/issues/{a}/relations/remove"),
        json!({ "type": "blocks", "issue": b }),
    )
    .await;
    assert_eq!(get(&b).await["depth"], 0);
    assert_eq!(get(&b).await["open_blockers"], json!([]));
    t.stop().await;
}

/// LLM-214: a closed issue that an open issue still waits for, directly or
/// through other closed issues, has `has_open_dependents`.
#[tokio::test]
async fn closed_issues_held_by_open_dependents() {
    let t = T::new().await;
    // A blocks B blocks C. E is closed and only a closed issue waits for it.
    let a = t.issue("A", json!({})).await;
    let b = t
        .issue("B", json!({ "relations": { "blocked-by": [a.clone()] } }))
        .await;
    let c = t
        .issue("C", json!({ "relations": { "blocked-by": [b.clone()] } }))
        .await;
    let e = t.issue("E", json!({})).await;
    let f = t
        .issue("F", json!({ "relations": { "blocked-by": [e.clone()] } }))
        .await;
    let status = async |key: &str, name: &str| {
        t.ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{key}"),
            json!({ "status": name }),
        )
        .await;
    };
    let held = async |key: &str| -> bool {
        t.ok(&owner(), "GET", &format!("/v1/issues/{key}"), Value::Null)
            .await["has_open_dependents"]
            .as_bool()
            .unwrap()
    };
    // An open issue is never held.
    assert!(!held(&a).await);
    for key in [&a, &b, &e, &f] {
        status(key, "Done").await;
    }
    // C is open, so A and B stay held, A through the closed B.
    assert!(held(&a).await);
    assert!(held(&b).await);
    assert!(!held(&c).await);
    // Only a closed issue waits for E.
    assert!(!held(&e).await);
    assert!(!held(&f).await);

    // The list carries the same flag.
    let page = t.ok(&owner(), "POST", "/v1/issues/query", json!({})).await;
    let flag = |key: &str| {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["key"] == key)
            .unwrap()["has_open_dependents"]
            .clone()
    };
    assert_eq!(flag(&a), true);
    assert_eq!(flag(&e), false);

    // The closed blockers of C, through closed issues.
    let found = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "closed_blockers_of": c }),
        )
        .await;
    let mut keys: Vec<&str> = found["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, [a.as_str(), b.as_str()]);

    // When the whole chain is closed, nothing holds it.
    status(&c, "Canceled").await;
    assert!(!held(&a).await);
    assert!(!held(&b).await);
    // Reopening the last step holds the chain again.
    status(&c, "Todo").await;
    assert!(held(&a).await);
    // A trashed dependent holds nothing.
    t.ok(&owner(), "DELETE", &format!("/v1/issues/{c}"), json!({}))
        .await;
    assert!(!held(&a).await);
    // A trashed issue sits outside the graph, but its closed blockers are
    // still found, so the app can refresh them.
    let found = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "closed_blockers_of": c }),
        )
        .await;
    let mut keys: Vec<&str> = found["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, [a.as_str(), b.as_str()]);
    t.stop().await;
}

#[tokio::test]
async fn relations_cycles_and_duplicates() {
    let t = T::new().await;
    let a = t.issue("A", json!({})).await;
    let b = t
        .issue("B", json!({ "relations": { "blocked-by": [a.clone()] } }))
        .await;
    let c = t.issue("C", json!({})).await;
    t.ok(
        &owner(),
        "POST",
        &format!("/v1/issues/{b}/relations"),
        json!({ "type": "blocks", "issue": c }),
    )
    .await;
    assert_eq!(
        t.err(
            &owner(),
            "POST",
            &format!("/v1/issues/{c}/relations"),
            json!({ "type": "blocks", "issue": a })
        )
        .await,
        "validation"
    );

    // Frontier skips blocked issues, and a done blocker stops counting.
    let p = t
        .ok(&owner(), "POST", "/v1/projects", json!({ "name": "Proj" }))
        .await;
    for k in [&a, &b, &c] {
        t.ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "project": p["slug"] }),
        )
        .await;
    }
    let f = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/frontier",
            json!({ "project": "proj" }),
        )
        .await;
    assert_eq!(
        f.as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [a.as_str()]
    );
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{a}"),
        json!({ "status": "Done" }),
    )
    .await;
    let f = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/frontier",
            json!({ "project": "proj" }),
        )
        .await;
    assert_eq!(
        f.as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [b.as_str()]
    );

    // Duplicate needs a relation, and the relation sets the status.
    let d = t.issue("A again", json!({})).await;
    assert_eq!(
        t.err(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{d}"),
            json!({ "status": "Duplicate" })
        )
        .await,
        "validation"
    );
    let v = t
        .ok(
            &owner(),
            "POST",
            &format!("/v1/issues/{d}/relations"),
            json!({ "type": "duplicate", "issue": a }),
        )
        .await;
    assert_eq!(v["status"]["name"], "Duplicate");
    assert_eq!(v["relations"][0]["direction"], "duplicate");

    let v = t
        .ok(
            &owner(),
            "POST",
            &format!("/v1/issues/{b}/relations/remove"),
            json!({ "type": "blocks", "issue": c }),
        )
        .await;
    assert!(
        v["relations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["direction"] != "blocks")
    );

    // Similar search finds the duplicate candidate.
    let s = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "similar": "again" }),
        )
        .await;
    assert_eq!(s["items"][0]["key"], d);
    t.stop().await;
}

#[tokio::test]
async fn team_move_keeps_old_key() {
    let t = T::new().await;
    t.ok(
        &owner(),
        "POST",
        "/v1/teams",
        json!({ "key": "WEB", "name": "Web" }),
    )
    .await;
    let k = t.issue("Move me", json!({ "team": "ALX" })).await;
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "team": "WEB" }),
        )
        .await;
    assert_eq!(v["key"], "WEB-1");
    assert_eq!(v["aliases"], json!(["ALX-1"]));
    let v = t.ok(&owner(), "GET", "/v1/issues/ALX-1", Value::Null).await;
    assert_eq!(v["key"], "WEB-1");
    // Keys are never reused.
    let n = t.issue("Next", json!({ "team": "ALX" })).await;
    assert_eq!(n, "ALX-2");
    t.stop().await;
}

#[tokio::test]
async fn a_query_by_keys_returns_only_those_issues() {
    let t = T::new().await;
    t.ok(
        &owner(),
        "POST",
        "/v1/teams",
        json!({ "key": "WEB", "name": "Web" }),
    )
    .await;
    let a = t.issue("One", json!({ "team": "ALX" })).await;
    let b = t.issue("Two", json!({ "team": "ALX" })).await;
    t.issue("Three", json!({ "team": "ALX" })).await;
    // An old key still names its issue after a team move.
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{b}"),
        json!({ "team": "WEB" }),
    )
    .await;
    let keys = |v: &Value| -> Vec<String> {
        let mut k: Vec<String> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap().to_string())
            .collect();
        k.sort();
        k
    };
    let page = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "keys": [a, b, "ALX-99"], "include_description": true }),
        )
        .await;
    assert_eq!(keys(&page), ["ALX-1", "WEB-1"]);
    // Only unknown keys: an empty page, not an error.
    let page = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "keys": ["ALX-99"] }),
        )
        .await;
    assert!(keys(&page).is_empty());
    // The trash filter still applies.
    t.ok(&owner(), "DELETE", &format!("/v1/issues/{a}"), Value::Null)
        .await;
    let live = t
        .ok(&owner(), "POST", "/v1/issues/query", json!({ "keys": [a] }))
        .await;
    assert!(keys(&live).is_empty());
    let trash = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "keys": [a], "trashed": true }),
        )
        .await;
    assert_eq!(keys(&trash), ["ALX-1"]);
    t.stop().await;
}

#[tokio::test]
async fn guards_and_prune() {
    let t = T::new().await;
    let res = t
        .http
        .get(format!("{}/v1/health", t.url))
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 403);
    let res = t
        .http
        .get(format!("{}/v1/health", t.url))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 403);
    let res = t
        .http
        .get(format!("{}/v1/health", t.url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()[VERSION_HEADER], VERSION);

    let k = t.issue("Old", json!({})).await;
    t.ok(
        &agent("s"),
        "DELETE",
        &format!("/v1/issues/{k}"),
        Value::Null,
    )
    .await;
    let future = "2999-01-01T00:00:00Z";
    assert_eq!(
        t.err(
            &agent("s"),
            "POST",
            "/v1/prune",
            json!({ "before": future })
        )
        .await,
        "forbidden"
    );
    let r = t
        .ok(
            &owner(),
            "POST",
            "/v1/prune",
            json!({ "before": future, "dry_run": true }),
        )
        .await;
    assert_eq!(r["issues"], json!([k]));
    let r = t
        .ok(&owner(), "POST", "/v1/prune", json!({ "before": future }))
        .await;
    assert!(r["backup"].is_string());
    assert_eq!(
        t.err(&owner(), "GET", &format!("/v1/issues/{k}"), Value::Null)
            .await,
        "not_found"
    );
    t.stop().await;
}

/// The owner confirms a dry-run list. Then an agent restores one issue and
/// trashes another. The confirmed prune must fail and delete nothing.
#[tokio::test]
async fn prune_deletes_only_what_the_owner_confirmed() {
    let t = T::new().await;
    let a = t.issue("Confirmed", json!({})).await;
    let b = t.issue("Trashed later", json!({})).await;
    t.ok(
        &agent("s"),
        "DELETE",
        &format!("/v1/issues/{a}"),
        Value::Null,
    )
    .await;
    let future = "2999-01-01T00:00:00Z";
    let preview = t
        .ok(
            &owner(),
            "POST",
            "/v1/prune",
            json!({ "before": future, "dry_run": true }),
        )
        .await;
    assert_eq!(preview["issues"], json!([a]));

    // A third issue goes to the trash before the owner answers.
    t.ok(
        &agent("s"),
        "DELETE",
        &format!("/v1/issues/{b}"),
        Value::Null,
    )
    .await;
    let code = t
        .err(
            &owner(),
            "POST",
            "/v1/prune",
            json!({ "before": future, "confirm_issues": [a], "confirm_revisions": 0 }),
        )
        .await;
    assert_eq!(code, "stale_version");

    // The confirmed issue is restored instead.
    t.ok(
        &agent("s"),
        "POST",
        &format!("/v1/issues/{b}/restore"),
        json!({}),
    )
    .await;
    t.ok(
        &agent("s"),
        "POST",
        &format!("/v1/issues/{a}/restore"),
        json!({}),
    )
    .await;
    let code = t
        .err(
            &owner(),
            "POST",
            "/v1/prune",
            json!({ "before": future, "confirm_issues": [a], "confirm_revisions": 0 }),
        )
        .await;
    assert_eq!(code, "stale_version");
    for k in [&a, &b] {
        let v = t
            .ok(&owner(), "GET", &format!("/v1/issues/{k}"), Value::Null)
            .await;
        assert!(v["deleted_at"].is_null(), "{k} must still be live: {v}");
    }

    // A matching confirmation deletes the confirmed issue.
    t.ok(
        &agent("s"),
        "DELETE",
        &format!("/v1/issues/{a}"),
        Value::Null,
    )
    .await;
    let r = t
        .ok(
            &owner(),
            "POST",
            "/v1/prune",
            json!({ "before": future, "confirm_issues": [a], "confirm_revisions": 0 }),
        )
        .await;
    assert_eq!(r["issues"], json!([a]));
    assert_eq!(
        t.err(&owner(), "GET", &format!("/v1/issues/{a}"), Value::Null)
            .await,
        "not_found"
    );
    t.stop().await;
}

#[tokio::test]
async fn activity_catch_up() {
    let t = T::new().await;
    let start = t.ok(&owner(), "GET", "/v1/activity", Value::Null).await["last_seq"]
        .as_i64()
        .unwrap();
    t.issue("One", NO_ESTIMATES()).await;
    t.issue("Two", NO_ESTIMATES()).await;
    let page = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/activity?after_seq={start}"),
            Value::Null,
        )
        .await;
    let fields: Vec<&str> = page["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["created", "created"]);
    assert_eq!(page["reload"], false);
    t.stop().await;
}

#[tokio::test]
async fn documents_and_views() {
    let t = T::new().await;
    t.ok(
        &owner(),
        "POST",
        "/v1/projects",
        json!({ "name": "Notebook", "teams": ["ALX"] }),
    )
    .await;
    let d = t.ok(&owner(), "POST", "/v1/documents", json!({ "project": "notebook", "title": "Game Rules", "body": "## Scoring\n\nOne point.\n" })).await;
    assert_eq!(d["slug"], "game-rules");
    let d = t.ok(&owner(), "PATCH", "/v1/documents/notebook%2Fgame-rules", json!({ "patches": [{ "op": "replace_section", "section": "Scoring", "body": "\nTwo points.\n" }] })).await;
    assert_eq!(d["version"], 2);
    assert!(d["body"].as_str().unwrap().contains("Two points."));
    // LLM-53: text search finds a term that only a document holds.
    t.ok(&owner(), "POST", "/v1/documents", json!({ "project": "notebook", "title": "Architecture", "body": "The server owns the websocket fanout.\n" })).await;
    let found = t
        .ok(&owner(), "GET", "/v1/documents?search=fanout", json!(null))
        .await;
    assert_eq!(found.as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found[0]["slug"], "architecture");
    let found = t
        .ok(
            &owner(),
            "GET",
            "/v1/documents?search=points&project=notebook",
            json!(null),
        )
        .await;
    assert_eq!(found[0]["slug"], "game-rules", "{found}");
    // A trashed document leaves the results, and shows with --trashed.
    t.ok(
        &owner(),
        "DELETE",
        "/v1/documents/notebook%2Farchitecture",
        json!(null),
    )
    .await;
    let found = t
        .ok(&owner(), "GET", "/v1/documents?search=fanout", json!(null))
        .await;
    assert_eq!(found, json!([]));
    let found = t
        .ok(
            &owner(),
            "GET",
            "/v1/documents?search=fanout&trashed=true",
            json!(null),
        )
        .await;
    assert_eq!(found[0]["slug"], "architecture", "{found}");
    let m = t
        .ok(
            &owner(),
            "POST",
            "/v1/projects/notebook/milestones",
            json!({ "name": "Beta", "target_date": "2026-12-01" }),
        )
        .await;
    assert_eq!(m["name"], "Beta");
    let k = t
        .issue(
            "Thing",
            json!({ "project": "notebook", "milestone": "Beta" }),
        )
        .await;
    t.ok(
        &owner(),
        "POST",
        "/v1/views",
        json!({ "name": "beta", "query": { "milestone": "Beta" } }),
    )
    .await;
    let q = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "view": "beta" }),
        )
        .await;
    assert_eq!(q["items"][0]["key"], k);
    t.stop().await;
}

#[tokio::test]
async fn websocket_drop_and_catch_up() {
    use futures_util::StreamExt;
    let t = T::new().await;
    let ws_url = format!("{}/v1/ws", t.url.replacen("http://", "ws://", 1));

    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let hello: Value = match ws.next().await.unwrap().unwrap() {
        tokio_tungstenite::tungstenite::Message::Text(t) => serde_json::from_str(&t).unwrap(),
        m => panic!("unexpected {m:?}"),
    };
    assert_eq!(hello["type"], "hello");
    let k = t.issue("Live", NO_ESTIMATES()).await;
    let event: Value = loop {
        if let tokio_tungstenite::tungstenite::Message::Text(t) = ws.next().await.unwrap().unwrap()
        {
            break serde_json::from_str(&t).unwrap();
        }
    };
    assert_eq!(event["type"], "event");
    assert_eq!(event["event"]["issue"], k);
    let last_seen = event["event"]["seq"].as_i64().unwrap();

    // The client drops. Writes happen while it is gone.
    drop(ws);
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "title": "Live, renamed" }),
    )
    .await;
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "priority": "urgent" }),
    )
    .await;

    // It reconnects and asks for everything after its last sequence number.
    let page = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/activity?after_seq={last_seen}"),
            Value::Null,
        )
        .await;
    let fields: Vec<&str> = page["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["title", "priority"]);
    assert_eq!(page["reload"], false);
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let hello: Value = match ws.next().await.unwrap().unwrap() {
        tokio_tungstenite::tungstenite::Message::Text(t) => serde_json::from_str(&t).unwrap(),
        m => panic!("unexpected {m:?}"),
    };
    assert_eq!(hello["last_seq"], page["last_seq"]);
    t.stop().await;
}

#[tokio::test]
async fn sync_handoff_between_two_machines() {
    let shared = tempfile::tempdir().unwrap();
    let folder = shared.path().display().to_string();
    let a = T::bare(Some("mac-a")).await;
    let b = T::bare(Some("mac-b")).await;
    a.ok(
        &owner(),
        "POST",
        "/v1/teams",
        json!({ "key": "ALX", "name": "Alex" }),
    )
    .await;
    a.issue("Written on A", json!({})).await;

    let r = a
        .ok(
            &owner(),
            "POST",
            "/v1/sync/push",
            json!({ "folder": folder }),
        )
        .await;
    assert_eq!(r["manifest"]["generation"], 1);
    assert_eq!(r["manifest"]["owner_host"], "mac-a");

    // B cannot pull while A holds the lock, unless it takes it.
    assert_eq!(
        b.err(
            &owner(),
            "POST",
            "/v1/sync/pull",
            json!({ "folder": folder })
        )
        .await,
        "claim_held"
    );
    let r = b
        .ok(
            &owner(),
            "POST",
            "/v1/sync/pull",
            json!({ "folder": folder, "take": true }),
        )
        .await;
    assert!(r["backup"].is_string());
    let v = b.ok(&owner(), "GET", "/v1/issues/ALX-1", Value::Null).await;
    assert_eq!(v["title"], "Written on A");

    // A now finds B holding the lock.
    a.issue("Late write on A", json!({})).await;
    assert_eq!(
        a.err(
            &owner(),
            "POST",
            "/v1/sync/push",
            json!({ "folder": folder })
        )
        .await,
        "claim_held"
    );

    // B writes, pushes with --release, and becomes read-only.
    b.issue("Written on B", json!({})).await;
    let r = b
        .ok(
            &owner(),
            "POST",
            "/v1/sync/push",
            json!({ "folder": folder, "release": true }),
        )
        .await;
    assert_eq!(r["manifest"]["generation"], 2);
    assert!(r["manifest"]["owner_host"].is_null());
    assert_eq!(
        b.err(&owner(), "POST", "/v1/issues", json!({ "title": "nope" }))
            .await,
        "not_primary"
    );

    // A has unpushed writes, so a pull refuses unless forced.
    assert_eq!(
        a.err(
            &owner(),
            "POST",
            "/v1/sync/pull",
            json!({ "folder": folder })
        )
        .await,
        "conflict"
    );
    a.ok(
        &owner(),
        "POST",
        "/v1/sync/pull",
        json!({ "folder": folder, "force": true }),
    )
    .await;
    let v = a.ok(&owner(), "GET", "/v1/issues/ALX-2", Value::Null).await;
    assert_eq!(v["title"], "Written on B");
    let st = a.ok(&owner(), "GET", "/v1/sync/status", Value::Null).await;
    assert_eq!(st["local_generation"], 2);
    assert_eq!(st["unpushed_writes"], false);
    assert_eq!(st["is_primary"], true);

    // A copy from a newer schema is refused.
    let mpath = shared.path().join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&mpath).unwrap()).unwrap();
    m["schema_version"] = json!(999_999);
    std::fs::write(&mpath, serde_json::to_vec(&m).unwrap()).unwrap();
    assert_eq!(
        b.err(
            &owner(),
            "POST",
            "/v1/sync/pull",
            json!({ "folder": folder, "take": true })
        )
        .await,
        "validation"
    );
    a.stop().await;
    b.stop().await;
}

/// LLM-24: the manifest for generation 2 arrives before its database file.
/// The old file has the same unpacked size, so a size check alone passes.
#[tokio::test]
async fn sync_pull_refuses_a_database_older_than_its_manifest() {
    let shared = tempfile::tempdir().unwrap();
    let folder = shared.path().display().to_string();
    let a = T::bare(Some("mac-a")).await;
    let b = T::bare(Some("mac-b")).await;
    a.ok(
        &owner(),
        "POST",
        "/v1/teams",
        json!({ "key": "ALX", "name": "Alex" }),
    )
    .await;
    a.issue("Generation 1", json!({})).await;
    a.ok(
        &owner(),
        "POST",
        "/v1/sync/push",
        json!({ "folder": folder, "release": true }),
    )
    .await;
    let old_db = std::fs::read(shared.path().join("llmtodo.db.gz")).unwrap();
    let old_manifest: Value =
        serde_json::from_slice(&std::fs::read(shared.path().join("manifest.json")).unwrap())
            .unwrap();

    // A takes the lock back, writes and pushes generation 2.
    a.ok(
        &owner(),
        "POST",
        "/v1/sync/pull",
        json!({ "folder": folder }),
    )
    .await;
    a.issue("Generation 2", json!({})).await;
    a.ok(
        &owner(),
        "POST",
        "/v1/sync/push",
        json!({ "folder": folder, "release": true }),
    )
    .await;

    // On B, the new manifest is in place but the old database file is not yet replaced.
    std::fs::write(shared.path().join("llmtodo.db.gz"), &old_db).unwrap();
    let mpath = shared.path().join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&mpath).unwrap()).unwrap();
    assert_eq!(m["generation"], 2);
    m["db_bytes"] = old_manifest["db_bytes"].clone();
    std::fs::write(&mpath, serde_json::to_vec(&m).unwrap()).unwrap();

    b.err(
        &owner(),
        "POST",
        "/v1/sync/pull",
        json!({ "folder": folder }),
    )
    .await;
    let st = b.ok(&owner(), "GET", "/v1/sync/status", Value::Null).await;
    assert_eq!(st["local_generation"], 0);
    a.stop().await;
    b.stop().await;
}

/// LLM-32: the incoming database cannot be opened. The pull fails, and
/// the server keeps serving reads and writes from the old database.
#[tokio::test]
async fn failed_sync_pull_leaves_the_server_working() {
    use sha2::Digest;
    use std::io::Write as _;
    let shared = tempfile::tempdir().unwrap();
    let folder = shared.path().display().to_string();
    let t = T::new().await;
    t.issue("Before the pull", json!({})).await;
    t.ok(
        &owner(),
        "POST",
        "/v1/sync/push",
        json!({ "folder": folder }),
    )
    .await;

    // A complete-looking copy whose database is not a SQLite file.
    let raw = vec![7u8; 8192];
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&raw).unwrap();
    std::fs::write(shared.path().join("llmtodo.db.gz"), gz.finish().unwrap()).unwrap();
    let mpath = shared.path().join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&mpath).unwrap()).unwrap();
    m["generation"] = json!(2);
    m["db_bytes"] = json!(raw.len());
    m["db_sha256"] = json!(hex::encode(sha2::Sha256::digest(&raw)));
    std::fs::write(&mpath, serde_json::to_vec(&m).unwrap()).unwrap();

    t.err(
        &owner(),
        "POST",
        "/v1/sync/pull",
        json!({ "folder": folder }),
    )
    .await;
    let v = t.ok(&owner(), "GET", "/v1/issues/ALX-1", Value::Null).await;
    assert_eq!(v["title"], "Before the pull");
    t.issue("After the failed pull", json!({})).await;
    let st = t.ok(&owner(), "GET", "/v1/sync/status", Value::Null).await;
    assert_eq!(st["is_primary"], true);
    assert_eq!(st["local_generation"], 1);
    t.stop().await;
}

#[tokio::test]
async fn usage_goes_to_the_ticket_being_written() {
    let t = T::new().await;
    let a = t.issue("A", json!({})).await;
    let b = t.issue("B", json!({})).await;
    let with = |requests: u64, output: u64| {
        let mut w = agent("u1");
        w.0.transcript_path = Some("/tmp/transcript-u1.jsonl".into());
        let u = TokenUsage {
            requests,
            input: 10 * requests,
            output,
            cache_write_5m: 0,
            cache_write_1h: 0,
            cache_read: 1_000_000 * requests,
        };
        w.0.usage = Some([("claude-opus-5-5".to_string(), u)].into());
        w
    };
    // First write: everything so far goes to A.
    t.ok(
        &with(2, 1_000),
        "PATCH",
        &format!("/v1/issues/{a}"),
        json!({ "priority": "low" }),
    )
    .await;
    // Second write: only the new part goes to B.
    t.ok(
        &with(5, 4_000),
        "PATCH",
        &format!("/v1/issues/{b}"),
        json!({ "priority": "low" }),
    )
    .await;
    // A write that touches no ticket keeps its usage for the next ticket write.
    t.ok(
        &with(6, 5_000),
        "POST",
        "/v1/labels",
        json!({ "name": "x" }),
    )
    .await;
    let v = t
        .ok(
            &with(6, 5_000),
            "PATCH",
            &format!("/v1/issues/{a}"),
            json!({ "priority": "high" }),
        )
        .await;
    let ua = &v["usage"]["by_model"]["claude-opus-5-5"];
    assert_eq!(
        (ua["requests"].as_u64(), ua["output"].as_u64()),
        (Some(3), Some(2_000))
    );
    // 20 input @ $4, 2000 output @ $20, 3M cache reads @ $0.20 per million.
    let cost = v["usage"]["cost_usd"].as_f64().unwrap();
    assert!(
        (cost - (20.0 * 4.0 + 2_000.0 * 20.0 + 3_000_000.0 * 0.20) / 1e6).abs() < 1e-3,
        "{cost}"
    );
    assert_eq!(v["sessions"][0]["usage"]["requests"], 3);

    // The list carries the same cost, and nothing for an issue with no usage.
    let idle = t
        .ok(&owner(), "POST", "/v1/issues", json!({ "title": "Idle" }))
        .await;
    let list = t.ok(&owner(), "POST", "/v1/issues/query", json!({})).await;
    let listed = |key: &str| {
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["key"] == key)
            .unwrap()
            .clone()
    };
    assert_eq!(listed(&a)["cost_usd"], v["usage"]["cost_usd"]);
    assert!(
        listed(idle["key"].as_str().unwrap())
            .get("cost_usd")
            .is_none()
    );

    let r = t.ok(&owner(), "GET", "/v1/usage", Value::Null).await;
    assert_eq!(r["total"]["requests"], 6);
    assert_eq!(r["issues"].as_array().unwrap().len(), 2);

    // A price override from settings wins.
    t.ok(&owner(), "PATCH", "/v1/settings", json!({ "model_prices": { "claude-opus-5-5": { "input": 0, "output": 1000000, "cache_write_5m": 0, "cache_write_1h": 0, "cache_read": 0 } } })).await;
    let r = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/usage?issue={a}"),
            Value::Null,
        )
        .await;
    assert_eq!(r["total"]["cost_usd"], 2000.0);
    // Owner writes never carry usage.
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{a}"),
        json!({ "priority": "low" }),
    )
    .await;
    t.stop().await;
}

#[tokio::test]
async fn waiting_on_agent_or_owner() {
    let t = T::new().await;
    let map = t.issue("Map", json!({})).await;
    let ready = t
        .issue("Ready", json!({ "parent": map, "waiting_on": "agent" }))
        .await;
    let ask = t.issue("Needs an answer", json!({ "parent": map })).await;
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{ask}"),
            json!({ "waiting_on": "owner" }),
        )
        .await;
    assert_eq!(v["waiting_on"], "owner");
    assert_eq!(
        t.err(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{ask}"),
            json!({ "waiting_on": "robot" })
        )
        .await,
        "validation"
    );

    // The frontier skips issues that wait on the owner.
    let f = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/frontier",
            json!({ "issue": map }),
        )
        .await;
    let keys: Vec<&str> = f
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, [ready.as_str()]);
    let q = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "waiting_on": "owner" }),
        )
        .await;
    assert_eq!(q["items"][0]["key"], ask);
    let q = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "waiting_on": "none" }),
        )
        .await;
    assert_eq!(q["items"][0]["key"], map);

    // Clearing, and finishing, both empty it.
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{ask}"),
            json!({ "waiting_on": "" }),
        )
        .await;
    assert!(v["waiting_on"].is_null());
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{ready}"),
            json!({ "status": "Done" }),
        )
        .await;
    assert!(v["waiting_on"].is_null());
    t.stop().await;
}

/// Owner questions: asking adds to the `+++ Owner questions` section, and
/// the last answer hands the issue back to the agents.
#[tokio::test]
async fn answering_every_question_hands_back() {
    let t = T::new().await;
    let k = t.issue("Decide", json!({ "description": "Body." })).await;
    let ask = |q: &str, opts: serde_json::Value| {
        json!({
            "waiting_on": "owner",
            "patches": [{ "op": "ask_question", "question": q, "options": opts }]
        })
    };
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        ask(
            "Fork or PR?",
            json!([{ "letter": "a", "text": "Fork" }, { "letter": "b", "text": "PR" }]),
        ),
    )
    .await;
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            ask("Name?", json!([])),
        )
        .await;
    assert_eq!(v["waiting_on"], "owner");
    let d = v["description"].as_str().unwrap();
    assert!(
        d.contains("+++ Owner questions\n### Q1. Fork or PR?"),
        "{d}"
    );
    assert!(d.contains("### Q2. Name?"), "{d}");

    let answer = |id: &str, a: &str| json!({ "patches": [{ "op": "answer_question", "id": id, "answer": a }] });
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            answer("Q1", "b"),
        )
        .await;
    assert_eq!(v["waiting_on"], "owner", "one question is still open");
    assert!(
        v["description"]
            .as_str()
            .unwrap()
            .contains("**Answer:** b) PR")
    );
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            answer("Q2", "lt"),
        )
        .await;
    assert_eq!(v["waiting_on"], "agent");

    // A bad question ID is a validation error, not a patch mismatch.
    assert_eq!(
        t.err(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            answer("Q9", "x")
        )
        .await,
        "validation"
    );
    // Editing an issue whose questions were all answered already does not
    // change waiting_on again.
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "waiting_on": "owner" }),
    )
    .await;
    let v = t
        .ok(
            &owner(),
            "PATCH",
            &format!("/v1/issues/{k}"),
            answer("Q2", "llmtodo"),
        )
        .await;
    assert_eq!(v["waiting_on"], "owner");
    t.stop().await;
}

/// An issue with one open choice question, waiting on the owner.
async fn asked(t: &T) -> String {
    let k = t.issue("Decide", json!({ "description": "Body." })).await;
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({
            "waiting_on": "owner",
            "patches": [{
                "op": "ask_question",
                "question": "Fork or PR?",
                "options": [{ "letter": "a", "text": "Fork" }, { "letter": "b", "text": "PR" }],
            }]
        }),
    )
    .await;
    k
}

async fn answer_q1(t: &T, k: &str) -> Value {
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "patches": [{ "op": "answer_question", "id": "Q1", "answer": "b" }] }),
    )
    .await
}

/// Undo reopens the question and waits on the owner again, and the
/// description is the same text as before the answer.
#[tokio::test]
async fn undo_answer_reopens_the_question() {
    let t = T::new().await;
    let k = asked(&t).await;
    let before = t
        .ok(&owner(), "GET", &format!("/v1/issues/{k}"), json!(null))
        .await;
    let v = answer_q1(&t, &k).await;
    assert_eq!(v["waiting_on"], "agent");

    let path = format!("/v1/issues/{k}/questions/Q1/undo-answer");
    let v = t.ok(&owner(), "POST", &path, json!({})).await;
    assert_eq!(v["waiting_on"], "owner");
    assert_eq!(v["description"], before["description"]);

    // Nothing is left to undo, and the second try says so.
    let (code, body) = t.call(&owner(), "POST", &path, json!({})).await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["code"], "conflict");
    assert_eq!(body["error"]["details"]["reason"], "already_open");
    t.stop().await;
}

/// Any write by another actor after the answer means an agent may have
/// acted on it, so undo refuses and changes nothing.
#[tokio::test]
async fn undo_answer_refuses_after_an_agent_acted() {
    let t = T::new().await;
    let k = asked(&t).await;
    answer_q1(&t, &k).await;
    t.ok(
        &agent("a1"),
        "POST",
        &format!("/v1/issues/{k}/comments"),
        json!({ "body": "Going with the PR." }),
    )
    .await;

    let path = format!("/v1/issues/{k}/questions/Q1/undo-answer");
    let (code, body) = t.call(&owner(), "POST", &path, json!({})).await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["details"]["reason"], "agent_acted");
    let v = t
        .ok(&owner(), "GET", &format!("/v1/issues/{k}"), json!(null))
        .await;
    assert!(
        v["description"]
            .as_str()
            .unwrap()
            .contains("**Answer:** b) PR")
    );
    assert_eq!(v["waiting_on"], "agent");
    t.stop().await;
}

/// A claim from before the answer is no reaction to it, so undo still works.
/// The owner's own later writes do not block undo either.
#[tokio::test]
async fn undo_answer_ignores_earlier_agent_writes_and_the_owners_own() {
    let t = T::new().await;
    let k = asked(&t).await;
    t.ok(
        &agent("a1"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    answer_q1(&t, &k).await;
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "priority": "high" }),
    )
    .await;
    let path = format!("/v1/issues/{k}/questions/Q1/undo-answer");
    let v = t.ok(&owner(), "POST", &path, json!({})).await;
    assert_eq!(v["waiting_on"], "owner");
    t.stop().await;
}

/// An undo must clear the answer it means. A rewritten answer or question
/// is not that answer any more.
#[tokio::test]
async fn undo_answer_refuses_when_the_question_changed() {
    let t = T::new().await;
    let k = asked(&t).await;
    answer_q1(&t, &k).await;
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "patches": [{ "op": "replace_exact", "old": "Fork or PR?", "new": "Fork, PR or wait?" }] }),
    )
    .await;
    let path = format!("/v1/issues/{k}/questions/Q1/undo-answer");
    let (code, body) = t.call(&owner(), "POST", &path, json!({})).await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["details"]["reason"], "question_changed");

    let k = asked(&t).await;
    answer_q1(&t, &k).await;
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "patches": [{ "op": "answer_question", "id": "Q1", "answer": "a" }] }),
    )
    .await;
    let path = format!("/v1/issues/{k}/questions/Q1/undo-answer");
    let (code, body) = t.call(&owner(), "POST", &path, json!({})).await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["details"]["reason"], "question_changed");
    t.stop().await;
}

/// Only the actor who answered can take the answer back, a finished issue
/// keeps its answer, and an unknown question is a validation error.
#[tokio::test]
async fn undo_answer_guards() {
    let t = T::new().await;
    let k = asked(&t).await;
    answer_q1(&t, &k).await;
    let path = format!("/v1/issues/{k}/questions/Q1/undo-answer");
    let (code, body) = t.call(&agent("a1"), "POST", &path, json!({})).await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["details"]["reason"], "not_the_answerer");

    assert_eq!(
        t.err(
            &owner(),
            "POST",
            &format!("/v1/issues/{k}/questions/Q9/undo-answer"),
            json!({})
        )
        .await,
        "validation"
    );

    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "status": "Done" }),
    )
    .await;
    let (code, body) = t.call(&owner(), "POST", &path, json!({})).await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["details"]["reason"], "issue_closed");
    t.stop().await;
}

/// A `head` that is not a commit hash could become a git option when
/// `issue view` reads the commit range, so the server rejects it.
#[tokio::test]
async fn head_must_be_a_commit_hash() {
    let t = T::new().await;
    let k = t.issue("Work", json!({})).await;
    let long = "a".repeat(65);
    for bad in [
        "--output=/tmp/x",
        "abc",
        "main",
        "abc1234 --all",
        long.as_str(),
    ] {
        let mut who = agent("s1");
        who.0.head = Some(bad.to_string());
        let code = t
            .err(&who, "POST", &format!("/v1/issues/{k}/claim"), json!({}))
            .await;
        assert_eq!(code, "bad_request", "{bad}");
    }
    let mut who = agent("s1");
    who.0.head = Some("0123456789abcdef0123456789abcdef01234567".into());
    t.ok(&who, "POST", &format!("/v1/issues/{k}/claim"), json!({}))
        .await;
    t.stop().await;
}

#[tokio::test]
async fn claim_respects_an_assignee_without_a_claim() {
    let t = T::new().await;
    let k = t.issue("Owner work", json!({})).await;
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "assignee": "owner" }),
    )
    .await;
    let (s, v) = t
        .call(
            &agent("s1"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({}),
        )
        .await;
    assert_eq!(s, 409, "claim should fail: {v}");
    assert_eq!(v["error"]["code"], "claim_held");
    let v = t
        .ok(&owner(), "GET", &format!("/v1/issues/{k}"), Value::Null)
        .await;
    assert_eq!(v["assignee"]["kind"], "human");

    // The assignee may claim it, and --steal takes it over.
    let r = t
        .ok(
            &owner(),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({}),
        )
        .await;
    assert_eq!(r["changed"], true);
    let r = t
        .ok(
            &agent("s1"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({ "steal": true }),
        )
        .await;
    assert_eq!(r["issue"]["assignee"]["name"], "claude-code/opus-5.5/high");
    t.stop().await;
}

/// Walks every page of a query. `between` runs after the first page.
async fn walk_pages(t: &T, mut q: Value, between: impl AsyncFnOnce()) -> Vec<String> {
    let mut keys = Vec::new();
    let mut between = Some(between);
    loop {
        let page = t.ok(&owner(), "POST", "/v1/issues/query", q.clone()).await;
        keys.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["key"].as_str().unwrap().to_string()),
        );
        if let Some(f) = between.take() {
            f().await;
        }
        match page["next_cursor"].as_str() {
            Some(c) => q["cursor"] = json!(c),
            None => break,
        }
    }
    keys
}

/// A write between two pages moves an issue to the front. The rows after the
/// cursor must not shift, so no other issue is skipped or repeated.
#[tokio::test]
async fn query_pages_survive_a_write_between_pages() {
    let t = T::new().await;
    for n in 1..=5 {
        t.issue(&format!("Issue {n}"), json!({})).await;
    }
    // Newest update first: ALX-5, ALX-4 | ALX-3, ALX-2 | ALX-1.
    let keys = walk_pages(&t, json!({ "limit": 2 }), async || {
        t.ok(
            &owner(),
            "PATCH",
            "/v1/issues/ALX-3",
            json!({ "title": "Issue 3, edited" }),
        )
        .await;
    })
    .await;
    assert_eq!(keys, ["ALX-5", "ALX-4", "ALX-2", "ALX-1"]);

    // In ID order a write moves nothing, so the walk sees every issue once.
    let keys = walk_pages(&t, json!({ "limit": 2, "order": "id" }), async || {
        t.ok(
            &owner(),
            "PATCH",
            "/v1/issues/ALX-1",
            json!({ "priority": "low" }),
        )
        .await;
        t.ok(
            &owner(),
            "PATCH",
            "/v1/issues/ALX-4",
            json!({ "priority": "urgent" }),
        )
        .await;
    })
    .await;
    assert_eq!(keys, ["ALX-1", "ALX-2", "ALX-3", "ALX-4", "ALX-5"]);

    assert_eq!(
        t.err(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "cursor": "2" })
        )
        .await,
        "bad_request"
    );
    t.stop().await;
}

/// The executor filter runs before paging, so every page but the last is
/// full and the walk finds every match.
#[tokio::test]
async fn executor_filter_fills_each_page() {
    let t = T::new().await;
    for n in 1..=5 {
        t.issue(&format!("Issue {n}"), json!({})).await;
    }
    t.ok(
        &owner(),
        "PATCH",
        "/v1/issues/ALX-1",
        json!({ "wanted_executor": { "kind": "agent" } }),
    )
    .await;
    t.ok(
        &owner(),
        "PATCH",
        "/v1/issues/ALX-5",
        json!({ "wanted_executor": { "kind": "HUMAN" } }),
    )
    .await;
    let mut q = json!({ "limit": 2, "for_executor": { "kind": "human" } });
    let mut keys = Vec::new();
    loop {
        let page = t.ok(&owner(), "POST", "/v1/issues/query", q.clone()).await;
        let items = page["items"].as_array().unwrap();
        keys.extend(items.iter().map(|i| i["key"].as_str().unwrap().to_string()));
        match page["next_cursor"].as_str() {
            Some(c) => {
                assert_eq!(
                    items.len(),
                    2,
                    "a page with a next cursor must be full: {page}"
                );
                q["cursor"] = json!(c);
            }
            None => break,
        }
    }
    keys.sort();
    assert_eq!(keys, ["ALX-2", "ALX-3", "ALX-4", "ALX-5"]);
    t.stop().await;
}

/// One session may change its model or effort. It still holds its claim.
#[tokio::test]
async fn a_session_keeps_its_claim_after_its_effort_changes() {
    let t = T::new().await;
    let k = t.issue("Work", json!({})).await;
    t.ok(
        &agent("s1"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    let mut medium = agent("s1");
    medium.0.actor.effort = Some("medium".into());
    let r = t
        .ok(&medium, "POST", &format!("/v1/issues/{k}/claim"), json!({}))
        .await;
    assert_eq!(r["issue"]["claim"]["session_id"], "s1");
    assert_eq!(
        r["issue"]["claim"]["actor"]["name"],
        "claude-code/opus-5.5/medium"
    );
    assert_eq!(
        r["issue"]["assignee"]["name"],
        "claude-code/opus-5.5/medium"
    );
    // Another session still cannot take it.
    assert_eq!(
        t.err(
            &agent("s2"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({})
        )
        .await,
        "claim_held"
    );
    let r = t
        .ok(
            &medium,
            "POST",
            &format!("/v1/issues/{k}/release"),
            json!({}),
        )
        .await;
    assert!(r["claim"].is_null());
    t.stop().await;
}

/// Many writes commit at once. The live channel must send their events in
/// seq order with no gap, so a client that keeps the newest seq it saw can
/// catch up from it after a drop.
#[tokio::test]
async fn live_events_go_out_in_seq_order() {
    let t = std::sync::Arc::new(T::new().await);
    for n in 0..8 {
        t.issue(&format!("Issue {n}"), json!({})).await;
    }
    let mut rx = t.running.as_ref().unwrap().state.events.subscribe();
    let mut tasks = Vec::new();
    for round in 0..5 {
        for n in 1..=8 {
            let t = t.clone();
            tasks.push(tokio::spawn(async move {
                t.ok(
                    &owner(),
                    "PATCH",
                    &format!("/v1/issues/ALX-{n}"),
                    json!({ "title": format!("Issue {n}, round {round}") }),
                )
                .await;
            }));
        }
    }
    for task in tasks {
        task.await.unwrap();
    }
    let mut seqs = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let WsMessage::Event { event } = msg {
            seqs.push(event.seq);
        }
    }
    assert!(!seqs.is_empty());
    for pair in seqs.windows(2) {
        assert_eq!(
            pair[1],
            pair[0] + 1,
            "events out of order or with a gap: {seqs:?}"
        );
    }
    let t = std::sync::Arc::into_inner(t).unwrap();
    t.stop().await;
}

/// The git panel keeps the claim's branch and worktree after the claim ends,
/// and agents can change the branch, worktree and PR by hand.
#[tokio::test]
async fn git_panel_outlives_the_claim() {
    let t = T::new().await;
    let k = t.issue("Git work", json!({})).await;
    let url = format!("/v1/issues/{k}");
    let v = t.ok(&owner(), "GET", &url, Value::Null).await;
    assert_eq!(v["git"]["branch"], Value::Null);

    let mut who = agent("g1");
    who.0.worktree = Some("/tmp/wt-g1".into());
    t.ok(&who, "POST", &format!("{url}/claim"), json!({})).await;
    t.ok(&who, "POST", &format!("{url}/release"), json!({}))
        .await;
    let v = t.ok(&owner(), "GET", &url, Value::Null).await;
    assert_eq!(v["claim"], Value::Null);
    assert_eq!(v["git"]["branch"], "g1-branch");
    assert_eq!(v["git"]["worktree"], "/tmp/wt-g1");
    assert_eq!(v["git"]["host"], "test-host");
    assert_eq!(v["git"]["head"], "abc1234");
    assert_eq!(v["git"]["pr"], Value::Null);

    // A PR attachment shows as the PR until one is set by hand.
    t.ok(
        &owner(),
        "POST",
        &format!("{url}/attachments"),
        json!({ "url": "https://github.com/o/r/pull/1" }),
    )
    .await;
    let v = t.ok(&owner(), "GET", &url, Value::Null).await;
    assert_eq!(v["git"]["pr"]["url"], "https://github.com/o/r/pull/1");

    let pr = "https://github.com/o/r/pull/2";
    let v = t
        .ok(
            &who,
            "PATCH",
            &url,
            json!({ "branch": "fix/other", "worktree": "/tmp/wt-2", "pr": pr }),
        )
        .await;
    assert_eq!(v["git"]["branch"], "fix/other");
    assert_eq!(v["git"]["worktree"], "/tmp/wt-2");
    assert_eq!(v["git"]["pr"]["url"], pr);
    // The PR is attached too, so the GitHub sync follows it.
    let attached: Vec<&str> = v["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["url"].as_str())
        .collect();
    assert!(attached.contains(&pr), "{attached:?}");

    // Empty strings clear the fields; the worktree takes its host with it.
    let v = t
        .ok(
            &who,
            "PATCH",
            &url,
            json!({ "branch": "", "worktree": "", "pr": "" }),
        )
        .await;
    assert_eq!(v["git"]["branch"], Value::Null);
    assert_eq!(v["git"]["worktree"], Value::Null);
    assert_eq!(v["git"]["host"], Value::Null);
    // The newest PR attachment shows again.
    assert_eq!(v["git"]["pr"]["url"], pr);

    assert_eq!(
        t.err(&who, "PATCH", &url, json!({ "pr": "not a url" }))
            .await,
        "validation"
    );
    t.stop().await;
}

/// LLM-108: `claimed` and `analyzing` narrow a query to the tasks an agent
/// is on, the same set the app calls Locked and Analyzing.
#[tokio::test]
async fn query_filters_by_llm_status() {
    let t = T::new().await;
    let locked = t.issue("Locked", json!({})).await;
    let reading = t.issue("Reading", json!({})).await;
    let expired = t.issue("Expired", json!({})).await;
    let by_owner = t.issue("Held by the owner", json!({})).await;
    let released = t.issue("Released", json!({})).await;
    t.issue("Nobody on it", json!({})).await;
    let path = |k: &str, p: &str| format!("/v1/issues/{k}{p}");
    let a = agent("worker");
    t.ok(&a, "POST", &path(&locked, "/claim"), json!({})).await;
    t.ok(&a, "POST", &path(&released, "/claim"), json!({}))
        .await;
    t.ok(&a, "POST", &path(&released, "/release"), json!({}))
        .await;
    t.ok(&owner(), "POST", &path(&by_owner, "/claim"), json!({}))
        .await;
    let reader = agent("reader");
    t.ok(&reader, "POST", &path(&reading, "/analyze"), json!({}))
        .await;
    let old = agent("old-reader");
    t.ok(&old, "POST", &path(&expired, "/analyze"), json!({}))
        .await;
    let expired_id = t.ok(&owner(), "GET", &path(&expired, ""), json!({})).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        llmtodo_server::db::db_path(t._dir.path()).display()
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE analyzing SET seen_at = '2020-01-01T00:00:00.000000Z' WHERE issue_id = ?")
        .bind(expired_id)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let keys = |q: Value| {
        let t = &t;
        async move {
            let page = t.ok(&owner(), "POST", "/v1/issues/query", q).await;
            let mut keys: Vec<String> = page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["key"].as_str().unwrap().to_string())
                .collect();
            keys.sort();
            keys
        }
    };
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    assert_eq!(keys(json!({ "claimed": true })).await, vec![locked.clone()]);
    assert_eq!(
        keys(json!({ "analyzing": true })).await,
        vec![reading.clone()]
    );
    // Both flags keep a task with either one.
    assert_eq!(
        keys(json!({ "claimed": true, "analyzing": true })).await,
        sorted(vec![locked, reading])
    );
    assert_eq!(keys(json!({})).await.len(), 6);
}

#[tokio::test]
async fn analyzing_marks_never_block_and_expire() {
    let t = T::new().await;
    let key = t.issue("Fill me in", json!({})).await;
    let path = |p: &str| format!("/v1/issues/{key}{p}");
    let (a, b) = (agent("reader"), agent("worker"));

    let v = t.ok(&a, "POST", &path("/analyze"), json!({})).await;
    assert_eq!(v["changed"], true);
    assert_eq!(v["issue"]["analyzing"]["session_id"], "reader");
    assert!(v["issue"]["claim"].is_null());
    // A repeat keeps the mark and changes nothing.
    let again = t.ok(&a, "POST", &path("/analyze"), json!({})).await;
    assert_eq!(again["changed"], false);
    assert_eq!(
        again["issue"]["analyzing"]["started_at"],
        v["issue"]["analyzing"]["started_at"]
    );

    // Another session writes freely while the mark is on.
    t.ok(&b, "PATCH", &path(""), json!({ "title": "Filled in" }))
        .await;
    let view = t.ok(&owner(), "GET", &path(""), json!({})).await;
    assert_eq!(view["analyzing"]["session_id"], "reader");

    // `done` from another session leaves the mark.
    let v = t
        .ok(&b, "POST", &path("/analyze"), json!({ "done": true }))
        .await;
    assert_eq!(v["changed"], false);
    assert_eq!(v["issue"]["analyzing"]["session_id"], "reader");
    let v = t
        .ok(&a, "POST", &path("/analyze"), json!({ "done": true }))
        .await;
    assert_eq!(v["changed"], true);
    assert!(v["issue"]["analyzing"].is_null());

    // A claim ends the mark: Locked replaces Analyzing.
    t.ok(&a, "POST", &path("/analyze"), json!({})).await;
    let v = t.ok(&b, "POST", &path("/claim"), json!({})).await;
    assert!(v["issue"]["analyzing"].is_null());
    assert_eq!(v["issue"]["claim"]["session_id"], "worker");
    // Analyzing a claimed issue works, with a warning.
    let v = t.ok(&a, "POST", &path("/analyze"), json!({})).await;
    assert_eq!(v["issue"]["analyzing"]["session_id"], "reader");
    assert!(v["warnings"][0].as_str().unwrap().contains("claimed"));
    let h = t.ok(&owner(), "GET", &path("/history"), json!({})).await;
    assert!(h.to_string().contains("analyzing"), "{h}");

    // A mark without a write for 15 minutes expires. A later write by the
    // same session does not bring it back.
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        llmtodo_server::db::db_path(t._dir.path()).display()
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE analyzing SET seen_at = '2020-01-01T00:00:00.000000Z'")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let view = t.ok(&owner(), "GET", &path(""), json!({})).await;
    assert!(view["analyzing"].is_null(), "{view}");
    t.ok(&a, "PATCH", &path(""), json!({ "title": "Again" }))
        .await;
    let view = t.ok(&owner(), "GET", &path(""), json!({})).await;
    assert!(view["analyzing"].is_null(), "{view}");

    // Finishing the issue ends a live mark.
    t.ok(&a, "POST", &path("/analyze"), json!({})).await;
    let v = t
        .ok(&b, "PATCH", &path(""), json!({ "status": "Done" }))
        .await;
    assert!(v["analyzing"].is_null(), "{v}");
    t.stop().await;
}

// ---------------------------------------------------------------- questions

const ONE_OPEN: &str =
    "Intro.\n\n+++ Owner questions\n### Q1. Name?\nShort, please.\n\n**Answer:**\n+++\n";
const ONE_ANSWERED: &str =
    "Intro.\n\n+++ Owner questions\n### Q1. Name?\nShort, please.\n\n**Answer:** lt\n+++\n";

async fn answer_as(t: &T, who: &Who, k: &str, q: &str, text: &str) -> Value {
    t.ok(
        who,
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "patches": [{ "op": "answer_question", "id": q, "answer": text }] }),
    )
    .await
}

fn ids_of(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| {
            format!(
                "{} {}",
                q["key"].as_str().unwrap(),
                q["id"].as_str().unwrap()
            )
        })
        .collect()
}

/// Every write that changes a description keeps the count and the list in
/// step with the Markdown.
#[tokio::test]
async fn questions_table_follows_every_description_write() {
    let t = T::new().await;
    let o = owner();
    let k = t.issue("Decide", json!({ "description": ONE_OPEN })).await;
    let count = |v: &Value| v["open_questions"].as_u64().unwrap();
    let path = format!("/v1/issues/{k}");

    let v = t.ok(&o, "GET", &path, json!(null)).await;
    assert_eq!(count(&v), 1, "a new issue counts its questions");
    assert_eq!(
        ids_of(&t.ok(&o, "GET", "/v1/questions", json!(null)).await),
        [format!("{k} Q1")]
    );

    let q = t
        .ok(&o, "POST", "/v1/issues/query", json!({ "open": true }))
        .await;
    assert_eq!(count(&q["items"][0]), 1, "lists carry the count: {q}");

    let v = answer_as(&t, &o, &k, "Q1", "lt").await;
    assert_eq!(count(&v), 0);
    assert!(
        ids_of(&t.ok(&o, "GET", "/v1/questions", json!(null)).await).is_empty(),
        "an answered question leaves the open list"
    );
    let all = t.ok(&o, "GET", "/v1/questions?all=true", json!(null)).await;
    assert_eq!(all["items"][0]["answer"], "lt");
    assert_eq!(all["items"][0]["text"], "Name?");
    assert_eq!(all["items"][0]["context"], "Short, please.");

    // Restoring the first revision opens the question again.
    let v = t
        .ok(
            &o,
            "POST",
            &format!("{path}/restore"),
            json!({ "revision": 1 }),
        )
        .await;
    assert_eq!(count(&v), 1);

    // A full replace can drop the section.
    let v = t
        .ok(
            &o,
            "PATCH",
            &path,
            json!({ "body": "No section.", "force": true }),
        )
        .await;
    assert_eq!(count(&v), 0);
    let all = t.ok(&o, "GET", "/v1/questions?all=true", json!(null)).await;
    assert!(ids_of(&all).is_empty(), "{all}");
    t.stop().await;
}

#[tokio::test]
async fn question_list_filters_and_pages() {
    let t = T::new().await;
    let o = owner();
    t.ok(&o, "POST", "/v1/projects", json!({ "name": "Proj" }))
        .await;
    let two = ONE_OPEN.replace(
        "**Answer:**\n+++",
        "**Answer:**\n\n### Q2. Colour?\n\n- a) Red\n- b) Blue\n\n**Answer:**\n+++",
    );
    let a = t
        .issue("A", json!({ "description": two, "project": "proj" }))
        .await;
    let b = t.issue("B", json!({ "description": ONE_OPEN })).await;
    let closed = t
        .issue("C", json!({ "description": ONE_OPEN, "status": "Done" }))
        .await;
    let trashed = t.issue("D", json!({ "description": ONE_OPEN })).await;
    t.ok(&o, "DELETE", &format!("/v1/issues/{trashed}"), json!(null))
        .await;

    let page = t.ok(&o, "GET", "/v1/questions", json!(null)).await;
    assert_eq!(
        ids_of(&page),
        [format!("{a} Q1"), format!("{a} Q2"), format!("{b} Q1")],
        "closed and trashed issues stay out of the list"
    );
    assert_eq!(page["items"][1]["options"][1]["text"], "Blue");

    let page = t
        .ok(&o, "GET", "/v1/questions?project=proj", json!(null))
        .await;
    assert_eq!(ids_of(&page).len(), 2);

    // One issue is listed whatever its status.
    let page = t
        .ok(
            &o,
            "GET",
            &format!("/v1/questions?issue={closed}"),
            json!(null),
        )
        .await;
    assert_eq!(ids_of(&page), [format!("{closed} Q1")]);

    // Pages cover every row once.
    let mut seen = Vec::new();
    let mut path = "/v1/questions?limit=1".to_string();
    loop {
        let page = t.ok(&o, "GET", &path, json!(null)).await;
        seen.extend(ids_of(&page));
        match page["next_cursor"].as_str() {
            Some(c) => path = format!("/v1/questions?limit=1&cursor={}", c.replace('|', "%7C")),
            None => break,
        }
    }
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_eq!(
        t.err(&o, "GET", "/v1/questions?cursor=nope", json!(null))
            .await,
        "bad_request"
    );
    assert_eq!(
        t.err(&o, "GET", "/v1/questions?issue=ALX-999", json!(null))
            .await,
        "not_found"
    );
    t.stop().await;
}

/// Undo reads the table, so it still works after the revisions are gone.
#[tokio::test]
async fn undo_answer_needs_no_revisions() {
    let t = T::new().await;
    let k = asked(&t).await;
    answer_q1(&t, &k).await;
    let pool = t.running.as_ref().unwrap().state.pool();
    sqlx::query("DELETE FROM revisions WHERE entity_type = 'issue'")
        .execute(&pool)
        .await
        .unwrap();
    let path = format!("/v1/issues/{k}/questions/Q1/undo-answer");
    let v = t.ok(&owner(), "POST", &path, json!({})).await;
    assert_eq!(v["waiting_on"], "owner");
    assert_eq!(v["open_questions"], 1);
    t.stop().await;
}

/// An answer present from the first revision has no recorded author.
#[tokio::test]
async fn undo_refuses_an_answer_written_at_creation() {
    let t = T::new().await;
    let k = t
        .issue("Decide", json!({ "description": ONE_ANSWERED }))
        .await;
    let path = format!("/v1/issues/{k}/questions/Q1/undo-answer");
    let (code, body) = t.call(&owner(), "POST", &path, json!({})).await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["details"]["reason"], "no_answer_record");
    t.stop().await;
}

type Row = (String, String, i64, String);

async fn issue_rows(pool: &sqlx::SqlitePool) -> Vec<Row> {
    sqlx::query_as(
        "SELECT id, description, description_version, updated_at FROM issues ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn count(pool: &sqlx::SqlitePool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// Turns a current database back into one from before the questions table,
/// so the next open runs the migration and the backfill as an upgrade does.
async fn downgrade(pool: &sqlx::SqlitePool) {
    for sql in [
        "DROP TABLE questions",
        "DELETE FROM _sqlx_migrations WHERE version = 9",
    ] {
        sqlx::query(sql).execute(pool).await.unwrap();
    }
}

async fn author_of(
    pool: &sqlx::SqlitePool,
    key: &str,
    number: i64,
) -> (Option<i64>, Option<String>) {
    sqlx::query_as(
        "SELECT q.answered_version, CASE WHEN a.is_owner THEN 'owner' ELSE a.name END FROM questions q
         JOIN issues i ON i.id = q.issue_id
         LEFT JOIN actors a ON a.id = q.answered_by
         WHERE i.key = ? AND q.number = ? ORDER BY q.position LIMIT 1",
    )
    .bind(key)
    .bind(number)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The upgrade fills the table from existing descriptions, finds who wrote
/// each answer in the revisions, and changes nothing else.
#[tokio::test]
async fn backfill_rebuilds_the_table_and_keeps_answer_authors() {
    let mut t = T::new().await;
    let o = owner();
    let a1 = agent("s1");
    let a2 = agent("s2");

    // The owner answers Q1. Q2 stays open.
    let mixed = asked(&t).await;
    t.ok(
        &a1,
        "PATCH",
        &format!("/v1/issues/{mixed}"),
        json!({ "patches": [{ "op": "ask_question", "question": "Name?" }] }),
    )
    .await;
    answer_as(&t, &o, &mixed, "Q1", "b").await;
    // A later agent write to another section must not take over the answer.
    t.ok(
        &a2,
        "PATCH",
        &format!("/v1/issues/{mixed}"),
        json!({ "patches": [{ "op": "upsert_collapsible", "section": "Agent notes", "body": "notes" }] }),
    )
    .await;

    // An agent answered this one, and undo must stay theirs.
    let by_agent = t
        .issue("By agent", json!({ "description": ONE_OPEN }))
        .await;
    answer_as(&t, &a1, &by_agent, "Q1", "lt").await;

    // Answered, taken back, then answered by someone else: the newest wins.
    let again = t.issue("Again", json!({ "description": ONE_OPEN })).await;
    answer_as(&t, &a1, &again, "Q1", "one").await;
    answer_as(&t, &o, &again, "Q1", "").await;
    answer_as(&t, &o, &again, "Q1", "two").await;

    // The answer is in the first revision, so no revision shows its author.
    let at_birth = t
        .issue("Birth", json!({ "description": ONE_ANSWERED }))
        .await;

    // The older revisions are gone, so the answer has no record.
    let trimmed = t.issue("Trimmed", json!({ "description": ONE_OPEN })).await;
    answer_as(&t, &o, &trimmed, "Q1", "lt").await;

    // Closed and trashed issues are filled too.
    let closed = t
        .issue(
            "Closed",
            json!({ "description": ONE_OPEN, "status": "Done" }),
        )
        .await;
    let trashed = t.issue("Trashed", json!({ "description": ONE_OPEN })).await;
    t.ok(&o, "DELETE", &format!("/v1/issues/{trashed}"), json!(null))
        .await;

    // Hand-edited text: a repeated number, a section in another case, two
    // sections, and no section.
    let dup = "+++ Owner questions\n### Q1. First?\n\n**Answer:**\n\n### Q1. Again?\n\n**Answer:** x\n+++\n";
    let dup = t.issue("Dup", json!({ "description": dup })).await;
    let lower = "+++ owner QUESTIONS\n### Q7: Odd case?\n\n**Answer:**\n+++\n";
    let lower = t.issue("Lower", json!({ "description": lower })).await;
    let two = format!("{ONE_OPEN}\n{ONE_OPEN}");
    let two = t.issue("Two sections", json!({ "description": two })).await;
    let plain = t
        .issue(
            "Plain",
            json!({ "description": "Owner questions are fun." }),
        )
        .await;

    let running = t.running.take().unwrap();
    let pool = running.state.pool();
    sqlx::query(
        "DELETE FROM revisions WHERE entity_type = 'issue' AND version = 1
         AND entity_id = (SELECT id FROM issues WHERE key = ?)",
    )
    .bind(&trimmed)
    .execute(&pool)
    .await
    .unwrap();
    // A description with no matching revision must still be read.
    sqlx::query("DELETE FROM revisions WHERE entity_id = (SELECT id FROM issues WHERE key = ?)")
        .bind(&lower)
        .execute(&pool)
        .await
        .unwrap();
    let live: Vec<(String, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT i.key, q.number, q.text, q.answer FROM questions q JOIN issues i ON i.id = q.issue_id
         ORDER BY i.key, q.position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    downgrade(&pool).await;
    let before = issue_rows(&pool).await;
    let revisions = count(&pool, "SELECT count(*) FROM revisions").await;
    let activity = count(&pool, "SELECT count(*) FROM activity").await;
    let sessions = count(&pool, "SELECT count(*) FROM work_sessions").await;
    running.stop().await.unwrap();

    let dir = t._dir.path().to_path_buf();
    let pool = llmtodo_server::db::open(&dir).await.unwrap();

    assert_eq!(
        issue_rows(&pool).await,
        before,
        "the backfill changed an issue"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM revisions").await,
        revisions
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM activity").await,
        activity
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM work_sessions").await,
        sessions
    );
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM settings WHERE key = 'questions_backfill'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pending, 0);

    // The backfill agrees with what the live writes stored.
    let filled: Vec<(String, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT i.key, q.number, q.text, q.answer FROM questions q JOIN issues i ON i.id = q.issue_id
         ORDER BY i.key, q.position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(filled, live);
    assert!(filled.iter().any(|(k, ..)| k == &closed));
    assert!(filled.iter().any(|(k, ..)| k == &trashed));
    assert!(filled.iter().any(|(k, n, ..)| k == &lower && *n == 7));
    assert_eq!(filled.iter().filter(|(k, ..)| k == &dup).count(), 2);
    assert!(!filled.iter().any(|(k, ..)| k == &two || k == &plain));

    let (version, name) = author_of(&pool, &mixed, 1).await;
    assert_eq!(version, Some(4), "the answer is the fourth revision");
    assert_eq!(name.as_deref(), Some("owner"));
    assert_eq!(author_of(&pool, &mixed, 2).await, (None, None));
    let (version, name) = author_of(&pool, &by_agent, 1).await;
    assert_eq!(version, Some(2));
    assert!(name.unwrap().starts_with("claude-code"));
    let (version, name) = author_of(&pool, &again, 1).await;
    assert_eq!(version, Some(4));
    assert_eq!(name.as_deref(), Some("owner"));
    assert_eq!(author_of(&pool, &at_birth, 1).await, (None, None));
    assert_eq!(author_of(&pool, &trimmed, 1).await, (None, None));
    pool.close().await;

    // A second start has nothing to do, and undo works on the filled rows.
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.clone(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    t.url = running.url();
    t.running = Some(running);

    let path = format!("/v1/issues/{by_agent}/questions/Q1/undo-answer");
    let (code, body) = t.call(&o, "POST", &path, json!({})).await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["details"]["reason"], "not_the_answerer");
    let v = t.ok(&a1, "POST", &path, json!({})).await;
    assert_eq!(v["open_questions"], 1);

    let path = format!("/v1/issues/{trimmed}/questions/Q1/undo-answer");
    let (code, body) = t.call(&o, "POST", &path, json!({})).await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["details"]["reason"], "no_answer_record");

    let v = t
        .ok(&o, "GET", &format!("/v1/issues/{mixed}"), json!(null))
        .await;
    assert_eq!(v["open_questions"], 1);
    t.stop().await;
}

const NO_ESTIMATES: fn() -> Value =
    || json!({ "estimate_minutes": null, "estimate_cost_usd": null });

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_needs_both_estimates() {
    let t = T::new().await;
    let k = t.issue("No estimates", NO_ESTIMATES()).await;
    let url = format!("/v1/issues/{k}");
    let claim = format!("{url}/claim");
    let who = agent("gate");

    let (status, v) = t.call(&who, "POST", &claim, json!({})).await;
    assert_eq!(status, 422, "{v}");
    assert_eq!(v["error"]["details"]["reason"], "estimate_missing");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(message.contains("--estimate-minutes"), "{message}");
    assert!(message.contains("--estimate-cost"), "{message}");
    assert!(message.contains(&k), "{message}");

    // One estimate is not enough, and the error names the other one.
    t.ok(&who, "PATCH", &url, json!({ "estimate_minutes": 45 }))
        .await;
    let (_, v) = t.call(&who, "POST", &claim, json!({})).await;
    assert_eq!(
        v["error"]["details"]["missing"],
        json!(["estimate_cost_usd"])
    );
    assert!(t.ok(&owner(), "GET", &url, Value::Null).await["claim"].is_null());

    t.ok(&who, "PATCH", &url, json!({ "estimate_cost_usd": 2.25 }))
        .await;
    let v = t.ok(&who, "POST", &claim, json!({})).await;
    assert_eq!(v["changed"], true);
    let v = t.ok(&owner(), "GET", &url, Value::Null).await;
    assert_eq!(v["estimate_minutes"], 45);
    assert_eq!(v["estimate_cost_usd"], 2.25);
    t.stop().await;
}

/// Tasks claimed before the gate existed have no estimates. Their holder
/// can claim again, and another session can still take them over.
#[tokio::test(flavor = "multi_thread")]
async fn the_estimate_gate_leaves_claimed_tasks_alone() {
    let t = T::new().await;
    let k = t.issue("Old claim", json!({})).await;
    let claim = format!("/v1/issues/{k}/claim");
    t.ok(&agent("first"), "POST", &claim, json!({})).await;
    sqlx::query("UPDATE issues SET estimate_minutes = NULL, estimate_cost_usd = NULL")
        .execute(&t.running.as_ref().unwrap().state.pool())
        .await
        .unwrap();

    let again = t.ok(&agent("first"), "POST", &claim, json!({})).await;
    assert_eq!(again["changed"], false);
    assert_eq!(
        t.err(&agent("second"), "POST", &claim, json!({})).await,
        "claim_held",
        "a claim held by another session stays exit code 4"
    );
    t.ok(&agent("second"), "POST", &claim, json!({ "steal": true }))
        .await;
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_set_estimate_changes_only_with_a_reason() {
    let t = T::new().await;
    let k = t.issue("Frozen", NO_ESTIMATES()).await;
    let url = format!("/v1/issues/{k}");
    let who = agent("frozen");

    // Filling in an empty estimate needs no reason.
    t.ok(
        &who,
        "PATCH",
        &url,
        json!({ "estimate_minutes": 60, "estimate_cost_usd": 3.0 }),
    )
    .await;
    // The same value is not a change.
    t.ok(&who, "PATCH", &url, json!({ "estimate_minutes": 60 }))
        .await;

    for change in [
        json!({ "estimate_minutes": 90 }),
        json!({ "estimate_cost_usd": 4.0 }),
    ] {
        let (status, v) = t.call(&who, "PATCH", &url, change).await;
        assert_eq!(status, 422, "{v}");
        assert_eq!(v["error"]["details"]["reason"], "estimate_frozen");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--estimate-reason")
        );
    }
    let blank = json!({ "estimate_minutes": 90, "estimate_reason": "  " });
    assert_eq!(t.call(&who, "PATCH", &url, blank).await.0, 422);
    let v = t.ok(&owner(), "GET", &url, Value::Null).await;
    assert_eq!(v["estimate_minutes"], 60);
    assert_eq!(v["estimate_cost_usd"], 3.0);

    t.ok(
        &who,
        "PATCH",
        &url,
        json!({ "estimate_minutes": 90, "estimate_reason": "The spec added CSV export." }),
    )
    .await;
    let history = t
        .ok(&owner(), "GET", &format!("{url}/history"), Value::Null)
        .await;
    let events = history["activity"].as_array().unwrap();
    let field = |name: &str| events.iter().rfind(|e| e["field"] == name).unwrap().clone();
    let minutes = field("estimate_minutes");
    assert_eq!((&minutes["old"], &minutes["new"]), (&json!(60), &json!(90)));
    assert_eq!(
        field("estimate_reason")["new"],
        "The spec added CSV export."
    );

    // Bad values and a reason with no estimate are refused.
    for bad in [
        json!({ "estimate_minutes": 0 }),
        json!({ "estimate_cost_usd": -1.0 }),
        json!({ "estimate_reason": "why" }),
    ] {
        assert_eq!(t.call(&who, "PATCH", &url, bad).await.0, 422);
    }
    t.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn active_time_adds_up_across_sessions() {
    let t = T::new().await;
    let a = t.issue("A", json!({})).await;
    let b = t.issue("B", json!({})).await;
    let idle = t.issue("No work", json!({})).await;
    let with = |session: &str, seconds: u64| {
        let mut w = agent(session);
        w.0.transcript_path = Some(format!("/tmp/transcript-{session}.jsonl"));
        w.0.active_seconds = Some(seconds);
        w
    };
    let patch = json!({ "priority": "low" });
    let url = |k: &str| format!("/v1/issues/{k}");

    // Two sessions work on A. A later write adds only the new part.
    t.ok(&with("s1", 100), "PATCH", &url(&a), patch.clone())
        .await;
    t.ok(
        &with("s2", 40),
        "PATCH",
        &url(&a),
        json!({ "priority": "medium" }),
    )
    .await;
    t.ok(
        &with("s1", 130),
        "PATCH",
        &url(&a),
        json!({ "priority": "high" }),
    )
    .await;
    // The same session moves to B, which gets only what is new.
    t.ok(&with("s1", 150), "PATCH", &url(&b), patch.clone())
        .await;

    let v = t.ok(&owner(), "GET", &url(&a), Value::Null).await;
    assert_eq!(v["active_seconds"], 170);
    let by_session = |id: &str| {
        v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["session_id"] == id)
            .unwrap()["active_seconds"]
            .clone()
    };
    assert_eq!(by_session("s1"), 130);
    assert_eq!(by_session("s2"), 40);

    let list = t.ok(&owner(), "POST", "/v1/issues/query", json!({})).await;
    let in_list = |k: &str| {
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["key"] == k)
            .unwrap()
            .clone()
    };
    assert_eq!(in_list(&a)["active_seconds"], 170);
    assert_eq!(in_list(&b)["active_seconds"], 20);
    assert!(
        in_list(&idle).get("active_seconds").is_none(),
        "no time data leaves the field out"
    );

    let usage = t.ok(&owner(), "GET", "/v1/usage", Value::Null).await;
    assert_eq!(usage["total_active_seconds"], 190);
    t.stop().await;
}

/// A session that reported tokens before time tracking existed gets no
/// time for that past work. Only what comes after counts.
#[tokio::test(flavor = "multi_thread")]
async fn time_tracking_invents_nothing_for_past_work() {
    let t = T::new().await;
    let k = t.issue("Older", json!({})).await;
    let url = format!("/v1/issues/{k}");
    let mut w = agent("old");
    w.0.transcript_path = Some("/tmp/transcript-old.jsonl".into());
    w.0.usage = Some(
        [(
            "claude-opus-5-5".to_string(),
            TokenUsage {
                requests: 1,
                input: 10,
                output: 10,
                cache_write_5m: 0,
                cache_write_1h: 0,
                cache_read: 0,
            },
        )]
        .into(),
    );
    t.ok(&w, "PATCH", &url, json!({ "priority": "low" })).await;

    w.0.active_seconds = Some(5_000);
    t.ok(&w, "PATCH", &url, json!({ "priority": "high" })).await;
    let v = t.ok(&owner(), "GET", &url, Value::Null).await;
    assert!(v.get("active_seconds").is_none(), "{v}");

    w.0.active_seconds = Some(5_060);
    t.ok(&w, "PATCH", &url, json!({ "priority": "low" })).await;
    let v = t.ok(&owner(), "GET", &url, Value::Null).await;
    assert_eq!(v["active_seconds"], 60);
    t.stop().await;
}

/// An issue from the API, as the shared filter reads it.
struct Subject(Issue);

impl FilterSubject for Subject {
    fn status_name(&self) -> &str {
        &self.0.status.name
    }
    fn status_type(&self) -> StatusType {
        self.0.status.kind
    }
    fn label_names(&self) -> impl Iterator<Item = &str> {
        self.0.labels.iter().map(String::as_str)
    }
    fn assignee(&self) -> Option<AssigneeRef<'_>> {
        self.0.assignee.as_ref().map(|a| AssigneeRef {
            name: &a.name,
            is_owner: a.is_owner,
        })
    }
    fn claimed(&self) -> bool {
        self.0.claim.as_ref().is_some_and(|c| !c.actor.is_owner)
    }
    fn analyzing(&self) -> bool {
        self.0.analyzing.is_some() && !self.claimed()
    }
}

/// LLM-128: the server's SQL and `IssueFilter::matches` in core keep the
/// same issues, so a saved view and the app's list agree.
#[tokio::test]
async fn sql_and_core_filters_keep_the_same_issues() {
    let t = T::new().await;
    for body in [
        json!({ "name": "Area", "is_group": true }),
        json!({ "name": "app", "group": "Area" }),
        json!({ "name": "bug" }),
    ] {
        t.ok(&owner(), "POST", "/v1/labels", body).await;
    }
    let a = agent("worker");
    let path = |k: &str, p: &str| format!("/v1/issues/{k}{p}");
    t.issue("Plain", json!({})).await;
    t.issue(
        "Owner bug in review",
        json!({ "status": "In Review", "labels": ["bug"], "assignee": "owner",
                "description": "+++ Review\n### Steps to reproduce\n1. Open it.\n\n### What to check\n- It works.\n+++\n" }),
    )
    .await;
    t.issue(
        "App work done",
        json!({ "status": "Done", "labels": ["Area/app"] }),
    )
    .await;
    let locked = t
        .issue("Claimed app bug", json!({ "labels": ["app", "bug"] }))
        .await;
    t.ok(&a, "POST", &path(&locked, "/claim"), json!({})).await;
    let reading = t.issue("Being read", json!({ "status": "Backlog" })).await;
    t.ok(
        &agent("reader"),
        "POST",
        &path(&reading, "/analyze"),
        json!({}),
    )
    .await;

    let page = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "limit": 1000 }),
        )
        .await;
    let all: Vec<Issue> = serde_json::from_value(page["items"].clone()).unwrap();
    assert_eq!(all.len(), 5);
    let filters = [
        json!({}),
        json!({ "statuses": ["in review", "done"] }),
        json!({ "status_types": ["started", "completed"] }),
        json!({ "statuses": ["Done"], "status_types": ["started"] }),
        json!({ "open": true }),
        json!({ "labels": ["app"] }),
        json!({ "labels": ["Area/app", "bug"] }),
        json!({ "assignee": "owner" }),
        json!({ "assignee": "none" }),
        json!({ "assignee": "OPUS" }),
        json!({ "claimed": true }),
        json!({ "analyzing": true }),
        json!({ "claimed": true, "analyzing": true, "open": true }),
    ];
    for f in filters {
        let filter: IssueFilter = serde_json::from_value(f.clone()).unwrap();
        let mut want: Vec<String> = all
            .iter()
            .filter(|i| filter.matches(&Subject((*i).clone())))
            .map(|i| i.key.to_string())
            .collect();
        want.sort();
        let page = t.ok(&owner(), "POST", "/v1/issues/query", f.clone()).await;
        let mut got: Vec<String> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap().to_string())
            .collect();
        got.sort();
        assert_eq!(got, want, "filter {f}");
    }
    // A word that matches no actor is still an error on the server.
    let err = t
        .err(
            &owner(),
            "POST",
            "/v1/issues/query",
            json!({ "assignee": "nobody" }),
        )
        .await;
    assert_eq!(err, "not_found");
    t.stop().await;
}

/// A saved view stored before the filter moved into its own type still
/// reads, because the fields stay at the top level of the query.
#[test]
fn an_old_saved_query_still_reads() {
    let q: IssueQuery = serde_json::from_value(json!({
        "projects": ["app"],
        "statuses": ["Todo"],
        "labels": ["bug"],
        "assignee": "owner",
        "claimed": true,
        "text": "crash"
    }))
    .unwrap();
    assert_eq!(q.filter.statuses, ["Todo"]);
    assert_eq!(q.filter.labels, ["bug"]);
    assert_eq!(q.filter.assignee.as_deref(), Some("owner"));
    assert!(q.filter.claimed);
    assert_eq!(q.text.as_deref(), Some("crash"));
    let back = serde_json::to_value(&q).unwrap();
    assert_eq!(back["statuses"], json!(["Todo"]));
}

/// LLM-180: the team and project lists load statuses, teams and repos in one
/// query each, and each item still matches its own view.
#[tokio::test]
async fn catalog_lists_match_each_view() {
    let t = T::new().await;
    let o = owner();
    t.ok(
        &o,
        "POST",
        "/v1/teams",
        json!({ "key": "BEE", "name": "Bee" }),
    )
    .await;
    t.ok(
        &o,
        "PATCH",
        "/v1/teams/BEE",
        json!({ "pr_open_status": "In Progress" }),
    )
    .await;
    for (name, teams) in [
        ("One", json!(["ALX"])),
        ("Two", json!(["BEE", "ALX"])),
        ("Three", json!([])),
    ] {
        t.ok(
            &o,
            "POST",
            "/v1/projects",
            json!({ "name": name, "teams": teams }),
        )
        .await;
    }
    for (p, repo) in [("one", "o/a"), ("two", "o/c"), ("two", "o/b")] {
        t.ok(
            &o,
            "POST",
            &format!("/v1/projects/{p}/repos"),
            json!({ "repo": repo }),
        )
        .await;
    }

    let teams = t.ok(&o, "GET", "/v1/teams", json!(null)).await;
    assert_eq!(teams.as_array().unwrap().len(), 2);
    for team in teams.as_array().unwrap() {
        let key = team["key"].as_str().unwrap();
        let one = t
            .ok(&o, "GET", &format!("/v1/teams/{key}"), json!(null))
            .await;
        assert_eq!(team, &one);
        assert!(!team["statuses"].as_array().unwrap().is_empty());
    }
    assert_eq!(teams[1]["pr_open_status"], "In Progress");

    let projects = t.ok(&o, "GET", "/v1/projects", json!(null)).await;
    let by_slug = |slug: &str| {
        projects
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["slug"] == slug)
            .unwrap()
            .clone()
    };
    assert_eq!(by_slug("one")["teams"], json!(["ALX"]));
    assert_eq!(by_slug("one")["repos"], json!(["o/a"]));
    assert_eq!(by_slug("two")["teams"], json!(["ALX", "BEE"]));
    assert_eq!(by_slug("two")["repos"], json!(["o/b", "o/c"]));
    assert_eq!(by_slug("three")["teams"], json!([]));
    for p in projects.as_array().unwrap() {
        let slug = p["slug"].as_str().unwrap();
        let one = t
            .ok(&o, "GET", &format!("/v1/projects/{slug}"), json!(null))
            .await;
        assert_eq!(p, &one);
    }

    // The document list sends no body, and a document view still does.
    t.ok(
        &o,
        "POST",
        "/v1/documents",
        json!({ "project": "one", "title": "Notes", "body": "Long text.\n" }),
    )
    .await;
    let docs = t.ok(&o, "GET", "/v1/documents", json!(null)).await;
    assert!(docs[0]["body"].is_null(), "{docs}");
    let doc = t
        .ok(&o, "GET", "/v1/documents/one%2Fnotes", json!(null))
        .await;
    assert_eq!(doc["body"], "Long text.\n");

    // A relation shows the other issue's short form.
    let a = t.issue("Blocked", json!({ "team": "ALX" })).await;
    let b = t.issue("Blocker", json!({ "team": "ALX" })).await;
    t.ok(
        &o,
        "POST",
        &format!("/v1/issues/{b}/relations"),
        json!({ "type": "blocks", "issue": a }),
    )
    .await;
    let d = t
        .ok(&o, "GET", &format!("/v1/issues/{a}"), json!(null))
        .await;
    assert_eq!(d["relations"][0]["direction"], "blocked-by");
    assert_eq!(d["relations"][0]["issue"]["key"], b);
    assert_eq!(d["relations"][0]["issue"]["title"], "Blocker");
    assert_eq!(d["relations"][0]["issue"]["status"]["name"], "Todo");
    t.stop().await;
}

/// The keys of an order view, step by step.
fn order_steps(v: &Value) -> Vec<Vec<String>> {
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
}

fn keys_of(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap().to_string())
        .collect();
    k.sort();
    k
}

/// An epic with six sub-issues, `a` to `f`.
async fn epic(t: &T) -> (String, Vec<String>) {
    let p = t.issue("Epic", json!({})).await;
    let mut kids = Vec::new();
    for name in ["a", "b", "c", "d", "e", "f"] {
        kids.push(t.issue(name, json!({ "parent": p })).await);
    }
    (p, kids)
}

async fn set_order(t: &T, p: &str, steps: Value) -> (u16, Value) {
    t.call(
        &owner(),
        "PUT",
        &format!("/v1/issues/{p}/order"),
        json!({ "steps": steps }),
    )
    .await
}

#[tokio::test]
async fn order_set_show_and_the_frontier_follows_it() {
    let t = T::new().await;
    let (p, k) = epic(&t).await;
    let start = t.ok(&owner(), "GET", "/v1/activity", Value::Null).await["last_seq"]
        .as_i64()
        .unwrap();
    // a, then b and c in any order, then d. e and f have no step.
    let (s, v) = set_order(&t, &p, json!([[k[0]], [k[1], k[2]], [k[3]]])).await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(
        order_steps(&v),
        [
            vec![k[0].clone()],
            vec![k[1].clone(), k[2].clone()],
            vec![k[3].clone()]
        ]
    );
    assert_eq!(keys_of(&v["unordered"]), [k[4].clone(), k[5].clone()]);
    assert_eq!(v["steps"][0]["done"], false);
    let shown = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{p}/order"),
            Value::Null,
        )
        .await;
    assert_eq!(order_steps(&shown), order_steps(&v));

    // One live event, on the parent, for the whole change.
    let events = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/activity?after_seq={start}"),
            Value::Null,
        )
        .await;
    let orders: Vec<&Value> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["field"] == "order")
        .collect();
    assert_eq!(orders.len(), 1, "{events}");
    let relation_events = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["field"] == "relation_added")
        .count();
    assert_eq!(relation_events, 0);

    let view = async |key: &str| {
        t.ok(&owner(), "GET", &format!("/v1/issues/{key}"), Value::Null)
            .await
    };
    // Steps and depth follow the order.
    let d = view(&k[3]).await;
    assert_eq!(d["order_step"], 3);
    assert_eq!(d["depth"], 2);
    assert_eq!(keys_of(&d["open_blockers"]), [k[1].clone(), k[2].clone()]);
    assert_eq!(view(&k[4]).await["order_step"], Value::Null);
    // The order edges show as relations with source `order`, and only
    // between neighbouring steps.
    let b = view(&k[1]).await;
    let rels = b["relations"].as_array().unwrap();
    assert_eq!(rels.len(), 2, "{rels:?}");
    assert!(rels.iter().all(|r| r["source"] == "order"));
    let a = view(&k[0]).await;
    assert!(
        a["relations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["issue"]["key"] != k[3]),
        "no edge skips a step"
    );

    // The frontier offers the first step and the tasks with no step.
    let frontier = async || {
        t.ok(
            &owner(),
            "POST",
            "/v1/issues/frontier",
            json!({ "issue": p }),
        )
        .await
    };
    assert_eq!(
        keys_of(&frontier().await),
        [k[0].clone(), k[4].clone(), k[5].clone()]
    );
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{}", k[0]),
        json!({ "status": "Done" }),
    )
    .await;
    assert_eq!(
        keys_of(&frontier().await),
        [k[1].clone(), k[2].clone(), k[4].clone(), k[5].clone()]
    );
    // A closed task keeps its step, and its step reads as done.
    let shown = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{p}/order"),
            Value::Null,
        )
        .await;
    assert_eq!(shown["steps"][0]["done"], true);
    assert_eq!(order_steps(&shown)[0], [k[0].clone()]);

    // Clearing the order removes every order edge.
    let (s, v) = set_order(&t, &p, json!([])).await;
    assert_eq!(s, 200, "{v}");
    assert!(order_steps(&v).is_empty());
    assert!(
        view(&k[1]).await["relations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(view(&k[3]).await["depth"], 0);
    t.stop().await;
}

#[tokio::test]
async fn order_refuses_foreign_keys_and_repeats() {
    let t = T::new().await;
    let (p, k) = epic(&t).await;
    let other = t.issue("Other", json!({})).await;
    let nephew = t.issue("Nephew", json!({ "parent": other })).await;
    for bad in [other.as_str(), nephew.as_str(), p.as_str(), "ALX-999"] {
        let (s, v) = set_order(&t, &p, json!([[k[0]], [bad]])).await;
        assert_eq!(s, 422, "{bad}: {v}");
        assert_eq!(v["error"]["code"], "validation");
        assert_eq!(v["error"]["details"]["reason"], "not_a_sub_issue");
    }
    let (s, v) = set_order(&t, &p, json!([[k[0]], [k[1], k[0]]])).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["details"]["reason"], "repeated");
    // Nothing was written.
    let shown = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{p}/order"),
            Value::Null,
        )
        .await;
    assert!(order_steps(&shown).is_empty());
    t.stop().await;
}

#[tokio::test]
async fn order_and_manual_edges_never_make_a_cycle() {
    let t = T::new().await;
    let (p, k) = epic(&t).await;
    // d blocks a by hand, so an order with a before d is a cycle.
    t.ok(
        &owner(),
        "POST",
        &format!("/v1/issues/{}/relations", k[3]),
        json!({ "type": "blocks", "issue": k[0] }),
    )
    .await;
    let (s, v) = set_order(&t, &p, json!([[k[0]], [k[1]], [k[3]]])).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["details"]["reason"], "order_cycle");
    let shown = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{p}/order"),
            Value::Null,
        )
        .await;
    assert!(order_steps(&shown).is_empty(), "the write rolled back");

    // The other way round works, and a manual edge against it is refused.
    let (s, v) = set_order(&t, &p, json!([[k[3]], [k[1]], [k[0]]])).await;
    assert_eq!(s, 200, "{v}");
    let code = t
        .err(
            &owner(),
            "POST",
            &format!("/v1/issues/{}/relations", k[0]),
            json!({ "type": "blocks", "issue": k[1] }),
        )
        .await;
    assert_eq!(code, "validation");

    // `relation remove` refuses an order edge and names the way out.
    let (s, v) = t
        .call(
            &owner(),
            "POST",
            &format!("/v1/issues/{}/relations/remove", k[1]),
            json!({ "type": "blocks", "issue": k[0] }),
        )
        .await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["details"]["reason"], "order_edge");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--order-step none"),
        "{v}"
    );

    // A manual relation takes over an order edge, so it stays when the
    // order goes.
    t.ok(
        &owner(),
        "POST",
        &format!("/v1/issues/{}/relations", k[1]),
        json!({ "type": "blocks", "issue": k[0] }),
    )
    .await;
    set_order(&t, &p, json!([])).await;
    let a = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{}", k[0]),
            Value::Null,
        )
        .await;
    let rels: Vec<(&str, &str)> = a["relations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["issue"]["key"].as_str().unwrap(),
                r["source"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(rels.len(), 2, "{rels:?}");
    assert!(rels.contains(&(k[1].as_str(), "manual")));
    assert!(rels.contains(&(k[3].as_str(), "manual")));
    t.stop().await;
}

#[tokio::test]
async fn order_step_moves_one_task_and_trash_rewrites_the_edges() {
    let t = T::new().await;
    let (p, k) = epic(&t).await;
    set_order(&t, &p, json!([[k[0]], [k[1], k[2]], [k[3]]])).await;
    let patch = async |key: &str, body: Value| {
        t.call(&owner(), "PATCH", &format!("/v1/issues/{key}"), body)
            .await
    };
    let steps = async || {
        order_steps(
            &t.ok(
                &owner(),
                "GET",
                &format!("/v1/issues/{p}/order"),
                Value::Null,
            )
            .await,
        )
    };
    let depth = async |key: &str| {
        t.ok(&owner(), "GET", &format!("/v1/issues/{key}"), Value::Null)
            .await["depth"]
            .as_u64()
            .unwrap()
    };

    // e joins step 1, f starts a new last step.
    assert_eq!(patch(&k[4], json!({ "order_step": 1 })).await.0, 200);
    assert_eq!(patch(&k[5], json!({ "order_step": 4 })).await.0, 200);
    assert_eq!(
        steps().await,
        [
            vec![k[0].clone(), k[4].clone()],
            vec![k[1].clone(), k[2].clone()],
            vec![k[3].clone()],
            vec![k[5].clone()],
        ]
    );
    assert_eq!(depth(&k[5]).await, 3);
    // Past the last step plus one is refused.
    let (s, v) = patch(&k[5], json!({ "order_step": 9 })).await;
    assert_eq!(s, 422, "{v}");
    assert_eq!(v["error"]["details"]["reason"], "step_out_of_range");
    // A task with no parent has no order.
    let lone = t.issue("Lone", json!({})).await;
    let (s, v) = patch(&lone, json!({ "order_step": 1 })).await;
    assert_eq!(s, 422, "{v}");

    // d leaves the order: its step closes up, and f follows b and c.
    assert_eq!(patch(&k[3], json!({ "order_step": "" })).await.0, 200);
    assert_eq!(steps().await.len(), 3);
    assert_eq!(depth(&k[5]).await, 2);

    // Trash drops b out of the order, and the edges follow.
    t.ok(
        &owner(),
        "DELETE",
        &format!("/v1/issues/{}", k[1]),
        Value::Null,
    )
    .await;
    assert_eq!(
        steps().await,
        [
            vec![k[0].clone(), k[4].clone()],
            vec![k[2].clone()],
            vec![k[5].clone()],
        ]
    );
    let f = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{}", k[5]),
            Value::Null,
        )
        .await;
    assert_eq!(keys_of(&f["open_blockers"]), [k[2].clone()]);
    // Restore brings b back with no step.
    t.ok(
        &owner(),
        "POST",
        &format!("/v1/issues/{}/restore", k[1]),
        json!({}),
    )
    .await;
    let b = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{}", k[1]),
            Value::Null,
        )
        .await;
    assert_eq!(b["order_step"], Value::Null);
    assert!(b["relations"].as_array().unwrap().is_empty());
    assert_eq!(steps().await.len(), 3);

    // A new parent takes the task out of the old order.
    let other = t.issue("Other epic", json!({})).await;
    assert_eq!(patch(&k[2], json!({ "parent": other })).await.0, 200);
    assert_eq!(
        steps().await,
        [vec![k[0].clone(), k[4].clone()], vec![k[5].clone()]]
    );
    let c = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{}", k[2]),
            Value::Null,
        )
        .await;
    assert_eq!(c["order_step"], Value::Null);
    assert!(c["relations"].as_array().unwrap().is_empty());
    assert_eq!(depth(&k[5]).await, 1);
    t.stop().await;
}
