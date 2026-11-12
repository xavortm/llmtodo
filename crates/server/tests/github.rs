//! GitHub sync against recorded responses. No network.

use std::sync::atomic::{AtomicU32, Ordering};

use base64::Engine;
use llmtodo_server::error::ApiResult;
use llmtodo_server::github::{GithubApi, Response, poll};
use serde_json::{Value, json};

struct Recorded {
    calls: AtomicU32,
}

impl GithubApi for Recorded {
    async fn get(&self, path: &str, etag: Option<&str>) -> ApiResult<Response> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if path.contains("/pulls") {
            if etag == Some("\"v1\"") {
                return Ok(Response {
                    status: 304,
                    etag: None,
                    body: Value::Null,
                    rate_reset: None,
                    next: None,
                });
            }
            let pr = |n: i64, branch: &str, title: &str, state: &str, draft: bool, merged: bool| {
                json!({ "number": n, "title": title, "state": state, "draft": draft, "merged_at": if merged { json!("2026-11-10T08:00:00Z") } else { Value::Null },
                        "head": { "ref": branch, "repo": { "full_name": "o/r" } }, "base": { "repo": { "full_name": "o/r", "default_branch": "main" } }, "html_url": format!("https://github.com/o/r/pull/{n}"), "updated_at": "2026-11-10T08:00:00Z" })
            };
            let body = json!([
                pr(1, "feature/login", "Login page", "open", false, false),
                pr(2, "alx-2-thing", "Draft work", "open", true, false),
                pr(3, "fix", "ALX-3 fix", "closed", false, true),
                pr(4, "x", "Finish ALX-2", "closed", false, true),
            ]);
            return Ok(Response {
                status: 200,
                etag: Some("\"v1\"".into()),
                body,
                rate_reset: None,
                next: None,
            });
        }
        let commits = json!([{ "sha": "abc123", "html_url": "https://github.com/o/r/commit/abc123", "commit": { "message": "feat: ALX-1 part one\n\nmore" } }]);
        Ok(Response {
            status: 200,
            etag: None,
            body: commits,
            rate_reset: None,
            next: None,
        })
    }
}

async fn call(base: &str, method: &str, path: &str, body: Value, session: Option<&str>) -> Value {
    call_on(base, method, path, body, session, "feature/login").await
}

async fn call_on(
    base: &str,
    method: &str,
    path: &str,
    body: Value,
    session: Option<&str>,
    branch: &str,
) -> Value {
    let http = reqwest::Client::new();
    let mut req = http
        .request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{base}{path}"),
        )
        .json(&body);
    if let Some(s) = session {
        let ctx = json!({ "actor": { "kind": "agent", "harness": "claude-code", "model": "m", "effort": "e" }, "session_id": s, "branch": branch });
        req = req.header(
            "x-llmtodo-context",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(ctx.to_string()),
        );
    }
    let res = req.send().await.unwrap();
    let v: Value = res.json().await.unwrap();
    v
}

#[tokio::test]
async fn pull_requests_attach_and_move_issues() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let b = running.url();
    call(
        &b,
        "POST",
        "/v1/teams",
        json!({ "key": "ALX", "name": "A" }),
        None,
    )
    .await;
    call(&b, "POST", "/v1/projects", json!({ "name": "Quiz" }), None).await;
    call(
        &b,
        "POST",
        "/v1/projects/quiz/repos",
        json!({ "repo": "o/r" }),
        None,
    )
    .await;
    // In Review is a review status, so the PR move needs a review section.
    let section =
        "+++ Review\n### Steps to reproduce\n1. Log in.\n\n### What to check\n- It works.\n+++\n";
    for (t, d) in [("Login", section), ("Thing", ""), ("Done one", "")] {
        call(
            &b,
            "POST",
            "/v1/issues",
            json!({ "title": t, "description": d, "estimate_minutes": 30, "estimate_cost_usd": 1.5 }),
            None,
        )
        .await;
    }
    call(
        &b,
        "PATCH",
        "/v1/issues/ALX-3",
        json!({ "status": "Done" }),
        None,
    )
    .await;
    call(&b, "POST", "/v1/issues/ALX-1/claim", json!({}), Some("s1")).await;

    let api = Recorded {
        calls: AtomicU32::new(0),
    };
    let r = poll(&running.state, &api).await.unwrap();
    assert_eq!(r.pull_requests_seen, 4);
    assert_eq!(
        r.moves,
        ["ALX-1: In Progress -> In Review", "ALX-2: Todo -> Done"]
    );

    let one = call(&b, "GET", "/v1/issues/ALX-1", Value::Null, None).await;
    let kinds: Vec<&str> = one["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["pull_request", "commit"]);
    assert_eq!(one["attachments"][0]["metadata"]["state"], "open");
    let two = call(&b, "GET", "/v1/issues/ALX-2", Value::Null, None).await;
    assert_eq!(
        two["attachments"].as_array().unwrap().len(),
        2,
        "draft and merged PRs both attach"
    );
    let three = call(&b, "GET", "/v1/issues/ALX-3", Value::Null, None).await;
    assert_eq!(
        three["status"]["name"], "Done",
        "a finished issue never moves"
    );
    assert_eq!(three["attachments"].as_array().unwrap().len(), 1);
    let history = call(&b, "GET", "/v1/issues/ALX-2/history", Value::Null, None).await;
    let moved = history["activity"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["field"] == "status")
        .unwrap();
    assert_eq!(moved["actor"]["name"], "github");

    // The next poll sends the ETag and gets 304, so nothing changes.
    let dirty = || async {
        llmtodo_server::db::get_setting(&running.state.pool(), "sync_dirty")
            .await
            .unwrap()
    };
    let before = dirty().await;
    let r = poll(&running.state, &api).await.unwrap();
    assert_eq!(r.pull_requests_seen, 0);
    assert!(r.moves.is_empty());
    // LLM-181: a poll that changed nothing adds no unpushed write.
    assert_eq!(dirty().await, before);
    running.stop().await.unwrap();
}

/// PRs whose head branch is `main`, comes from a fork, or matches only an
/// ended claim. None of them belongs to the claimed issues.
struct Strangers;

impl GithubApi for Strangers {
    async fn get(&self, path: &str, _etag: Option<&str>) -> ApiResult<Response> {
        if !path.contains("/pulls") {
            return Ok(Response {
                status: 200,
                etag: None,
                body: json!([]),
                rate_reset: None,
                next: None,
            });
        }
        let pr = |n: i64, branch: &str, head_repo: &str| {
            json!({ "number": n, "title": "Unrelated", "state": "closed", "draft": false, "merged_at": "2026-11-10T08:00:00Z",
                    "head": { "ref": branch, "repo": { "full_name": head_repo } }, "base": { "repo": { "full_name": "o/r", "default_branch": "main" } },
                    "html_url": format!("https://github.com/o/r/pull/{n}"), "updated_at": "2026-11-10T08:00:00Z" })
        };
        let body = json!([
            pr(10, "main", "stranger/r"),
            pr(11, "fix-tests", "o/r"),
            pr(12, "feature/x", "stranger/r")
        ]);
        Ok(Response {
            status: 200,
            etag: None,
            body,
            rate_reset: None,
            next: None,
        })
    }
}

#[tokio::test]
async fn pull_requests_skip_shared_branches_forks_and_ended_claims() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let b = running.url();
    call(
        &b,
        "POST",
        "/v1/teams",
        json!({ "key": "ALX", "name": "A" }),
        None,
    )
    .await;
    call(&b, "POST", "/v1/projects", json!({ "name": "Quiz" }), None).await;
    call(
        &b,
        "POST",
        "/v1/projects/quiz/repos",
        json!({ "repo": "o/r" }),
        None,
    )
    .await;
    for t in ["On main", "Ended", "Forked"] {
        call(
            &b,
            "POST",
            "/v1/issues",
            json!({ "title": t, "estimate_minutes": 30, "estimate_cost_usd": 1.5 }),
            None,
        )
        .await;
    }
    // A live claim on main, an ended claim on fix-tests, a live claim on a
    // branch that only a fork's PR uses.
    call_on(
        &b,
        "POST",
        "/v1/issues/ALX-1/claim",
        json!({}),
        Some("s1"),
        "main",
    )
    .await;
    call_on(
        &b,
        "POST",
        "/v1/issues/ALX-2/claim",
        json!({}),
        Some("s2"),
        "fix-tests",
    )
    .await;
    call_on(
        &b,
        "POST",
        "/v1/issues/ALX-2/release",
        json!({}),
        Some("s2"),
        "fix-tests",
    )
    .await;
    call_on(
        &b,
        "POST",
        "/v1/issues/ALX-3/claim",
        json!({}),
        Some("s3"),
        "feature/x",
    )
    .await;

    let r = poll(&running.state, &Strangers).await.unwrap();
    assert_eq!(r.pull_requests_seen, 3);
    assert_eq!(r.attached, 0);
    assert!(r.moves.is_empty(), "{:?}", r.moves);
    for (key, status) in [
        ("ALX-1", "In Progress"),
        ("ALX-2", "Todo"),
        ("ALX-3", "In Progress"),
    ] {
        let v = call(&b, "GET", &format!("/v1/issues/{key}"), Value::Null, None).await;
        assert_eq!(v["status"]["name"], status, "{key}");
        assert!(v["attachments"].as_array().unwrap().is_empty(), "{key}");
    }
    running.stop().await.unwrap();
}

/// 70 PRs changed while the server was off. GitHub returns them in pages of
/// 50, and the oldest one names ALX-1. The first poll hits the rate limit on
/// page 2, so the next poll must read page 2 again.
struct TwoPages {
    limit_page_two: std::sync::atomic::AtomicBool,
}

impl GithubApi for TwoPages {
    async fn get(&self, path: &str, etag: Option<&str>) -> ApiResult<Response> {
        if !path.contains("/pulls") {
            return Ok(Response {
                status: 200,
                etag: None,
                body: json!([]),
                rate_reset: None,
                next: None,
            });
        }
        let pr = |n: i64| {
            let title = if n == 1 {
                "ALX-1 old fix".to_string()
            } else {
                format!("Change {n}")
            };
            json!({ "number": n, "title": title, "state": "closed", "draft": false, "merged_at": "2026-11-03T08:00:00Z",
                    "head": { "ref": format!("b{n}"), "repo": { "full_name": "o/r" } }, "base": { "repo": { "full_name": "o/r", "default_branch": "main" } },
                    "html_url": format!("https://github.com/o/r/pull/{n}"), "updated_at": format!("2026-11-{:02}T08:00:00Z", 3 + n / 10) })
        };
        if path.contains("page=2") {
            if self.limit_page_two.swap(false, Ordering::SeqCst) {
                return Ok(Response {
                    status: 403,
                    etag: None,
                    body: Value::Null,
                    rate_reset: Some(60),
                    next: None,
                });
            }
            let body: Vec<Value> = (1..=20).rev().map(pr).collect();
            return Ok(Response {
                status: 200,
                etag: None,
                body: json!(body),
                rate_reset: None,
                next: None,
            });
        }
        if etag == Some("\"p1\"") {
            return Ok(Response {
                status: 304,
                etag: None,
                body: Value::Null,
                rate_reset: None,
                next: None,
            });
        }
        let body: Vec<Value> = (21..=70).rev().map(pr).collect();
        Ok(Response {
            status: 200,
            etag: Some("\"p1\"".into()),
            body: json!(body),
            rate_reset: None,
            next: Some("/repos/o/r/pulls?state=all&page=2".into()),
        })
    }
}

#[tokio::test]
async fn poll_reads_every_page_before_moving_the_poll_time() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let b = running.url();
    call(
        &b,
        "POST",
        "/v1/teams",
        json!({ "key": "ALX", "name": "A" }),
        None,
    )
    .await;
    call(&b, "POST", "/v1/projects", json!({ "name": "Quiz" }), None).await;
    call(
        &b,
        "POST",
        "/v1/projects/quiz/repos",
        json!({ "repo": "o/r" }),
        None,
    )
    .await;
    call(
        &b,
        "POST",
        "/v1/issues",
        json!({ "title": "Old work" }),
        None,
    )
    .await;

    let api = TwoPages {
        limit_page_two: std::sync::atomic::AtomicBool::new(true),
    };
    let r = poll(&running.state, &api).await.unwrap();
    assert!(r.paused.is_some(), "page 2 hit the rate limit");
    let one = call(&b, "GET", "/v1/issues/ALX-1", Value::Null, None).await;
    assert_eq!(one["status"]["name"], "Todo");

    // The next poll walks both pages again and reaches the oldest PR.
    let r = poll(&running.state, &api).await.unwrap();
    assert!(r.paused.is_none(), "{:?}", r.paused);
    assert_eq!(r.pull_requests_seen, 70);
    assert_eq!(r.moves, ["ALX-1: Todo -> Done"]);

    // Now the walk finished, so the ETag is kept and nothing changes.
    let r = poll(&running.state, &api).await.unwrap();
    assert_eq!(r.pull_requests_seen, 0);
    running.stop().await.unwrap();
}
