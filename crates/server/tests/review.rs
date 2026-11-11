//! Review rounds: the review status gate, approve and request changes.

use base64::Engine;
use llmtodo_api_types::*;
use serde_json::{Value, json};

struct T {
    url: String,
    http: reqwest::Client,
    running: Option<llmtodo_server::Running>,
    _dir: tempfile::TempDir,
}

fn owner() -> ClientContext {
    ClientContext {
        session_id: Some("owner-session".into()),
        ..Default::default()
    }
}

fn agent(session: &str) -> ClientContext {
    ClientContext {
        actor: ActorInput {
            kind: ActorKind::Agent,
            name: None,
            harness: Some("claude-code".into()),
            model: Some("opus-5.5".into()),
            effort: Some("high".into()),
        },
        session_id: Some(session.into()),
        ..Default::default()
    }
}

const SECTION: &str =
    "+++ Review\n### Steps to reproduce\n1. Open the app.\n\n### What to check\n- It opens.\n+++\n";

async fn start(dir: &std::path::Path) -> llmtodo_server::Running {
    llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap()
}

impl T {
    async fn new() -> Self {
        let t = Self::bare().await;
        t.ok(
            &owner(),
            "POST",
            "/v1/teams",
            json!({ "key": "ALX", "name": "Alex" }),
        )
        .await;
        t.ok(
            &owner(),
            "POST",
            "/v1/teams/ALX/statuses",
            json!({ "name": "User review", "type": "started", "review": true }),
        )
        .await;
        t
    }

    /// A server with no team yet.
    async fn bare() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let running = start(dir.path()).await;
        T {
            url: running.url(),
            http: reqwest::Client::new(),
            running: Some(running),
            _dir: dir,
        }
    }

    async fn restart(&mut self) {
        self.running.take().unwrap().stop().await.unwrap();
        let running = start(self._dir.path()).await;
        self.url = running.url();
        self.running = Some(running);
    }

    async fn claim_ends(&self, key: &str) -> Vec<Value> {
        let h = self
            .ok(
                &owner(),
                "GET",
                &format!("/v1/issues/{key}/history"),
                json!({}),
            )
            .await;
        h["activity"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["entity_type"] == "claim" && e["field"] == "ended")
            .cloned()
            .collect()
    }

    async fn call(
        &self,
        who: &ClientContext,
        method: &str,
        path: &str,
        body: Value,
    ) -> (u16, Value) {
        let ctx = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(who).unwrap());
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
        (status, res.json().await.unwrap_or(Value::Null))
    }

    async fn ok(&self, who: &ClientContext, method: &str, path: &str, body: Value) -> Value {
        let (s, v) = self.call(who, method, path, body).await;
        assert!(s < 300, "{method} {path} -> {s}: {v}");
        v
    }

    /// The error's `details.reason`, after checking it is a validation error.
    async fn refused(&self, who: &ClientContext, method: &str, path: &str, body: Value) -> Value {
        let (s, v) = self.call(who, method, path, body).await;
        assert!(s >= 400, "{method} {path} should fail: {v}");
        assert_eq!(v["error"]["code"], "validation", "{v}");
        v["error"].clone()
    }

    async fn issue(&self, description: &str) -> String {
        let v = self
            .ok(
                &owner(),
                "POST",
                "/v1/issues",
                json!({ "title": "Task", "description": description,
                        "estimate_minutes": 30, "estimate_cost_usd": 1.5 }),
            )
            .await;
        v["key"].as_str().unwrap().to_string()
    }

    async fn to_review(&self, key: &str) -> Value {
        self.ok(
            &agent("a1"),
            "PATCH",
            &format!("/v1/issues/{key}"),
            json!({ "status": "User review" }),
        )
        .await
    }

    async fn answer(&self, key: &str, verdict: &str, feedback: &str) -> (u16, Value) {
        self.call(
            &owner(),
            "POST",
            &format!("/v1/issues/{key}/review"),
            json!({ "verdict": verdict, "feedback": feedback }),
        )
        .await
    }

    async fn stop(mut self) {
        self.running.take().unwrap().stop().await.unwrap();
    }
}

#[tokio::test]
async fn user_review_needs_a_review_section() {
    let t = T::new().await;
    let k = t.issue("Plain.").await;
    let e = t
        .refused(
            &agent("a1"),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "status": "User review" }),
        )
        .await;
    assert_eq!(e["details"]["reason"], "review_section_missing");
    assert_eq!(
        e["details"]["missing"],
        json!(["Steps to reproduce", "What to check"])
    );
    assert!(e["message"].as_str().unwrap().contains("### What to check"));

    // A half section is refused too.
    let half = "### Steps to reproduce\n1. Open it.\n";
    let e = t
        .refused(
            &agent("a1"),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "status": "User review",
                    "patches": [{ "op": "upsert_collapsible", "section": "Review", "body": half }] }),
        )
        .await;
    assert_eq!(e["details"]["missing"], json!(["What to check"]), "{e}");

    // A new issue cannot start there without one.
    let e = t
        .refused(
            &owner(),
            "POST",
            "/v1/issues",
            json!({ "title": "New", "status": "User review" }),
        )
        .await;
    assert_eq!(e["details"]["reason"], "review_section_missing");

    // One update can add the section and move the issue. It then waits on
    // the owner.
    let body = "### Steps to reproduce\n1. Open the app.\n\n### What to check\n- It opens.\n";
    let v = t
        .ok(
            &agent("a1"),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "status": "User review",
                    "patches": [{ "op": "upsert_collapsible", "section": "Review", "body": body }] }),
        )
        .await;
    assert_eq!(v["status"]["name"], "User review");
    assert_eq!(v["waiting_on"], "owner");
    let r = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{k}/review"),
            json!({}),
        )
        .await;
    assert_eq!(r["section"]["steps"], json!(["Open the app."]));
    assert_eq!(r["section"]["checks"], json!(["It opens."]));
    t.stop().await;
}

#[tokio::test]
async fn approve_without_a_pr_closes_the_ticket() {
    let t = T::new().await;
    let k = t.issue(SECTION).await;
    t.ok(
        &agent("a1"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    t.to_review(&k).await;
    let (s, v) = t.answer(&k, "approved", "").await;
    assert!(s < 300, "{v}");
    assert_eq!(v["status"]["name"], "Done");
    assert_eq!(v["waiting_on"], Value::Null);
    assert_eq!(v["claim"], Value::Null);
    assert_eq!(v["reviews"][0]["round"], 1);
    assert_eq!(v["reviews"][0]["verdict"], "approved");

    let h = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{k}/history"),
            json!({}),
        )
        .await;
    let review = h["activity"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["field"] == "review")
        .unwrap();
    assert_eq!(review["new"]["verdict"], "approved");

    // A closed ticket has no review to answer.
    let (_, v) = t.answer(&k, "approved", "").await;
    assert_eq!(v["error"]["details"]["reason"], "not_in_review");
    t.stop().await;
}

#[tokio::test]
async fn approve_with_an_open_pr_waits_for_an_agent() {
    let t = T::new().await;
    let k = t.issue(SECTION).await;
    t.ok(
        &agent("a1"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    t.ok(
        &agent("a1"),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "pr": "https://github.com/acme/widgets/pull/7" }),
    )
    .await;
    t.to_review(&k).await;
    let (_, v) = t.answer(&k, "approved", "Looks good.").await;
    assert_eq!(v["status"]["name"], "User review");
    assert_eq!(v["waiting_on"], "agent");
    assert_eq!(v["assignee"], Value::Null);
    assert_eq!(v["reviews"][0]["feedback"], "Looks good.");

    // The round has its answer.
    let (_, v) = t.answer(&k, "changes_requested", "No, wait.").await;
    assert_eq!(v["error"]["details"]["reason"], "already_answered");

    t.stop().await;
}

#[tokio::test]
async fn request_changes_hands_the_ticket_back() {
    let t = T::new().await;
    let k = t.issue(SECTION).await;
    t.ok(
        &agent("a1"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    t.to_review(&k).await;
    let (_, v) = t.answer(&k, "changes_requested", "  ").await;
    assert_eq!(v["error"]["details"]["reason"], "feedback_missing");

    let (s, v) = t
        .answer(&k, "changes_requested", "The text is too small.")
        .await;
    assert!(s < 300, "{v}");
    assert_eq!(v["status"]["name"], "Todo");
    assert_eq!(v["waiting_on"], "agent");
    assert_eq!(v["claim"], Value::Null);
    assert_eq!(v["assignee"], Value::Null);

    // Round two: the agent fixes it and sends it back.
    t.ok(
        &agent("a2"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    let v = t.to_review(&k).await;
    assert_eq!(v["waiting_on"], "owner");
    let (_, v) = t.answer(&k, "approved", "").await;
    assert_eq!(v["status"]["name"], "Done");
    let rounds: Vec<(i64, String)> = v["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["round"].as_i64().unwrap(),
                r["verdict"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        rounds,
        [
            (1, "changes_requested".to_string()),
            (2, "approved".to_string())
        ]
    );
    t.stop().await;
}

#[tokio::test]
async fn user_review_ends_the_claim() {
    let t = T::new().await;
    let k = t.issue(SECTION).await;
    let c = t
        .ok(
            &agent("a1"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({}),
        )
        .await;
    assert!(
        c["issue"]["claim"].is_object() || c["claim"].is_object(),
        "{c}"
    );
    let v = t.to_review(&k).await;
    assert_eq!(v["status"]["name"], "User review");
    assert_eq!(v["claim"], Value::Null, "{v}");
    let h = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{k}/history"),
            json!({}),
        )
        .await;
    let ended = h["activity"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entity_type"] == "claim" && e["field"] == "ended")
        .unwrap();
    assert_eq!(ended["new"], "review");

    // The release path ends it the same way.
    let k = t.issue(SECTION).await;
    t.ok(
        &agent("a2"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    let v = t
        .ok(
            &agent("a2"),
            "POST",
            &format!("/v1/issues/{k}/release"),
            json!({ "status": "User review" }),
        )
        .await;
    assert_eq!(v["claim"], Value::Null, "{v}");
    assert_eq!(v["status"]["name"], "User review");
    t.stop().await;
}

#[tokio::test]
async fn claim_skips_user_review_when_it_comes_first() {
    let t = T::new().await;
    let id = t.ok(&owner(), "GET", "/v1/teams", json!({})).await[0]["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "User review")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/statuses/{id}"),
        json!({ "position": -1 }),
    )
    .await;
    let k = t.issue("").await;
    let c = t
        .ok(
            &agent("a1"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({}),
        )
        .await;
    let status = c["issue"]["status"]["name"]
        .as_str()
        .or(c["status"]["name"].as_str());
    assert_eq!(status, Some("In Progress"), "{c}");
    t.stop().await;
}

/// LLM-151: the review flag decides, not the name. A new team's In Review
/// has the flag, so a move there ends the claim and needs the section.
#[tokio::test]
async fn in_review_is_a_review_status() {
    let t = T::new().await;
    let k = t.issue(SECTION).await;
    t.ok(
        &agent("a1"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    let v = t
        .ok(
            &agent("a1"),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "status": "In Review" }),
        )
        .await;
    assert_eq!(v["status"]["review"], true, "{v}");
    assert_eq!(v["claim"], Value::Null, "{v}");
    assert_eq!(v["waiting_on"], "owner", "{v}");
    let (s, _) = t.answer(&k, "approved", "").await;
    assert!(s < 300);

    let plain = t.issue("Plain.").await;
    let e = t
        .refused(
            &agent("a1"),
            "PATCH",
            &format!("/v1/issues/{plain}"),
            json!({ "status": "In Review" }),
        )
        .await;
    assert_eq!(e["details"]["reason"], "review_section_missing");
    t.stop().await;
}

/// A status without the flag is plain work, whatever its name. Setting the
/// flag ends the claims on its issues, and the frontier skips them.
#[tokio::test]
async fn the_flag_can_be_set_and_cleared() {
    let t = T::new().await;
    let qa = t
        .ok(
            &owner(),
            "POST",
            "/v1/teams/ALX/statuses",
            json!({ "name": "QA", "type": "started" }),
        )
        .await;
    let qa_id = qa["id"].as_str().unwrap().to_string();
    let map = t.issue("").await;
    let v = t
        .ok(
            &owner(),
            "POST",
            "/v1/issues",
            json!({ "title": "Task", "parent": map,
                    "estimate_minutes": 30, "estimate_cost_usd": 1.5 }),
        )
        .await;
    let k = v["key"].as_str().unwrap().to_string();
    t.ok(
        &agent("a1"),
        "POST",
        &format!("/v1/issues/{k}/claim"),
        json!({}),
    )
    .await;
    let v = t
        .ok(
            &agent("a1"),
            "PATCH",
            &format!("/v1/issues/{k}"),
            json!({ "status": "QA" }),
        )
        .await;
    assert!(v["claim"].is_object(), "QA has no flag yet: {v}");

    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/statuses/{qa_id}"),
        json!({ "review": true }),
    )
    .await;
    let v = t
        .ok(&owner(), "GET", &format!("/v1/issues/{k}"), json!({}))
        .await;
    assert_eq!(v["claim"], Value::Null, "{v}");
    assert_eq!(v["status"]["review"], true);
    let ends = t.claim_ends(&k).await;
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["new"], "review");

    // Unassigned and in a review status: not on the frontier.
    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/issues/{k}"),
        json!({ "assignee": "" }),
    )
    .await;
    let frontier = async || {
        t.ok(
            &owner(),
            "POST",
            "/v1/issues/frontier",
            json!({ "issue": map }),
        )
        .await
        .to_string()
    };
    let f = frontier().await;
    assert!(!f.contains(&format!("\"{k}\"")), "{f}");

    t.ok(
        &owner(),
        "PATCH",
        &format!("/v1/statuses/{qa_id}"),
        json!({ "review": false }),
    )
    .await;
    let v = t
        .ok(&owner(), "GET", &format!("/v1/issues/{k}"), json!({}))
        .await;
    assert_eq!(v["status"]["review"], false);
    let f = frontier().await;
    assert!(f.contains(&format!("\"{k}\"")), "{f}");
    t.stop().await;
}

/// A claim on a ticket in a review status takes it back into work, so the
/// ticket is never locked while it waits for review.
#[tokio::test]
async fn a_claim_moves_a_review_ticket_back_to_work() {
    let t = T::new().await;
    let k = t.issue(SECTION).await;
    t.to_review(&k).await;
    let c = t
        .ok(
            &agent("a2"),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({}),
        )
        .await;
    let status = c["issue"]["status"]["name"]
        .as_str()
        .or(c["status"]["name"].as_str());
    assert_eq!(status, Some("In Progress"), "{c}");
    t.stop().await;
}

/// Migration 0012 on data from before it: issues sat claimed in In Review
/// and User review. The next start sets the flag in every team and ends
/// those claims as the migration actor, once.
#[tokio::test]
async fn the_migration_ends_claims_on_review_tickets() {
    let mut t = T::bare().await;
    for (key, name) in [("ASTRO", "Astro starter"), ("LLM", "llmtodo")] {
        t.ok(
            &owner(),
            "POST",
            "/v1/teams",
            json!({ "key": key, "name": name }),
        )
        .await;
    }
    t.ok(
        &owner(),
        "POST",
        "/v1/teams/LLM/statuses",
        json!({ "name": "User review", "type": "started" }),
    )
    .await;
    let mut keys = Vec::new();
    for (team, session) in [
        ("ASTRO", "s1"),
        ("ASTRO", "s2"),
        ("LLM", "s3"),
        ("LLM", "s4"),
    ] {
        let v = t
            .ok(
                &owner(),
                "POST",
                "/v1/issues",
                json!({ "team": team, "title": "Task", "description": SECTION,
                        "estimate_minutes": 30, "estimate_cost_usd": 1.5 }),
            )
            .await;
        let k = v["key"].as_str().unwrap().to_string();
        t.ok(
            &agent(session),
            "POST",
            &format!("/v1/issues/{k}/claim"),
            json!({}),
        )
        .await;
        keys.push(k);
    }
    t.running.take().unwrap().stop().await.unwrap();

    // Put the database back to schema 11, as the old build left it: issues
    // in review statuses keep their claims, and statuses have no flag.
    {
        let path = llmtodo_server::db::db_path(t._dir.path());
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        let moves = [
            (&keys[0], "ASTRO", "In Review"),
            (&keys[2], "LLM", "User review"),
        ];
        for (k, team, status) in moves {
            let number: i64 = k.rsplit('-').next().unwrap().parse().unwrap();
            sqlx::query(
                "UPDATE issues SET status_id = (SELECT s.id FROM statuses s JOIN teams tm ON tm.id = s.team_id WHERE tm.key = ? AND s.name = ?)
                 WHERE number = ? AND team_id = (SELECT id FROM teams WHERE key = ?)",
            )
            .bind(team)
            .bind(status)
            .bind(number)
            .bind(team)
            .execute(&pool)
            .await
            .unwrap();
        }
        for sql in [
            "ALTER TABLE statuses DROP COLUMN review",
            "DELETE FROM _sqlx_migrations WHERE version = 12",
            "DELETE FROM settings WHERE key = 'review_claims_sweep'",
        ] {
            sqlx::query(sql).execute(&pool).await.unwrap();
        }
        pool.close().await;
    }

    t.running = Some(start(t._dir.path()).await);
    t.url = t.running.as_ref().unwrap().url();
    for team in ["ASTRO", "LLM"] {
        let v = t
            .ok(&owner(), "GET", &format!("/v1/teams/{team}"), json!({}))
            .await;
        for s in v["statuses"].as_array().unwrap() {
            let review = matches!(s["name"].as_str(), Some("In Review" | "User review"));
            assert_eq!(s["review"], review, "{team}: {s}");
        }
    }
    for (i, k) in keys.iter().enumerate() {
        let v = t
            .ok(&owner(), "GET", &format!("/v1/issues/{k}"), json!({}))
            .await;
        let ends = t.claim_ends(k).await;
        if i == 0 || i == 2 {
            assert_eq!(v["claim"], Value::Null, "{k}: {v}");
            assert_eq!(ends.len(), 1, "{k}");
            assert_eq!(ends[0]["new"], "review");
            assert_eq!(ends[0]["actor"]["name"], "migration", "{}", ends[0]);
        } else {
            assert!(v["claim"].is_object(), "{k} is in work: {v}");
            assert!(ends.is_empty());
        }
    }

    // The sweep runs once. A later start leaves new claims alone.
    t.restart().await;
    let v = t
        .ok(
            &owner(),
            "GET",
            &format!("/v1/issues/{}", keys[1]),
            json!({}),
        )
        .await;
    assert!(v["claim"].is_object());
    t.stop().await;
}
