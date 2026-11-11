//! The Linear import against recorded responses. No network.

use llmtodo_api_types::LinearImportRequest;
use llmtodo_server::error::ApiResult;
use llmtodo_server::import::{Fetch, run};
use serde_json::{Value, json};

struct Recorded {
    issues: Value,
    image: Vec<u8>,
}

fn page(nodes: Value) -> Value {
    json!({ "nodes": nodes, "pageInfo": { "hasNextPage": false, "endCursor": null } })
}

fn workspace() -> Recorded {
    let issue = |id: &str, number: i64, title: &str, extra: Value| {
        let mut v = json!({
            "id": id, "identifier": format!("ALX-{number}"), "number": number, "title": title, "description": "",
            "priority": 0, "createdAt": "2026-03-01T10:00:00.000Z", "updatedAt": "2026-03-02T10:00:00.000Z",
            "archivedAt": null, "completedAt": null, "canceledAt": null,
            "state": { "id": "st-todo" }, "team": { "id": "team-alx" }, "project": { "id": "proj-nb" }, "projectMilestone": null,
            "parent": null, "assignee": null, "creator": { "id": "user-me" },
            "labels": page(json!([])), "relations": page(json!([])), "attachments": page(json!([])), "comments": page(json!([]))
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap());
        v
    };
    let issues = json!([
        issue(
            "i5",
            5,
            "Wayfinder map",
            json!({ "priority": 2, "description": "Map.\n\n+++ Agent notes\nnotes\n+++\n", "assignee": { "id": "user-me" } })
        ),
        issue(
            "i7",
            7,
            "Child task",
            json!({
                "parent": { "id": "i5" }, "state": { "id": "st-progress" }, "assignee": { "id": "user-sam" },
                "labels": page(json!([{ "id": "lab-fe" }, { "id": "lab-triage" }])),
                "projectMilestone": { "id": "ms-beta" },
                "relations": page(json!([{ "id": "rel-1", "type": "blocks", "relatedIssue": { "id": "i9" } }])),
                "attachments": page(json!([{ "id": "att-1", "url": "https://github.com/example/notebook/pull/12", "title": "PR 12", "subtitle": "open", "metadata": { "status": "open" }, "createdAt": "2026-03-02T11:00:00.000Z", "sourceType": "github" }])),
                "comments": page(json!([{ "id": "com-1", "body": "Screenshot: ![s](https://uploads.linear.app/ws/abc/shot.png)", "createdAt": "2026-03-02T12:00:00.000Z", "updatedAt": "2026-03-02T12:00:00.000Z", "user": { "id": "user-sam" }, "parent": null }])),
            })
        ),
        issue(
            "i8",
            8,
            "Archived one",
            json!({ "archivedAt": "2026-03-05T00:00:00.000Z", "state": { "id": "st-done" }, "completedAt": "2026-03-04T00:00:00.000Z" })
        ),
        issue(
            "i9",
            9,
            "Blocked task",
            json!({ "relations": page(json!([{ "id": "rel-2", "type": "similar", "relatedIssue": { "id": "i5" } }])) })
        ),
    ]);
    Recorded {
        issues,
        image: b"\x89PNG fake image".to_vec(),
    }
}

impl Fetch for Recorded {
    async fn graphql(&self, query: &str, _vars: Value) -> ApiResult<Value> {
        let op = query
            .split_whitespace()
            .nth(1)
            .unwrap_or("")
            .split('(')
            .next()
            .unwrap_or("");
        Ok(match op {
            "Meta" => json!({
                "viewer": { "id": "user-me", "name": "Alex", "displayName": "alex", "email": "x@example.com" },
                "teams": { "nodes": [{ "id": "team-alx", "key": "ALX", "name": "Alex", "states": page(json!([
                    { "id": "st-triage", "name": "Triage", "type": "triage", "color": "#ff0000", "position": -1.0 },
                    { "id": "st-todo", "name": "Todo", "type": "unstarted", "color": "#e2e2e2", "position": 1.0 },
                    { "id": "st-progress", "name": "In Progress", "type": "started", "color": "#f2c94c", "position": 2.0 },
                    { "id": "st-done", "name": "Done", "type": "completed", "color": "#5e6ad2", "position": 3.0 },
                ])) }] }
            }),
            "Labels" => json!({ "issueLabels": page(json!([
                { "id": "lab-fe", "name": "fe", "color": "#00ff00", "description": null, "isGroup": false, "parent": { "id": "lab-area" } },
                { "id": "lab-area", "name": "Area", "color": "#000000", "description": null, "isGroup": true, "parent": null },
                { "id": "lab-triage", "name": "needs-triage", "color": "#ff00ff", "description": "Needs a look", "isGroup": false, "parent": null },
            ])) }),
            "Users" => json!({ "users": page(json!([
                { "id": "user-me", "name": "Alex", "displayName": "alex", "email": "x@example.com" },
                { "id": "user-sam", "name": "Sam Doe", "displayName": "sam", "email": "s@example.com" },
            ])) }),
            "Projects" => json!({ "projects": page(json!([{
                "id": "proj-nb", "name": "Notebook", "description": "Notes app", "createdAt": "2026-01-01T00:00:00.000Z",
                "updatedAt": "2026-01-02T00:00:00.000Z", "archivedAt": null, "status": { "type": "started" },
                "teams": page(json!([{ "id": "team-alx" }])),
                "projectMilestones": page(json!([{ "id": "ms-beta", "name": "Beta", "description": "", "targetDate": "2026-12-01", "sortOrder": 1.0 }])),
            }])) }),
            "Documents" => json!({ "documents": page(json!([{
                "id": "doc-1", "title": "Game Rules", "content": "## Scoring\n\nOne point.", "slugId": "abc", "createdAt": "2026-01-03T00:00:00.000Z",
                "updatedAt": "2026-01-03T00:00:00.000Z", "archivedAt": null, "project": { "id": "proj-nb" },
            }])) }),
            "Issues" => json!({ "issues": page(self.issues.clone()) }),
            other => panic!("unexpected query {other}"),
        })
    }

    async fn download(&self, _url: &str) -> ApiResult<(Vec<u8>, Option<String>)> {
        Ok((self.image.clone(), Some("image/png".into())))
    }
}

async fn get(base: &str, path: &str) -> Value {
    reqwest::get(format!("{base}{path}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn import_keeps_keys_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let base = running.url();
    let rec = workspace();
    let req = LinearImportRequest {
        token: "test".into(),
        ..Default::default()
    };

    // A dry run writes nothing.
    let dry = run(
        &running.state,
        &rec,
        &LinearImportRequest {
            dry_run: true,
            ..req.clone()
        },
    )
    .await
    .unwrap();
    assert_eq!(dry.counts["issues"].created, 4);
    assert_eq!(get(&base, "/v1/teams").await.as_array().unwrap().len(), 0);

    let report = run(&running.state, &rec, &req).await.unwrap();
    assert_eq!(report.counts["issues"].created, 4);
    assert_eq!(report.counts["files"].created, 1);
    assert!(report.backup.is_some());

    let child = get(&base, "/v1/issues/ALX-7").await;
    assert_eq!(child["key"], "ALX-7");
    assert_eq!(child["parent"], "ALX-5");
    assert_eq!(child["status"]["name"], "In Progress");
    assert_eq!(child["assignee"]["name"], "sam");
    assert_eq!(child["labels"], json!(["Area/fe", "needs-triage"]));
    assert_eq!(child["milestone"], "Beta");
    assert_eq!(child["project"], "notebook");
    assert_eq!(child["relations"][0]["direction"], "blocks");
    assert_eq!(child["attachments"][0]["kind"], "pull_request");
    let body = child["comments"][0]["body"].as_str().unwrap();
    assert!(
        body.contains("/v1/files/") && !body.contains("uploads.linear.app"),
        "{body}"
    );
    let hash = body
        .split("/v1/files/")
        .nth(1)
        .unwrap()
        .trim_end_matches(')');
    let img = reqwest::get(format!("{base}/v1/files/{hash}"))
        .await
        .unwrap();
    assert_eq!(img.headers()["content-type"], "image/png");

    let map = get(&base, "/v1/issues/ALX-5").await;
    assert_eq!(map["assignee"]["is_owner"], true);
    assert_eq!(map["priority"], "high");
    assert!(
        map["description"]
            .as_str()
            .unwrap()
            .contains("+++ Agent notes")
    );
    assert_eq!(map["created_at"], "2026-03-01T10:00:00Z");
    // "similar" becomes "related".
    assert!(
        map["relations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["direction"] == "related")
    );

    // Archived issues land in the trash.
    let trashed = get(&base, "/v1/issues/ALX-8").await;
    assert!(trashed["deleted_at"].is_string());

    // Triage maps to backlog.
    let team = get(&base, "/v1/teams/ALX").await;
    assert!(
        team["statuses"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "Triage" && s["type"] == "backlog")
    );

    let doc = get(&base, "/v1/documents/notebook%2Fgame-rules").await;
    assert!(doc["body"].as_str().unwrap().contains("One point."));

    // A second run changes nothing.
    let again = run(&running.state, &rec, &req).await.unwrap();
    for (kind, c) in &again.counts {
        assert_eq!(c.created, 0, "{kind} created on the second run");
    }
    assert_eq!(again.counts["issues"].unchanged, 4);

    // New issues continue the numbering.
    let http = reqwest::Client::new();
    let created: Value = http
        .post(format!("{base}/v1/issues"))
        .json(&json!({ "title": "New", "team": "ALX" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(created["key"], "ALX-10");
    running.stop().await.unwrap();
}

async fn send(base: &str, method: &str, path: &str, body: Value, session: Option<&str>) -> Value {
    use base64::Engine;
    let http = reqwest::Client::new();
    let mut req = http
        .request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{base}{path}"),
        )
        .json(&body);
    if let Some(s) = session {
        let ctx = json!({ "actor": { "kind": "agent", "harness": "claude-code", "model": "m", "effort": "e" }, "session_id": s });
        req = req.header(
            "x-llmtodo-context",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(ctx.to_string()),
        );
    }
    req.send().await.unwrap().json().await.unwrap()
}

/// The owner made ALX-1 to ALX-5 here before the import. The import must not
/// write Linear's ALX-5 over the local one.
#[tokio::test]
async fn import_skips_keys_held_by_local_issues() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let base = running.url();
    send(
        &base,
        "POST",
        "/v1/teams",
        json!({ "key": "ALX", "name": "Alex" }),
        None,
    )
    .await;
    for n in 1..=5 {
        send(
            &base,
            "POST",
            "/v1/issues",
            json!({ "title": format!("Local {n}") }),
            None,
        )
        .await;
    }
    let req = LinearImportRequest {
        token: "test".into(),
        ..Default::default()
    };
    let report = run(&running.state, &workspace(), &req).await.unwrap();
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.starts_with("ALX-5:") && p.contains("made here")),
        "{:?}",
        report.problems
    );

    let five = get(&base, "/v1/issues/ALX-5").await;
    assert_eq!(five["title"], "Local 5");
    assert!(five["description"].as_str().unwrap_or_default().is_empty());
    // The other Linear issues still arrive.
    assert_eq!(
        get(&base, "/v1/issues/ALX-9").await["title"],
        "Blocked task"
    );
    running.stop().await.unwrap();
}

/// An agent claims ALX-7 after the import, then the owner re-runs it. The
/// claim and the local assignee stay. An issue nobody touched here still
/// takes Linear's change, and the change shows up in the activity log.
#[tokio::test]
async fn reimport_keeps_local_changes_and_claims() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let base = running.url();
    let req = LinearImportRequest {
        token: "test".into(),
        ..Default::default()
    };
    let mut rec = workspace();
    run(&running.state, &rec, &req).await.unwrap();

    send(
        &base,
        "PATCH",
        "/v1/issues/ALX-7",
        json!({ "estimate_minutes": 30, "estimate_cost_usd": 1.5 }),
        Some("agent-1"),
    )
    .await;
    // ALX-7 came from Linear assigned to sam, so the agent must steal it.
    send(
        &base,
        "POST",
        "/v1/issues/ALX-7/claim",
        json!({ "steal": true }),
        Some("agent-1"),
    )
    .await;
    let claimed = get(&base, "/v1/issues/ALX-7").await;
    let agent = claimed["assignee"]["name"].clone();
    assert_ne!(agent, "sam");

    // Linear changes both issues meanwhile.
    for i in rec.issues.as_array_mut().unwrap() {
        match i["id"].as_str().unwrap() {
            "i7" => {
                i["title"] = json!("Child task, renamed in Linear");
                i["assignee"] = json!({ "id": "user-me" });
            }
            "i9" => i["title"] = json!("Blocked task, renamed in Linear"),
            _ => {}
        }
    }
    let again = run(&running.state, &rec, &req).await.unwrap();
    assert!(
        again
            .problems
            .iter()
            .any(|p| p.starts_with("ALX-7:") && p.contains("changed here")),
        "{:?}",
        again.problems
    );

    let seven = get(&base, "/v1/issues/ALX-7").await;
    assert_eq!(seven["title"], "Child task");
    assert_eq!(seven["assignee"]["name"], agent);
    assert_eq!(seven["status"]["name"], "In Progress");
    assert_eq!(seven["parent"], "ALX-5");

    let nine = get(&base, "/v1/issues/ALX-9").await;
    assert_eq!(nine["title"], "Blocked task, renamed in Linear");
    let history = get(&base, "/v1/issues/ALX-9/history").await;
    let imported: Vec<&Value> = history["activity"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["field"] == "imported")
        .collect();
    assert_eq!(
        imported.len(),
        2,
        "one event for the create and one for the update"
    );
    running.stop().await.unwrap();
}

/// Linear returns ALX-5 with a full first page of comments and relations.
/// The rest come from per-issue queries, and none of them may be dropped.
struct Paged(Recorded);

fn comment(id: &str) -> Value {
    json!({ "id": id, "body": format!("comment {id}"), "createdAt": "2026-03-02T12:00:00.000Z",
        "updatedAt": "2026-03-02T12:00:00.000Z", "user": { "id": "user-sam" }, "parent": null })
}

fn more(nodes: Value, cursor: &str) -> Value {
    json!({ "nodes": nodes, "pageInfo": { "hasNextPage": true, "endCursor": cursor } })
}

impl Fetch for Paged {
    async fn graphql(&self, query: &str, vars: Value) -> ApiResult<Value> {
        let op = query.split_whitespace().nth(1).unwrap_or("");
        let after = vars["after"].as_str().unwrap_or("");
        Ok(match (op.split('(').next().unwrap_or(""), after) {
            ("Issues", _) => {
                let mut issues = self.0.issues.clone();
                let map = &mut issues[0];
                assert_eq!(map["id"], "i5");
                map["comments"] = more(json!([comment("c1"), comment("c2")]), "cc2");
                map["relations"] = more(
                    json!([{ "id": "rel-3", "type": "related", "relatedIssue": { "id": "i7" } }]),
                    "rc1",
                );
                json!({ "issues": page(issues) })
            }
            ("IssueComments", "cc2") => {
                assert_eq!(vars["id"], "i5");
                json!({ "issue": { "comments": more(json!([comment("c3")]), "cc3") } })
            }
            ("IssueComments", "cc3") => {
                json!({ "issue": { "comments": page(json!([comment("c4")])) } })
            }
            ("IssueRelations", "rc1") => json!({ "issue": { "relations": page(json!([
                { "id": "rel-4", "type": "blocks", "relatedIssue": { "id": "i8" } }
            ])) } }),
            (o, "") if o.starts_with("Issue") && o != "Issues" => {
                panic!("{o} read without a cursor")
            }
            _ => self.0.graphql(query, vars).await?,
        })
    }

    async fn download(&self, url: &str) -> ApiResult<(Vec<u8>, Option<String>)> {
        self.0.download(url).await
    }
}

#[tokio::test]
async fn import_reads_every_page_of_nested_lists() {
    let dir = tempfile::tempdir().unwrap();
    let running = llmtodo_server::start(llmtodo_server::Config {
        data_dir: dir.path().to_path_buf(),
        port: 0,
        host: None,
        github_poll: false,
    })
    .await
    .unwrap();
    let base = running.url();
    let req = LinearImportRequest {
        token: "test".into(),
        ..Default::default()
    };
    let report = run(&running.state, &Paged(workspace()), &req)
        .await
        .unwrap();
    assert_eq!(report.counts["comments"].created, 5, "{:?}", report.counts);

    let map = get(&base, "/v1/issues/ALX-5").await;
    let bodies: Vec<&str> = map["comments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert_eq!(
        bodies,
        ["comment c1", "comment c2", "comment c3", "comment c4"]
    );
    let related: Vec<&str> = map["relations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["issue"]["key"].as_str().unwrap_or_default())
        .collect();
    assert!(related.contains(&"ALX-7"), "{related:?}");
    assert!(related.contains(&"ALX-8"), "{related:?}");
    running.stop().await.unwrap();
}
